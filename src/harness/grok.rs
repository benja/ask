use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::{Harness, Model, ReasoningLevel, Response, RunOptions};
use crate::error::{Error, Result};

const REASONING_LEVELS: &[(&str, &str)] = &[
    ("minimal", "Least reasoning"),
    ("low", "Low reasoning"),
    ("medium", "Balanced reasoning"),
    ("high", "Deep reasoning"),
];

pub(super) struct Grok {
    program: OsString,
}

impl Grok {
    pub(super) fn new(program: OsString) -> Self {
        Self { program }
    }

    fn command(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Command {
        let mut command = Command::new(&self.program);
        command.args([
            "-p",
            question,
            "--output-format",
            "streaming-json",
            "--permission-mode",
            "plan",
            "--no-auto-update",
        ]);
        if let Some(session_id) = session_id {
            command.args(["--resume", session_id]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        if let Some(reasoning) = options.reasoning {
            command.args(["--reasoning-effort", reasoning]);
        }
        if let Some(instructions) = options.instructions {
            command.args(["--rules", instructions]);
        }
        command
    }
}

impl Harness for Grok {
    fn models(&mut self) -> Result<Vec<Model>> {
        let output = Command::new(&self.program)
            .arg("models")
            .output()
            .map_err(start_error)?;
        if !output.status.success() {
            return Err(Error::agent(
                "grok",
                format!(
                    "could not list Grok models: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let output = String::from_utf8(output.stdout).map_err(|_| {
            Error::agent(
                "grok",
                "Grok returned a model list that was not valid UTF-8",
            )
        })?;
        parse_models(&output)
    }

    fn ask(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        let mut child = self
            .command(question, session_id, &options)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(start_error)?;
        let stdout = child.stdout.take().expect("piped stdout is available");
        let mut stderr = child.stderr.take().expect("piped stderr is available");
        let stderr_reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = stderr.read_to_end(&mut output);
            output
        });

        let result = read_events(BufReader::new(stdout), on_delta);
        let status = child
            .wait()
            .map_err(|error| Error::agent("grok", format!("could not wait for Grok: {error}")))?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| Error::agent("grok", "could not read Grok error output"))?;

        if !status.success() && result.is_err() {
            return Err(failure(status, &stderr));
        }
        result
    }
}

fn start_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            "Grok is not installed or not on PATH",
            "install it with 'curl -fsSL https://x.ai/cli/install.sh | bash' and run 'grok login'",
        )
    } else {
        Error::agent("grok", format!("could not start Grok: {error}"))
    }
}

fn failure(status: std::process::ExitStatus, stderr: &[u8]) -> Error {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    if detail.is_empty() {
        Error::agent("grok", format!("Grok exited with {status}"))
    } else {
        Error::agent("grok", format!("Grok failed: {detail}"))
    }
}

fn parse_models(output: &str) -> Result<Vec<Model>> {
    let reasoning = REASONING_LEVELS
        .iter()
        .map(|(id, description)| ReasoningLevel {
            id: (*id).to_owned(),
            description: (*description).to_owned(),
        })
        .collect::<Vec<_>>();
    let mut models = Vec::new();
    let mut in_available = false;

    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("Available models") {
            in_available = true;
            continue;
        }
        if !in_available {
            continue;
        }
        let Some(entry) = line.strip_prefix('*').or_else(|| line.strip_prefix('-')) else {
            continue;
        };
        let entry = entry.trim();
        let Some(id) = entry.split_whitespace().next() else {
            continue;
        };
        models.push(Model {
            id: id.to_owned(),
            name: id.to_owned(),
            description: "xAI Grok".to_owned(),
            is_default: entry.contains("(default)"),
            reasoning: reasoning.clone(),
            default_reasoning: None,
        });
    }

    if models.is_empty() {
        Err(Error::agent(
            "grok",
            "Grok did not report any available models",
        ))
    } else {
        Ok(models)
    }
}

fn read_events(
    reader: impl BufRead,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Response> {
    let mut answer = String::new();
    let mut session_id = None;
    let mut reported_error = None;

    for line in reader.lines() {
        let line = line.map_err(|error| {
            Error::agent("grok", format!("could not read Grok response: {error}"))
        })?;
        let event: Value = serde_json::from_str(&line).map_err(|error| {
            Error::agent("grok", format!("could not parse Grok response: {error}"))
        })?;
        match event.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(delta) = event.get("data").and_then(Value::as_str) {
                    answer.push_str(delta);
                    on_delta(delta)?;
                }
            }
            Some("error") => {
                reported_error = event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("end") => {
                session_id = event
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                break;
            }
            _ => {}
        }
    }

    if let Some(error) = reported_error {
        return Err(Error::agent(
            "grok",
            format!("Grok reported an error: {error}"),
        ));
    }
    Ok(Response {
        answer: if answer.is_empty() {
            return Err(Error::agent(
                "grok",
                "Grok completed without returning an answer",
            ));
        } else {
            answer
        },
        session_id: session_id
            .ok_or_else(|| Error::agent("grok", "Grok completed without returning a session ID"))?,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{Grok, parse_models, read_events};
    use crate::harness::RunOptions;

    #[test]
    fn command_is_read_only_and_resumes_sessions() {
        let harness = Grok {
            program: "grok".into(),
        };
        let command = harness.command(
            "hello",
            Some("session-1"),
            &RunOptions {
                model: Some("grok-4.6"),
                reasoning: Some("high"),
                instructions: Some("Be concise."),
            },
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            args.windows(2)
                .any(|args| args == ["--permission-mode", "plan"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--resume", "session-1"])
        );
        assert!(args.windows(2).any(|args| args == ["--model", "grok-4.6"]));
        assert!(
            args.windows(2)
                .any(|args| args == ["--reasoning-effort", "high"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--rules", "Be concise."])
        );
    }

    #[test]
    fn parses_model_table() {
        let models = parse_models(concat!(
            "You are not authenticated.\n",
            "\n",
            "Default model: grok-4.6\n",
            "\n",
            "Available models:\n",
            "  * grok-4.6 (default)\n",
            "  - grok-4.5\n",
        ))
        .unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "grok-4.6");
        assert!(models[0].is_default);
        assert_eq!(models[0].reasoning.len(), 4);
        assert_eq!(models[1].id, "grok-4.5");
        assert!(!models[1].is_default);
    }

    #[test]
    fn streams_text_and_extracts_the_session() {
        let input = concat!(
            "{\"type\":\"text\",\"data\":\"hel\"}\n",
            "{\"type\":\"text\",\"data\":\"lo\"}\n",
            "{\"type\":\"usage\",\"messageId\":\"m-1\"}\n",
            "{\"type\":\"end\",\"stop_reason\":\"stop\",\"session_id\":\"session-1\",\"request_id\":\"r-1\"}\n"
        );
        let mut streamed = String::new();
        let response = read_events(Cursor::new(input), &mut |delta| {
            streamed.push_str(delta);
            Ok(())
        })
        .unwrap();

        assert_eq!(streamed, "hello");
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "session-1");
    }

    #[test]
    fn surfaces_reported_errors() {
        let input = concat!(
            "{\"type\":\"text\",\"data\":\"partial\"}\n",
            "{\"type\":\"error\",\"message\":\"rate limited\"}\n",
            "{\"type\":\"end\",\"stop_reason\":\"error\",\"session_id\":\"session-1\"}\n"
        );
        let error = read_events(Cursor::new(input), &mut |_| Ok(())).unwrap_err();
        assert_eq!(error.message(), "Grok reported an error: rate limited");
    }
}
