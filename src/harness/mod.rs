pub mod claude;
pub mod codex;
pub mod opencode;
pub mod pi;

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct Response {
    pub answer: String,
    pub session_id: String,
}

#[derive(Clone, Copy)]
pub struct RunOptions<'a> {
    pub model: Option<&'a str>,
    pub reasoning: Option<&'a str>,
    pub instructions: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_default: bool,
    pub reasoning: Vec<ReasoningLevel>,
    pub default_reasoning: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningLevel {
    pub id: String,
    pub description: String,
}

pub trait Harness {
    fn models(&mut self) -> Result<Vec<Model>>;

    fn ask(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response>;
}

#[derive(Clone, Copy)]
pub enum ReasoningControl {
    Selectable,
    Managed {
        label: &'static str,
        explanation: &'static str,
    },
}

pub struct Definition {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub name: &'static str,
    pub description: &'static str,
    pub default_model: Option<&'static str>,
    pub default_reasoning: Option<&'static str>,
    pub reasoning: ReasoningControl,
    available: fn() -> bool,
    create: fn() -> Box<dyn Harness>,
}

impl Definition {
    pub fn is_available(&self) -> bool {
        (self.available)()
    }

    pub fn create(&self) -> Box<dyn Harness> {
        (self.create)()
    }
}

pub static DEFINITIONS: &[Definition] = &[
    Definition {
        id: "codex",
        aliases: &[],
        name: "Codex",
        description: "OpenAI Codex",
        default_model: Some("fast"),
        default_reasoning: Some("low"),
        reasoning: ReasoningControl::Selectable,
        available: codex::Codex::is_available,
        create: || Box::new(codex::Codex::from_environment()),
    },
    Definition {
        id: "claude",
        aliases: &["claude-code"],
        name: "Claude Code",
        description: "Anthropic Claude Code",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Managed {
            label: "Managed by Claude",
            explanation: "Claude Code manages reasoning automatically for its selected model.",
        },
        available: claude::Claude::is_available,
        create: || Box::new(claude::Claude::from_environment()),
    },
    Definition {
        id: "opencode",
        aliases: &["open-code"],
        name: "OpenCode",
        description: "OpenCode coding agent",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Managed {
            label: "Managed by OpenCode",
            explanation: "OpenCode manages reasoning through model-specific variants.",
        },
        available: opencode::OpenCode::is_available,
        create: || Box::new(opencode::OpenCode::from_environment()),
    },
    Definition {
        id: "pi",
        aliases: &[],
        name: "Pi",
        description: "Pi coding agent",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Selectable,
        available: pi::Pi::is_available,
        create: || Box::new(pi::Pi::from_environment()),
    },
];

pub fn find(name: &str) -> Option<&'static Definition> {
    DEFINITIONS
        .iter()
        .find(|definition| definition.id == name || definition.aliases.contains(&name))
}

pub fn agent_name(agent: &str) -> &str {
    find(agent).map_or(agent, |definition| definition.name)
}

pub fn resolve(name: &str) -> Result<&'static Definition> {
    find(name).ok_or_else(|| {
        let available = DEFINITIONS
            .iter()
            .map(|definition| definition.id)
            .collect::<Vec<_>>()
            .join(", ");
        Error::new(
            format!("unknown agent '{name}' (available: {available})"),
            "run 'ask --settings' to choose an installed agent",
        )
    })
}

pub fn create(name: &str) -> Result<Box<dyn Harness>> {
    Ok(resolve(name)?.create())
}

fn executable_available(program: &OsStr) -> bool {
    let program = Path::new(program);
    if program.components().count() > 1 {
        return is_executable(program);
    }

    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|directory| is_executable(&directory.join(program)))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{executable_available, resolve};

    #[test]
    fn detects_absolute_executables_and_missing_paths() {
        let current = std::env::current_exe().unwrap();

        assert!(executable_available(current.as_os_str()));
        assert!(!executable_available(OsStr::new(
            "/definitely/not/an/ask-agent"
        )));
    }

    #[test]
    fn registry_resolves_ids_and_aliases() {
        assert_eq!(resolve("codex").unwrap().id, "codex");
        assert_eq!(resolve("claude-code").unwrap().id, "claude");
        assert_eq!(resolve("open-code").unwrap().id, "opencode");
        assert!(resolve("missing").is_err());
    }
}
