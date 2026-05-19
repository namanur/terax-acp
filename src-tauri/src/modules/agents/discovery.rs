use serde::{Deserialize, Serialize};

use super::capability_probe::{detect_agent_path, probe_agent_capabilities, AgentCapabilities};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProbeSummary {
    pub command: String,
    pub found: bool,
    pub resolved_path: Option<String>,
    pub version: Option<String>,
    pub capabilities: AgentCapabilities,
}

#[tauri::command]
pub async fn agent_probe(command: String) -> Result<AgentProbeSummary, String> {
    let capabilities = probe_agent_capabilities(&command);
    let resolved_path = detect_agent_path(&command).map(|path| path.display().to_string());
    let version = capabilities.version.clone();

    Ok(AgentProbeSummary {
        command,
        found: capabilities.detected,
        resolved_path,
        version,
        capabilities,
    })
}
