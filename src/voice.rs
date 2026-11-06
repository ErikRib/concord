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

/// Porta do LiveKit local (sinalização). A mídia usa UDP 50000–50100 e TCP 7881.
pub const LOCAL_PORT: u16 = 7880;
/// Validade do token de entrada. O LiveKit renova sozinho enquanto a pessoa está na sala.
const TOKEN_TTL_SECS: i64 = 600;

pub struct LiveKit {
    /// URL que os navegadores usam. `None`: LiveKit local, no mesmo host do Concord.
    pub public_url: Option<String>,
    api_key: String,
    api_secret: String,
    child: Mutex<Option<Child>>,
}

fn random_hex(bytes16: usize) -> String {
    (0..bytes16).map(|_| Uuid::new_v4().simple().to_string()).collect()
}

impl LiveKit {
    /// LiveKit externo (produção) pelas variáveis `CONCORD_LIVEKIT_URL`, `_KEY` e `_SECRET`;
    /// senão, inicia o LiveKit local se o executável existir; senão, voz desativada.
    pub fn start() -> Option<LiveKit> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let (Some(url), Some(key), Some(secret)) =
            (get("CONCORD_LIVEKIT_URL"), get("CONCORD_LIVEKIT_KEY"), get("CONCORD_LIVEKIT_SECRET"))
        {
            println!("Voz: usando o LiveKit em {url}");
            return Some(LiveKit { public_url: Some(url), api_key: key, api_secret: secret, child: Mutex::new(None) });
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

        // Chave nova a cada execução; vai por variável de ambiente, não pela linha de comando.
        let api_key = "concord".to_string();
        let api_secret = random_hex(4); // 64 caracteres hex
        let config = format!(
            "port: {LOCAL_PORT}\n\
             rtc:\n  tcp_port: 7881\n  port_range_start: 50000\n  port_range_end: 50100\n  use_external_ip: false\n\
             keys:\n  {api_key}: {api_secret}\n\
             room:\n  empty_timeout: 30\n\
             logging:\n  level: warn\n"
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
                println!("Voz: LiveKit local iniciado na porta {LOCAL_PORT}");
                Some(LiveKit { public_url: None, api_key, api_secret, child: Mutex::new(Some(child)) })
            }
            Err(e) => {
                eprintln!("AVISO: canais de voz desativados; não foi possível iniciar o LiveKit: {e}");
                None
            }
        }
    }

    /// Origens que a CSP precisa liberar para o navegador falar com o LiveKit.
    pub fn csp_sources(&self) -> String {
        match &self.public_url {
            Some(url) => {
                let http = url.replacen("wss://", "https://", 1).replacen("ws://", "http://", 1);
                format!("{url} {http}")
            }
            None => format!("ws://*:{LOCAL_PORT} wss://*:{LOCAL_PORT} http://*:{LOCAL_PORT} https://*:{LOCAL_PORT}"),
        }
    }

    /// Token para entrar na sala do canal. A identidade é o id pseudônimo:
    /// entrar de outra aba derruba a sessão anterior, como no Discord.
    pub fn token(&self, user: &User, room: Uuid, now_secs: i64) -> String {
        let header = json!({ "alg": "HS256", "typ": "JWT" });
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
                "canPublishSources": ["microphone", "camera", "screen_share", "screen_share_audio"],
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

/// Repassa os erros do LiveKit, sem o aviso inofensivo de "CPU monitoring unsupported"
/// do Windows (que vem com um rastreamento de pilha longo).
fn forward_stderr(stderr: std::process::ChildStderr) {
    use std::io::BufRead;
    let mut skipping = false;
    for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
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
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC aceita qualquer tamanho de chave");
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
        let h: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(h["alg"], "HS256");
        let mut mac = Hmac::<Sha256>::new_from_slice(b"your-256-bit-secret").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        assert_eq!(parts[2], URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()));
    }

    #[test]
    fn token_tem_sala_identidade_e_validade() {
        let lk = LiveKit { public_url: None, api_key: "k".into(), api_secret: "s".repeat(64), child: Mutex::new(None) };
        let user = User { id: Uuid::new_v4(), name: "Ana".into(), color: "#000000".into() };
        let room = Uuid::new_v4();
        let token = lk.token(&user, room, 1_000_000);
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(token.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["sub"], user.id.to_string());
        assert_eq!(claims["video"]["room"], room.to_string());
        assert_eq!(claims["exp"], 1_000_600);
        assert_eq!(claims["iss"], "k");
    }
}
