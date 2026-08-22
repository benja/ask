use std::io::{self, Write};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use unicode_width::UnicodeWidthStr;

const MAX_VISIBLE_ITEMS: usize = 8;
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

pub fn choose(
    title: &str,
    subtitle: &str,
    items: &[Item],
    initial: usize,
) -> Result<Choice, String> {
    choose_with_min_label_width(title, subtitle, items, initial, DEFAULT_LABEL_WIDTH)
}

pub fn choose_with_min_label_width(
    title: &str,
    subtitle: &str,
    items: &[Item],
    initial: usize,
    label_width: usize,
) -> Result<Choice, String> {
    debug_assert!(!items.is_empty());
    debug_assert!(items.iter().any(|item| item.selectable));
    let mut selected = selectable_at_or_after(items, initial.min(items.len() - 1));
    let mut offset = initial_offset(selected, items.len());
    loop {
        draw(title, subtitle, items, selected, offset, label_width)?;
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read().map_err(|error| format!("could not read menu input: {error}"))?
        else {
            continue;
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = move_selection(items, selected, false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = move_selection(items, selected, true);
            }
            KeyCode::Enter => return Ok(Choice::Selected(selected)),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(Choice::Cancelled),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Choice::Cancelled);
            }
            _ => {}
        }
        offset = keep_visible(selected, offset, items.len());
    }
}

fn draw(
    title: &str,
    subtitle: &str,
    items: &[Item],
    selected: usize,
    offset: usize,
    label_width: usize,
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

    let end = (offset + MAX_VISIBLE_ITEMS).min(items.len());
    for (index, item) in items.iter().enumerate().take(end).skip(offset) {
        if index == selected {
            execute!(
                output,
                SetAttribute(Attribute::Reverse),
                Print(format!("  › {}  ", item_line(item, label_width))),
                SetAttribute(Attribute::Reset),
                Print("\r\n")
            )
        } else if item.selectable {
            execute!(
                output,
                Print(format!("    {}\r\n", item_line(item, label_width)))
            )
        } else {
            execute!(
                output,
                SetAttribute(Attribute::Dim),
                Print(format!("    {}\r\n", item_line(item, label_width))),
                SetAttribute(Attribute::Reset)
            )
        }
        .map_err(|error| format!("could not draw menu: {error}"))?;
    }
    if items.len() > MAX_VISIBLE_ITEMS {
        let status = match (offset > 0, end < items.len()) {
            (false, true) => "↓ more",
            (true, true) => "↑ more — ↓ more",
            (true, false) => "↑ more",
            (false, false) => unreachable!("long menus always have hidden items"),
        };
        execute!(output, Print(format!("\r\n    {status}\r\n")))
            .map_err(|error| format!("could not draw menu: {error}"))?;
    }
    execute!(output, Print("\r\n↑/↓ move  enter select  esc back"))
        .and_then(|()| output.flush())
        .map_err(|error| format!("could not draw menu: {error}"))
}

fn item_line(item: &Item, label_width: usize) -> String {
    let padding = label_width.saturating_sub(UnicodeWidthStr::width(item.label.as_str()));
    format!("{}{} {}", item.label, " ".repeat(padding), item.detail)
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

fn initial_offset(selected: usize, item_count: usize) -> usize {
    selected
        .saturating_sub(MAX_VISIBLE_ITEMS / 2)
        .min(item_count.saturating_sub(MAX_VISIBLE_ITEMS))
}

fn keep_visible(selected: usize, offset: usize, item_count: usize) -> usize {
    if selected < offset {
        selected
    } else if selected >= offset + MAX_VISIBLE_ITEMS {
        (selected + 1 - MAX_VISIBLE_ITEMS).min(item_count.saturating_sub(MAX_VISIBLE_ITEMS))
    } else {
        offset
    }
}

pub struct Screen;

impl Screen {
    pub fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("could not open menu: {error}"))?;
        if let Err(error) = execute!(io::stderr(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(format!("could not open menu: {error}"));
        }
        Ok(Self)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Item, initial_offset, item_line, keep_visible, move_selection, selectable_at_or_after,
    };

    #[test]
    fn short_lists_do_not_scroll() {
        assert_eq!(initial_offset(4, 5), 0);
        assert_eq!(keep_visible(4, 0, 5), 0);
    }

    #[test]
    fn initial_selection_is_centered_when_possible() {
        assert_eq!(initial_offset(20, 100), 16);
        assert_eq!(initial_offset(99, 100), 92);
    }

    #[test]
    fn viewport_moves_only_when_selection_crosses_an_edge() {
        assert_eq!(keep_visible(4, 4, 100), 4);
        assert_eq!(keep_visible(11, 4, 100), 4);
        assert_eq!(keep_visible(12, 4, 100), 5);
        assert_eq!(keep_visible(3, 4, 100), 3);
    }

    #[test]
    fn wrapped_selection_moves_to_the_opposite_end() {
        assert_eq!(keep_visible(0, 92, 100), 0);
        assert_eq!(keep_visible(99, 0, 100), 92);
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
        assert_eq!(item_line(&Item::new("name", "meta"), 8), "name     meta");
        assert_eq!(item_line(&Item::new("好", "meta"), 4), "好   meta");
        assert_eq!(
            item_line(&Item::new("a longer name", "meta"), 4),
            "a longer name meta"
        );
    }
}
