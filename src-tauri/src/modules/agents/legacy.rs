use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A legacy session read from the tauri-plugin-store JSON format.
///
/// These are READ-ONLY — viewable, referenceable, but NOT resumable as ACP
/// sessions and NOT auto-migrated to SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacySession {
    pub id: String,
    pub title: Option<String>,
    pub created_at: Option<String>, // ISO 8601
    pub updated_at: Option<String>, // ISO 8601
    pub message_count: usize,
    pub last_preview: Option<String>,
}

/// Reader for legacy tauri-plugin-store JSON sessions.
///
/// Opens `terax-ai-sessions.json` (or a similarly structured file) and
/// exposes sessions as a read-only list.  Does NOT require SQLite.
pub struct LegacySessionStore {
    path: PathBuf,
}

impl LegacySessionStore {
    /// Open the legacy JSON store.
    ///
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns an error only if the file exists but cannot be read/parsed.
    pub fn open(path: &Path) -> Result<Option<Self>, std::io::Error> {
        if !path.exists() {
            log::debug!("no legacy session file at {:?}, skipping", path);
            return Ok(None);
        }
        log::info!(
            "legacy session file found at {:?}",
            path
        );
        Ok(Some(Self {
            path: path.to_path_buf(),
        }))
    }

    /// List legacy sessions from the JSON file. Read-only snapshot.
    pub fn list_sessions(&self) -> Result<Vec<LegacySession>, LegacyStoreError> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| LegacyStoreError::Io(e))?;

        let parsed: LegacyStoreFormat = serde_json::from_str(&raw)
            .map_err(|e| LegacyStoreError::Json(e))?;

        let sessions: Vec<LegacySession> = parsed
            .into_iter()
            .map(|(id, entry)| {
                let message_count = entry.messages.as_ref().map(|m| m.len()).unwrap_or(0);
                let last_preview = entry.messages.as_ref()
                    .and_then(|msgs| msgs.last())
                    .and_then(|m| m.content.as_ref())
                    .map(|c| truncate_preview(c, 120));
                LegacySession {
                id,
                title: entry.title,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
                message_count,
                last_preview,
            }
            })
            .collect();

        log::trace!("loaded {} legacy sessions", sessions.len());
        Ok(sessions)
    }

    /// Get messages for a specific legacy session. Read-only.
    pub fn get_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<serde_json::Value>, LegacyStoreError> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| LegacyStoreError::Io(e))?;

        let parsed: LegacyStoreFormat = serde_json::from_str(&raw)
            .map_err(|e| LegacyStoreError::Json(e))?;

        parsed
            .get(session_id)
            .and_then(|entry| entry.messages.as_ref())
            .map(|msgs| {
                msgs.iter()
                    .filter_map(|m| m.content.as_ref())
                    .map(|c| serde_json::Value::String(c.clone()))
                    .collect()
            })
            .ok_or_else(|| LegacyStoreError::SessionNotFound(session_id.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// The tauri-plugin-store format is typically `{ "session-id": { ... } }`.
type LegacyStoreFormat = std::collections::HashMap<String, LegacyStoreEntry>;

#[derive(Debug, Clone, Deserialize)]
struct LegacyStoreEntry {
    title: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    messages: Option<Vec<LegacyMessageEntry>>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyMessageEntry {
    content: Option<String>,
}

fn truncate_preview(s: &str, max_len: usize) -> String {
    let s = s.trim();
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated: String = s.chars().take(max_len).collect();
        truncated.push_str("…");
        truncated
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum LegacyStoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Session not found: {0}")]
    SessionNotFound(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_none() {
        let store = LegacySessionStore::open(Path::new("/nonexistent/terax-sessions.json"))
            .unwrap();
        assert!(store.is_none());
    }

    #[test]
    fn existing_json_parses_sessions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("terax-ai-sessions.json");

        let json = serde_json::json!({
            "abc-123": {
                "title": "My old session",
                "created_at": "2025-01-01T00:00:00Z",
                "updated_at": "2025-01-02T00:00:00Z",
                "messages": [
                    { "content": "Hello world" },
                    { "content": "How are you?" }
                ]
            },
            "def-456": {
                "title": "Another one",
                "created_at": "2025-02-01T00:00:00Z",
                "updated_at": null,
                "messages": [
                    { "content": "Just one message" }
                ]
            }
        });

        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        let store = LegacySessionStore::open(&path).unwrap().unwrap();
        let mut sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        sessions.sort_by(|a, b| a.id.cmp(&b.id));

        assert_eq!(sessions[0].id, "abc-123");
        assert_eq!(sessions[0].title.as_deref(), Some("My old session"));
        assert_eq!(sessions[0].message_count, 2);
        assert_eq!(sessions[0].last_preview.as_deref(), Some("How are you?"));

        assert_eq!(sessions[1].id, "def-456");
        assert_eq!(sessions[1].message_count, 1);
    }

    #[test]
    fn get_messages_for_session() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("terax-ai-sessions.json");

        let json = serde_json::json!({
            "abc-123": {
                "messages": [
                    { "content": "msg1" },
                    { "content": "msg2" }
                ]
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        let store = LegacySessionStore::open(&path).unwrap().unwrap();
        let msgs = store.get_messages("abc-123").unwrap();
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn preview_truncation() {
        assert_eq!(truncate_preview("hello", 120), "hello");
        let long = "a".repeat(200);
        let preview = truncate_preview(&long, 120);
        // 120 chars + '…' (3 bytes UTF-8) = 123 bytes
        assert_eq!(preview.len(), 123);
        assert!(preview.ends_with('…'));
    }
}
