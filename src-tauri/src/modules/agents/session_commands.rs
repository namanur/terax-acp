//! Tauri commands exposed to the frontend for agent operations.
//!
//! These are registered in lib.rs via tauri::generate_handler![].

use tauri::State;

use super::agent_router::{AgentContext, AgentState, ExecutionMode, SessionInfo};
use super::restoration::{self, RestorationOutcome};

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

/// List all sessions across the active backend AND legacy store.
#[tauri::command]
pub async fn agent_session_list(
    state: State<'_, AgentState>,
) -> Result<Vec<SessionInfo>, String> {
    // Get active sessions from the router's current backend
    let active_sessions = state
        .router
        .session_list()
        .await
        .map_err(|e| e.to_string())?;

    Ok(active_sessions)
}

/// Load a session by ID. Triggers the restoration contract.
#[tauri::command]
pub async fn agent_session_load(
    state: State<'_, AgentState>,
    session_id: String,
) -> Result<SerdeOutcome, String> {
    use restoration::load_session;

    match load_session(&state.session_store, &session_id) {
        Ok(outcome) => Ok(outcome.into()),
        Err(e) => {
            log::error!("agent_session_load failed: session_id={session_id} error={e}");
            Err(e.to_string())
        }
    }
}

/// Close a session via the active backend.
#[tauri::command]
pub async fn agent_session_close(
    state: State<'_, AgentState>,
    session_id: String,
) -> Result<(), String> {
    use super::session_store::SessionStatus;

    state
        .session_store
        .set_status(&session_id, SessionStatus::Closed)
        .map_err(|e| e.to_string())?;

    log::info!("agent_session_close: session_id={session_id}");
    Ok(())
}

/// Delete a session from the local store.
#[tauri::command]
pub async fn agent_session_delete(
    state: State<'_, AgentState>,
    session_id: String,
) -> Result<(), String> {
    use super::session_store::SessionStatus;

    state
        .session_store
        .set_status(&session_id, SessionStatus::Deleted)
        .map_err(|e| e.to_string())?;

    log::info!("agent_session_delete: session_id={session_id}");
    Ok(())
}

/// Switch execution mode (API Provider ↔ ACP Agent).
#[tauri::command]
pub async fn agent_set_execution_mode(
    state: State<'_, AgentState>,
    mode: String,
) -> Result<(), String> {
    let mode: ExecutionMode = mode.as_str().into();
    state.router.set_mode(mode);
    log::info!("agent_set_execution_mode: mode={mode}");
    Ok(())
}

/// Get current execution mode.
#[tauri::command]
pub async fn agent_get_execution_mode(
    state: State<'_, AgentState>,
) -> Result<String, String> {
    Ok(state.router.mode().to_string())
}

/// Send a message through the active backend.
#[tauri::command]
pub async fn agent_session_send(
    state: State<'_, AgentState>,
    session_id: String,
    message: String,
    cwd: Option<String>,
) -> Result<(), String> {
    let context = AgentContext {
        cwd,
        terminal_context: None,
        workspace_files: Vec::new(),
    };

    let _stream = state
        .router
        .send_message(&session_id, message, context)
        .await
        .map_err(|e| e.to_string())?;

    // TODO P5: Stream tokens to frontend via Tauri events
    // For now, the send is acknowledged

    Ok(())
}

// ---------------------------------------------------------------------------
// Debug commands (P6)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn agent_debug_enable(
    state: State<'_, AgentState>,
) -> Result<(), String> {
    state.acp_debug.enable();
    Ok(())
}

#[tauri::command]
pub async fn agent_debug_disable(
    state: State<'_, AgentState>,
) -> Result<(), String> {
    state.acp_debug.disable();
    Ok(())
}

#[tauri::command]
pub async fn agent_debug_messages(
    state: State<'_, AgentState>,
) -> Result<Vec<super::acp_debug::DebugMessage>, String> {
    Ok(state.acp_debug.get_messages())
}

#[tauri::command]
pub async fn agent_debug_stats(
    state: State<'_, AgentState>,
) -> Result<super::acp_debug::DebugBufferStats, String> {
    Ok(state.acp_debug.stats())
}

#[tauri::command]
pub async fn agent_debug_clear(
    state: State<'_, AgentState>,
) -> Result<(), String> {
    state.acp_debug.clear();
    Ok(())
}

// ---------------------------------------------------------------------------
// Config commands (P7)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn agent_session_get_config(
    _state: State<'_, AgentState>,
    _session_id: String,
) -> Result<super::session_config::SessionConfig, String> {
    // Get capabilities to check what's available
    // let cache = state.capability_cache.lock().unwrap();
    // ... fetch config from agent
    Err("Not yet implemented".to_string())
}

#[tauri::command]
pub async fn agent_session_set_mode(
    _state: State<'_, AgentState>,
    _session_id: String,
    _mode_id: String,
) -> Result<(), String> {
    // ... validate and set mode
    Err("Not yet implemented".to_string())
}

#[tauri::command]
pub async fn agent_session_set_model(
    _state: State<'_, AgentState>,
    _session_id: String,
    _model_id: String,
) -> Result<(), String> {
    // ... validate and set model
    Err("Not yet implemented".to_string())
}

#[tauri::command]
pub async fn agent_list_models(
    _state: State<'_, AgentState>,
    _agent_id: String,
) -> Result<Vec<super::session_config::ModelInfo>, String> {
    // ... query agent for available models
    Err("Not yet implemented".to_string())
}

// ---------------------------------------------------------------------------
// Serde-friendly outcome for Tauri commands
// ---------------------------------------------------------------------------

/// Serializable restoration outcome for Tauri IPC.
/// RestorationOutcome contains non-serializable fields (future, etc.),
/// so we convert to this bridge type for frontend consumption.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum SerdeOutcome {
    #[serde(rename = "full")]
    Full {
        session_id: String,
        title: Option<String>,
        title_source: Option<String>,
        agent_id: String,
    },
    #[serde(rename = "degraded")]
    Degraded {
        session_id: String,
        failed_may: Vec<String>,
    },
    #[serde(rename = "failed")]
    Failed {
        session_id: String,
        failed_must: Vec<String>,
        error: String,
    },
}

impl From<RestorationOutcome> for SerdeOutcome {
    fn from(o: RestorationOutcome) -> Self {
        match o {
            RestorationOutcome::Full(contract) => SerdeOutcome::Full {
                session_id: contract.session_id,
                title: contract.must.title,
                title_source: contract.must.title_source,
                agent_id: contract.must.agent_id,
            },
            RestorationOutcome::Degraded {
                contract,
                failed_may,
            } => SerdeOutcome::Degraded {
                session_id: contract.session_id,
                failed_may,
            },
            RestorationOutcome::Failed {
                contract,
                failed_must,
                error,
            } => SerdeOutcome::Failed {
                session_id: contract.session_id,
                failed_must,
                error,
            },
        }
    }
}
