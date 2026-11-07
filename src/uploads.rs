//! Rotas HTTP dos arquivos do chat e dos sons do Soundpad.
//!
//! - `POST /anexos/{upload_id}?token=…&name=…`: recebe o arquivo com uma
//!   autorização de uso único obtida pelo WebSocket (`upload_begin`), limpa os
//!   metadados que conseguir e guarda como pendente até a mensagem ser enviada.
//! - `GET /anexos/{id}?exp=…&sig=…`: entrega só com link assinado e dentro da
//!   validade. Imagem, vídeo e áudio abrem no chat; o resto é sempre baixado,
//!   nunca aberto como página do Concord (evita HTML/SVG com script).
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

/// Nome exibido: sem pastas, sem caracteres de controle. Imagens ganham a extensão real do
/// formato; outros arquivos mantêm a extensão enviada (só letras e números).
fn clean_filename(name: Option<&str>, kind: Option<media::Kind>) -> String {
    let fallback = if kind.is_some() { "imagem" } else { "arquivo" };
    let base = name
        .unwrap_or(fallback)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(fallback);
    let (stem, given_ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (base, None),
    };
    let stem: String = stem
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '"' | '<' | '>' | '\\' | '/')
                // Marcas de direção do texto disfarçam extensões ("foto\u{202e}gpj.exe").
                && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(60)
        .collect();
    let stem = if stem.trim().is_empty() {
        fallback.to_string()
    } else {
        stem.trim().to_string()
    };
    let ext = match kind {
        Some(media::Kind::Png) => "png".to_string(),
        Some(media::Kind::Jpeg) => "jpg".to_string(),
        Some(media::Kind::Gif) => "gif".to_string(),
        Some(media::Kind::Webp) => "webp".to_string(),
        None => given_ext
            .filter(|e| (1..=8).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default(),
    };
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
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
            "Arquivo vazio ou grande demais",
        );
    }
    // Identificação e limpeza de metadados fora da thread do chat.
    let prepared = match tokio::task::spawn_blocking(move || media::prepare(&body)).await {
        Ok(Ok(c)) => c,
        Ok(Err(msg)) => return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, msg),
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Erro ao processar o arquivo",
            )
        }
    };
    let clean = match prepared {
        media::Prepared::Image(c) => c,
        media::Prepared::File { data, mime } => media::Clean::file(data, mime),
    };
    let filename = clean_filename(q.name.as_deref(), clean.image_kind());
    let result = st.mutate(|_, tx| {
        db::insert_attachment(
            tx,
            &NewAttachment {
                id: upload_id,
                channel_id,
                uploader_id: grant.user_id,
                filename: &filename,
                mime: clean.mime(),
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
            axum::Json(json!({ "id": upload_id, "width": clean.width, "height": clean.height, "name": filename, "mime": clean.mime(), "size": clean.data.len() })),
        )
            .into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Erro ao guardar o arquivo"),
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
    headers: axum::http::HeaderMap,
) -> Response {
    if !st.verify_attachment(id, q.exp, &q.sig) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let file = st.read(|c| db::attachment_file(c, id));
    let Ok(Some((mime, name, data))) = file else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let media_type = ["image/", "video/", "audio/"]
        .iter()
        .any(|p| mime.starts_with(p));
    let total = data.len();
    // Pedidos parciais (Range): o player de vídeo usa para avançar e voltar.
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, total));
    let mut res = match range {
        Some((start, end)) => {
            let mut r = data[start..=end].to_vec().into_response();
            *r.status_mut() = StatusCode::PARTIAL_CONTENT;
            if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            r
        }
        None => data.into_response(),
    };
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(if media_type {
            &mime
        } else {
            "application/octet-stream"
        })
        .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    // Cache só no navegador de quem tem o link (nunca em proxies compartilhados).
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    // Aberto diretamente, o arquivo não roda nada com a origem do Concord.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; img-src 'self'; media-src 'self'; sandbox"),
    );
    let disposition = if media_type { "inline" } else { "attachment" };
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "{disposition}; filename*=UTF-8''{}",
            encode_rfc5987(&name)
        ))
        .unwrap_or(HeaderValue::from_static("attachment")),
    );
    res
}

/// "bytes=início-fim" (um intervalo só). Devolve (início, fim) inclusivos dentro do arquivo.
fn parse_range(v: &str, total: usize) -> Option<(usize, usize)> {
    if total == 0 {
        return None;
    }
    let spec = v.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: usize = b.parse().ok()?;
        (total.saturating_sub(n), total - 1)
    } else {
        let start: usize = a.parse().ok()?;
        let end = if b.is_empty() {
            total - 1
        } else {
            b.parse::<usize>().ok()?.min(total - 1)
        };
        (start, end)
    };
    (start <= end && start < total).then_some((start, end))
}

/// Nome de arquivo para `Content-Disposition` (RFC 5987).
fn encode_rfc5987(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_de_arquivo_limpo_e_com_extensao_real() {
        assert_eq!(
            clean_filename(Some("C:\\fotos\\praia.JPEG"), Some(media::Kind::Jpeg)),
            "praia.jpg"
        );
        assert_eq!(
            clean_filename(Some("../../etc/passwd"), Some(media::Kind::Png)),
            "passwd.png"
        );
        assert_eq!(
            clean_filename(Some("<script>.gif"), Some(media::Kind::Gif)),
            "script.gif"
        );
        assert_eq!(clean_filename(None, Some(media::Kind::Webp)), "imagem.webp");
        assert_eq!(
            clean_filename(Some("Setup Jogo.EXE"), None),
            "Setup Jogo.exe"
        );
        assert_eq!(clean_filename(Some("notas.tar.gz"), None), "notas.tar.gz");
        assert_eq!(clean_filename(Some("x.ex\u{202e}e"), None), "x");
        assert_eq!(clean_filename(None, None), "arquivo");
        assert_eq!(
            clean_filename(Some("foto\u{202e}gpj.exe"), None),
            "fotogpj.exe"
        );
    }

    #[test]
    fn intervalos_de_bytes() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=200-", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
    }
}
