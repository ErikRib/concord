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

use crate::model::{Ban, Channel, Message, ModLogEntry, Report, ReportView, Server, User};

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
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
            )?;
            configure(&r)?;
            readers.push(Mutex::new(r));
        }
        Ok(Db { writer: Mutex::new(writer), readers, next_reader: AtomicUsize::new(0) })
    }

    /// Executa uma leitura numa das conexões de leitura (em rodízio).
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
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
        let _ = self.writer.lock().unwrap().execute_batch("PRAGMA optimize;");
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
    c.query_row("SELECT NOT EXISTS (SELECT 1 FROM servers)", [], |r| r.get(0))
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

    let mut stmt = c.prepare("SELECT server_id, id, name FROM channels ORDER BY position, rowid")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let sid: Uuid = r.get(0)?;
        if let Some(&i) = index.get(&sid) {
            servers[i].channels.push(Channel { id: r.get(1)?, name: r.get(2)? });
        }
    }
    for (table, which) in [("server_members", 0), ("server_moderators", 1)] {
        let mut stmt = c.prepare(&format!("SELECT server_id, user_id FROM {table}"))?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let sid: Uuid = r.get(0)?;
            if let Some(&i) = index.get(&sid) {
                let list = if which == 0 { &mut servers[i].members } else { &mut servers[i].moderators };
                list.push(r.get(1)?);
            }
        }
    }
    let mut stmt = c.prepare("SELECT server_id, user_id, reason, at FROM bans ORDER BY at")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let sid: Uuid = r.get(0)?;
        if let Some(&i) = index.get(&sid) {
            servers[i].bans.push(Ban { user_id: r.get(1)?, reason: r.get(2)?, at: r.get(3)? });
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
        params![id, u.name, u.color, u.consent_version, u.consented_at, u.last_seen],
    )?;
    Ok(())
}

pub fn touch_user(c: &Connection, id: Uuid, at: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE users SET last_seen = ?2 WHERE id = ?1", params![id, at])?;
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

pub fn insert_channel(c: &Connection, server_id: Uuid, ch: &Channel, position: i64) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO channels (id, server_id, name, position) VALUES (?1, ?2, ?3, ?4)",
        params![ch.id, server_id, ch.name, position],
    )?;
    Ok(())
}

pub fn delete_channel(c: &Connection, id: Uuid) -> rusqlite::Result<()> {
    c.execute("DELETE FROM channels WHERE id = ?1", [id])?;
    Ok(())
}

/// `table` é sempre uma constante do código ("server_members" ou "server_moderators").
pub fn set_role(c: &Connection, table: &str, server_id: Uuid, user_id: Uuid, on: bool) -> rusqlite::Result<()> {
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
    c.execute("DELETE FROM bans WHERE server_id = ?1 AND user_id = ?2", params![server_id, user_id])?;
    Ok(())
}

// ---------- mensagens ----------

pub fn insert_message(c: &Connection, m: &Message) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO messages (id, channel_id, author_id, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![m.id, m.channel_id, m.author.id, m.content, m.timestamp])?;
    Ok(())
}

pub fn message_author(c: &Connection, channel_id: Uuid, message_id: Uuid) -> rusqlite::Result<Option<Uuid>> {
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

pub fn delete_user_messages_in_server(c: &Connection, server_id: Uuid, user_id: Uuid) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM messages WHERE author_id = ?2
           AND channel_id IN (SELECT id FROM channels WHERE server_id = ?1)",
        params![server_id, user_id],
    )
}

fn message_from_row(r: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        author: User { id: r.get(2)?, name: r.get(3)?, color: r.get(4)? },
        content: r.get(5)?,
        timestamp: r.get(6)?,
    })
}

const MESSAGE_COLUMNS: &str = "m.id, m.channel_id, m.author_id,
     COALESCE(u.name, 'Desconhecido'), COALESCE(u.color, '#5865f2'), m.content, m.created_at";

/// Página de histórico em ordem cronológica. Com `before`, devolve as mensagens
/// anteriores a ela. Paginação por chave: o custo não cresce com a profundidade.
pub fn history(c: &Connection, channel_id: Uuid, before: Option<Uuid>, limit: usize) -> rusqlite::Result<Vec<Message>> {
    let mut list: Vec<Message> = match before {
        Some(b) => c
            .prepare_cached(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages m LEFT JOIN users u ON u.id = m.author_id
                 WHERE m.channel_id = ?1 AND m.id < ?2 ORDER BY m.id DESC LIMIT ?3"
            ))?
            .query_map(params![channel_id, b, limit as i64], message_from_row)?
            .collect::<Result<_, _>>()?,
        None => c
            .prepare_cached(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages m LEFT JOIN users u ON u.id = m.author_id
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

pub fn report_exists(c: &Connection, message_id: Uuid, reporter_id: Uuid) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS (SELECT 1 FROM reports WHERE message_id = ?1 AND reporter_id = ?2)",
        params![message_id, reporter_id],
        |r| r.get(0),
    )
}

pub fn count_reports(c: &Connection, server_id: Uuid) -> rusqlite::Result<usize> {
    c.query_row("SELECT COUNT(*) FROM reports WHERE server_id = ?1", [server_id], |r| r.get(0))
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
         LEFT JOIN users u ON u.id = m.author_id
         WHERE r.server_id = ?1 ORDER BY r.created_at"
    ))?
    .query_map([server_id], |r| {
        let message = message_from_row(r)?;
        Ok(ReportView {
            id: r.get(7)?,
            channel_id: message.channel_id,
            channel_name: r.get(8)?,
            message,
            category: r.get(9)?,
            details: r.get(10)?,
            created_at: r.get(11)?,
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

pub struct ExportRows {
    /// (id, servidor, canal, conteúdo, data)
    pub messages: Vec<(Uuid, String, String, String, i64)>,
    pub reports: Vec<Report>,
    pub mod_log: Vec<ModLogEntry>,
}

pub fn export_rows(c: &Connection, uid: Uuid) -> rusqlite::Result<ExportRows> {
    let messages = c
        .prepare(
            "SELECT m.id, s.name, ch.name, m.content, m.created_at
             FROM messages m JOIN channels ch ON ch.id = m.channel_id JOIN servers s ON s.id = ch.server_id
             WHERE m.author_id = ?1 ORDER BY m.id",
        )?
        .query_map([uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
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
    Ok(ExportRows { messages, reports, mod_log })
}

// ---------- LGPD: eliminação ----------

/// Apaga tudo do titular que está no banco (os papéis em servidores são tratados
/// por quem chama, junto com o cache). Devolve quantas mensagens foram apagadas.
pub fn delete_user_rows(c: &Connection, uid: Uuid) -> rusqlite::Result<usize> {
    let removed = c.execute("DELETE FROM messages WHERE author_id = ?1", [uid])?;
    c.execute("DELETE FROM reports WHERE reporter_id = ?1", [uid])?;
    c.execute("DELETE FROM server_members WHERE user_id = ?1", [uid])?;
    c.execute("DELETE FROM server_moderators WHERE user_id = ?1", [uid])?;
    c.execute("DELETE FROM bans WHERE user_id = ?1", [uid])?;
    c.execute("UPDATE mod_log SET actor = NULL WHERE actor = ?1", [uid])?;
    c.execute("UPDATE mod_log SET target = NULL WHERE target = ?1", [uid])?;
    c.execute("DELETE FROM users WHERE id = ?1", [uid])?;
    Ok(removed)
}

// ---------- LGPD: retenção ----------

/// Apaga um lote de mensagens mais antigas que `cutoff_ms`. Lotes pequenos
/// mantêm cada transação curta, para não travar o chat.
pub fn purge_messages_batch(c: &Connection, cutoff_ms: i64, batch: usize) -> rusqlite::Result<usize> {
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
