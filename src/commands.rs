//! Comandos enviados pelos clientes. Toda permissão é verificada aqui,
//! no servidor; o cliente só esconde botões por conveniência.

use uuid::Uuid;

use crate::model::{
    new_invite_code, Audience, Ban, Channel, ClientMsg, Event, Message, Report, Server, ServerMsg,
    User, REPORT_CATEGORIES,
};
use crate::state::{
    now_ms, AppState, Data, HISTORY_SIZE, MAX_CHANNELS_PER_SERVER, MAX_MEMBERS_PER_SERVER,
    MAX_OWNED_SERVERS, MAX_REPORTS_PER_SERVER, MAX_SERVERS, MAX_STORED_PER_CHANNEL,
};
use crate::ws::Out;

type R = Result<(), String>;

pub struct Ctx<'a> {
    pub state: &'a AppState,
    pub me: &'a mut User,
    pub conn_id: Uuid,
    pub out: &'a Out,
}

impl Ctx<'_> {
    fn send(&self, msg: ServerMsg) {
        let _ = self.out.send(msg);
    }
}

const NO_PERMISSION: &str = "Você não tem permissão para isso";

/// Trava os dados, acha o servidor e executa `f`. Marca os dados como alterados.
fn with_server<T>(st: &AppState, sid: Uuid, f: impl FnOnce(&mut Data, usize) -> Result<T, String>) -> Result<T, String> {
    let mut data = st.data.lock().unwrap();
    let i = data.servers.iter().position(|s| s.id == sid).ok_or("Servidor não encontrado")?;
    let r = f(&mut data, i)?;
    drop(data);
    st.mark_dirty();
    Ok(r)
}

/// Igual a `with_server`, a partir de um canal. Passa (dados, índice do servidor).
fn with_channel<T>(st: &AppState, cid: Uuid, f: impl FnOnce(&mut Data, usize) -> Result<T, String>) -> Result<T, String> {
    let mut data = st.data.lock().unwrap();
    let i = data.server_index_of_channel(cid).ok_or("Canal não encontrado")?;
    let r = f(&mut data, i)?;
    drop(data);
    st.mark_dirty();
    Ok(r)
}

pub fn handle(cmd: ClientMsg, cx: &mut Ctx) -> R {
    let st = cx.state;
    let uid = cx.me.id;
    let admin = st.is_admin(uid);

    match cmd {
        // Troca de apelido/cor (LGPD art. 18, III: correção). O id não muda.
        ClientMsg::Hello { name, color, .. } => {
            cx.me.name = clean_name(&name);
            cx.me.color = clean_color(&color);
            st.online.lock().unwrap().insert(cx.conn_id, cx.me.clone());
            if st.update_profile(cx.me) {
                st.send_all(ServerMsg::ProfileUpdated { user: cx.me.clone() });
            }
            st.send_all(ServerMsg::Presence { online: st.online_users() });
        }

        // ---------- servidores ----------
        ClientMsg::CreateServer { name, public } => {
            let name = valid_server_name(&name)?;
            let channel = Channel { id: Uuid::new_v4(), name: "geral".into() };
            let server = Server {
                id: Uuid::new_v4(),
                name,
                channels: vec![channel.clone()],
                owner_id: Some(uid),
                public,
                members: vec![uid],
                moderators: vec![],
                bans: vec![],
                invite_code: new_invite_code(),
            };
            let sid = server.id;
            {
                let mut data = st.data.lock().unwrap();
                if data.servers.len() >= MAX_SERVERS {
                    return Err("Limite de servidores da instância atingido".into());
                }
                if data.servers.iter().filter(|s| s.owner_id == Some(uid)).count() >= MAX_OWNED_SERVERS {
                    return Err(format!("Cada pessoa pode ser dona de até {MAX_OWNED_SERVERS} servidores"));
                }
                data.servers.push(server);
            }
            st.mark_dirty();
            st.emit(Event::ServerChanged(sid));
            cx.send(ServerMsg::Focus { server_id: sid, channel_id: Some(channel.id) });
        }

        ClientMsg::UpdateServer { server_id, name, public } => {
            let name = name.map(|n| valid_server_name(&n)).transpose()?;
            with_server(st, server_id, |d, i| {
                if !d.servers[i].can_manage(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                let s = &mut d.servers[i];
                if let Some(n) = name {
                    s.name = n;
                }
                if let Some(p) = public {
                    s.public = p;
                }
                let detail = format!("público: {}", s.public);
                d.log(server_id, Some(uid), "servidor_alterado", None, &detail);
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
        }

        ClientMsg::DeleteServer { server_id } => {
            with_server(st, server_id, |d, i| {
                if !d.servers[i].can_manage(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                let s = d.servers.remove(i);
                for c in &s.channels {
                    d.messages.remove(&c.id);
                }
                d.reports.retain(|r| r.server_id != server_id);
                d.log(server_id, Some(uid), "servidor_apagado", None, "");
                Ok(())
            })?;
            st.emit(Event::ServerDeleted(server_id));
        }

        ClientMsg::RegenerateInvite { server_id } => {
            with_server(st, server_id, |d, i| {
                if !d.servers[i].can_moderate(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                d.servers[i].invite_code = new_invite_code();
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
        }

        ClientMsg::JoinServer { code } => {
            let code = code.trim();
            let (sid, first_channel) = {
                let mut data = st.data.lock().unwrap();
                let s = data
                    .servers
                    .iter_mut()
                    .find(|s| !code.is_empty() && s.invite_code == code)
                    .ok_or("Convite inválido ou expirado")?;
                if s.is_banned(uid) {
                    return Err("Você não pode entrar neste servidor".into());
                }
                if !s.is_member(uid) {
                    if s.members.len() >= MAX_MEMBERS_PER_SERVER {
                        return Err("Este servidor está cheio".into());
                    }
                    s.members.push(uid);
                }
                (s.id, s.channels.first().map(|c| c.id))
            };
            st.mark_dirty();
            st.emit(Event::ServerChanged(sid));
            cx.send(ServerMsg::Focus { server_id: sid, channel_id: first_channel });
        }

        ClientMsg::LeaveServer { server_id } => {
            with_server(st, server_id, |d, i| {
                let s = &mut d.servers[i];
                if s.owner_id == Some(uid) {
                    return Err("Quem é dono não pode sair. Apague o servidor ou apague seus dados para passá-lo adiante.".into());
                }
                s.members.retain(|&m| m != uid);
                s.moderators.retain(|&m| m != uid);
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
        }

        // ---------- canais ----------
        ClientMsg::CreateChannel { server_id, name } => {
            let name = clean_channel_name(&name);
            if name.is_empty() {
                return Err("Nome de canal inválido".into());
            }
            let channel = Channel { id: Uuid::new_v4(), name };
            let cid = channel.id;
            with_server(st, server_id, |d, i| {
                let s = &mut d.servers[i];
                if !s.can_moderate(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                if s.channels.len() >= MAX_CHANNELS_PER_SERVER {
                    return Err("Limite de canais atingido".into());
                }
                s.channels.push(channel);
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
            cx.send(ServerMsg::Focus { server_id, channel_id: Some(cid) });
        }

        ClientMsg::DeleteChannel { channel_id } => {
            let sid = with_channel(st, channel_id, |d, i| {
                let s = &mut d.servers[i];
                if !s.can_moderate(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                if s.channels.len() <= 1 {
                    return Err("O servidor precisa ter pelo menos um canal".into());
                }
                s.channels.retain(|c| c.id != channel_id);
                let sid = s.id;
                d.messages.remove(&channel_id);
                d.reports.retain(|r| r.channel_id != channel_id);
                d.log(sid, Some(uid), "canal_apagado", None, "");
                Ok(sid)
            })?;
            st.emit(Event::ServerChanged(sid));
            st.emit(Event::ReportsChanged(sid));
        }

        // ---------- papéis e banimentos ----------
        ClientMsg::SetModerator { server_id, user_id, enabled } => {
            with_server(st, server_id, |d, i| {
                if !d.servers[i].can_manage(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                if !d.users.contains_key(&user_id) {
                    return Err("Pessoa não encontrada".into());
                }
                let s = &mut d.servers[i];
                if s.owner_id == Some(user_id) || !s.can_view(user_id) {
                    return Err("Não é possível alterar o papel desta pessoa".into());
                }
                s.moderators.retain(|&m| m != user_id);
                if enabled {
                    s.moderators.push(user_id);
                    if !s.members.contains(&user_id) {
                        s.members.push(user_id);
                    }
                }
                let action = if enabled { "moderador_adicionado" } else { "moderador_removido" };
                d.log(server_id, Some(uid), action, Some(user_id), "");
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
        }

        ClientMsg::BanUser { server_id, user_id, reason, delete_messages } => {
            let reason: String = clean_text(&reason).chars().take(200).collect();
            if reason.is_empty() {
                return Err("Informe o motivo do banimento".into());
            }
            if user_id == uid {
                return Err("Você não pode banir a si mesmo".into());
            }
            if st.is_admin(user_id) {
                return Err("Administradores da instância não podem ser banidos".into());
            }
            let server_name = with_server(st, server_id, |d, i| {
                let s = &d.servers[i];
                if !s.can_moderate(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                if s.owner_id == Some(user_id) {
                    return Err("O dono do servidor não pode ser banido".into());
                }
                // Só quem gerencia pode banir moderadores.
                if s.moderators.contains(&user_id) && !s.can_manage(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                let channels: Vec<Uuid> = s.channels.iter().map(|c| c.id).collect();
                let s = &mut d.servers[i];
                s.members.retain(|&m| m != user_id);
                s.moderators.retain(|&m| m != user_id);
                if !s.is_banned(user_id) {
                    s.bans.push(Ban { user_id, reason: reason.clone(), at: now_ms() });
                }
                let name = s.name.clone();
                if delete_messages {
                    let mut removed = vec![];
                    for c in &channels {
                        if let Some(list) = d.messages.get_mut(c) {
                            list.retain(|m| {
                                let keep = m.author.id != user_id;
                                if !keep {
                                    removed.push(m.id);
                                }
                                keep
                            });
                        }
                    }
                    d.reports.retain(|r| !removed.contains(&r.message_id));
                }
                d.log(server_id, Some(uid), "banimento", Some(user_id), &reason);
                Ok(name)
            })?;
            // Transparência: a pessoa banida fica sabendo onde e por quê.
            st.emit(Event::Send(
                Audience::User(user_id),
                ServerMsg::Info { message: format!("Você foi banido(a) de \"{server_name}\". Motivo: {reason}") },
            ));
            st.emit(Event::ServerChanged(server_id));
            if delete_messages {
                st.emit(Event::Send(
                    Audience::Server(server_id),
                    ServerMsg::MessagesPurged { server_id, user_id },
                ));
                st.emit(Event::ReportsChanged(server_id));
            }
        }

        ClientMsg::UnbanUser { server_id, user_id } => {
            with_server(st, server_id, |d, i| {
                if !d.servers[i].can_moderate(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                d.servers[i].bans.retain(|b| b.user_id != user_id);
                d.log(server_id, Some(uid), "banimento_revogado", Some(user_id), "");
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
        }

        // ---------- mensagens ----------
        ClientMsg::SendMessage { channel_id, content } => {
            let content = content.trim();
            if content.is_empty() {
                return Ok(());
            }
            if content.chars().count() > 2000 {
                return Err("A mensagem passa de 2000 caracteres".into());
            }
            let message = Message {
                id: Uuid::new_v4(),
                channel_id,
                author: cx.me.clone(),
                content: content.to_string(),
                timestamp: now_ms(),
            };
            let sid = with_channel(st, channel_id, |d, i| {
                if !d.servers[i].can_view(uid) {
                    return Err(NO_PERMISSION.into());
                }
                let sid = d.servers[i].id;
                let list = d.messages.entry(channel_id).or_default();
                list.push(message.clone());
                if list.len() > MAX_STORED_PER_CHANNEL {
                    let excess = list.len() - MAX_STORED_PER_CHANNEL;
                    list.drain(..excess);
                }
                Ok(sid)
            })?;
            st.emit(Event::Send(Audience::Server(sid), ServerMsg::Message { message }));
        }

        ClientMsg::FetchHistory { channel_id } => {
            let messages = {
                let data = st.data.lock().unwrap();
                let i = data.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                if !data.servers[i].can_view(uid) {
                    return Err(NO_PERMISSION.into());
                }
                data.messages
                    .get(&channel_id)
                    .map(|l| l[l.len().saturating_sub(HISTORY_SIZE)..].to_vec())
                    .unwrap_or_default()
            };
            cx.send(ServerMsg::History { channel_id, messages });
        }

        ClientMsg::Typing { channel_id } => {
            let sid = {
                let data = st.data.lock().unwrap();
                data.server_index_of_channel(channel_id)
                    .filter(|&i| data.servers[i].can_view(uid))
                    .map(|i| data.servers[i].id)
            };
            if let Some(sid) = sid {
                st.emit(Event::Send(
                    Audience::Server(sid),
                    ServerMsg::Typing { channel_id, user: cx.me.clone() },
                ));
            }
        }

        // O autor apaga a própria mensagem; moderadores apagam qualquer uma.
        ClientMsg::DeleteMessage { channel_id, message_id } => {
            let sid = with_channel(st, channel_id, |d, i| {
                let s = &d.servers[i];
                let sid = s.id;
                let moderate = s.can_moderate(uid, admin);
                let author = d
                    .messages
                    .get(&channel_id)
                    .and_then(|l| l.iter().find(|m| m.id == message_id))
                    .map(|m| m.author.id)
                    .ok_or("Mensagem não encontrada")?;
                if author != uid && !moderate {
                    return Err(NO_PERMISSION.into());
                }
                d.remove_message(channel_id, message_id);
                if author != uid {
                    d.log(sid, Some(uid), "mensagem_apagada", Some(author), "");
                }
                Ok(sid)
            })?;
            st.emit(Event::Send(Audience::Server(sid), ServerMsg::MessageDeleted { channel_id, message_id }));
            st.emit(Event::ReportsChanged(sid));
        }

        // ---------- denúncias ----------
        ClientMsg::ReportMessage { channel_id, message_id, category, details } => {
            if !REPORT_CATEGORIES.contains(&category.as_str()) {
                return Err("Categoria de denúncia inválida".into());
            }
            let details: String = clean_text(&details).chars().take(500).collect();
            let sid = with_channel(st, channel_id, |d, i| {
                let s = &d.servers[i];
                let sid = s.id;
                if !s.can_view(uid) {
                    return Err(NO_PERMISSION.into());
                }
                let author = d
                    .messages
                    .get(&channel_id)
                    .and_then(|l| l.iter().find(|m| m.id == message_id))
                    .map(|m| m.author.id)
                    .ok_or("Mensagem não encontrada")?;
                if author == uid {
                    return Err("Você não pode denunciar a própria mensagem".into());
                }
                if d.reports.iter().any(|r| r.message_id == message_id && r.reporter_id == uid) {
                    return Err("Você já denunciou esta mensagem".into());
                }
                if d.reports.iter().filter(|r| r.server_id == sid).count() >= MAX_REPORTS_PER_SERVER {
                    return Err("Fila de denúncias cheia; tente mais tarde".into());
                }
                d.reports.push(Report {
                    id: Uuid::new_v4(),
                    server_id: sid,
                    channel_id,
                    message_id,
                    reporter_id: uid,
                    category,
                    details,
                    created_at: now_ms(),
                });
                Ok(sid)
            })?;
            cx.send(ServerMsg::Info {
                message: "Denúncia enviada. Os moderadores não ficam sabendo quem denunciou.".into(),
            });
            st.emit(Event::ReportsChanged(sid));
        }

        ClientMsg::FetchReports { server_id } => {
            let reports = st.reports_for(server_id, uid).ok_or(NO_PERMISSION)?;
            cx.send(ServerMsg::Reports { server_id, reports });
        }

        ClientMsg::ResolveReport { report_id, delete_message } => {
            let (sid, deleted) = {
                let mut data = st.data.lock().unwrap();
                let r = data.reports.iter().find(|r| r.id == report_id).cloned().ok_or("Denúncia não encontrada")?;
                let s = data.server(r.server_id).ok_or("Servidor não encontrado")?;
                if !s.can_moderate(uid, admin) {
                    return Err(NO_PERMISSION.into());
                }
                let deleted = if delete_message {
                    let author = data.remove_message(r.channel_id, r.message_id);
                    data.log(r.server_id, Some(uid), "mensagem_apagada_por_denuncia", author, &r.category);
                    author.map(|_| (r.channel_id, r.message_id))
                } else {
                    data.reports.retain(|x| x.id != report_id);
                    data.log(r.server_id, Some(uid), "denuncia_descartada", None, &r.category);
                    None
                };
                (r.server_id, deleted)
            };
            st.mark_dirty();
            if let Some((channel_id, message_id)) = deleted {
                st.emit(Event::Send(Audience::Server(sid), ServerMsg::MessageDeleted { channel_id, message_id }));
            }
            st.emit(Event::ReportsChanged(sid));
        }

        // ---------- LGPD ----------
        ClientMsg::ExportMyData => {
            let data = st.export_user_data(cx.me);
            cx.send(ServerMsg::MyData { data });
        }

        // Tratado na conexão, porque a encerra.
        ClientMsg::DeleteMyData => {}
    }
    Ok(())
}

/// Remove caracteres de controle e espaços nas pontas.
pub fn clean_text(s: &str) -> String {
    s.trim().chars().filter(|c| !c.is_control()).collect()
}

fn valid_server_name(name: &str) -> Result<String, String> {
    let name = clean_text(name);
    if name.is_empty() || name.chars().count() > 50 {
        return Err("O nome do servidor precisa ter de 1 a 50 caracteres".into());
    }
    Ok(name)
}

pub fn clean_name(name: &str) -> String {
    let name: String = clean_text(name).chars().take(32).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        "Anônimo".into()
    } else {
        name
    }
}

pub fn clean_color(color: &str) -> String {
    let ok = color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        color.to_string()
    } else {
        "#5865f2".into()
    }
}

/// "Meu Canal!" -> "meu-canal"
fn clean_channel_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_alphanumeric() || c == '_' {
            out.push(c);
        } else if (c.is_whitespace() || c == '-') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomes() {
        assert_eq!(clean_channel_name("  Meu Canal! "), "meu-canal");
        assert_eq!(clean_channel_name("a ! b"), "a-b");
        assert_eq!(clean_name("\u{0007}  "), "Anônimo");
        assert_eq!(clean_color("red"), "#5865f2");
        assert!(valid_server_name("   ").is_err());
    }
}
