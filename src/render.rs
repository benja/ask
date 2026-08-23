use std::io::{self, Write};

use unicode_width::UnicodeWidthStr;

use crate::error::{Error, Result};

pub struct TerminalRenderer<W = io::Stdout> {
    output: W,
    pending: String,
    code: Option<CodeBlock>,
    wrote: bool,
    ends_with_newline: bool,
    list_active: bool,
    list_has_children: bool,
}

#[derive(Default)]
struct CodeBlock {
    language: String,
    lines: Vec<String>,
}

impl TerminalRenderer<io::Stdout> {
    pub fn new() -> Self {
        Self::with_writer(io::stdout())
    }
}

impl<W: Write> TerminalRenderer<W> {
    fn with_writer(output: W) -> Self {
        Self {
            output,
            pending: String::new(),
            code: None,
            wrote: false,
            ends_with_newline: false,
            list_active: false,
            list_has_children: false,
        }
    }

    pub fn push(&mut self, text: &str) -> Result<()> {
        self.pending.push_str(text);

        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline].to_owned();
            self.pending.drain(..=newline);
            self.line(&line)?;
        }

        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line)?;
        }

        if let Some(block) = self.code.take() {
            self.draw_code(block)?;
        }

        if self.wrote && !self.ends_with_newline {
            self.write("\n")?;
        }
        Ok(())
    }

    fn line(&mut self, line: &str) -> Result<()> {
        if let Some(block) = &mut self.code {
            if line.trim_start().starts_with("```") {
                let block = self.code.take().expect("code block exists");
                self.draw_code(block)
            } else {
                block.lines.push(line.to_owned());
                Ok(())
            }
        } else if let Some(language) = line.trim_start().strip_prefix("```") {
            self.list_active = false;
            self.list_has_children = false;
            self.code = Some(CodeBlock {
                language: language.trim().to_owned(),
                lines: Vec::new(),
            });
            Ok(())
        } else {
            self.draw_markdown_line(line)
        }
    }

    fn draw_markdown_line(&mut self, line: &str) -> Result<()> {
        if let Some((nested, content)) = list_item(line) {
            if nested {
                self.list_active = true;
                self.list_has_children = true;
                return self.write(&format!("    - {}\n", render_inline(content)));
            }

            if self.list_active && (self.list_has_children || content.ends_with(':')) {
                self.write("\n")?;
            }
            self.list_active = true;
            self.list_has_children = false;
            return self.write(&format!("  • {}\n", render_inline(content)));
        }

        self.list_active = false;
        self.list_has_children = false;
        self.write(&format!("{}\n", render_inline(line)))
    }

    fn draw_code(&mut self, block: CodeBlock) -> Result<()> {
        let content_width = block
            .lines
            .iter()
            .map(|line| UnicodeWidthStr::width(line.as_str()))
            .max()
            .unwrap_or(0);
        let label_width = if block.language.is_empty() {
            0
        } else {
            UnicodeWidthStr::width(block.language.as_str()) + 1
        };
        let width = content_width.max(label_width).max(1);

        if block.language.is_empty() {
            self.write(&format!("┌{}┐\n", "─".repeat(width + 2)))?;
        } else {
            self.write(&format!(
                "┌ {} {}┐\n",
                block.language,
                "─".repeat(width - UnicodeWidthStr::width(block.language.as_str()))
            ))?;
        }

        for line in block.lines {
            let padding = width - UnicodeWidthStr::width(line.as_str());
            self.write(&format!("│ {line}{} │\n", " ".repeat(padding)))?;
        }
        self.write(&format!("└{}┘\n", "─".repeat(width + 2)))
    }

    fn write(&mut self, text: &str) -> Result<()> {
        match self
            .output
            .write_all(text.as_bytes())
            .and_then(|()| self.output.flush())
        {
            Ok(()) => {
                self.wrote = true;
                self.ends_with_newline = text.ends_with('\n');
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(Error::new(
                format!("could not write output: {error}"),
                "check the output destination and try again",
            )),
        }
    }
}

fn list_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start_matches(' ');
    let content = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;
    Some((trimmed.len() != line.len(), content))
}

fn render_inline(line: &str) -> String {
    let mut rendered = String::with_capacity(line.len());
    let mut remaining = line;

    while !remaining.is_empty() {
        let code = remaining.find('`').map(|index| (index, "`"));
        let bold = remaining.find("**").map(|index| (index, "**"));
        let Some((open, marker)) = [code, bold].into_iter().flatten().min_by_key(|item| item.0)
        else {
            rendered.push_str(remaining);
            break;
        };

        rendered.push_str(&remaining[..open]);
        let after_open = &remaining[open + marker.len()..];
        let Some(close) = after_open.find(marker) else {
            rendered.push_str(&remaining[open..]);
            break;
        };

        rendered.push_str("\x1b[1m");
        rendered.push_str(&after_open[..close]);
        rendered.push_str("\x1b[0m");
        remaining = &after_open[close + marker.len()..];
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::TerminalRenderer;

    #[test]
    fn accepts_fences_split_across_chunks() {
        let mut output = Vec::new();
        let mut renderer = TerminalRenderer::with_writer(&mut output);
        renderer.push("Before\n``").unwrap();
        renderer.push("`bash\necho hi\n```\nAfter").unwrap();
        renderer.finish().unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Before\n┌ bash ───┐\n│ echo hi │\n└─────────┘\nAfter\n"
        );
    }

    #[test]
    fn renders_compact_lists_and_inline_styles() {
        let mut output = Vec::new();
        let mut renderer = TerminalRenderer::with_writer(&mut output);
        renderer
            .push("Use `rg` from the **repo root**:\n\n- `rg --files -g '*.rs'` → lists Rust files")
            .unwrap();
        renderer.finish().unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Use \x1b[1mrg\x1b[0m from the \x1b[1mrepo root\x1b[0m:\n\n  • \x1b[1mrg --files -g '*.rs'\x1b[0m → lists Rust files\n"
        );
    }

    #[test]
    fn separates_grouped_list_sections_but_keeps_children_compact() {
        let mut output = Vec::new();
        let mut renderer = TerminalRenderer::with_writer(&mut output);
        renderer
            .push(
                "- Purpose: learn quickly\n- Install:\n  - Download ask\n  - Put it on PATH\n- Usage:\n  - Ask a question\n- License: MIT",
            )
            .unwrap();
        renderer.finish().unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "  • Purpose: learn quickly\n\n  • Install:\n    - Download ask\n    - Put it on PATH\n\n  • Usage:\n    - Ask a question\n\n  • License: MIT\n"
        );
    }

    #[test]
    fn preserves_unclosed_inline_markers() {
        assert_eq!(super::render_inline("try `rg"), "try `rg");
        assert_eq!(super::render_inline("try **rg"), "try **rg");
    }

    #[test]
    fn ignores_markdown_inside_inline_code() {
        assert_eq!(
            super::render_inline("run `echo **hello**`"),
            "run \x1b[1mecho **hello**\x1b[0m"
        );
    }

    #[test]
    fn aligns_wide_characters_by_terminal_width() {
        let mut output = Vec::new();
        let mut renderer = TerminalRenderer::with_writer(&mut output);
        renderer.push("```\n好\na\n```").unwrap();
        renderer.finish().unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "┌────┐\n│ 好 │\n│ a  │\n└────┘\n"
        );
    }
}
