//! Minimal markdown renderer for chat transcripts.
//!
//! Line-based: headings, fenced code blocks, bullet/numbered lists, block
//! quotes, and inline `code` / **bold** spans. Deliberately small; a full
//! document engine (tables, LaTeX) comes later.

use gpui::{FontWeight, HighlightStyle, StyledText, div, prelude::*, px, relative};

use crate::theme::{Theme, to_hsla};

#[derive(Debug, PartialEq)]
enum Block {
    Heading(u8, String),
    Paragraph(String),
    Bullet(String),
    Numbered(String, String),
    Quote(String),
    Code { lang: String, body: String },
    Rule,
}

fn parse(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = source.lines().peekable();
    let mut paragraph = String::new();

    let flush = |paragraph: &mut String, blocks: &mut Vec<Block>| {
        if !paragraph.trim().is_empty() {
            blocks.push(Block::Paragraph(std::mem::take(paragraph).trim().to_string()));
        } else {
            paragraph.clear();
        }
    };

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            flush(&mut paragraph, &mut blocks);
            let lang = rest.trim().to_string();
            let mut body = String::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                body.push_str(code_line);
                body.push('\n');
            }
            while body.ends_with('\n') {
                body.pop();
            }
            blocks.push(Block::Code { lang, body });
        } else if trimmed.starts_with('#') {
            flush(&mut paragraph, &mut blocks);
            let level = trimmed.chars().take_while(|c| *c == '#').count().min(6) as u8;
            let text = trimmed[level as usize..].trim().to_string();
            blocks.push(Block::Heading(level, text));
        } else if trimmed == "---" || trimmed == "***" {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "))
        {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Bullet(item.trim().to_string()));
        } else if let Some((number, item)) = split_numbered(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Numbered(number, item));
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(rest.to_string()));
        } else if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed);
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn split_numbered(line: &str) -> Option<(String, String)> {
    let dot = line.find(". ")?;
    if dot == 0 || dot > 3 || !line[..dot].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((line[..dot].to_string(), line[dot + 2..].trim().to_string()))
}

/// Inline spans: strip `code`, **bold**, *italic* markers and return the
/// plain text plus highlight ranges (byte offsets into the plain text).
fn inline_spans(source: &str) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let mut plain = String::with_capacity(source.len());
    let mut highlights = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            if let Some(end) = source[i + 1..].find('`') {
                let content = &source[i + 1..i + 1 + end];
                let start = plain.len();
                plain.push_str(content);
                highlights.push((
                    start..plain.len(),
                    HighlightStyle {
                        color: Some(to_hsla(Theme::CODE_TEXT)),
                        background_color: Some(to_hsla(Theme::INLINE_CODE_BG)),
                        ..Default::default()
                    },
                ));
                i += end + 2;
                continue;
            }
        }
        if bytes[i..].starts_with(b"**") {
            if let Some(end) = source[i + 2..].find("**") {
                let content = &source[i + 2..i + 2 + end];
                let start = plain.len();
                plain.push_str(content);
                highlights.push((
                    start..plain.len(),
                    HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                ));
                i += end + 4;
                continue;
            }
        }
        // Advance one full UTF-8 character.
        let ch_len = source[i..].chars().next().map_or(1, char::len_utf8);
        plain.push_str(&source[i..i + ch_len]);
        i += ch_len;
    }
    (plain, highlights)
}

fn styled_line(source: &str, window: &gpui::Window) -> StyledText {
    let (plain, highlights) = inline_spans(source);
    let style = window.text_style();
    StyledText::new(plain).with_default_highlights(&style, highlights)
}

/// Render markdown into a column of GPUI elements.
pub fn render(source: &str, window: &gpui::Window) -> impl IntoElement {
    let blocks = parse(source);
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .children(blocks.into_iter().map(|block| match block {
            Block::Heading(level, text) => {
                let size = match level {
                    1 => px(19.0),
                    2 => px(17.0),
                    _ => px(15.0),
                };
                div()
                    .text_size(size)
                    .font_weight(FontWeight::BOLD)
                    .text_color(Theme::HEADING)
                    .mt_1()
                    .child(styled_line(&text, window))
                    .into_any_element()
            }
            Block::Paragraph(text) => div()
                .line_height(relative(1.5))
                .child(styled_line(&text, window))
                .into_any_element(),
            Block::Bullet(text) => div()
                .flex()
                .flex_row()
                .gap_2()
                .child(div().text_color(Theme::ACCENT).child("•"))
                .child(
                    div()
                        .flex_1()
                        .line_height(relative(1.5))
                        .child(styled_line(&text, window)),
                )
                .into_any_element(),
            Block::Numbered(number, text) => div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    div()
                        .text_color(Theme::ACCENT)
                        .child(format!("{number}.")),
                )
                .child(
                    div()
                        .flex_1()
                        .line_height(relative(1.5))
                        .child(styled_line(&text, window)),
                )
                .into_any_element(),
            Block::Quote(text) => div()
                .border_l_2()
                .border_color(Theme::TEXT_DIM)
                .pl_2()
                .text_color(Theme::TEXT_DIM)
                .child(styled_line(&text, window))
                .into_any_element(),
            Block::Code { lang, body } => div()
                .flex()
                .flex_col()
                .bg(Theme::CODE_BG)
                .rounded_md()
                .my_0p5()
                .child(
                    div()
                        .px_3()
                        .pt_1p5()
                        .text_size(px(11.0))
                        .text_color(Theme::TEXT_DIM)
                        .when(lang.is_empty(), |el| el.invisible().h_0().pt_0())
                        .child(lang.clone()),
                )
                .child(
                    div()
                        .px_3()
                        .pb_2()
                        .pt_1()
                        .font_family(Theme::FONT_MONO)
                        .text_size(px(13.0))
                        .text_color(Theme::CODE_TEXT)
                        .child(body),
                )
                .into_any_element(),
            Block::Rule => div()
                .h(px(1.0))
                .my_1()
                .bg(Theme::PANEL_BORDER)
                .into_any_element(),
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks() {
        let blocks = parse("# Title\n\nBody text\n\n- item\n\n```rust\nfn main() {}\n```");
        assert_eq!(blocks[0], Block::Heading(1, "Title".into()));
        assert_eq!(blocks[1], Block::Paragraph("Body text".into()));
        assert_eq!(blocks[2], Block::Bullet("item".into()));
        assert_eq!(
            blocks[3],
            Block::Code {
                lang: "rust".into(),
                body: "fn main() {}".into()
            }
        );
    }

    #[test]
    fn inline_code_and_bold() {
        let (plain, highlights) = inline_spans("use `cargo` and **run** it");
        assert_eq!(plain, "use cargo and run it");
        assert_eq!(highlights.len(), 2);
        assert_eq!(highlights[0].0, 4..9);
        assert_eq!(highlights[1].0, 14..17);
    }
}
