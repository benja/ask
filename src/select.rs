use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use unicode_width::UnicodeWidthStr;

use crate::terminal::keyboard_enhancement_flags;

const DEFAULT_LABEL_WIDTH: usize = 18;

pub struct Item {
    label: String,
    detail: String,
    selectable: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            selectable: true,
        }
    }

    pub fn read_only(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            selectable: false,
        }
    }
}

pub enum Choice {
    Selected(usize),
    Cancelled,
}

pub enum DeletableChoice {
    Selected(usize),
    Deleted(usize),
    Cancelled,
}

enum MenuChoice {
    Selected(usize),
    Deleted(usize),
    Cancelled,
}

#[derive(Clone, Copy)]
enum DeleteState {
    Disabled,
    Ready,
    Confirming,
}

enum DeleteKey {
    Ignored,
    Consumed,
    Confirmed,
}

#[derive(Clone, Copy)]
struct Viewport {
    capacity: usize,
    width: usize,
}

#[derive(Clone, Copy)]
struct DrawState {
    selected: usize,
    offset: usize,
    delete: DeleteState,
    viewport: Viewport,
}

pub fn choose(
    title: &str,
    subtitle: &str,
    items: &[Item],
    initial: usize,
) -> Result<Choice, String> {
    match choose_inner(title, subtitle, items, initial, DEFAULT_LABEL_WIDTH, false)? {
        MenuChoice::Selected(index) => Ok(Choice::Selected(index)),
        MenuChoice::Cancelled => Ok(Choice::Cancelled),
        MenuChoice::Deleted(_) => unreachable!("deletion is disabled"),
    }
}

pub fn choose_deletable(
    title: &str,
    subtitle: &str,
    items: &[Item],
    initial: usize,
    label_width: usize,
) -> Result<DeletableChoice, String> {
    match choose_inner(title, subtitle, items, initial, label_width, true)? {
        MenuChoice::Selected(index) => Ok(DeletableChoice::Selected(index)),
        MenuChoice::Deleted(index) => Ok(DeletableChoice::Deleted(index)),
        MenuChoice::Cancelled => Ok(DeletableChoice::Cancelled),
    }
}

fn choose_inner(
    title: &str,
    subtitle: &str,
    items: &[Item],
    initial: usize,
    label_width: usize,
    deletable: bool,
) -> Result<MenuChoice, String> {
    debug_assert!(!items.is_empty());
    debug_assert!(items.iter().any(|item| item.selectable));
    let mut selected = selectable_at_or_after(items, initial.min(items.len() - 1));
    let initial_height = size().map_or(24, |(_, height)| height);
    let mut offset = initial_offset(
        selected,
        items.len(),
        visible_capacity(usize::from(initial_height), items.len()),
    );
    let mut delete_state = if deletable {
        DeleteState::Ready
    } else {
        DeleteState::Disabled
    };
    loop {
        let (terminal_width, terminal_height) = size().unwrap_or((80, 24));
        let viewport = Viewport {
            capacity: visible_capacity(usize::from(terminal_height), items.len()),
            width: usize::from(terminal_width),
        };
        offset = keep_visible(selected, offset, items.len(), viewport.capacity);
        draw(
            title,
            subtitle,
            items,
            label_width,
            DrawState {
                selected,
                offset,
                delete: delete_state,
                viewport,
            },
        )?;
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read().map_err(|error| format!("could not read menu input: {error}"))?
        else {
            continue;
        };
        match handle_delete_key(&mut delete_state, code) {
            DeleteKey::Confirmed => return Ok(MenuChoice::Deleted(selected)),
            DeleteKey::Consumed => continue,
            DeleteKey::Ignored => {}
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = move_selection(items, selected, false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = move_selection(items, selected, true);
            }
            KeyCode::Enter => return Ok(MenuChoice::Selected(selected)),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(MenuChoice::Cancelled),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(MenuChoice::Cancelled);
            }
            _ => {}
        }
        offset = keep_visible(selected, offset, items.len(), viewport.capacity);
    }
}

fn handle_delete_key(state: &mut DeleteState, code: KeyCode) -> DeleteKey {
    match (*state, code) {
        (DeleteState::Ready, KeyCode::Char('d')) => {
            *state = DeleteState::Confirming;
            DeleteKey::Consumed
        }
        (DeleteState::Confirming, KeyCode::Enter) => DeleteKey::Confirmed,
        (DeleteState::Confirming, KeyCode::Esc) => {
            *state = DeleteState::Ready;
            DeleteKey::Consumed
        }
        (DeleteState::Confirming, _) => DeleteKey::Consumed,
        _ => DeleteKey::Ignored,
    }
}

fn draw(
    title: &str,
    subtitle: &str,
    items: &[Item],
    label_width: usize,
    state: DrawState,
) -> Result<(), String> {
    let mut output = io::stderr();
    execute!(
        output,
        MoveTo(0, 0),
        Clear(ClearType::All),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset),
        Print("\r\n"),
        Print(subtitle),
        Print("\r\n\r\n")
    )
    .map_err(|error| format!("could not draw menu: {error}"))?;

    let end = (state.offset + state.viewport.capacity).min(items.len());
    let item_width = state.viewport.width.saturating_sub(6);
    for (index, item) in items.iter().enumerate().take(end).skip(state.offset) {
        let line = item_line(item, label_width, item_width);
        if index == state.selected {
            execute!(
                output,
                SetAttribute(Attribute::Reverse),
                Print(format!("  › {line}  ")),
                SetAttribute(Attribute::Reset),
                Print("\r\n")
            )
        } else if item.selectable {
            execute!(output, Print(format!("    {line}\r\n")))
        } else {
            execute!(
                output,
                SetAttribute(Attribute::Dim),
                Print(format!("    {line}\r\n")),
                SetAttribute(Attribute::Reset)
            )
        }
        .map_err(|error| format!("could not draw menu: {error}"))?;
    }
    if items.len() > state.viewport.capacity {
        let status = match (state.offset > 0, end < items.len()) {
            (false, true) => "↓ more",
            (true, true) => "↑ more — ↓ more",
            (true, false) => "↑ more",
            (false, false) => unreachable!("long menus always have hidden items"),
        };
        execute!(output, Print(format!("\r\n    {status}\r\n")))
            .map_err(|error| format!("could not draw menu: {error}"))?;
    }
    let help = match state.delete {
        DeleteState::Confirming => "Delete this saved session?  enter delete  esc cancel",
        DeleteState::Ready => "↑/↓ move  enter continue  d delete  esc back",
        DeleteState::Disabled => "↑/↓ move  enter select  esc back",
    };
    execute!(output, Print("\r\n"), Print(help))
        .and_then(|()| output.flush())
        .map_err(|error| format!("could not draw menu: {error}"))
}

fn item_line(item: &Item, label_width: usize, available_width: usize) -> String {
    let padding = label_width.saturating_sub(UnicodeWidthStr::width(item.label.as_str()));
    truncate_width(
        &format!("{}{} {}", item.label, " ".repeat(padding), item.detail),
        available_width,
    )
}

fn truncate_width(value: &str, available_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= available_width {
        return value.to_owned();
    }
    if available_width == 0 {
        return String::new();
    }

    let content_width = available_width - 1;
    let mut width = 0;
    let mut truncated = String::new();
    for character in value.chars() {
        let character_width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if width + character_width > content_width {
            break;
        }
        truncated.push(character);
        width += character_width;
    }
    truncated.push('…');
    truncated
}

fn selectable_at_or_after(items: &[Item], initial: usize) -> usize {
    (0..items.len())
        .map(|offset| (initial + offset) % items.len())
        .find(|index| items[*index].selectable)
        .unwrap_or(initial)
}

fn move_selection(items: &[Item], selected: usize, forward: bool) -> usize {
    (1..=items.len())
        .map(|distance| {
            if forward {
                (selected + distance) % items.len()
            } else {
                (selected + items.len() - distance % items.len()) % items.len()
            }
        })
        .find(|index| items[*index].selectable)
        .unwrap_or(selected)
}

fn initial_offset(selected: usize, item_count: usize, capacity: usize) -> usize {
    selected
        .saturating_sub(capacity / 2)
        .min(item_count.saturating_sub(capacity))
}

fn keep_visible(selected: usize, offset: usize, item_count: usize, capacity: usize) -> usize {
    let offset = offset.min(item_count.saturating_sub(capacity));
    if selected < offset {
        selected
    } else if selected >= offset + capacity {
        (selected + 1 - capacity).min(item_count.saturating_sub(capacity))
    } else {
        offset
    }
}

fn visible_capacity(terminal_height: usize, item_count: usize) -> usize {
    let without_overflow = terminal_height.saturating_sub(5).max(1);
    if item_count <= without_overflow {
        item_count
    } else {
        terminal_height.saturating_sub(7).max(1).min(item_count)
    }
}

pub struct Screen;

impl Screen {
    pub fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("could not open menu: {error}"))?;
        if let Err(error) = execute!(
            io::stderr(),
            EnterAlternateScreen,
            Hide,
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(keyboard_enhancement_flags())
        ) {
            let _ = execute!(
                io::stderr(),
                PopKeyboardEnhancementFlags,
                DisableBracketedPaste,
                Show,
                LeaveAlternateScreen
            );
            let _ = disable_raw_mode();
            return Err(format!("could not open menu: {error}"));
        }
        Ok(Self)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(
            io::stderr(),
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeleteKey, DeleteState, Item, handle_delete_key, initial_offset, item_line, keep_visible,
        move_selection, selectable_at_or_after, visible_capacity,
    };
    use crossterm::event::KeyCode;

    #[test]
    fn delete_requires_confirmation_and_escape_cancels_it() {
        let mut state = DeleteState::Ready;
        assert!(matches!(
            handle_delete_key(&mut state, KeyCode::Char('d')),
            DeleteKey::Consumed
        ));
        assert!(matches!(state, DeleteState::Confirming));
        assert!(matches!(
            handle_delete_key(&mut state, KeyCode::Esc),
            DeleteKey::Consumed
        ));
        assert!(matches!(state, DeleteState::Ready));
    }

    #[test]
    fn enter_confirms_a_pending_delete() {
        let mut state = DeleteState::Confirming;
        assert!(matches!(
            handle_delete_key(&mut state, KeyCode::Enter),
            DeleteKey::Confirmed
        ));
    }

    #[test]
    fn short_lists_do_not_scroll() {
        assert_eq!(initial_offset(4, 5, 8), 0);
        assert_eq!(keep_visible(4, 0, 5, 8), 0);
    }

    #[test]
    fn initial_selection_is_centered_when_possible() {
        assert_eq!(initial_offset(20, 100, 8), 16);
        assert_eq!(initial_offset(99, 100, 8), 92);
    }

    #[test]
    fn viewport_moves_only_when_selection_crosses_an_edge() {
        assert_eq!(keep_visible(4, 4, 100, 8), 4);
        assert_eq!(keep_visible(11, 4, 100, 8), 4);
        assert_eq!(keep_visible(12, 4, 100, 8), 5);
        assert_eq!(keep_visible(3, 4, 100, 8), 3);
    }

    #[test]
    fn wrapped_selection_moves_to_the_opposite_end() {
        assert_eq!(keep_visible(0, 92, 100, 8), 0);
        assert_eq!(keep_visible(99, 0, 100, 8), 92);
    }

    #[test]
    fn read_only_items_are_skipped() {
        let items = [
            Item::read_only("Agent", "Fixed"),
            Item::new("Model", "Default"),
            Item::new("Done", "Exit"),
        ];

        assert_eq!(selectable_at_or_after(&items, 0), 1);
        assert_eq!(move_selection(&items, 1, false), 2);
        assert_eq!(move_selection(&items, 2, true), 1);
    }

    #[test]
    fn label_width_is_a_minimum_in_terminal_columns() {
        assert_eq!(
            item_line(&Item::new("name", "meta"), 8, 80),
            "name     meta"
        );
        assert_eq!(item_line(&Item::new("好", "meta"), 4, 80), "好   meta");
        assert_eq!(
            item_line(&Item::new("a longer name", "meta"), 4, 80),
            "a longer name meta"
        );
    }

    #[test]
    fn viewport_uses_available_terminal_height() {
        assert_eq!(visible_capacity(12, 20), 5);
        assert_eq!(visible_capacity(40, 100), 33);
        assert_eq!(visible_capacity(40, 4), 4);
        assert_eq!(visible_capacity(3, 20), 1);
    }

    #[test]
    fn long_items_are_kept_to_one_terminal_row() {
        assert_eq!(
            item_line(&Item::new("model", "long metadata"), 5, 12),
            "model long …"
        );
    }
}
