//! Integração com o LiveKit (servidor de mídia/SFU dos canais de voz).
//!
//! O Concord decide quem pode entrar em cada canal e emite um token de acesso
//! curto (JWT HS256). Áudio, vídeo e tela trafegam direto entre o navegador e o
//! LiveKit, criptografados (DTLS-SRTP), e **nunca são gravados**.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;
use uuid::Uuid;

use crate::model::User;

/// Porta padrão do LiveKit local (sinalização). A mídia usa TCP porta+1 e UDP 50000–50100.
pub const LOCAL_PORT: u16 = 7880;
/// Validade do token de entrada. O LiveKit renova sozinho enquanto a pessoa está na sala.
const TOKEN_TTL_SECS: i64 = 600;

pub struct LiveKit {
    /// URL que os navegadores usam. `None`: LiveKit local, no mesmo host do Concord,
    /// na porta `port` (o navegador monta a URL com o próprio endereço).
    pub public_url: Option<String>,
    pub port: u16,
    /// URL interna (http://) da API administrativa, usada para aplicar a moderação.
    api_url: Option<String>,
    api_key: String,
    api_secret: String,
    child: Mutex<Option<Child>>,
}

/// Cliente da API administrativa do LiveKit (Twirp/JSON). Clonável para tarefas assíncronas.
#[derive(Clone)]
pub struct LiveKitAdmin {
    api_url: Option<String>,
    api_key: String,
    api_secret: String,
}

fn random_hex(bytes16: usize) -> String {
    (0..bytes16)
        .map(|_| Uuid::new_v4().simple().to_string())
        .collect()
}

impl LiveKit {
    /// LiveKit externo (produção) pelas variáveis `CONCORD_LIVEKIT_URL`, `_KEY` e `_SECRET`;
    /// senão, inicia o LiveKit local se o executável existir; senão, voz desativada.
    pub fn start() -> Option<LiveKit> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let (Some(url), Some(key), Some(secret)) = (
            get("CONCORD_LIVEKIT_URL"),
            get("CONCORD_LIVEKIT_KEY"),
            get("CONCORD_LIVEKIT_SECRET"),
        ) {
            // A API administrativa deve ser acessada por rede interna, em http://.
            let api_url = get("CONCORD_LIVEKIT_API_URL").or_else(|| {
                url.starts_with("ws://")
                    .then(|| url.replacen("ws://", "http://", 1))
            });
            if api_url.is_none() {
                eprintln!(
                    "AVISO: defina CONCORD_LIVEKIT_API_URL (http:// interno do LiveKit) para que a \
                     moderação de voz seja aplicada no servidor de mídia."
                );
            }
            println!("Voz: usando o LiveKit em {url}");
            return Some(LiveKit {
                public_url: Some(url),
                port: 0,
                api_url,
                api_key: key,
                api_secret: secret,
                child: Mutex::new(None),
            });
        }

        let bin = get("CONCORD_LIVEKIT_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("tools/livekit/livekit-server.exe"));
        if !bin.exists() {
            eprintln!(
                "AVISO: canais de voz desativados. Coloque o livekit-server em {} \
                 ou defina CONCORD_LIVEKIT_URL/KEY/SECRET.",
                bin.display()
            );
            return None;
        }

        let port: u16 = get("CONCORD_LIVEKIT_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(LOCAL_PORT);
        let udp: u16 = get("CONCORD_LIVEKIT_UDP_START")
            .and_then(|p| p.parse().ok())
            .unwrap_or(50000);
        // Outro LiveKit (ex.: outro Concord aberto) já na porta: os tokens daqui seriam
        // recusados por ele. Melhor desativar a voz com um aviso claro.
        if std::net::TcpListener::bind(("0.0.0.0", port)).is_err() {
            eprintln!(
                "AVISO: canais de voz desativados: a porta {port} já está em uso (outro Concord ou \
                 LiveKit aberto?). Feche-o ou use CONCORD_LIVEKIT_PORT e CONCORD_LIVEKIT_UDP_START."
            );
            return None;
        }
        // URL pública opcional para o LiveKit local (ex.: wss:// via Tailscale ou proxy HTTPS).
        let public_url = get("CONCORD_LIVEKIT_PUBLIC_URL");

        // Chave nova a cada execução; vai por variável de ambiente, não pela linha de comando.
        let api_key = "concord".to_string();
        let api_secret = random_hex(4); // 64 caracteres hex
        let config = format!(
            "port: {port}\n\
             rtc:\n  tcp_port: {}\n  port_range_start: {udp}\n  port_range_end: {}\n  use_external_ip: false\n\
             keys:\n  {api_key}: {api_secret}\n\
             room:\n  empty_timeout: 30\n\
             logging:\n  level: warn\n",
            port + 1,
            udp + 100
        );
        match Command::new(&bin)
            .env("LIVEKIT_CONFIG", config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                #[cfg(windows)]
                tie_to_this_process(&child);
                if let Some(stderr) = child.stderr.take() {
                    std::thread::spawn(move || forward_stderr(stderr));
                }
                println!(
                    "Voz: LiveKit local iniciado na porta {port} (mídia: TCP {}, UDP {udp}–{})",
                    port + 1,
                    udp + 100
                );
                Some(LiveKit {
                    public_url,
                    port,
                    api_url: Some(format!("http://127.0.0.1:{port}")),
                    api_key,
                    api_secret,
                    child: Mutex::new(Some(child)),
                })
            }
            Err(e) => {
                eprintln!(
                    "AVISO: canais de voz desativados; não foi possível iniciar o LiveKit: {e}"
                );
                None
            }
        }
    }

    /// Origens que a CSP precisa liberar para o navegador falar com o LiveKit.
    pub fn csp_sources(&self) -> String {
        match &self.public_url {
            Some(url) => {
                let http = url
                    .replacen("wss://", "https://", 1)
                    .replacen("ws://", "http://", 1);
                format!("{url} {http}")
            }
            None => {
                let p = self.port;
                format!("ws://*:{p} wss://*:{p} http://*:{p} https://*:{p}")
            }
        }
    }

    pub fn admin(&self) -> LiveKitAdmin {
        LiveKitAdmin {
            api_url: self.api_url.clone(),
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
        }
    }

    /// Token para entrar na sala do canal. A identidade é o id pseudônimo:
    /// entrar de outra aba derruba a sessão anterior, como no Discord.
    /// Sem `can_speak` (silenciado pela moderação), o microfone não pode ser publicado.
    pub fn token(&self, user: &User, room: Uuid, now_secs: i64, can_speak: bool) -> String {
        let header = json!({ "alg": "HS256", "typ": "JWT" });
        let mut sources = vec!["camera", "screen_share", "screen_share_audio"];
        if can_speak {
            sources.insert(0, "microphone");
        }
        let claims = json!({
            "iss": self.api_key,
            "sub": user.id.to_string(),
            "name": user.name,
            "nbf": now_secs - 10,
            "exp": now_secs + TOKEN_TTL_SECS,
            "video": {
                "room": room.to_string(),
                "roomJoin": true,
                "canPublish": true,
                "canSubscribe": true,
                "canPublishData": true,
                "canPublishSources": sources,
            },
        });
        sign_jwt(&header, &claims, self.api_secret.as_bytes())
    }

    pub fn shutdown(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for LiveKit {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl LiveKitAdmin {
    /// Token de administrador, restrito a uma sala e válido por 1 minuto.
    fn admin_token(&self, room: Uuid) -> String {
        let now = crate::state::now_ms() / 1000;
        let claims = json!({
            "iss": self.api_key,
            "sub": "concord-moderacao",
            "nbf": now - 10,
            "exp": now + 60,
            "video": { "room": room.to_string(), "roomAdmin": true },
        });
        sign_jwt(
            &json!({ "alg": "HS256", "typ": "JWT" }),
            &claims,
            self.api_secret.as_bytes(),
        )
    }

    /// Chama um método do RoomService. "not_found" (pessoa já saiu) conta como sucesso.
    async fn call(
        &self,
        method: &str,
        room: Uuid,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let api = self
            .api_url
            .as_deref()
            .ok_or("API administrativa do LiveKit não configurada")?;
        let path = format!("/twirp/livekit.RoomService/{method}");
        let (status, text) =
            http_post(api, &path, &self.admin_token(room), &body.to_string()).await?;
        if status == 200 {
            return Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null));
        }
        if text.contains("not_found") {
            return Ok(serde_json::Value::Null);
        }
        Err(format!("LiveKit {method}: HTTP {status}: {text}"))
    }

    /// Tira a pessoa da sala no servidor de mídia (não depende do navegador dela).
    pub async fn remove(&self, room: Uuid, identity: Uuid) -> Result<(), String> {
        self.call(
            "RemoveParticipant",
            room,
            json!({ "room": room.to_string(), "identity": identity.to_string() }),
        )
        .await
        .map(|_| ())
    }

    /// Silenciar pelo servidor: retira a permissão de publicar o microfone e muta a
    /// trilha que já estiver no ar. A pessoa não consegue reativar sozinha.
    pub async fn set_can_speak(
        &self,
        room: Uuid,
        identity: Uuid,
        can_speak: bool,
    ) -> Result<(), String> {
        let mut sources = vec!["CAMERA", "SCREEN_SHARE", "SCREEN_SHARE_AUDIO"];
        if can_speak {
            sources.insert(0, "MICROPHONE");
        }
        let id = identity.to_string();
        let room_s = room.to_string();
        self.call(
            "UpdateParticipant",
            room,
            json!({
                "room": room_s,
                "identity": id,
                "permission": {
                    "can_subscribe": true,
                    "can_publish": true,
                    "can_publish_data": true,
                    "can_publish_sources": sources,
                },
            }),
        )
        .await?;
        if !can_speak {
            let info = self
                .call(
                    "GetParticipant",
                    room,
                    json!({ "room": room_s, "identity": id }),
                )
                .await?;
            let tracks = info
                .get("tracks")
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default();
            for t in tracks {
                if t.get("source").and_then(|s| s.as_str()) == Some("MICROPHONE") {
                    if let Some(sid) = t.get("sid").and_then(|s| s.as_str()) {
                        self.call(
                            "MutePublishedTrack",
                            room,
                            json!({ "room": room_s, "identity": id, "track_sid": sid, "muted": true }),
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// POST HTTP/1.1 mínimo (só http://, para a rede interna do LiveKit).
async fn http_post(
    base: &str,
    path: &str,
    bearer: &str,
    body: &str,
) -> Result<(u16, String), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let host = base
        .strip_prefix("http://")
        .ok_or("A API do LiveKit precisa ser http:// (rede interna)")?;
    let host = host.trim_end_matches('/');
    let fut = async {
        let mut stream = tokio::net::TcpStream::connect(host)
            .await
            .map_err(|e| e.to_string())?;
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {bearer}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(raw)
    };
    let raw = tokio::time::timeout(std::time::Duration::from_secs(5), fut)
        .await
        .map_err(|_| "tempo esgotado falando com o LiveKit".to_string())??;
    parse_http_response(&raw)
}

fn parse_http_response(raw: &[u8]) -> Result<(u16, String), String> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or("resposta HTTP inválida")?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("resposta HTTP sem status")?;
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let body = if chunked {
        decode_chunked(body)
    } else {
        body.to_string()
    };
    Ok((status, body))
}

fn decode_chunked(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        s = rest[n..].trim_start_matches("\r\n");
    }
    out
}

/// Repassa os erros do LiveKit, sem o aviso inofensivo de "CPU monitoring unsupported"
/// do Windows (que vem com um rastreamento de pilha longo).
fn forward_stderr(stderr: std::process::ChildStderr) {
    use std::io::BufRead;
    let mut skipping = false;
    for line in std::io::BufReader::new(stderr)
        .lines()
        .map_while(Result::ok)
    {
        if line.contains("CPU monitoring unsupported") {
            skipping = true;
            continue;
        }
        // O rastreamento termina quando aparece uma nova linha de log (começa com a data).
        if skipping && !line.starts_with("20") {
            continue;
        }
        skipping = false;
        eprintln!("[livekit] {line}");
    }
}

/// Coloca o LiveKit num Job Object com KILL_ON_JOB_CLOSE: quando o processo do
/// Concord termina, de qualquer forma, o Windows fecha o job e encerra o LiveKit.
#[cfg(windows)]
fn tie_to_this_process(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // SAFETY: chamadas Win32 com ponteiros válidos; o handle do job fica aberto
    // de propósito até o fim do processo (é isso que mantém o vínculo).
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            eprintln!("AVISO: não foi possível vincular o LiveKit ao Concord (CreateJobObject)");
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
            eprintln!("AVISO: não foi possível vincular o LiveKit ao Concord; encerre-o manualmente se ficar aberto");
        }
    }
}

fn sign_jwt(header: &serde_json::Value, claims: &serde_json::Value, secret: &[u8]) -> String {
    let h = URL_SAFE_NO_PAD.encode(header.to_string());
    let c = URL_SAFE_NO_PAD.encode(claims.to_string());
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret).expect("HMAC aceita qualquer tamanho de chave");
    mac.update(format!("{h}.{c}").as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{h}.{c}.{sig}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_hs256_tem_tres_partes_e_assinatura_verificavel() {
        let header = json!({"alg":"HS256","typ":"JWT"});
        let claims = json!({"sub":"1234567890","name":"John Doe","iat":1516239022});
        let token = sign_jwt(&header, &claims, b"your-256-bit-secret");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        // Cabeçalho legível e assinatura recalculada sobre as duas primeiras partes.
        let h: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(h["alg"], "HS256");
        let mut mac = Hmac::<Sha256>::new_from_slice(b"your-256-bit-secret").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        assert_eq!(
            parts[2],
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        );
    }

    #[test]
    fn token_tem_sala_identidade_e_validade() {
        let lk = LiveKit {
            public_url: None,
            port: 7880,
            api_url: None,
            api_key: "k".into(),
            api_secret: "s".repeat(64),
            child: Mutex::new(None),
        };
        let user = User {
            id: Uuid::new_v4(),
            name: "Ana".into(),
            color: "#000000".into(),
        };
        let room = Uuid::new_v4();
        let decode = |t: String| -> serde_json::Value {
            serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(t.split('.').nth(1).unwrap())
                    .unwrap(),
            )
            .unwrap()
        };
        let claims = decode(lk.token(&user, room, 1_000_000, true));
        assert_eq!(claims["sub"], user.id.to_string());
        assert_eq!(claims["video"]["room"], room.to_string());
        assert_eq!(claims["exp"], 1_000_600);
        assert_eq!(claims["iss"], "k");
        assert!(claims["video"]["canPublishSources"]
            .as_array()
            .unwrap()
            .contains(&json!("microphone")));
        let muted = decode(lk.token(&user, room, 1_000_000, false));
        assert!(!muted["video"]["canPublishSources"]
            .as_array()
            .unwrap()
            .contains(&json!("microphone")));
    }

    #[test]
    fn resposta_http_simples_e_chunked() {
        let (s, b) =
            parse_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!((s, b.as_str()), (200, "{}"));
        let raw = b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n2\r\n1}\r\n0\r\n\r\n";
        let (s, b) = parse_http_response(raw).unwrap();
        assert_eq!((s, b.as_str()), (404, "{\"a\":1}"));
    }
}
