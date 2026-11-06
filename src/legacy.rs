//! Migração única do formato antigo (`concord-data.json`) para o SQLite.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::Connection;
use serde::Deserialize;
use uuid::{NoContext, Timestamp, Uuid};

use crate::db::{self, UserRecord};
use crate::model::{Message, ModLogEntry, Report, Server};
use crate::state::now_ms;

#[derive(Deserialize)]
struct LegacyData {
    servers: Vec<Server>,
    #[serde(default)]
    messages: HashMap<Uuid, Vec<Message>>,
    #[serde(default)]
    users: HashMap<Uuid, UserRecord>,
    #[serde(default)]
    reports: Vec<Report>,
    #[serde(default)]
    mod_log: Vec<ModLogEntry>,
}

/// UUIDv7 a partir da data original, para manter a ordem cronológica.
fn v7_at(ms: i64) -> Uuid {
    let ms = ms.max(0) as u64;
    Uuid::new_v7(Timestamp::from_unix(
        NoContext,
        ms / 1000,
        (ms % 1000) as u32 * 1_000_000,
    ))
}

pub fn import(conn: &Connection, path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Erro ao ler {}: {e}", path.display()))?;
    let data: LegacyData = serde_json::from_str(&text)
        .map_err(|e| format!("Erro ao interpretar {}: {e}", path.display()))?;

    let run = || -> rusqlite::Result<usize> {
        let tx = conn.unchecked_transaction()?;
        let now = now_ms();
        for (i, s) in data.servers.iter().enumerate() {
            db::insert_server(&tx, s, now + i as i64)?;
        }
        for (id, u) in &data.users {
            db::upsert_user(&tx, *id, u)?;
        }
        // Mensagens ganham ids UUIDv7 com a data original; as denúncias seguem o novo id.
        let mut new_ids: HashMap<Uuid, Uuid> = HashMap::new();
        let mut count = 0;
        for list in data.messages.values() {
            for m in list {
                let mut m = m.clone();
                let new_id = v7_at(m.timestamp);
                new_ids.insert(m.id, new_id);
                m.id = new_id;
                db::insert_message(&tx, &m)?;
                count += 1;
            }
        }
        for r in &data.reports {
            if let Some(&mid) = new_ids.get(&r.message_id) {
                db::insert_report(
                    &tx,
                    &Report {
                        message_id: mid,
                        ..r.clone()
                    },
                )?;
            }
        }
        for e in &data.mod_log {
            db::log(&tx, e)?;
        }
        tx.commit()?;
        Ok(count)
    };
    let count = run().map_err(|e| format!("Erro ao migrar {}: {e}", path.display()))?;

    // O arquivo antigo é renomeado, não apagado, para permitir conferência.
    let done = path.with_extension("json.migrado");
    std::fs::rename(path, &done)
        .map_err(|e| format!("Migração feita, mas não foi possível renomear o JSON: {e}"))?;
    println!(
        "Migração concluída: {} servidor(es), {} titular(es) e {count} mensagem(ns) importados.",
        data.servers.len(),
        data.users.len()
    );
    println!(
        "AVISO (LGPD): o arquivo antigo foi guardado como {}. Ele contém dados pessoais e não é \
         alcançado por \"Apagar meus dados\" nem pela retenção. Apague-o assim que conferir a migração.",
        done.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::db;
    use crate::state::{AppState, Config};
    use uuid::Uuid;

    #[test]
    fn migra_json_antigo_mantendo_ordem_e_denuncias() {
        let dir = std::env::temp_dir().join(format!("concord-legacy-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (sid, cid, uid, m1, m2) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        // Formato da primeira versão: servidor sem dono/convite, mensagens com ids v4.
        let json = format!(
            r##"{{"servers":[{{"id":"{sid}","name":"Antigo","channels":[{{"id":"{cid}","name":"geral"}}]}}],
            "messages":{{"{cid}":[
              {{"id":"{m2}","channel_id":"{cid}","author":{{"id":"{uid}","name":"Ana","color":"#ff0000"}},"content":"segunda","timestamp":2000}},
              {{"id":"{m1}","channel_id":"{cid}","author":{{"id":"{uid}","name":"Ana","color":"#ff0000"}},"content":"primeira","timestamp":1000}}]}},
            "users":{{"{uid}":{{"name":"Ana","color":"#ff0000","consent_version":1,"consented_at":0,"last_seen":0}}}},
            "reports":[{{"id":"{}","server_id":"{sid}","channel_id":"{cid}","message_id":"{m2}","reporter_id":"{}","category":"spam","details":"","created_at":5}}]}}"##,
            Uuid::new_v4(),
            Uuid::new_v4()
        );
        let legacy = dir.join("concord-data.json");
        std::fs::write(&legacy, json).unwrap();
        let config = Config {
            retention_days: 30,
            controller: String::new(),
            contact: String::new(),
            admins: Default::default(),
        };
        let st = AppState::open(&dir.join("t.db"), &legacy, config).unwrap();

        let cache = st.cache.lock().unwrap();
        assert_eq!(cache.servers[0].name, "Antigo");
        assert!(cache.servers[0].public && cache.servers[0].owner_id.is_none());
        drop(cache);
        let hist = st.read(|c| db::history(c, cid, None, 10)).unwrap();
        let order: Vec<_> = hist.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            order,
            ["primeira", "segunda"],
            "ordem cronológica preservada com UUIDv7"
        );
        assert_eq!(
            st.read(|c| db::count_reports(c, sid)).unwrap(),
            1,
            "denúncia segue o novo id da mensagem"
        );
        assert!(!legacy.exists() && dir.join("concord-data.json.migrado").exists());
        drop(st);
        let _ = std::fs::remove_dir_all(dir);
    }
}
