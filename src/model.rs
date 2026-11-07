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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    #[default]
    Text,
    Voice,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Text => "text",
            ChannelKind::Voice => "voice",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "voice" {
            ChannelKind::Voice
        } else {
            ChannelKind::Text
        }
    }
}

fn default_bitrate() -> u32 {
    64
}

fn default_quality() -> String {
    "auto".into()
}

pub const VIDEO_QUALITIES: &[&str] = &["auto", "720p"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub kind: ChannelKind,
    /// Voz: limite de pessoas (0 = sem limite).
    #[serde(default)]
    pub user_limit: u32,
    /// Voz: taxa de bits do áudio em kbps.
    #[serde(default = "default_bitrate")]
    pub bitrate: u32,
    /// Voz: qualidade da câmera ("auto" ou "720p").
    #[serde(default = "default_quality")]
    pub video_quality: String,
    /// Voz: status do canal (ex.: "jogando X"). Some quando o canal esvazia.
    #[serde(default)]
    pub status: Option<String>,
    /// Quem definiu o status (para exportação e eliminação). Nunca vai ao cliente.
    #[serde(skip)]
    pub status_by: Option<Uuid>,
}

impl Channel {
    pub fn new(name: &str, kind: ChannelKind) -> Self {
        Channel {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            user_limit: 0,
            bitrate: default_bitrate(),
            video_quality: default_quality(),
            status: None,
            status_by: None,
        }
    }
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
    /// Última edição (só a versão atual é guardada).
    #[serde(default)]
    pub edited_at: Option<i64>,
    /// Mensagem respondida (prévia montada na leitura).
    #[serde(default)]
    pub reply_to: Option<ReplyPreview>,
    #[serde(default)]
    pub reactions: Vec<ReactionView>,
    #[serde(default)]
    pub attachments: Vec<AttachmentView>,
}

impl Message {
    pub fn new(id: Uuid, channel_id: Uuid, author: User, content: String, timestamp: i64) -> Self {
        Message {
            id,
            channel_id,
            author,
            content,
            timestamp,
            edited_at: None,
            reply_to: None,
            reactions: vec![],
            attachments: vec![],
        }
    }
}

/// Prévia da mensagem respondida. `author` nulo: a original foi apagada.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplyPreview {
    pub id: Uuid,
    pub author: Option<User>,
    pub excerpt: String,
}

/// Uma reação e quem reagiu (ids pseudônimos).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionView {
    pub emoji: String,
    pub users: Vec<Uuid>,
}

/// Imagem anexada. `url` é assinada e expira (o arquivo nunca fica público).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentView {
    pub id: Uuid,
    pub url: String,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub width: u32,
    pub height: u32,
}

/// Som do Soundpad de um servidor. `url` é assinada e expira.
#[derive(Clone, Debug, Serialize)]
pub struct SoundView {
    pub id: Uuid,
    pub name: String,
    pub emoji: Option<String>,
    pub duration_ms: u32,
    pub url: String,
}

/// Sons que já vêm com o Concord (sintetizados no navegador; nada é enviado).
pub const BUILTIN_SOUNDS: &[&str] = &[
    "badumtss", "buzina", "trombone", "tada", "moeda", "sino", "grilos", "palmas", "laser", "boing",
];

pub const REPORT_CATEGORIES: &[&str] = &[
    "spam",
    "assedio",
    "odio",
    "dados_pessoais",
    "ilegal",
    "outro",
];

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
    Hello {
        secret: String,
        name: String,
        color: String,
        consent_version: u32,
    },
    CreateServer {
        name: String,
        #[serde(default)]
        public: bool,
    },
    UpdateServer {
        server_id: Uuid,
        name: Option<String>,
        public: Option<bool>,
    },
    DeleteServer {
        server_id: Uuid,
    },
    RegenerateInvite {
        server_id: Uuid,
    },
    JoinServer {
        code: String,
    },
    LeaveServer {
        server_id: Uuid,
    },
    CreateChannel {
        server_id: Uuid,
        name: String,
        #[serde(default)]
        kind: ChannelKind,
    },
    UpdateChannel {
        channel_id: Uuid,
        name: Option<String>,
        user_limit: Option<u32>,
        bitrate: Option<u32>,
        video_quality: Option<String>,
    },
    DeleteChannel {
        channel_id: Uuid,
    },
    /// Pede para entrar num canal de voz (o servidor devolve um token do LiveKit).
    VoiceJoin {
        channel_id: Uuid,
    },
    /// `channel_id`: só sai se ainda estiver nesse canal (evita corrida ao ser movido).
    VoiceLeave {
        #[serde(default)]
        channel_id: Option<Uuid>,
    },
    /// Moderação de voz: "mute", "unmute", "deafen", "undeafen" ou "disconnect".
    VoiceModerate {
        server_id: Uuid,
        user_id: Uuid,
        action: String,
    },
    /// Moderação de voz: move alguém para outro canal de voz do mesmo servidor.
    VoiceMove {
        user_id: Uuid,
        channel_id: Uuid,
    },
    /// Estado visível para os outros na barra lateral.
    VoiceUpdate {
        muted: bool,
        deafened: bool,
        streaming: bool,
        camera: bool,
    },
    /// Status do canal de voz; texto vazio limpa.
    SetVoiceStatus {
        channel_id: Uuid,
        status: String,
    },
    SetModerator {
        server_id: Uuid,
        user_id: Uuid,
        enabled: bool,
    },
    BanUser {
        server_id: Uuid,
        user_id: Uuid,
        reason: String,
        #[serde(default)]
        delete_messages: bool,
    },
    UnbanUser {
        server_id: Uuid,
        user_id: Uuid,
    },
    SendMessage {
        channel_id: Uuid,
        content: String,
        #[serde(default)]
        reply_to: Option<Uuid>,
        /// Ids de imagens já enviadas por `UploadBegin` + POST /anexos.
        #[serde(default)]
        attachments: Vec<Uuid>,
    },
    EditMessage {
        channel_id: Uuid,
        message_id: Uuid,
        content: String,
    },
    /// Liga (`on`) ou desliga uma reação minha.
    React {
        channel_id: Uuid,
        message_id: Uuid,
        emoji: String,
        on: bool,
    },
    /// Pede autorização para enviar uma imagem (devolve um token de uso único).
    UploadBegin {
        channel_id: Uuid,
        size: u64,
    },
    /// GIF colado: o servidor busca a imagem original no endereço (https).
    UploadFromUrl {
        channel_id: Uuid,
        url: String,
        request_id: String,
    },
    /// Autorização para POST /sons/{upload_id} (moderação do servidor).
    SoundUploadBegin {
        server_id: Uuid,
        size: u64,
        name: String,
        #[serde(default)]
        emoji: Option<String>,
    },
    FetchSounds {
        server_id: Uuid,
    },
    /// `sound`: id de um som do servidor ou `builtin:<nome>`. Só para quem está na voz.
    PlaySound {
        sound: String,
    },
    DeleteSound {
        sound_id: Uuid,
    },
    /// Sem `before`: a página mais recente. Com `before`: mensagens anteriores a ela.
    FetchHistory {
        channel_id: Uuid,
        #[serde(default)]
        before: Option<Uuid>,
    },
    Typing {
        channel_id: Uuid,
    },
    DeleteMessage {
        channel_id: Uuid,
        message_id: Uuid,
    },
    ReportMessage {
        channel_id: Uuid,
        message_id: Uuid,
        category: String,
        #[serde(default)]
        details: String,
    },
    FetchReports {
        server_id: Uuid,
    },
    ResolveReport {
        report_id: Uuid,
        delete_message: bool,
    },
    /// LGPD art. 18, II e V: acesso e portabilidade.
    ExportMyData,
    /// LGPD art. 18, VI e IX: eliminação e revogação do consentimento.
    DeleteMyData,
}

/// Mensagens que o servidor envia.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Ready {
        me: User,
        servers: Vec<ServerView>,
        online: Vec<User>,
        is_admin: bool,
        voice_enabled: bool,
    },
    /// O titular ainda não aceitou a versão atual da política.
    ConsentRequired {
        version: u32,
        retention_days: u32,
    },
    /// Servidor novo ou alterado, já filtrado para quem recebe.
    ServerUpdated {
        server: ServerView,
    },
    /// O servidor sumiu para quem recebe (apagado, banido, saiu, ficou privado).
    ServerRemoved {
        server_id: Uuid,
    },
    /// Sugestão para o cliente abrir um servidor/canal (após criar ou entrar).
    Focus {
        server_id: Uuid,
        channel_id: Option<Uuid>,
    },
    Message {
        message: Message,
    },
    MessageEdited {
        channel_id: Uuid,
        message_id: Uuid,
        content: String,
        edited_at: i64,
    },
    ReactionUpdate {
        channel_id: Uuid,
        message_id: Uuid,
        emoji: String,
        user_id: Uuid,
        on: bool,
    },
    /// Autorização para POST /anexos/{upload_id}?token=… (vale 2 minutos, uso único).
    UploadToken {
        upload_id: Uuid,
        token: String,
        max_bytes: u64,
        /// "image" ou "sound".
        kind: &'static str,
    },
    /// Resultado de `UploadFromUrl`: o anexo pendente já está guardado.
    UrlUpload {
        request_id: String,
        ok: bool,
        id: Option<Uuid>,
        name: String,
        mime: String,
        size: usize,
        width: u32,
        height: u32,
        error: Option<String>,
    },
    Sounds {
        server_id: Uuid,
        sounds: Vec<SoundView>,
        /// Quem pode adicionar e apagar sons neste servidor.
        can_manage: bool,
        max: usize,
    },
    /// A lista de sons mudou: quem a abriu busca de novo.
    SoundsChanged {
        server_id: Uuid,
    },
    /// Alguém tocou um som num canal de voz.
    SoundPlayed {
        server_id: Uuid,
        channel_id: Uuid,
        user_id: Uuid,
        sound: String,
    },
    MessageDeleted {
        channel_id: Uuid,
        message_id: Uuid,
    },
    /// Mensagens de alguém apagadas num servidor (ex.: banimento).
    MessagesPurged {
        server_id: Uuid,
        user_id: Uuid,
    },
    History {
        channel_id: Uuid,
        messages: Vec<Message>,
        before: Option<Uuid>,
        has_more: bool,
    },
    Presence {
        online: Vec<User>,
    },
    Typing {
        channel_id: Uuid,
        user: User,
    },
    Reports {
        server_id: Uuid,
        reports: Vec<ReportView>,
    },
    MyData {
        data: serde_json::Value,
    },
    DataDeleted {
        messages_removed: usize,
    },
    /// Um titular apagou seus dados: clientes removem as mensagens dele da tela.
    UserPurged {
        user_id: Uuid,
    },
    /// Um titular mudou apelido/cor: clientes atualizam as mensagens antigas.
    ProfileUpdated {
        user: User,
    },
    /// Autorização para entrar na sala do LiveKit. `url` nulo: o cliente usa
    /// o LiveKit local na porta 7880 do mesmo host.
    VoiceToken {
        server_id: Uuid,
        channel_id: Uuid,
        url: Option<String>,
        /// Porta do LiveKit local (usada quando `url` é nulo).
        port: u16,
        token: String,
        bitrate: u32,
        video_quality: String,
        /// Enviado porque um moderador moveu a pessoa (só a aba que está na chamada usa).
        #[serde(default)]
        moved: bool,
    },
    /// Quem está em cada canal de voz do servidor.
    VoiceStates {
        server_id: Uuid,
        states: Vec<VoiceStateView>,
    },
    /// A sessão de voz desta pessoa foi encerrada pelo servidor (ex.: canal apagado).
    VoiceEnded {
        reason: String,
    },
    Info {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct VoiceStateView {
    pub channel_id: Uuid,
    pub user: User,
    pub muted: bool,
    pub deafened: bool,
    pub streaming: bool,
    pub camera: bool,
    /// Silenciado/ensurdecido por um moderador.
    pub server_muted: bool,
    pub server_deafened: bool,
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
// Eventos vivem pouco (só o tempo de repassar às conexões); não vale alocar.
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Send(Audience, ServerMsg),
    ServerChanged(Uuid),
    ServerDeleted(Uuid),
    ReportsChanged(Uuid),
}
