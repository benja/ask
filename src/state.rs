use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::storage;

#[derive(Debug, Eq, PartialEq)]
pub struct Session {
    pub agent: String,
    pub harness_session_id: String,
    pub cwd: String,
    pub updated_at: u64,
    pub settings: Option<SessionSettings>,
    pub turns: Vec<Turn>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSettings {
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub instructions: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Turn {
    pub user: String,
    pub assistant: String,
}

impl Session {
    pub fn new(agent: &str, harness_session_id: String, cwd: &Path) -> Self {
        Self {
            agent: agent.to_owned(),
            harness_session_id,
            cwd: cwd.to_string_lossy().into_owned(),
            updated_at: now(),
            settings: None,
            turns: Vec::new(),
        }
    }

    pub fn add_turn(&mut self, user: &str, assistant: String) {
        self.turns.push(Turn {
            user: user.to_owned(),
            assistant,
        });
        self.updated_at = now();
    }

    fn to_json(&self) -> Value {
        json!({
            "version": 1,
            "agent": self.agent,
            "harness_session_id": self.harness_session_id,
            "cwd": self.cwd,
            "updated_at": self.updated_at,
            "settings": self.settings.as_ref().map(|settings| json!({
                "model": settings.model,
                "reasoning": settings.reasoning,
                "instructions": settings.instructions,
            })),
            "turns": self.turns.iter().map(|turn| json!({
                "user": turn.user,
                "assistant": turn.assistant,
            })).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Result<Self, String> {
        let string = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("missing or invalid '{key}'"))
        };
        let turns = value["turns"]
            .as_array()
            .ok_or_else(|| "missing or invalid 'turns'".to_string())?
            .iter()
            .map(|turn| {
                Ok(Turn {
                    user: turn["user"]
                        .as_str()
                        .ok_or_else(|| "turn is missing 'user'".to_string())?
                        .to_owned(),
                    assistant: turn["assistant"]
                        .as_str()
                        .ok_or_else(|| "turn is missing 'assistant'".to_string())?
                        .to_owned(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let harness_session_id = value["harness_session_id"]
            .as_str()
            .or_else(|| value["harness_id"].as_str())
            .ok_or_else(|| "missing or invalid 'harness_session_id'".to_string())?
            .to_owned();
        let settings = value
            .get("settings")
            .filter(|settings| !settings.is_null())
            .map(|settings| {
                if !settings.is_object() {
                    return Err("missing or invalid 'settings'".to_string());
                }
                Ok(SessionSettings {
                    model: optional_string(settings, "model")?,
                    reasoning: optional_string(settings, "reasoning")?,
                    instructions: optional_string(settings, "instructions")?,
                })
            })
            .transpose()?;

        Ok(Self {
            agent: string("agent")?,
            harness_session_id,
            cwd: string("cwd")?,
            updated_at: value["updated_at"]
                .as_u64()
                .ok_or_else(|| "missing or invalid 'updated_at'".to_string())?,
            settings,
            turns,
        })
    }
}

pub fn save(session: &Session) -> Result<(), String> {
    let directory = directory()?;
    let destination = directory.join(format!("{}.json", file_key(&session.harness_session_id)));
    let bytes = serde_json::to_vec_pretty(&session.to_json())
        .map_err(|error| format!("could not encode session: {error}"))?;
    storage::write_private(&destination, &bytes, "session")
}

pub fn latest(cwd: &Path) -> Result<Session, String> {
    let cwd = cwd.to_string_lossy();
    load_all()?
        .into_iter()
        .filter(|session| session.cwd == cwd)
        .max_by_key(|session| session.updated_at)
        .ok_or_else(|| "no saved sessions for this folder".to_string())
}

pub fn load_all() -> Result<Vec<Session>, String> {
    let directory = directory()?;
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("could not read session directory: {error}"))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("could not read session entry: {error}"))?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path)
            .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("could not parse '{}': {error}", path.display()))?;
        sessions.push(
            Session::from_json(&value)
                .map_err(|error| format!("invalid session '{}': {error}", path.display()))?,
        );
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
}

pub fn directory() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join("ask/sessions"));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "HOME is not set; set XDG_STATE_HOME to store sessions".to_string())?;
    Ok(PathBuf::from(home).join(".local/state/ask/sessions"))
}

fn file_key(harness_session_id: &str) -> String {
    harness_session_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("missing or invalid 'settings.{key}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, SessionSettings, Turn};

    #[test]
    fn session_round_trips_through_json() {
        let session = Session {
            agent: "codex".into(),
            harness_session_id: "full-id".into(),
            cwd: "/tmp/project".into(),
            updated_at: 42,
            settings: Some(SessionSettings {
                model: Some("gpt-test".into()),
                reasoning: Some("high".into()),
                instructions: None,
            }),
            turns: vec![Turn {
                user: "hello".into(),
                assistant: "hi".into(),
            }],
        };

        assert_eq!(Session::from_json(&session.to_json()).unwrap(), session);
    }

    #[test]
    fn reads_the_unreleased_harness_id_name() {
        let session = Session::from_json(&serde_json::json!({
            "version": 1,
            "agent": "codex",
            "harness_id": "legacy-id",
            "cwd": "/tmp/project",
            "updated_at": 42,
            "turns": []
        }))
        .unwrap();

        assert_eq!(session.harness_session_id, "legacy-id");
        assert_eq!(session.settings, None);
    }
}
