mod commands;
mod model;
mod state;
mod ws;

use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use state::{AppState, Config, POLICY_UPDATED_AT, POLICY_VERSION};

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3000);
    let data_path = PathBuf::from(
        std::env::var("CONCORD_DATA").unwrap_or_else(|_| "concord-data.json".into()),
    );

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
        println!("{} administrador(es) da instância configurado(s)", config.admins.len());
    }
    let policy_page: &'static str = Box::leak(render_policy(&config).into_boxed_str());

    let state = Arc::new(AppState::load(data_path, config));

    // Salva em disco a cada 5 s se houve mudança.
    let saver = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            saver.save_if_dirty();
        }
    });

    // Retenção: apaga o que passou do prazo, na inicialização e a cada hora.
    let janitor = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            janitor.purge_expired();
        }
    });

    // O cliente web vai embutido no binário.
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../static/index.html")) }))
        // O código é lido pelo cliente; o servidor só entrega a mesma página.
        .route("/convite/:code", get(|| async { Html(include_str!("../static/index.html")) }))
        .route("/privacidade", get(move || async move { Html(policy_page) }))
        .route("/style.css", get(|| async { asset("text/css", include_str!("../static/style.css")) }))
        .route(
            "/app.js",
            get(|| async { asset("application/javascript", include_str!("../static/app.js")) }),
        )
        .route("/ws", get(ws::ws_handler))
        .layer(middleware::from_fn(security_headers))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| panic!("Não foi possível usar a porta {port}: {e}"));
    println!("Concord rodando em http://localhost:{port}");

    tokio::select! {
        result = axum::serve(listener, app).into_future() => {
            if let Err(e) = result {
                eprintln!("Erro no servidor: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => println!("\nEncerrando…"),
    }

    if let Err(e) = state.save() {
        eprintln!("Erro ao salvar dados: {e}");
    }
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
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
             connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
             frame-ancestors 'none'",
        ),
    );
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), interest-cohort=()"),
    );
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
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
