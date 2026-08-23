use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::error::{Error, Result};
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
            "version": 2,
            "agent": self.agent,
            "harness_session_id": self.harness_session_id,
            "cwd": self.cwd,
            "updated_at": self.updated_at,
            "settings": self.settings.as_ref().map(|settings| json!({
                "model": settings.model,
                "reasoning": settings.reasoning,
            })),
            "turns": self.turns.iter().map(|turn| json!({
                "user": turn.user,
                "assistant": turn.assistant,
            })).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Result<Self> {
        let version = value.get("version").and_then(Value::as_u64).unwrap_or(1);
        if version > 2 {
            return Err(Error::new(
                format!("session version {version} requires a newer version of ask"),
                "run 'ask --upgrade'",
            ));
        }
        if version == 0 {
            return Err(invalid_session("session version 0 is not supported"));
        }
        let string = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid_session(format!("missing or invalid '{key}'")))
        };
        let turns = value["turns"]
            .as_array()
            .ok_or_else(|| invalid_session("missing or invalid 'turns'"))?
            .iter()
            .map(|turn| {
                Ok(Turn {
                    user: turn["user"]
                        .as_str()
                        .ok_or_else(|| invalid_session("turn is missing 'user'"))?
                        .to_owned(),
                    assistant: turn["assistant"]
                        .as_str()
                        .ok_or_else(|| invalid_session("turn is missing 'assistant'"))?
                        .to_owned(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let harness_session_id = value["harness_session_id"]
            .as_str()
            .ok_or_else(|| invalid_session("missing or invalid 'harness_session_id'"))?
            .to_owned();
        let settings = value
            .get("settings")
            .filter(|settings| !settings.is_null())
            .map(|settings| {
                if !settings.is_object() {
                    return Err(invalid_session("missing or invalid 'settings'"));
                }
                Ok(SessionSettings {
                    model: optional_string(settings, "model")?,
                    reasoning: optional_string(settings, "reasoning")?,
                })
            })
            .transpose()?;

        Ok(Self {
            agent: string("agent")?,
            harness_session_id,
            cwd: string("cwd")?,
            updated_at: value["updated_at"]
                .as_u64()
                .ok_or_else(|| invalid_session("missing or invalid 'updated_at'"))?,
            settings,
            turns,
        })
    }
}

pub fn save(session: &Session) -> Result<()> {
    let directory = directory()?;
    let destination = directory.join(format!("{}.json", file_key(&session.harness_session_id)));
    let bytes = serde_json::to_vec_pretty(&session.to_json())
        .map_err(|error| Error::internal(format!("could not encode session: {error}")))?;
    storage::write_private(&destination, &bytes, "session")
}

pub fn delete(session: &Session) -> Result<()> {
    delete_from(&directory()?, session)
}

fn delete_from(directory: &Path, session: &Session) -> Result<()> {
    let path = directory.join(format!("{}.json", file_key(&session.harness_session_id)));
    fs::remove_file(&path).map_err(|error| {
        Error::new(
            format!("could not delete session '{}': {error}", path.display()),
            "check its permissions and try again",
        )
    })
}

pub fn latest(cwd: &Path) -> Result<Session> {
    let cwd = cwd.to_string_lossy();
    load_all()?
        .into_iter()
        .filter(|session| session.cwd == cwd)
        .max_by_key(|session| session.updated_at)
        .ok_or_else(|| {
            Error::new(
                "no saved sessions for this folder",
                "start one by running 'ask'",
            )
        })
}

pub fn load_all() -> Result<Vec<Session>> {
    let directory = directory()?;
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let entries = fs::read_dir(&directory).map_err(|error| {
        Error::new(
            format!(
                "could not read session directory '{}': {error}",
                directory.display()
            ),
            "check its permissions and try again",
        )
    })?;
    for entry in entries {
        let path = entry
            .map_err(|error| {
                Error::new(
                    format!("could not read a session entry: {error}"),
                    "check the session directory permissions and try again",
                )
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| {
            Error::new(
                format!("could not read '{}': {error}", path.display()),
                "check its permissions and try again",
            )
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            Error::new(
                format!("could not parse '{}': {error}", path.display()),
                "remove this file and try again",
            )
        })?;
        sessions.push(
            Session::from_json(&value)
                .map_err(|error| error.context(format!("invalid session '{}'", path.display())))?,
        );
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
}

pub fn directory() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join("ask/sessions"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        Error::new(
            "HOME is not set",
            "set XDG_STATE_HOME to a writable directory and try again",
        )
    })?;
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

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_session(format!(
            "missing or invalid 'settings.{key}'"
        ))),
    }
}

fn invalid_session(message: impl Into<String>) -> Error {
    Error::new(message, "remove the invalid session file and try again")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{Session, SessionSettings, Turn, delete_from, file_key};

    #[test]
    fn deleting_a_session_removes_only_its_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ask-delete-test-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();

        let deleted = Session::new("codex", "delete-me".into(), &directory);
        let kept = Session::new("codex", "keep-me".into(), &directory);
        let deleted_path = directory.join(format!("{}.json", file_key("delete-me")));
        let kept_path = directory.join(format!("{}.json", file_key(&kept.harness_session_id)));
        fs::write(&deleted_path, b"deleted").unwrap();
        fs::write(&kept_path, b"kept").unwrap();

        delete_from(&directory, &deleted).unwrap();

        assert!(!deleted_path.exists());
        assert!(kept_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

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
            }),
            turns: vec![Turn {
                user: "hello".into(),
                assistant: "hi".into(),
            }],
        };

        assert_eq!(Session::from_json(&session.to_json()).unwrap(), session);
    }
}
