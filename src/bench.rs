//! Teste de carga do banco: `cargo run --release -- bench [mensagens]`.
//! Usa um banco temporário (apagado no fim) com dados fictícios.

use std::time::{Duration, Instant};

use rusqlite::TransactionBehavior;
use uuid::{NoContext, Timestamp, Uuid};

use crate::db::{self, Db, UserRecord};
use crate::model::{Channel, ChannelKind, Message, Server, User};
use crate::state::now_ms;

const CHANNELS: usize = 100;
const USERS: usize = 1000;
const DAY_MS: i64 = 86_400_000;

fn fmt_rate(n: usize, d: Duration) -> String {
    format!("{:>10.0}/s", n as f64 / d.as_secs_f64())
}

fn fmt_time(d: Duration) -> String {
    if d.as_secs_f64() >= 1.0 {
        format!("{:.2} s", d.as_secs_f64())
    } else if d.as_millis() >= 1 {
        format!("{:.1} ms", d.as_secs_f64() * 1000.0)
    } else {
        format!("{:.0} µs", d.as_secs_f64() * 1_000_000.0)
    }
}

pub fn run(n: usize) {
    let dir = std::env::temp_dir().join(format!("concord-bench-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bench.db");
    let db = Db::open(&path).expect("abrir banco");
    println!("Teste de carga: {n} mensagens, {CHANNELS} canais, {USERS} pessoas\n");

    // Dados de base.
    let channels: Vec<Channel> = (0..CHANNELS)
        .map(|i| Channel::new(&format!("canal-{i}"), ChannelKind::Text))
        .collect();
    let users: Vec<User> = (0..USERS)
        .map(|i| User {
            id: Uuid::new_v4(),
            name: format!("Pessoa {i}"),
            color: "#5865f2".into(),
        })
        .collect();
    {
        let mut c = db.writer.lock().unwrap();
        let tx = c.transaction().unwrap();
        let server = Server {
            id: Uuid::new_v4(),
            name: "Bench".into(),
            channels: channels.clone(),
            owner_id: None,
            public: true,
            members: vec![],
            moderators: vec![],
            bans: vec![],
            invite_code: "benchbench12".into(),
        };
        db::insert_server(&tx, &server, now_ms()).unwrap();
        for u in &users {
            let rec = UserRecord {
                name: u.name.clone(),
                color: u.color.clone(),
                consent_version: 2,
                consented_at: 0,
                last_seen: now_ms(),
            };
            db::upsert_user(&tx, u.id, &rec).unwrap();
        }
        tx.commit().unwrap();
    }

    // 1) Importação em massa: lotes de 10 mil por transação, espalhadas em 300 dias.
    let start_ms = now_ms() - 300 * DAY_MS;
    let step = (300 * DAY_MS) as f64 / n as f64;
    let t = Instant::now();
    let mut i = 0;
    while i < n {
        let mut c = db.writer.lock().unwrap();
        let tx = c.transaction().unwrap();
        for _ in 0..10_000.min(n - i) {
            let ms = start_ms + (i as f64 * step) as i64;
            let m = Message {
                id: Uuid::new_v7(Timestamp::from_unix(NoContext, (ms / 1000) as u64, (ms % 1000) as u32 * 1_000_000)),
                channel_id: channels[i % CHANNELS].id,
                author: users[i % USERS].clone(),
                content: format!("Mensagem de teste número {i}, com um texto de tamanho parecido com uma conversa real."),
                timestamp: ms,
            };
            db::insert_message(&tx, &m).unwrap();
            i += 1;
        }
        tx.commit().unwrap();
    }
    let bulk = t.elapsed();
    println!(
        "Importação em massa ({n} msgs) ........ {} | {}",
        fmt_time(bulk),
        fmt_rate(n, bulk)
    );

    // 2) Chat ao vivo: uma transação por mensagem, como no uso real.
    let live = 20_000;
    let t = Instant::now();
    for k in 0..live {
        let mut c = db.writer.lock().unwrap();
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let m = Message {
            id: Uuid::now_v7(),
            channel_id: channels[k % CHANNELS].id,
            author: users[k % USERS].clone(),
            content: "Mensagem enviada ao vivo.".into(),
            timestamp: now_ms(),
        };
        db::insert_message(&tx, &m).unwrap();
        tx.commit().unwrap();
    }
    let d = t.elapsed();
    println!(
        "Chat ao vivo (1 transação/msg) ........ {} | {}",
        fmt_time(d),
        fmt_rate(live, d)
    );

    // 3) Abrir um canal: página mais recente.
    let reps = 2000;
    let t = Instant::now();
    for k in 0..reps {
        let page = db
            .read(|c| db::history(c, channels[k % CHANNELS].id, None, 50))
            .unwrap();
        assert_eq!(page.len(), 50);
    }
    println!(
        "Abrir canal (50 msgs mais recentes) ... {} por consulta",
        fmt_time(t.elapsed() / reps as u32)
    );

    // 4) Rolar para o passado: página a partir do meio e do começo do canal.
    let ch = channels[0].id;
    let ids: Vec<Uuid> = db
        .read(|c| {
            c.prepare("SELECT id FROM messages WHERE channel_id = ?1 ORDER BY id")?
                .query_map([ch], |r| r.get(0))?
                .collect()
        })
        .unwrap();
    for (label, idx) in [
        ("meio do histórico", ids.len() / 2),
        ("início do histórico", 60.min(ids.len() - 1)),
    ] {
        let before = ids[idx];
        let t = Instant::now();
        for _ in 0..reps {
            db.read(|c| db::history(c, ch, Some(before), 50)).unwrap();
        }
        println!(
            "Rolar até o {label:<20} {} por consulta",
            fmt_time(t.elapsed() / reps as u32)
        );
    }

    // 5) Direitos do titular.
    let target = users[7].id;
    let t = Instant::now();
    let rows = db.read(|c| db::export_rows(c, target)).unwrap();
    println!(
        "Exportar dados de 1 titular ({} msgs) .. {}",
        rows.messages.len(),
        fmt_time(t.elapsed())
    );
    let t = Instant::now();
    let removed = {
        let mut c = db.writer.lock().unwrap();
        let tx = c.transaction().unwrap();
        let r = db::delete_user_rows(&tx, target).unwrap();
        tx.commit().unwrap();
        r
    };
    println!(
        "Apagar dados de 1 titular ({removed} msgs) ... {}",
        fmt_time(t.elapsed())
    );

    // 6) Retenção de 180 dias: ~40% das mensagens vencem, apagadas em lotes de 5 mil.
    let cutoff = now_ms() - 180 * DAY_MS;
    let t = Instant::now();
    let mut purged = 0;
    loop {
        let c = db.writer.lock().unwrap();
        let k = db::purge_messages_batch(&c, cutoff, 5000).unwrap();
        purged += k;
        if k < 5000 {
            break;
        }
    }
    let d = t.elapsed();
    println!(
        "Retenção ({purged} msgs vencidas) ...... {} | {}",
        fmt_time(d),
        fmt_rate(purged, d)
    );

    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        + std::fs::metadata(path.with_extension("db-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
    println!("\nTamanho do banco: {:.1} MB", size as f64 / 1_048_576.0);

    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
