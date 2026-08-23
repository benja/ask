mod cli;
mod config;
mod harness;
mod instructions;
mod prompt;
mod render;
mod select;
mod settings_ui;
mod spinner;
mod state;
mod storage;
mod terminal;
mod update_check;
mod upgrade;

use std::process::ExitCode;

use harness::Harness;
use instructions::Instructions;

const SESSION_NAME_WIDTH: usize = 24;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mode = cli::parse(std::env::args_os().skip(1))?;
    let update_check = update_check::Check::start(update_checks_enabled(&mode));

    let result = match mode {
        cli::Mode::Help => {
            write_stdout(&[cli::HELP.as_bytes()])?;
            Ok(())
        }
        cli::Mode::Version => {
            write_stdout(&[env!("CARGO_PKG_VERSION").as_bytes(), b"\n"])?;
            Ok(())
        }
        cli::Mode::Upgrade => {
            if let Some(message) = upgrade::run()? {
                write_stdout(&[message.as_bytes(), b"\n"])?;
            }
            Ok(())
        }
        cli::Mode::OneShot(question) => {
            let config = config::Config::load()?;
            let settings = settings(None, &config, None)?;
            run_one_shot(&settings, config.instructions(), &question, None)
        }
        cli::Mode::Interactive => {
            let config = config::Config::load()?;
            let settings = settings(None, &config, None)?;
            run_interactive(settings, None, config)
        }
        cli::Mode::Continue(words) => {
            let config = config::Config::load()?;
            let cwd = std::env::current_dir()
                .map_err(|error| format!("could not determine current directory: {error}"))?;
            let session = state::latest(&cwd)?;
            let agent = session.agent.clone();
            let settings = settings(Some(&agent), &config, Some(&session))?;
            if words.is_empty() {
                run_interactive(settings, Some(session), config)
            } else {
                run_one_shot(
                    &settings,
                    config.instructions(),
                    &words.join(" "),
                    Some(session),
                )
            }
        }
        cli::Mode::Sessions => sessions(),
        cli::Mode::Settings => standalone_settings(),
    };
    if result.is_ok()
        && let Some(notice) = update_check.notice()
    {
        eprintln!("{notice}");
    }
    result
}

fn update_checks_enabled(mode: &cli::Mode) -> bool {
    use std::io::{self, IsTerminal};

    std::env::var_os("ASK_NO_UPDATE_CHECK").is_none()
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && io::stderr().is_terminal()
        && !matches!(
            mode,
            cli::Mode::Help | cli::Mode::Version | cli::Mode::Upgrade
        )
}

struct Settings {
    agent: String,
    model: Option<String>,
    reasoning: Option<String>,
}

fn settings(
    selected_agent: Option<&str>,
    config: &config::Config,
    session: Option<&state::Session>,
) -> Result<Settings, String> {
    let definition = harness::resolve(selected_agent.unwrap_or(&config.agent))?;
    let agent = definition.id.to_owned();
    let saved = session.and_then(|session| session.settings.as_ref());
    let model = saved.map_or_else(
        || config.model(&agent).map(str::to_owned),
        |settings| settings.model.clone(),
    );
    let reasoning = match definition.reasoning {
        harness::ReasoningControl::Selectable => saved.map_or_else(
            || config.reasoning(&agent).map(str::to_owned),
            |settings| settings.reasoning.clone(),
        ),
        harness::ReasoningControl::Managed { .. } => None,
    };
    Ok(Settings {
        agent,
        model,
        reasoning,
    })
}

fn run_one_shot(
    settings: &Settings,
    instructions: &Instructions,
    question: &str,
    mut saved: Option<state::Session>,
) -> Result<(), String> {
    use std::io::{self, IsTerminal};

    let mut harness = harness::create(&settings.agent)?;
    let decorate = io::stdout().is_terminal();
    let cwd = std::env::current_dir()
        .map_err(|error| format!("could not determine current directory: {error}"))?;
    let response = ask_turn(
        harness.as_mut(),
        settings,
        instructions,
        question,
        saved
            .as_ref()
            .map(|session| session.harness_session_id.as_str()),
        decorate,
    )?;
    record_turn(&mut saved, settings, &cwd, question, response)?;
    Ok(())
}

fn run_interactive(
    mut settings: Settings,
    mut saved: Option<state::Session>,
    mut config: config::Config,
) -> Result<(), String> {
    use std::io::{self, IsTerminal};

    let mut harness = harness::create(&settings.agent)?;
    let terminal = io::stdin().is_terminal() && io::stderr().is_terminal();
    let decorate = terminal && io::stdout().is_terminal();
    if let Some(session) = &saved
        && terminal
    {
        show_history(session, decorate)?;
    }

    let cwd = std::env::current_dir()
        .map_err(|error| format!("could not determine current directory: {error}"))?;
    let mut session = saved
        .as_ref()
        .map(|session| session.harness_session_id.clone());
    let mut settings_cache = settings_ui::Cache::default();
    let mut piped_line = String::new();

    loop {
        let line = if terminal {
            match prompt::Prompt::read()? {
                prompt::Input::Line(line) => line,
                prompt::Input::Eof => break,
            }
        } else {
            piped_line.clear();
            let bytes = io::stdin()
                .read_line(&mut piped_line)
                .map_err(|error| format!("could not read input: {error}"))?;
            if bytes == 0 {
                break;
            }
            piped_line.clone()
        };

        let question = line.trim();
        if question.is_empty() {
            continue;
        }
        if question.starts_with('/') {
            let mut parts = question.split_whitespace();
            let command = parts.next().unwrap_or_default();
            let value = parts.next();
            if parts.next().is_some() {
                eprintln!("ask: {command} accepts at most one value");
                if terminal {
                    eprintln!();
                }
                continue;
            }

            match command {
                "/settings" if value.is_none() && terminal => {
                    settings_ui::run_session(&mut settings, &mut config, &mut settings_cache)?;
                    if let Some(saved) = &mut saved {
                        saved.settings = Some(session_settings(&settings));
                        state::save(saved)?;
                    }
                }
                "/settings" if value.is_none() => {
                    eprintln!("ask: /settings requires an interactive terminal");
                }
                _ => {
                    eprintln!("ask: unknown command '{command}' (available: /settings)");
                    if terminal {
                        eprintln!();
                    }
                }
            }
            continue;
        }

        let response = match ask_turn(
            harness.as_mut(),
            &settings,
            config.instructions(),
            question,
            session.as_deref(),
            decorate,
        ) {
            Ok(response) => response,
            Err(error) if terminal => {
                eprintln!("ask: {error}");
                eprintln!();
                continue;
            }
            Err(error) => return Err(error),
        };
        if decorate {
            eprintln!();
        }
        session = Some(record_turn(
            &mut saved, &settings, &cwd, question, response,
        )?);
    }

    Ok(())
}

fn standalone_settings() -> Result<(), String> {
    use std::io::{self, IsTerminal};

    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("--settings requires an interactive terminal".into());
    }
    let mut config = config::Config::load()?;
    let mut cache = settings_ui::Cache::default();
    settings_ui::run_defaults(&mut config, &mut cache)
}

fn ask_turn(
    harness: &mut dyn Harness,
    settings: &Settings,
    instructions: &Instructions,
    question: &str,
    session_id: Option<&str>,
    decorate: bool,
) -> Result<harness::Response, String> {
    let mut output = TurnOutput::new(decorate);
    let mut spinner = spinner::Spinner::start(decorate && spinner::enabled());
    let response = harness.ask(
        question,
        session_id,
        harness::RunOptions {
            model: settings.model.as_deref(),
            reasoning: settings.reasoning.as_deref(),
            instructions: instructions.prompt(),
        },
        &mut |delta| {
            spinner.stop();
            output.push(delta)
        },
    );
    spinner.stop();
    let response = response?;
    output.finish(&response.answer)?;
    Ok(response)
}

fn record_turn(
    saved: &mut Option<state::Session>,
    settings: &Settings,
    cwd: &std::path::Path,
    question: &str,
    response: harness::Response,
) -> Result<String, String> {
    let stored = saved.get_or_insert_with(|| {
        state::Session::new(&settings.agent, response.session_id.clone(), cwd)
    });
    stored.harness_session_id.clone_from(&response.session_id);
    stored.settings = Some(session_settings(settings));
    stored.add_turn(question, response.answer);
    state::save(stored)?;
    Ok(response.session_id)
}

fn session_settings(settings: &Settings) -> state::SessionSettings {
    state::SessionSettings {
        model: settings.model.clone(),
        reasoning: settings.reasoning.clone(),
    }
}

fn show_history(session: &state::Session, decorate: bool) -> Result<(), String> {
    eprintln!(
        "Continuing {} — {} turn{}",
        harness::agent_name(&session.agent),
        session.turns.len(),
        if session.turns.len() == 1 { "" } else { "s" }
    );

    for turn in &session.turns {
        if decorate {
            prompt::write_submitted(&turn.user)?;
        } else {
            eprintln!("> {}", turn.user);
        }
        let mut output = TurnOutput::new(decorate);
        output.push(&turn.assistant)?;
        output.finish(&turn.assistant)?;
        if decorate {
            eprintln!();
        }
    }
    Ok(())
}

fn sessions() -> Result<(), String> {
    use std::io::{self, IsTerminal};

    let mut sessions = state::load_all()?;
    if sessions.is_empty() {
        write_stdout(&[b"No saved sessions.\n"])?;
        return Ok(());
    }

    if !(io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()) {
        return list_sessions(&sessions);
    }

    let (selected, deleted_all) = {
        let _screen = select::Screen::enter()?;
        let mut initial = 0;
        loop {
            let items = session_items(&sessions);
            match select::choose_deletable(
                "Sessions",
                "Choose a session to continue",
                &items,
                initial,
                SESSION_NAME_WIDTH,
            )? {
                select::DeletableChoice::Selected(index) => break (Some(index), false),
                select::DeletableChoice::Deleted(index) => {
                    state::delete(&sessions[index])?;
                    sessions.remove(index);
                    let Some(next) = selection_after_delete(index, sessions.len()) else {
                        break (None, true);
                    };
                    initial = next;
                }
                select::DeletableChoice::Cancelled => break (None, false),
            }
        }
    };
    if deleted_all {
        write_stdout(&[b"No saved sessions.\n"])?;
    }
    let Some(selected) = selected else {
        return Ok(());
    };

    let session = sessions.remove(selected);
    let config = config::Config::load()?;
    let settings = settings(Some(&session.agent), &config, Some(&session))?;
    run_interactive(settings, Some(session), config)
}

fn selection_after_delete(deleted: usize, remaining: usize) -> Option<usize> {
    (remaining > 0).then(|| deleted.min(remaining - 1))
}

fn session_items(sessions: &[state::Session]) -> Vec<select::Item> {
    sessions
        .iter()
        .map(|session| {
            let question = session
                .turns
                .last()
                .map_or_else(|| "Empty session".into(), |turn| preview(&turn.user, 34));
            let folder = std::path::Path::new(&session.cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&session.cwd);
            select::Item::new(
                question,
                format!(
                    "{} — {} — {}",
                    harness::agent_name(&session.agent),
                    folder,
                    age(session.updated_at)
                ),
            )
        })
        .collect()
}

fn list_sessions(sessions: &[state::Session]) -> Result<(), String> {
    for session in sessions {
        let line = format!(
            "{:<6}  {:>3} turn{}  {:>8}  {}\n",
            harness::agent_name(&session.agent),
            session.turns.len(),
            if session.turns.len() == 1 { " " } else { "s" },
            age(session.updated_at),
            session.cwd
        );
        write_stdout(&[line.as_bytes()])?;
    }
    Ok(())
}

fn preview(value: &str, limit: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = value.chars();
    let preview = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn age(timestamp: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        0..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn print_answer(answer: &str) -> Result<(), String> {
    if answer.ends_with('\n') {
        write_stdout(&[answer.as_bytes()])
    } else {
        write_stdout(&[answer.as_bytes(), b"\n"])
    }
}

#[derive(Default)]
struct RawStream {
    wrote: bool,
    ends_with_newline: bool,
}

struct TurnOutput {
    kind: TurnOutputKind,
    streamed: bool,
}

enum TurnOutputKind {
    Raw(RawStream),
    Rich(render::TerminalRenderer),
}

impl TurnOutput {
    fn new(rich: bool) -> Self {
        Self {
            kind: if rich {
                TurnOutputKind::Rich(render::TerminalRenderer::new())
            } else {
                TurnOutputKind::Raw(RawStream::default())
            },
            streamed: false,
        }
    }

    fn push(&mut self, delta: &str) -> Result<(), String> {
        if !delta.is_empty() {
            self.streamed = true;
        }
        match &mut self.kind {
            TurnOutputKind::Raw(output) => output.push(delta),
            TurnOutputKind::Rich(output) => output.push(delta),
        }
    }

    fn finish(mut self, fallback: &str) -> Result<(), String> {
        match &mut self.kind {
            TurnOutputKind::Rich(output) if !self.streamed => output.push(fallback)?,
            TurnOutputKind::Raw(_) | TurnOutputKind::Rich(_) => {}
        }

        match self.kind {
            TurnOutputKind::Raw(output) => output.finish(fallback),
            TurnOutputKind::Rich(output) => output.finish(),
        }
    }
}

impl RawStream {
    fn push(&mut self, delta: &str) -> Result<(), String> {
        if delta.is_empty() {
            return Ok(());
        }
        self.wrote = true;
        self.ends_with_newline = delta.ends_with('\n');
        write_stdout(&[delta.as_bytes()])
    }

    fn finish(&self, fallback: &str) -> Result<(), String> {
        if !self.wrote {
            print_answer(fallback)
        } else if !self.ends_with_newline {
            write_stdout(&[b"\n"])
        } else {
            Ok(())
        }
    }
}

fn write_stdout(parts: &[&[u8]]) -> Result<(), String> {
    use std::io::{self, Write};

    let mut stdout = io::stdout().lock();
    for part in parts {
        match stdout.write_all(part) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => return Err(format!("could not write output: {error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{preview, selection_after_delete, settings};
    use crate::config::Config;
    use crate::state::{Session, SessionSettings};

    #[test]
    fn session_preview_is_single_line_and_truncated() {
        assert_eq!(preview("one\n  two", 20), "one two");
        assert_eq!(preview("abcdefgh", 4), "abcd…");
    }

    #[test]
    fn deletion_selects_the_nearest_remaining_session() {
        assert_eq!(selection_after_delete(1, 3), Some(1));
        assert_eq!(selection_after_delete(3, 3), Some(2));
        assert_eq!(selection_after_delete(0, 0), None);
    }

    #[test]
    fn saved_session_settings_override_current_defaults() {
        let config = Config::default();
        let mut session = Session::new("codex", "session-id".into(), Path::new("/tmp/project"));
        session.settings = Some(SessionSettings {
            model: None,
            reasoning: Some("high".into()),
        });

        let settings = settings(Some("codex"), &config, Some(&session)).unwrap();

        assert_eq!(settings.model, None);
        assert_eq!(settings.reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn older_sessions_inherit_current_defaults() {
        let config = Config::default();
        let session = Session::new("codex", "session-id".into(), Path::new("/tmp/project"));

        let settings = settings(Some("codex"), &config, Some(&session)).unwrap();

        assert_eq!(settings.model.as_deref(), Some("fast"));
        assert_eq!(settings.reasoning.as_deref(), Some("low"));
    }
}
