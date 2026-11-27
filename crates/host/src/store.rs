use std::{path::Path, sync::{Mutex, MutexGuard}, time::{Duration, SystemTime, UNIX_EPOCH}};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior::Immediate, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::Result;

const ALERT_LIFETIME: i64 = 120;

pub struct Store(Mutex<Connection>);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind { Motion, Sound, Test }

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Motion => "motion",
            Self::Sound => "sound",
            Self::Test => "test",
        }
    }
}

#[derive(Serialize)]
pub struct Event {
    pub id: String,
    pub kind: String,
    pub occurred_at_ms: i64,
    pub score: f32,
}

#[derive(Serialize)]
pub struct History {
    pub events: Vec<Event>,
    pub next_before: Option<i64>,
}

pub struct Pending {
    pub event_id: String,
    pub token: String,
    pub expires: i64,
    pub attempts: i64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(3))?;
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS state (slot INTEGER PRIMARY KEY CHECK (slot = 1), armed INTEGER NOT NULL);
            INSERT OR IGNORE INTO state (slot, armed) VALUES (1, 0);
            CREATE TABLE IF NOT EXISTS pairing (slot INTEGER PRIMARY KEY CHECK (slot = 1), hash BLOB NOT NULL, expires INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS phone (slot INTEGER PRIMARY KEY CHECK (slot = 1), hash BLOB NOT NULL, push_token TEXT);
            CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, kind TEXT NOT NULL, occurred_at_ms INTEGER NOT NULL, score REAL NOT NULL);
            CREATE INDEX IF NOT EXISTS events_age ON events (occurred_at_ms);
            CREATE TABLE IF NOT EXISTS outbox (
                event_id TEXT PRIMARY KEY REFERENCES events(id) ON DELETE CASCADE,
                expires INTEGER NOT NULL, next_at INTEGER NOT NULL, attempts INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS outbox_due ON outbox (next_at);
        ")?;
        Ok(Self(Mutex::new(conn)))
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.0.lock().map_err(|_| "database lock poisoned".into())
    }

    pub fn armed(&self) -> Result<bool> {
        Ok(self.connection()?.query_row("SELECT armed FROM state WHERE slot = 1", [], |row| row.get(0))?)
    }

    pub fn issue_code(&self) -> Result<String> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(Immediate)?;
        let paired: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM phone)", [], |row| row.get(0))?;
        if paired {
            return Err("a phone is already paired; revoke it before pairing again".into());
        }
        let code = random_token();
        tx.execute("INSERT OR REPLACE INTO pairing VALUES (1, ?1, ?2)", params![digest(&code), now() + 300])?;
        tx.commit()?;
        Ok(code)
    }

    pub fn pair(&self, code: &str) -> Result<Option<String>> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(Immediate)?;
        let hash: Option<Vec<u8>> = tx.query_row("SELECT hash FROM pairing WHERE slot = 1 AND expires > ?1", [now()], |row| row.get(0)).optional()?;
        if !hash.is_some_and(|hash| bool::from(hash.ct_eq(&digest(code)))) {
            return Ok(None);
        }
        let credential = random_token();
        tx.execute("INSERT INTO phone (slot, hash) VALUES (1, ?1)", [digest(&credential)])?;
        tx.execute("DELETE FROM pairing", [])?;
        tx.commit()?;
        Ok(Some(credential))
    }

    pub fn revoke(&self) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(Immediate)?;
        tx.execute_batch("DELETE FROM phone; DELETE FROM pairing; DELETE FROM outbox;")?;
        tx.commit()?;
        Ok(())
    }

    pub fn disconnect(&self, bearer: &str) -> Result<bool> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(Immediate)?;
        if tx.execute("DELETE FROM phone WHERE hash = ?1", [digest(bearer)])? == 0 {
            return Ok(false);
        }
        tx.execute("DELETE FROM outbox", [])?;
        tx.commit()?;
        Ok(true)
    }

    pub fn authorized(&self, bearer: &str) -> Result<bool> {
        let hash: Option<Vec<u8>> = self.connection()?.query_row("SELECT hash FROM phone WHERE slot = 1", [], |row| row.get(0)).optional()?;
        Ok(hash.is_some_and(|hash| bool::from(hash.ct_eq(&digest(bearer)))))
    }

    pub fn set_armed(&self, bearer: &str, armed: bool) -> Result<bool> {
        Ok(self.connection()?.execute("UPDATE state SET armed = ?1 WHERE EXISTS (SELECT 1 FROM phone WHERE hash = ?2)", params![armed, digest(bearer)])? == 1)
    }

    pub fn set_push_token(&self, bearer: &str, token: &str) -> Result<bool> {
        Ok(self.connection()?.execute("UPDATE phone SET push_token = ?1 WHERE hash = ?2", params![token, digest(bearer)])? == 1)
    }

    pub fn has_push_token(&self) -> Result<bool> {
        Ok(self.connection()?.query_row("SELECT EXISTS (SELECT 1 FROM phone WHERE push_token IS NOT NULL)", [], |row| row.get(0))?)
    }

    pub fn record(&self, kind: Kind, score: f32, require_armed: bool) -> Result<Option<Event>> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(Immediate)?;
        if require_armed && !tx.query_row("SELECT armed FROM state WHERE slot = 1", [], |row| row.get::<_, bool>(0))? {
            return Ok(None);
        }
        let event = Event {
            id: Uuid::new_v4().to_string(),
            kind: kind.as_str().into(),
            occurred_at_ms: unix_time().as_millis() as i64,
            score,
        };
        tx.execute("INSERT INTO events (id, kind, occurred_at_ms, score) VALUES (?1, ?2, ?3, ?4)",
            params![event.id, event.kind, event.occurred_at_ms, event.score])?;
        let timestamp = event.occurred_at_ms / 1000;
        tx.execute("INSERT INTO outbox (event_id, expires, next_at)
                    SELECT ?1, ?2, ?3 FROM phone WHERE push_token IS NOT NULL",
            params![event.id, timestamp + ALERT_LIFETIME, timestamp])?;
        tx.commit()?;
        Ok(Some(event))
    }

    pub fn event(&self, id: Uuid) -> Result<Option<Event>> {
        Ok(self.connection()?.query_row("SELECT id, kind, occurred_at_ms, score FROM events WHERE id = ?1", [id.to_string()], event_at).optional()?)
    }

    pub fn history(&self, before: Option<i64>, limit: usize) -> Result<History> {
        let conn = self.connection()?;
        let mut statement = conn.prepare("SELECT id, kind, occurred_at_ms, score, seq FROM events WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2")?;
        let rows = statement.query_map(params![before.unwrap_or(i64::MAX), limit + 1], |row| {
            Ok((event_at(row)?, row.get::<_, i64>(4)?))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        let next_before = (rows.len() > limit).then(|| rows[limit - 1].1);
        let events = rows.into_iter().take(limit).map(|(event, _)| event).collect();
        Ok(History { events, next_before })
    }

    pub fn pending(&self) -> Result<Option<Pending>> {
        let conn = self.connection()?;
        let timestamp = now();
        conn.execute("DELETE FROM outbox WHERE expires <= ?1", [timestamp])?;
        Ok(conn.query_row("SELECT event_id, push_token, expires, attempts
            FROM outbox CROSS JOIN phone WHERE next_at <= ?1 AND push_token IS NOT NULL
            ORDER BY next_at, event_id LIMIT 1", [timestamp], |row| {
            Ok(Pending { event_id: row.get(0)?, token: row.get(1)?, expires: row.get(2)?, attempts: row.get(3)? })
        }).optional()?)
    }

    pub fn sent(&self, id: &str) -> Result<()> {
        self.connection()?.execute("DELETE FROM outbox WHERE event_id = ?1", [id])?;
        Ok(())
    }

    pub fn retry(&self, item: &Pending, delay: i64) -> Result<()> {
        self.connection()?.execute("UPDATE outbox SET attempts = attempts + 1, next_at = ?1 WHERE event_id = ?2",
            params![now() + delay, item.event_id])?;
        Ok(())
    }

    pub fn invalid_token(&self, item: &Pending) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(Immediate)?;
        if tx.execute("UPDATE phone SET push_token = NULL WHERE push_token = ?1", [&item.token])? == 1 {
            tx.execute("DELETE FROM outbox", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn prune(&self, retention_days: u64) -> Result<()> {
        self.connection()?.execute("DELETE FROM events WHERE occurred_at_ms < ?1",
            [unix_time().as_millis().saturating_sub(u128::from(retention_days) * 86_400_000) as i64])?;
        Ok(())
    }
}

fn event_at(row: &Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event { id: row.get(0)?, kind: row.get(1)?, occurred_at_ms: row.get(2)?, score: row.get(3)? })
}

fn random_token() -> String {
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn digest(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}

fn unix_time() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
}

pub fn now() -> i64 {
    unix_time().as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_alert_is_removed_while_event_remains() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let credential = store.pair(&store.issue_code().unwrap()).unwrap().unwrap();
        assert!(store.set_push_token(&credential, "test-token").unwrap());
        let event = store.record(Kind::Test, 0.0, false).unwrap().unwrap();
        let pending = store.pending().unwrap().unwrap();
        assert_eq!(pending.event_id, event.id);
        store.retry(&pending, 10).unwrap();
        assert!(store.pending().unwrap().is_none());
        store.connection().unwrap().execute("UPDATE outbox SET expires = 0", []).unwrap();
        assert!(store.pending().unwrap().is_none());
        assert!(store.event(Uuid::parse_str(&event.id).unwrap()).unwrap().is_some());
    }
}
