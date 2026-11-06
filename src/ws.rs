//! Conexões WebSocket: uma task por cliente.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast::error::RecvError, mpsc};
use uuid::Uuid;

use crate::commands::{self, clean_color, clean_name, Ctx};
use crate::model::{ClientMsg, Event, ServerMsg, User};
use crate::state::{AppState, MAX_CONNECTIONS, POLICY_VERSION};

pub type Out = mpsc::UnboundedSender<ServerMsg>;

/// Tamanho máximo de uma mensagem WebSocket (2000 caracteres cabem com folga).
const MAX_WS_MESSAGE: usize = 16 * 1024;
/// Tempo para o cliente se identificar depois de conectar.
const HELLO_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    // Bloqueia Cross-Site WebSocket Hijacking: outro site não pode abrir
    // uma conexão em nome do visitante.
    if !origin_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "Origem não permitida").into_response();
    }
    if state.connections.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
        return (StatusCode::SERVICE_UNAVAILABLE, "Servidor lotado").into_response();
    }
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| handle_socket(socket, state))
}

/// Navegadores sempre mandam `Origin`; ela precisa ser o mesmo host do servidor.
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true; // clientes fora do navegador não são alvo de CSWSH
    };
    let (Ok(origin), Some(host)) = (
        origin.to_str(),
        headers.get(header::HOST).and_then(|h| h.to_str().ok()),
    ) else {
        return false;
    };
    let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or("");
    origin_host.eq_ignore_ascii_case(host)
}

/// Conta conexões abertas e garante o decremento mesmo em caso de erro.
struct ConnGuard(Arc<AppState>);
impl ConnGuard {
    fn new(state: &Arc<AppState>) -> Self {
        state.connections.fetch_add(1, Ordering::SeqCst);
        ConnGuard(state.clone())
    }
}
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Limite de ações por conexão (anti-spam / anti-abuso).
struct RateLimiter {
    tokens: f64,
    capacity: f64,
    per_sec: f64,
    last: Instant,
    last_warn: Option<Instant>,
}

impl RateLimiter {
    fn new(capacity: f64, per_sec: f64) -> Self {
        RateLimiter { tokens: capacity, capacity, per_sec, last: Instant::now(), last_warn: None }
    }

    fn take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * self.per_sec)
            .min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Evita responder a cada mensagem bloqueada.
    fn should_warn(&mut self) -> bool {
        let now = Instant::now();
        if self.last_warn.is_none_or(|t| now.duration_since(t) > Duration::from_secs(3)) {
            self.last_warn = Some(now);
            true
        } else {
            false
        }
    }
}

/// O segredo (256 bits em hex) fica só no navegador. O id público é derivado dele,
/// então ninguém consegue se passar por outra pessoa só conhecendo o id.
fn derive_user_id(secret: &str) -> Option<Uuid> {
    if secret.len() != 64 || !secret.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut h = Sha256::new();
    h.update(b"concord-user-id:");
    h.update(secret.to_ascii_lowercase().as_bytes());
    let digest = h.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Some(Uuid::from_bytes(bytes))
}

/// Traduz um evento interno no que esta conexão pode ver.
fn deliver(state: &AppState, uid: Uuid, ev: Event, out: &Out) -> bool {
    let msg = match ev {
        Event::Send(aud, msg) => state.audience_includes(&aud, uid).then_some(msg),
        Event::ServerChanged(sid) => Some(match state.server_view_for(sid, uid) {
            Some(server) => ServerMsg::ServerUpdated { server },
            None => ServerMsg::ServerRemoved { server_id: sid },
        }),
        Event::ServerDeleted(sid) => Some(ServerMsg::ServerRemoved { server_id: sid }),
        Event::ReportsChanged(sid) => state
            .reports_for(sid, uid)
            .map(|reports| ServerMsg::Reports { server_id: sid, reports }),
    };
    match msg {
        Some(m) => out.send(m).is_ok(),
        None => true,
    }
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let _guard = ConnGuard::new(&state);
    let (mut sink, mut stream) = socket.split();

    // Tudo que vai para este cliente passa por esta fila.
    let (out, mut out_rx) = mpsc::unbounded_channel::<ServerMsg>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let Ok(text) = serde_json::to_string(&msg) else { continue };
            if sink.send(WsMessage::Text(text)).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });

    // Espera o "hello". Sem senha e sem cadastro.
    let hello = tokio::time::timeout(HELLO_TIMEOUT, async {
        while let Some(Ok(msg)) = stream.next().await {
            if let WsMessage::Text(text) = msg {
                return serde_json::from_str::<ClientMsg>(&text).ok();
            }
        }
        None
    })
    .await
    .ok()
    .flatten();

    let Some(ClientMsg::Hello { secret, name, color, consent_version }) = hello else {
        return finish(out, writer).await;
    };
    let Some(user_id) = derive_user_id(&secret) else {
        let _ = out.send(ServerMsg::Error { message: "Identidade inválida".into() });
        return finish(out, writer).await;
    };
    // Sem consentimento para a versão atual da política, nada é tratado.
    if consent_version != POLICY_VERSION {
        let _ = out.send(ServerMsg::ConsentRequired {
            version: POLICY_VERSION,
            retention_days: state.config.retention_days,
        });
        return finish(out, writer).await;
    }

    let mut me = User { id: user_id, name: clean_name(&name), color: clean_color(&color) };
    if state.register_user(&me, consent_version) {
        state.send_all(ServerMsg::ProfileUpdated { user: me.clone() });
    }

    let conn_id = Uuid::new_v4();
    state.online.lock().unwrap().insert(conn_id, me.clone());

    // Inscreve antes de mandar o "ready" para não perder eventos.
    let mut rx = state.tx.subscribe();
    let fwd_out = out.clone();
    let fwd_state = state.clone();
    let forwarder = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if !deliver(&fwd_state, user_id, ev, &fwd_out) {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });

    let _ = out.send(ServerMsg::Ready {
        me: me.clone(),
        servers: state.views_for(user_id),
        online: state.online_users(),
        is_admin: state.is_admin(user_id),
    });
    state.send_all(ServerMsg::Presence { online: state.online_users() });

    // ~3 ações por segundo, com rajada de até 10.
    let mut limiter = RateLimiter::new(10.0, 3.0);
    let mut deleted = false;

    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            WsMessage::Text(text) => {
                if !limiter.take() {
                    if limiter.should_warn() {
                        let _ = out.send(ServerMsg::Error {
                            message: "Você está indo rápido demais. Aguarde um instante.".into(),
                        });
                    }
                    continue;
                }
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::DeleteMyData) => {
                        let result = state.delete_user_data(me.id);
                        state.send_all(ServerMsg::UserPurged { user_id: me.id });
                        for sid in result.servers_changed {
                            state.emit(Event::ServerChanged(sid));
                        }
                        let _ = out.send(ServerMsg::DataDeleted { messages_removed: result.messages_removed });
                        deleted = true;
                        break;
                    }
                    Ok(cmd) => {
                        let mut cx = Ctx { state: &state, me: &mut me, conn_id, out: &out };
                        if let Err(message) = commands::handle(cmd, &mut cx) {
                            let _ = out.send(ServerMsg::Error { message });
                        }
                    }
                    Err(_) => {
                        let _ = out.send(ServerMsg::Error { message: "Mensagem inválida".into() });
                    }
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }

    forwarder.abort();
    state.online.lock().unwrap().remove(&conn_id);
    if !deleted {
        state.touch(me.id);
    }
    state.send_all(ServerMsg::Presence { online: state.online_users() });
    finish(out, writer).await;
}

/// Fecha a fila de saída e dá um tempo para as últimas mensagens serem entregues.
async fn finish(out: Out, writer: tokio::task::JoinHandle<()>) {
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_derivado_e_estavel_e_valida_segredo() {
        let s = "a".repeat(64);
        assert_eq!(derive_user_id(&s), derive_user_id(&s.to_uppercase()));
        assert!(derive_user_id("curto").is_none());
        assert!(derive_user_id(&"z".repeat(64)).is_none());
    }

    #[test]
    fn origem() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "localhost:3000".parse().unwrap());
        assert!(origin_allowed(&h));
        h.insert(header::ORIGIN, "http://localhost:3000".parse().unwrap());
        assert!(origin_allowed(&h));
        h.insert(header::ORIGIN, "https://malicioso.com".parse().unwrap());
        assert!(!origin_allowed(&h));
    }

    #[test]
    fn limitador() {
        let mut r = RateLimiter::new(3.0, 0.0);
        assert!(r.take() && r.take() && r.take());
        assert!(!r.take());
    }
}
