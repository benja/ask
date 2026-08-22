use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

use super::{Harness, Model, ReasoningLevel, Response, RunOptions, executable_available};

const FAST_MODELS: &[&str] = &["gpt-5.3-codex-spark", "gpt-5.6-luna"];

pub struct Codex {
    program: OsString,
    server: Option<AppServer>,
}

impl Codex {
    pub fn from_environment() -> Self {
        Self {
            program: std::env::var_os("ASK_CODEX_BIN").unwrap_or_else(|| "codex".into()),
            server: None,
        }
    }

    pub fn is_available() -> bool {
        let program = std::env::var_os("ASK_CODEX_BIN").unwrap_or_else(|| "codex".into());
        executable_available(&program)
    }

    fn server(&mut self) -> Result<&mut AppServer, String> {
        if self.server.is_none() {
            self.server = Some(AppServer::start(&self.program)?);
        }
        Ok(self.server.as_mut().expect("app server was initialized"))
    }
}

impl Harness for Codex {
    fn models(&mut self) -> Result<Vec<Model>, String> {
        let mut models = self.server()?.models()?;
        if !models.iter().any(|model| model.id == "fast")
            && let Some(model) = fastest_model(&models).cloned()
        {
            models.insert(
                0,
                Model {
                    id: "fast".into(),
                    name: "Fastest available".into(),
                    description: format!("Currently uses {}", model.name),
                    is_default: false,
                    reasoning: model.reasoning,
                    default_reasoning: model.default_reasoning,
                },
            );
        }
        Ok(models)
    }

    fn ask(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<Response, String> {
        self.server()?.ask(question, session_id, options, on_delta)
    }
}

struct AppServer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    thread_id: Option<String>,
    instructions: Option<String>,
    resolved_fast_model: Option<String>,
}

impl AppServer {
    fn start(program: &OsStr) -> Result<Self, String> {
        let mut child = Command::new(program)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "Codex is not installed or not on PATH; install it and run 'codex login'".into()
                } else {
                    format!("could not start Codex app server: {error}")
                }
            })?;

        let input = child.stdin.take().expect("piped stdin is available");
        let output = BufReader::new(child.stdout.take().expect("piped stdout is available"));
        let mut server = Self {
            child,
            input,
            output,
            next_id: 1,
            thread_id: None,
            instructions: None,
            resolved_fast_model: None,
        };

        server.send(&json!({
            "method": "initialize",
            "id": 0,
            "params": {
                "clientInfo": {
                    "name": "ask",
                    "title": "ask",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }))?;
        server.wait_for_response(0)?;
        server.send(&json!({"method": "initialized", "params": {}}))?;
        Ok(server)
    }

    fn ask(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), String>,
    ) -> Result<Response, String> {
        let model = if options.model == Some("fast") {
            Some(self.fast_model()?)
        } else {
            options.model.map(str::to_owned)
        };
        let thread_id = self.prepare_thread(session_id, options.instructions)?;
        let request_id = self.request_id();
        self.send(&json!({
            "method": "turn/start",
            "id": request_id,
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": question}],
                "model": model,
                "effort": options.reasoning
            }
        }))?;

        let mut streamed_answer = String::new();
        let mut final_answer = None;
        let mut reported_error = None;

        loop {
            let message = self.read()?;
            if message.get("id").and_then(Value::as_u64) == Some(request_id) {
                response_error(&message)?;
            }

            match message.get("method").and_then(Value::as_str) {
                Some("item/agentMessage/delta") => {
                    if let Some(delta) = message["params"]["delta"].as_str() {
                        streamed_answer.push_str(delta);
                        on_delta(delta)?;
                    }
                }
                Some("item/completed") => {
                    let item = &message["params"]["item"];
                    if item.get("type").and_then(Value::as_str) == Some("agentMessage")
                        && let Some(text) = item.get("text").and_then(Value::as_str)
                    {
                        final_answer = Some(text.to_owned());
                    }
                }
                Some("error") => {
                    reported_error = message["params"]["error"]["message"]
                        .as_str()
                        .map(str::to_owned);
                }
                Some("turn/completed") => {
                    let status = message["params"]["turn"]["status"].as_str();
                    if status != Some("completed") {
                        return Err(reported_error.unwrap_or_else(|| {
                            format!(
                                "Codex turn ended with status '{}'",
                                status.unwrap_or("unknown")
                            )
                        }));
                    }
                    break;
                }
                _ => {}
            }
        }

        if let Some(error) = reported_error {
            return Err(format!("Codex reported an error: {error}"));
        }

        let answer = final_answer.unwrap_or(streamed_answer);
        if answer.is_empty() {
            return Err("Codex completed without returning an answer".into());
        }

        Ok(Response {
            answer,
            session_id: thread_id,
        })
    }

    fn fast_model(&mut self) -> Result<String, String> {
        if let Some(model) = &self.resolved_fast_model {
            return Ok(model.clone());
        }
        let request_id = self.request_id();
        self.send(&json!({
            "method": "model/list",
            "id": request_id,
            "params": {"limit": 100, "includeHidden": false}
        }))?;
        let response = self.wait_for_response(request_id)?;
        let model = select_fast_model(&response["result"]["data"])
            .ok_or_else(|| "Codex did not report an available model".to_string())?;
        self.resolved_fast_model = Some(model.clone());
        Ok(model)
    }

    fn models(&mut self) -> Result<Vec<Model>, String> {
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let id = self.request_id();
            self.send(&json!({
                "method": "model/list",
                "id": id,
                "params": {
                    "cursor": cursor,
                    "limit": 100,
                    "includeHidden": false
                }
            }))?;
            let response = self.wait_for_response(id)?;
            let result = &response["result"];
            let data = result["data"]
                .as_array()
                .ok_or_else(|| "Codex returned an invalid model list".to_string())?;

            for value in data {
                let id = required_string(value, "model", "model")?;
                let name = required_string(value, "displayName", "model display name")?;
                let description = value["description"].as_str().unwrap_or_default().to_owned();
                let reasoning = value["supportedReasoningEfforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|effort| {
                        Some(ReasoningLevel {
                            id: effort["reasoningEffort"].as_str()?.to_owned(),
                            description: effort["description"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                        })
                    })
                    .collect();
                models.push(Model {
                    id,
                    name,
                    description,
                    is_default: value["isDefault"].as_bool().unwrap_or(false),
                    reasoning,
                    default_reasoning: value["defaultReasoningEffort"].as_str().map(str::to_owned),
                });
            }

            cursor = result["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }

        if models.is_empty() {
            Err("Codex did not report any available models".into())
        } else {
            Ok(models)
        }
    }

    fn prepare_thread(
        &mut self,
        requested: Option<&str>,
        instructions: Option<&str>,
    ) -> Result<String, String> {
        if let Some(current) = &self.thread_id
            && requested == Some(current.as_str())
            && self.instructions.as_deref() == instructions
        {
            return Ok(current.clone());
        }

        let id = self.request_id();
        let request = if let Some(thread_id) = requested {
            json!({
                "method": "thread/resume",
                "id": id,
                "params": {
                    "threadId": thread_id,
                    "approvalPolicy": "never",
                    "sandbox": "read-only",
                    "developerInstructions": instructions
                }
            })
        } else {
            let cwd = std::env::current_dir()
                .map_err(|error| format!("could not determine current directory: {error}"))?;
            json!({
                "method": "thread/start",
                "id": id,
                "params": {
                    "cwd": cwd,
                    "approvalPolicy": "never",
                    "sandbox": "read-only",
                    "developerInstructions": instructions
                }
            })
        };

        self.send(&request)?;
        let response = self.wait_for_response(id)?;
        let thread_id = response["result"]["thread"]["id"]
            .as_str()
            .ok_or_else(|| "Codex did not return a thread ID".to_string())?
            .to_owned();
        self.thread_id = Some(thread_id.clone());
        self.instructions = instructions.map(str::to_owned);
        Ok(thread_id)
    }

    fn request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.input, message)
            .map_err(|error| format!("could not encode Codex request: {error}"))?;
        self.input
            .write_all(b"\n")
            .and_then(|()| self.input.flush())
            .map_err(|error| format!("could not send request to Codex: {error}"))
    }

    fn read(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        let bytes = self
            .output
            .read_line(&mut line)
            .map_err(|error| format!("could not read Codex response: {error}"))?;
        if bytes == 0 {
            return Err("Codex app server stopped unexpectedly".into());
        }
        serde_json::from_str(&line)
            .map_err(|error| format!("could not parse Codex response: {error}"))
    }

    fn wait_for_response(&mut self, id: u64) -> Result<Value, String> {
        loop {
            let message = self.read()?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                response_error(&message)?;
                return Ok(message);
            }
        }
    }
}

fn select_fast_model(models: &Value) -> Option<String> {
    let models = models.as_array()?;
    for preferred in FAST_MODELS {
        if models.iter().any(|model| model["model"] == *preferred) {
            return Some((*preferred).to_owned());
        }
    }
    models
        .iter()
        .find(|model| model["isDefault"].as_bool() == Some(true))
        .or_else(|| models.first())?
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn fastest_model(models: &[Model]) -> Option<&Model> {
    FAST_MODELS
        .iter()
        .find_map(|id| models.iter().find(|model| model.id == *id))
        .or_else(|| models.iter().find(|model| model.is_default))
        .or_else(|| models.first())
}

fn required_string(value: &Value, key: &str, name: &str) -> Result<String, String> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("Codex returned a model without a {name}"))
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn response_error(response: &Value) -> Result<(), String> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown protocol error");
        Err(format!("Codex app server error: {message}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{fastest_model, select_fast_model};
    use crate::harness::Model;

    fn model(id: &str, is_default: bool) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            is_default,
            reasoning: Vec::new(),
            default_reasoning: None,
        }
    }

    #[test]
    fn fast_model_prefers_spark_and_falls_back_to_luna() {
        let with_spark = json!([
            {"model": "gpt-5.6-luna"},
            {"model": "gpt-5.3-codex-spark"}
        ]);
        assert_eq!(
            select_fast_model(&with_spark).as_deref(),
            Some("gpt-5.3-codex-spark")
        );

        let without_spark = json!([{"model": "gpt-5.6-luna"}]);
        assert_eq!(
            select_fast_model(&without_spark).as_deref(),
            Some("gpt-5.6-luna")
        );
    }

    #[test]
    fn fast_catalog_model_uses_the_same_preference() {
        let models = [
            model("gpt-5.6-luna", true),
            model("gpt-5.3-codex-spark", false),
        ];

        assert_eq!(fastest_model(&models).unwrap().id, "gpt-5.3-codex-spark");
        assert_eq!(
            fastest_model(&[model("custom", true)]).unwrap().id,
            "custom"
        );
    }
}
