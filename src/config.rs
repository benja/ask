use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::instructions::Instructions;
use crate::{harness, storage};

pub struct Config {
    pub agent: String,
    instructions: Instructions,
    agents: BTreeMap<String, AgentSettings>,
}

struct AgentSettings {
    model: Option<String>,
    reasoning: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let agents = harness::DEFINITIONS
            .iter()
            .map(|definition| {
                (
                    definition.id.to_owned(),
                    AgentSettings {
                        model: definition.default_model.map(str::to_owned),
                        reasoning: definition.default_reasoning.map(str::to_owned),
                    },
                )
            })
            .collect();
        Self {
            agent: "codex".into(),
            instructions: Instructions::default(),
            agents,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let path = path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let bytes = fs::read(&path)
            .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("could not parse '{}': {error}", path.display()))?;

        Self::from_value(&value)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = path()?;
        let agents = self
            .agents
            .iter()
            .map(|(id, settings)| {
                (
                    id.clone(),
                    json!({
                        "model": settings.model,
                        "reasoning": settings.reasoning,
                    }),
                )
            })
            .collect::<Map<_, _>>();
        let bytes = serde_json::to_vec_pretty(&json!({
            "version": 2,
            "agent": self.agent,
            "instructions": self.instructions.to_json(),
            "agents": agents,
        }))
        .map_err(|error| format!("could not encode ask config: {error}"))?;
        storage::write_private(&path, &bytes, "ask config")
    }

    pub fn model(&self, agent: &str) -> Option<&str> {
        match self.agents.get(agent) {
            Some(settings) => settings.model.as_deref(),
            None => harness::find(agent)?.default_model,
        }
    }

    pub fn set_model(&mut self, agent: &str, model: Option<String>) {
        self.agent_settings_mut(agent).model = model;
    }

    pub fn reasoning(&self, agent: &str) -> Option<&str> {
        match self.agents.get(agent) {
            Some(settings) => settings.reasoning.as_deref(),
            None => harness::find(agent)?.default_reasoning,
        }
    }

    pub fn set_reasoning(&mut self, agent: &str, reasoning: Option<String>) {
        self.agent_settings_mut(agent).reasoning = reasoning;
    }

    pub fn instructions(&self) -> &Instructions {
        &self.instructions
    }

    pub fn set_instructions(&mut self, instructions: Instructions) {
        self.instructions = instructions;
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let raw_agent = value["agent"]
            .as_str()
            .ok_or_else(|| "ask config is missing 'agent'".to_string())?;
        let agent = harness::find(raw_agent)
            .map_or(raw_agent, |definition| definition.id)
            .to_owned();

        let version = value.get("version").and_then(Value::as_u64).unwrap_or(1);
        if version > 2 {
            return Err(format!(
                "config version {version} requires a newer version of ask; run 'ask --upgrade'"
            ));
        }
        if version == 0 {
            return Err("config version 0 is not supported".into());
        }

        let values = value
            .get("agents")
            .and_then(Value::as_object)
            .ok_or_else(|| "ask config is missing 'agents'".to_string())?;
        let mut config = Self {
            agent,
            instructions: Instructions::from_json(
                value.get("instructions"),
                version,
                "instructions",
            )?,
            ..Self::default()
        };
        config.load_agent_settings(values)?;
        Ok(config)
    }

    fn load_agent_settings(&mut self, values: &Map<String, Value>) -> Result<(), String> {
        for (raw_id, value) in values {
            let id = harness::find(raw_id)
                .map_or(raw_id.as_str(), |definition| definition.id)
                .to_owned();
            if !value.is_object() {
                return Err(format!("ask config has invalid 'agents.{raw_id}'"));
            }
            self.agents.insert(
                id,
                AgentSettings {
                    model: optional_string(value, "model")?,
                    reasoning: optional_string(value, "reasoning")?,
                },
            );
        }
        Ok(())
    }

    fn agent_settings_mut(&mut self, agent: &str) -> &mut AgentSettings {
        self.agents.entry(agent.to_owned()).or_insert_with(|| {
            let definition = harness::find(agent);
            AgentSettings {
                model: definition
                    .and_then(|definition| definition.default_model.map(str::to_owned)),
                reasoning: definition
                    .and_then(|definition| definition.default_reasoning.map(str::to_owned)),
            }
        })
    }
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("ask config has invalid '{key}'")),
    }
}

fn path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path).join("ask/config.json"));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| "HOME is not set; set XDG_CONFIG_HOME to store settings".to_string())?;
    Ok(PathBuf::from(home).join(".config/ask/config.json"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Config;
    use crate::instructions::Instructions;

    #[test]
    fn fast_codex_defaults_are_explicit() {
        let config = Config::default();
        assert_eq!(config.agent, "codex");
        assert_eq!(config.model("codex"), Some("fast"));
        assert_eq!(config.reasoning("codex"), Some("low"));
        assert_eq!(config.model("claude"), None);
        assert_eq!(config.instructions(), &Instructions::Concise);
    }

    #[test]
    fn v1_preserves_agent_settings_and_resets_instruction_strings() {
        let config = Config::from_value(&json!({
            "version": 1,
            "agent": "pi",
            "instructions": "Use examples.",
            "agents": {
                "codex": {"model": "gpt-test", "reasoning": "high"},
                "pi": {"model": "fast", "reasoning": null}
            }
        }))
        .unwrap();

        assert_eq!(config.agent, "pi");
        assert_eq!(config.model("codex"), Some("gpt-test"));
        assert_eq!(config.reasoning("codex"), Some("high"));
        assert_eq!(config.model("pi"), Some("fast"));
        assert_eq!(config.reasoning("pi"), None);
        assert_eq!(config.instructions(), &Instructions::Concise);
    }

    #[test]
    fn v1_can_disable_instructions() {
        let disabled = Config::from_value(&json!({
            "version": 1,
            "agent": "codex",
            "instructions": null,
            "agents": {}
        }))
        .unwrap();

        assert_eq!(disabled.instructions(), &Instructions::AgentDefault);
    }

    #[test]
    fn v2_uses_explicit_instruction_modes() {
        let previous_default = Config::from_value(&json!({
            "version": 2,
            "agent": "codex",
            "instructions": "concise",
            "agents": {}
        }))
        .unwrap();
        let custom = Config::from_value(&json!({
            "version": 2,
            "agent": "codex",
            "instructions": { "custom": "Answer like a pirate." },
            "agents": {}
        }))
        .unwrap();

        assert_eq!(previous_default.instructions(), &Instructions::Concise);
        assert_eq!(
            custom.instructions(),
            &Instructions::Custom("Answer like a pirate.".into())
        );
    }
}
