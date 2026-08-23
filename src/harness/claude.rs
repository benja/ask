use std::ffi::OsString;
use std::process::Command;

use serde_json::Value;

use super::{Harness, Model, Response, RunOptions, executable_available};
use crate::error::{Error, Result};

pub struct Claude {
    program: OsString,
}

impl Claude {
    pub fn from_environment() -> Self {
        Self {
            program: std::env::var_os("ASK_CLAUDE_BIN").unwrap_or_else(|| "claude".into()),
        }
    }

    pub fn is_available() -> bool {
        let program = std::env::var_os("ASK_CLAUDE_BIN").unwrap_or_else(|| "claude".into());
        executable_available(&program)
    }

    fn command(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Command {
        let mut command = Command::new(&self.program);
        command.args([
            "--print",
            "--output-format",
            "json",
            "--permission-mode",
            "plan",
        ]);
        if let Some(session_id) = session_id {
            command.args(["--resume", session_id]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        if let Some(instructions) = options.instructions {
            command.args(["--append-system-prompt", instructions]);
        }
        command.arg(question);
        command
    }
}

impl Harness for Claude {
    fn models(&mut self) -> Result<Vec<Model>> {
        Ok([("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku")]
            .into_iter()
            .map(|(id, name)| Model {
                id: id.into(),
                name: name.into(),
                description: format!("Claude Code's latest {name} model"),
                is_default: false,
                reasoning: Vec::new(),
                default_reasoning: None,
            })
            .collect())
    }

    fn ask(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        _on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        if options.reasoning.is_some() {
            return Err(Error::new(
                "reasoning control is not supported for Claude Code",
                "choose Model default reasoning in 'ask --settings' and try again",
            ));
        }
        let output = self
            .command(question, session_id, &options)
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error::new(
                        "Claude Code is not installed or not on PATH",
                        "install it, authenticate, then try again",
                    )
                } else {
                    Error::agent("claude", format!("could not start Claude Code: {error}"))
                }
            })?;

        let stdout = String::from_utf8(output.stdout).map_err(|_| {
            Error::agent(
                "claude",
                "Claude Code returned output that was not valid UTF-8",
            )
        })?;
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
            let detail = stderr.trim();
            let message = if detail.is_empty() {
                format!("Claude Code exited with {}", output.status)
            } else {
                format!("Claude Code failed: {detail}")
            };
            return Err(Error::agent("claude", message));
        }

        Ok(response)
    }
}

fn parse_response(output: &str) -> Result<Response> {
    let response: Value = serde_json::from_str(output).map_err(|error| {
        Error::agent(
            "claude",
            format!("could not parse Claude Code response: {error}"),
        )
    })?;

    if response.get("is_error").and_then(Value::as_bool) == Some(true) {
        let detail = response
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(Error::agent(
            "claude",
            format!("Claude Code reported an error: {detail}"),
        ));
    }

    Ok(Response {
        answer: response
            .get("result")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::agent(
                    "claude",
                    "Claude Code completed without returning an answer",
                )
            })?
            .to_owned(),
        session_id: response
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::agent(
                    "claude",
                    "Claude Code completed without returning a session ID",
                )
            })?
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Claude, parse_response};
    use crate::harness::RunOptions;

    #[test]
    fn appends_ask_instructions() {
        let harness = Claude {
            program: "claude".into(),
        };
        let command = harness.command(
            "hello",
            None,
            &RunOptions {
                model: None,
                reasoning: None,
                instructions: Some("Be concise."),
            },
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            args.windows(2)
                .any(|args| args == ["--append-system-prompt", "Be concise."])
        );
    }

    #[test]
    fn extracts_answer_and_session() {
        let response = parse_response(
            r#"{"type":"result","is_error":false,"result":"hello","session_id":"abc"}"#,
        )
        .unwrap();

        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "abc");
    }

    #[test]
    fn surfaces_reported_error() {
        let error = parse_response(
            r#"{"type":"result","is_error":true,"result":"not authenticated","session_id":"abc"}"#,
        )
        .unwrap_err();

        assert_eq!(
            error.message(),
            "Claude Code reported an error: not authenticated"
        );
    }
}
