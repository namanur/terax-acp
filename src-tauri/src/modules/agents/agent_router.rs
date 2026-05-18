#![allow(dead_code)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::capability_probe::CapabilityCache;
use super::restoration::RestorationError;
use super::session_store::{self, SessionStore, SessionStoreError};

// ---------------------------------------------------------------------------
// Agent Context
// ---------------------------------------------------------------------------

/// Context passed from the UI/Tauri layer to the active backend.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub cwd: Option<String>,
    pub terminal_context: Option<String>,
    pub workspace_files: Vec<String>,
}

// Type alias to reduce complexity of the streaming return type
/// Boxed future that yields optional token results.
pub type NextTokenFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Option<Result<String, RouterError>>> + Send + 'a>,
>;

// ---------------------------------------------------------------------------
// Response Stream
// ---------------------------------------------------------------------------

/// Streaming response from a backend.
/// Implementations yield tokens as they arrive.
pub trait ResponseStream: Send {
    fn next_token(&mut self) -> NextTokenFuture<'_>;
    fn is_complete(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Approval types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub session_id: String,
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub tool_description: String,
}

#[derive(Debug, Clone)]
pub enum ApprovalOutcome {
    Approved,
    Denied,
    ApprovedWithParams(serde_json::Value),
}

// ---------------------------------------------------------------------------
// Session types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub session_id: String,
    pub messages: Vec<String>,
    pub title: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub title: Option<String>,
    pub title_source: Option<String>,
    pub agent_id: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_message_preview: Option<String>,
    pub token_usage: Option<serde_json::Value>,
}

impl From<session_store::SessionRecord> for SessionInfo {
    fn from(r: session_store::SessionRecord) -> Self {
        Self {
            session_id: r.session_id,
            title: r.title,
            title_source: Some(match r.title_source {
                session_store::TitleSource::User => "user".into(),
                session_store::TitleSource::Agent => "agent".into(),
                session_store::TitleSource::Provisional => "provisional".into(),
            }),
            agent_id: r.agent_id,
            status: r.status.as_str().to_string(),
            created_at: r.created_at.to_string(),
            updated_at: r.updated_at.to_string(),
            last_message_preview: r.last_message_preview,
            token_usage: r.token_usage.map(|t| {
                serde_json::json!({
                    "input_tokens": t.input_tokens,
                    "output_tokens": t.output_tokens,
                    "total_tokens": t.total_tokens,
                    "max_tokens": t.max_tokens,
                })
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Execution Mode
// ---------------------------------------------------------------------------

/// Execution mode: switches between API Provider and ACP Agent backends.
/// Replaces the "Provider" concept. One switch. No UI obesity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    #[serde(rename = "api_provider")]
    ApiProvider,
    #[serde(rename = "acp_agent")]
    AcpAgent,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::ApiProvider => write!(f, "api_provider"),
            ExecutionMode::AcpAgent => write!(f, "acp_agent"),
        }
    }
}

impl From<&str> for ExecutionMode {
    fn from(s: &str) -> Self {
        match s {
            "acp_agent" => ExecutionMode::AcpAgent,
            _ => ExecutionMode::ApiProvider,
        }
    }
}

// ---------------------------------------------------------------------------
// Router Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("No backend registered for mode '{0}'")]
    NoBackend(String),
    #[error("Backend error: {0}")]
    BackendError(String),
    #[error("Store error: {0}")]
    StoreError(#[from] SessionStoreError),
    #[error("Restoration error: {0}")]
    RestorationError(#[from] RestorationError),
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Probe error: {0}")]
    ProbeError(String),
    #[error("Operation not supported by this backend")]
    NotSupported,
}

// ---------------------------------------------------------------------------
// AgentBackend trait
// ---------------------------------------------------------------------------

/// Unified contract for all AI execution backends.
///
/// Both the existing API provider backend (net.rs) and the new ACP agent
/// backend implement this trait. The UI never knows which backend it's
/// talking to — it goes through the AgentRouter.
#[async_trait]
pub trait AgentBackend: Send + Sync + std::fmt::Debug {
    /// Send a message to the backend and get a response stream.
    async fn send_message(
        &self,
        session_id: &str,
        message: String,
        context: AgentContext,
    ) -> Result<Box<dyn ResponseStream>, RouterError>;

    /// Stream response tokens from an active session.
    async fn stream_response(
        &self,
        session_id: &str,
    ) -> Result<Box<dyn ResponseStream>, RouterError>;

    /// Request tool call approval from the user.
    async fn tool_approval(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalOutcome, RouterError>;

    /// Resume an existing session.
    async fn session_resume(
        &self,
        session_id: &str,
    ) -> Result<SessionHandle, RouterError>;

    /// List all sessions for this backend.
    async fn session_list(&self) -> Result<Vec<SessionInfo>, RouterError>;

    /// Human-readable display name (e.g. "API Provider", "Claude Code").
    fn display_name(&self) -> &str;

    /// Unique backend identifier (e.g. "api_provider", "acp_agent").
    fn backend_id(&self) -> &str;
}

// ---------------------------------------------------------------------------
// AgentRouter
// ---------------------------------------------------------------------------

/// Central dispatch layer. Routes to API Provider or ACP Agent backend
/// based on the current ExecutionMode setting.
pub struct AgentRouter {
    backends: HashMap<String, Box<dyn AgentBackend>>,
    mode: RwLock<ExecutionMode>,
}

impl std::fmt::Debug for AgentRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRouter")
            .field("mode", &self.mode)
            .field("backends", &self.backends.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl AgentRouter {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
            mode: RwLock::new(ExecutionMode::ApiProvider),
        }
    }

    /// Register a backend implementation.
    pub fn register(&mut self, backend: Box<dyn AgentBackend>) {
        let id = backend.backend_id().to_string();
        log::info!("agent router: registered backend id={id} name={}", backend.display_name());
        self.backends.insert(id, backend);
    }

    /// Set the active execution mode.
    pub fn set_mode(&self, mode: ExecutionMode) {
        let old = *self.mode.read().unwrap();
        *self.mode.write().unwrap() = mode;
        log::info!("agent router: execution mode {old} → {mode}");
    }

    /// Get the current execution mode.
    pub fn mode(&self) -> ExecutionMode {
        *self.mode.read().unwrap()
    }

    /// Get the active backend based on current execution mode.
    fn active_backend(&self) -> Result<&dyn AgentBackend, RouterError> {
        let mode = self.mode();
        let backend_id = match mode {
            ExecutionMode::ApiProvider => "api_provider",
            ExecutionMode::AcpAgent => "acp_agent",
        };
        self.backends
            .get(backend_id)
            .map(|b| b.as_ref())
            .ok_or_else(|| {
                log::error!("agent router: no backend registered for mode {mode}");
                RouterError::NoBackend(mode.to_string())
            })
    }

    /// Check if a specific backend is registered.
    pub fn has_backend(&self, backend_id: &str) -> bool {
        self.backends.contains_key(backend_id)
    }

    /// List all registered backend IDs.
    pub fn backend_ids(&self) -> Vec<String> {
        self.backends.keys().cloned().collect()
    }

    // -- Dispatch methods ------------------------------------------------

    pub async fn send_message(
        &self,
        session_id: &str,
        message: String,
        context: AgentContext,
    ) -> Result<Box<dyn ResponseStream>, RouterError> {
        let backend = self.active_backend()?;
        log::debug!(
            "agent router: send_message backend={} session_id={session_id}",
            backend.backend_id()
        );
        backend.send_message(session_id, message, context).await
    }

    pub async fn session_list(&self) -> Result<Vec<SessionInfo>, RouterError> {
        let backend = self.active_backend()?;
        log::debug!(
            "agent router: session_list backend={}",
            backend.backend_id()
        );
        backend.session_list().await
    }

    pub async fn session_resume(&self, session_id: &str) -> Result<SessionHandle, RouterError> {
        let backend = self.active_backend()?;
        log::debug!(
            "agent router: session_resume backend={} session_id={session_id}",
            backend.backend_id()
        );
        backend.session_resume(session_id).await
    }

    pub async fn stream_response(
        &self,
        session_id: &str,
    ) -> Result<Box<dyn ResponseStream>, RouterError> {
        let backend = self.active_backend()?;
        log::debug!(
            "agent router: stream_response backend={} session_id={session_id}",
            backend.backend_id()
        );
        backend.stream_response(session_id).await
    }

    pub async fn tool_approval(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalOutcome, RouterError> {
        let backend = self.active_backend()?;
        log::debug!(
            "agent router: tool_approval backend={} tool={}",
            backend.backend_id(),
            request.tool_name
        );
        backend.tool_approval(request).await
    }
}

// ---------------------------------------------------------------------------
// AgentState — Tauri managed state
// ---------------------------------------------------------------------------

/// Tauri managed state for the entire agents module.
/// Registered once at app startup via `.manage()`.
pub struct AgentState {
    pub router: Arc<AgentRouter>,
    pub session_store: Arc<SessionStore>,
    pub capability_cache: Arc<std::sync::Mutex<CapabilityCache>>,
    pub acp_debug: Arc<super::acp_debug::AcpDebugBuffer>,
}

impl AgentState {
    /// Create the agent state with a SQLite-backed session store,
    /// capability cache, and router (backends registered separately).
    pub fn new(db_path: &Path) -> Result<Self, SessionStoreError> {
        let session_store = Arc::new(SessionStore::open(db_path)?);
        let router = AgentRouter::new();
        let capability_cache = Arc::new(std::sync::Mutex::new(
            CapabilityCache::new(Duration::from_secs(30)),
        ));
        let acp_debug = Arc::new(super::acp_debug::AcpDebugBuffer::new(None));

        log::info!(
            "agent state initialized: db={}",
            db_path.display()
        );

        Ok(Self {
            router: Arc::new(router),
            session_store,
            capability_cache,
            acp_debug,
        })
    }

    /// Close debounce channel on graceful shutdown.
    pub fn shutdown(&self) {
        log::info!("agent state: graceful shutdown — flushing debounce");
        self.session_store.close_debounce_channel();
        if let Err(e) = self.session_store.wal_checkpoint() {
            log::warn!("agent state: WAL checkpoint on shutdown failed: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Stub backends for testing ---------------------------------------

    struct StubApiProviderBackend;

    impl std::fmt::Debug for StubApiProviderBackend {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("StubApiProviderBackend").finish()
        }
    }

    #[async_trait]
    impl AgentBackend for StubApiProviderBackend {
        async fn send_message(
            &self,
            _session_id: &str,
            _message: String,
            _context: AgentContext,
        ) -> Result<Box<dyn ResponseStream>, RouterError> {
            Err(RouterError::NotSupported)
        }

        async fn stream_response(
            &self,
            _session_id: &str,
        ) -> Result<Box<dyn ResponseStream>, RouterError> {
            Err(RouterError::NotSupported)
        }

        async fn tool_approval(
            &self,
            _request: ApprovalRequest,
        ) -> Result<ApprovalOutcome, RouterError> {
            Ok(ApprovalOutcome::Approved)
        }

        async fn session_resume(
            &self,
            session_id: &str,
        ) -> Result<SessionHandle, RouterError> {
            Ok(SessionHandle {
                session_id: session_id.to_string(),
                messages: vec!["Hello from API Provider".into()],
                title: Some("API Session".into()),
                status: "active".into(),
            })
        }

        async fn session_list(&self) -> Result<Vec<SessionInfo>, RouterError> {
            Ok(vec![SessionInfo {
                session_id: "api-1".into(),
                title: Some("API Session".into()),
                title_source: Some("user".into()),
                agent_id: "openai".into(),
                status: "active".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                last_message_preview: None,
                token_usage: None,
            }])
        }

        fn display_name(&self) -> &str {
            "API Provider"
        }

        fn backend_id(&self) -> &str {
            "api_provider"
        }
    }

    struct StubAcpBackend;

    impl std::fmt::Debug for StubAcpBackend {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("StubAcpBackend").finish()
        }
    }

    #[async_trait]
    impl AgentBackend for StubAcpBackend {
        async fn send_message(
            &self,
            _session_id: &str,
            _message: String,
            _context: AgentContext,
        ) -> Result<Box<dyn ResponseStream>, RouterError> {
            Err(RouterError::NotSupported)
        }

        async fn stream_response(
            &self,
            _session_id: &str,
        ) -> Result<Box<dyn ResponseStream>, RouterError> {
            Err(RouterError::NotSupported)
        }

        async fn tool_approval(
            &self,
            _request: ApprovalRequest,
        ) -> Result<ApprovalOutcome, RouterError> {
            Ok(ApprovalOutcome::Denied)
        }

        async fn session_resume(
            &self,
            session_id: &str,
        ) -> Result<SessionHandle, RouterError> {
            Ok(SessionHandle {
                session_id: session_id.to_string(),
                messages: vec!["Hello from ACP Agent".into()],
                title: Some("ACP Session".into()),
                status: "active".into(),
            })
        }

        async fn session_list(&self) -> Result<Vec<SessionInfo>, RouterError> {
            Ok(vec![SessionInfo {
                session_id: "acp-1".into(),
                title: Some("ACP Session".into()),
                title_source: Some("agent".into()),
                agent_id: "claude-code".into(),
                status: "active".into(),
                created_at: "2026-01-02T00:00:00Z".into(),
                updated_at: "2026-01-02T00:00:00Z".into(),
                last_message_preview: None,
                token_usage: None,
            }])
        }

        fn display_name(&self) -> &str {
            "ACP Agent"
        }

        fn backend_id(&self) -> &str {
            "acp_agent"
        }
    }

    // -- Tests -----------------------------------------------------------

    #[test]
    fn router_dispatches_to_active_backend() {
        let mut router = AgentRouter::new();
        router.register(Box::new(StubApiProviderBackend));
        router.register(Box::new(StubAcpBackend));

        // Default: API Provider
        assert_eq!(router.mode(), ExecutionMode::ApiProvider);
        let backend = router.active_backend().unwrap();
        assert_eq!(backend.display_name(), "API Provider");
        assert_eq!(backend.backend_id(), "api_provider");

        // Switch to ACP
        router.set_mode(ExecutionMode::AcpAgent);
        assert_eq!(router.mode(), ExecutionMode::AcpAgent);
        let backend = router.active_backend().unwrap();
        assert_eq!(backend.display_name(), "ACP Agent");
        assert_eq!(backend.backend_id(), "acp_agent");
    }

    #[test]
    fn router_no_backend_error() {
        let router = AgentRouter::new();
        // No backends registered
        let result = router.active_backend();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RouterError::NoBackend(_)));
    }

    #[test]
    fn router_only_api_provider_errors_on_acp_mode() {
        let mut router = AgentRouter::new();
        router.register(Box::new(StubApiProviderBackend));
        // Switch to ACP mode but no ACP backend registered
        router.set_mode(ExecutionMode::AcpAgent);
        let result = router.active_backend();
        assert!(result.is_err());
    }

    #[test]
    fn router_has_backend() {
        let mut router = AgentRouter::new();
        router.register(Box::new(StubApiProviderBackend));

        assert!(router.has_backend("api_provider"));
        assert!(!router.has_backend("acp_agent"));
        assert_eq!(router.backend_ids(), vec!["api_provider"]);
    }

    #[tokio::test]
    async fn router_session_list_dispatches() {
        let mut router = AgentRouter::new();
        router.register(Box::new(StubApiProviderBackend));

        let sessions = router.session_list().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_id, "openai");
    }

    #[tokio::test]
    async fn router_session_resume_dispatches() {
        let mut router = AgentRouter::new();
        router.register(Box::new(StubAcpBackend));
        router.set_mode(ExecutionMode::AcpAgent);

        let handle = router.session_resume("acp-1").await.unwrap();
        assert_eq!(handle.session_id, "acp-1");
        assert_eq!(handle.messages.first().unwrap(), "Hello from ACP Agent");
    }

    #[test]
    fn execution_mode_from_str() {
        assert_eq!(ExecutionMode::from("api_provider"), ExecutionMode::ApiProvider);
        assert_eq!(ExecutionMode::from("acp_agent"), ExecutionMode::AcpAgent);
        assert_eq!(ExecutionMode::from("unknown"), ExecutionMode::ApiProvider); // safe default
    }

    #[test]
    fn execution_mode_display() {
        assert_eq!(ExecutionMode::ApiProvider.to_string(), "api_provider");
        assert_eq!(ExecutionMode::AcpAgent.to_string(), "acp_agent");
    }

    #[test]
    fn session_info_from_record() {
        let record = session_store::SessionRecord {
            session_id: "s1".into(),
            title: Some("My Session".into()),
            title_source: session_store::TitleSource::User,
            agent_id: "claude-code".into(),
            work_dirs: vec![],
            created_at: 1000,
            updated_at: 2000,
            last_message_preview: Some("Hello...".into()),
            token_usage: Some(session_store::TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                total_tokens: 150,
                max_tokens: 4096,
            }),
            meta: None,
            status: session_store::SessionStatus::Active,
        };

        let info: SessionInfo = record.into();
        assert_eq!(info.session_id, "s1");
        assert_eq!(info.title.as_deref(), Some("My Session"));
        assert_eq!(info.title_source.as_deref(), Some("user"));
        assert_eq!(info.agent_id, "claude-code");
        assert_eq!(info.status, "active");
    }

    #[test]
    fn agent_state_creates_with_session_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let state = AgentState::new(&db_path).unwrap();

        // Verify session store works
        state.session_store.integrity_check().unwrap();

        // Verify router is empty by default
        assert!(!state.router.has_backend("api_provider"));
    }
}
