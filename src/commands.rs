//! Comandos enviados pelos clientes. Toda permissão é verificada aqui,
//! no servidor; o cliente só esconde botões por conveniência.
//!
//! Cada alteração roda em `AppState::mutate`: valida, grava no banco numa
//! transação e só então atualiza o cache em memória.

use uuid::Uuid;

use crate::db;
use crate::model::{
    new_invite_code, Audience, Ban, Channel, ChannelKind, ClientMsg, Event, Message, Report, Server,
    ServerMsg, User, REPORT_CATEGORIES, VIDEO_QUALITIES,
};
use crate::state::{
    log, now_ms, AppState, Fail, Res, VoiceSession, HISTORY_PAGE, MAX_CHANNELS_PER_SERVER,
    MAX_MEMBERS_PER_SERVER, MAX_OWNED_SERVERS, MAX_REPORTS_PER_SERVER, MAX_SERVERS,
};
use crate::ws::Out;

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

fn no_permission<T>() -> Res<T> {
    Err(Fail::User(NO_PERMISSION.into()))
}

pub fn handle(cmd: ClientMsg, cx: &mut Ctx) -> Res<()> {
    let st = cx.state;
    let uid = cx.me.id;
    let admin = st.is_admin(uid);

    match cmd {
        // Troca de apelido/cor (LGPD art. 18, III: correção). O id não muda.
        ClientMsg::Hello { name, color, .. } => {
            cx.me.name = clean_name(&name);
            cx.me.color = clean_color(&color);
            st.online.lock().unwrap().insert(cx.conn_id, cx.me.clone());
            let voice_server = st.voice_sessions.lock().unwrap().get_mut(&uid).map(|s| {
                s.user = cx.me.clone();
                s.server_id
            });
            if let Some(sid) = voice_server {
                st.emit_voice(sid);
            }
            if st.update_profile(cx.me)? {
                st.send_all(ServerMsg::ProfileUpdated { user: cx.me.clone() });
                // Listas de membros de servidores privados mostram o nome atual.
                let servers: Vec<Uuid> = st
                    .cache
                    .lock()
                    .unwrap()
                    .servers
                    .iter()
                    .filter(|s| !s.public && s.is_member(uid))
                    .map(|s| s.id)
                    .collect();
                for sid in servers {
                    st.emit(Event::ServerChanged(sid));
                }
            }
            st.send_all(ServerMsg::Presence { online: st.online_users() });
        }

        // ---------- servidores ----------
        ClientMsg::CreateServer { name, public } => {
            let name = valid_server_name(&name)?;
            let channel = Channel::new("geral", ChannelKind::Text);
            let server = Server {
                id: Uuid::new_v4(),
                name,
                channels: vec![channel.clone(), Channel::new("Geral", ChannelKind::Voice)],
                owner_id: Some(uid),
                public,
                members: vec![uid],
                moderators: vec![],
                bans: vec![],
                invite_code: new_invite_code(),
            };
            let sid = server.id;
            st.mutate(|cache, tx| {
                if cache.servers.len() >= MAX_SERVERS {
                    return Err("Limite de servidores da instância atingido".into());
                }
                if cache.servers.iter().filter(|s| s.owner_id == Some(uid)).count() >= MAX_OWNED_SERVERS {
                    return Err(format!("Cada pessoa pode ser dona de até {MAX_OWNED_SERVERS} servidores").into());
                }
                db::insert_server(tx, &server, now_ms())?;
                cache.servers.push(server);
                Ok(())
            })?;
            st.emit(Event::ServerChanged(sid));
            cx.send(ServerMsg::Focus { server_id: sid, channel_id: Some(channel.id) });
        }

        ClientMsg::UpdateServer { server_id, name, public } => {
            let name = name.map(|n| valid_server_name(&n)).transpose()?;
            st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                if !cache.servers[i].can_manage(uid, admin) {
                    return no_permission();
                }
                let mut s = cache.servers[i].clone();
                if let Some(n) = name {
                    s.name = n;
                }
                if let Some(p) = public {
                    s.public = p;
                }
                db::update_server(tx, &s)?;
                log(tx, server_id, Some(uid), "servidor_alterado", None, &format!("público: {}", s.public))?;
                cache.servers[i] = s;
                Ok(())
            })?;
            // Ficou privado: quem não é membro sai da voz. (A cópia solta o cache antes.)
            let server = st.cache.lock().unwrap().server(server_id).cloned();
            if let Some(s) = server {
                st.kick_voice_where(|v| v.server_id == server_id && !s.can_view(v.user.id), "O servidor ficou privado");
            }
            st.emit(Event::ServerChanged(server_id));
        }

        ClientMsg::DeleteServer { server_id } => {
            st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                if !cache.servers[i].can_manage(uid, admin) {
                    return no_permission();
                }
                // Canais, mensagens, denúncias e papéis vão junto (ON DELETE CASCADE).
                db::delete_server(tx, server_id)?;
                log(tx, server_id, Some(uid), "servidor_apagado", None, "")?;
                cache.servers.remove(i);
                Ok(())
            })?;
            st.kick_voice_where(|v| v.server_id == server_id, "O servidor foi apagado");
            st.emit(Event::ServerDeleted(server_id));
        }

        ClientMsg::RegenerateInvite { server_id } => {
            st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                if !cache.servers[i].can_moderate(uid, admin) {
                    return no_permission();
                }
                let mut s = cache.servers[i].clone();
                s.invite_code = new_invite_code();
                db::update_server(tx, &s)?;
                cache.servers[i] = s;
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
        }

        ClientMsg::JoinServer { code } => {
            let code = code.trim().to_string();
            let (sid, first_channel) = st.mutate(|cache, tx| {
                let i = cache
                    .servers
                    .iter()
                    .position(|s| !code.is_empty() && s.invite_code == code)
                    .ok_or("Convite inválido ou expirado")?;
                let s = &cache.servers[i];
                if s.is_banned(uid) {
                    return Err("Você não pode entrar neste servidor".into());
                }
                let result = (s.id, s.channels.first().map(|c| c.id));
                if !s.is_member(uid) {
                    if s.members.len() >= MAX_MEMBERS_PER_SERVER {
                        return Err("Este servidor está cheio".into());
                    }
                    db::set_role(tx, "server_members", s.id, uid, true)?;
                    cache.servers[i].members.push(uid);
                }
                Ok(result)
            })?;
            st.emit(Event::ServerChanged(sid));
            cx.send(ServerMsg::Focus { server_id: sid, channel_id: first_channel });
        }

        ClientMsg::LeaveServer { server_id } => {
            st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                if cache.servers[i].owner_id == Some(uid) {
                    return Err("Quem é dono não pode sair. Apague o servidor ou apague seus dados para passá-lo adiante.".into());
                }
                db::set_role(tx, "server_members", server_id, uid, false)?;
                db::set_role(tx, "server_moderators", server_id, uid, false)?;
                let s = &mut cache.servers[i];
                s.members.retain(|&m| m != uid);
                s.moderators.retain(|&m| m != uid);
                Ok(())
            })?;
            let server = st.cache.lock().unwrap().server(server_id).cloned();
            if let Some(s) = server {
                st.kick_voice_where(|v| v.server_id == server_id && !s.can_view(v.user.id), "Você saiu do servidor");
            }
            st.emit(Event::ServerChanged(server_id));
        }

        // ---------- canais ----------
        ClientMsg::CreateChannel { server_id, name, kind } => {
            let name = channel_name_for(kind, &name);
            if name.is_empty() {
                return Err("Nome de canal inválido".into());
            }
            let channel = Channel::new(&name, kind);
            let cid = channel.id;
            st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_moderate(uid, admin) {
                    return no_permission();
                }
                if s.channels.len() >= MAX_CHANNELS_PER_SERVER {
                    return Err("Limite de canais atingido".into());
                }
                db::insert_channel(tx, server_id, &channel, s.channels.len() as i64)?;
                cache.servers[i].channels.push(channel);
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
            cx.send(ServerMsg::Focus { server_id, channel_id: Some(cid) });
        }

        ClientMsg::DeleteChannel { channel_id } => {
            let sid = st.mutate(|cache, tx| {
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_moderate(uid, admin) {
                    return no_permission();
                }
                if s.channels.len() <= 1 {
                    return Err("O servidor precisa ter pelo menos um canal".into());
                }
                let sid = s.id;
                // Mensagens e denúncias do canal vão junto (ON DELETE CASCADE).
                db::delete_channel(tx, channel_id)?;
                log(tx, sid, Some(uid), "canal_apagado", None, "")?;
                cache.servers[i].channels.retain(|c| c.id != channel_id);
                Ok(sid)
            })?;
            st.kick_voice_where(|v| v.channel_id == channel_id, "O canal de voz foi apagado");
            st.emit(Event::ServerChanged(sid));
            st.emit(Event::ReportsChanged(sid));
        }

        ClientMsg::UpdateChannel { channel_id, name, user_limit, bitrate, video_quality } => {
            if user_limit.is_some_and(|l| l > 99) {
                return Err("O limite de pessoas vai de 0 (sem limite) a 99".into());
            }
            if bitrate.is_some_and(|b| !(8..=384).contains(&b)) {
                return Err("A taxa de bits do áudio vai de 8 a 384 kbps".into());
            }
            if video_quality.as_deref().is_some_and(|q| !VIDEO_QUALITIES.contains(&q)) {
                return Err("Qualidade de vídeo inválida".into());
            }
            let sid = st.mutate(|cache, tx| {
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_moderate(uid, admin) {
                    return no_permission();
                }
                let sid = cache.servers[i].id;
                let pos = cache.servers[i].channels.iter().position(|c| c.id == channel_id).ok_or("Canal não encontrado")?;
                let mut ch = cache.servers[i].channels[pos].clone();
                if let Some(n) = name {
                    let n = channel_name_for(ch.kind, &n);
                    if n.is_empty() {
                        return Err("Nome de canal inválido".into());
                    }
                    ch.name = n;
                }
                if let Some(l) = user_limit {
                    ch.user_limit = l;
                }
                if let Some(b) = bitrate {
                    ch.bitrate = b;
                }
                if let Some(q) = video_quality {
                    ch.video_quality = q;
                }
                db::update_channel(tx, &ch)?;
                log(tx, sid, Some(uid), "canal_alterado", None, "")?;
                cache.servers[i].channels[pos] = ch;
                Ok(sid)
            })?;
            st.emit(Event::ServerChanged(sid));
        }

        // ---------- voz ----------
        ClientMsg::VoiceJoin { channel_id } => {
            let lk = st.voice.as_ref().ok_or("Os canais de voz estão desativados nesta instância")?;
            let (sid, ch, moderate) = {
                let cache = st.cache.lock().unwrap();
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_view(uid) {
                    return no_permission();
                }
                let ch = s.channels.iter().find(|c| c.id == channel_id).cloned().ok_or("Canal não encontrado")?;
                if ch.kind != ChannelKind::Voice {
                    return Err("Este não é um canal de voz".into());
                }
                (s.id, ch, s.can_moderate(uid, admin))
            };
            if ch.user_limit > 0 && !moderate {
                let inside = st
                    .voice_sessions
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|v| v.channel_id == channel_id && v.user.id != uid)
                    .count();
                if inside >= ch.user_limit as usize {
                    return Err("O canal de voz está cheio".into());
                }
            }
            // Uma sessão por pessoa: sai do canal anterior, se houver.
            st.leave_voice(uid, None);
            st.voice_sessions.lock().unwrap().insert(
                uid,
                VoiceSession {
                    conn_id: cx.conn_id,
                    server_id: sid,
                    channel_id,
                    user: cx.me.clone(),
                    muted: false,
                    deafened: false,
                    streaming: false,
                    camera: false,
                },
            );
            let token = lk.token(cx.me, channel_id, now_ms() / 1000);
            cx.send(ServerMsg::VoiceToken {
                server_id: sid,
                channel_id,
                url: lk.public_url.clone(),
                token,
                bitrate: ch.bitrate,
                video_quality: ch.video_quality.clone(),
            });
            st.emit_voice(sid);
        }

        ClientMsg::VoiceLeave => st.leave_voice(uid, Some(cx.conn_id)),

        ClientMsg::VoiceUpdate { muted, deafened, streaming, camera } => {
            let sid = {
                let mut sessions = st.voice_sessions.lock().unwrap();
                match sessions.get_mut(&uid) {
                    Some(s) if s.conn_id == cx.conn_id => {
                        s.muted = muted || deafened;
                        s.deafened = deafened;
                        s.streaming = streaming;
                        s.camera = camera;
                        Some(s.server_id)
                    }
                    _ => None,
                }
            };
            if let Some(sid) = sid {
                st.emit_voice(sid);
            }
        }

        ClientMsg::SetVoiceStatus { channel_id, status } => {
            let status: String = clean_text(&status).chars().take(100).collect();
            let in_channel = st.voice_sessions.lock().unwrap().get(&uid).is_some_and(|s| s.channel_id == channel_id);
            let sid = st.mutate(|cache, tx| {
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                // Como no Discord: quem está no canal define o status; moderadores também.
                if !in_channel && !cache.servers[i].can_moderate(uid, admin) {
                    return Err("Entre no canal de voz para definir o status".into());
                }
                let sid = cache.servers[i].id;
                let ch = cache.servers[i].channels.iter_mut().find(|c| c.id == channel_id).ok_or("Canal não encontrado")?;
                if ch.kind != ChannelKind::Voice {
                    return Err("Só canais de voz têm status".into());
                }
                let mut updated = ch.clone();
                updated.status = (!status.is_empty()).then(|| status.clone());
                updated.status_by = updated.status.as_ref().map(|_| uid);
                db::update_channel(tx, &updated)?;
                *ch = updated;
                Ok(sid)
            })?;
            st.emit(Event::ServerChanged(sid));
        }

        // ---------- papéis e banimentos ----------
        ClientMsg::SetModerator { server_id, user_id, enabled } => {
            st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                if !cache.servers[i].can_manage(uid, admin) {
                    return no_permission();
                }
                if !cache.users.contains_key(&user_id) {
                    return Err("Pessoa não encontrada".into());
                }
                let s = &cache.servers[i];
                if s.owner_id == Some(user_id) || !s.can_view(user_id) {
                    return Err("Não é possível alterar o papel desta pessoa".into());
                }
                db::set_role(tx, "server_moderators", server_id, user_id, enabled)?;
                if enabled {
                    db::set_role(tx, "server_members", server_id, user_id, true)?;
                }
                let action = if enabled { "moderador_adicionado" } else { "moderador_removido" };
                log(tx, server_id, Some(uid), action, Some(user_id), "")?;
                let s = &mut cache.servers[i];
                s.moderators.retain(|&m| m != user_id);
                if enabled {
                    s.moderators.push(user_id);
                    if !s.members.contains(&user_id) {
                        s.members.push(user_id);
                    }
                }
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
            let server_name = st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_moderate(uid, admin) {
                    return no_permission();
                }
                if s.owner_id == Some(user_id) {
                    return Err("O dono do servidor não pode ser banido".into());
                }
                // Só quem gerencia pode banir moderadores.
                if s.moderators.contains(&user_id) && !s.can_manage(uid, admin) {
                    return no_permission();
                }
                let ban = Ban { user_id, reason: reason.clone(), at: now_ms() };
                db::set_role(tx, "server_members", server_id, user_id, false)?;
                db::set_role(tx, "server_moderators", server_id, user_id, false)?;
                db::insert_ban(tx, server_id, &ban)?;
                if delete_messages {
                    db::delete_user_messages_in_server(tx, server_id, user_id)?;
                }
                log(tx, server_id, Some(uid), "banimento", Some(user_id), &reason)?;
                let s = &mut cache.servers[i];
                s.members.retain(|&m| m != user_id);
                s.moderators.retain(|&m| m != user_id);
                s.bans.retain(|b| b.user_id != user_id);
                s.bans.push(ban);
                Ok(s.name.clone())
            })?;
            st.kick_voice_where(|v| v.server_id == server_id && v.user.id == user_id, "Você foi banido(a) do servidor");
            // Transparência: a pessoa banida fica sabendo onde e por quê.
            st.emit(Event::Send(
                Audience::User(user_id),
                ServerMsg::Info { message: format!("Você foi banido(a) de \"{server_name}\". Motivo: {reason}") },
            ));
            st.emit(Event::ServerChanged(server_id));
            if delete_messages {
                st.emit(Event::Send(Audience::Server(server_id), ServerMsg::MessagesPurged { server_id, user_id }));
                st.emit(Event::ReportsChanged(server_id));
            }
        }

        ClientMsg::UnbanUser { server_id, user_id } => {
            st.mutate(|cache, tx| {
                let i = cache.server_index(server_id).ok_or("Servidor não encontrado")?;
                if !cache.servers[i].can_moderate(uid, admin) {
                    return no_permission();
                }
                db::delete_ban(tx, server_id, user_id)?;
                log(tx, server_id, Some(uid), "banimento_revogado", Some(user_id), "")?;
                cache.servers[i].bans.retain(|b| b.user_id != user_id);
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
                id: Uuid::now_v7(),
                channel_id,
                author: cx.me.clone(),
                content: content.to_string(),
                timestamp: now_ms(),
            };
            let sid = st.mutate(|cache, tx| {
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_view(uid) {
                    return no_permission();
                }
                if cache.servers[i].channels.iter().any(|c| c.id == channel_id && c.kind == ChannelKind::Voice) {
                    return Err("Canais de voz não têm chat de texto".into());
                }
                db::insert_message(tx, &message)?;
                Ok(cache.servers[i].id)
            })?;
            st.emit(Event::Send(Audience::Server(sid), ServerMsg::Message { message }));
        }

        ClientMsg::FetchHistory { channel_id, before } => {
            {
                let cache = st.cache.lock().unwrap();
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_view(uid) {
                    return no_permission();
                }
            }
            // Pede uma a mais para saber se ainda há mensagens antigas.
            let mut messages = st.read(|c| db::history(c, channel_id, before, HISTORY_PAGE + 1))?;
            let has_more = messages.len() > HISTORY_PAGE;
            if has_more {
                messages.remove(0);
            }
            cx.send(ServerMsg::History { channel_id, messages, before, has_more });
        }

        ClientMsg::Typing { channel_id } => {
            let sid = {
                let cache = st.cache.lock().unwrap();
                cache
                    .server_index_of_channel(channel_id)
                    .filter(|&i| cache.servers[i].can_view(uid))
                    .map(|i| cache.servers[i].id)
            };
            if let Some(sid) = sid {
                st.emit(Event::Send(Audience::Server(sid), ServerMsg::Typing { channel_id, user: cx.me.clone() }));
            }
        }

        // O autor apaga a própria mensagem; moderadores apagam qualquer uma.
        ClientMsg::DeleteMessage { channel_id, message_id } => {
            let sid = st.mutate(|cache, tx| {
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                let author = db::message_author(tx, channel_id, message_id)?.ok_or("Mensagem não encontrada")?;
                if author != uid && !s.can_moderate(uid, admin) {
                    return no_permission();
                }
                db::delete_message(tx, message_id)?;
                if author != uid {
                    log(tx, s.id, Some(uid), "mensagem_apagada", Some(author), "")?;
                }
                Ok(s.id)
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
            let sid = st.mutate(|cache, tx| {
                let i = cache.server_index_of_channel(channel_id).ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_view(uid) {
                    return no_permission();
                }
                let author = db::message_author(tx, channel_id, message_id)?.ok_or("Mensagem não encontrada")?;
                if author == uid {
                    return Err("Você não pode denunciar a própria mensagem".into());
                }
                if db::report_exists(tx, message_id, uid)? {
                    return Err("Você já denunciou esta mensagem".into());
                }
                if db::count_reports(tx, s.id)? >= MAX_REPORTS_PER_SERVER {
                    return Err("Fila de denúncias cheia; tente mais tarde".into());
                }
                db::insert_report(
                    tx,
                    &Report {
                        id: Uuid::now_v7(),
                        server_id: s.id,
                        channel_id,
                        message_id,
                        reporter_id: uid,
                        category,
                        details,
                        created_at: now_ms(),
                    },
                )?;
                Ok(s.id)
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
            let (sid, deleted) = st.mutate(|cache, tx| {
                let r = db::get_report(tx, report_id)?.ok_or("Denúncia não encontrada")?;
                let s = cache.server(r.server_id).ok_or("Servidor não encontrado")?;
                if !s.can_moderate(uid, admin) {
                    return no_permission();
                }
                if delete_message {
                    let author = db::message_author(tx, r.channel_id, r.message_id)?;
                    // Apagar a mensagem leva junto todas as denúncias sobre ela.
                    db::delete_message(tx, r.message_id)?;
                    log(tx, r.server_id, Some(uid), "mensagem_apagada_por_denuncia", author, &r.category)?;
                    Ok((r.server_id, Some((r.channel_id, r.message_id))))
                } else {
                    db::delete_report(tx, report_id)?;
                    log(tx, r.server_id, Some(uid), "denuncia_descartada", None, &r.category)?;
                    Ok((r.server_id, None))
                }
            })?;
            if let Some((channel_id, message_id)) = deleted {
                st.emit(Event::Send(Audience::Server(sid), ServerMsg::MessageDeleted { channel_id, message_id }));
            }
            st.emit(Event::ReportsChanged(sid));
        }

        // ---------- LGPD ----------
        ClientMsg::ExportMyData => {
            let data = st.export_user_data(cx.me)?;
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

fn valid_server_name(name: &str) -> Res<String> {
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

/// Canais de texto usam "meu-canal"; canais de voz aceitam espaços e maiúsculas ("Sala de Jogos").
fn channel_name_for(kind: ChannelKind, name: &str) -> String {
    match kind {
        ChannelKind::Text => clean_channel_name(name),
        ChannelKind::Voice => clean_text(name).chars().take(32).collect::<String>().trim().to_string(),
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
