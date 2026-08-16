//! Minimal markdown renderer for chat transcripts.
//!
//! Line-based: headings, fenced code blocks, lists, quotes, mathematical
//! notation, and common inline spans. Deliberately small, but tolerant of the
//! Markdown emitted by agents while a response is still streaming.

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
    Math(String),
    Rule,
}

fn parse(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = source.lines().peekable();
    let mut paragraph = String::new();

    let flush = |paragraph: &mut String, blocks: &mut Vec<Block>| {
        if !paragraph.trim().is_empty() {
            blocks.push(Block::Paragraph(
                std::mem::take(paragraph).trim().to_string(),
            ));
        } else {
            paragraph.clear();
        }
    };

    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed == "$$" || trimmed == "\\[" {
            flush(&mut paragraph, &mut blocks);
            let closing = if trimmed == "$$" { "$$" } else { "\\]" };
            let mut body = String::new();
            for math_line in lines.by_ref() {
                if math_line.trim() == closing {
                    break;
                }
                if !body.is_empty() {
                    body.push(' ');
                }
                body.push_str(math_line.trim());
            }
            blocks.push(Block::Math(latex_to_text(&body)));
        } else if trimmed.starts_with("$$") && trimmed.ends_with("$$") && trimmed.len() > 4 {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Math(latex_to_text(
                trimmed[2..trimmed.len() - 2].trim(),
            )));
        } else if let Some(rest) = trimmed.strip_prefix("```") {
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
        } else if trimmed.starts_with("# ")
            || trimmed.starts_with("## ")
            || trimmed.starts_with("### ")
            || trimmed.starts_with("#### ")
            || trimmed.starts_with("##### ")
            || trimmed.starts_with("###### ")
        {
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
        if bytes[i] == b'$' && !bytes[i..].starts_with(b"$$") {
            if let Some(end) = source[i + 1..].find('$') {
                let content = latex_to_text(&source[i + 1..i + 1 + end]);
                let start = plain.len();
                plain.push_str(&content);
                highlights.push((
                    start..plain.len(),
                    HighlightStyle {
                        color: Some(to_hsla(Theme::CODE_TEXT)),
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
        if bytes[i] == b'*' {
            if let Some(end) = source[i + 1..].find('*') {
                let content = &source[i + 1..i + 1 + end];
                let start = plain.len();
                plain.push_str(content);
                highlights.push((
                    start..plain.len(),
                    HighlightStyle {
                        font_style: Some(gpui::FontStyle::Italic),
                        ..Default::default()
                    },
                ));
                i += end + 2;
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

/// Turn common LaTeX notation into readable Unicode. GPUI does not currently
/// provide a TeX layout engine, so this keeps formulas clean instead of
/// exposing commands and delimiters. Unknown commands remain visible.
fn latex_to_text(source: &str) -> String {
    const REPLACEMENTS: &[(&str, &str)] = &[
        ("\\rightarrow", "→"),
        ("\\leftarrow", "←"),
        ("\\infty", "∞"),
        ("\\notin", "∉"),
        ("\\partial", "∂"),
        ("\\approx", "≈"),
        ("\\theta", "θ"),
        ("\\alpha", "α"),
        ("\\gamma", "γ"),
        ("\\delta", "δ"),
        ("\\lambda", "λ"),
        ("\\sigma", "σ"),
        ("\\omega", "ω"),
        ("\\times", "×"),
        ("\\cdot", "·"),
        ("\\nabla", "∇"),
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\ldots", "…"),
        ("\\cdots", "⋯"),
        ("\\sqrt", "√"),
        ("\\mathbf", ""),
        ("\\mathrm", ""),
        ("\\boxed", ""),
        ("\\text", ""),
        ("\\beta", "β"),
        ("\\phi", "φ"),
        ("\\pi", "π"),
        ("\\mu", "μ"),
        ("\\pm", "±"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\neq", "≠"),
        ("\\to", "→"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\int", "∫"),
        ("\\in", "∈"),
        ("\\cup", "∪"),
        ("\\cap", "∩"),
        ("\\quad", "  "),
        ("\\,", " "),
        ("\\!", ""),
    ];
    let mut text = source.trim().to_string();
    for (from, to) in REPLACEMENTS {
        text = text.replace(from, to);
    }
    text = text.replace("\\left", "").replace("\\right", "");
    text = convert_scripts(&text, '^');
    text = convert_scripts(&text, '_');
    text.replace(['{', '}'], "")
}

fn convert_scripts(source: &str, marker: char) -> String {
    let table = if marker == '^' {
        "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁿⁱ"
    } else {
        "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₙᵢ"
    };
    let keys = "0123456789+-=()ni";
    let map = |c: char| keys.find(c).and_then(|i| table.chars().nth(i));
    let chars: Vec<char> = source.chars().collect();
    let mut output = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != marker {
            output.push(chars[i]);
            i += 1;
            continue;
        }
        let (start, end) = if chars.get(i + 1) == Some(&'{') {
            let end = chars[i + 2..]
                .iter()
                .position(|c| *c == '}')
                .map(|p| i + 2 + p);
            match end {
                Some(end) => (i + 2, end),
                None => {
                    output.push(marker);
                    i += 1;
                    continue;
                }
            }
        } else if i + 1 < chars.len() {
            (i + 1, i + 2)
        } else {
            output.push(marker);
            break;
        };
        let converted: Option<String> = chars[start..end].iter().map(|c| map(*c)).collect();
        if let Some(converted) = converted {
            output.push_str(&converted);
        } else {
            output.push(marker);
            if end - start > 1 {
                output.push('(');
            }
            output.extend(chars[start..end].iter());
            if end - start > 1 {
                output.push(')');
            }
        }
        i = if chars.get(i + 1) == Some(&'{') {
            end + 1
        } else {
            end
        };
    }
    output
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
        .children(blocks.into_iter().map(|block| {
            match block {
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
                    .child(div().text_color(Theme::ACCENT).child(format!("{number}.")))
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
                Block::Math(text) => div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(Theme::CODE_BG)
                    .text_center()
                    .text_size(px(16.0))
                    .text_color(Theme::TEXT)
                    .child(text)
                    .into_any_element(),
                Block::Rule => div()
                    .h(px(1.0))
                    .my_1()
                    .bg(Theme::PANEL_BORDER)
                    .into_any_element(),
            }
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

    #[test]
    fn parses_display_math_and_does_not_misread_hashes() {
        let blocks = parse("\\[\ne^{i\\pi}+1=0\n\\]\n\n#hashtag");
        assert_eq!(blocks[0], Block::Math("e^(iπ)+1=0".into()));
        assert_eq!(blocks[1], Block::Paragraph("#hashtag".into()));
    }

    #[test]
    fn renders_common_inline_math_as_unicode() {
        let (plain, highlights) = inline_spans("Euler: $e^{i\\pi}+1=0$ and $x_2$");
        assert_eq!(plain, "Euler: e^(iπ)+1=0 and x₂");
        assert_eq!(highlights.len(), 2);
    }

    #[test]
    fn preserves_unknown_latex_readably() {
        assert_eq!(latex_to_text(r"\\unknown{x}^2"), r"\\unknownx²");
        assert_eq!(latex_to_text(r"a \rightarrow \infty"), "a → ∞");
    }
}
