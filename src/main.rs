mod bench;
mod commands;
mod db;
mod legacy;
mod media;
mod model;
mod state;
mod uploads;
mod voice;
mod ws;

use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use state::{AppState, Config, POLICY_UPDATED_AT, POLICY_VERSION};

fn env_path(var: &str, default: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| default.into()))
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let db_path = env_path("CONCORD_DB", "concord.db");
    // Formato antigo: importado uma única vez, se o banco estiver vazio.
    let legacy_path = env_path("CONCORD_DATA", "concord-data.json");

    // Subcomandos: `concord backup <arquivo>` e `concord bench [mensagens]`.
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("backup") => {
            let Some(dest) = args.get(2) else {
                eprintln!("Uso: concord backup <arquivo-de-destino.db>");
                std::process::exit(2);
            };
            match db::Db::open(&db_path).and_then(|d| d.backup_to(std::path::Path::new(dest))) {
                Ok(()) => println!("Backup consistente salvo em {dest}"),
                Err(e) => {
                    eprintln!("Erro no backup: {e}");
                    std::process::exit(1);
                }
            }
            return;
        }
        Some("bench") => {
            let n = args
                .get(2)
                .and_then(|s| s.parse().ok())
                .unwrap_or(1_000_000);
            bench::run(n);
            return;
        }
        _ => {}
    }

    let config = Config::from_env();
    if config.controller.is_empty() || config.contact.is_empty() {
        eprintln!(
            "AVISO (LGPD): defina CONCORD_CONTROLADOR e CONCORD_CONTATO para identificar \
             o controlador e o canal de atendimento ao titular na política de privacidade."
        );
    }
    if config.admins.is_empty() {
        eprintln!(
            "AVISO: nenhum administrador definido (CONCORD_ADMINS). O servidor público \"Concord\" \
             ficará sem moderação. Seu id está em \"Baixar meus dados\" → id_pseudonimo."
        );
    } else {
        println!(
            "{} administrador(es) da instância configurado(s)",
            config.admins.len()
        );
    }
    let policy_page: &'static str = Box::leak(render_policy(&config).into_boxed_str());

    let livekit = voice::LiveKit::start();
    let _ = CSP.set(build_csp(
        livekit
            .as_ref()
            .map(|l| l.csp_sources())
            .unwrap_or_default(),
    ));

    let state = match AppState::open_with_voice(&db_path, &legacy_path, config, livekit) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("Banco de dados: {}", db_path.display());

    // Retenção: apaga o que passou do prazo, na inicialização e a cada hora.
    // Roda numa thread separada para não atrasar o chat.
    let janitor = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let j = janitor.clone();
            let _ = tokio::task::spawn_blocking(move || j.purge_expired()).await;
        }
    });

    // O cliente web vai embutido no binário.
    let app = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../static/index.html")) }),
        )
        // O código é lido pelo cliente; o servidor só entrega a mesma página.
        .route(
            "/convite/:code",
            get(|| async { Html(include_str!("../static/index.html")) }),
        )
        .route(
            "/privacidade",
            get(move || async move { Html(policy_page) }),
        )
        .route(
            "/style.css",
            get(|| async { asset("text/css", include_str!("../static/style.css")) }),
        )
        .route(
            "/app.js",
            get(|| async { asset("application/javascript", include_str!("../static/app.js")) }),
        )
        .route(
            "/voice.js",
            get(|| async { asset("application/javascript", include_str!("../static/voice.js")) }),
        )
        .route(
            "/vendor/livekit-client.umd.js",
            get(|| async {
                asset(
                    "application/javascript",
                    include_str!("../static/vendor/livekit-client.umd.js"),
                )
            }),
        )
        // Imagens do chat: envio com autorização de uso único e entrega com link assinado.
        .route(
            "/anexos/:id",
            get(uploads::serve)
                .post(uploads::upload)
                .layer(DefaultBodyLimit::max(
                    state::MAX_UPLOAD_BYTES as usize + 1024,
                )),
        )
        .route(
            "/sons/:id",
            get(uploads::serve_sound)
                .post(uploads::upload_sound)
                .layer(DefaultBodyLimit::max(
                    state::MAX_SOUND_BYTES as usize + 1024,
                )),
        )
        .route(
            "/sounds.js",
            get(|| async {
                asset(
                    "application/javascript",
                    include_str!("../static/sounds.js"),
                )
            }),
        )
        .route(
            "/chat.js",
            get(|| async { asset("application/javascript", include_str!("../static/chat.js")) }),
        )
        .route(
            "/favicon.svg",
            get(|| async { asset("image/svg+xml", include_str!("../static/favicon.svg")) }),
        )
        .route(
            "/favicon.ico",
            get(|| async { asset("image/svg+xml", include_str!("../static/favicon.svg")) }),
        )
        .route("/ws", get(ws::ws_handler))
        .layer(middleware::from_fn(security_headers))
        .with_state(state.clone());

    // Em servidor, use CONCORD_HOST=127.0.0.1: só o proxy HTTPS (Caddy) acessa o Concord.
    let host = std::env::var("CONCORD_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .unwrap_or_else(|e| panic!("Não foi possível usar {host}:{port}: {e}"));
    println!("Concord rodando em http://{host}:{port}");

    tokio::select! {
        result = axum::serve(listener, app).into_future() => {
            if let Err(e) = result {
                eprintln!("Erro no servidor: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => println!("\nEncerrando…"),
    }

    // Tudo já está gravado (cada alteração é uma transação). Só otimiza o banco.
    state.db.optimize();
    if let Some(lk) = &state.voice {
        lk.shutdown();
    }
}

static CSP: std::sync::OnceLock<HeaderValue> = std::sync::OnceLock::new();

/// CSP restritiva; a única origem externa liberada é a do LiveKit (voz).
fn build_csp(livekit_sources: String) -> HeaderValue {
    let csp = format!(
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; \
         media-src 'self' blob: data:; connect-src 'self' {livekit_sources}; object-src 'none'; \
         base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
    );
    HeaderValue::from_str(&csp).unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'"))
}

fn asset(mime: &'static str, body: &'static str) -> Response {
    let ct = format!("{mime}; charset=utf-8");
    ([(header::CONTENT_TYPE, ct)], body).into_response()
}

/// Cabeçalhos de segurança em todas as respostas.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        CSP.get()
            .cloned()
            .unwrap_or_else(|| build_csp(String::new())),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(
        "permissions-policy",
        // Microfone, câmera e tela só para o próprio Concord (canais de voz); o resto, bloqueado.
        HeaderValue::from_static(
            "camera=(self), microphone=(self), display-capture=(self), speaker-selection=(self), \
             geolocation=(), interest-cohort=()",
        ),
    );
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    // Sem cache por padrão; respostas com política própria (imagens) a mantêm.
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    res
}

fn render_policy(cfg: &Config) -> String {
    let or_pending = |v: &str| {
        if v.is_empty() {
            "[não configurado pelo operador]".to_string()
        } else {
            escape_html(v)
        }
    };
    include_str!("../static/privacidade.html")
        .replace("{{CONTROLADOR}}", &or_pending(&cfg.controller))
        .replace("{{CONTATO}}", &or_pending(&cfg.contact))
        .replace("{{RETENCAO_DIAS}}", &cfg.retention_days.to_string())
        .replace("{{VERSAO}}", &POLICY_VERSION.to_string())
        .replace("{{ATUALIZADO_EM}}", POLICY_UPDATED_AT)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
