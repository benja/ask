use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, MoveToColumn, MoveUp, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use unicode_width::UnicodeWidthStr;

const COMMANDS: &[Command] = &[Command {
    name: "/settings",
    description: "change this session",
}];

struct Command {
    name: &'static str,
    description: &'static str,
}

pub enum Input {
    Line(String),
    Eof,
}

pub struct Prompt;

impl Prompt {
    pub fn read() -> Result<Input, String> {
        let _terminal = TerminalInput::enter()?;
        let mut output = io::stderr();
        let mut line = String::new();
        let mut cursor = 0;
        let mut selected = 0;
        let mut dismissed = false;
        let mut rendered_suggestions = 0;

        loop {
            let suggestions = if dismissed {
                Vec::new()
            } else {
                matching_commands(&line)
            };
            selected = selected.min(suggestions.len().saturating_sub(1));
            draw(
                &mut output,
                &line,
                cursor,
                &suggestions,
                selected,
                rendered_suggestions,
            )?;
            rendered_suggestions = suggestions.len();

            match event::read().map_err(|error| format!("could not read input: {error}"))? {
                Event::Paste(value) => {
                    line.insert_str(cursor, &value);
                    cursor += value.len();
                    selected = 0;
                    dismissed = false;
                }
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if let Some(input) = handle_key(
                        key,
                        &mut line,
                        &mut cursor,
                        &suggestions,
                        &mut selected,
                        &mut dismissed,
                    ) {
                        clear_suggestions(&mut output, &line, cursor, rendered_suggestions)?;
                        execute!(output, Print("\r\n"))
                            .and_then(|()| output.flush())
                            .map_err(|error| format!("could not write prompt: {error}"))?;
                        return Ok(input);
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn edit_line(title: &str, initial: &str) -> Result<Option<String>, String> {
    let mut output = io::stderr();
    execute!(output, Show).map_err(|error| format!("could not show text input: {error}"))?;
    let _cursor = HideCursor;
    let mut line = initial.to_owned();
    let mut cursor = line.len();
    let mut selected = 0;
    let mut dismissed = true;

    loop {
        execute!(
            output,
            MoveTo(0, 0),
            Clear(ClearType::All),
            SetAttribute(Attribute::Bold),
            Print(title),
            SetAttribute(Attribute::Reset),
            Print("\r\nOne line. Enter saves; Esc cancels.\r\n\r\n> "),
            Print(&line),
            MoveToColumn(
                u16::try_from(2_usize.saturating_add(UnicodeWidthStr::width(&line[..cursor])))
                    .unwrap_or(u16::MAX)
            )
        )
        .and_then(|()| output.flush())
        .map_err(|error| format!("could not draw text input: {error}"))?;

        match event::read().map_err(|error| format!("could not read text input: {error}"))? {
            Event::Paste(value) => {
                let value = value.replace(['\r', '\n'], " ");
                line.insert_str(cursor, &value);
                cursor += value.len();
            }
            Event::Key(key) if key.code == KeyCode::Esc => return Ok(None),
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                if let Some(input) = handle_key(
                    key,
                    &mut line,
                    &mut cursor,
                    &[],
                    &mut selected,
                    &mut dismissed,
                ) {
                    return match input {
                        Input::Line(line) => Ok(Some(line)),
                        Input::Eof => Ok(None),
                    };
                }
            }
            _ => {}
        }
    }
}

struct HideCursor;

impl Drop for HideCursor {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), Hide);
    }
}

fn handle_key(
    key: KeyEvent,
    line: &mut String,
    cursor: &mut usize,
    suggestions: &[&Command],
    selected: &mut usize,
    dismissed: &mut bool,
) -> Option<Input> {
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Backspace {
        delete_previous_word(line, cursor);
        *selected = 0;
        *dismissed = false;
        return None;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Some(Input::Eof),
            KeyCode::Char('d') if line.is_empty() => return Some(Input::Eof),
            KeyCode::Char('d') => delete_at(line, *cursor),
            KeyCode::Char('a') => *cursor = 0,
            KeyCode::Char('e') => *cursor = line.len(),
            KeyCode::Char('k') => line.truncate(*cursor),
            KeyCode::Char('u') => {
                line.drain(..*cursor);
                *cursor = 0;
            }
            KeyCode::Char('w') => delete_previous_word(line, cursor),
            _ => return None,
        }
        *selected = 0;
        *dismissed = false;
        return None;
    }

    match key.code {
        KeyCode::Char(character) => {
            line.insert(*cursor, character);
            *cursor += character.len_utf8();
            *selected = 0;
            *dismissed = false;
        }
        KeyCode::Backspace => {
            backspace(line, cursor);
            *selected = 0;
            *dismissed = false;
        }
        KeyCode::Delete => {
            delete_at(line, *cursor);
            *selected = 0;
            *dismissed = false;
        }
        KeyCode::Left => *cursor = previous_boundary(line, *cursor),
        KeyCode::Right => *cursor = next_boundary(line, *cursor),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = line.len(),
        KeyCode::Up if !suggestions.is_empty() => {
            *selected = selected
                .checked_sub(1)
                .unwrap_or(suggestions.len().saturating_sub(1));
        }
        KeyCode::Down if !suggestions.is_empty() => *selected = (*selected + 1) % suggestions.len(),
        KeyCode::Tab if !suggestions.is_empty() => {
            suggestions[*selected].name.clone_into(line);
            *cursor = line.len();
            *selected = 0;
        }
        KeyCode::Enter => {
            if !suggestions.is_empty() {
                suggestions[*selected].name.clone_into(line);
            }
            return Some(Input::Line(line.clone()));
        }
        KeyCode::Esc => *dismissed = true,
        _ => {}
    }
    None
}

fn matching_commands(input: &str) -> Vec<&'static Command> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(input))
        .collect()
}

fn draw(
    output: &mut impl Write,
    line: &str,
    cursor: usize,
    suggestions: &[&Command],
    selected: usize,
    previous_count: usize,
) -> Result<(), String> {
    let rows = previous_count.max(suggestions.len());
    execute!(
        output,
        MoveToColumn(0),
        Clear(ClearType::CurrentLine),
        Print("> "),
        Print(line)
    )
    .map_err(|error| format!("could not draw prompt: {error}"))?;

    for index in 0..rows {
        execute!(output, Print("\r\n"), Clear(ClearType::CurrentLine))
            .map_err(|error| format!("could not draw prompt: {error}"))?;
        if let Some(command) = suggestions.get(index) {
            if index == selected {
                execute!(
                    output,
                    SetAttribute(Attribute::Reverse),
                    Print(format!(
                        "  › {:<12} {}  ",
                        command.name, command.description
                    )),
                    SetAttribute(Attribute::Reset)
                )
            } else {
                execute!(
                    output,
                    Print(format!("    {:<12} {}", command.name, command.description))
                )
            }
            .map_err(|error| format!("could not draw prompt: {error}"))?;
        }
    }

    if rows > 0 {
        execute!(output, MoveUp(u16::try_from(rows).unwrap_or(u16::MAX)))
            .map_err(|error| format!("could not draw prompt: {error}"))?;
    }
    let column = 2_usize.saturating_add(UnicodeWidthStr::width(&line[..cursor]));
    execute!(
        output,
        MoveToColumn(u16::try_from(column).unwrap_or(u16::MAX))
    )
    .and_then(|()| output.flush())
    .map_err(|error| format!("could not draw prompt: {error}"))
}

fn clear_suggestions(
    output: &mut impl Write,
    line: &str,
    cursor: usize,
    previous_count: usize,
) -> Result<(), String> {
    draw(output, line, cursor, &[], 0, previous_count)
}

fn previous_boundary(line: &str, cursor: usize) -> usize {
    line[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(line: &str, cursor: usize) -> usize {
    line[cursor..]
        .char_indices()
        .nth(1)
        .map_or(line.len(), |(index, _)| cursor + index)
}

fn backspace(line: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let previous = previous_boundary(line, *cursor);
    line.drain(previous..*cursor);
    *cursor = previous;
}

fn delete_at(line: &mut String, cursor: usize) {
    if cursor < line.len() {
        line.drain(cursor..next_boundary(line, cursor));
    }
}

fn delete_previous_word(line: &mut String, cursor: &mut usize) {
    let before = &line[..*cursor];
    let trimmed = before.trim_end_matches(char::is_whitespace);
    let start = trimmed
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    line.drain(start..*cursor);
    *cursor = start;
}

struct TerminalInput;

impl TerminalInput {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("could not enable terminal input: {error}"))?;
        if let Err(error) = execute!(io::stderr(), EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(format!("could not enable terminal paste: {error}"));
        }
        Ok(Self)
    }
}

impl Drop for TerminalInput {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), DisableBracketedPaste);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Input, backspace, delete_previous_word, handle_key, matching_commands};

    #[test]
    fn suggestions_filter_by_prefix() {
        let names = matching_commands("/s")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["/settings"]);
        assert!(matching_commands("hello").is_empty());
        assert!(matching_commands("/settings now").is_empty());
    }

    #[test]
    fn control_c_requests_a_clean_exit() {
        let mut line = String::new();
        let mut cursor = 0;
        let mut selected = 0;
        let mut dismissed = false;
        let input = handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut line,
            &mut cursor,
            &[],
            &mut selected,
            &mut dismissed,
        );

        assert!(matches!(input, Some(Input::Eof)));
    }

    #[test]
    fn editing_respects_character_boundaries() {
        let mut line = "ask ø".to_owned();
        let mut cursor = line.len();
        backspace(&mut line, &mut cursor);
        assert_eq!(line, "ask ");
        assert_eq!(cursor, line.len());

        line.push_str("this now");
        cursor = line.len();
        delete_previous_word(&mut line, &mut cursor);
        assert_eq!(line, "ask this ");
    }

    #[test]
    fn option_backspace_deletes_the_previous_word() {
        let mut line = "ask this now".to_owned();
        let mut cursor = line.len();
        let mut selected = 0;
        let mut dismissed = false;

        handle_key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
            &mut line,
            &mut cursor,
            &[],
            &mut selected,
            &mut dismissed,
        );

        assert_eq!(line, "ask this ");
        assert_eq!(cursor, line.len());
    }
}
