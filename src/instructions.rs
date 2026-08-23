use serde_json::{Value, json};

use crate::error::{Error, Result};

pub const CONCISE_INSTRUCTIONS: &str = concat!(
    "Be friendly, direct, conversational, and concise. Lead with the answer and stop once it is clear. ",
    "Use recent context to interpret typos and corrections. ",
    "Skip canned reactions, restatements, repeated conclusions, and unsolicited offers to continue. ",
    "Treat brief acknowledgments as closure. Keep simple definitions and follow-ups to one short paragraph unless asked for more. ",
    "Do not use em dashes or emoji. Plain ASCII emoticons such as :) are fine."
);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Instructions {
    #[default]
    Concise,
    AgentDefault,
    Custom(String),
}

impl Instructions {
    pub fn prompt(&self) -> Option<&str> {
        match self {
            Self::Concise => Some(CONCISE_INSTRUCTIONS),
            Self::AgentDefault => None,
            Self::Custom(instructions) => Some(instructions),
        }
    }

    pub fn custom(&self) -> Option<&str> {
        match self {
            Self::Custom(instructions) => Some(instructions),
            Self::Concise | Self::AgentDefault => None,
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Concise => json!("concise"),
            Self::AgentDefault => json!("agent_default"),
            Self::Custom(instructions) => json!({ "custom": instructions }),
        }
    }

    pub fn from_json(value: Option<&Value>, version: u64, field: &str) -> Result<Self> {
        match version {
            1 => Self::from_v1_json(value, field),
            2 => Self::from_v2_json(value, field),
            _ => Err(Error::new(
                format!("instruction version {version} is not supported"),
                "run 'ask --upgrade'",
            )),
        }
    }

    fn from_v1_json(value: Option<&Value>, field: &str) -> Result<Self> {
        match value {
            None | Some(Value::Null) => Ok(Self::AgentDefault),
            Some(Value::String(_)) => Ok(Self::Concise),
            Some(_) => Err(invalid_instructions(field)),
        }
    }

    fn from_v2_json(value: Option<&Value>, field: &str) -> Result<Self> {
        match value {
            None => Ok(Self::Concise),
            Some(Value::Null) => Ok(Self::AgentDefault),
            Some(Value::String(value)) if value == "concise" => Ok(Self::Concise),
            Some(Value::String(value)) if value == "agent_default" => Ok(Self::AgentDefault),
            Some(Value::String(_)) => Ok(Self::Concise),
            Some(Value::Object(value))
                if value.len() == 1 && value.get("custom").is_some_and(Value::is_string) =>
            {
                Ok(Self::Custom(
                    value["custom"].as_str().unwrap_or_default().to_owned(),
                ))
            }
            Some(_) => Err(invalid_instructions(field)),
        }
    }
}

fn invalid_instructions(field: &str) -> Error {
    Error::new(
        format!("config has invalid '{field}'"),
        "fix or remove the config file, then try again",
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Instructions;

    #[test]
    fn instruction_modes_have_explicit_v2_values() {
        assert_eq!(Instructions::Concise.to_json(), json!("concise"));
        assert_eq!(Instructions::AgentDefault.to_json(), json!("agent_default"));
        assert_eq!(
            Instructions::Custom("Use examples.\nKeep them short.".into()).to_json(),
            json!({ "custom": "Use examples.\nKeep them short." })
        );
    }

    #[test]
    fn unknown_v2_presets_fall_back_to_concise() {
        assert_eq!(
            Instructions::from_json(Some(&json!("removed_preset")), 2, "instructions").unwrap(),
            Instructions::Concise
        );
    }
}
