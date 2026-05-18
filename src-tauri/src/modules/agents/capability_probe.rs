use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Probe Result
// ---------------------------------------------------------------------------

/// Result of probing a specific capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ProbeResult {
    /// Capability is confirmed supported.
    Supported,
    /// Capability is NOT supported by this agent.
    NotSupported,
    /// Probe timed out — agent may be slow or unresponsive.
    Timeout,
    /// Version mismatch — agent version is too old.
    UnsupportedVersion {
        minimum_version: String,
        current_version: String,
    },
    /// Some other error during probing.
    Error(String),
}

impl ProbeResult {
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }

    pub fn is_definitive(&self) -> bool {
        matches!(
            self,
            Self::Supported | Self::NotSupported | Self::UnsupportedVersion { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Agent Capabilities
// ---------------------------------------------------------------------------

/// Runtime-detected capabilities for an ACP agent.
///
/// **EVERY field is probed at runtime. NONE are assumed from configuration.**
/// Agents lie. Reality beats configuration. Always.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapabilities {
    /// Was the agent binary found on $PATH?
    pub detected: bool,
    /// Observed agent version string (e.g. "claude-code 2.1.0").
    pub version: Option<String>,
    /// Negotiated ACP protocol version.
    pub acp_version: Option<String>,
    /// Does the agent support sessions/list?
    pub session_list: ProbeResult,
    /// Does the agent support sessions/load?
    pub session_load: ProbeResult,
    /// Does the agent support sessions/resume?
    pub session_resume: ProbeResult,
    /// Does the agent support model selection?
    pub model_selection: ProbeResult,
    /// Does the agent support session modes?
    pub mode_selection: ProbeResult,
    /// Maximum observed response time in milliseconds.
    pub timeout_ms: u64,
    /// When were these capabilities probed? (stored as epoch millis for serde)
    #[serde(with = "instant_serde")]
    pub probed_at: Instant,
}

impl AgentCapabilities {
    /// Minimum ACP protocol version we require.
    pub const MINIMUM_SUPPORTED_VERSION: &'static str = "1.0.0";

    /// Create an unknown/unsupported default.
    pub fn unknown() -> Self {
        Self {
            detected: false,
            version: None,
            acp_version: None,
            session_list: ProbeResult::NotSupported,
            session_load: ProbeResult::NotSupported,
            session_resume: ProbeResult::NotSupported,
            model_selection: ProbeResult::NotSupported,
            mode_selection: ProbeResult::NotSupported,
            timeout_ms: 10_000,
            probed_at: Instant::now(),
        }
    }

    /// Create a fully-supported stub for a known-good agent.
    pub fn fully_supported(detected: bool, version: Option<String>, acp_version: Option<String>) -> Self {
        Self {
            detected,
            version,
            acp_version,
            session_list: ProbeResult::Supported,
            session_load: ProbeResult::Supported,
            session_resume: ProbeResult::Supported,
            model_selection: ProbeResult::Supported,
            mode_selection: ProbeResult::Supported,
            timeout_ms: 10_000,
            probed_at: Instant::now(),
        }
    }

    /// Check if the agent meets the minimum ACP version requirement.
    pub fn meets_minimum_version(&self) -> bool {
        match &self.acp_version {
            Some(v) => {
                // Simple semver: compare as string (works for well-formed versions)
                // Full semver parsing will be wired in P7 (session_config)
                v.as_str() >= Self::MINIMUM_SUPPORTED_VERSION
            }
            None => false,
        }
    }

    /// Returns true if the agent can handle session history (list + load).
    pub fn supports_session_history(&self) -> bool {
        self.session_list.is_supported() && self.session_load.is_supported()
    }

    /// Returns true if the probe is still fresh (within TTL).
    pub fn is_fresh(&self, ttl: Duration) -> bool {
        self.probed_at.elapsed() < ttl
    }
}

// ---------------------------------------------------------------------------
// Serde helper for Instant
// ---------------------------------------------------------------------------

mod instant_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, Instant};

    pub fn serialize<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Store as a dummy value — Instant can't be serialized directly
        // We use elapsed since probe and store as zero (re-probed on deserialize)
        0u64.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Instant, D::Error>
    where
        D: Deserializer<'de>,
    {
        let _val = u64::deserialize(deserializer)?;
        // Deserialized capabilities are always considered stale
        Ok(Instant::now() - Duration::from_secs(3600))
    }
}

// ---------------------------------------------------------------------------
// Capability Cache (TTL-based)
// ---------------------------------------------------------------------------

/// TTL cache for agent capabilities.
///
/// Prevents calling sessions/list on every UI poll.
/// Cache hit → no ACP call. Manual refresh → force sync.
/// Load session → force validation for that specific session_id.
pub struct CapabilityCache {
    capabilities: HashMap<String, AgentCapabilities>,
    ttl: Duration,
}

impl CapabilityCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            capabilities: HashMap::new(),
            ttl,
        }
    }

    /// Default 30-second TTL cache.
    pub fn with_default_ttl() -> Self {
        Self::new(Duration::from_secs(30))
    }

    /// Get cached capabilities if still fresh.
    pub fn get(&self, agent_id: &str) -> Option<&AgentCapabilities> {
        self.capabilities
            .get(agent_id)
            .filter(|c| c.is_fresh(self.ttl))
    }

    /// Get capabilities, returning whether it was a cache hit or miss.
    pub fn get_with_status(&self, agent_id: &str) -> (Option<&AgentCapabilities>, CacheStatus) {
        match self.capabilities.get(agent_id) {
            Some(caps) if caps.is_fresh(self.ttl) => {
                let age_ms = caps.probed_at.elapsed().as_millis() as u64;
                log::debug!("cache hit: agent={agent_id} age_ms={age_ms}");
                (Some(caps), CacheStatus::Hit {
                    age_ms,
                    total_cached: self.capabilities.len(),
                })
            }
            Some(_) => {
                log::debug!("cache miss (stale): agent={agent_id}");
                (None, CacheStatus::MissStale)
            }
            None => {
                log::debug!("cache miss (absent): agent={agent_id}");
                (None, CacheStatus::MissAbsent)
            }
        }
    }

    /// Insert or update cached capabilities.
    pub fn insert(&mut self, agent_id: String, capabilities: AgentCapabilities) {
        log::info!(
            "capability probe complete: agent={agent_id} detected={} session_list={:?} session_load={:?}",
            capabilities.detected,
            capabilities.session_list,
            capabilities.session_load,
        );
        self.capabilities.insert(agent_id, capabilities);
    }

    /// Invalidate cache for a specific agent.
    pub fn invalidate(&mut self, agent_id: &str) {
        if self.capabilities.remove(agent_id).is_some() {
            log::debug!("cache invalidated: agent={agent_id}");
        }
    }

    /// Invalidate all cached capabilities.
    pub fn invalidate_all(&mut self) {
        let count = self.capabilities.len();
        self.capabilities.clear();
        log::debug!("cache invalidated ALL ({count} entries)");
    }

    /// Force a refresh — will probe the agent again on next get.
    pub fn refresh(&mut self, agent_id: &str) -> &mut Self {
        self.invalidate(agent_id);
        self
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// List all cached agent IDs.
    pub fn agent_ids(&self) -> Vec<&String> {
        self.capabilities.keys().collect()
    }
}

// ---------------------------------------------------------------------------
// Cache Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    Hit {
        age_ms: u64,
        total_cached: usize,
    },
    MissStale,
    MissAbsent,
}

// ---------------------------------------------------------------------------
// Agent Detection
// ---------------------------------------------------------------------------

/// Detect if an ACP agent binary is on $PATH.
pub fn detect_agent(agent_command: &str) -> bool {
    match which::which(agent_command) {
        Ok(path) => {
            log::info!("agent detected: {agent_command} at {}", path.display());
            true
        }
        Err(_) => {
            log::debug!("agent not found on PATH: {agent_command}");
            false
        }
    }
}

/// Detect an agent and return its full path if found.
pub fn detect_agent_path(agent_command: &str) -> Option<std::path::PathBuf> {
    which::which(agent_command).ok()
}

// ---------------------------------------------------------------------------
// Probe functions (stubs — wired in P4 with actual ACP protocol)
// ---------------------------------------------------------------------------

/// Probe the agent's ACP capabilities via actual protocol calls.
///
/// In P4 (AgentRouter), this will:
/// 1. Spawn the ACP shim subprocess
/// 2. Send InitializeRequest
/// 3. Read InitializeResponse.agent_capabilities
/// 4. Probe sessions/list, sessions/load
/// 5. Detect model/mode support from capability flags
///
/// For now, returns a detection-only probe based on PATH availability.
pub fn probe_agent_capabilities(agent_command: &str) -> AgentCapabilities {
    let detected = detect_agent(agent_command);

    if !detected {
        log::warn!("agent not available: {agent_command}");
        return AgentCapabilities::unknown();
    }

    // TODO P4: Wire actual ACP initialization + capability enumeration
    // For now, mark as fully supported if the binary exists on PATH.
    // The actual capabilities will be refined when the ACP shim is connected.
    log::info!(
        "agent capability probe (stub): agent={agent_command} — marking as fully supported (actual probe in P4)"
    );

    let version = probe_agent_version(agent_command);
    AgentCapabilities::fully_supported(
        true,
        version.clone(),
        version, // Use agent version as ACP version placeholder until real init
    )
}

/// Try to get the agent version string (e.g. `claude --version`).
fn probe_agent_version(agent_command: &str) -> Option<String> {
    match std::process::Command::new(agent_command)
        .arg("--version")
        .output()
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let version = stdout.lines().next().unwrap_or("").trim().to_string();
            if version.is_empty() {
                None
            } else {
                Some(version)
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Probe Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("Agent not found on PATH: {0}")]
    NotFound(String),
    #[error("Probe timed out after {0:?}")]
    Timeout(Duration),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Version mismatch: need {minimum}, found {found}")]
    VersionMismatch { minimum: String, found: String },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn cache_hit_within_ttl() {
        let mut cache = CapabilityCache::new(Duration::from_secs(30));
        cache.insert("test-agent".into(), AgentCapabilities::unknown());
        assert!(cache.get("test-agent").is_some());
    }

    #[test]
    fn cache_miss_after_ttl_expires() {
        let mut cache = CapabilityCache::new(Duration::from_millis(1));
        cache.insert("test-agent".into(), AgentCapabilities::unknown());
        thread::sleep(Duration::from_millis(5));
        assert!(cache.get("test-agent").is_none());
    }

    #[test]
    fn cache_invalidate_removes_entry() {
        let mut cache = CapabilityCache::new(Duration::from_secs(30));
        cache.insert("test-agent".into(), AgentCapabilities::unknown());
        cache.invalidate("test-agent");
        assert!(cache.get("test-agent").is_none());
    }

    #[test]
    fn cache_invalidate_all_clears_everything() {
        let mut cache = CapabilityCache::new(Duration::from_secs(30));
        cache.insert("a".into(), AgentCapabilities::unknown());
        cache.insert("b".into(), AgentCapabilities::unknown());
        cache.invalidate_all();
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_refresh_invalidates_and_returns_self() {
        let mut cache = CapabilityCache::new(Duration::from_secs(30));
        cache.insert("test-agent".into(), AgentCapabilities::unknown());
        cache.refresh("test-agent");
        assert!(cache.get("test-agent").is_none());
    }

    #[test]
    fn probe_result_is_supported() {
        assert!(ProbeResult::Supported.is_supported());
        assert!(!ProbeResult::NotSupported.is_supported());
        assert!(!ProbeResult::Timeout.is_supported());
        assert!(!ProbeResult::Error("oops".into()).is_supported());
    }

    #[test]
    fn probe_result_is_definitive() {
        assert!(ProbeResult::Supported.is_definitive());
        assert!(ProbeResult::NotSupported.is_definitive());
        assert!(!ProbeResult::Timeout.is_definitive());
        assert!(!ProbeResult::Error("oops".into()).is_definitive());
    }

    #[test]
    fn unknown_caps_all_not_supported() {
        let caps = AgentCapabilities::unknown();
        assert!(!caps.detected);
        assert!(!caps.session_list.is_supported());
        assert!(!caps.session_load.is_supported());
        assert!(!caps.session_resume.is_supported());
        assert!(!caps.model_selection.is_supported());
        assert!(!caps.mode_selection.is_supported());
    }

    #[test]
    fn fully_supported_caps() {
        let caps = AgentCapabilities::fully_supported(
            true,
            Some("claude-code 2.1.0".into()),
            Some("1.2.0".into()),
        );
        assert!(caps.detected);
        assert!(caps.session_list.is_supported());
        assert!(caps.session_load.is_supported());
        assert!(caps.meets_minimum_version());
        assert!(caps.supports_session_history());
    }

    #[test]
    fn version_check_minimum() {
        let mut caps = AgentCapabilities::unknown();
        assert!(!caps.meets_minimum_version());

        caps.acp_version = Some("0.9.0".into());
        assert!(!caps.meets_minimum_version());

        caps.acp_version = Some("1.0.0".into());
        assert!(caps.meets_minimum_version());

        caps.acp_version = Some("2.0.0".into());
        assert!(caps.meets_minimum_version());
    }

    #[test]
    fn detect_non_existent_agent() {
        assert!(!detect_agent("this-binary-does-not-exist-xyzzy"));
    }

    #[test]
    fn get_with_status_returns_correct_status() {
        let mut cache = CapabilityCache::new(Duration::from_secs(30));
        cache.insert("agent-a".into(), AgentCapabilities::unknown());

        let (caps, status) = cache.get_with_status("agent-a");
        assert!(caps.is_some());
        assert!(matches!(status, CacheStatus::Hit { .. }));

        let (caps, status) = cache.get_with_status("agent-b");
        assert!(caps.is_none());
        assert_eq!(status, CacheStatus::MissAbsent);
    }

    #[test]
    fn stale_cache_returns_miss_stale() {
        let mut cache = CapabilityCache::new(Duration::from_millis(1));
        cache.insert("agent-a".into(), AgentCapabilities::unknown());
        thread::sleep(Duration::from_millis(5));

        let (caps, status) = cache.get_with_status("agent-a");
        assert!(caps.is_none());
        assert_eq!(status, CacheStatus::MissStale);
    }

    #[test]
    fn probe_error_display() {
        let e = ProbeError::NotFound("claude".into());
        assert!(e.to_string().contains("claude"));

        let e = ProbeError::Timeout(Duration::from_secs(5));
        assert!(e.to_string().contains("5s"));

        let e = ProbeError::VersionMismatch {
            minimum: "1.0.0".into(),
            found: "0.9.0".into(),
        };
        assert!(e.to_string().contains("1.0.0"));
    }
}
