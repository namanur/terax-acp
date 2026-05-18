use serde::{Deserialize, Serialize};

/// An agent session mode (e.g., "code", "architect", "plan").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMode {
    pub mode_id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
}

/// Current session configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub session_id: String,
    pub current_mode: Option<SessionMode>,
    pub available_modes: Vec<SessionMode>,
    pub current_model: Option<ModelInfo>,
    pub available_models: Vec<ModelInfo>,
    pub config_options: Vec<SessionConfigOption>,
}

/// A selectable model for the ACP agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub is_latest: bool,
    pub cost_info: Option<String>,
}

/// A configurable option for the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfigOption {
    pub config_id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub current_value: ConfigValue,
    pub available_values: Vec<ConfigValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValue {
    pub value_id: String,
    pub display_name: String,
}

use super::capability_probe::AgentCapabilities;

impl SessionConfig {
    /// Get session config from the ACP agent.
    /// This calls the ACP protocol to query available modes and models.
    pub async fn fetch_from_agent(
        session_id: &str,
        agent_capabilities: &AgentCapabilities,
    ) -> Result<SessionConfig, ConfigError> {
        let config = SessionConfig {
            session_id: session_id.to_string(),
            current_mode: None,
            available_modes: Vec::new(),
            current_model: None,
            available_models: Vec::new(),
            config_options: Vec::new(),
        };

        // Only probe if agent supports it
        if agent_capabilities.mode_selection.is_supported() {
            // TODO: Call ACP sessions/modes/list
            // config.available_modes = agent.list_modes(session_id).await?;
            // config.current_mode = agent.current_mode(session_id).await?;
        }

        if agent_capabilities.model_selection.is_supported() {
            // TODO: Call ACP models/list
            // config.available_models = agent.list_models(session_id).await?;
            // config.current_model = agent.current_model(session_id).await?;
        }

        Ok(config)
    }

    /// Set the active mode for a session.
    pub async fn set_mode(
        &mut self,
        mode_id: &str,
        // agent: &dyn AcpClient,
    ) -> Result<(), ConfigError> {
        if !self.available_modes.iter().any(|m| m.mode_id == mode_id) {
            return Err(ConfigError::InvalidMode(mode_id.to_string()));
        }
        // TODO: agent.set_mode(session_id, mode_id).await?;
        self.current_mode = self.available_modes
            .iter()
            .find(|m| m.mode_id == mode_id)
            .cloned();
        Ok(())
    }

    /// Set the active model for a session.
    pub async fn set_model(
        &mut self,
        model_id: &str,
        // agent: &dyn AcpClient,
    ) -> Result<(), ConfigError> {
        if !self.available_models.iter().any(|m| m.model_id == model_id) {
            return Err(ConfigError::InvalidModel(model_id.to_string()));
        }
        // TODO: agent.set_model(session_id, model_id).await?;
        self.current_model = self.available_models
            .iter()
            .find(|m| m.model_id == model_id)
            .cloned();
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Invalid mode: {0}")]
    InvalidMode(String),
    #[error("Invalid model: {0}")]
    InvalidModel(String),
    #[error("Agent doesn't support mode selection")]
    ModeSelectionNotSupported,
    #[error("Agent doesn't support model selection")]
    ModelSelectionNotSupported,
    #[error("Protocol error: {0}")]
    Protocol(String),
}
