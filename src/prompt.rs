use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, MoveToColumn, MoveUp, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode, size};
use unicode_width::UnicodeWidthStr;

use crate::error::{Error, Result};
use crate::terminal::keyboard_enhancement_flags;

const COMMANDS: &[Command] = &[Command {
    name: "/settings",
    description: "change settings",
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
    pub fn read() -> Result<Input> {
        let _terminal = TerminalInput::enter()?;
        let mut output = io::stderr();
        let mut line = String::new();
        let mut cursor = 0;
        let mut selected = 0;
        let mut dismissed = false;
        let mut rendered_suggestions = 0;
        let mut rendered_cursor_row = 0;

        loop {
            let suggestions = if dismissed {
                Vec::new()
            } else {
                matching_commands(&line)
            };
            selected = selected.min(suggestions.len().saturating_sub(1));
            rendered_cursor_row = draw(
                &mut output,
                &line,
                cursor,
                &suggestions,
                selected,
                rendered_suggestions,
                rendered_cursor_row,
            )?;
            rendered_suggestions = suggestions.len();

            match event::read().map_err(|error| Error::terminal("could not read input", error))? {
                Event::Paste(value) => {
                    let value = value.replace("\r\n", "\n").replace('\r', "\n");
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
                        clear_suggestions(
                            &mut output,
                            &line,
                            line.len(),
                            rendered_suggestions,
                            rendered_cursor_row,
                        )?;
                        match &input {
                            Input::Line(line) if !line.trim().is_empty() => {
                                redraw_submitted(&mut output, line)?;
                            }
                            Input::Line(_) | Input::Eof => {
                                execute!(output, Print("\r\n"))
                                    .and_then(|()| output.flush())
                                    .map_err(|error| {
                                        Error::terminal("could not write prompt", error)
                                    })?;
                            }
                        }
                        return Ok(input);
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn write_submitted(line: &str) -> Result<()> {
    let mut output = io::stderr();
    draw_submitted(&mut output, line)
}

fn redraw_submitted(output: &mut impl Write, line: &str) -> Result<()> {
    let terminal_width = usize::from(size().map_or(80, |(width, _)| width).max(1));
    let row = cursor_position(line, line.len(), terminal_width).0;
    if row > 0 {
        execute!(output, MoveUp(u16::try_from(row).unwrap_or(u16::MAX)))
            .map_err(|error| Error::terminal("could not draw submitted message", error))?;
    }
    execute!(output, MoveToColumn(0), Clear(ClearType::FromCursorDown))
        .map_err(|error| Error::terminal("could not draw submitted message", error))?;
    draw_submitted(output, line)
}

fn draw_submitted(output: &mut impl Write, line: &str) -> Result<()> {
    for (index, logical_line) in line.split('\n').enumerate() {
        if index > 0 {
            execute!(output, Print("\r\n"))
                .map_err(|error| Error::terminal("could not draw submitted message", error))?;
        }
        execute!(
            output,
            SetAttribute(Attribute::Bold),
            Print(if index == 0 { "> " } else { "  " }),
            Print(logical_line),
            SetAttribute(Attribute::Reset)
        )
        .map_err(|error| Error::terminal("could not draw submitted message", error))?;
    }
    execute!(output, Print("\r\n\r\n"))
        .and_then(|()| output.flush())
        .map_err(|error| Error::terminal("could not draw submitted message", error))
}

pub fn edit_text(title: &str, initial: &str) -> Result<Option<String>> {
    let mut output = io::stderr();
    execute!(output, Show).map_err(|error| Error::terminal("could not show text input", error))?;
    let _cursor = HideCursor;
    let mut text = initial.to_owned();
    let mut cursor = text.len();
    let mut selected = 0;
    let mut dismissed = true;

    loop {
        draw_text_editor(&mut output, title, &text, cursor)?;

        match event::read().map_err(|error| Error::terminal("could not read text input", error))? {
            Event::Paste(value) => {
                let value = value.replace("\r\n", "\n").replace('\r', "\n");
                text.insert_str(cursor, &value);
                cursor += value.len();
            }
            Event::Key(key) if key.code == KeyCode::Esc => return Ok(None),
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                if let Some(input) = handle_key(
                    key,
                    &mut text,
                    &mut cursor,
                    &[],
                    &mut selected,
                    &mut dismissed,
                ) {
                    return match input {
                        Input::Line(text) => Ok(Some(text)),
                        Input::Eof => Ok(None),
                    };
                }
            }
            _ => {}
        }
    }
}

fn draw_text_editor(output: &mut impl Write, title: &str, text: &str, cursor: usize) -> Result<()> {
    let width = usize::from(size().map_or(80, |(width, _)| width).max(1));
    let (cursor_row, cursor_column) = cursor_position(text, cursor, width);

    execute!(
        output,
        MoveTo(0, 0),
        Clear(ClearType::All),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset),
        Print("\r\nShift+Enter new line  Enter save  Esc cancel\r\n\r\n> "),
        Print(text.replace('\n', "\r\n")),
        MoveTo(
            u16::try_from(cursor_column).unwrap_or(u16::MAX),
            u16::try_from(cursor_row.saturating_add(3)).unwrap_or(u16::MAX)
        )
    )
    .and_then(|()| output.flush())
    .map_err(|error| Error::terminal("could not draw text input", error))
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
            if key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
            {
                line.insert(*cursor, '\n');
                *cursor += 1;
                *selected = 0;
                *dismissed = false;
                return None;
            }
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
    previous_cursor_row: usize,
) -> Result<usize> {
    let terminal_width = usize::from(size().map_or(80, |(width, _)| width).max(1));
    let rows = previous_count.max(suggestions.len());
    if previous_cursor_row > 0 {
        execute!(
            output,
            MoveUp(u16::try_from(previous_cursor_row).unwrap_or(u16::MAX))
        )
        .map_err(|error| Error::terminal("could not draw prompt", error))?;
    }
    execute!(
        output,
        MoveToColumn(0),
        Clear(ClearType::FromCursorDown),
        Print("> "),
        Print(line.replace('\n', "\r\n"))
    )
    .map_err(|error| Error::terminal("could not draw prompt", error))?;

    for index in 0..rows {
        execute!(output, Print("\r\n"), Clear(ClearType::CurrentLine))
            .map_err(|error| Error::terminal("could not draw prompt", error))?;
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
            .map_err(|error| Error::terminal("could not draw prompt", error))?;
        }
    }

    if rows > 0 {
        execute!(output, MoveUp(u16::try_from(rows).unwrap_or(u16::MAX)))
            .map_err(|error| Error::terminal("could not draw prompt", error))?;
    }
    let (cursor_row, column) = cursor_position(line, cursor, terminal_width);
    let end_row = cursor_position(line, line.len(), terminal_width)
        .0
        .saturating_add(rows);
    if end_row > cursor_row {
        execute!(
            output,
            MoveUp(u16::try_from(end_row - cursor_row).unwrap_or(u16::MAX))
        )
        .map_err(|error| Error::terminal("could not draw prompt", error))?;
    }
    execute!(
        output,
        MoveToColumn(u16::try_from(column).unwrap_or(u16::MAX))
    )
    .and_then(|()| output.flush())
    .map_err(|error| Error::terminal("could not draw prompt", error))?;
    Ok(cursor_row)
}

fn clear_suggestions(
    output: &mut impl Write,
    line: &str,
    cursor: usize,
    previous_count: usize,
    previous_cursor_row: usize,
) -> Result<()> {
    draw(
        output,
        line,
        cursor,
        &[],
        0,
        previous_count,
        previous_cursor_row,
    )
    .map(|_| ())
}

fn cursor_position(line: &str, cursor: usize, terminal_width: usize) -> (usize, usize) {
    let before = &line[..cursor];
    let mut row = 0_usize;
    let mut cells = 2_usize;

    for (index, logical_line) in before.split('\n').enumerate() {
        if index > 0 {
            row = row.saturating_add(1);
            cells = 0;
        }
        cells = cells.saturating_add(UnicodeWidthStr::width(logical_line));
        row = row.saturating_add(cells.saturating_sub(1) / terminal_width);
        cells = cells.saturating_sub(
            (cells.saturating_sub(1) / terminal_width).saturating_mul(terminal_width),
        );
    }

    (row, cells.min(terminal_width.saturating_sub(1)))
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
    fn enter() -> Result<Self> {
        enable_raw_mode()
            .map_err(|error| Error::terminal("could not enable terminal input", error))?;
        if let Err(error) = execute!(io::stderr(), EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(Error::terminal("could not enable terminal paste", error));
        }
        execute!(
            io::stderr(),
            PushKeyboardEnhancementFlags(keyboard_enhancement_flags())
        )
        .map_err(|error| Error::terminal("could not enable enhanced keyboard input", error))?;
        Ok(Self)
    }
}

impl Drop for TerminalInput {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), PopKeyboardEnhancementFlags);
        let _ = execute!(io::stderr(), DisableBracketedPaste);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        Input, backspace, cursor_position, delete_previous_word, draw_submitted, handle_key,
        matching_commands,
    };

    #[test]
    fn submitted_messages_are_bold_aligned_and_have_breathing_room() {
        let mut output = Vec::new();
        draw_submitted(&mut output, "first\nsecond").unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("\x1b[1m> first"));
        assert!(output.contains("\r\n\x1b[1m  second"));
        assert!(output.ends_with("\r\n\r\n"));
    }

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

    #[test]
    fn shift_enter_inserts_a_newline_without_submitting() {
        let mut line = "first".to_owned();
        let mut cursor = line.len();
        let mut selected = 0;
        let mut dismissed = false;

        let input = handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            &mut line,
            &mut cursor,
            &[],
            &mut selected,
            &mut dismissed,
        );

        assert!(input.is_none());
        assert_eq!(line, "first\n");
        assert_eq!(cursor, line.len());
    }

    #[test]
    fn option_enter_inserts_a_newline_for_terminal_fallbacks() {
        let mut line = "first".to_owned();
        let mut cursor = line.len();
        let mut selected = 0;
        let mut dismissed = false;

        let input = handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &mut line,
            &mut cursor,
            &[],
            &mut selected,
            &mut dismissed,
        );

        assert!(input.is_none());
        assert_eq!(line, "first\n");
    }

    #[test]
    fn multiline_cursor_position_uses_the_current_line() {
        assert_eq!(cursor_position("first\nsecond", 8, 80), (1, 2));
        assert_eq!(cursor_position("first\nsecond", 12, 80), (1, 6));
    }

    #[test]
    fn cursor_position_accounts_for_terminal_wrapping() {
        assert_eq!(cursor_position("123456789", 9, 10), (1, 1));
        assert_eq!(cursor_position("1234567890123456789", 19, 10), (2, 1));
    }
}
