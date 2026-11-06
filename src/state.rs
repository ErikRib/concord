//! Estado compartilhado, persistência em JSON e operações da LGPD
//! (registro de consentimento, retenção, acesso/portabilidade e eliminação).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::model::{Channel, Message, Server, ServerMsg, User};

/// Versão da política de privacidade. Aumente ao mudar a política:
/// todos precisarão aceitar de novo (LGPD art. 8º, §6º e art. 9º, §2º).
pub const POLICY_VERSION: u32 = 1;
pub const POLICY_UPDATED_AT: &str = "05/11/2026";

/// Máximo de mensagens guardadas por canal.
pub const MAX_STORED_PER_CHANNEL: usize = 1000;
/// Quantas mensagens o cliente recebe ao abrir um canal.
pub const HISTORY_SIZE: usize = 100;
pub const MAX_SERVERS: usize = 200;
pub const MAX_CHANNELS_PER_SERVER: usize = 100;
pub const MAX_CONNECTIONS: usize = 500;

const DAY_MS: i64 = 86_400_000;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Data legível em UTC (ISO 8601), para o titular entender a exportação (LGPD art. 19).
pub fn iso_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Conversão de dias desde 1970-01-01 para data civil (algoritmo de Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Atualiza o registro do titular e o nome/cor em todas as mensagens dele,
/// para que os dados fiquem corretos e atualizados (LGPD art. 6º, V).
fn apply_profile(data: &mut Data, user: &User) -> bool {
    let changed = match data.users.get_mut(&user.id) {
        Some(rec) if rec.name != user.name || rec.color != user.color => {
            rec.name = user.name.clone();
            rec.color = user.color.clone();
            true
        }
        _ => false,
    };
    if changed {
        for m in data.messages.values_mut().flatten() {
            if m.author.id == user.id {
                m.author.name = user.name.clone();
                m.author.color = user.color.clone();
            }
        }
    }
    changed
}

/// Configuração lida das variáveis de ambiente.
pub struct Config {
    /// Por quantos dias mensagens e registros inativos são mantidos.
    pub retention_days: u32,
    /// Nome de quem opera a instância (controlador, LGPD art. 5º, VI).
    pub controller: String,
    /// Contato do encarregado/canal do titular (LGPD art. 41).
    pub contact: String,
}

impl Config {
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Config {
            retention_days: get("CONCORD_RETENCAO_DIAS")
                .and_then(|v| v.parse().ok())
                .filter(|&d| d > 0)
                .unwrap_or(180),
            controller: get("CONCORD_CONTROLADOR").unwrap_or_default(),
            contact: get("CONCORD_CONTATO").unwrap_or_default(),
        }
    }
}

/// Registro do titular: prova do consentimento (LGPD art. 8º, §2º)
/// e controle de retenção. Não guardamos IP, e-mail ou qualquer outro dado.
#[derive(Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub name: String,
    pub color: String,
    pub consent_version: u32,
    pub consented_at: i64,
    pub last_seen: i64,
}

/// Tudo que é salvo em disco.
#[derive(Default, Serialize, Deserialize)]
pub struct Data {
    pub servers: Vec<Server>,
    pub messages: HashMap<Uuid, Vec<Message>>,
    #[serde(default)]
    pub users: HashMap<Uuid, UserRecord>,
}

impl Data {
    fn with_defaults() -> Self {
        let ch = |name: &str| Channel { id: Uuid::new_v4(), name: name.into() };
        Data {
            servers: vec![Server {
                id: Uuid::new_v4(),
                name: "Concord".into(),
                channels: vec![ch("geral"), ch("off-topic")],
            }],
            ..Default::default()
        }
    }

    pub fn channel_exists(&self, channel_id: Uuid) -> bool {
        self.servers
            .iter()
            .any(|s| s.channels.iter().any(|c| c.id == channel_id))
    }
}

pub struct AppState {
    pub config: Config,
    pub data: Mutex<Data>,
    /// Conexões abertas: id da conexão -> usuário. Só em memória.
    pub online: Mutex<HashMap<Uuid, User>>,
    pub connections: AtomicUsize,
    pub tx: broadcast::Sender<ServerMsg>,
    dirty: AtomicBool,
    path: PathBuf,
}

impl AppState {
    pub fn load(path: PathBuf, config: Config) -> Self {
        let data = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(d) => d,
                Err(e) => {
                    // Não sobrescreve um arquivo corrompido: melhor parar.
                    eprintln!("Erro ao ler {}: {e}", path.display());
                    std::process::exit(1);
                }
            },
            Err(_) => Data::with_defaults(),
        };
        let (tx, _) = broadcast::channel(1024);
        AppState {
            config,
            data: Mutex::new(data),
            online: Mutex::new(HashMap::new()),
            connections: AtomicUsize::new(0),
            tx,
            dirty: AtomicBool::new(true),
            path,
        }
    }

    pub fn broadcast(&self, msg: ServerMsg) {
        let _ = self.tx.send(msg);
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Salva só se algo mudou desde o último salvamento.
    pub fn save_if_dirty(&self) {
        if self.dirty.swap(false, Ordering::SeqCst) {
            if let Err(e) = self.save() {
                eprintln!("Erro ao salvar dados: {e}");
                self.mark_dirty();
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let json = {
            let data = self.data.lock().unwrap();
            serde_json::to_string(&*data)?
        };
        // Escreve num arquivo temporário e renomeia, para não corromper em caso de queda.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)
    }

    /// Usuários online, sem repetir quem está com várias abas abertas.
    pub fn online_users(&self) -> Vec<User> {
        let online = self.online.lock().unwrap();
        let mut by_id: HashMap<Uuid, User> = HashMap::new();
        for u in online.values() {
            by_id.insert(u.id, u.clone());
        }
        let mut users: Vec<User> = by_id.into_values().collect();
        users.sort_by_key(|u| u.name.to_lowercase());
        users
    }

    /// Registra o acesso e, se for um aceite novo, a data do consentimento.
    /// Devolve `true` se o apelido ou a cor mudaram desde a última vez.
    pub fn register_user(&self, user: &User, consent_version: u32) -> bool {
        let now = now_ms();
        let mut data = self.data.lock().unwrap();
        let rec = data.users.entry(user.id).or_insert_with(|| UserRecord {
            name: user.name.clone(),
            color: user.color.clone(),
            consent_version,
            consented_at: now,
            last_seen: now,
        });
        if rec.consent_version != consent_version {
            rec.consent_version = consent_version;
            rec.consented_at = now;
        }
        rec.last_seen = now;
        let changed = apply_profile(&mut data, user);
        drop(data);
        self.mark_dirty();
        changed
    }

    /// Correção de dados (LGPD art. 18, III). Devolve `true` se algo mudou.
    pub fn update_profile(&self, user: &User) -> bool {
        let mut data = self.data.lock().unwrap();
        if let Some(rec) = data.users.get_mut(&user.id) {
            rec.last_seen = now_ms();
        }
        let changed = apply_profile(&mut data, user);
        drop(data);
        self.mark_dirty();
        changed
    }

    pub fn touch(&self, user_id: Uuid) {
        let mut data = self.data.lock().unwrap();
        if let Some(rec) = data.users.get_mut(&user_id) {
            rec.last_seen = now_ms();
        }
        drop(data);
        self.mark_dirty();
    }

    /// Acesso e portabilidade (LGPD art. 18, II e V): tudo que temos sobre o titular,
    /// em JSON legível e estruturado.
    pub fn export_user_data(&self, user: &User) -> serde_json::Value {
        let data = self.data.lock().unwrap();
        let mut where_is: HashMap<Uuid, (&str, &str)> = HashMap::new();
        for s in &data.servers {
            for c in &s.channels {
                where_is.insert(c.id, (&s.name, &c.name));
            }
        }
        let mut msgs: Vec<&Message> = data
            .messages
            .values()
            .flatten()
            .filter(|m| m.author.id == user.id)
            .collect();
        msgs.sort_by_key(|m| m.timestamp);
        let mensagens: Vec<_> = msgs
            .iter()
            .map(|m| {
                let (srv, ch) = where_is.get(&m.channel_id).copied().unwrap_or(("?", "?"));
                json!({
                    "id": m.id,
                    "servidor": srv,
                    "canal": ch,
                    "conteudo": m.content,
                    "enviada_em": iso_utc(m.timestamp),
                })
            })
            .collect();
        let rec = data.users.get(&user.id);
        json!({
            "observacao": "Datas em UTC (horário de Brasília = UTC-3).",
            "gerado_em": iso_utc(now_ms()),
            "controlador": self.config.controller,
            "contato": self.config.contact,
            "titular": {
                "id_pseudonimo": user.id,
                "apelido": user.name,
                "cor": user.color,
            },
            "consentimento": rec.map(|r| json!({
                "versao_politica": r.consent_version,
                "aceito_em": iso_utc(r.consented_at),
            })),
            "ultimo_acesso": rec.map(|r| iso_utc(r.last_seen)),
            "retencao_dias": self.config.retention_days,
            "dados_nao_coletados": "Não armazenamos IP, e-mail, telefone, localização nem dados de navegação.",
            "mensagens": mensagens,
        })
    }

    /// Eliminação (LGPD art. 18, VI): apaga mensagens e o registro do titular.
    pub fn delete_user_data(&self, user_id: Uuid) -> usize {
        let mut data = self.data.lock().unwrap();
        let mut removed = 0;
        for list in data.messages.values_mut() {
            let before = list.len();
            list.retain(|m| m.author.id != user_id);
            removed += before - list.len();
        }
        data.messages.retain(|_, l| !l.is_empty());
        data.users.remove(&user_id);
        drop(data);
        self.mark_dirty();
        self.save_if_dirty(); // grava já, sem esperar o ciclo
        removed
    }

    /// Término do tratamento (LGPD art. 15 e 16): remove mensagens mais antigas
    /// que o prazo de retenção e registros de titulares inativos.
    pub fn purge_expired(&self) {
        let cutoff = now_ms() - self.config.retention_days as i64 * DAY_MS;
        let online: HashSet<Uuid> = self.online.lock().unwrap().values().map(|u| u.id).collect();
        let mut data = self.data.lock().unwrap();

        let mut removed_msgs = 0;
        for list in data.messages.values_mut() {
            let before = list.len();
            list.retain(|m| m.timestamp >= cutoff);
            removed_msgs += before - list.len();
        }
        data.messages.retain(|_, l| !l.is_empty());

        let authors: HashSet<Uuid> = data.messages.values().flatten().map(|m| m.author.id).collect();
        let before_users = data.users.len();
        data.users
            .retain(|id, u| u.last_seen >= cutoff || authors.contains(id) || online.contains(id));
        let removed_users = before_users - data.users.len();
        drop(data);

        if removed_msgs + removed_users > 0 {
            println!("Retenção: {removed_msgs} mensagem(ns) e {removed_users} registro(s) expirados removidos");
            self.mark_dirty();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datas_iso() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_793_926_220_566), "2026-11-06T00:50:20Z");
    }

    #[test]
    fn renomear_atualiza_mensagens_antigas() {
        let id = Uuid::new_v4();
        let old = User { id, name: "Antigo".into(), color: "#000000".into() };
        let ch = Uuid::new_v4();
        let mut data = Data::default();
        data.users.insert(
            id,
            UserRecord { name: old.name.clone(), color: old.color.clone(), consent_version: 1, consented_at: 0, last_seen: 0 },
        );
        data.messages.insert(
            ch,
            vec![Message { id: Uuid::new_v4(), channel_id: ch, author: old, content: "oi".into(), timestamp: 0 }],
        );
        let new = User { id, name: "Novo".into(), color: "#ffffff".into() };
        assert!(apply_profile(&mut data, &new));
        assert_eq!(data.messages[&ch][0].author.name, "Novo");
        assert!(!apply_profile(&mut data, &new), "sem mudança não deve sinalizar");
    }
}
