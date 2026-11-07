//! Camada de banco de dados. **Todo o SQL do Concord fica aqui**: para trocar
//! o SQLite por PostgreSQL no futuro, só este módulo precisa ser reescrito.
//!
//! Modelo de concorrência: uma conexão de escrita (o SQLite aceita um escritor
//! por vez) e várias de leitura, que no modo WAL trabalham em paralelo com a escrita.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use uuid::Uuid;

use crate::model::{
    AttachmentView, Ban, Channel, ChannelKind, Message, ModLogEntry, ReactionView, ReplyPreview,
    Report, ReportView, Server, User,
};

const READERS: usize = 4;

pub struct Db {
    pub writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
}

/// Dados pequenos e muito consultados, mantidos em memória (cópia do banco).
#[derive(Default)]
pub struct Cache {
    pub servers: Vec<Server>,
    pub users: HashMap<Uuid, UserRecord>,
}

/// Registro do titular: prova do consentimento (LGPD art. 8º, §2º) e controle de retenção.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct UserRecord {
    pub name: String,
    pub color: String,
    pub consent_version: u32,
    pub consented_at: i64,
    pub last_seen: i64,
}

/// Migrações, em ordem. Cada uma roda uma única vez (controle por `PRAGMA user_version`).
const MIGRATIONS: &[&str] = &[
    // 1: esquema inicial
    r#"
    CREATE TABLE users (
        id              BLOB PRIMARY KEY,
        name            TEXT NOT NULL,
        color           TEXT NOT NULL,
        consent_version INTEGER NOT NULL,
        consented_at    INTEGER NOT NULL,
        last_seen       INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE servers (
        id          BLOB PRIMARY KEY,
        name        TEXT NOT NULL,
        owner_id    BLOB,
        public      INTEGER NOT NULL,
        invite_code TEXT NOT NULL UNIQUE,
        created_at  INTEGER NOT NULL
    );

    CREATE TABLE channels (
        id        BLOB PRIMARY KEY,
        server_id BLOB NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
        name      TEXT NOT NULL,
        position  INTEGER NOT NULL
    );
    CREATE INDEX channels_server ON channels(server_id, position);

    CREATE TABLE server_members (
        server_id BLOB NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
        user_id   BLOB NOT NULL,
        PRIMARY KEY (server_id, user_id)
    ) WITHOUT ROWID;
    CREATE INDEX server_members_user ON server_members(user_id);

    CREATE TABLE server_moderators (
        server_id BLOB NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
        user_id   BLOB NOT NULL,
        PRIMARY KEY (server_id, user_id)
    ) WITHOUT ROWID;
    CREATE INDEX server_moderators_user ON server_moderators(user_id);

    CREATE TABLE bans (
        server_id BLOB NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
        user_id   BLOB NOT NULL,
        reason    TEXT NOT NULL,
        at        INTEGER NOT NULL,
        PRIMARY KEY (server_id, user_id)
    ) WITHOUT ROWID;
    CREATE INDEX bans_user ON bans(user_id);
    CREATE INDEX bans_at ON bans(at);

    -- id é UUIDv7: os bytes já vêm em ordem cronológica, então (channel_id, id)
    -- serve para paginar e o próprio id serve para a retenção por data.
    CREATE TABLE messages (
        id         BLOB PRIMARY KEY,
        channel_id BLOB NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
        author_id  BLOB NOT NULL,
        content    TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE INDEX messages_channel ON messages(channel_id, id);
    CREATE INDEX messages_author ON messages(author_id);

    CREATE TABLE reports (
        id          BLOB PRIMARY KEY,
        server_id   BLOB NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
        channel_id  BLOB NOT NULL,
        message_id  BLOB NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        reporter_id BLOB NOT NULL,
        category    TEXT NOT NULL,
        details     TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        UNIQUE (message_id, reporter_id)
    );
    CREATE INDEX reports_server ON reports(server_id, created_at);
    CREATE INDEX reports_reporter ON reports(reporter_id);
    CREATE INDEX reports_created ON reports(created_at);

    -- Sem chave estrangeira: o registro sobrevive à exclusão do servidor (responsabilização).
    CREATE TABLE mod_log (
        id        INTEGER PRIMARY KEY,
        at        INTEGER NOT NULL,
        server_id BLOB NOT NULL,
        actor     BLOB,
        action    TEXT NOT NULL,
        target    BLOB,
        detail    TEXT NOT NULL
    );
    CREATE INDEX mod_log_at ON mod_log(at);
    CREATE INDEX mod_log_actor ON mod_log(actor);
    CREATE INDEX mod_log_target ON mod_log(target);
    "#,
    // 2: canais de voz (tipo, limite, taxa de bits, qualidade de vídeo e status)
    r#"
    ALTER TABLE channels ADD COLUMN kind TEXT NOT NULL DEFAULT 'text';
    ALTER TABLE channels ADD COLUMN user_limit INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE channels ADD COLUMN bitrate INTEGER NOT NULL DEFAULT 64;
    ALTER TABLE channels ADD COLUMN video_quality TEXT NOT NULL DEFAULT 'auto';
    ALTER TABLE channels ADD COLUMN status TEXT;
    ALTER TABLE channels ADD COLUMN status_by BLOB;
    CREATE INDEX channels_status_by ON channels(status_by) WHERE status_by IS NOT NULL;
    -- Todo servidor existente ganha um canal de voz "Geral".
    INSERT INTO channels (id, server_id, name, position, kind)
        SELECT randomblob(16), s.id, 'Geral',
               (SELECT COALESCE(MAX(position), 0) + 1 FROM channels c WHERE c.server_id = s.id), 'voice'
        FROM servers s;
    "#,
    // 3: edição, respostas, reações e imagens
    r#"
    ALTER TABLE messages ADD COLUMN edited_at INTEGER;
    -- Sem chave estrangeira: a resposta continua existindo se a original for apagada.
    ALTER TABLE messages ADD COLUMN reply_to BLOB;

    CREATE TABLE reactions (
        message_id BLOB NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        emoji      TEXT NOT NULL,
        user_id    BLOB NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (message_id, emoji, user_id)
    ) WITHOUT ROWID;
    CREATE INDEX reactions_user ON reactions(user_id);

    -- Imagens ficam no banco: apagar a mensagem, o canal ou os dados do titular
    -- (e a retenção) leva o arquivo junto. `data` é a última coluna para que as
    -- consultas de metadados não precisem ler o conteúdo.
    CREATE TABLE attachments (
        id          BLOB PRIMARY KEY,
        message_id  BLOB REFERENCES messages(id) ON DELETE CASCADE,
        channel_id  BLOB NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
        uploader_id BLOB NOT NULL,
        filename    TEXT NOT NULL,
        mime        TEXT NOT NULL,
        size        INTEGER NOT NULL,
        width       INTEGER NOT NULL,
        height      INTEGER NOT NULL,
        created_at  INTEGER NOT NULL,
        data        BLOB NOT NULL
    );
    CREATE INDEX attachments_message ON attachments(message_id);
    CREATE INDEX attachments_uploader ON attachments(uploader_id);
    CREATE INDEX attachments_pending ON attachments(created_at) WHERE message_id IS NULL;
    "#,
    // 4: Soundpad (sons curtos de cada servidor, tocados nos canais de voz).
    r#"
    CREATE TABLE sounds (
        id          BLOB PRIMARY KEY,
        server_id   BLOB NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
        uploader_id BLOB NOT NULL,
        name        TEXT NOT NULL,
        emoji       TEXT,
        duration_ms INTEGER NOT NULL,
        size        INTEGER NOT NULL,
        created_at  INTEGER NOT NULL,
        data        BLOB NOT NULL
    );
    CREATE INDEX sounds_server ON sounds(server_id, created_at);
    CREATE INDEX sounds_uploader ON sounds(uploader_id);
    "#,
];

fn configure(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -32000;
         PRAGMA mmap_size = 268435456;",
    )
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        let writer = Connection::open(path)?;
        // WAL: leitores não bloqueiam o escritor e uma queda não corrompe o banco.
        writer.pragma_update(None, "journal_mode", "WAL")?;
        configure(&writer)?;
        migrate(&writer)?;
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let r = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_URI,
            )?;
            configure(&r)?;
            readers.push(Mutex::new(r));
        }
        Ok(Db {
            writer: Mutex::new(writer),
            readers,
            next_reader: AtomicUsize::new(0),
        })
    }

    /// Executa uma leitura numa das conexões de leitura (em rodízio).
    pub fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let i = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        let conn = self.readers[i].lock().unwrap();
        f(&conn)
    }

    /// Cópia consistente do banco com o servidor rodando.
    pub fn backup_to(&self, dest: &Path) -> rusqlite::Result<()> {
        let dest = dest.to_string_lossy().to_string();
        self.read(|c| c.execute("VACUUM INTO ?1", [dest]).map(|_| ()))
    }

    /// Mantém as estatísticas do planejador de consultas em dia.
    pub fn optimize(&self) {
        let _ = self
            .writer
            .lock()
            .unwrap()
            .execute_batch("PRAGMA optimize;");
    }
}

fn migrate(c: &Connection) -> rusqlite::Result<()> {
    let current: usize = c.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = c.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1)?;
        tx.commit()?;
        println!("Banco de dados: migração {} aplicada", i + 1);
    }
    Ok(())
}

pub fn is_empty(c: &Connection) -> rusqlite::Result<bool> {
    c.query_row("SELECT NOT EXISTS (SELECT 1 FROM servers)", [], |r| {
        r.get(0)
    })
}

/// Menor UUIDv7 possível no instante `ms`: tudo abaixo dele é mais antigo.
pub fn v7_floor(ms: i64) -> Uuid {
    let ms = ms.max(0) as u64;
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    Uuid::from_bytes(b)
}

// ---------- cache ----------

pub fn load_cache(c: &Connection) -> rusqlite::Result<Cache> {
    let mut servers: Vec<Server> = c
        .prepare("SELECT id, name, owner_id, public, invite_code FROM servers ORDER BY created_at, rowid")?
        .query_map([], |r| {
            Ok(Server {
                id: r.get(0)?,
                name: r.get(1)?,
                owner_id: r.get(2)?,
                public: r.get(3)?,
                invite_code: r.get(4)?,
                channels: vec![],
                members: vec![],
                moderators: vec![],
                bans: vec![],
            })
        })?
        .collect::<Result<_, _>>()?;
    let index: HashMap<Uuid, usize> = servers.iter().enumerate().map(|(i, s)| (s.id, i)).collect();

    let mut stmt = c.prepare(
        "SELECT server_id, id, name, kind, user_limit, bitrate, video_quality, status, status_by
         FROM channels ORDER BY position, rowid",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let sid: Uuid = r.get(0)?;
        if let Some(&i) = index.get(&sid) {
            let kind: String = r.get(3)?;
            servers[i].channels.push(Channel {
                id: r.get(1)?,
                name: r.get(2)?,
                kind: ChannelKind::parse(&kind),
                user_limit: r.get(4)?,
                bitrate: r.get(5)?,
                video_quality: r.get(6)?,
                status: r.get(7)?,
                status_by: r.get(8)?,
            });
        }
    }
    for (table, which) in [("server_members", 0), ("server_moderators", 1)] {
        let mut stmt = c.prepare(&format!("SELECT server_id, user_id FROM {table}"))?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let sid: Uuid = r.get(0)?;
            if let Some(&i) = index.get(&sid) {
                let list = if which == 0 {
                    &mut servers[i].members
                } else {
                    &mut servers[i].moderators
                };
                list.push(r.get(1)?);
            }
        }
    }
    let mut stmt = c.prepare("SELECT server_id, user_id, reason, at FROM bans ORDER BY at")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let sid: Uuid = r.get(0)?;
        if let Some(&i) = index.get(&sid) {
            servers[i].bans.push(Ban {
                user_id: r.get(1)?,
                reason: r.get(2)?,
                at: r.get(3)?,
            });
        }
    }

    let users = c
        .prepare("SELECT id, name, color, consent_version, consented_at, last_seen FROM users")?
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                UserRecord {
                    name: r.get(1)?,
                    color: r.get(2)?,
                    consent_version: r.get(3)?,
                    consented_at: r.get(4)?,
                    last_seen: r.get(5)?,
                },
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(Cache { servers, users })
}

// ---------- titulares ----------

pub fn upsert_user(c: &Connection, id: Uuid, u: &UserRecord) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO users (id, name, color, consent_version, consented_at, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET name = ?2, color = ?3, consent_version = ?4,
                                       consented_at = ?5, last_seen = ?6",
        params![
            id,
            u.name,
            u.color,
            u.consent_version,
            u.consented_at,
            u.last_seen
        ],
    )?;
    Ok(())
}

pub fn touch_user(c: &Connection, id: Uuid, at: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE users SET last_seen = ?2 WHERE id = ?1",
        params![id, at],
    )?;
    Ok(())
}

// ---------- servidores, canais e papéis ----------

pub fn insert_server(c: &Connection, s: &Server, created_at: i64) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO servers (id, name, owner_id, public, invite_code, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![s.id, s.name, s.owner_id, s.public, s.invite_code, created_at],
    )?;
    for (pos, ch) in s.channels.iter().enumerate() {
        insert_channel(c, s.id, ch, pos as i64)?;
    }
    for m in &s.members {
        set_role(c, "server_members", s.id, *m, true)?;
    }
    for m in &s.moderators {
        set_role(c, "server_moderators", s.id, *m, true)?;
    }
    for b in &s.bans {
        insert_ban(c, s.id, b)?;
    }
    Ok(())
}

pub fn update_server(c: &Connection, s: &Server) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE servers SET name = ?2, owner_id = ?3, public = ?4, invite_code = ?5 WHERE id = ?1",
        params![s.id, s.name, s.owner_id, s.public, s.invite_code],
    )?;
    Ok(())
}

/// Apaga o servidor; canais, mensagens, denúncias e papéis vão junto (ON DELETE CASCADE).
pub fn delete_server(c: &Connection, id: Uuid) -> rusqlite::Result<()> {
    c.execute("DELETE FROM servers WHERE id = ?1", [id])?;
    Ok(())
}

pub fn insert_channel(
    c: &Connection,
    server_id: Uuid,
    ch: &Channel,
    position: i64,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO channels (id, server_id, name, position, kind, user_limit, bitrate, video_quality, status, status_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            ch.id,
            server_id,
            ch.name,
            position,
            ch.kind.as_str(),
            ch.user_limit,
            ch.bitrate,
            ch.video_quality,
            ch.status,
            ch.status_by
        ],
    )?;
    Ok(())
}

/// Grava nome, configurações e status do canal.
pub fn update_channel(c: &Connection, ch: &Channel) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE channels SET name = ?2, user_limit = ?3, bitrate = ?4, video_quality = ?5,
                             status = ?6, status_by = ?7
         WHERE id = ?1",
        params![
            ch.id,
            ch.name,
            ch.user_limit,
            ch.bitrate,
            ch.video_quality,
            ch.status,
            ch.status_by
        ],
    )?;
    Ok(())
}

pub fn delete_channel(c: &Connection, id: Uuid) -> rusqlite::Result<()> {
    c.execute("DELETE FROM channels WHERE id = ?1", [id])?;
    Ok(())
}

/// `table` é sempre uma constante do código ("server_members" ou "server_moderators").
pub fn set_role(
    c: &Connection,
    table: &str,
    server_id: Uuid,
    user_id: Uuid,
    on: bool,
) -> rusqlite::Result<()> {
    let sql = if on {
        format!("INSERT OR IGNORE INTO {table} (server_id, user_id) VALUES (?1, ?2)")
    } else {
        format!("DELETE FROM {table} WHERE server_id = ?1 AND user_id = ?2")
    };
    c.execute(&sql, params![server_id, user_id])?;
    Ok(())
}

pub fn insert_ban(c: &Connection, server_id: Uuid, b: &Ban) -> rusqlite::Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO bans (server_id, user_id, reason, at) VALUES (?1, ?2, ?3, ?4)",
        params![server_id, b.user_id, b.reason, b.at],
    )?;
    Ok(())
}

pub fn delete_ban(c: &Connection, server_id: Uuid, user_id: Uuid) -> rusqlite::Result<()> {
    c.execute(
        "DELETE FROM bans WHERE server_id = ?1 AND user_id = ?2",
        params![server_id, user_id],
    )?;
    Ok(())
}

// ---------- mensagens ----------

pub fn insert_message(c: &Connection, m: &Message) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO messages (id, channel_id, author_id, content, created_at, edited_at, reply_to)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?
    .execute(params![
        m.id,
        m.channel_id,
        m.author.id,
        m.content,
        m.timestamp,
        m.edited_at,
        m.reply_to.as_ref().map(|r| r.id)
    ])?;
    Ok(())
}

/// Edição: guarda só a versão atual (sem histórico de edições).
pub fn edit_message(c: &Connection, id: Uuid, content: &str, at: i64) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE messages SET content = ?2, edited_at = ?3 WHERE id = ?1",
        params![id, content, at],
    )? > 0)
}

/// Prévia da mensagem respondida (autor e trecho), se ainda existir.
pub fn reply_preview(
    c: &Connection,
    channel_id: Uuid,
    id: Uuid,
) -> rusqlite::Result<Option<ReplyPreview>> {
    c.query_row(
        "SELECT m.author_id, COALESCE(u.name, 'Desconhecido'), COALESCE(u.color, '#5865f2'), m.content
         FROM messages m LEFT JOIN users u ON u.id = m.author_id
         WHERE m.id = ?1 AND m.channel_id = ?2",
        params![id, channel_id],
        |r| {
            Ok(ReplyPreview {
                id,
                author: Some(User { id: r.get(0)?, name: r.get(1)?, color: r.get(2)? }),
                excerpt: excerpt(&r.get::<_, String>(3)?),
            })
        },
    )
    .optional()
}

fn excerpt(s: &str) -> String {
    let mut out: String = s.chars().take(120).collect();
    if s.chars().count() > 120 {
        out.push('…');
    }
    out
}

// ---------- reações ----------

pub fn set_reaction(
    c: &Connection,
    message_id: Uuid,
    user_id: Uuid,
    emoji: &str,
    on: bool,
    at: i64,
) -> rusqlite::Result<bool> {
    let n = if on {
        c.execute(
            "INSERT OR IGNORE INTO reactions (message_id, emoji, user_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![message_id, emoji, user_id, at],
        )?
    } else {
        c.execute(
            "DELETE FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND user_id = ?3",
            params![message_id, emoji, user_id],
        )?
    };
    Ok(n > 0)
}

pub fn reaction_emoji_count(c: &Connection, message_id: Uuid) -> rusqlite::Result<usize> {
    c.query_row(
        "SELECT COUNT(DISTINCT emoji) FROM reactions WHERE message_id = ?1",
        [message_id],
        |r| r.get(0),
    )
}

pub fn has_reaction(c: &Connection, message_id: Uuid, emoji: &str) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS (SELECT 1 FROM reactions WHERE message_id = ?1 AND emoji = ?2)",
        params![message_id, emoji],
        |r| r.get(0),
    )
}

// ---------- imagens ----------

pub struct NewAttachment<'a> {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub uploader_id: Uuid,
    pub filename: &'a str,
    pub mime: &'a str,
    pub width: u32,
    pub height: u32,
    pub data: &'a [u8],
    pub created_at: i64,
}

/// Guarda uma imagem ainda sem mensagem (pendente até o envio).
pub fn insert_attachment(c: &Connection, a: &NewAttachment) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO attachments (id, message_id, channel_id, uploader_id, filename, mime, size, width, height, created_at, data)
         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            a.id,
            a.channel_id,
            a.uploader_id,
            a.filename,
            a.mime,
            a.data.len() as i64,
            a.width,
            a.height,
            a.created_at,
            a.data
        ],
    )?;
    Ok(())
}

pub fn count_pending_attachments(c: &Connection, uploader_id: Uuid) -> rusqlite::Result<usize> {
    c.query_row(
        "SELECT COUNT(*) FROM attachments WHERE uploader_id = ?1 AND message_id IS NULL",
        [uploader_id],
        |r| r.get(0),
    )
}

/// Liga uma imagem pendente à mensagem. Só funciona se for de quem envia e do mesmo canal.
pub fn link_attachment(
    c: &Connection,
    id: Uuid,
    message_id: Uuid,
    uploader_id: Uuid,
    channel_id: Uuid,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE attachments SET message_id = ?2
         WHERE id = ?1 AND uploader_id = ?3 AND channel_id = ?4 AND message_id IS NULL",
        params![id, message_id, uploader_id, channel_id],
    )? > 0)
}

/// Conteúdo para servir. Só imagens já ligadas a uma mensagem.
pub fn attachment_file(
    c: &Connection,
    id: Uuid,
) -> rusqlite::Result<Option<(String, String, Vec<u8>)>> {
    c.query_row(
        "SELECT mime, filename, data FROM attachments WHERE id = ?1 AND message_id IS NOT NULL",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
}

/// Imagens enviadas e nunca usadas numa mensagem (ex.: aba fechada) somem em 1 hora.
pub fn purge_pending_attachments(c: &Connection, older_than_ms: i64) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM attachments WHERE message_id IS NULL AND created_at < ?1",
        [older_than_ms],
    )
}

/// Completa as mensagens com reações e imagens (a URL das imagens é assinada por quem chama).
pub fn hydrate(
    c: &Connection,
    list: &mut [Message],
    sign: &dyn Fn(Uuid) -> String,
) -> rusqlite::Result<()> {
    let mut reactions = c.prepare_cached(
        "SELECT emoji, user_id FROM reactions WHERE message_id = ?1 ORDER BY created_at",
    )?;
    let mut attachments = c.prepare_cached(
        "SELECT id, filename, mime, size, width, height FROM attachments WHERE message_id = ?1 ORDER BY created_at",
    )?;
    for m in list.iter_mut() {
        let mut rows = reactions.query([m.id])?;
        while let Some(r) = rows.next()? {
            let emoji: String = r.get(0)?;
            let user: Uuid = r.get(1)?;
            match m.reactions.iter_mut().find(|x| x.emoji == emoji) {
                Some(x) => x.users.push(user),
                None => m.reactions.push(ReactionView {
                    emoji,
                    users: vec![user],
                }),
            }
        }
        m.attachments = attachments
            .query_map([m.id], |r| {
                let id: Uuid = r.get(0)?;
                Ok(AttachmentView {
                    id,
                    url: sign(id),
                    name: r.get(1)?,
                    mime: r.get(2)?,
                    size: r.get(3)?,
                    width: r.get(4)?,
                    height: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
    }
    Ok(())
}

pub fn message_author(
    c: &Connection,
    channel_id: Uuid,
    message_id: Uuid,
) -> rusqlite::Result<Option<Uuid>> {
    c.query_row(
        "SELECT author_id FROM messages WHERE id = ?1 AND channel_id = ?2",
        params![message_id, channel_id],
        |r| r.get(0),
    )
    .optional()
}

/// Apaga uma mensagem (as denúncias sobre ela vão junto).
pub fn delete_message(c: &Connection, message_id: Uuid) -> rusqlite::Result<bool> {
    Ok(c.execute("DELETE FROM messages WHERE id = ?1", [message_id])? > 0)
}

pub fn delete_user_messages_in_server(
    c: &Connection,
    server_id: Uuid,
    user_id: Uuid,
) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM messages WHERE author_id = ?2
           AND channel_id IN (SELECT id FROM channels WHERE server_id = ?1)",
        params![server_id, user_id],
    )
}

fn message_from_row(r: &Row) -> rusqlite::Result<Message> {
    let reply_id: Option<Uuid> = r.get(8)?;
    let reply_to = match reply_id {
        None => None,
        Some(id) => {
            let author_id: Option<Uuid> = r.get(9)?;
            Some(match author_id {
                Some(aid) => ReplyPreview {
                    id,
                    author: Some(User {
                        id: aid,
                        name: r.get(10)?,
                        color: r.get(11)?,
                    }),
                    excerpt: excerpt(&r.get::<_, String>(12)?),
                },
                None => ReplyPreview {
                    id,
                    author: None,
                    excerpt: String::new(),
                },
            })
        }
    };
    Ok(Message {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        author: User {
            id: r.get(2)?,
            name: r.get(3)?,
            color: r.get(4)?,
        },
        content: r.get(5)?,
        timestamp: r.get(6)?,
        edited_at: r.get(7)?,
        reply_to,
        reactions: vec![],
        attachments: vec![],
    })
}

/// 13 colunas (0–12); consultas que acrescentam colunas começam no índice 13.
const MESSAGE_COLUMNS: &str = "m.id, m.channel_id, m.author_id,
     COALESCE(u.name, 'Desconhecido'), COALESCE(u.color, '#5865f2'), m.content, m.created_at,
     m.edited_at, m.reply_to, rm.author_id, COALESCE(ru.name, 'Desconhecido'),
     COALESCE(ru.color, '#5865f2'), COALESCE(rm.content, '')";
const MESSAGE_JOINS: &str = "LEFT JOIN users u ON u.id = m.author_id
     LEFT JOIN messages rm ON rm.id = m.reply_to
     LEFT JOIN users ru ON ru.id = rm.author_id";

/// Página de histórico em ordem cronológica. Com `before`, devolve as mensagens
/// anteriores a ela. Paginação por chave: o custo não cresce com a profundidade.
pub fn history(
    c: &Connection,
    channel_id: Uuid,
    before: Option<Uuid>,
    limit: usize,
) -> rusqlite::Result<Vec<Message>> {
    let mut list: Vec<Message> = match before {
        Some(b) => c
            .prepare_cached(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages m {MESSAGE_JOINS}
                 WHERE m.channel_id = ?1 AND m.id < ?2 ORDER BY m.id DESC LIMIT ?3"
            ))?
            .query_map(params![channel_id, b, limit as i64], message_from_row)?
            .collect::<Result<_, _>>()?,
        None => c
            .prepare_cached(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages m {MESSAGE_JOINS}
                 WHERE m.channel_id = ?1 ORDER BY m.id DESC LIMIT ?2"
            ))?
            .query_map(params![channel_id, limit as i64], message_from_row)?
            .collect::<Result<_, _>>()?,
    };
    list.reverse();
    Ok(list)
}

// ---------- denúncias ----------

pub fn insert_report(c: &Connection, r: &Report) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO reports (id, server_id, channel_id, message_id, reporter_id, category, details, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![r.id, r.server_id, r.channel_id, r.message_id, r.reporter_id, r.category, r.details, r.created_at],
    )?;
    Ok(())
}

pub fn report_exists(
    c: &Connection,
    message_id: Uuid,
    reporter_id: Uuid,
) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS (SELECT 1 FROM reports WHERE message_id = ?1 AND reporter_id = ?2)",
        params![message_id, reporter_id],
        |r| r.get(0),
    )
}

pub fn count_reports(c: &Connection, server_id: Uuid) -> rusqlite::Result<usize> {
    c.query_row(
        "SELECT COUNT(*) FROM reports WHERE server_id = ?1",
        [server_id],
        |r| r.get(0),
    )
}

pub fn get_report(c: &Connection, id: Uuid) -> rusqlite::Result<Option<Report>> {
    c.query_row(
        "SELECT id, server_id, channel_id, message_id, reporter_id, category, details, created_at
         FROM reports WHERE id = ?1",
        [id],
        |r| {
            Ok(Report {
                id: r.get(0)?,
                server_id: r.get(1)?,
                channel_id: r.get(2)?,
                message_id: r.get(3)?,
                reporter_id: r.get(4)?,
                category: r.get(5)?,
                details: r.get(6)?,
                created_at: r.get(7)?,
            })
        },
    )
    .optional()
}

pub fn delete_report(c: &Connection, id: Uuid) -> rusqlite::Result<()> {
    c.execute("DELETE FROM reports WHERE id = ?1", [id])?;
    Ok(())
}

/// Denúncias como os moderadores veem: **sem** quem denunciou.
pub fn reports_view(c: &Connection, server_id: Uuid) -> rusqlite::Result<Vec<ReportView>> {
    c.prepare_cached(&format!(
        "SELECT {MESSAGE_COLUMNS}, r.id, ch.name, r.category, r.details, r.created_at
         FROM reports r
         JOIN messages m ON m.id = r.message_id
         JOIN channels ch ON ch.id = r.channel_id
         {MESSAGE_JOINS}
         WHERE r.server_id = ?1 ORDER BY r.created_at"
    ))?
    .query_map([server_id], |r| {
        let message = message_from_row(r)?;
        Ok(ReportView {
            id: r.get(13)?,
            channel_id: message.channel_id,
            channel_name: r.get(14)?,
            message,
            category: r.get(15)?,
            details: r.get(16)?,
            created_at: r.get(17)?,
        })
    })?
    .collect()
}

// ---------- registro de moderação ----------

pub fn log(c: &Connection, e: &ModLogEntry) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO mod_log (at, server_id, actor, action, target, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![e.at, e.server_id, e.actor, e.action, e.target, e.detail],
    )?;
    Ok(())
}

// ---------- LGPD: exportação ----------

pub struct ExportMessage {
    pub id: Uuid,
    pub server: String,
    pub channel: String,
    pub content: String,
    pub at: i64,
    pub edited_at: Option<i64>,
    pub reply_to: Option<Uuid>,
}

pub struct ExportAttachment {
    pub id: Uuid,
    pub message_id: Option<Uuid>,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub at: i64,
}

pub struct ExportRows {
    pub messages: Vec<ExportMessage>,
    /// (mensagem, emoji, data)
    pub reactions: Vec<(Uuid, String, i64)>,
    pub attachments: Vec<ExportAttachment>,
    pub sounds: Vec<ExportSound>,
    pub reports: Vec<Report>,
    pub mod_log: Vec<ModLogEntry>,
}

pub fn export_rows(c: &Connection, uid: Uuid) -> rusqlite::Result<ExportRows> {
    let messages = c
        .prepare(
            "SELECT m.id, s.name, ch.name, m.content, m.created_at, m.edited_at, m.reply_to
             FROM messages m JOIN channels ch ON ch.id = m.channel_id JOIN servers s ON s.id = ch.server_id
             WHERE m.author_id = ?1 ORDER BY m.id",
        )?
        .query_map([uid], |r| {
            Ok(ExportMessage {
                id: r.get(0)?,
                server: r.get(1)?,
                channel: r.get(2)?,
                content: r.get(3)?,
                at: r.get(4)?,
                edited_at: r.get(5)?,
                reply_to: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let reactions = c
        .prepare("SELECT message_id, emoji, created_at FROM reactions WHERE user_id = ?1 ORDER BY created_at")?
        .query_map([uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let attachments = c
        .prepare(
            "SELECT id, message_id, filename, mime, size, created_at FROM attachments
             WHERE uploader_id = ?1 ORDER BY created_at",
        )?
        .query_map([uid], |r| {
            Ok(ExportAttachment {
                id: r.get(0)?,
                message_id: r.get(1)?,
                filename: r.get(2)?,
                mime: r.get(3)?,
                size: r.get(4)?,
                at: r.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let sounds = c
        .prepare(
            "SELECT so.id, s.name, so.name, so.emoji, so.duration_ms, so.size, so.created_at
             FROM sounds so JOIN servers s ON s.id = so.server_id
             WHERE so.uploader_id = ?1 ORDER BY so.created_at",
        )?
        .query_map([uid], |r| {
            Ok(ExportSound {
                id: r.get(0)?,
                server: r.get(1)?,
                name: r.get(2)?,
                emoji: r.get(3)?,
                duration_ms: r.get(4)?,
                size: r.get(5)?,
                at: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let reports = c
        .prepare(
            "SELECT id, server_id, channel_id, message_id, reporter_id, category, details, created_at
             FROM reports WHERE reporter_id = ?1 ORDER BY created_at",
        )?
        .query_map([uid], |r| {
            Ok(Report {
                id: r.get(0)?,
                server_id: r.get(1)?,
                channel_id: r.get(2)?,
                message_id: r.get(3)?,
                reporter_id: r.get(4)?,
                category: r.get(5)?,
                details: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mod_log = c
        .prepare(
            "SELECT at, server_id, actor, action, target, detail FROM mod_log
             WHERE actor = ?1 OR target = ?1 ORDER BY at",
        )?
        .query_map([uid], |r| {
            Ok(ModLogEntry {
                at: r.get(0)?,
                server_id: r.get(1)?,
                actor: r.get(2)?,
                action: r.get(3)?,
                target: r.get(4)?,
                detail: r.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(ExportRows {
        messages,
        reactions,
        attachments,
        sounds,
        reports,
        mod_log,
    })
}

// ---------- Soundpad ----------

pub struct ExportSound {
    pub id: Uuid,
    pub server: String,
    pub name: String,
    pub emoji: Option<String>,
    pub duration_ms: i64,
    pub size: i64,
    pub at: i64,
}

pub struct NewSound<'a> {
    pub id: Uuid,
    pub server_id: Uuid,
    pub uploader_id: Uuid,
    pub name: &'a str,
    pub emoji: Option<&'a str>,
    pub duration_ms: u32,
    pub data: &'a [u8],
    pub created_at: i64,
}

pub fn insert_sound(c: &Connection, s: &NewSound) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO sounds (id, server_id, uploader_id, name, emoji, duration_ms, size, created_at, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            s.id,
            s.server_id,
            s.uploader_id,
            s.name,
            s.emoji,
            s.duration_ms,
            s.data.len() as i64,
            s.created_at,
            s.data
        ],
    )?;
    Ok(())
}

pub fn count_sounds(c: &Connection, server_id: Uuid) -> rusqlite::Result<usize> {
    c.query_row(
        "SELECT COUNT(*) FROM sounds WHERE server_id = ?1",
        [server_id],
        |r| r.get(0),
    )
}

/// (id, nome, emoji, duração em ms)
pub type SoundRow = (Uuid, String, Option<String>, u32);

/// Metadados dos sons de um servidor (sem quem enviou: não é preciso para tocar).
pub fn list_sounds(c: &Connection, server_id: Uuid) -> rusqlite::Result<Vec<SoundRow>> {
    c.prepare_cached(
        "SELECT id, name, emoji, duration_ms FROM sounds WHERE server_id = ?1 ORDER BY created_at",
    )?
    .query_map([server_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?
    .collect()
}

/// (servidor, quem enviou) de um som.
pub fn sound_owner(c: &Connection, id: Uuid) -> rusqlite::Result<Option<(Uuid, Uuid)>> {
    c.query_row(
        "SELECT server_id, uploader_id FROM sounds WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

pub fn sound_data(c: &Connection, id: Uuid) -> rusqlite::Result<Option<Vec<u8>>> {
    c.query_row("SELECT data FROM sounds WHERE id = ?1", [id], |r| r.get(0))
        .optional()
}

pub fn delete_sound(c: &Connection, id: Uuid) -> rusqlite::Result<bool> {
    Ok(c.execute("DELETE FROM sounds WHERE id = ?1", [id])? > 0)
}

// ---------- LGPD: eliminação ----------

/// Apaga tudo do titular que está no banco (os papéis em servidores são tratados
/// por quem chama, junto com o cache). Devolve quantas mensagens foram apagadas.
pub fn delete_user_rows(c: &Connection, uid: Uuid) -> rusqlite::Result<usize> {
    // Imagens e reações das mensagens apagadas vão junto (ON DELETE CASCADE).
    let removed = c.execute("DELETE FROM messages WHERE author_id = ?1", [uid])?;
    c.execute("DELETE FROM attachments WHERE uploader_id = ?1", [uid])?;
    c.execute("DELETE FROM reactions WHERE user_id = ?1", [uid])?;
    c.execute("DELETE FROM sounds WHERE uploader_id = ?1", [uid])?;
    c.execute("DELETE FROM reports WHERE reporter_id = ?1", [uid])?;
    c.execute("DELETE FROM server_members WHERE user_id = ?1", [uid])?;
    c.execute("DELETE FROM server_moderators WHERE user_id = ?1", [uid])?;
    c.execute("DELETE FROM bans WHERE user_id = ?1", [uid])?;
    c.execute(
        "UPDATE channels SET status = NULL, status_by = NULL WHERE status_by = ?1",
        [uid],
    )?;
    c.execute("UPDATE mod_log SET actor = NULL WHERE actor = ?1", [uid])?;
    c.execute("UPDATE mod_log SET target = NULL WHERE target = ?1", [uid])?;
    c.execute("DELETE FROM users WHERE id = ?1", [uid])?;
    Ok(removed)
}

// ---------- LGPD: retenção ----------

/// Apaga um lote de mensagens mais antigas que `cutoff_ms`. Lotes pequenos
/// mantêm cada transação curta, para não travar o chat.
pub fn purge_messages_batch(
    c: &Connection,
    cutoff_ms: i64,
    batch: usize,
) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM messages WHERE id IN (SELECT id FROM messages WHERE id < ?1 LIMIT ?2)",
        params![v7_floor(cutoff_ms), batch as i64],
    )
}

pub struct PurgeCounts {
    pub reports: usize,
    pub mod_log: usize,
    pub bans: usize,
}

pub fn purge_small_tables(c: &Connection, cutoff_ms: i64) -> rusqlite::Result<PurgeCounts> {
    Ok(PurgeCounts {
        reports: c.execute("DELETE FROM reports WHERE created_at < ?1", [cutoff_ms])?,
        mod_log: c.execute("DELETE FROM mod_log WHERE at < ?1", [cutoff_ms])?,
        bans: c.execute("DELETE FROM bans WHERE at < ?1", [cutoff_ms])?,
    })
}

/// Titulares inativos e sem mensagens guardadas.
pub fn inactive_users(c: &Connection, cutoff_ms: i64) -> rusqlite::Result<Vec<Uuid>> {
    c.prepare(
        "SELECT id FROM users WHERE last_seen < ?1
           AND NOT EXISTS (SELECT 1 FROM messages WHERE author_id = users.id)",
    )?
    .query_map([cutoff_ms], |r| r.get(0))?
    .collect()
}

pub fn delete_user_record(c: &Connection, uid: Uuid) -> rusqlite::Result<()> {
    c.execute("DELETE FROM users WHERE id = ?1", [uid])?;
    Ok(())
}
