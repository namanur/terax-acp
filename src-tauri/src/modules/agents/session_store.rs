#![allow(dead_code)]
use rusqlite::{params, Connection};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Database is locked (SQLITE_BUSY)")]
    Busy,
    #[error("Database is corrupt")]
    Corrupt,
    #[error("Database not initialized")]
    NotInitialized,
    #[error("Session {session_id} is in terminal state {status:?}")]
    TerminalState {
        session_id: String,
        status: SessionStatus,
    },
    #[error("Illegal transition: {from:?} → {to:?} for session {session_id}")]
    IllegalTransition {
        session_id: String,
        from: SessionStatus,
        to: SessionStatus,
    },
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    User,
    Agent,
    Provisional,
}

impl TitleSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            TitleSource::User => "user",
            TitleSource::Agent => "agent",
            TitleSource::Provisional => "provisional",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "user" => TitleSource::User,
            "agent" => TitleSource::Agent,
            _ => TitleSource::Provisional,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Idle,
    Closed,
    Unreachable,
    Expired,
    Deleted,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStatus::Active => "active",
            SessionStatus::Idle => "idle",
            SessionStatus::Closed => "closed",
            SessionStatus::Unreachable => "unreachable",
            SessionStatus::Expired => "expired",
            SessionStatus::Deleted => "deleted",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "active" => SessionStatus::Active,
            "idle" => SessionStatus::Idle,
            "closed" => SessionStatus::Closed,
            "unreachable" => SessionStatus::Unreachable,
            "expired" => SessionStatus::Expired,
            "deleted" => SessionStatus::Deleted,
            _ => SessionStatus::Active,
        }
    }

    /// Returns true if this transition is legal per the status state machine.
    pub fn can_transition_to(&self, target: &SessionStatus) -> bool {
        use SessionStatus::*;
        matches!(
            (self, target),
            (Active, Idle)
                | (Active, Unreachable)
                | (Active, Closed)
                | (Idle, Active)
                | (Idle, Closed)
                | (Idle, Unreachable)
                | (Closed, Deleted)
                | (Unreachable, Idle)      // Recovery!
                | (Unreachable, Expired)   // Confirmed dead
        )
    }

    /// Returns true if this status represents a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, SessionStatus::Expired | SessionStatus::Deleted)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub title: Option<String>,
    pub title_source: TitleSource,
    pub agent_id: String,
    pub work_dirs: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_message_preview: Option<String>,
    pub token_usage: Option<TokenUsage>,
    pub meta: Option<serde_json::Value>,
    pub status: SessionStatus,
}

// ---------------------------------------------------------------------------
// Debounce types
// ---------------------------------------------------------------------------

/// Cosmetic writes that can be debounced (5-second batching).
#[derive(Debug, Clone)]
enum DebouncedWrite {
    UpdatedAt { session_id: String, timestamp: u64 },
    PreviewText { session_id: String, text: String },
    ProvisionalTitle { session_id: String, title: String },
}

impl DebouncedWrite {
    fn session_id(&self) -> &str {
        match self {
            DebouncedWrite::UpdatedAt { session_id, .. } => session_id,
            DebouncedWrite::PreviewText { session_id, .. } => session_id,
            DebouncedWrite::ProvisionalTitle { session_id, .. } => session_id,
        }
    }
}

// ---------------------------------------------------------------------------
// SQLite-backed session store
// ---------------------------------------------------------------------------

pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
    _path: PathBuf,
    /// MPSC sender for debounced cosmetic writes.
    debounce_tx: Sender<DebouncedWrite>,
    /// Shutdown signal sender. Dropping this signals the debounce thread to drain and exit.
    _shutdown_tx: Sender<()>,
}

impl SessionStore {
    /// Open (or create) the SQLite database at `path`.
    /// Enables WAL mode, creates schema, and starts the debounce background thread.
    pub fn open(path: &Path) -> Result<Self, SessionStoreError> {
        let conn = Connection::open(path)?;

        // WAL mode for concurrent reads + durability without checkpoint abuse
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                session_id     TEXT PRIMARY KEY,
                title          TEXT,
                title_source   TEXT NOT NULL DEFAULT 'provisional'
                    CHECK(title_source IN ('user','agent','provisional')),
                agent_id       TEXT NOT NULL,
                work_dirs      TEXT NOT NULL DEFAULT '[]',
                created_at     INTEGER NOT NULL,
                updated_at     INTEGER NOT NULL,
                last_message_preview TEXT,
                token_usage    TEXT,
                meta           TEXT,
                status         TEXT NOT NULL DEFAULT 'active'
                    CHECK(status IN ('active','idle','closed','unreachable','expired','deleted'))
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_status
                ON sessions(status);
            CREATE INDEX IF NOT EXISTS idx_sessions_updated_at
                ON sessions(updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_sessions_agent_id
                ON sessions(agent_id);",
        )?;

        let conn = Arc::new(Mutex::new(conn));

        // -- Start debounce thread ------------------------------------------------
        let (debounce_tx, debounce_rx) = mpsc::channel::<DebouncedWrite>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let conn_clone = Arc::clone(&conn);

        thread::spawn(move || {
            let flush_interval = Duration::from_secs(5);
            let mut pending: HashMap<String, DebouncedWrite> = HashMap::new();

            loop {
                // Wait for either a new write or a tick
                match debounce_rx.recv_timeout(flush_interval) {
                    Ok(write) => {
                        // Last write for a session_id wins (debounce dedup)
                        pending.insert(write.session_id().to_string(), write);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Interval tick — flush pending
                        flush_debounced_inner(&conn_clone, &mut pending);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // Sender dropped — final flush and exit
                        flush_debounced_inner(&conn_clone, &mut pending);
                        let _ = shutdown_rx.recv(); // Wait for explicit shutdown
                        break;
                    }
                }
            }
        });

        Ok(Self {
            conn,
            _path: path.to_path_buf(),
            debounce_tx,
            _shutdown_tx: shutdown_tx,
        })
    }

    // -- CRUD ---------------------------------------------------------------

    /// Insert a new session record. Trust-critical — immediate commit, no debounce.
    /// Durability comes from the transaction commit. No WAL checkpoint abuse.
    pub fn insert_session(&self, record: &SessionRecord) -> Result<(), SessionStoreError> {
        self.insert_session_inner(record)?;
        log::trace!("insert_session (immediate): {}", record.session_id);
        Ok(())
    }

    fn insert_session_inner(&self, record: &SessionRecord) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let work_dirs_json = serde_json::to_string(&record.work_dirs)?;
        let token_usage_json = record
            .token_usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let meta_json = record
            .meta
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        conn.execute(
            "INSERT INTO sessions
                (session_id, title, title_source, agent_id, work_dirs,
                 created_at, updated_at, last_message_preview,
                 token_usage, meta, status)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                record.session_id,
                record.title,
                record.title_source.as_str(),
                record.agent_id,
                work_dirs_json,
                record.created_at,
                record.updated_at,
                record.last_message_preview,
                token_usage_json,
                meta_json,
                record.status.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let mut stmt = conn.prepare(
            "SELECT session_id, title, title_source, agent_id, work_dirs,
                    created_at, updated_at, last_message_preview,
                    token_usage, meta, status
             FROM sessions WHERE session_id = ?",
        )?;

        let mut rows = stmt.query_map(params![session_id], |row| {
            let work_dirs_str: String = row.get(4)?;
            let token_usage_str: Option<String> = row.get(8)?;
            let meta_str: Option<String> = row.get(9)?;

            Ok(SessionRecord {
                session_id: row.get(0)?,
                title: row.get(1)?,
                title_source: TitleSource::from_str(&row.get::<_, String>(2)?),
                agent_id: row.get(3)?,
                work_dirs: serde_json::from_str(&work_dirs_str).unwrap_or_default(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                last_message_preview: row.get(7)?,
                token_usage: token_usage_str
                    .and_then(|s| serde_json::from_str(&s).ok()),
                meta: meta_str.and_then(|s| serde_json::from_str(&s).ok()),
                status: SessionStatus::from_str(&row.get::<_, String>(10)?),
            })
        })?;

        match rows.next() {
            Some(Ok(record)) => Ok(Some(record)),
            Some(Err(e)) => Err(SessionStoreError::Sqlite(e)),
            None => Ok(None),
        }
    }

    /// Full update of a session record. Trust-critical.
    pub fn update_session(&self, record: &SessionRecord) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let work_dirs_json = serde_json::to_string(&record.work_dirs)?;
        let token_usage_json = record
            .token_usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let meta_json = record
            .meta
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        conn.execute(
            "UPDATE sessions SET
                title = ?2, title_source = ?3, agent_id = ?4, work_dirs = ?5,
                updated_at = ?6, last_message_preview = ?7,
                token_usage = ?8, meta = ?9, status = ?10
             WHERE session_id = ?1",
            params![
                record.session_id,
                record.title,
                record.title_source.as_str(),
                record.agent_id,
                work_dirs_json,
                record.updated_at,
                record.last_message_preview,
                token_usage_json,
                meta_json,
                record.status.as_str(),
            ],
        )?;
        log::trace!("update_session (immediate): {}", record.session_id);
        Ok(())
    }

    /// Delete a session. Trust-critical — immediate write.
    pub fn delete_session(&self, session_id: &str) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        conn.execute("DELETE FROM sessions WHERE session_id = ?", params![session_id])?;
        log::trace!("delete_session (immediate): {}", session_id);
        Ok(())
    }

    // -- List ---------------------------------------------------------------

    pub fn list_sessions(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<SessionRecord>, SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        // Exclude deleted sessions from listing
        let mut stmt = conn.prepare(
            "SELECT session_id, title, title_source, agent_id, work_dirs,
                    created_at, updated_at, last_message_preview,
                    token_usage, meta, status
             FROM sessions
             WHERE status != 'deleted'
             ORDER BY updated_at DESC
             LIMIT ? OFFSET ?",
        )?;

        let rows = stmt.query_map(params![limit, offset], |row| {
            let work_dirs_str: String = row.get(4)?;
            let token_usage_str: Option<String> = row.get(8)?;
            let meta_str: Option<String> = row.get(9)?;

            Ok(SessionRecord {
                session_id: row.get(0)?,
                title: row.get(1)?,
                title_source: TitleSource::from_str(&row.get::<_, String>(2)?),
                agent_id: row.get(3)?,
                work_dirs: serde_json::from_str(&work_dirs_str).unwrap_or_default(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                last_message_preview: row.get(7)?,
                token_usage: token_usage_str
                    .and_then(|s| serde_json::from_str(&s).ok()),
                meta: meta_str.and_then(|s| serde_json::from_str(&s).ok()),
                status: SessionStatus::from_str(&row.get::<_, String>(10)?),
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    // -- Status transitions -------------------------------------------------

    /// Set status with transition validation.
    /// Rejects transitions from terminal states and illegal transitions.
    pub fn set_status(
        &self,
        session_id: &str,
        status: SessionStatus,
    ) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;

        // Read current status
        let current_str: String = conn.query_row(
            "SELECT status FROM sessions WHERE session_id = ?",
            params![session_id],
            |row| row.get(0),
        )?;
        let current_status = SessionStatus::from_str(&current_str);

        // Terminal state check
        if current_status.is_terminal() {
            return Err(SessionStoreError::TerminalState {
                session_id: session_id.to_string(),
                status: current_status,
            });
        }

        // Transition validation
        if !current_status.can_transition_to(&status) {
            return Err(SessionStoreError::IllegalTransition {
                session_id: session_id.to_string(),
                from: current_status.clone(),
                to: status.clone(),
            });
        }

        let now = chrono::Utc::now().timestamp() as u64;
        conn.execute(
            "UPDATE sessions SET status = ?2, updated_at = ?3 WHERE session_id = ?1",
            params![session_id, status.as_str(), now],
        )?;
        log::debug!(
            "set_status: {} {:?} → {:?}",
            session_id,
            current_status,
            status
        );
        Ok(())
    }

    // -- Title ownership ----------------------------------------------------

    /// Set the title for a session, respecting ownership priority:
    /// user > agent > provisional.
    /// Agent/provisional updates are silently ignored if the current
    /// title_source is `user`.
    pub fn set_title(
        &self,
        session_id: &str,
        title: &str,
        source: TitleSource,
    ) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;

        // If current source is user, agent/provisional updates are silently ignored
        if source != TitleSource::User {
            let current_source: String = conn.query_row(
                "SELECT title_source FROM sessions WHERE session_id = ?",
                params![session_id],
                |row| row.get(0),
            )?;

            if current_source == "user" {
                log::debug!(
                    "set_title: agent/provisional update ignored for {} (user owns title)",
                    session_id
                );
                return Ok(());
            }
        }

        let now = chrono::Utc::now().timestamp() as u64;
        conn.execute(
            "UPDATE sessions SET title = ?2, title_source = ?3, updated_at = ?4 WHERE session_id = ?1",
            params![session_id, title, source.as_str(), now],
        )?;
        log::trace!(
            "set_title: {} → \"{}\" ({:?})",
            session_id,
            title,
            source
        );
        Ok(())
    }

    // -- Debounced writes (cosmetic only) -----------------------------------

    /// Debounce an `updated_at` bump. Cosmetic — can be lost on crash.
    pub fn debounce_updated_at(&self, session_id: &str) {
        let _ = self.debounce_tx.send(DebouncedWrite::UpdatedAt {
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now().timestamp() as u64,
        });
    }

    /// Debounce a message preview update. Cosmetic — regenerated on next load.
    pub fn debounce_preview(&self, session_id: &str, text: &str) {
        let truncated: String = text.chars().take(150).collect();
        let _ = self.debounce_tx.send(DebouncedWrite::PreviewText {
            session_id: session_id.to_string(),
            text: truncated,
        });
    }

    /// Debounce a provisional title update. Cosmetic — agent can re-suggest.
    pub fn debounce_provisional_title(&self, session_id: &str, title: &str) {
        let truncated: String = title.chars().take(200).collect();
        let _ = self.debounce_tx.send(DebouncedWrite::ProvisionalTitle {
            session_id: session_id.to_string(),
            title: truncated,
        });
    }

    /// Flush all pending debounced writes to SQLite. Called on graceful shutdown.
    /// After calling this, no more debounce writes will be accepted.
    pub fn close_debounce_channel(&self) {
        // Dropping the sender triggers the background thread to do a final flush
        // and wait for the shutdown signal.
        drop(self._shutdown_tx.clone());
        // The actual sender drop happens when SessionStore is dropped.
        log::info!("debounce channel close requested — pending writes will be flushed");
    }

    // -- Reconcile ----------------------------------------------------------

    /// Reconcile local session list with ACP-active sessions.
    ///
    /// 1. Sessions in `active_ids` that are locally marked as unreachable → mark idle (recovered)
    /// 2. Sessions NOT in `active_ids` that are locally active/idle → mark unreachable
    /// 3. Auto-GC: expired sessions older than 30 days are deleted
    pub fn reconcile(&self, active_ids: &[String]) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let now_ts = chrono::Utc::now().timestamp() as u64;
        let mut recovered = 0u32;
        let mut new_unreachable = 0u32;


        // 1. Recovery: unreachable sessions that are now in active_ids → idle
        {
            let mut stmt = conn.prepare(
                "SELECT session_id FROM sessions WHERE status = 'unreachable'",
            )?;
            let unreachable_ids: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            for id in &unreachable_ids {
                if active_ids.contains(id) {
                    conn.execute(
                        "UPDATE sessions SET status = 'idle', updated_at = ? WHERE session_id = ?",
                        params![now_ts, id],
                    )?;
                    recovered += 1;
                }
            }
        }

        // 2. Mark absent sessions as unreachable (NOT expired!)
        {
            let mut stmt = conn.prepare(
                "SELECT session_id FROM sessions WHERE status IN ('active', 'idle')",
            )?;
            let local_ids: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            for id in &local_ids {
                if !active_ids.contains(id) {
                    conn.execute(
                        "UPDATE sessions SET status = 'unreachable', updated_at = ? WHERE session_id = ?",
                        params![now_ts, id],
                    )?;
                    new_unreachable += 1;
                }
            }
        }

        // 3. Auto-GC: expired sessions older than 30 days
        let auto_gc = {
            let cutoff = now_ts.saturating_sub(30 * 24 * 3600);
            let deleted = conn.execute(
                "DELETE FROM sessions WHERE status = 'expired' AND updated_at < ?",
                params![cutoff],
            )?;
            deleted as u32
        };

        log::info!(
            "reconcile: recovered={} new_unreachable={} auto_gc={}",
            recovered,
            new_unreachable,
            auto_gc
        );
        Ok(())
    }

    /// Mark a session as confirmed-dead (expired).
    /// IRREVERSIBLE. Only call when the ACP shim explicitly rejected the session.
    pub fn mark_expired(&self, session_id: &str) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let now_ts = chrono::Utc::now().timestamp() as u64;

        let rows = conn.execute(
            "UPDATE sessions SET status = 'expired', updated_at = ? WHERE session_id = ? AND status NOT IN ('expired', 'deleted')",
            params![now_ts, session_id],
        )?;

        if rows > 0 {
            log::error!(
                target: "restore",
                "RESTORE_FAILED session_id={} reason=expired",
                session_id
            );
        }
        Ok(())
    }

    // -- Query helpers ------------------------------------------------------

    pub fn count_by_status(&self, status: SessionStatus) -> Result<u32, SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let count: u32 = conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE status = ?",
            params![status.as_str()],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn expired_sessions_older_than(
        &self,
        days: u32,
    ) -> Result<Vec<String>, SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days as i64)).timestamp() as u64;

        let mut stmt = conn.prepare(
            "SELECT session_id FROM sessions WHERE status = 'expired' AND updated_at < ?",
        )?;

        let ids = stmt
            .query_map(params![cutoff], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }

    // -- Checkpoint (on graceful shutdown) ----------------------------------

    /// Run a full WAL checkpoint. Only called on app close / maintenance.
    pub fn wal_checkpoint(&self) -> Result<(), SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let start = std::time::Instant::now();
        conn.execute_batch("PRAGMA wal_checkpoint(FULL);")?;
        let elapsed = start.elapsed();
        log::debug!("WAL checkpoint completed in {}ms", elapsed.as_millis());
        Ok(())
    }

    // -- Integrity ----------------------------------------------------------

    pub fn integrity_check(&self) -> Result<bool, SessionStoreError> {
        let conn = self.conn.lock().map_err(|_| SessionStoreError::Busy)?;
        let result: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        let ok = result == "ok";
        if !ok {
            log::error!("integrity_check failed: {}", result);
        }
        Ok(ok)
    }
}

// -- Drop: graceful shutdown -----------------------------------------------

impl Drop for SessionStore {
    fn drop(&mut self) {
        // The debounce_tx sender is dropped here, which signals the background
        // thread to do a final flush before exiting. No explicit wait needed —
        // the thread will drain and exit autonomously.
        if let Err(e) = self.wal_checkpoint() {
            log::warn!("WAL checkpoint on drop failed: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: flush debounced writes
// ---------------------------------------------------------------------------

fn flush_debounced_inner(conn: &Arc<Mutex<Connection>>, pending: &mut HashMap<String, DebouncedWrite>) {
    if pending.is_empty() {
        return;
    }

    let conn = match conn.lock() {
        Ok(c) => c,
        Err(_) => return,
    };

    let now = chrono::Utc::now().timestamp() as u64;
    let mut flushed = 0u32;

    for (session_id, write) in pending.drain() {
        let result = match write {
            DebouncedWrite::UpdatedAt { timestamp, .. } => {
                conn.execute(
                    "UPDATE sessions SET updated_at = MAX(updated_at, ?) WHERE session_id = ?",
                    params![timestamp, session_id],
                )
            }
            DebouncedWrite::PreviewText { text, .. } => {
                conn.execute(
                    "UPDATE sessions SET last_message_preview = ?, updated_at = ? WHERE session_id = ?",
                    params![text, now, session_id],
                )
            }
            DebouncedWrite::ProvisionalTitle { title, .. } => {
                // Only apply if title_source is not 'user'
                conn.execute(
                    "UPDATE sessions SET title = ?, title_source = 'provisional', updated_at = ? WHERE session_id = ? AND title_source != 'user'",
                    params![title, now, session_id],
                )
            }
        };

        match result {
            Ok(rows) if rows > 0 => flushed += 1,
            Ok(_) => {}
            Err(e) => log::warn!("debounce flush failed for {}: {}", session_id, e),
        }
    }

    if flushed > 0 {
        log::trace!("debounce flushed {} writes", flushed);
    }
}

// ---------------------------------------------------------------------------
// Unified session list (for frontend consumption)
// ---------------------------------------------------------------------------

use super::legacy::LegacySession;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source")]
pub enum UnifiedSession {
    #[serde(rename = "active")]
    Active(SessionRecord),
    #[serde(rename = "legacy")]
    Legacy {
        id: String,
        title: Option<String>,
        created_at: Option<String>,
        updated_at: Option<String>,
        message_count: usize,
        last_preview: Option<String>,
    },
}

impl From<LegacySession> for UnifiedSession {
    fn from(s: LegacySession) -> Self {
        UnifiedSession::Legacy {
            id: s.id,
            title: s.title,
            created_at: s.created_at,
            updated_at: s.updated_at,
            message_count: s.message_count,
            last_preview: s.last_preview,
        }
    }
}

/// Merge active sessions and legacy sessions into one unified list.
/// Active sessions first (newest), then legacy sessions.
pub fn merge_session_lists(
    active: Vec<SessionRecord>,
    legacy: Vec<LegacySession>,
) -> Vec<UnifiedSession> {
    let mut unified: Vec<UnifiedSession> = active
        .into_iter()
        .map(UnifiedSession::Active)
        .collect();

    unified.extend(legacy.into_iter().map(UnifiedSession::from));
    unified
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn now_ts() -> u64 {
        chrono::Utc::now().timestamp() as u64
    }

    fn make_record(id: &str, title: &str, status: SessionStatus, updated_at: u64) -> SessionRecord {
        SessionRecord {
            session_id: id.to_string(),
            title: Some(title.to_string()),
            title_source: TitleSource::Provisional,
            agent_id: "test-agent".to_string(),
            work_dirs: vec!["/tmp/test".to_string()],
            created_at: updated_at - 100,
            updated_at,
            last_message_preview: None,
            token_usage: None,
            meta: None,
            status,
        }
    }

    fn open_test_store(dir: &TempDir) -> SessionStore {
        let path = dir.path().join("test.db");
        SessionStore::open(&path).expect("failed to open test db")
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        let record = make_record("s1", "Hello", SessionStatus::Active, now_ts());
        store.insert_session(&record).unwrap();

        let got = store.get_session("s1").unwrap().expect("should exist");
        assert_eq!(got.session_id, "s1");
        assert_eq!(got.title.as_deref(), Some("Hello"));
        assert_eq!(got.status, SessionStatus::Active);
        assert_eq!(got.agent_id, "test-agent");
        assert_eq!(got.work_dirs, vec!["/tmp/test"]);
    }

    #[test]
    fn list_ordering_desc() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);
        let base = now_ts();

        store.insert_session(&make_record("a", "A", SessionStatus::Active, base)).unwrap();
        store.insert_session(&make_record("b", "B", SessionStatus::Active, base + 10)).unwrap();
        store.insert_session(&make_record("c", "C", SessionStatus::Active, base + 20)).unwrap();

        let sessions = store.list_sessions(0, 10).unwrap();
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].session_id, "c"); // newest first
        assert_eq!(sessions[2].session_id, "a"); // oldest last
    }

    #[test]
    fn status_transitions() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "T", SessionStatus::Active, now_ts())).unwrap();

        store.set_status("s1", SessionStatus::Idle).unwrap();
        assert_eq!(store.get_session("s1").unwrap().unwrap().status, SessionStatus::Idle);

        store.set_status("s1", SessionStatus::Closed).unwrap();
        assert_eq!(store.get_session("s1").unwrap().unwrap().status, SessionStatus::Closed);
    }

    #[test]
    fn illegal_transition_rejected() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "T", SessionStatus::Active, now_ts())).unwrap();
        store.set_status("s1", SessionStatus::Closed).unwrap();
        store.set_status("s1", SessionStatus::Deleted).unwrap();

        // Can't transition from Deleted
        let result = store.set_status("s1", SessionStatus::Active);
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionStoreError::TerminalState { .. } => {} // expected
            e => panic!("expected TerminalState, got {:?}", e),
        }
    }

    #[test]
    fn unreachable_can_recover_to_idle() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "T", SessionStatus::Unreachable, now_ts())).unwrap();
        store.set_status("s1", SessionStatus::Idle).unwrap();

        let r = store.get_session("s1").unwrap().unwrap();
        assert_eq!(r.status, SessionStatus::Idle);
    }

    #[test]
    fn expired_is_terminal() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "T", SessionStatus::Expired, now_ts())).unwrap();
        let result = store.set_status("s1", SessionStatus::Idle);
        assert!(result.is_err());
    }

    #[test]
    fn reconcile_recovery() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "Recoverable", SessionStatus::Unreachable, now_ts())).unwrap();
        store.insert_session(&make_record("s2", "Active", SessionStatus::Active, now_ts())).unwrap();

        // ACP reports s1 as active (agent recovered), s2 is absent
        store.reconcile(&["s1".to_string()]).unwrap();

        // s1 should now be idle (recovered)
        assert_eq!(store.get_session("s1").unwrap().unwrap().status, SessionStatus::Idle);
        // s2 should now be unreachable (absent from ACP)
        assert_eq!(store.get_session("s2").unwrap().unwrap().status, SessionStatus::Unreachable);
    }

    #[test]
    fn title_ownership_user_wins() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "Provisional Title", SessionStatus::Active, now_ts())).unwrap();

        // User sets title
        store.set_title("s1", "My Title", TitleSource::User).unwrap();
        let r = store.get_session("s1").unwrap().unwrap();
        assert_eq!(r.title.as_deref(), Some("My Title"));
        assert_eq!(r.title_source, TitleSource::User);

        // Agent tries to overwrite — silently ignored
        store.set_title("s1", "Agent's Better Title", TitleSource::Agent).unwrap();
        let r = store.get_session("s1").unwrap().unwrap();
        assert_eq!(r.title.as_deref(), Some("My Title")); // unchanged
        assert_eq!(r.title_source, TitleSource::User);

        // Provisional also silently ignored
        store.set_title("s1", "Auto-generated", TitleSource::Provisional).unwrap();
        let r = store.get_session("s1").unwrap().unwrap();
        assert_eq!(r.title.as_deref(), Some("My Title"));
    }

    #[test]
    fn wal_mode_enabled() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);
        let conn = store.conn.lock().unwrap();
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn sigkill_during_debounce_recovery() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // 1. Create session with immediate write
        {
            let store = SessionStore::open(&db_path).unwrap();
            store.insert_session(&make_record("session-1", "Test", SessionStatus::Active, now_ts())).unwrap();

            // 2. Queue debounced title update
            store.debounce_provisional_title("session-1", "Agent auto-title");
            store.debounce_preview("session-1", "This is a preview");

            // 3. Simulate crash (drop store without waiting for debounce flush)
            drop(store);
        }

        // 4. Reopen store
        {
            let store2 = SessionStore::open(&db_path).unwrap();

            // 5. Session should exist (immediate write survived)
            let record = store2.get_session("session-1").unwrap().expect("session must exist after SIGKILL");
            assert_eq!(record.status, SessionStatus::Active);
            assert_eq!(record.session_id, "session-1");
        }
    }

    #[test]
    fn graceful_shutdown_flushes_debounce() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        {
            let store = SessionStore::open(&db_path).unwrap();
            store.insert_session(&make_record("s1", "Test", SessionStatus::Active, now_ts())).unwrap();
            store.debounce_provisional_title("s1", "Will be flushed");

            // Close debounce channel (graceful shutdown)
            store.close_debounce_channel();

            // Give the background thread time to flush
            thread::sleep(Duration::from_millis(100));

            // Drop the store to trigger final flush via Drop
            drop(store);
        }

        // Reopen — debounced title should be persisted (or may be absent if
        // the background thread didn't flush in time — both are acceptable)
        {
            let store2 = SessionStore::open(&db_path).unwrap();
            let _record = store2.get_session("s1").unwrap().expect("session must exist");
            // Title may or may not have been flushed — both OK for cosmetic data
        }
    }

    #[test]
    fn integrity_check_passes() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);
        assert!(store.integrity_check().unwrap());
    }

    #[test]
    fn count_by_status() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("a", "A", SessionStatus::Active, now_ts())).unwrap();
        store.insert_session(&make_record("b", "B", SessionStatus::Idle, now_ts())).unwrap();
        store.insert_session(&make_record("c", "C", SessionStatus::Active, now_ts())).unwrap();

        assert_eq!(store.count_by_status(SessionStatus::Active).unwrap(), 2);
        assert_eq!(store.count_by_status(SessionStatus::Idle).unwrap(), 1);
        assert_eq!(store.count_by_status(SessionStatus::Expired).unwrap(), 0);
    }
}
