#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

use super::session_store::{self, SessionStatus, TitleSource};

// ---------------------------------------------------------------------------
// Restoration Contract
// ---------------------------------------------------------------------------

/// The restoration contract defines what state is recovered when a session loads.
/// This is the SINGLE RESTORATION AUTHORITY. No code outside this module may
/// restore session state. Violations are a hard architectural regression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorationContract {
    pub session_id: String,
    pub must: MustRestore,
    pub may: MayRestore,
    pub dropped: Vec<DroppedItem>,
}

/// Items that MUST be restored. Failure = session load fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MustRestore {
    pub message_history: bool,
    pub title: Option<String>,
    pub title_source: Option<String>,
    pub token_usage: Option<session_store::TokenUsage>,
    pub agent_id: String,
}

/// Items that MAY be restored. Best-effort. Failure = degrade gracefully.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MayRestore {
    pub pending_approvals: bool,
    pub interrupted_tools: bool,
    pub diff_queue: bool,
    pub active_model: Option<String>,
    pub session_mode: Option<String>,
}

/// Items intentionally dropped on session load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedItem {
    pub name: String,
    pub rationale: String,
}

impl Default for RestorationContract {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            must: MustRestore {
                message_history: true,
                title: None,
                title_source: None,
                token_usage: None,
                agent_id: String::new(),
            },
            may: MayRestore {
                pending_approvals: false,
                interrupted_tools: false,
                diff_queue: false,
                active_model: None,
                session_mode: None,
            },
            dropped: vec![
                DroppedItem {
                    name: "In-flight shell processes".into(),
                    rationale: "Shell state is ephemeral. Cannot be resumed. User must re-run."
                        .into(),
                },
                DroppedItem {
                    name: "Scroll position".into(),
                    rationale: "Too brittle. User scrolls to position naturally after reload."
                        .into(),
                },
                DroppedItem {
                    name: "Unsent draft text".into(),
                    rationale: "Saved to local store last_draft. MAY restore but not guaranteed."
                        .into(),
                },
                DroppedItem {
                    name: "Temporary render state".into(),
                    rationale: "Caches, animations, collapsed/expanded UI state. All thrown away."
                        .into(),
                },
            ],
        }
    }
}

impl RestorationContract {
    /// What to do when a MAY item fails during restore.
    pub fn degradation_policy_for(&self, may_item: &str) -> DegradationPolicy {
        match may_item {
            "pending_approvals" => DegradationPolicy::Skip,
            "interrupted_tools" => DegradationPolicy::RetryOnce,
            "diff_queue" => DegradationPolicy::Skip,
            "active_model" => DegradationPolicy::Unavailable,
            "session_mode" => DegradationPolicy::Unavailable,
            _ => DegradationPolicy::Skip,
        }
    }
}

/// What to do when a MAY item fails during restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradationPolicy {
    /// Skip the item entirely, session still loads.
    Skip,
    /// Retry once after 1 second.
    RetryOnce,
    /// Mark as unavailable, expose in UI.
    Unavailable,
}

// ---------------------------------------------------------------------------
// Outcome & Errors
// ---------------------------------------------------------------------------

/// Outcome of a restoration attempt.
#[derive(Debug, Clone)]
pub enum RestorationOutcome {
    Full(RestorationContract),
    Degraded {
        contract: RestorationContract,
        failed_may: Vec<String>,
    },
    Failed {
        contract: RestorationContract,
        failed_must: Vec<String>,
        error: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RestorationError {
    #[error("MUST restore item '{0}' failed: {1}")]
    MustItemFailed(String, String),
    #[error("Session expired")]
    Expired,
    #[error("Agent offline: {0}")]
    AgentOffline(String),
    #[error("Store error: {0}")]
    StoreError(#[from] session_store::SessionStoreError),
}

// ---------------------------------------------------------------------------
// Session Action (traceability)
// ---------------------------------------------------------------------------

/// Every session action gets a trace_id for end-to-end tracing.
#[derive(Debug, Clone)]
pub struct SessionAction {
    pub trace_id: Uuid,
    pub session_id: String,
    pub action: ActionKind,
    pub started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Load,
    Resume,
    Close,
    Delete,
    Rename,
    Save,
    Approve,
}

impl SessionAction {
    pub fn new(session_id: String, action: ActionKind) -> Self {
        Self {
            trace_id: Uuid::new_v4(),
            session_id,
            action,
            started_at: Instant::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// load_session — core restoration function
// ---------------------------------------------------------------------------

/// Load a session by ID.
///
/// 1. Read SessionRecord from SQLite
/// 2. Check status gate: expired → error, unreachable → AgentOffline, deleted → error
/// 3. Build restoration contract from local data (MUST items)
/// 4. Fill MAY items from session record metadata
/// 5. Update status to active
/// 6. Log RESTORE_OK or RESTORE_FAILED
///
/// NOTE: ACP shim calls (sessions/load) will be wired in P4 (AgentRouter).
/// For now, restoration uses local store data only.
pub fn load_session(
    store: &session_store::SessionStore,
    session_id: &str,
) -> Result<RestorationOutcome, RestorationError> {
    let trace_id = Uuid::new_v4();
    let started = Instant::now();

    log::info!(
        target: "restore",
        "LOAD_START trace_id={trace_id} session_id={session_id}"
    );

    // 1. Read from local store
    let record = store
        .get_session(session_id)?
        .ok_or_else(|| {
            RestorationError::MustItemFailed(
                "session_record".into(),
                "Session not found in local store".into(),
            )
        })?;

    // 2. Status gate
    match record.status {
        SessionStatus::Expired => {
            log::error!(
                target: "restore",
                "RESTORE_FAILED trace_id={trace_id} session_id={session_id} error=EXPIRED"
            );
            return Err(RestorationError::Expired);
        }
        SessionStatus::Unreachable => {
            log::warn!(
                target: "restore",
                "LOAD_UNREACHABLE trace_id={trace_id} session_id={session_id} — agent may be offline"
            );
            return Err(RestorationError::AgentOffline(
                "Agent is currently unreachable. Try again later.".into(),
            ));
        }
        SessionStatus::Deleted => {
            return Err(RestorationError::MustItemFailed(
                "session_record".into(),
                "Session has been deleted".into(),
            ));
        }
        _ => {} // active, idle, closed — proceed
    }

    // 3. Build restoration contract from local data
    let mut may = MayRestore {
        pending_approvals: false,
        interrupted_tools: false,
        diff_queue: false,
        active_model: None,
        session_mode: None,
    };

    // Fill MAY from session metadata if available
    if let Some(ref meta) = record.meta {
        may.pending_approvals = meta
            .get("pending_approvals")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        may.interrupted_tools = meta
            .get("interrupted_tools")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        may.active_model = meta
            .get("active_model")
            .and_then(|v| v.as_str())
            .map(String::from);
        may.session_mode = meta
            .get("session_mode")
            .and_then(|v| v.as_str())
            .map(String::from);
    }

    let contract = RestorationContract {
        session_id: session_id.to_string(),
        must: MustRestore {
            message_history: true,
            title: record.title.clone(),
            title_source: Some(
                match record.title_source {
                    TitleSource::User => "user",
                    TitleSource::Agent => "agent",
                    TitleSource::Provisional => "provisional",
                }
                .to_string(),
            ),
            token_usage: record.token_usage.clone(),
            agent_id: record.agent_id.clone(),
        },
        may,
        dropped: RestorationContract::default().dropped,
    };

    // 4. TODO P4: Call ACP sessions/load(session_id) via AgentRouter
    // For now, local data is the restoration authority.

    // 5. Update status to active (if not already terminal)
    if !record.status.is_terminal() {
        let _ = store.set_status(session_id, SessionStatus::Active);
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    log::info!(
        target: "restore",
        "RESTORE_OK trace_id={trace_id} session_id={session_id} duration_ms={elapsed_ms}"
    );

    Ok(RestorationOutcome::Full(contract))
}

// ---------------------------------------------------------------------------
// validate_session — pre-load validation
// ---------------------------------------------------------------------------

/// Validate a specific session against the local store before loading.
/// Returns Ok(()) if the session is valid and loadable.
pub fn validate_session(
    store: &session_store::SessionStore,
    session_id: &str,
) -> Result<(), RestorationError> {
    let record = store.get_session(session_id)?.ok_or(
        RestorationError::MustItemFailed(
            "session_record".into(),
            "Session not found".into(),
        ),
    )?;

    match record.status {
        SessionStatus::Expired => {
            log::error!(target: "restore", "VALIDATE_EXPIRED session_id={session_id}");
            Err(RestorationError::Expired)
        }
        SessionStatus::Deleted => Err(RestorationError::MustItemFailed(
            "session_record".into(),
            "Session deleted".into(),
        )),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Reconcile — sync local store with ACP active sessions
// ---------------------------------------------------------------------------

/// Report produced by a reconcile operation.
#[derive(Debug, Clone)]
pub struct ReconcileReport {
    pub recovered: Vec<String>,        // unreachable → idle
    pub newly_unreachable: Vec<String>, // active/idle → unreachable
    pub expired: Vec<String>,           // confirmed dead
    pub auto_gc: u32,                  // auto-GC'd expired >30d
}

/// Reconcile the local session store with the ACP shim's active sessions.
///
/// Called when:
/// - User clicks "refresh" in the UI
/// - User opens the session list
/// - ACP shim restarts (reconnection)
pub fn reconcile(
    store: &session_store::SessionStore,
    acp_active_ids: &[String],
) -> Result<ReconcileReport, RestorationError> {
    let trace_id = Uuid::new_v4();

    log::info!(
        target: "restore",
        "RECONCILE_START trace_id={trace_id} acp_active_count={}",
        acp_active_ids.len()
    );

    let mut report = ReconcileReport {
        recovered: Vec::new(),
        newly_unreachable: Vec::new(),
        expired: Vec::new(),
        auto_gc: 0,
    };

    // 1. Get all local sessions (excluding deleted)
    let local_sessions = store.list_sessions(0, 10000)?;

    // 2. Recovery: unreachable → idle if session is back in acp_active_ids
    for session in &local_sessions {
        if session.status == SessionStatus::Unreachable
            && acp_active_ids.contains(&session.session_id)
        {
            store.set_status(&session.session_id, SessionStatus::Idle)?;
            report.recovered.push(session.session_id.clone());
            log::info!(
                target: "restore",
                "RECOVERED trace_id={trace_id} session_id={} unreachable→idle",
                session.session_id
            );
        }
    }

    // 3. Absence: active/idle → unreachable if NOT in acp_active_ids
    for session in &local_sessions {
        if matches!(
            session.status,
            SessionStatus::Active | SessionStatus::Idle
        ) && !acp_active_ids.contains(&session.session_id)
        {
            store.set_status(&session.session_id, SessionStatus::Unreachable)?;
            report
                .newly_unreachable
                .push(session.session_id.clone());
            log::warn!(
                target: "restore",
                "UNREACHABLE trace_id={trace_id} session_id={} active/idle→unreachable",
                session.session_id
            );
        }
    }

    // 4. Auto-GC: expired sessions older than 30 days
    let expired_ids = store.expired_sessions_older_than(30)?;
    for id in &expired_ids {
        store.delete_session(id)?;
    }
    report.auto_gc = expired_ids.len() as u32;

    if report.auto_gc > 0 {
        log::info!(
            target: "restore",
            "AUTO_GC trace_id={trace_id} expired_removed={}",
            report.auto_gc
        );
    }

    // 5. Mark confirmed-dead sessions
    // We don't auto-expire — only ACP shim rejection causes expired.
    // This is handled by mark_expired() which must be called explicitly.

    log::info!(
        target: "restore",
        "RECONCILE_OK trace_id={trace_id} recovered={} unreachable={} auto_gc={}",
        report.recovered.len(),
        report.newly_unreachable.len(),
        report.auto_gc
    );

    Ok(report)
}

// ---------------------------------------------------------------------------
// ACP response helpers
// ---------------------------------------------------------------------------

/// Check if the ACP shim reports pending approvals for a session.
pub fn has_pending_approvals(acp_session_info: &serde_json::Value) -> bool {
    acp_session_info
        .get("pending_approvals")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Check if the ACP shim reports in-progress tools for a session.
pub fn has_in_progress_tools(acp_session_info: &serde_json::Value) -> bool {
    acp_session_info
        .get("in_progress_tools")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::session_store::{SessionRecord, SessionStatus, TitleSource};
    use tempfile::TempDir;

    fn open_test_store(dir: &TempDir) -> session_store::SessionStore {
        let path = dir.path().join("restore-test.db");
        session_store::SessionStore::open(&path).expect("failed to open test db")
    }

    fn now_ts() -> u64 {
        chrono::Utc::now().timestamp() as u64
    }

    fn make_record(id: &str, title: &str, status: SessionStatus) -> SessionRecord {
        let ts = now_ts();
        SessionRecord {
            session_id: id.to_string(),
            title: Some(title.to_string()),
            title_source: TitleSource::User,
            agent_id: "claude-code".to_string(),
            work_dirs: vec!["/tmp/test".to_string()],
            created_at: ts - 100,
            updated_at: ts,
            last_message_preview: None,
            token_usage: None,
            meta: None,
            status,
        }
    }

    #[test]
    fn contract_covers_all_dropped_items() {
        let contract = RestorationContract::default();
        assert!(contract.dropped.iter().any(|d| d.name.contains("shell")));
        assert!(contract.dropped.iter().any(|d| d.name.contains("Scroll")));
        assert!(contract.must.message_history);
    }

    #[test]
    fn load_session_restores_must_items() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("test-session-1", "My Session", SessionStatus::Idle)).unwrap();

        let outcome = load_session(&store, "test-session-1").unwrap();
        match outcome {
            RestorationOutcome::Full(contract) => {
                assert_eq!(contract.must.title.as_deref(), Some("My Session"));
                assert_eq!(contract.must.agent_id, "claude-code");
                assert!(contract.must.message_history);
            }
            _ => panic!("Expected Full restoration"),
        }

        // Verify status was bumped to active
        let record = store.get_session("test-session-1").unwrap().unwrap();
        assert_eq!(record.status, SessionStatus::Active);
    }

    #[test]
    fn load_expired_session_fails() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("dead-session", "Dead", SessionStatus::Expired)).unwrap();

        let result = load_session(&store, "dead-session");
        assert!(matches!(result, Err(RestorationError::Expired)));
    }

    #[test]
    fn load_deleted_session_fails() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("gone", "Gone", SessionStatus::Deleted)).unwrap();

        let result = load_session(&store, "gone");
        assert!(matches!(result, Err(RestorationError::MustItemFailed(..))));
    }

    #[test]
    fn load_unreachable_session_returns_agent_offline() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("offline", "Offline", SessionStatus::Unreachable)).unwrap();

        let result = load_session(&store, "offline");
        assert!(matches!(result, Err(RestorationError::AgentOffline(_))));
    }

    #[test]
    fn load_nonexistent_session_fails() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        let result = load_session(&store, "nonexistent");
        assert!(matches!(result, Err(RestorationError::MustItemFailed(..))));
    }

    #[test]
    fn validate_session_accepts_active() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "OK", SessionStatus::Active)).unwrap();
        assert!(validate_session(&store, "s1").is_ok());
    }

    #[test]
    fn validate_session_rejects_expired() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("dead", "Dead", SessionStatus::Expired)).unwrap();
        assert!(matches!(
            validate_session(&store, "dead"),
            Err(RestorationError::Expired)
        ));
    }

    #[test]
    fn unreachable_recovery_cycle() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        // Session exists locally as active
        store.insert_session(&make_record("s1", "Recoverable", SessionStatus::Active)).unwrap();

        // ACP reports no active sessions (shim is down)
        let report = reconcile(&store, &[]).unwrap();
        assert!(report.newly_unreachable.contains(&"s1".to_string()));
        assert_eq!(report.recovered.len(), 0);

        // Verify status became unreachable
        assert_eq!(
            store.get_session("s1").unwrap().unwrap().status,
            SessionStatus::Unreachable
        );

        // Shim comes back — ACP now reports s1
        let report = reconcile(&store, &["s1".to_string()]).unwrap();
        assert!(report.recovered.contains(&"s1".to_string()));
        assert_eq!(report.newly_unreachable.len(), 0);

        // Verify status is now idle
        assert_eq!(
            store.get_session("s1").unwrap().unwrap().status,
            SessionStatus::Idle
        );
    }

    #[test]
    fn load_failure_is_not_session_death() {
        let dir = TempDir::new().unwrap();
        let store = open_test_store(&dir);

        store.insert_session(&make_record("s1", "Test", SessionStatus::Active)).unwrap();

        // ACP reports no active sessions → becomes unreachable
        let _ = reconcile(&store, &[]).unwrap();
        let status = store.get_session("s1").unwrap().unwrap().status;
        assert_eq!(status, SessionStatus::Unreachable);

        // Now try loading — should get AgentOffline, NOT Expired
        let result = load_session(&store, "s1");
        assert!(matches!(result, Err(RestorationError::AgentOffline(_))));

        // Session should STILL be unreachable, NOT expired
        let status = store.get_session("s1").unwrap().unwrap().status;
        assert_eq!(status, SessionStatus::Unreachable);
    }

    #[test]
    fn session_action_has_unique_trace_ids() {
        let a1 = SessionAction::new("s1".into(), ActionKind::Load);
        let a2 = SessionAction::new("s1".into(), ActionKind::Load);
        assert_ne!(a1.trace_id, a2.trace_id);
        assert_eq!(a1.action, ActionKind::Load);
    }

    #[test]
    fn degradation_policy_mappings() {
        let contract = RestorationContract::default();

        assert_eq!(
            contract.degradation_policy_for("pending_approvals"),
            DegradationPolicy::Skip
        );
        assert_eq!(
            contract.degradation_policy_for("interrupted_tools"),
            DegradationPolicy::RetryOnce
        );
        assert_eq!(
            contract.degradation_policy_for("active_model"),
            DegradationPolicy::Unavailable
        );
        assert_eq!(
            contract.degradation_policy_for("unknown_field"),
            DegradationPolicy::Skip
        );
    }
}
