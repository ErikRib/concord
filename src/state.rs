//! Estado compartilhado: banco de dados, cache em memória, conexões abertas
//! e operações da LGPD (consentimento, acesso, eliminação e retenção).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Transaction, TransactionBehavior};
use serde_json::json;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::db::{self, Cache, Db, UserRecord};
use crate::model::{
    new_invite_code, Audience, BanView, Channel, ChannelKind, Event, ModLogEntry, ReportView,
    Server, ServerMsg, ServerView, User, VoiceStateView,
};
use crate::voice::LiveKit;

/// Uma pessoa num canal de voz. Só em memória: nada disso é gravado.
#[derive(Clone)]
pub struct VoiceSession {
    pub conn_id: Uuid,
    pub server_id: Uuid,
    pub channel_id: Uuid,
    pub user: User,
    pub muted: bool,
    pub deafened: bool,
    pub streaming: bool,
    pub camera: bool,
}

/// Versão da política de privacidade. Aumente ao mudar a política:
/// todos precisarão aceitar de novo (LGPD art. 8º, §6º e art. 9º, §2º).
pub const POLICY_VERSION: u32 = 3;
pub const POLICY_UPDATED_AT: &str = "06/11/2026";

/// Quantas mensagens o cliente recebe por página de histórico.
pub const HISTORY_PAGE: usize = 50;
pub const MAX_SERVERS: usize = 10_000;
pub const MAX_OWNED_SERVERS: usize = 10;
pub const MAX_CHANNELS_PER_SERVER: usize = 100;
pub const MAX_MEMBERS_PER_SERVER: usize = 5000;
pub const MAX_REPORTS_PER_SERVER: usize = 500;
pub const MAX_CONNECTIONS: usize = 500;

const DAY_MS: i64 = 86_400_000;
const PURGE_BATCH: usize = 5000;

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

/// Falha de um comando: erro do usuário (mensagem para mostrar) ou erro interno.
#[derive(Debug)]
pub enum Fail {
    User(String),
    Internal(String),
}

impl From<rusqlite::Error> for Fail {
    fn from(e: rusqlite::Error) -> Self {
        Fail::Internal(e.to_string())
    }
}
impl From<&str> for Fail {
    fn from(s: &str) -> Self {
        Fail::User(s.into())
    }
}
impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::User(s)
    }
}

pub type Res<T> = Result<T, Fail>;

/// Configuração lida das variáveis de ambiente.
pub struct Config {
    /// Por quantos dias mensagens, denúncias e registros inativos são mantidos.
    pub retention_days: u32,
    /// Nome de quem opera a instância (controlador, LGPD art. 5º, VI).
    pub controller: String,
    /// Contato do encarregado/canal do titular (LGPD art. 41).
    pub contact: String,
    /// Ids pseudônimos dos administradores da instância.
    pub admins: HashSet<Uuid>,
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
            admins: get("CONCORD_ADMINS")
                .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
                .unwrap_or_default(),
        }
    }
}

impl Cache {
    pub fn server(&self, sid: Uuid) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == sid)
    }

    pub fn server_index(&self, sid: Uuid) -> Option<usize> {
        self.servers.iter().position(|s| s.id == sid)
    }

    /// Índice do servidor que contém o canal.
    pub fn server_index_of_channel(&self, channel_id: Uuid) -> Option<usize> {
        self.servers
            .iter()
            .position(|s| s.channels.iter().any(|c| c.id == channel_id))
    }

    pub fn user_name(&self, uid: Uuid) -> String {
        self.users
            .get(&uid)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "Desconhecido".into())
    }

    pub fn server_view(&self, s: &Server, uid: Uuid, admin: bool) -> ServerView {
        let moderate = s.can_moderate(uid, admin);
        ServerView {
            id: s.id,
            name: s.name.clone(),
            channels: s.channels.clone(),
            public: s.public,
            owner_id: s.owner_id,
            moderators: s.moderators.clone(),
            members: (!s.public).then(|| {
                s.owner_id
                    .iter()
                    .chain(s.members.iter())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .filter_map(|id| {
                        let u = self.users.get(id)?;
                        Some(User {
                            id: *id,
                            name: u.name.clone(),
                            color: u.color.clone(),
                        })
                    })
                    .collect()
            }),
            role: s.role(uid),
            can_moderate: moderate,
            can_manage: s.can_manage(uid, admin),
            invite_code: moderate.then(|| s.invite_code.clone()),
            bans: moderate.then(|| {
                s.bans
                    .iter()
                    .map(|b| BanView {
                        user_id: b.user_id,
                        name: self.user_name(b.user_id),
                        reason: b.reason.clone(),
                        at: b.at,
                    })
                    .collect()
            }),
        }
    }
}

/// Registro de moderação (responsabilização, LGPD art. 6º, X). Sem conteúdo de mensagens.
pub fn log(
    tx: &Transaction,
    server_id: Uuid,
    actor: Option<Uuid>,
    action: &str,
    target: Option<Uuid>,
    detail: &str,
) -> Res<()> {
    db::log(
        tx,
        &ModLogEntry {
            at: now_ms(),
            server_id,
            actor,
            action: action.into(),
            target,
            detail: detail.into(),
        },
    )?;
    Ok(())
}

/// Resultado da eliminação dos dados de um titular.
pub struct Deletion {
    pub messages_removed: usize,
    pub servers_changed: Vec<Uuid>,
}

pub struct AppState {
    pub config: Config,
    pub db: Db,
    /// Cópia em memória de servidores, papéis e perfis (consultados a cada evento).
    pub cache: Mutex<Cache>,
    /// Conexões abertas: id da conexão -> usuário. Só em memória.
    pub online: Mutex<HashMap<Uuid, User>>,
    pub connections: AtomicUsize,
    pub tx: broadcast::Sender<Event>,
    /// LiveKit (voz). `None`: canais de voz desativados.
    pub voice: Option<LiveKit>,
    /// Sessões de voz por pessoa (id pseudônimo).
    pub voice_sessions: Mutex<HashMap<Uuid, VoiceSession>>,
    /// Silenciados/ensurdecidos pela moderação: (servidor, pessoa) -> (mudo, surdo).
    /// Só em memória; vale até um moderador desfazer ou o Concord reiniciar.
    pub server_mutes: Mutex<HashMap<(Uuid, Uuid), (bool, bool)>>,
}

/// Executa uma chamada ao LiveKit em segundo plano (não trava o chat).
fn spawn_lk<F>(fut: F)
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(async move {
                if let Err(e) = fut.await {
                    eprintln!("Voz: falha ao aplicar moderação no LiveKit: {e}");
                }
            });
        }
        Err(_) => eprintln!("Voz: sem runtime para falar com o LiveKit"),
    }
}

impl AppState {
    /// Sem voz (usado nos testes).
    #[cfg(test)]
    pub fn open(db_path: &Path, legacy_json: &Path, config: Config) -> Result<Self, String> {
        Self::open_with_voice(db_path, legacy_json, config, None)
    }

    /// Abre o banco, migra o esquema e, se for a primeira vez, importa o JSON antigo.
    pub fn open_with_voice(
        db_path: &Path,
        legacy_json: &Path,
        config: Config,
        voice: Option<LiveKit>,
    ) -> Result<Self, String> {
        let db =
            Db::open(db_path).map_err(|e| format!("Erro ao abrir {}: {e}", db_path.display()))?;
        {
            let conn = db.writer.lock().unwrap();
            if db::is_empty(&conn).map_err(|e| e.to_string())? {
                if legacy_json.exists() {
                    crate::legacy::import(&conn, legacy_json)?;
                } else {
                    seed_default_server(&conn).map_err(|e| e.to_string())?;
                }
            }
        }
        let cache = {
            let conn = db.writer.lock().unwrap();
            db::load_cache(&conn).map_err(|e| e.to_string())?
        };
        let (tx, _) = broadcast::channel(1024);
        Ok(AppState {
            config,
            db,
            cache: Mutex::new(cache),
            online: Mutex::new(HashMap::new()),
            connections: AtomicUsize::new(0),
            tx,
            voice,
            voice_sessions: Mutex::new(HashMap::new()),
            server_mutes: Mutex::new(HashMap::new()),
        })
    }

    pub fn server_mute_of(&self, sid: Uuid, uid: Uuid) -> (bool, bool) {
        self.server_mutes
            .lock()
            .unwrap()
            .get(&(sid, uid))
            .copied()
            .unwrap_or((false, false))
    }

    /// Remove a pessoa da sala no LiveKit (aplicação no servidor de mídia).
    pub fn lk_remove(&self, room: Uuid, identity: Uuid) {
        if let Some(lk) = &self.voice {
            let admin = lk.admin();
            spawn_lk(async move { admin.remove(room, identity).await });
        }
    }

    /// Libera ou retira a permissão de falar no LiveKit.
    pub fn lk_set_can_speak(&self, room: Uuid, identity: Uuid, can_speak: bool) {
        if let Some(lk) = &self.voice {
            let admin = lk.admin();
            spawn_lk(async move { admin.set_can_speak(room, identity, can_speak).await });
        }
    }

    // ---------- voz ----------

    pub fn voice_states(&self, sid: Uuid) -> Vec<VoiceStateView> {
        let mutes = self.server_mutes.lock().unwrap().clone();
        let mut states: Vec<VoiceStateView> = self
            .voice_sessions
            .lock()
            .unwrap()
            .values()
            .filter(|v| v.server_id == sid)
            .map(|v| {
                let (server_muted, server_deafened) = mutes
                    .get(&(sid, v.user.id))
                    .copied()
                    .unwrap_or((false, false));
                VoiceStateView {
                    channel_id: v.channel_id,
                    user: v.user.clone(),
                    muted: v.muted,
                    deafened: v.deafened,
                    streaming: v.streaming,
                    camera: v.camera,
                    server_muted,
                    server_deafened,
                }
            })
            .collect();
        states.sort_by_key(|s| s.user.name.to_lowercase());
        states
    }

    /// Avisa quem vê o servidor sobre quem está em cada canal de voz.
    pub fn emit_voice(&self, sid: Uuid) {
        let states = self.voice_states(sid);
        self.emit(Event::Send(
            Audience::Server(sid),
            ServerMsg::VoiceStates {
                server_id: sid,
                states,
            },
        ));
    }

    /// Tira a pessoa do canal de voz. Com `conn_id`, só se a sessão for daquela
    /// conexão (outra aba pode ter assumido); com `channel_id`, só se ainda estiver
    /// nesse canal (ela pode ter sido movida). Se o canal esvaziar, o status some.
    pub fn leave_voice(
        &self,
        user_id: Uuid,
        conn_id: Option<Uuid>,
        channel_id: Option<Uuid>,
    ) -> Option<VoiceSession> {
        let left = {
            let mut sessions = self.voice_sessions.lock().unwrap();
            match sessions.get(&user_id) {
                Some(s)
                    if conn_id.is_none_or(|c| c == s.conn_id)
                        && channel_id.is_none_or(|c| c == s.channel_id) =>
                {
                    sessions.remove(&user_id)
                }
                _ => None,
            }
        };
        let session = left?;
        self.after_channel_left(session.channel_id);
        self.emit_voice(session.server_id);
        Some(session)
    }

    /// Se o canal esvaziou, o status some (minimização).
    pub fn after_channel_left(&self, channel_id: Uuid) {
        let empty = !self
            .voice_sessions
            .lock()
            .unwrap()
            .values()
            .any(|v| v.channel_id == channel_id);
        if empty {
            self.clear_voice_status(channel_id);
        }
    }

    /// Minimização: o status existe só enquanto há gente no canal.
    pub fn clear_voice_status(&self, channel_id: Uuid) {
        let result = self.mutate(|cache, tx| {
            let Some(i) = cache.server_index_of_channel(channel_id) else {
                return Ok(None);
            };
            let Some(ch) = cache.servers[i]
                .channels
                .iter_mut()
                .find(|c| c.id == channel_id)
            else {
                return Ok(None);
            };
            if ch.status.is_none() {
                return Ok(None);
            }
            let mut updated = ch.clone();
            updated.status = None;
            updated.status_by = None;
            db::update_channel(tx, &updated)?;
            *ch = updated;
            Ok(Some(cache.servers[i].id))
        });
        match result {
            Ok(Some(sid)) => self.emit(Event::ServerChanged(sid)),
            Ok(None) => {}
            Err(e) => eprintln!("Erro ao limpar status do canal de voz: {e:?}"),
        }
    }

    /// Encerra as sessões de voz de quem não pode mais estar no canal
    /// (canal ou servidor apagado, banimento, saída de servidor privado).
    pub fn kick_voice_where(&self, should_kick: impl Fn(&VoiceSession) -> bool, reason: &str) {
        let kicked: Vec<VoiceSession> = self
            .voice_sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| should_kick(s))
            .cloned()
            .collect();
        for s in kicked {
            self.emit(Event::Send(
                Audience::User(s.user.id),
                ServerMsg::VoiceEnded {
                    reason: reason.into(),
                },
            ));
            self.leave_voice(s.user.id, None, None);
            // Não depende do navegador obedecer: o LiveKit tira a pessoa da sala.
            self.lk_remove(s.channel_id, s.user.id);
        }
    }

    /// Executa uma alteração de forma atômica: trava o cache, abre uma transação,
    /// roda `f` e confirma. Convenção: `f` valida, depois grava no banco e só então
    /// altera o cache. Se o banco falhar, o cache é recarregado do banco.
    pub fn mutate<T>(&self, f: impl FnOnce(&mut Cache, &Transaction) -> Res<T>) -> Res<T> {
        let mut cache = self.cache.lock().unwrap();
        let mut conn = self.db.writer.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = match f(&mut cache, &tx) {
            Ok(v) => tx.commit().map(|_| v).map_err(Fail::from),
            Err(e) => {
                drop(tx); // desfaz
                Err(e)
            }
        };
        if let Err(Fail::Internal(msg)) = &result {
            eprintln!("Erro no banco de dados: {msg}");
            match db::load_cache(&conn) {
                Ok(c) => *cache = c,
                Err(e) => eprintln!("Erro ao recarregar o cache: {e}"),
            }
        }
        result
    }

    /// Leitura no banco (em paralelo com as escritas).
    pub fn read<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> Res<T> {
        Ok(self.db.read(f)?)
    }

    pub fn emit(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }

    pub fn send_all(&self, msg: ServerMsg) {
        self.emit(Event::Send(Audience::All, msg));
    }

    pub fn is_admin(&self, uid: Uuid) -> bool {
        self.config.admins.contains(&uid)
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

    // ---------- filtros por conexão ----------

    pub fn audience_includes(&self, aud: &Audience, uid: Uuid) -> bool {
        match aud {
            Audience::All => true,
            Audience::User(id) => *id == uid,
            Audience::Server(sid) => self
                .cache
                .lock()
                .unwrap()
                .server(*sid)
                .is_some_and(|s| s.can_view(uid)),
        }
    }

    pub fn server_view_for(&self, sid: Uuid, uid: Uuid) -> Option<ServerView> {
        let admin = self.is_admin(uid);
        let cache = self.cache.lock().unwrap();
        let s = cache.server(sid)?;
        s.can_view(uid).then(|| cache.server_view(s, uid, admin))
    }

    pub fn views_for(&self, uid: Uuid) -> Vec<ServerView> {
        let admin = self.is_admin(uid);
        let cache = self.cache.lock().unwrap();
        cache
            .servers
            .iter()
            .filter(|s| s.can_view(uid))
            .map(|s| cache.server_view(s, uid, admin))
            .collect()
    }

    pub fn can_moderate(&self, sid: Uuid, uid: Uuid) -> bool {
        let admin = self.is_admin(uid);
        self.cache
            .lock()
            .unwrap()
            .server(sid)
            .is_some_and(|s| s.can_moderate(uid, admin))
    }

    pub fn reports_for(&self, sid: Uuid, uid: Uuid) -> Option<Vec<ReportView>> {
        if !self.can_moderate(sid, uid) {
            return None;
        }
        match self.read(|c| db::reports_view(c, sid)) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("Erro ao ler denúncias: {e:?}");
                None
            }
        }
    }

    // ---------- titular ----------

    /// Registra o acesso e, se for um aceite novo, a data do consentimento.
    /// Devolve `true` se o apelido ou a cor mudaram desde a última vez.
    pub fn register_user(&self, user: &User, consent_version: u32) -> Res<bool> {
        self.mutate(|cache, tx| {
            let now = now_ms();
            let old = cache.users.get(&user.id).cloned();
            let changed = old
                .as_ref()
                .is_some_and(|o| o.name != user.name || o.color != user.color);
            let rec = UserRecord {
                name: user.name.clone(),
                color: user.color.clone(),
                consent_version,
                consented_at: match &old {
                    Some(o) if o.consent_version == consent_version => o.consented_at,
                    _ => now,
                },
                last_seen: now,
            };
            db::upsert_user(tx, user.id, &rec)?;
            cache.users.insert(user.id, rec);
            Ok(changed)
        })
    }

    /// Correção de dados (LGPD art. 18, III). As mensagens leem o nome atual
    /// do titular, então todas ficam corretas automaticamente (art. 6º, V).
    pub fn update_profile(&self, user: &User) -> Res<bool> {
        self.mutate(|cache, tx| {
            let Some(old) = cache.users.get(&user.id).cloned() else {
                return Ok(false);
            };
            let changed = old.name != user.name || old.color != user.color;
            let rec = UserRecord {
                name: user.name.clone(),
                color: user.color.clone(),
                last_seen: now_ms(),
                ..old
            };
            db::upsert_user(tx, user.id, &rec)?;
            cache.users.insert(user.id, rec);
            Ok(changed)
        })
    }

    pub fn touch(&self, user_id: Uuid) {
        let now = now_ms();
        let result = self.mutate(|cache, tx| {
            db::touch_user(tx, user_id, now)?;
            if let Some(rec) = cache.users.get_mut(&user_id) {
                rec.last_seen = now;
            }
            Ok(())
        });
        if let Err(e) = result {
            eprintln!("Erro ao registrar último acesso: {e:?}");
        }
    }

    /// Acesso e portabilidade (LGPD art. 18, II e V): tudo que temos sobre o titular,
    /// em JSON legível e estruturado.
    pub fn export_user_data(&self, user: &User) -> Res<serde_json::Value> {
        let uid = user.id;
        let rows = self.read(|c| db::export_rows(c, uid))?;
        let cache = self.cache.lock().unwrap();
        let server_name = |sid: Uuid| {
            cache
                .server(sid)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "(apagado)".into())
        };

        let mensagens: Vec<_> = rows
            .messages
            .iter()
            .map(|(id, srv, ch, content, at)| json!({
                "id": id, "servidor": srv, "canal": ch, "conteudo": content, "enviada_em": iso_utc(*at),
            }))
            .collect();
        let servidores: Vec<_> = cache
            .servers
            .iter()
            .filter(|s| s.is_member(uid) || s.moderators.contains(&uid))
            .map(|s| json!({ "nome": s.name, "papel": s.role(uid), "publico": s.public }))
            .collect();
        let denuncias: Vec<_> = rows
            .reports
            .iter()
            .map(|r| {
                json!({
                    "servidor": server_name(r.server_id),
                    "mensagem_id": r.message_id,
                    "categoria": r.category,
                    "detalhes": r.details,
                    "feita_em": iso_utc(r.created_at),
                })
            })
            .collect();
        let banimentos: Vec<_> = cache
            .servers
            .iter()
            .flat_map(|s| {
                s.bans
                    .iter()
                    .filter(|b| b.user_id == uid)
                    .map(move |b| (s, b))
            })
            .map(|(s, b)| json!({ "servidor": s.name, "motivo": b.reason, "em": iso_utc(b.at) }))
            .collect();
        let moderacao: Vec<_> = rows
            .mod_log
            .iter()
            .map(|e| json!({
                "servidor": server_name(e.server_id),
                "acao": e.action,
                "meu_papel": if e.actor == Some(uid) { "autor da ação" } else { "alvo da ação" },
                "detalhe": e.detail,
                "em": iso_utc(e.at),
            }))
            .collect();
        let status_voz: Vec<_> = cache
            .servers
            .iter()
            .flat_map(|s| s.channels.iter().map(move |c| (s, c)))
            .filter(|(_, c)| c.status_by == Some(uid))
            .map(|(s, c)| json!({ "servidor": s.name, "canal": c.name, "status": c.status }))
            .collect();
        let rec = cache.users.get(&uid);
        Ok(json!({
            "observacao": "Datas em UTC (horário de Brasília = UTC-3).",
            "gerado_em": iso_utc(now_ms()),
            "controlador": self.config.controller,
            "contato": self.config.contact,
            "titular": {
                "id_pseudonimo": uid,
                "apelido": user.name,
                "cor": user.color,
                "administrador_da_instancia": self.is_admin(uid),
            },
            "consentimento": rec.map(|r| json!({
                "versao_politica": r.consent_version,
                "aceito_em": iso_utc(r.consented_at),
            })),
            "ultimo_acesso": rec.map(|r| iso_utc(r.last_seen)),
            "retencao_dias": self.config.retention_days,
            "dados_nao_coletados": "Não armazenamos IP, e-mail, telefone, localização nem dados de navegação.",
            "servidores": servidores,
            "mensagens": mensagens,
            "denuncias_feitas": denuncias,
            "banimentos_recebidos": banimentos,
            "registro_de_moderacao": moderacao,
            "status_de_canais_de_voz": status_voz,
            "voz": "Áudio, vídeo e transmissões de tela não são gravados. Quem está em cada canal de voz fica só em memória.",
        }))
    }

    /// Eliminação (LGPD art. 18, VI): apaga mensagens, denúncias, papéis,
    /// banimentos e o registro do titular, anonimiza o registro de moderação e
    /// passa os servidores dele adiante. Tudo numa única transação.
    pub fn delete_user_data(&self, uid: Uuid) -> Res<Deletion> {
        self.server_mutes
            .lock()
            .unwrap()
            .retain(|(_, u), _| *u != uid);
        self.mutate(|cache, tx| {
            // Servidores do titular: primeiro moderador, senão o membro mais antigo.
            let mut transfers = vec![];
            for s in &cache.servers {
                if s.owner_id == Some(uid) {
                    let heir = s
                        .moderators
                        .iter()
                        .chain(s.members.iter())
                        .find(|&&m| m != uid)
                        .copied();
                    transfers.push((s.id, heir));
                }
            }
            let messages_removed = db::delete_user_rows(tx, uid)?;
            for &(sid, heir) in &transfers {
                if let Some(i) = cache.server_index(sid) {
                    let mut s = cache.servers[i].clone();
                    s.owner_id = heir;
                    db::update_server(tx, &s)?;
                    if let Some(h) = heir {
                        db::set_role(tx, "server_moderators", sid, h, false)?;
                    }
                }
            }

            // Banco confirmado: agora o cache.
            let mut servers_changed = vec![];
            for s in cache.servers.iter_mut() {
                let before = (
                    s.owner_id,
                    s.members.len(),
                    s.moderators.len(),
                    s.bans.len(),
                );
                if let Some(&(_, heir)) = transfers.iter().find(|(sid, _)| *sid == s.id) {
                    s.owner_id = heir;
                    if let Some(h) = heir {
                        s.moderators.retain(|&m| m != h);
                    }
                }
                s.members.retain(|&m| m != uid);
                s.moderators.retain(|&m| m != uid);
                s.bans.retain(|b| b.user_id != uid);
                let mut status_cleared = false;
                for c in s.channels.iter_mut().filter(|c| c.status_by == Some(uid)) {
                    c.status = None;
                    c.status_by = None;
                    status_cleared = true;
                }
                if status_cleared
                    || before
                        != (
                            s.owner_id,
                            s.members.len(),
                            s.moderators.len(),
                            s.bans.len(),
                        )
                {
                    servers_changed.push(s.id);
                }
            }
            cache.users.remove(&uid);
            Ok(Deletion {
                messages_removed,
                servers_changed,
            })
        })
    }

    /// Término do tratamento (LGPD art. 15 e 16): remove o que passou do prazo.
    /// Mensagens são apagadas em lotes curtos para não travar o chat.
    pub fn purge_expired(&self) {
        let cutoff = now_ms() - self.config.retention_days as i64 * DAY_MS;
        let mut removed_msgs = 0;
        loop {
            let n = {
                let conn = self.db.writer.lock().unwrap();
                match db::purge_messages_batch(&conn, cutoff, PURGE_BATCH) {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("Retenção: erro ao apagar mensagens: {e}");
                        break;
                    }
                }
            };
            removed_msgs += n;
            if n < PURGE_BATCH {
                break;
            }
        }

        let online: HashSet<Uuid> = self.online.lock().unwrap().values().map(|u| u.id).collect();
        let result = self.mutate(|cache, tx| {
            let counts = db::purge_small_tables(tx, cutoff)?;
            let inactive: Vec<Uuid> = db::inactive_users(tx, cutoff)?
                .into_iter()
                .filter(|id| !online.contains(id))
                .collect();
            for id in &inactive {
                db::delete_user_record(tx, *id)?;
            }
            for s in cache.servers.iter_mut() {
                s.bans.retain(|b| b.at >= cutoff);
            }
            for id in &inactive {
                cache.users.remove(id);
            }
            Ok((counts, inactive.len()))
        });
        match result {
            Ok((c, users)) => {
                if removed_msgs + c.reports + c.mod_log + c.bans + users > 0 {
                    println!(
                        "Retenção: removidos {removed_msgs} mensagem(ns), {} denúncia(s), {} registro(s) de \
                         moderação, {} banimento(s) e {users} titular(es) inativo(s)",
                        c.reports, c.mod_log, c.bans
                    );
                }
            }
            Err(e) => eprintln!("Retenção: erro: {e:?}"),
        }
        self.db.optimize();
    }
}

/// Servidor da instância: público e sem dono (moderado pelos administradores).
fn seed_default_server(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let server = Server {
        id: Uuid::new_v4(),
        name: "Concord".into(),
        channels: vec![
            Channel::new("geral", ChannelKind::Text),
            Channel::new("off-topic", ChannelKind::Text),
            Channel::new("Geral", ChannelKind::Voice),
        ],
        owner_id: None,
        public: true,
        members: vec![],
        moderators: vec![],
        bans: vec![],
        invite_code: new_invite_code(),
    };
    db::insert_server(conn, &server, now_ms())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::model::Message;

    /// Estado com banco temporário (apagado ao fim do teste).
    pub fn temp_state(name: &str) -> (AppState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("concord-test-{name}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Config {
            retention_days: 30,
            controller: String::new(),
            contact: String::new(),
            admins: HashSet::new(),
        };
        let st = AppState::open(&dir.join("t.db"), &dir.join("nao-existe.json"), config).unwrap();
        (st, dir)
    }

    fn user(name: &str) -> User {
        User {
            id: Uuid::new_v4(),
            name: name.into(),
            color: "#000000".into(),
        }
    }

    #[test]
    fn datas_iso() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_793_926_220_566), "2026-11-06T00:50:20Z");
    }

    #[test]
    fn renomear_reflete_nas_mensagens_antigas() {
        let (st, dir) = temp_state("renomear");
        let u = user("Antigo");
        st.register_user(&u, 2).unwrap();
        let ch = st.cache.lock().unwrap().servers[0].channels[0].id;
        let msg = Message {
            id: Uuid::now_v7(),
            channel_id: ch,
            author: u.clone(),
            content: "oi".into(),
            timestamp: now_ms(),
        };
        st.mutate(|_, tx| Ok(db::insert_message(tx, &msg)?))
            .unwrap();
        let novo = User {
            name: "Novo".into(),
            ..u
        };
        assert!(st.update_profile(&novo).unwrap());
        let hist = st.read(|c| db::history(c, ch, None, 10)).unwrap();
        assert_eq!(hist[0].author.name, "Novo");
        assert!(
            !st.update_profile(&novo).unwrap(),
            "sem mudança não deve sinalizar"
        );
        drop(st);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn falha_no_banco_nao_deixa_cache_inconsistente() {
        let (st, dir) = temp_state("rollback");
        let before = st.cache.lock().unwrap().servers.len();
        // A alteração muda o cache e depois o banco falha (canal inexistente na FK).
        let r: Res<()> = st.mutate(|cache, tx| {
            cache.servers.clear();
            let bad = Message {
                id: Uuid::now_v7(),
                channel_id: Uuid::new_v4(),
                author: user("x"),
                content: "x".into(),
                timestamp: 0,
            };
            db::insert_message(tx, &bad)?;
            Ok(())
        });
        assert!(matches!(r, Err(Fail::Internal(_))));
        assert_eq!(
            st.cache.lock().unwrap().servers.len(),
            before,
            "cache recarregado do banco"
        );
        drop(st);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retencao_por_faixa_de_uuid_v7() {
        let old = db::v7_floor(1_000);
        let new = db::v7_floor(2_000);
        assert!(old < new);
        let mid = Uuid::new_v7(uuid::Timestamp::from_unix(uuid::NoContext, 1, 500_000_000));
        assert!(old < mid && mid < new);
    }

    #[test]
    fn permissoes_de_servidor_privado() {
        let (dono, membro, mod_, estranho) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let mut s = Server {
            id: Uuid::new_v4(),
            name: "x".into(),
            channels: vec![],
            owner_id: Some(dono),
            public: false,
            members: vec![dono, membro, mod_],
            moderators: vec![mod_],
            bans: vec![],
            invite_code: new_invite_code(),
        };
        assert!(s.can_view(membro) && !s.can_view(estranho));
        assert!(s.can_moderate(mod_, false) && !s.can_moderate(membro, false));
        assert!(s.can_manage(dono, false) && !s.can_manage(mod_, false));
        assert!(!s.can_moderate(estranho, true));
        s.bans.push(crate::model::Ban {
            user_id: membro,
            reason: "x".into(),
            at: 0,
        });
        assert!(!s.can_view(membro));
        assert_eq!(new_invite_code().len(), 12);
    }
}
