//! Persists `ScanSession`s to SQLite so results survive a process
//! restart — the concrete requirement behind the architecture's
//! "SessionStore" box: large scans take real time, and nobody wants to
//! lose results because the app closed.
//!
//! Deliberately simple schema: each session is stored as a single JSON
//! blob in one row, not normalized across multiple tables. For a tool
//! whose access pattern is "load one session by id" or "list all
//! sessions," a normalized schema would add real complexity (joins,
//! migrations touching several tables) for no queryability benefit we
//! actually need yet. If a future feature needs to query, say, "all
//! files across every session with confidence > 80%," that's the trigger
//! to revisit this and normalize `results` into its own table.

use crate::error::Result;
use crate::scan_session::ScanSession;
use rusqlite::{params, Connection};

pub struct SessionStore {
    conn: Connection,
}

impl SessionStore {
    pub fn open(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                data TEXT NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    /// In-memory store — handy for tests that don't want to touch disk.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL,
                data TEXT NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn save(&self, session: &ScanSession) -> Result<()> {
        let data = serde_json::to_string(session)?;
        let updated_at = chrono_now_string();
        self.conn.execute(
            "INSERT INTO sessions (id, updated_at, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, data = excluded.data",
            params![session.id, updated_at, data],
        )?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Option<ScanSession>> {
        let mut stmt = self.conn.prepare("SELECT data FROM sessions WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let data: String = row.get(0)?;
            let session: ScanSession = serde_json::from_str(&data)?;
            Ok(Some(session))
        } else {
            Ok(None)
        }
    }

    /// Lightweight summaries for listing — avoids deserializing every
    /// session's full (potentially large) results list just to show an
    /// overview.
    pub fn list_summaries(&self) -> Result<Vec<SessionSummary>> {
        let mut stmt = self.conn.prepare("SELECT id, updated_at, data FROM sessions ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let updated_at: String = row.get(1)?;
            let data: String = row.get(2)?;
            Ok((id, updated_at, data))
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            let (id, updated_at, data) = row?;
            if let Ok(session) = serde_json::from_str::<ScanSession>(&data) {
                summaries.push(SessionSummary {
                    id,
                    updated_at,
                    device_label: session.device_label,
                    state: session.state,
                    files_found: session.results.len(),
                });
            }
        }
        Ok(summaries)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub updated_at: String,
    pub device_label: String,
    pub state: crate::scan_session::ScanState,
    pub files_found: usize,
}

/// A minimal timestamp without pulling in the `chrono` crate for one
/// formatted string — `SystemTime` plus a manual Unix-epoch-seconds
/// rendering is all "when was this last updated" needs here.
fn chrono_now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan_session::{ScanMode, ScanProgress, ScanState};

    fn sample_session(id: &str) -> ScanSession {
        ScanSession {
            id: id.to_string(),
            image_path: "/tmp/whatever.img".to_string(),
            device_label: "/tmp/whatever.img".to_string(),
            mode: ScanMode::Quick,
            state: ScanState::Complete,
            progress: ScanProgress { scanned_bytes: 100, total_bytes: 100, current_phase: "done".to_string() },
            filesystem_detected: Some("FAT32".to_string()),
            error_message: None,
            results: vec![],
        }
    }

    #[test]
    fn saves_and_reloads_a_session() {
        let store = SessionStore::open_in_memory().unwrap();
        let session = sample_session("session-1");
        store.save(&session).unwrap();

        let loaded = store.load("session-1").unwrap().expect("session should exist");
        assert_eq!(loaded.id, "session-1");
        assert_eq!(loaded.device_label, "/tmp/whatever.img");
        assert_eq!(loaded.filesystem_detected, Some("FAT32".to_string()));
    }

    #[test]
    fn missing_session_returns_none_not_error() {
        let store = SessionStore::open_in_memory().unwrap();
        let loaded = store.load("does-not-exist").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn save_is_an_upsert() {
        let store = SessionStore::open_in_memory().unwrap();
        let mut session = sample_session("session-1");
        store.save(&session).unwrap();

        session.state = ScanState::Failed;
        session.error_message = Some("disk pulled mid-scan".to_string());
        store.save(&session).unwrap(); // same id — should update, not duplicate

        let loaded = store.load("session-1").unwrap().unwrap();
        assert_eq!(loaded.state, ScanState::Failed);

        let summaries = store.list_summaries().unwrap();
        assert_eq!(summaries.len(), 1, "upsert should not create a duplicate row");
    }
}
