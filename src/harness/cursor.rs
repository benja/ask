use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::{Harness, Model, Response, RunOptions};
use crate::error::{Error, Result};

pub(super) struct Cursor {
    program: OsString,
}

impl Cursor {
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
            "--mode",
            "ask",
            "--trust",
            "--output-format",
            "stream-json",
            "--stream-partial-output",
        ]);
        if let Some(session_id) = session_id {
            command.args(["--resume", session_id]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        let prompt = match options.instructions {
            Some(instructions) if !instructions.is_empty() => {
                format!("{question}\n\n{instructions}")
            }
            _ => question.to_owned(),
        };
        command.arg(prompt);
        command
    }
}

impl Harness for Cursor {
    fn models(&mut self) -> Result<Vec<Model>> {
        let output = Command::new(&self.program)
            .arg("models")
            .output()
            .map_err(start_error)?;
        if !output.status.success() {
            return Err(Error::agent(
                "cursor",
                format!(
                    "could not list Cursor models: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let output = String::from_utf8(output.stdout).map_err(|_| {
            Error::agent(
                "cursor",
                "Cursor returned a model list that was not valid UTF-8",
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
        let status = child.wait().map_err(|error| {
            Error::agent("cursor", format!("could not wait for Cursor: {error}"))
        })?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| Error::agent("cursor", "could not read Cursor error output"))?;

        if !status.success() && result.is_err() {
            return Err(failure(status, &stderr));
        }
        result
    }
}

fn start_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            "Cursor is not installed or not on PATH",
            "install it with 'curl https://cursor.com/install -fsS | bash' and run 'agent login'",
        )
    } else {
        Error::agent("cursor", format!("could not start Cursor: {error}"))
    }
}

fn failure(status: std::process::ExitStatus, stderr: &[u8]) -> Error {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    if detail.is_empty() {
        Error::agent("cursor", format!("Cursor exited with {status}"))
    } else {
        Error::agent("cursor", format!("Cursor failed: {detail}"))
    }
}

fn parse_models(output: &str) -> Result<Vec<Model>> {
    let mut models = Vec::new();
    for line in output.lines() {
        let mut parts = line.splitn(2, " - ");
        let Some(id) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        let name = name.trim();
        models.push(Model {
            id: id.to_owned(),
            name: name.replace(" (default)", "").to_owned(),
            description: String::new(),
            is_default: name.contains("(default)"),
            reasoning: Vec::new(),
            default_reasoning: None,
        });
    }

    if models.is_empty() {
        Err(Error::agent(
            "cursor",
            "Cursor did not report any available models",
        ))
    } else {
        Ok(models)
    }
}

fn read_events(
    reader: impl BufRead,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Response> {
    let mut streamed_answer = String::new();
    let mut final_answer = None;
    let mut session_id = None;
    let mut reported_error = None;

    for line in reader.lines() {
        let line = line.map_err(|error| {
            Error::agent("cursor", format!("could not read Cursor response: {error}"))
        })?;
        let event: Value = serde_json::from_str(&line).map_err(|error| {
            Error::agent(
                "cursor",
                format!("could not parse Cursor response: {error}"),
            )
        })?;
        match event.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let text = message_text(&event);
                if event.get("timestamp_ms").is_some() {
                    if let Some(text) = &text {
                        streamed_answer.push_str(text);
                        on_delta(text)?;
                    }
                } else if let Some(text) = text {
                    final_answer = Some(text);
                }
            }
            Some("result") => {
                session_id = event
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if event.get("is_error").and_then(Value::as_bool) == Some(true) {
                    reported_error = Some(
                        event
                            .get("result")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_owned(),
                    );
                } else if final_answer.is_none() {
                    final_answer = event
                        .get("result")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                break;
            }
            _ => {}
        }
    }

    if let Some(error) = reported_error {
        return Err(Error::agent(
            "cursor",
            format!("Cursor reported an error: {error}"),
        ));
    }
    Ok(Response {
        answer: final_answer.unwrap_or(streamed_answer),
        session_id: session_id.ok_or_else(|| {
            Error::agent("cursor", "Cursor completed without returning a session ID")
        })?,
    })
}

fn message_text(event: &Value) -> Option<String> {
    let content = event["message"]["content"].as_array()?;
    let text = content
        .iter()
        .filter(|block| block["type"].as_str() == Some("text"))
        .filter_map(|block| block["text"].as_str())
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor as IoCursor;

    use super::{Cursor, parse_models, read_events};
    use crate::harness::RunOptions;

    #[test]
    fn command_is_ask_mode_and_appends_instructions() {
        let harness = Cursor {
            program: "cursor-agent".into(),
        };
        let command = harness.command(
            "why is this failing",
            None,
            &RunOptions {
                model: Some("cursor-grok-4.6-high-fast"),
                reasoning: None,
                instructions: Some("Be concise."),
            },
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(args.windows(2).any(|args| args == ["--mode", "ask"]));
        assert!(
            args.windows(2)
                .any(|args| args == ["--output-format", "stream-json"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--model", "cursor-grok-4.6-high-fast"])
        );
        assert_eq!(
            args.last().map(|arg| arg.as_ref()),
            Some("why is this failing\n\nBe concise.")
        );
    }

    #[test]
    fn command_resumes_with_saved_session() {
        let harness = Cursor {
            program: "cursor-agent".into(),
        };
        let command = harness.command(
            "hello",
            Some("session-1"),
            &RunOptions {
                model: None,
                reasoning: None,
                instructions: None,
            },
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            args.windows(2)
                .any(|args| args == ["--resume", "session-1"])
        );
    }

    #[test]
    fn parses_model_table() {
        let models = parse_models(concat!(
            "Available models\n",
            "\n",
            "auto - Auto (default)\n",
            "cursor-grok-4.6-high-fast - Cursor Grok 4.6 Fast\n",
            "gpt-5.2 - GPT-5.2\n",
        ))
        .unwrap();

        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "auto");
        assert_eq!(models[0].name, "Auto");
        assert!(models[0].is_default);
        assert_eq!(models[1].id, "cursor-grok-4.6-high-fast");
        assert_eq!(models[1].name, "Cursor Grok 4.6 Fast");
        assert!(!models[1].is_default);
    }

    #[test]
    fn streams_partials_and_uses_the_complete_message() {
        let input = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hel\"}]},\"session_id\":\"s-1\",\"timestamp_ms\":1}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"lo\"}]},\"session_id\":\"s-1\",\"timestamp_ms\":2}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]},\"session_id\":\"s-1\"}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"hello\",\"session_id\":\"s-1\",\"request_id\":\"r-1\"}\n"
        );
        let mut streamed = String::new();
        let response = read_events(IoCursor::new(input), &mut |delta| {
            streamed.push_str(delta);
            Ok(())
        })
        .unwrap();

        assert_eq!(streamed, "hello");
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "s-1");
    }

    #[test]
    fn falls_back_to_result_text_without_a_complete_message() {
        let input = concat!(
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"par\"}]},\"session_id\":\"s-1\",\"timestamp_ms\":1}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"partial\",\"session_id\":\"s-1\"}\n"
        );
        let response = read_events(IoCursor::new(input), &mut |_| Ok(())).unwrap();
        assert_eq!(response.answer, "partial");
    }

    #[test]
    fn surfaces_reported_errors() {
        let input = "{\"type\":\"result\",\"subtype\":\"error\",\"is_error\":true,\"result\":\"rate limited\",\"session_id\":\"s-1\"}\n";
        let error = read_events(IoCursor::new(input), &mut |_| Ok(())).unwrap_err();
        assert_eq!(error.message(), "Cursor reported an error: rate limited");
    }
}
