//! Rotas HTTP das imagens do chat e dos sons do Soundpad.
//!
//! - `POST /anexos/{upload_id}?token=…&name=…`: recebe a imagem com uma
//!   autorização de uso único obtida pelo WebSocket (`upload_begin`), limpa os
//!   metadados e guarda como pendente até a mensagem ser enviada.
//! - `GET /anexos/{id}?exp=…&sig=…`: entrega a imagem só com link assinado e
//!   dentro da validade.
//! - `POST /sons/{upload_id}` e `GET /sons/{id}`: o mesmo esquema para os sons.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::db::{self, NewAttachment, NewSound};
use crate::media;
use crate::model::{Audience, Event, ServerMsg};
use crate::state::{now_ms, AppState, Fail, UploadPurpose, MAX_SOUNDS_PER_SERVER};

#[derive(Deserialize)]
pub struct UploadQuery {
    token: String,
    #[serde(default)]
    name: Option<String>,
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}

/// Nome exibido: sem pastas, sem caracteres de controle, com a extensão real do formato.
fn clean_filename(name: Option<&str>, kind: media::Kind) -> String {
    let base = name
        .unwrap_or("imagem")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("imagem");
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let stem: String = stem
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '<' | '>' | '\\' | '/'))
        .take(60)
        .collect();
    let stem = if stem.trim().is_empty() {
        "imagem".to_string()
    } else {
        stem.trim().to_string()
    };
    let ext = match kind {
        media::Kind::Png => "png",
        media::Kind::Jpeg => "jpg",
        media::Kind::Gif => "gif",
        media::Kind::Webp => "webp",
    };
    format!("{stem}.{ext}")
}

pub async fn upload(
    State(st): State<Arc<AppState>>,
    Path(upload_id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    let Some(grant) = st.take_upload(upload_id, &q.token) else {
        return error(
            StatusCode::FORBIDDEN,
            "Autorização de envio inválida ou vencida",
        );
    };
    let UploadPurpose::Image { channel_id } = grant.purpose else {
        return error(StatusCode::FORBIDDEN, "Autorização de envio inválida");
    };
    if body.is_empty() || body.len() as u64 > grant.max_bytes {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Imagem vazia ou grande demais",
        );
    }
    // Limpeza de metadados fora da thread do chat.
    let clean = match tokio::task::spawn_blocking(move || media::sanitize(&body)).await {
        Ok(Ok(c)) => c,
        Ok(Err(msg)) => return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, msg),
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Erro ao processar a imagem",
            )
        }
    };
    let filename = clean_filename(q.name.as_deref(), clean.kind);
    let result = st.mutate(|_, tx| {
        db::insert_attachment(
            tx,
            &NewAttachment {
                id: upload_id,
                channel_id,
                uploader_id: grant.user_id,
                filename: &filename,
                mime: clean.kind.mime(),
                width: clean.width,
                height: clean.height,
                data: &clean.data,
                created_at: now_ms(),
            },
        )?;
        Ok(())
    });
    match result {
        Ok(()) => (
            StatusCode::CREATED,
            axum::Json(json!({ "id": upload_id, "width": clean.width, "height": clean.height, "name": filename })),
        )
            .into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Erro ao guardar a imagem"),
    }
}

/// POST /sons/{upload_id}?token=…: som do Soundpad (WAV gerado no navegador).
pub async fn upload_sound(
    State(st): State<Arc<AppState>>,
    Path(upload_id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    let Some(grant) = st.take_upload(upload_id, &q.token) else {
        return error(
            StatusCode::FORBIDDEN,
            "Autorização de envio inválida ou vencida",
        );
    };
    let UploadPurpose::Sound {
        server_id,
        name,
        emoji,
    } = grant.purpose
    else {
        return error(StatusCode::FORBIDDEN, "Autorização de envio inválida");
    };
    if body.is_empty() || body.len() as u64 > grant.max_bytes {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "O som deve ter até 5 segundos",
        );
    }
    let clean = match tokio::task::spawn_blocking(move || media::sanitize_wav(&body)).await {
        Ok(Ok(c)) => c,
        Ok(Err(msg)) => return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, msg),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "Erro ao processar o som"),
    };
    let result = st.mutate(|_, tx| {
        // De novo dentro da transação: dois envios ao mesmo tempo não passam do limite.
        if db::count_sounds(tx, server_id)? >= MAX_SOUNDS_PER_SERVER {
            return Err("Este servidor já atingiu o limite de sons".into());
        }
        db::insert_sound(
            tx,
            &NewSound {
                id: upload_id,
                server_id,
                uploader_id: grant.user_id,
                name: &name,
                emoji: emoji.as_deref(),
                duration_ms: clean.duration_ms,
                data: &clean.data,
                created_at: now_ms(),
            },
        )?;
        Ok(())
    });
    match result {
        Ok(()) => {
            st.emit(Event::Send(
                Audience::Server(server_id),
                ServerMsg::SoundsChanged { server_id },
            ));
            (
                StatusCode::CREATED,
                axum::Json(json!({ "id": upload_id, "duration_ms": clean.duration_ms })),
            )
                .into_response()
        }
        Err(Fail::User(msg)) => error(StatusCode::CONFLICT, &msg),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Erro ao guardar o som"),
    }
}

/// GET /sons/{id}?exp=…&sig=…
pub async fn serve_sound(
    State(st): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<ServeQuery>,
) -> Response {
    if !st.verify_sound(id, q.exp, &q.sig) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(Some(data)) = st.read(|c| db::sound_data(c, id)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut res = data.into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    res
}

#[derive(Deserialize)]
pub struct ServeQuery {
    exp: i64,
    sig: String,
}

pub async fn serve(
    State(st): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<ServeQuery>,
) -> Response {
    if !st.verify_attachment(id, q.exp, &q.sig) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let file = st.read(|c| db::attachment_file(c, id));
    let Ok(Some((mime, _name, data))) = file else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut res = data.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    // Cache só no navegador de quem tem o link (nunca em proxies compartilhados).
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_de_arquivo_limpo_e_com_extensao_real() {
        assert_eq!(
            clean_filename(Some("C:\\fotos\\praia.JPEG"), media::Kind::Jpeg),
            "praia.jpg"
        );
        assert_eq!(
            clean_filename(Some("../../etc/passwd"), media::Kind::Png),
            "passwd.png"
        );
        assert_eq!(
            clean_filename(Some("<script>.gif"), media::Kind::Gif),
            "script.gif"
        );
        assert_eq!(clean_filename(None, media::Kind::Webp), "imagem.webp");
    }
}
