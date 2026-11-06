//! Tipos de dados e protocolo WebSocket (JSON) entre cliente e servidor.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Perfil público. Não existe cadastro: o `id` é pseudônimo, derivado
/// (SHA-256) de um segredo que só o navegador do titular conhece.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub name: String,
}

fn default_true() -> bool {
    true
}

/// Código de convite aleatório (~70 bits), sem caracteres ambíguos.
pub fn new_invite_code() -> String {
    const ALPHA: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let b = *Uuid::new_v4().as_bytes();
    // Bytes 6 e 8 de um UUID v4 têm bits fixos; usa só os aleatórios.
    [0, 1, 2, 3, 4, 5, 10, 11, 12, 13, 14, 15]
        .iter()
        .map(|&i| ALPHA[b[i] as usize % ALPHA.len()] as char)
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ban {
    pub user_id: Uuid,
    pub reason: String,
    pub at: i64,
}

/// Servidor como fica guardado. Nunca é enviado inteiro ao cliente
/// (membros, banidos e convite são restritos): veja `ServerView`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Server {
    pub id: Uuid,
    pub name: String,
    pub channels: Vec<Channel>,
    /// `None` = servidor da instância, moderado pelos administradores.
    #[serde(default)]
    pub owner_id: Option<Uuid>,
    /// Público: qualquer pessoa da instância vê e participa. Privado: só com convite.
    #[serde(default = "default_true")]
    pub public: bool,
    #[serde(default)]
    pub members: Vec<Uuid>,
    #[serde(default)]
    pub moderators: Vec<Uuid>,
    #[serde(default)]
    pub bans: Vec<Ban>,
    #[serde(default = "new_invite_code")]
    pub invite_code: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Moderator,
    Member,
    Visitor,
}

impl Server {
    pub fn is_banned(&self, uid: Uuid) -> bool {
        self.bans.iter().any(|b| b.user_id == uid)
    }

    pub fn is_member(&self, uid: Uuid) -> bool {
        self.owner_id == Some(uid) || self.members.contains(&uid)
    }

    pub fn can_view(&self, uid: Uuid) -> bool {
        !self.is_banned(uid) && (self.public || self.is_member(uid))
    }

    pub fn role(&self, uid: Uuid) -> Role {
        if self.owner_id == Some(uid) {
            Role::Owner
        } else if self.moderators.contains(&uid) {
            Role::Moderator
        } else if self.is_member(uid) {
            Role::Member
        } else {
            Role::Visitor
        }
    }

    /// Apagar mensagens alheias, banir, criar/apagar canais, ver denúncias.
    pub fn can_moderate(&self, uid: Uuid, admin: bool) -> bool {
        self.can_view(uid) && (admin || matches!(self.role(uid), Role::Owner | Role::Moderator))
    }

    /// Renomear, mudar visibilidade, apagar o servidor, nomear moderadores.
    pub fn can_manage(&self, uid: Uuid, admin: bool) -> bool {
        self.can_view(uid) && (admin || self.owner_id == Some(uid))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BanView {
    pub user_id: Uuid,
    pub name: String,
    pub reason: String,
    pub at: i64,
}

/// O que cada pessoa vê de um servidor, de acordo com o próprio papel.
#[derive(Clone, Debug, Serialize)]
pub struct ServerView {
    pub id: Uuid,
    pub name: String,
    pub channels: Vec<Channel>,
    pub public: bool,
    pub owner_id: Option<Uuid>,
    pub moderators: Vec<Uuid>,
    /// Só em servidores privados: quem participa (para a lista online/offline).
    /// Em públicos a lista é quem está online na instância.
    pub members: Option<Vec<User>>,
    pub role: Role,
    pub can_moderate: bool,
    pub can_manage: bool,
    /// Só para quem modera.
    pub invite_code: Option<String>,
    /// Só para quem modera.
    pub bans: Option<Vec<BanView>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub author: User,
    pub content: String,
    /// Milissegundos desde a época Unix.
    pub timestamp: i64,
}

pub const REPORT_CATEGORIES: &[&str] =
    &["spam", "assedio", "odio", "dados_pessoais", "ilegal", "outro"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub id: Uuid,
    pub server_id: Uuid,
    pub channel_id: Uuid,
    pub message_id: Uuid,
    /// Guardado só para evitar denúncia duplicada e para exportação/eliminação.
    /// Nunca é mostrado aos moderadores.
    pub reporter_id: Uuid,
    pub category: String,
    pub details: String,
    pub created_at: i64,
}

/// Denúncia como os moderadores veem: sem quem denunciou.
#[derive(Clone, Debug, Serialize)]
pub struct ReportView {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub channel_name: String,
    pub message: Message,
    pub category: String,
    pub details: String,
    pub created_at: i64,
}

/// Registro de ações de moderação (responsabilização, LGPD art. 6º, X).
/// Não guarda o conteúdo das mensagens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModLogEntry {
    pub at: i64,
    pub server_id: Uuid,
    pub actor: Option<Uuid>,
    pub action: String,
    pub target: Option<Uuid>,
    pub detail: String,
}

/// Mensagens que o cliente envia.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Primeira mensagem da conexão. `consent_version` é a versão da
    /// política de privacidade que o titular aceitou.
    /// Enviada de novo durante a conexão só para trocar apelido/cor.
    Hello { secret: String, name: String, color: String, consent_version: u32 },
    CreateServer {
        name: String,
        #[serde(default)]
        public: bool,
    },
    UpdateServer { server_id: Uuid, name: Option<String>, public: Option<bool> },
    DeleteServer { server_id: Uuid },
    RegenerateInvite { server_id: Uuid },
    JoinServer { code: String },
    LeaveServer { server_id: Uuid },
    CreateChannel { server_id: Uuid, name: String },
    DeleteChannel { channel_id: Uuid },
    SetModerator { server_id: Uuid, user_id: Uuid, enabled: bool },
    BanUser {
        server_id: Uuid,
        user_id: Uuid,
        reason: String,
        #[serde(default)]
        delete_messages: bool,
    },
    UnbanUser { server_id: Uuid, user_id: Uuid },
    SendMessage { channel_id: Uuid, content: String },
    /// Sem `before`: a página mais recente. Com `before`: mensagens anteriores a ela.
    FetchHistory {
        channel_id: Uuid,
        #[serde(default)]
        before: Option<Uuid>,
    },
    Typing { channel_id: Uuid },
    DeleteMessage { channel_id: Uuid, message_id: Uuid },
    ReportMessage {
        channel_id: Uuid,
        message_id: Uuid,
        category: String,
        #[serde(default)]
        details: String,
    },
    FetchReports { server_id: Uuid },
    ResolveReport { report_id: Uuid, delete_message: bool },
    /// LGPD art. 18, II e V: acesso e portabilidade.
    ExportMyData,
    /// LGPD art. 18, VI e IX: eliminação e revogação do consentimento.
    DeleteMyData,
}

/// Mensagens que o servidor envia.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Ready { me: User, servers: Vec<ServerView>, online: Vec<User>, is_admin: bool },
    /// O titular ainda não aceitou a versão atual da política.
    ConsentRequired { version: u32, retention_days: u32 },
    /// Servidor novo ou alterado, já filtrado para quem recebe.
    ServerUpdated { server: ServerView },
    /// O servidor sumiu para quem recebe (apagado, banido, saiu, ficou privado).
    ServerRemoved { server_id: Uuid },
    /// Sugestão para o cliente abrir um servidor/canal (após criar ou entrar).
    Focus { server_id: Uuid, channel_id: Option<Uuid> },
    Message { message: Message },
    MessageDeleted { channel_id: Uuid, message_id: Uuid },
    /// Mensagens de alguém apagadas num servidor (ex.: banimento).
    MessagesPurged { server_id: Uuid, user_id: Uuid },
    History { channel_id: Uuid, messages: Vec<Message>, before: Option<Uuid>, has_more: bool },
    Presence { online: Vec<User> },
    Typing { channel_id: Uuid, user: User },
    Reports { server_id: Uuid, reports: Vec<ReportView> },
    MyData { data: serde_json::Value },
    DataDeleted { messages_removed: usize },
    /// Um titular apagou seus dados: clientes removem as mensagens dele da tela.
    UserPurged { user_id: Uuid },
    /// Um titular mudou apelido/cor: clientes atualizam as mensagens antigas.
    ProfileUpdated { user: User },
    Info { message: String },
    Error { message: String },
}

/// Quem deve receber um evento.
#[derive(Clone, Debug)]
pub enum Audience {
    All,
    /// Quem pode ver o servidor.
    Server(Uuid),
    User(Uuid),
}

/// O que circula entre as conexões. Cada conexão filtra e monta
/// a própria visão antes de enviar ao navegador.
#[derive(Clone, Debug)]
pub enum Event {
    Send(Audience, ServerMsg),
    ServerChanged(Uuid),
    ServerDeleted(Uuid),
    ReportsChanged(Uuid),
}
