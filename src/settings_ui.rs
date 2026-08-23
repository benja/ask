use std::io::{self, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{Clear, ClearType};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::harness::{self, Model, ReasoningControl};
use crate::instructions::Instructions;
use crate::select::{self, Choice, Item};

#[derive(Default)]
pub struct Cache {
    agent: Option<String>,
    models: Vec<Model>,
}

impl Cache {
    fn models(&mut self, agent: &str) -> Result<&[Model]> {
        if self.agent.as_deref() != Some(agent) {
            self.models = load_models_with_delayed_feedback(agent)?;
            self.agent = Some(agent.to_owned());
        }
        Ok(&self.models)
    }
}

pub fn run_defaults(config: &mut Config, cache: &mut Cache) -> Result<()> {
    let _screen = select::Screen::enter()?;
    defaults_menu(config, cache)
}

pub fn run_session(
    settings: &mut crate::Settings,
    config: &mut Config,
    cache: &mut Cache,
) -> Result<()> {
    let _screen = select::Screen::enter()?;
    session_menu(settings, config, cache)
}

fn defaults_menu(config: &mut Config, cache: &mut Cache) -> Result<()> {
    let mut selected = 0;
    loop {
        let definition = harness::resolve(&config.agent)?;
        let reasoning = match definition.reasoning {
            ReasoningControl::Selectable => {
                config.reasoning(&config.agent).unwrap_or("Model default")
            }
            ReasoningControl::Managed { label, .. } => label,
        };
        let items = [
            Item::new("Default agent", harness::agent_name(&config.agent)),
            Item::new("Model", config.model(&config.agent).unwrap_or("Default")),
            Item::new("Reasoning", reasoning),
            Item::new("Instructions", instructions_name(config)),
            Item::new("Done", "Exit settings"),
        ];

        match select::choose(
            "Settings",
            "Defaults and answer instructions",
            &items,
            selected,
        )? {
            Choice::Selected(index) => {
                selected = index;
                match index {
                    0 => select_agent(config)?,
                    1 => {
                        if let Err(error) = select_model(config, cache) {
                            show_error("Could not load models", &error)?;
                        }
                    }
                    2 => match definition.reasoning {
                        ReasoningControl::Selectable => {
                            if let Err(error) = select_reasoning(config, cache) {
                                show_error("Could not load reasoning levels", &error)?;
                            }
                        }
                        ReasoningControl::Managed { explanation, .. } => {
                            show_message("Reasoning", explanation)?
                        }
                    },
                    3 => select_instructions(config)?,
                    _ => return Ok(()),
                }
            }
            Choice::Cancelled => return Ok(()),
        }
    }
}

fn session_menu(
    settings: &mut crate::Settings,
    config: &mut Config,
    cache: &mut Cache,
) -> Result<()> {
    let mut selected = 1;
    loop {
        let definition = harness::resolve(&settings.agent)?;
        let reasoning = match definition.reasoning {
            ReasoningControl::Selectable => {
                settings.reasoning.as_deref().unwrap_or("Model default")
            }
            ReasoningControl::Managed { label, .. } => label,
        };
        let items = [
            Item::read_only(
                "Agent",
                format!(
                    "{} — fixed for this session",
                    harness::agent_name(&settings.agent)
                ),
            ),
            Item::new("Model", settings.model.as_deref().unwrap_or("Default")),
            Item::new("Reasoning", reasoning),
            Item::new("Instructions", instructions_name(config)),
            Item::new("Done", "Return to your session"),
        ];

        match select::choose(
            "Session settings",
            "Model and reasoning apply only to this session",
            &items,
            selected,
        )? {
            Choice::Selected(index) => {
                selected = index;
                match index {
                    1 => {
                        if let Err(error) = select_model(settings, cache) {
                            show_error("Could not load models", &error)?;
                        }
                    }
                    2 => match definition.reasoning {
                        ReasoningControl::Selectable => {
                            if let Err(error) = select_reasoning(settings, cache) {
                                show_error("Could not load reasoning levels", &error)?;
                            }
                        }
                        ReasoningControl::Managed { explanation, .. } => {
                            show_message("Reasoning", explanation)?
                        }
                    },
                    3 => select_instructions(config)?,
                    _ => return Ok(()),
                }
            }
            Choice::Cancelled => return Ok(()),
        }
    }
}

trait EditableSettings {
    fn agent(&self) -> &str;
    fn model(&self) -> Option<&str>;
    fn reasoning(&self) -> Option<&str>;
    fn set_model(&mut self, model: Option<String>);
    fn set_reasoning(&mut self, reasoning: Option<String>);
    fn save(&self) -> Result<()>;
}

impl EditableSettings for Config {
    fn agent(&self) -> &str {
        &self.agent
    }

    fn model(&self) -> Option<&str> {
        Config::model(self, &self.agent)
    }

    fn reasoning(&self) -> Option<&str> {
        Config::reasoning(self, &self.agent)
    }

    fn set_model(&mut self, model: Option<String>) {
        let agent = self.agent.clone();
        Config::set_model(self, &agent, model);
    }

    fn set_reasoning(&mut self, reasoning: Option<String>) {
        let agent = self.agent.clone();
        Config::set_reasoning(self, &agent, reasoning);
    }

    fn save(&self) -> Result<()> {
        Config::save(self)
    }
}

impl EditableSettings for crate::Settings {
    fn agent(&self) -> &str {
        &self.agent
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_deref()
    }

    fn set_model(&mut self, model: Option<String>) {
        self.model = model;
    }

    fn set_reasoning(&mut self, reasoning: Option<String>) {
        self.reasoning = reasoning;
    }

    fn save(&self) -> Result<()> {
        Ok(())
    }
}

fn instructions_name(config: &Config) -> &'static str {
    match config.instructions() {
        Instructions::Concise => "Concise",
        Instructions::Custom(_) => "Custom",
        Instructions::AgentDefault => "Agent default",
    }
}

fn select_instructions(config: &mut Config) -> Result<()> {
    let items = [
        Item::new("Concise", "Friendly, direct answers"),
        Item::new("Agent default", "Do not add ask instructions"),
        Item::new("Custom…", "Write your own instructions"),
    ];
    let selected = match config.instructions() {
        Instructions::Concise => 0,
        Instructions::AgentDefault => 1,
        Instructions::Custom(_) => 2,
    };
    let Choice::Selected(index) = select::choose(
        "Instructions",
        "How answers should be written",
        &items,
        selected,
    )?
    else {
        return Ok(());
    };

    match index {
        0 => config.set_instructions(Instructions::Concise),
        1 => config.set_instructions(Instructions::AgentDefault),
        _ => {
            let initial = config.instructions().custom().unwrap_or_default();
            let Some(instructions) = crate::prompt::edit_text("Custom instructions", initial)?
            else {
                return Ok(());
            };
            let instructions = instructions.trim();
            if instructions.is_empty() {
                show_message("Instructions", "Custom instructions cannot be empty.")?;
                return Ok(());
            }
            config.set_instructions(Instructions::Custom(instructions.to_owned()));
        }
    }
    config.save()
}

fn select_agent(config: &mut Config) -> Result<()> {
    let agents = detected_agents();
    if agents.is_empty() {
        show_message(
            "No agents found",
            "Install a supported coding agent and make sure its binary is on PATH.",
        )?;
        return Ok(());
    }
    let items = agents
        .iter()
        .map(|agent| Item::new(agent.name, agent.description))
        .collect::<Vec<_>>();
    let selected = agents
        .iter()
        .position(|agent| agent.id == config.agent)
        .unwrap_or(0);
    let Choice::Selected(index) = select::choose(
        "Default agent",
        "Choose the agent for new sessions",
        &items,
        selected,
    )?
    else {
        return Ok(());
    };

    config.agent = agents[index].id.to_owned();
    config.save()
}

fn detected_agents() -> Vec<&'static harness::Definition> {
    harness::DEFINITIONS
        .iter()
        .filter(|definition| definition.is_available())
        .collect()
}

fn select_model(settings: &mut impl EditableSettings, cache: &mut Cache) -> Result<()> {
    let agent = settings.agent().to_owned();
    let models = cache.models(&agent)?;
    let mut items = Vec::with_capacity(models.len() + 1);
    items.push(Item::new(
        "Default",
        format!("Let {} choose", harness::agent_name(&agent)),
    ));
    items.extend(
        models
            .iter()
            .map(|model| Item::new(&model.name, &model.description)),
    );
    let selected = settings
        .model()
        .and_then(|current| models.iter().position(|model| model.id == current))
        .map_or(0, |index| index + 1);

    let Choice::Selected(index) = select::choose(
        "Model",
        &format!("Models supported by {}", harness::agent_name(&agent)),
        &items,
        selected,
    )?
    else {
        return Ok(());
    };

    let model = index.checked_sub(1).and_then(|index| models.get(index));
    settings.set_model(model.map(|model| model.id.clone()));

    if matches!(
        harness::resolve(&agent)?.reasoning,
        ReasoningControl::Selectable
    ) {
        let effective = model.or_else(|| models.iter().find(|model| model.is_default));
        if let Some(model) = effective
            && !supports_reasoning(model, settings.reasoning())
        {
            settings.set_reasoning(model.default_reasoning.clone());
        }
    }
    settings.save()
}

fn select_reasoning(settings: &mut impl EditableSettings, cache: &mut Cache) -> Result<()> {
    let agent = settings.agent().to_owned();
    let models = cache.models(&agent)?;
    let selected_model = settings.model().ok_or_else(|| {
        Error::new(
            format!("no {} model is selected", harness::agent_name(&agent)),
            "choose a model before setting reasoning",
        )
    })?;
    let model = models
        .iter()
        .find(|model| model.id == selected_model)
        .or_else(|| models.iter().find(|model| model.is_default))
        .ok_or_else(|| {
            Error::new(
                format!(
                    "{} did not identify a default model",
                    harness::agent_name(&agent)
                ),
                "choose a model explicitly and try again",
            )
        })?;

    if model.reasoning.is_empty() {
        return Err(Error::new(
            format!("{} did not report any reasoning levels", model.name),
            "choose Model default or another model",
        ));
    }

    let mut items = Vec::with_capacity(model.reasoning.len() + 1);
    items.push(Item::new(
        "Model default",
        model.default_reasoning.as_deref().unwrap_or("Recommended"),
    ));
    items.extend(
        model
            .reasoning
            .iter()
            .map(|level| Item::new(title_case(&level.id), &level.description)),
    );
    let selected = settings
        .reasoning()
        .and_then(|current| model.reasoning.iter().position(|level| level.id == current))
        .map_or(0, |index| index + 1);

    let Choice::Selected(index) = select::choose(
        "Reasoning",
        &format!("Levels supported by {}", model.name),
        &items,
        selected,
    )?
    else {
        return Ok(());
    };
    settings.set_reasoning(
        index
            .checked_sub(1)
            .and_then(|index| model.reasoning.get(index))
            .map(|level| level.id.clone()),
    );
    settings.save()
}

fn load_models_with_delayed_feedback(agent: &str) -> Result<Vec<Model>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let requested = agent.to_owned();
    std::thread::spawn(move || {
        let result = harness::create(&requested).and_then(|mut harness| harness.models());
        let _ = sender.send(result);
    });

    match receiver.recv_timeout(Duration::from_millis(150)) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            show_loading(
                "Models",
                &format!("Loading models from {}…", harness::agent_name(agent)),
            )?;
            receiver.recv().map_err(|_| model_discovery_error())?
        }
        Err(RecvTimeoutError::Disconnected) => Err(model_discovery_error()),
    }
}

fn supports_reasoning(model: &Model, reasoning: Option<&str>) -> bool {
    reasoning.is_none()
        || model
            .reasoning
            .iter()
            .any(|level| Some(level.id.as_str()) == reasoning)
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
        .unwrap_or_default()
}

fn show_loading(title: &str, message: &str) -> Result<()> {
    let mut output = io::stderr();
    execute!(
        output,
        MoveTo(0, 0),
        Clear(ClearType::All),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset),
        Print(format!("\r\n\r\n{message}"))
    )
    .and_then(|()| output.flush())
    .map_err(|error| Error::terminal("could not draw settings", error))
}

fn show_message(title: &str, message: &str) -> Result<()> {
    show_loading(title, message)?;
    let mut output = io::stderr();
    execute!(output, Print("\r\n\r\nPress any key to go back."))
        .and_then(|()| output.flush())
        .map_err(|error| Error::terminal("could not draw settings", error))?;
    loop {
        if matches!(
            event::read()
                .map_err(|error| Error::terminal("could not read settings input", error))?,
            Event::Key(_)
        ) {
            return Ok(());
        }
    }
}

fn show_error(title: &str, error: &Error) -> Result<()> {
    show_message(
        title,
        &format!("{}\r\n\r\n{}", error.message(), error.help()),
    )
}

fn model_discovery_error() -> Error {
    Error::new(
        "model discovery stopped unexpectedly",
        "close settings and try again",
    )
}

#[cfg(test)]
mod tests {
    use super::Cache;
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
    fn cached_models_are_reused_for_the_same_agent() {
        let mut cache = Cache {
            agent: Some("codex".into()),
            models: vec![model("cached", true)],
        };

        assert_eq!(cache.models("codex").unwrap()[0].id, "cached");
    }
}
