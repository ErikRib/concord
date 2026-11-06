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

use crate::model::{
    new_invite_code, Audience, BanView, Channel, Event, Message, ModLogEntry, Report, ReportView,
    Server, ServerMsg, ServerView, User,
};

/// Versão da política de privacidade. Aumente ao mudar a política:
/// todos precisarão aceitar de novo (LGPD art. 8º, §6º e art. 9º, §2º).
pub const POLICY_VERSION: u32 = 2;
pub const POLICY_UPDATED_AT: &str = "06/11/2026";

/// Máximo de mensagens guardadas por canal.
pub const MAX_STORED_PER_CHANNEL: usize = 1000;
/// Quantas mensagens o cliente recebe ao abrir um canal.
pub const HISTORY_SIZE: usize = 100;
pub const MAX_SERVERS: usize = 200;
pub const MAX_OWNED_SERVERS: usize = 10;
pub const MAX_CHANNELS_PER_SERVER: usize = 100;
pub const MAX_MEMBERS_PER_SERVER: usize = 1000;
pub const MAX_REPORTS_PER_SERVER: usize = 500;
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
    #[serde(default)]
    pub reports: Vec<Report>,
    #[serde(default)]
    pub mod_log: Vec<ModLogEntry>,
}

impl Data {
    fn with_defaults() -> Self {
        let ch = |name: &str| Channel { id: Uuid::new_v4(), name: name.into() };
        Data {
            // Servidor da instância: público e sem dono (moderado pelos administradores).
            servers: vec![Server {
                id: Uuid::new_v4(),
                name: "Concord".into(),
                channels: vec![ch("geral"), ch("off-topic")],
                owner_id: None,
                public: true,
                members: vec![],
                moderators: vec![],
                bans: vec![],
                invite_code: new_invite_code(),
            }],
            ..Default::default()
        }
    }

    pub fn server(&self, sid: Uuid) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == sid)
    }

    /// Índice do servidor que contém o canal.
    pub fn server_index_of_channel(&self, channel_id: Uuid) -> Option<usize> {
        self.servers
            .iter()
            .position(|s| s.channels.iter().any(|c| c.id == channel_id))
    }

    pub fn user_name(&self, uid: Uuid) -> String {
        self.users.get(&uid).map(|u| u.name.clone()).unwrap_or_else(|| "Desconhecido".into())
    }

    /// Remove uma mensagem e as denúncias sobre ela. Devolve o autor, se existia.
    pub fn remove_message(&mut self, channel_id: Uuid, message_id: Uuid) -> Option<Uuid> {
        let list = self.messages.get_mut(&channel_id)?;
        let pos = list.iter().position(|m| m.id == message_id)?;
        let author = list.remove(pos).author.id;
        self.reports.retain(|r| r.message_id != message_id);
        Some(author)
    }

    pub fn log(&mut self, server_id: Uuid, actor: Option<Uuid>, action: &str, target: Option<Uuid>, detail: &str) {
        self.mod_log.push(ModLogEntry {
            at: now_ms(),
            server_id,
            actor,
            action: action.into(),
            target,
            detail: detail.into(),
        });
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
                        Some(User { id: *id, name: u.name.clone(), color: u.color.clone() })
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

    pub fn reports_view(&self, sid: Uuid) -> Vec<ReportView> {
        let Some(server) = self.server(sid) else { return vec![] };
        self.reports
            .iter()
            .filter(|r| r.server_id == sid)
            .filter_map(|r| {
                let message = self.messages.get(&r.channel_id)?.iter().find(|m| m.id == r.message_id)?;
                let channel_name = server.channels.iter().find(|c| c.id == r.channel_id)?.name.clone();
                Some(ReportView {
                    id: r.id,
                    channel_id: r.channel_id,
                    channel_name,
                    message: message.clone(),
                    category: r.category.clone(),
                    details: r.details.clone(),
                    created_at: r.created_at,
                })
            })
            .collect()
    }
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

/// Resultado da eliminação dos dados de um titular.
pub struct Deletion {
    pub messages_removed: usize,
    pub servers_changed: Vec<Uuid>,
}

pub struct AppState {
    pub config: Config,
    pub data: Mutex<Data>,
    /// Conexões abertas: id da conexão -> usuário. Só em memória.
    pub online: Mutex<HashMap<Uuid, User>>,
    pub connections: AtomicUsize,
    pub tx: broadcast::Sender<Event>,
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

    pub fn emit(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }

    pub fn send_all(&self, msg: ServerMsg) {
        self.emit(Event::Send(Audience::All, msg));
    }

    pub fn is_admin(&self, uid: Uuid) -> bool {
        self.config.admins.contains(&uid)
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

    // ---------- filtros por conexão ----------

    pub fn audience_includes(&self, aud: &Audience, uid: Uuid) -> bool {
        match aud {
            Audience::All => true,
            Audience::User(id) => *id == uid,
            Audience::Server(sid) => {
                self.data.lock().unwrap().server(*sid).is_some_and(|s| s.can_view(uid))
            }
        }
    }

    pub fn server_view_for(&self, sid: Uuid, uid: Uuid) -> Option<ServerView> {
        let admin = self.is_admin(uid);
        let data = self.data.lock().unwrap();
        let s = data.server(sid)?;
        s.can_view(uid).then(|| data.server_view(s, uid, admin))
    }

    pub fn views_for(&self, uid: Uuid) -> Vec<ServerView> {
        let admin = self.is_admin(uid);
        let data = self.data.lock().unwrap();
        data.servers
            .iter()
            .filter(|s| s.can_view(uid))
            .map(|s| data.server_view(s, uid, admin))
            .collect()
    }

    pub fn reports_for(&self, sid: Uuid, uid: Uuid) -> Option<Vec<ReportView>> {
        let admin = self.is_admin(uid);
        let data = self.data.lock().unwrap();
        let s = data.server(sid)?;
        s.can_moderate(uid, admin).then(|| data.reports_view(sid))
    }

    // ---------- titular ----------

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
        let uid = user.id;
        let data = self.data.lock().unwrap();
        let server_name = |sid: Uuid| data.server(sid).map(|s| s.name.clone()).unwrap_or_else(|| "(apagado)".into());
        let mut where_is: HashMap<Uuid, (&str, &str)> = HashMap::new();
        for s in &data.servers {
            for c in &s.channels {
                where_is.insert(c.id, (&s.name, &c.name));
            }
        }
        let mut msgs: Vec<&Message> =
            data.messages.values().flatten().filter(|m| m.author.id == uid).collect();
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
        let servidores: Vec<_> = data
            .servers
            .iter()
            .filter(|s| s.is_member(uid) || s.moderators.contains(&uid))
            .map(|s| json!({ "nome": s.name, "papel": s.role(uid), "publico": s.public }))
            .collect();
        let denuncias: Vec<_> = data
            .reports
            .iter()
            .filter(|r| r.reporter_id == uid)
            .map(|r| json!({
                "servidor": server_name(r.server_id),
                "mensagem_id": r.message_id,
                "categoria": r.category,
                "detalhes": r.details,
                "feita_em": iso_utc(r.created_at),
            }))
            .collect();
        let banimentos: Vec<_> = data
            .servers
            .iter()
            .flat_map(|s| s.bans.iter().filter(|b| b.user_id == uid).map(move |b| (s, b)))
            .map(|(s, b)| json!({ "servidor": s.name, "motivo": b.reason, "em": iso_utc(b.at) }))
            .collect();
        let moderacao: Vec<_> = data
            .mod_log
            .iter()
            .filter(|e| e.actor == Some(uid) || e.target == Some(uid))
            .map(|e| json!({
                "servidor": server_name(e.server_id),
                "acao": e.action,
                "meu_papel": if e.actor == Some(uid) { "autor da ação" } else { "alvo da ação" },
                "detalhe": e.detail,
                "em": iso_utc(e.at),
            }))
            .collect();
        let rec = data.users.get(&uid);
        json!({
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
        })
    }

    /// Eliminação (LGPD art. 18, VI): apaga mensagens, denúncias, papéis,
    /// banimentos e o registro do titular, e anonimiza o registro de moderação.
    pub fn delete_user_data(&self, uid: Uuid) -> Deletion {
        let mut data = self.data.lock().unwrap();
        let mut removed_ids = HashSet::new();
        for list in data.messages.values_mut() {
            list.retain(|m| {
                let keep = m.author.id != uid;
                if !keep {
                    removed_ids.insert(m.id);
                }
                keep
            });
        }
        data.messages.retain(|_, l| !l.is_empty());
        data.reports
            .retain(|r| r.reporter_id != uid && !removed_ids.contains(&r.message_id));

        let mut servers_changed = vec![];
        for s in data.servers.iter_mut() {
            let mut changed = false;
            if s.owner_id == Some(uid) {
                // Passa o servidor adiante: primeiro moderador, senão o membro mais antigo.
                let heir = s.moderators.iter().chain(s.members.iter()).find(|&&m| m != uid).copied();
                s.owner_id = heir;
                changed = true;
            }
            let before = (s.members.len(), s.moderators.len(), s.bans.len());
            s.members.retain(|&m| m != uid && Some(m) != s.owner_id);
            if let Some(o) = s.owner_id {
                s.moderators.retain(|&m| m != o);
            }
            s.moderators.retain(|&m| m != uid);
            s.bans.retain(|b| b.user_id != uid);
            if changed || before != (s.members.len(), s.moderators.len(), s.bans.len()) {
                servers_changed.push(s.id);
            }
        }
        for e in data.mod_log.iter_mut() {
            if e.actor == Some(uid) {
                e.actor = None;
            }
            if e.target == Some(uid) {
                e.target = None;
            }
        }
        data.users.remove(&uid);
        drop(data);
        self.mark_dirty();
        self.save_if_dirty(); // grava já, sem esperar o ciclo
        Deletion { messages_removed: removed_ids.len(), servers_changed }
    }

    /// Término do tratamento (LGPD art. 15 e 16): remove o que passou do prazo.
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

        let existing: HashSet<Uuid> = data.messages.values().flatten().map(|m| m.id).collect();
        let before_reports = data.reports.len();
        data.reports.retain(|r| r.created_at >= cutoff && existing.contains(&r.message_id));
        let removed_reports = before_reports - data.reports.len();

        data.mod_log.retain(|e| e.at >= cutoff);
        let mut removed_bans = 0;
        for s in data.servers.iter_mut() {
            let before = s.bans.len();
            s.bans.retain(|b| b.at >= cutoff);
            removed_bans += before - s.bans.len();
        }

        let authors: HashSet<Uuid> = data.messages.values().flatten().map(|m| m.author.id).collect();
        let before_users = data.users.len();
        data.users
            .retain(|id, u| u.last_seen >= cutoff || authors.contains(id) || online.contains(id));
        let removed_users = before_users - data.users.len();
        drop(data);

        if removed_msgs + removed_reports + removed_bans + removed_users > 0 {
            println!(
                "Retenção: removidos {removed_msgs} mensagem(ns), {removed_reports} denúncia(s), \
                 {removed_bans} banimento(s) e {removed_users} registro(s) expirados"
            );
            self.mark_dirty();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(name: &str) -> User {
        User { id: Uuid::new_v4(), name: name.into(), color: "#000000".into() }
    }

    fn record(u: &User) -> UserRecord {
        UserRecord { name: u.name.clone(), color: u.color.clone(), consent_version: 1, consented_at: 0, last_seen: 0 }
    }

    #[test]
    fn datas_iso() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_793_926_220_566), "2026-11-06T00:50:20Z");
    }

    #[test]
    fn renomear_atualiza_mensagens_antigas() {
        let old = user("Antigo");
        let ch = Uuid::new_v4();
        let mut data = Data::default();
        data.users.insert(old.id, record(&old));
        data.messages.insert(
            ch,
            vec![Message { id: Uuid::new_v4(), channel_id: ch, author: old.clone(), content: "oi".into(), timestamp: 0 }],
        );
        let new = User { id: old.id, name: "Novo".into(), color: "#ffffff".into() };
        assert!(apply_profile(&mut data, &new));
        assert_eq!(data.messages[&ch][0].author.name, "Novo");
        assert!(!apply_profile(&mut data, &new), "sem mudança não deve sinalizar");
    }

    #[test]
    fn permissoes_de_servidor_privado() {
        let (dono, membro, mod_, estranho) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
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
        // Admin da instância não enxerga servidor privado do qual não participa.
        assert!(!s.can_moderate(estranho, true));
        s.bans.push(crate::model::Ban { user_id: membro, reason: "x".into(), at: 0 });
        assert!(!s.can_view(membro));
        assert_eq!(new_invite_code().len(), 12);
    }
}
