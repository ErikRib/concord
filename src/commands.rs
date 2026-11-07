//! Comandos enviados pelos clientes. Toda permissão é verificada aqui,
//! no servidor; o cliente só esconde botões por conveniência.
//!
//! Cada alteração roda em `AppState::mutate`: valida, grava no banco numa
//! transação e só então atualiza o cache em memória.

use uuid::Uuid;

use crate::db;
use crate::model::{
    new_invite_code, Audience, Ban, Channel, ChannelKind, ClientMsg, Event, Message, Report,
    Server, ServerMsg, User, BUILTIN_SOUNDS, REPORT_CATEGORIES, VIDEO_QUALITIES,
};
use crate::state::UploadPurpose;
use crate::state::{
    log, now_ms, AppState, Fail, Res, VoiceSession, HISTORY_PAGE, MAX_CHANNELS_PER_SERVER,
    MAX_MEMBERS_PER_SERVER, MAX_OWNED_SERVERS, MAX_PENDING_UPLOADS, MAX_REPORTS_PER_SERVER,
    MAX_SERVERS, MAX_SOUNDS_PER_SERVER, MAX_SOUND_BYTES, MAX_UPLOAD_BYTES,
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
                st.send_all(ServerMsg::ProfileUpdated {
                    user: cx.me.clone(),
                });
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
            st.send_all(ServerMsg::Presence {
                online: st.online_users(),
            });
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
                if cache
                    .servers
                    .iter()
                    .filter(|s| s.owner_id == Some(uid))
                    .count()
                    >= MAX_OWNED_SERVERS
                {
                    return Err(format!(
                        "Cada pessoa pode ser dona de até {MAX_OWNED_SERVERS} servidores"
                    )
                    .into());
                }
                db::insert_server(tx, &server, now_ms())?;
                cache.servers.push(server);
                Ok(())
            })?;
            st.emit(Event::ServerChanged(sid));
            cx.send(ServerMsg::Focus {
                server_id: sid,
                channel_id: Some(channel.id),
            });
        }

        ClientMsg::UpdateServer {
            server_id,
            name,
            public,
        } => {
            let name = name.map(|n| valid_server_name(&n)).transpose()?;
            st.mutate(|cache, tx| {
                let i = cache
                    .server_index(server_id)
                    .ok_or("Servidor não encontrado")?;
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
                log(
                    tx,
                    server_id,
                    Some(uid),
                    "servidor_alterado",
                    None,
                    &format!("público: {}", s.public),
                )?;
                cache.servers[i] = s;
                Ok(())
            })?;
            // Ficou privado: quem não é membro sai da voz. (A cópia solta o cache antes.)
            let server = st.cache.lock().unwrap().server(server_id).cloned();
            if let Some(s) = server {
                st.kick_voice_where(
                    |v| v.server_id == server_id && !s.can_view(v.user.id),
                    "O servidor ficou privado",
                );
            }
            st.emit(Event::ServerChanged(server_id));
        }

        ClientMsg::DeleteServer { server_id } => {
            st.mutate(|cache, tx| {
                let i = cache
                    .server_index(server_id)
                    .ok_or("Servidor não encontrado")?;
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
                let i = cache
                    .server_index(server_id)
                    .ok_or("Servidor não encontrado")?;
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
            cx.send(ServerMsg::Focus {
                server_id: sid,
                channel_id: first_channel,
            });
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
                st.kick_voice_where(
                    |v| v.server_id == server_id && !s.can_view(v.user.id),
                    "Você saiu do servidor",
                );
            }
            st.emit(Event::ServerChanged(server_id));
        }

        // ---------- canais ----------
        ClientMsg::CreateChannel {
            server_id,
            name,
            kind,
        } => {
            let name = channel_name_for(kind, &name);
            if name.is_empty() {
                return Err("Nome de canal inválido".into());
            }
            let channel = Channel::new(&name, kind);
            let cid = channel.id;
            st.mutate(|cache, tx| {
                let i = cache
                    .server_index(server_id)
                    .ok_or("Servidor não encontrado")?;
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
            cx.send(ServerMsg::Focus {
                server_id,
                channel_id: Some(cid),
            });
        }

        ClientMsg::DeleteChannel { channel_id } => {
            let sid = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
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

        ClientMsg::UpdateChannel {
            channel_id,
            name,
            user_limit,
            bitrate,
            video_quality,
        } => {
            if user_limit.is_some_and(|l| l > 99) {
                return Err("O limite de pessoas vai de 0 (sem limite) a 99".into());
            }
            if bitrate.is_some_and(|b| !(8..=384).contains(&b)) {
                return Err("A taxa de bits do áudio vai de 8 a 384 kbps".into());
            }
            if video_quality
                .as_deref()
                .is_some_and(|q| !VIDEO_QUALITIES.contains(&q))
            {
                return Err("Qualidade de vídeo inválida".into());
            }
            let sid = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_moderate(uid, admin) {
                    return no_permission();
                }
                let sid = cache.servers[i].id;
                let pos = cache.servers[i]
                    .channels
                    .iter()
                    .position(|c| c.id == channel_id)
                    .ok_or("Canal não encontrado")?;
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
            let lk = st
                .voice
                .as_ref()
                .ok_or("Os canais de voz estão desativados nesta instância")?;
            let (sid, ch, moderate) = {
                let cache = st.cache.lock().unwrap();
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_view(uid) {
                    return no_permission();
                }
                let ch = s
                    .channels
                    .iter()
                    .find(|c| c.id == channel_id)
                    .cloned()
                    .ok_or("Canal não encontrado")?;
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
            // Mesmo canal (ex.: o chat reconectou): só atualiza a conexão, sem "esvaziar" o canal.
            let same = {
                let mut sessions = st.voice_sessions.lock().unwrap();
                match sessions.get_mut(&uid) {
                    Some(s) if s.channel_id == channel_id => {
                        s.conn_id = cx.conn_id;
                        true
                    }
                    _ => false,
                }
            };
            if !same {
                // Uma sessão por pessoa: sai do canal anterior, se houver.
                if let Some(old) = st.leave_voice(uid, None, None) {
                    st.lk_remove(old.channel_id, uid);
                }
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
            }
            let can_speak = {
                let (m, d) = st.server_mute_of(sid, uid);
                !(m || d)
            };
            cx.send(ServerMsg::VoiceToken {
                server_id: sid,
                channel_id,
                url: lk.public_url.clone(),
                port: lk.port,
                token: lk.token(cx.me, channel_id, now_ms() / 1000, can_speak),
                bitrate: ch.bitrate,
                video_quality: ch.video_quality.clone(),
                moved: false,
            });
            st.emit_voice(sid);
        }

        ClientMsg::VoiceLeave { channel_id } => {
            if let Some(s) = st.leave_voice(uid, Some(cx.conn_id), channel_id) {
                st.lk_remove(s.channel_id, uid);
            }
        }

        ClientMsg::VoiceModerate {
            server_id,
            user_id,
            action,
        } => {
            let server_name = {
                let cache = st.cache.lock().unwrap();
                let s = cache.server(server_id).ok_or("Servidor não encontrado")?;
                check_can_act_on(st, s, uid, admin, user_id)?;
                s.name.clone()
            };
            let session = st
                .voice_sessions
                .lock()
                .unwrap()
                .get(&user_id)
                .filter(|v| v.server_id == server_id)
                .cloned();
            let (log_action, notice) = match action.as_str() {
                "mute" | "unmute" | "deafen" | "undeafen" => {
                    // Silenciar e ensurdecer são independentes; quem está surdo também
                    // não fala (ver `can_speak`), mas reativar o áudio não desfaz o silêncio.
                    let (mut mute, mut deaf) = st.server_mute_of(server_id, user_id);
                    match action.as_str() {
                        "mute" => mute = true,
                        "unmute" => mute = false,
                        "deafen" => deaf = true,
                        _ => deaf = false,
                    }
                    {
                        let mut m = st.server_mutes.lock().unwrap();
                        if mute || deaf {
                            m.insert((server_id, user_id), (mute, deaf));
                        } else {
                            m.remove(&(server_id, user_id));
                        }
                    }
                    if let Some(s) = &session {
                        st.lk_set_can_speak(s.channel_id, user_id, !(mute || deaf));
                    }
                    let notice = match action.as_str() {
                        "mute" => format!("Um moderador silenciou você em \"{server_name}\""),
                        "unmute" => {
                            format!("Um moderador liberou seu microfone em \"{server_name}\"")
                        }
                        "deafen" => {
                            format!("Um moderador desativou seu áudio em \"{server_name}\"")
                        }
                        _ => format!("Um moderador reativou seu áudio em \"{server_name}\""),
                    };
                    (format!("voz_{action}"), notice)
                }
                "disconnect" => {
                    let s = session.ok_or("Esta pessoa não está numa chamada deste servidor")?;
                    st.leave_voice(user_id, None, None);
                    st.lk_remove(s.channel_id, user_id);
                    st.emit(Event::Send(
                        Audience::User(user_id),
                        ServerMsg::VoiceEnded {
                            reason: format!(
                                "Um moderador desconectou você da voz em \"{server_name}\""
                            ),
                        },
                    ));
                    ("voz_desconectado".into(), String::new())
                }
                _ => return Err("Ação de moderação de voz inválida".into()),
            };
            st.mutate(|_, tx| log(tx, server_id, Some(uid), &log_action, Some(user_id), ""))?;
            if !notice.is_empty() {
                st.emit(Event::Send(
                    Audience::User(user_id),
                    ServerMsg::Info { message: notice },
                ));
            }
            st.emit_voice(server_id);
        }

        ClientMsg::VoiceMove {
            user_id,
            channel_id,
        } => {
            let lk = st
                .voice
                .as_ref()
                .ok_or("Os canais de voz estão desativados nesta instância")?;
            let (sid, ch) = {
                let cache = st.cache.lock().unwrap();
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                check_can_act_on(st, s, uid, admin, user_id)?;
                let ch = s
                    .channels
                    .iter()
                    .find(|c| c.id == channel_id)
                    .cloned()
                    .ok_or("Canal não encontrado")?;
                if ch.kind != ChannelKind::Voice {
                    return Err("Só é possível mover para um canal de voz".into());
                }
                if !s.can_view(user_id) {
                    return Err("Essa pessoa não pode ver este servidor".into());
                }
                (s.id, ch)
            };
            // Move a sessão (como no Discord, moderadores ignoram o limite do canal).
            let (old_channel, target_user) = {
                let mut sessions = st.voice_sessions.lock().unwrap();
                let s = sessions
                    .get_mut(&user_id)
                    .filter(|v| v.server_id == sid)
                    .ok_or("Esta pessoa não está numa chamada deste servidor")?;
                if s.channel_id == channel_id {
                    return Err("A pessoa já está nesse canal".into());
                }
                let old = s.channel_id;
                s.channel_id = channel_id;
                (old, s.user.clone())
            };
            st.after_channel_left(old_channel);
            let can_speak = {
                let (m, d) = st.server_mute_of(sid, user_id);
                !(m || d)
            };
            st.emit(Event::Send(
                Audience::User(user_id),
                ServerMsg::VoiceToken {
                    server_id: sid,
                    channel_id,
                    url: lk.public_url.clone(),
                    port: lk.port,
                    token: lk.token(&target_user, channel_id, now_ms() / 1000, can_speak),
                    bitrate: ch.bitrate,
                    video_quality: ch.video_quality.clone(),
                    moved: true,
                },
            ));
            // Garante a saída da sala antiga mesmo que o navegador não obedeça.
            let st_old = old_channel;
            let admin_lk = lk.admin();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if let Err(e) = admin_lk.remove(st_old, user_id).await {
                    eprintln!("Voz: falha ao remover da sala antiga: {e}");
                }
            });
            st.mutate(|_, tx| log(tx, sid, Some(uid), "voz_movido", Some(user_id), &ch.name))?;
            st.emit_voice(sid);
        }

        ClientMsg::VoiceUpdate {
            muted,
            deafened,
            streaming,
            camera,
        } => {
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
            let in_channel = st
                .voice_sessions
                .lock()
                .unwrap()
                .get(&uid)
                .is_some_and(|s| s.channel_id == channel_id);
            let sid = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                // Como no Discord: quem está no canal define o status; moderadores também.
                if !in_channel && !cache.servers[i].can_moderate(uid, admin) {
                    return Err("Entre no canal de voz para definir o status".into());
                }
                let sid = cache.servers[i].id;
                let ch = cache.servers[i]
                    .channels
                    .iter_mut()
                    .find(|c| c.id == channel_id)
                    .ok_or("Canal não encontrado")?;
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
        ClientMsg::SetModerator {
            server_id,
            user_id,
            enabled,
        } => {
            st.mutate(|cache, tx| {
                let i = cache
                    .server_index(server_id)
                    .ok_or("Servidor não encontrado")?;
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
                let action = if enabled {
                    "moderador_adicionado"
                } else {
                    "moderador_removido"
                };
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

        ClientMsg::BanUser {
            server_id,
            user_id,
            reason,
            delete_messages,
        } => {
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
                let i = cache
                    .server_index(server_id)
                    .ok_or("Servidor não encontrado")?;
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
                let ban = Ban {
                    user_id,
                    reason: reason.clone(),
                    at: now_ms(),
                };
                db::set_role(tx, "server_members", server_id, user_id, false)?;
                db::set_role(tx, "server_moderators", server_id, user_id, false)?;
                db::insert_ban(tx, server_id, &ban)?;
                if delete_messages {
                    db::delete_user_messages_in_server(tx, server_id, user_id)?;
                }
                log(
                    tx,
                    server_id,
                    Some(uid),
                    "banimento",
                    Some(user_id),
                    &reason,
                )?;
                let s = &mut cache.servers[i];
                s.members.retain(|&m| m != user_id);
                s.moderators.retain(|&m| m != user_id);
                s.bans.retain(|b| b.user_id != user_id);
                s.bans.push(ban);
                Ok(s.name.clone())
            })?;
            st.kick_voice_where(
                |v| v.server_id == server_id && v.user.id == user_id,
                "Você foi banido(a) do servidor",
            );
            // Transparência: a pessoa banida fica sabendo onde e por quê.
            st.emit(Event::Send(
                Audience::User(user_id),
                ServerMsg::Info {
                    message: format!("Você foi banido(a) de \"{server_name}\". Motivo: {reason}"),
                },
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
            st.mutate(|cache, tx| {
                let i = cache
                    .server_index(server_id)
                    .ok_or("Servidor não encontrado")?;
                if !cache.servers[i].can_moderate(uid, admin) {
                    return no_permission();
                }
                db::delete_ban(tx, server_id, user_id)?;
                log(
                    tx,
                    server_id,
                    Some(uid),
                    "banimento_revogado",
                    Some(user_id),
                    "",
                )?;
                cache.servers[i].bans.retain(|b| b.user_id != user_id);
                Ok(())
            })?;
            st.emit(Event::ServerChanged(server_id));
        }

        // ---------- mensagens ----------
        ClientMsg::SendMessage {
            channel_id,
            content,
            reply_to,
            attachments,
        } => {
            let content = content.trim();
            if content.is_empty() && attachments.is_empty() {
                return Ok(());
            }
            if content.chars().count() > 2000 {
                return Err("A mensagem passa de 2000 caracteres".into());
            }
            if attachments.len() > MAX_ATTACHMENTS {
                return Err(
                    format!("Envie no máximo {MAX_ATTACHMENTS} imagens por mensagem").into(),
                );
            }
            let mut message = Message::new(
                Uuid::now_v7(),
                channel_id,
                cx.me.clone(),
                content.to_string(),
                now_ms(),
            );
            let (sid, message) = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_view(uid) {
                    return no_permission();
                }
                if cache.servers[i]
                    .channels
                    .iter()
                    .any(|c| c.id == channel_id && c.kind == ChannelKind::Voice)
                {
                    return Err("Canais de voz não têm chat de texto".into());
                }
                if let Some(rid) = reply_to {
                    message.reply_to = Some(
                        db::reply_preview(tx, channel_id, rid)?
                            .ok_or("A mensagem respondida não existe mais")?,
                    );
                }
                db::insert_message(tx, &message)?;
                for a in &attachments {
                    // Só imagens enviadas por esta pessoa, para este canal, ainda não usadas.
                    if !db::link_attachment(tx, *a, message.id, uid, channel_id)? {
                        return Err("Arquivo inválido ou expirado; envie de novo".into());
                    }
                }
                let sign = |id: Uuid| st.sign_attachment(id);
                db::hydrate(tx, std::slice::from_mut(&mut message), &sign)?;
                Ok((cache.servers[i].id, message))
            })?;
            st.emit(Event::Send(
                Audience::Server(sid),
                ServerMsg::Message { message },
            ));
        }

        // Só o autor edita; guarda apenas a versão atual (minimização).
        ClientMsg::EditMessage {
            channel_id,
            message_id,
            content,
        } => {
            let content = content.trim().to_string();
            if content.is_empty() {
                return Err("A mensagem não pode ficar vazia (para remover, apague-a)".into());
            }
            if content.chars().count() > 2000 {
                return Err("A mensagem passa de 2000 caracteres".into());
            }
            let at = now_ms();
            let sid = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_view(uid) {
                    return no_permission();
                }
                let author = db::message_author(tx, channel_id, message_id)?
                    .ok_or("Mensagem não encontrada")?;
                if author != uid {
                    return Err("Só quem escreveu pode editar".into());
                }
                db::edit_message(tx, message_id, &content, at)?;
                Ok(cache.servers[i].id)
            })?;
            st.emit(Event::Send(
                Audience::Server(sid),
                ServerMsg::MessageEdited {
                    channel_id,
                    message_id,
                    content,
                    edited_at: at,
                },
            ));
        }

        ClientMsg::React {
            channel_id,
            message_id,
            emoji,
            on,
        } => {
            if !valid_emoji(&emoji) {
                return Err("Reação inválida".into());
            }
            let (sid, changed) = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_view(uid) {
                    return no_permission();
                }
                db::message_author(tx, channel_id, message_id)?.ok_or("Mensagem não encontrada")?;
                if on
                    && !db::has_reaction(tx, message_id, &emoji)?
                    && db::reaction_emoji_count(tx, message_id)? >= MAX_REACTION_EMOJIS
                {
                    return Err(format!(
                        "Limite de {MAX_REACTION_EMOJIS} reações diferentes por mensagem"
                    )
                    .into());
                }
                let changed = db::set_reaction(tx, message_id, uid, &emoji, on, now_ms())?;
                Ok((cache.servers[i].id, changed))
            })?;
            if changed {
                st.emit(Event::Send(
                    Audience::Server(sid),
                    ServerMsg::ReactionUpdate {
                        channel_id,
                        message_id,
                        emoji,
                        user_id: uid,
                        on,
                    },
                ));
            }
        }

        // Autorização de uso único para enviar uma imagem por HTTP (o segredo de
        // identidade nunca trafega fora do WebSocket).
        ClientMsg::UploadBegin { channel_id, size } => {
            if size == 0 || size > MAX_UPLOAD_BYTES {
                return Err(format!(
                    "O arquivo deve ter até {} MB",
                    MAX_UPLOAD_BYTES / 1024 / 1024
                )
                .into());
            }
            {
                let cache = st.cache.lock().unwrap();
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_view(uid) {
                    return no_permission();
                }
                if s.channels
                    .iter()
                    .any(|c| c.id == channel_id && c.kind == ChannelKind::Voice)
                {
                    return Err("Canais de voz não têm chat de texto".into());
                }
            }
            if st.read(|c| db::count_pending_attachments(c, uid))? >= MAX_PENDING_UPLOADS {
                return Err(
                    "Muitos arquivos aguardando envio; envie a mensagem ou espere um pouco".into(),
                );
            }
            let (upload_id, token) = st.grant_upload(uid, UploadPurpose::Image { channel_id });
            cx.send(ServerMsg::UploadToken {
                upload_id,
                token,
                max_bytes: MAX_UPLOAD_BYTES,
                kind: "image",
            });
        }

        // ---------- Soundpad ----------
        ClientMsg::FetchSounds { server_id } => {
            let can_manage = {
                let cache = st.cache.lock().unwrap();
                let s = cache.server(server_id).ok_or("Servidor não encontrado")?;
                if !s.can_view(uid) {
                    return no_permission();
                }
                s.can_moderate(uid, admin)
            };
            cx.send(ServerMsg::Sounds {
                server_id,
                sounds: st.sounds(server_id)?,
                can_manage,
                max: MAX_SOUNDS_PER_SERVER,
            });
        }

        // Como no Discord, quem modera o servidor cuida dos sons.
        ClientMsg::SoundUploadBegin {
            server_id,
            size,
            name,
            emoji,
        } => {
            if size == 0 || size > MAX_SOUND_BYTES {
                return Err("O som deve ter até 5 segundos".into());
            }
            let name = clean_text(&name).chars().take(32).collect::<String>();
            if name.is_empty() {
                return Err("Dê um nome ao som".into());
            }
            let emoji = emoji.filter(|e| !e.is_empty());
            if emoji.as_deref().is_some_and(|e| !valid_emoji(e)) {
                return Err("Emoji inválido".into());
            }
            {
                let cache = st.cache.lock().unwrap();
                let s = cache.server(server_id).ok_or("Servidor não encontrado")?;
                if !s.can_moderate(uid, admin) {
                    return no_permission();
                }
            }
            if st.read(|c| db::count_sounds(c, server_id))? >= MAX_SOUNDS_PER_SERVER {
                return Err(format!(
                    "Este servidor já tem {MAX_SOUNDS_PER_SERVER} sons; apague algum para adicionar outro"
                )
                .into());
            }
            let (upload_id, token) = st.grant_upload(
                uid,
                UploadPurpose::Sound {
                    server_id,
                    name,
                    emoji,
                },
            );
            cx.send(ServerMsg::UploadToken {
                upload_id,
                token,
                max_bytes: MAX_SOUND_BYTES,
                kind: "sound",
            });
        }

        ClientMsg::DeleteSound { sound_id } => {
            let (server_id, uploader) = st
                .read(|c| db::sound_owner(c, sound_id))?
                .ok_or("Som não encontrado")?;
            {
                let cache = st.cache.lock().unwrap();
                let s = cache.server(server_id).ok_or("Servidor não encontrado")?;
                if !s.can_moderate(uid, admin) && !(uploader == uid && s.can_view(uid)) {
                    return no_permission();
                }
            }
            st.mutate(|_, tx| Ok(db::delete_sound(tx, sound_id)?))?;
            st.emit(Event::Send(
                Audience::Server(server_id),
                ServerMsg::SoundsChanged { server_id },
            ));
        }

        // Só quem está num canal de voz toca sons, e só para aquele canal.
        ClientMsg::PlaySound { sound } => {
            let Some(session) = st.voice_sessions.lock().unwrap().get(&uid).cloned() else {
                return Err("Entre num canal de voz para usar o Soundpad".into());
            };
            let (muted, deafened) = st.server_mute_of(session.server_id, uid);
            if muted || deafened {
                return Err("Um moderador silenciou você neste servidor".into());
            }
            let valid = match sound.strip_prefix("builtin:") {
                Some(name) => BUILTIN_SOUNDS.contains(&name),
                None => match sound.parse::<Uuid>() {
                    Ok(id) => st
                        .read(|c| db::sound_owner(c, id))?
                        .is_some_and(|(sid, _)| sid == session.server_id),
                    Err(_) => false,
                },
            };
            if !valid {
                return Err("Som não encontrado".into());
            }
            if !st.sound_allowed(uid) {
                return Ok(()); // cliques rápidos demais: ignora em silêncio
            }
            st.emit(Event::Send(
                Audience::Server(session.server_id),
                ServerMsg::SoundPlayed {
                    server_id: session.server_id,
                    channel_id: session.channel_id,
                    user_id: uid,
                    sound,
                },
            ));
        }

        ClientMsg::FetchHistory { channel_id, before } => {
            {
                let cache = st.cache.lock().unwrap();
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                if !cache.servers[i].can_view(uid) {
                    return no_permission();
                }
            }
            // Pede uma a mais para saber se ainda há mensagens antigas.
            let mut messages = st.history(channel_id, before, HISTORY_PAGE + 1)?;
            let has_more = messages.len() > HISTORY_PAGE;
            if has_more {
                messages.remove(0);
            }
            cx.send(ServerMsg::History {
                channel_id,
                messages,
                before,
                has_more,
            });
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
                st.emit(Event::Send(
                    Audience::Server(sid),
                    ServerMsg::Typing {
                        channel_id,
                        user: cx.me.clone(),
                    },
                ));
            }
        }

        // O autor apaga a própria mensagem; moderadores apagam qualquer uma.
        ClientMsg::DeleteMessage {
            channel_id,
            message_id,
        } => {
            let sid = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                let author = db::message_author(tx, channel_id, message_id)?
                    .ok_or("Mensagem não encontrada")?;
                if author != uid && !s.can_moderate(uid, admin) {
                    return no_permission();
                }
                db::delete_message(tx, message_id)?;
                if author != uid {
                    log(tx, s.id, Some(uid), "mensagem_apagada", Some(author), "")?;
                }
                Ok(s.id)
            })?;
            st.emit(Event::Send(
                Audience::Server(sid),
                ServerMsg::MessageDeleted {
                    channel_id,
                    message_id,
                },
            ));
            st.emit(Event::ReportsChanged(sid));
        }

        // ---------- denúncias ----------
        ClientMsg::ReportMessage {
            channel_id,
            message_id,
            category,
            details,
        } => {
            if !REPORT_CATEGORIES.contains(&category.as_str()) {
                return Err("Categoria de denúncia inválida".into());
            }
            let details: String = clean_text(&details).chars().take(500).collect();
            let sid = st.mutate(|cache, tx| {
                let i = cache
                    .server_index_of_channel(channel_id)
                    .ok_or("Canal não encontrado")?;
                let s = &cache.servers[i];
                if !s.can_view(uid) {
                    return no_permission();
                }
                let author = db::message_author(tx, channel_id, message_id)?
                    .ok_or("Mensagem não encontrada")?;
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
                message: "Denúncia enviada. Os moderadores não ficam sabendo quem denunciou."
                    .into(),
            });
            st.emit(Event::ReportsChanged(sid));
        }

        ClientMsg::FetchReports { server_id } => {
            let reports = st.reports_for(server_id, uid).ok_or(NO_PERMISSION)?;
            cx.send(ServerMsg::Reports { server_id, reports });
        }

        ClientMsg::ResolveReport {
            report_id,
            delete_message,
        } => {
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
                    log(
                        tx,
                        r.server_id,
                        Some(uid),
                        "mensagem_apagada_por_denuncia",
                        author,
                        &r.category,
                    )?;
                    Ok((r.server_id, Some((r.channel_id, r.message_id))))
                } else {
                    db::delete_report(tx, report_id)?;
                    log(
                        tx,
                        r.server_id,
                        Some(uid),
                        "denuncia_descartada",
                        None,
                        &r.category,
                    )?;
                    Ok((r.server_id, None))
                }
            })?;
            if let Some((channel_id, message_id)) = deleted {
                st.emit(Event::Send(
                    Audience::Server(sid),
                    ServerMsg::MessageDeleted {
                        channel_id,
                        message_id,
                    },
                ));
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

/// Mesma regra do banimento: moderadores agem sobre membros; só quem gerencia age
/// sobre moderadores; ninguém age sobre o dono, sobre admins da instância ou sobre si.
fn check_can_act_on(st: &AppState, s: &Server, actor: Uuid, admin: bool, target: Uuid) -> Res<()> {
    if !s.can_moderate(actor, admin) {
        return no_permission();
    }
    if target == actor {
        return Err("Use os seus próprios controles de voz".into());
    }
    if s.owner_id == Some(target) || st.is_admin(target) {
        return Err(
            "Não é possível moderar o dono do servidor nem um administrador da instância".into(),
        );
    }
    if s.moderators.contains(&target) && !s.can_manage(actor, admin) {
        return no_permission();
    }
    Ok(())
}

const MAX_ATTACHMENTS: usize = 10;
const MAX_REACTION_EMOJIS: usize = 20;

/// Reação: um emoji (pode ter modificadores de tom, ZWJ e seletores de variação).
/// Recusa texto comum, espaços e caracteres de controle.
fn valid_emoji(e: &str) -> bool {
    let n = e.chars().count();
    (1..=12).contains(&n)
        && e.len() <= 48
        && !e.is_ascii()
        && e.chars().all(|c| {
            !c.is_control()
                && !c.is_whitespace()
                && !c.is_ascii_alphanumeric()
                && c != '<'
                && c != '>'
        })
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
        ChannelKind::Voice => clean_text(name)
            .chars()
            .take(32)
            .collect::<String>()
            .trim()
            .to_string(),
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
