use std::ffi::OsString;
use std::process::Command;

use serde_json::Value;

use super::{Harness, Model, Response, RunOptions};
use crate::error::{Error, Result};

pub(super) struct Fx {
    program: OsString,
}

impl Fx {
    pub(super) fn new(program: OsString) -> Self {
        Self { program }
    }

    fn command(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Result<Command> {
        if options.reasoning.is_some() {
            return Err(Error::new(
                "reasoning control is not supported for fx",
                "choose fx-managed reasoning in 'ask --settings' and try again",
            ));
        }
        let mut command = Command::new(&self.program);
        command
            .args(["ask", "--json"])
            .env("FX_PERMISSION_MODE", "ask");
        if let Some(session_id) = session_id {
            command.args(["--resume-id", session_id]);
        }
        if let Some(model) = options.model {
            command.env("FX_MODEL", model);
        }
        if let Some(instructions) = options.instructions {
            command.args(["--system", instructions]);
        }
        command.arg(question);
        Ok(command)
    }
}

impl Harness for Fx {
    fn models(&mut self) -> Result<Vec<Model>> {
        let output = Command::new(&self.program)
            .args(["models", "--json"])
            .output()
            .map_err(start_error)?;
        if !output.status.success() {
            return Err(command_error("could not list fx models", &output.stderr));
        }
        let output = String::from_utf8(output.stdout)
            .map_err(|_| Error::agent("fx", "fx returned a model list that was not valid UTF-8"))?;
        parse_models(&output)
    }

    fn ask(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        _on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        let output = self
            .command(question, session_id, &options)?
            .output()
            .map_err(start_error)?;
        let stdout = String::from_utf8(output.stdout)
            .map_err(|_| Error::agent("fx", "fx returned output that was not valid UTF-8"))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let response = parse_response(&stdout).map_err(|error| {
            let detail = stderr.trim();
            if detail.is_empty() {
                error
            } else {
                error.detail(detail)
            }
        })?;

        if !output.status.success() {
            return Err(command_error("fx failed", &output.stderr));
        }
        Ok(response)
    }
}

fn parse_models(output: &str) -> Result<Vec<Model>> {
    let response: Value = serde_json::from_str(output)
        .map_err(|error| Error::agent("fx", format!("could not parse fx model list: {error}")))?;
    let ids = response
        .get("ids")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::agent("fx", "fx returned an invalid model list"))?;
    let details = response.get("models").and_then(Value::as_array);
    let models = ids
        .iter()
        .map(|value| {
            let id = value
                .as_str()
                .ok_or_else(|| Error::agent("fx", "fx returned an invalid model ID"))?;
            let (provider, name) = id
                .split_once('/')
                .map_or((None, id), |(provider, name)| (Some(provider), name));
            let source = details
                .into_iter()
                .flatten()
                .find(|model| model.get("id").and_then(Value::as_str) == Some(id))
                .and_then(|model| model.get("source"))
                .and_then(Value::as_str);
            Ok(Model {
                id: id.to_owned(),
                name: name.to_owned(),
                description: source.or(provider).map_or_else(
                    || "fx model".to_owned(),
                    |source| format!("{source} provider"),
                ),
                is_default: false,
                reasoning: Vec::new(),
                default_reasoning: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if models.is_empty() {
        Err(Error::agent("fx", "fx did not report any available models"))
    } else {
        Ok(models)
    }
}

fn parse_response(output: &str) -> Result<Response> {
    let response: Value = serde_json::from_str(output)
        .map_err(|error| Error::agent("fx", format!("could not parse fx response: {error}")))?;
    if response.get("exit_code").and_then(Value::as_i64) != Some(0) {
        let detail = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(Error::agent(
            "fx",
            format!("fx reported an error: {detail}"),
        ));
    }
    Ok(Response {
        answer: response
            .get("output")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::agent("fx", "fx completed without returning an answer"))?
            .to_owned(),
        session_id: response
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.is_empty())
            .ok_or_else(|| Error::agent("fx", "fx completed without returning a session ID"))?
            .to_owned(),
    })
}

fn start_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            "fx is not installed or not on PATH",
            "install it, authenticate, then try again",
        )
    } else {
        Error::agent("fx", format!("could not start fx: {error}"))
    }
}

fn command_error(message: &str, stderr: &[u8]) -> Error {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    let message = if detail.is_empty() {
        message.to_owned()
    } else {
        format!("{message}: {detail}")
    };
    Error::agent("fx", message)
}

#[cfg(test)]
mod tests {
    use super::{Fx, parse_models, parse_response};
    use crate::harness::RunOptions;

    #[test]
    fn builds_a_read_only_session_command() {
        let harness = Fx {
            program: "fx".into(),
        };
        let command = harness
            .command(
                "hello",
                Some("session-1"),
                &RunOptions {
                    model: Some("openai/gpt-test"),
                    reasoning: None,
                    instructions: Some("Be concise."),
                },
            )
            .unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();
        let permission_mode = command
            .get_envs()
            .find(|(name, _)| *name == "FX_PERMISSION_MODE")
            .and_then(|(_, value)| value)
            .unwrap();
        let model = command
            .get_envs()
            .find(|(name, _)| *name == "FX_MODEL")
            .and_then(|(_, value)| value)
            .unwrap();

        assert!(args.starts_with(&["ask".into(), "--json".into()]));
        assert!(
            args.windows(2)
                .any(|args| args == ["--resume-id", "session-1"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--system", "Be concise."])
        );
        assert_eq!(permission_mode, "ask");
        assert_eq!(model, "openai/gpt-test");
    }

    #[test]
    fn rejects_reasoning_control() {
        let harness = Fx {
            program: "fx".into(),
        };
        let error = harness
            .command(
                "hello",
                None,
                &RunOptions {
                    model: None,
                    reasoning: Some("high"),
                    instructions: None,
                },
            )
            .unwrap_err();

        assert_eq!(error.message(), "reasoning control is not supported for fx");
    }

    #[test]
    fn parses_model_ids() {
        let models = parse_models(
            r#"{"kind":"models","count":2,"shown_count":2,"more_count":0,"private_models_hidden":false,"ids":["openai/gpt-test","anthropic/claude-test"]}"#,
        )
        .unwrap();

        assert_eq!(models[0].id, "openai/gpt-test");
        assert_eq!(models[0].name, "gpt-test");
        assert_eq!(models[0].description, "openai provider");
        assert_eq!(models[1].id, "anthropic/claude-test");
    }

    #[test]
    fn parses_subscription_model_ids() {
        let models = parse_models(
            r#"{"kind":"models","count":1,"shown_count":1,"more_count":0,"private_models_hidden":false,"ids":["gpt-5.4"],"models":[{"id":"gpt-5.4","source":"Codex"}]}"#,
        )
        .unwrap();

        assert_eq!(models[0].id, "gpt-5.4");
        assert_eq!(models[0].name, "gpt-5.4");
        assert_eq!(models[0].description, "Codex provider");
    }

    #[test]
    fn extracts_answer_and_session() {
        let response = parse_response(
            r#"{"output":"hello","exit_code":0,"model":"openai/gpt-test","session_id":"session-1","steps":1,"tool_calls":[]}"#,
        )
        .unwrap();

        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "session-1");
    }

    #[test]
    fn surfaces_reported_error() {
        let error = parse_response(
            r#"{"output":"","exit_code":1,"model":"","session_id":"","steps":0,"tool_calls":[],"error":"MissingCredentials"}"#,
        )
        .unwrap_err();

        assert_eq!(error.message(), "fx reported an error: MissingCredentials");
    }
}
