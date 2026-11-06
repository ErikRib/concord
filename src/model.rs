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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Server {
    pub id: Uuid,
    pub name: String,
    pub channels: Vec<Channel>,
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

/// Mensagens que o cliente envia.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Primeira mensagem da conexão. `consent_version` é a versão da
    /// política de privacidade que o titular aceitou.
    /// Enviada de novo durante a conexão só para trocar apelido/cor.
    Hello { secret: String, name: String, color: String, consent_version: u32 },
    CreateServer { name: String },
    CreateChannel { server_id: Uuid, name: String },
    SendMessage { channel_id: Uuid, content: String },
    FetchHistory { channel_id: Uuid },
    Typing { channel_id: Uuid },
    DeleteMessage { channel_id: Uuid, message_id: Uuid },
    /// LGPD art. 18, II e V: acesso e portabilidade.
    ExportMyData,
    /// LGPD art. 18, VI e IX: eliminação e revogação do consentimento.
    DeleteMyData,
}

/// Mensagens que o servidor envia.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Ready { me: User, servers: Vec<Server>, online: Vec<User> },
    /// O titular ainda não aceitou a versão atual da política.
    ConsentRequired { version: u32, retention_days: u32 },
    ServerCreated { server: Server },
    ChannelCreated { server_id: Uuid, channel: Channel },
    Message { message: Message },
    MessageDeleted { channel_id: Uuid, message_id: Uuid },
    History { channel_id: Uuid, messages: Vec<Message> },
    Presence { online: Vec<User> },
    Typing { channel_id: Uuid, user: User },
    MyData { data: serde_json::Value },
    DataDeleted { messages_removed: usize },
    /// Um titular apagou seus dados: clientes removem as mensagens dele da tela.
    UserPurged { user_id: Uuid },
    /// Um titular mudou apelido/cor: clientes atualizam as mensagens antigas.
    ProfileUpdated { user: User },
    Error { message: String },
}
