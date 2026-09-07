//! Markdown renderer for chat transcripts.
//!
//! Line-based and streaming-tolerant: headings, fenced code with lightweight
//! syntax highlighting, nested and task lists, tables, block quotes,
//! mathematical notation, links, and the common inline spans. Deliberately
//! self-contained, but shaped so a half-finished response still reads well.

use gpui::{
    FontWeight, HighlightStyle, InteractiveText, SharedString, StrikethroughStyle, StyledText,
    UnderlineStyle, div, prelude::*, px, relative,
};
use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};

use crate::text_selection::{self, TextSelection};
use crate::theme::{Theme, to_hsla};

#[derive(Debug, PartialEq)]
enum Block {
    Heading(u8, String),
    Paragraph(String),
    Reasoning(String),
    Bullet {
        depth: usize,
        text: String,
        task: Option<bool>,
    },
    Numbered {
        depth: usize,
        number: String,
        text: String,
    },
    Quote(Vec<String>),
    Code {
        lang: String,
        body: String,
    },
    Mermaid(String),
    HtmlPreview(String),
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Math(String),
    Rule,
}

fn indent_depth(line: &str) -> usize {
    let spaces = line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum::<usize>();
    (spaces / 2).min(4)
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
        let depth = indent_depth(line);

        if let Some(mut content) = jcode_render_core::reasoning_line_content(trimmed) {
            // Rendered history embeds the terminal's escaped reasoning lines.
            // Decode only that explicit format, before inline emphasis sees
            // its escaped asterisks. Fenced code is consumed separately below.
            flush(&mut paragraph, &mut blocks);
            loop {
                let mut ahead = lines.clone();
                let mut blank_lines = 0;
                while ahead.peek().is_some_and(|line| line.trim().is_empty()) {
                    ahead.next();
                    blank_lines += 1;
                }
                let Some(next) = ahead
                    .next()
                    .and_then(jcode_render_core::reasoning_line_content)
                else {
                    break;
                };
                for _ in 0..=blank_lines {
                    lines.next();
                    content.push('\n');
                }
                content.push_str(&next);
            }
            blocks.push(Block::Reasoning(content));
        } else if trimmed == "$$" || trimmed == "\\[" {
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
            blocks.push(Block::Math(body));
        } else if trimmed.starts_with("$$") && trimmed.ends_with("$$") && trimmed.len() > 4 {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Math(
                trimmed[2..trimmed.len() - 2].trim().to_string(),
            ));
        } else if trimmed.starts_with("\\[") && trimmed.ends_with("\\]") && trimmed.len() > 4 {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Math(
                trimmed[2..trimmed.len() - 2].trim().to_string(),
            ));
        } else if let Some(rest) = trimmed
            .strip_prefix("```")
            .or_else(|| trimmed.strip_prefix("~~~"))
        {
            flush(&mut paragraph, &mut blocks);
            let lang = rest.trim().to_string();
            let mut body = String::new();
            let marker = trimmed.chars().next().unwrap_or('`');
            let mut closed = false;
            for code_line in lines.by_ref() {
                let t = code_line.trim();
                if t.len() >= 3 && t.chars().all(|c| c == marker) {
                    closed = true;
                    break;
                }
                body.push_str(code_line);
                body.push('\n');
            }
            while body.ends_with('\n') {
                body.pop();
            }
            if lang.eq_ignore_ascii_case("html-preview") && closed {
                blocks.push(Block::HtmlPreview(body));
            } else if lang.eq_ignore_ascii_case("mermaid") || lang.eq_ignore_ascii_case("mmd") {
                blocks.push(Block::Mermaid(body));
            } else {
                blocks.push(Block::Code { lang, body });
            }
        } else if lines.peek().is_some_and(|next| is_setext_rule(next.trim())) {
            flush(&mut paragraph, &mut blocks);
            let level = if lines
                .next()
                .is_some_and(|rule| rule.trim().starts_with('='))
            {
                1
            } else {
                2
            };
            blocks.push(Block::Heading(level, trimmed.trim().to_string()));
        } else if is_heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            let level = trimmed.chars().take_while(|c| *c == '#').count().min(6) as u8;
            let text = trimmed[level as usize..].trim().to_string();
            blocks.push(Block::Heading(level, text));
        } else if is_rule(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
        } else if is_table_row(trimmed)
            && lines.peek().map(|l| is_table_divider(l.trim())) == Some(true)
        {
            flush(&mut paragraph, &mut blocks);
            let header = table_cells(trimmed);
            lines.next();
            let mut rows = Vec::new();
            while let Some(next) = lines.peek() {
                let next = next.trim();
                if !is_table_row(next) {
                    break;
                }
                rows.push(table_cells(next));
                lines.next();
            }
            blocks.push(Block::Table { header, rows });
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
            .or_else(|| trimmed.strip_prefix("• "))
        {
            flush(&mut paragraph, &mut blocks);
            let (task, text) = split_task(item.trim());
            blocks.push(Block::Bullet { depth, text, task });
        } else if let Some((number, item)) = split_numbered(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Numbered {
                depth,
                number,
                text: item,
            });
        } else if let Some(rest) = trimmed
            .strip_prefix("> ")
            .or_else(|| if trimmed == ">" { Some("") } else { None })
        {
            flush(&mut paragraph, &mut blocks);
            let mut quoted = vec![rest.to_string()];
            while let Some(next) = lines.peek() {
                let t = next.trim_start();
                if let Some(more) = t.strip_prefix("> ") {
                    quoted.push(more.to_string());
                } else if t == ">" {
                    quoted.push(String::new());
                } else {
                    break;
                }
                lines.next();
            }
            blocks.push(Block::Quote(quoted));
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

fn is_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(' ')
}

fn is_rule(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 3
        && (line.chars().all(|c| c == '-')
            || line.chars().all(|c| c == '*')
            || line.chars().all(|c| c == '_'))
}

fn is_setext_rule(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 3 && (line.chars().all(|c| c == '=') || line.chars().all(|c| c == '-'))
}

fn is_table_row(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('|') && line.len() > 1 && line.matches('|').count() >= 2
}

fn is_table_divider(line: &str) -> bool {
    is_table_row(line)
        && table_cells(line)
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':' || c == ' '))
}

fn table_cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let inner = line
        .strip_prefix('|')
        .unwrap_or(line)
        .strip_suffix('|')
        .unwrap_or_else(|| line.strip_prefix('|').unwrap_or(line));
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

fn split_task(item: &str) -> (Option<bool>, String) {
    for (prefix, state) in [
        ("[ ] ", false),
        ("[x] ", true),
        ("[X] ", true),
        ("[-] ", true),
    ] {
        if let Some(rest) = item.strip_prefix(prefix) {
            return (Some(state), rest.trim().to_string());
        }
    }
    (None, item.to_string())
}

fn split_numbered(line: &str) -> Option<(String, String)> {
    let dot = line.find(". ").or_else(|| line.find(") "))?;
    if dot == 0 || dot > 3 || !line[..dot].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((line[..dot].to_string(), line[dot + 2..].trim().to_string()))
}

#[derive(Debug, PartialEq)]
struct Inline {
    plain: String,
    code_ranges: Vec<std::ops::Range<usize>>,
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    links: Vec<(std::ops::Range<usize>, String)>,
}

/// Inline spans: strip `code`, **bold**, *italic*, ~~strike~~ and link syntax,
/// returning plain text plus highlight ranges (byte offsets into the plain
/// text) and any link targets.
fn inline_spans(source: &str) -> Inline {
    // Same-binary control is restricted to offline profiling, never live sessions.
    static SCALAR: LazyLock<bool> = LazyLock::new(|| {
        (crate::harness::screenshot_mode() || cfg!(test))
            && std::env::var("JCODE_DESKTOP_SCREENSHOT_SCALAR_INLINE").as_deref() == Ok("1")
    });
    if *SCALAR {
        inline_spans_impl::<false>(source)
    } else {
        inline_spans_impl::<true>(source)
    }
}

fn inline_spans_impl<const BULK_TEXT: bool>(source: &str) -> Inline {
    let mut plain = String::with_capacity(source.len());
    let mut code_ranges = Vec::new();
    let mut highlights = Vec::new();
    let mut links = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;

    let code_style = HighlightStyle {
        color: Some(to_hsla(Theme::global().CODE_TEXT)),
        background_color: Some(to_hsla(Theme::global().INLINE_CODE_BG)),
        ..Default::default()
    };

    while i < bytes.len() {
        if BULK_TEXT {
            // Only these ASCII bytes can begin syntax handled below (h starts
            // a bare http(s) URL). Copy ordinary prose in one run instead of
            // constructing/checking every highlight style for every character.
            // ASCII delimiters cannot occur inside a UTF-8 continuation byte.
            let count = bytes[i..]
                .iter()
                .position(|byte| {
                    matches!(
                        byte,
                        b'\\' | b'`' | b'$' | b'[' | b'<' | b'h' | b'~' | b'*' | b'_'
                    )
                })
                .unwrap_or(bytes.len() - i);
            if count > 0 {
                plain.push_str(&source[i..i + count]);
                i += count;
                continue;
            }
        }
        // Inline math delimiters \( ... \) come before the generic escape
        // rule, which would otherwise eat the opening parenthesis.
        if source[i..].starts_with("\\(") {
            if let Some(end) = source[i + 2..].find("\\)") {
                let content = latex_to_text(&source[i + 2..i + 2 + end]);
                let start = plain.len();
                plain.push_str(&content);
                highlights.push((
                    start..plain.len(),
                    HighlightStyle {
                        color: Some(to_hsla(Theme::global().CODE_TEXT)),
                        ..Default::default()
                    },
                ));
                i += end + 4;
                continue;
            }
        }
        // Escapes: \* keeps the literal character.
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_punctuation() {
            plain.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let ticks = source[i..].chars().take_while(|c| *c == '`').count();
            let fence = "`".repeat(ticks);
            if let Some(end) = source[i + ticks..].find(&fence) {
                let content = source[i + ticks..i + ticks + end].trim();
                let start = plain.len();
                plain.push_str(content);
                code_ranges.push(start..plain.len());
                highlights.push((start..plain.len(), code_style));
                i += ticks + end + ticks;
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
                        color: Some(to_hsla(Theme::global().CODE_TEXT)),
                        ..Default::default()
                    },
                ));
                i += end + 2;
                continue;
            }
        }
        // Links: [label](url) and bare <url>.
        if bytes[i] == b'[' {
            if let Some(close) = source[i..].find("](") {
                let label_end = i + close;
                if let Some(paren) = source[label_end + 2..].find(')') {
                    let label = &source[i + 1..label_end];
                    let url = source[label_end + 2..label_end + 2 + paren].trim();
                    let nested = inline_spans_impl::<BULK_TEXT>(label);
                    let start = plain.len();
                    plain.push_str(&nested.plain);
                    code_ranges.extend(
                        nested
                            .code_ranges
                            .into_iter()
                            .map(|range| start + range.start..start + range.end),
                    );
                    for (range, style) in nested.highlights {
                        highlights.push((start + range.start..start + range.end, style));
                    }
                    let range = start..plain.len();
                    highlights.push((range.clone(), link_style()));
                    links.push((range, url.to_string()));
                    i = label_end + 3 + paren;
                    continue;
                }
            }
        }
        if bytes[i] == b'<' {
            if let Some(close) = source[i..].find('>') {
                let inner = &source[i + 1..i + close];
                if inner.starts_with("http://") || inner.starts_with("https://") {
                    let start = plain.len();
                    plain.push_str(inner);
                    let range = start..plain.len();
                    highlights.push((range.clone(), link_style()));
                    links.push((range, inner.to_string()));
                    i += close + 1;
                    continue;
                }
            }
        }
        if source[i..].starts_with("http://") || source[i..].starts_with("https://") {
            let end = source[i..]
                .find(|c: char| c.is_whitespace())
                .map_or(source.len(), |p| i + p);
            let url = source[i..end].trim_end_matches(['.', ',', ')', ']', '!', '?', ';', ':']);
            let start = plain.len();
            plain.push_str(url);
            let range = start..plain.len();
            highlights.push((range.clone(), link_style()));
            links.push((range, url.to_string()));
            i += url.len();
            continue;
        }
        if bytes[i..].starts_with(b"~~") {
            if let Some(end) = source[i + 2..].find("~~") {
                let nested = inline_spans_impl::<BULK_TEXT>(&source[i + 2..i + 2 + end]);
                let start = plain.len();
                plain.push_str(&nested.plain);
                code_ranges.extend(
                    nested
                        .code_ranges
                        .into_iter()
                        .map(|range| start + range.start..start + range.end),
                );
                for (range, style) in nested.highlights {
                    highlights.push((start + range.start..start + range.end, style));
                }
                for (range, url) in nested.links {
                    links.push((start + range.start..start + range.end, url));
                }
                highlights.push((
                    start..plain.len(),
                    HighlightStyle {
                        strikethrough: Some(StrikethroughStyle {
                            thickness: px(1.0),
                            color: Some(to_hsla(Theme::global().TEXT_DIM)),
                        }),
                        color: Some(to_hsla(Theme::global().TEXT_DIM)),
                        ..Default::default()
                    },
                ));
                i += end + 4;
                continue;
            }
        }
        let strong_markers: [(&str, HighlightStyle); 3] = [
            (
                "***",
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    font_style: Some(gpui::FontStyle::Italic),
                    ..Default::default()
                },
            ),
            (
                "**",
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ),
            (
                "__",
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ),
        ];
        let mut matched_strong = false;
        for (marker, style) in strong_markers {
            if !source[i..].starts_with(marker) {
                continue;
            }
            // Unterminated emphasis falls through and prints literally, so a
            // response still reads correctly mid-stream.
            if let Some(end) = source[i + marker.len()..].find(marker) {
                let nested = inline_spans_impl::<BULK_TEXT>(
                    &source[i + marker.len()..i + marker.len() + end],
                );
                let start = plain.len();
                plain.push_str(&nested.plain);
                code_ranges.extend(
                    nested
                        .code_ranges
                        .into_iter()
                        .map(|range| start + range.start..start + range.end),
                );
                for (range, nested_style) in nested.highlights {
                    highlights.push((start + range.start..start + range.end, nested_style));
                }
                for (range, url) in nested.links {
                    links.push((start + range.start..start + range.end, url));
                }
                highlights.push((start..plain.len(), style));
                i += end + marker.len() * 2;
            } else {
                plain.push_str(marker);
                i += marker.len();
            }
            matched_strong = true;
            break;
        }
        if matched_strong {
            continue;
        }
        if bytes[i] == b'*' || bytes[i] == b'_' {
            let marker = bytes[i] as char;
            let boundary_ok = marker != '_'
                || i == 0
                || !source[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric());
            if boundary_ok {
                if let Some(end) = source[i + 1..].find(marker) {
                    let inner = &source[i + 1..i + 1 + end];
                    if !inner.is_empty() && !inner.starts_with(' ') {
                        let nested = inline_spans_impl::<BULK_TEXT>(inner);
                        let start = plain.len();
                        plain.push_str(&nested.plain);
                        code_ranges.extend(
                            nested
                                .code_ranges
                                .into_iter()
                                .map(|range| start + range.start..start + range.end),
                        );
                        for (range, style) in nested.highlights {
                            highlights.push((start + range.start..start + range.end, style));
                        }
                        for (range, url) in nested.links {
                            links.push((start + range.start..start + range.end, url));
                        }
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
            }
        }
        // Advance one full UTF-8 character.
        let ch_len = source[i..].chars().next().map_or(1, char::len_utf8);
        plain.push_str(&source[i..i + ch_len]);
        i += ch_len;
    }
    Inline {
        plain,
        code_ranges,
        highlights,
        links,
    }
}

fn link_style() -> HighlightStyle {
    HighlightStyle {
        color: Some(to_hsla(Theme::global().LINK)),
        underline: Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(to_hsla(Theme::global().LINK)),
            wavy: false,
        }),
        ..Default::default()
    }
}

/// GPUI's `StyledText::with_default_highlights` requires highlight ranges to
/// be sorted and non-overlapping; nested spans (code inside bold, a link
/// inside italics) naturally produce overlapping ranges, and feeding those in
/// directly makes GPUI compute text runs that overrun the string and abort
/// the process. Flatten overlaps into disjoint, sorted segments, merging the
/// styles of every range covering each segment (inner spans first, so an
/// outer wrapper refines rather than replaces them).
pub(crate) fn flatten_highlights(
    highlights: &[(std::ops::Range<usize>, HighlightStyle)],
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let mut bounds: Vec<usize> = highlights
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    let mut flattened: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for pair in bounds.windows(2) {
        let (segment_start, segment_end) = (pair[0], pair[1]);
        let mut merged: Option<HighlightStyle> = None;
        for (range, style) in highlights {
            if range.start <= segment_start && segment_end <= range.end {
                merged = Some(match merged {
                    Some(base) => base.highlight(*style),
                    None => *style,
                });
            }
        }
        let Some(style) = merged else { continue };
        if let Some((last_range, last_style)) = flattened.last_mut()
            && last_range.end == segment_start
            && *last_style == style
        {
            last_range.end = segment_end;
            continue;
        }
        flattened.push((segment_start..segment_end, style));
    }
    flattened
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
    // Preserve the structure of commands whose arguments matter before the
    // simple command replacement pass removes braces. In particular, merely
    // stripping `\frac` and braces produced mangled output such as
    // `x=\frac-b±√b²-4ac2a`.
    let mut text = expand_latex_groups(source.trim());
    // Layout-only commands have no useful textual representation. Models
    // commonly put `\displaystyle` at the start of inline formulas, and
    // leaving it intact is especially distracting in transcript output.
    for command in ["\\displaystyle", "\\textstyle", "\\scriptstyle"] {
        text = text.replace(command, "");
    }
    for (from, to) in REPLACEMENTS {
        text = text.replace(from, to);
    }
    text = text.replace("\\left", "").replace("\\right", "");
    text = convert_scripts(&text, '^');
    text = convert_scripts(&text, '_');
    text.replace(['{', '}'], "").trim().to_string()
}

/// Expand the small set of braced TeX commands that affect how an expression
/// reads in plain text. This is intentionally not a TeX engine, but it keeps
/// common model output mathematically unambiguous.
fn expand_latex_groups(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut i = 0;
    while i < source.len() {
        let rest = &source[i..];
        if let Some(after_command) = rest.strip_prefix("\\frac") {
            let offset = i + rest.len() - after_command.len();
            if let Some((numerator, after_numerator)) = braced_group(source, offset)
                && let Some((denominator, after_denominator)) =
                    braced_group(source, after_numerator)
            {
                output.push('(');
                output.push_str(&expand_latex_groups(numerator));
                output.push_str(")/(");
                output.push_str(&expand_latex_groups(denominator));
                output.push(')');
                i = after_denominator;
                continue;
            }
        }
        if let Some(after_command) = rest.strip_prefix("\\sqrt") {
            let offset = i + rest.len() - after_command.len();
            if let Some((radicand, after_radicand)) = braced_group(source, offset) {
                output.push_str("√(");
                output.push_str(&expand_latex_groups(radicand));
                output.push(')');
                i = after_radicand;
                continue;
            }
        }
        let wrapper = ["\\mathbf", "\\mathrm", "\\boxed", "\\text"]
            .iter()
            .find_map(|command| rest.strip_prefix(command));
        if let Some(after_command) = wrapper {
            let offset = i + rest.len() - after_command.len();
            if let Some((content, after_content)) = braced_group(source, offset) {
                output.push_str(&expand_latex_groups(content));
                i = after_content;
                continue;
            }
        }
        let ch = rest.chars().next().expect("non-empty remainder");
        output.push(ch);
        i += ch.len_utf8();
    }
    output
}

fn braced_group(source: &str, mut start: usize) -> Option<(&str, usize)> {
    while source[start..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        start += source[start..].chars().next()?.len_utf8();
    }
    if !source[start..].starts_with('{') {
        return None;
    }
    let content_start = start + 1;
    let mut depth = 1;
    for (offset, ch) in source[content_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = content_start + offset;
                    return Some((&source[content_start..end], end + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn convert_scripts(source: &str, marker: char) -> String {
    let (keys, table) = if marker == '^' {
        ("0123456789+-=()niab", "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁿⁱᵃᵇ")
    } else {
        ("0123456789+-=()nia", "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₙᵢₐ")
    };
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

/// A styled inline run. Links are clickable when any are present.
fn styled_line(
    source: &str,
    selection: &gpui::Entity<TextSelection>,
    key: SharedString,
    _window: &gpui::Window,
    cx: &gpui::App,
) -> gpui::AnyElement {
    let inline = inline_spans(source);
    let mut highlights = inline.highlights.clone();
    if let Some(highlight) = selection.read(cx).highlight(&key, inline.plain.len()) {
        highlights.push(highlight);
    }
    // Resolve the base style during layout, inside the surrounding element.
    // Capturing window.text_style() here bypasses the dimmed reasoning color
    // (and heading weight), since the parent has not been laid out yet.
    let text = StyledText::new(inline.plain.clone())
        .with_highlights(flatten_highlights(&highlights))
        .with_font_family_overrides(
            inline
                .code_ranges
                .iter()
                .cloned()
                .map(|range| (range, Theme::global().FONT_MONO.into())),
        );
    let layout = text.layout().clone();
    let child = if inline.links.is_empty() {
        text.into_any_element()
    } else {
        let ranges: Vec<_> = inline
            .links
            .iter()
            .map(|(range, _)| range.clone())
            .collect();
        let urls: Vec<String> = inline.links.iter().map(|(_, url)| url.clone()).collect();
        let id: SharedString = format!("md-link-{:x}", hash(&inline.plain)).into();
        InteractiveText::new(id, text)
            .on_click(ranges, move |index, _window, cx| {
                if let Some(url) = urls.get(index) {
                    cx.open_url(url);
                }
            })
            .into_any_element()
    };
    text_selection::selectable(selection.clone(), key, inline.plain, layout, child, cx)
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone)]
struct RenderedMath {
    svg: Arc<[u8]>,
    width: f32,
    height: f32,
}

const MATH_CACHE_CAPACITY: usize = 128;
static MATH_CACHE: LazyLock<Mutex<VecDeque<(u64, RenderedMath)>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

/// Typeset display math natively and embed the glyph outlines in a crisp SVG.
fn render_math_svg(source: &str) -> Result<RenderedMath, String> {
    let key = hash(source);
    if let Ok(mut cache) = MATH_CACHE.lock()
        && let Some(index) = cache.iter().position(|(cached_key, _)| *cached_key == key)
    {
        let entry = cache.remove(index).expect("cached math entry exists");
        let rendered = entry.1.clone();
        cache.push_back(entry);
        return Ok(rendered);
    }

    let source = source.to_owned();
    let rendered = std::panic::catch_unwind(move || {
        let ast = ratex_parser::parse(&source).map_err(|error| error.to_string())?;
        let layout = ratex_layout::layout(&ast, &ratex_layout::LayoutOptions::default());
        let display_list = ratex_layout::to_display_list(&layout);
        let font_size = 20.0;
        let padding = 3.0;
        let options = ratex_svg::SvgOptions {
            font_size,
            padding,
            stroke_width: 1.0,
            embed_glyphs: true,
            font_dir: String::new(),
        };
        let width = (display_list.width * font_size + padding * 2.0) as f32;
        let height =
            ((display_list.height + display_list.depth) * font_size + padding * 2.0) as f32;
        // currentColor allows one cached SVG to follow live theme changes.
        let svg = ratex_svg::render_to_svg(&display_list, &options)
            .replace("rgba(0,0,0,1)", "currentColor");
        Ok::<_, String>(RenderedMath {
            svg: Arc::from(svg.into_bytes()),
            width: width.max(1.0),
            height: height.max(1.0),
        })
    })
    .map_err(|_| "math renderer panicked".to_string())??;

    if let Ok(mut cache) = MATH_CACHE.lock() {
        cache.push_back((key, rendered.clone()));
        while cache.len() > MATH_CACHE_CAPACITY {
            cache.pop_front();
        }
    }
    Ok(rendered)
}

fn math_block(
    source: &str,
    selection: &gpui::Entity<TextSelection>,
    key: SharedString,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui::AnyElement {
    let shell = div()
        .debug_selector(|| "md-math".into())
        .w_full()
        // Equations are transcript content, not code cards. Keep the SVG
        // transparent and let ordinary block spacing separate display math.
        .my_2()
        .text_color(Theme::global().TEXT);

    match render_math_svg(source) {
        Ok(rendered) => shell
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .child(
                gpui::svg()
                    .data(rendered.svg.as_ref())
                    .w(px(rendered.width))
                    .h(px(rendered.height))
                    .max_w_full()
                    .text_color(Theme::global().TEXT),
            )
            .into_any_element(),
        Err(_) => shell
            .text_center()
            .text_size(px(16.0))
            .child(text_selection::plain(
                selection.clone(),
                key,
                latex_to_text(source),
                window,
                cx,
            ))
            .into_any_element(),
    }
}

// --- Code highlighting -----------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Token {
    Plain,
    Keyword,
    Str,
    Comment,
    Number,
    Type,
    Punct,
}

fn token_color(token: Token) -> gpui::Rgba {
    match token {
        Token::Plain => Theme::global().CODE_TEXT,
        Token::Keyword => Theme::global().CODE_KEYWORD,
        Token::Str => Theme::global().CODE_STRING,
        Token::Comment => Theme::global().CODE_COMMENT,
        Token::Number => Theme::global().CODE_NUMBER,
        Token::Type => Theme::global().CODE_TYPE,
        Token::Punct => Theme::global().CODE_PUNCT,
    }
}

const KEYWORDS: &[&str] = &[
    "fn",
    "let",
    "mut",
    "const",
    "static",
    "struct",
    "enum",
    "impl",
    "trait",
    "pub",
    "use",
    "mod",
    "match",
    "if",
    "else",
    "for",
    "while",
    "loop",
    "return",
    "break",
    "continue",
    "in",
    "as",
    "where",
    "async",
    "await",
    "move",
    "dyn",
    "ref",
    "self",
    "Self",
    "super",
    "crate",
    "def",
    "class",
    "import",
    "from",
    "lambda",
    "pass",
    "raise",
    "try",
    "except",
    "finally",
    "with",
    "yield",
    "elif",
    "not",
    "and",
    "or",
    "None",
    "True",
    "False",
    "function",
    "var",
    "new",
    "typeof",
    "instanceof",
    "export",
    "default",
    "extends",
    "interface",
    "type",
    "public",
    "private",
    "protected",
    "void",
    "null",
    "undefined",
    "true",
    "false",
    "then",
    "fi",
    "do",
    "done",
    "esac",
    "case",
    "echo",
    "local",
    "export",
    "unset",
    "require",
];

pub(crate) fn highlight_code(
    body: &str,
    lang: &str,
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let mut highlights = Vec::new();
    if lang.eq_ignore_ascii_case("text") || lang.eq_ignore_ascii_case("txt") {
        return (body.to_string(), highlights);
    }
    let line_comment = if matches!(
        lang.to_ascii_lowercase().as_str(),
        "py" | "python" | "sh" | "bash" | "zsh" | "ruby" | "rb" | "yaml" | "yml" | "toml" | "conf"
    ) {
        "#"
    } else {
        "//"
    };

    let bytes = body.as_bytes();
    let mut i = 0;
    let push = |range: std::ops::Range<usize>,
                token: Token,
                highlights: &mut Vec<(std::ops::Range<usize>, HighlightStyle)>| {
        if token != Token::Plain {
            highlights.push((
                range,
                HighlightStyle {
                    color: Some(to_hsla(token_color(token))),
                    ..Default::default()
                },
            ));
        }
    };

    while i < bytes.len() {
        let rest = &body[i..];
        if rest.starts_with(line_comment) || rest.starts_with('#') && line_comment == "#" {
            let end = rest.find('\n').map_or(body.len(), |p| i + p);
            push(i..end, Token::Comment, &mut highlights);
            i = end;
            continue;
        }
        if rest.starts_with("/*") {
            let end = rest.find("*/").map_or(body.len(), |p| i + p + 2);
            push(i..end, Token::Comment, &mut highlights);
            i = end;
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' || bytes[i] == b'`' {
            let quote = bytes[i];
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if bytes[j] == quote {
                    j += 1;
                    break;
                }
                if bytes[j] == b'\n' && quote != b'`' {
                    break;
                }
                j += 1;
            }
            let end = j.min(body.len());
            push(i..end, Token::Str, &mut highlights);
            i = end;
            continue;
        }
        if bytes[i].is_ascii_digit() {
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || bytes[j] == b'_')
            {
                j += 1;
            }
            push(i..j, Token::Number, &mut highlights);
            i = j;
            continue;
        }
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            let word = &body[i..j];
            let token = if KEYWORDS.contains(&word) {
                Token::Keyword
            } else if word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                Token::Type
            } else {
                Token::Plain
            };
            push(i..j, token, &mut highlights);
            i = j;
            continue;
        }
        if bytes[i].is_ascii_punctuation() {
            push(i..i + 1, Token::Punct, &mut highlights);
            i += 1;
            continue;
        }
        let ch_len = body[i..].chars().next().map_or(1, char::len_utf8);
        i += ch_len;
    }
    (body.to_string(), highlights)
}

pub(crate) fn code_block(lang: &str, body: &str, window: &gpui::Window) -> gpui::AnyElement {
    let (plain, highlights) = highlight_code(body, lang);
    let line_count = plain.lines().count().max(1);
    let gutter = (1..=line_count)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut style = window.text_style();
    style.font_family = Theme::global().FONT_MONO.into();
    style.font_size = px(12.5).into();
    style.color = to_hsla(Theme::global().CODE_TEXT);
    // Multi-line blocks always get the header so copy is reachable even when
    // the fence carried no language.
    let show_header = !lang.is_empty() || line_count > 1;
    let copy_body = body.to_string();
    let copy_id: SharedString = format!("md-copy-{:x}", hash(body)).into();

    div()
        .flex()
        .flex_col()
        .my_1()
        .w_full()
        .overflow_hidden()
        .rounded_md()
        .border_1()
        .border_color(Theme::global().CODE_BORDER)
        .bg(Theme::global().CODE_BG)
        .when(show_header, |el| {
            el.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2p5()
                    .py_1()
                    .bg(Theme::global().CODE_HEADER_BG)
                    .border_b_1()
                    .border_color(Theme::global().CODE_BORDER)
                    .text_size(px(10.5))
                    .text_color(Theme::global().TEXT_DIM)
                    .font_family(Theme::global().FONT_MONO)
                    .child(if lang.is_empty() {
                        "code".to_string()
                    } else {
                        lang.to_ascii_lowercase()
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(format!(
                                "{line_count} line{}",
                                if line_count == 1 { "" } else { "s" }
                            ))
                            .child(
                                div()
                                    .id(copy_id)
                                    .debug_selector(|| "code-copy".into())
                                    .cursor_pointer()
                                    .text_color(Theme::global().TEXT_FAINT)
                                    .hover(|el| el.text_color(Theme::global().TEXT))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        move |_event, _window, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                copy_body.clone(),
                                            ));
                                        },
                                    )
                                    .child("copy"),
                            ),
                    ),
            )
        })
        .child(
            div()
                .flex()
                .flex_row()
                .px_2()
                .py_1p5()
                .gap_2p5()
                .when(line_count > 1, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .font_family(Theme::global().FONT_MONO)
                            .text_size(px(12.5))
                            .line_height(relative(1.5))
                            .text_color(Theme::global().CODE_GUTTER)
                            .text_right()
                            .child(gutter),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_height(relative(1.5))
                        .child(StyledText::new(plain).with_default_highlights(&style, highlights)),
                ),
        )
        .into_any_element()
}

#[derive(Clone)]
struct RenderedMermaid {
    native: Arc<crate::native_mermaid::NativeMermaid>,
}

const MERMAID_CACHE_CAPACITY: usize = 64;
static MERMAID_CACHE: LazyLock<Mutex<VecDeque<(u64, RenderedMermaid)>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

fn mermaid_theme(theme: &Theme) -> mermaid_rs_renderer::Theme {
    fn hex(color: gpui::Rgba) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            (color.r * 255.0).round() as u8,
            (color.g * 255.0).round() as u8,
            (color.b * 255.0).round() as u8
        )
    }
    let mut diagram = if theme.PANEL_BG.r + theme.PANEL_BG.g + theme.PANEL_BG.b < 1.5 {
        mermaid_rs_renderer::Theme::dark()
    } else {
        mermaid_rs_renderer::Theme::modern()
    };
    diagram.font_family = theme.FONT_UI.to_owned();
    diagram.font_size = 14.0;
    diagram.background = hex(theme.PANEL_BG);
    diagram.primary_color = hex(theme.USER_BG);
    diagram.primary_text_color = hex(theme.TEXT);
    diagram.primary_border_color = hex(theme.TEXT_DIM);
    diagram.text_color = hex(theme.TEXT);
    diagram.line_color = hex(theme.TEXT_DIM);
    diagram.secondary_color = hex(theme.TOOL_BG);
    diagram.tertiary_color = hex(theme.PANEL_BG);
    diagram.edge_label_background = hex(theme.PANEL_BG);
    diagram.cluster_background = hex(theme.QUOTE_BG);
    diagram.cluster_border = hex(theme.PANEL_BORDER);
    diagram.sequence_actor_fill = diagram.primary_color.clone();
    diagram.sequence_actor_border = diagram.primary_border_color.clone();
    diagram.sequence_actor_line = diagram.line_color.clone();
    diagram.sequence_note_fill = diagram.secondary_color.clone();
    diagram.sequence_note_border = diagram.primary_border_color.clone();
    diagram.sequence_activation_fill = diagram.secondary_color.clone();
    diagram.sequence_activation_border = diagram.primary_border_color.clone();
    diagram.pie_title_text_color = diagram.text_color.clone();
    diagram.pie_section_text_color = diagram.text_color.clone();
    diagram.pie_legend_text_color = diagram.text_color.clone();
    diagram
}

fn render_mermaid_scene(
    body: &str,
    theme: mermaid_rs_renderer::Theme,
) -> Result<RenderedMermaid, String> {
    // Include the effective palette, not just source. Switching desktop themes
    // must never reuse paths filled with colors from the previous theme.
    let key = hash(&format!("{body}\0{theme:?}"));
    if let Ok(mut cache) = MERMAID_CACHE.lock()
        && let Some(index) = cache.iter().position(|(cached_key, _)| *cached_key == key)
    {
        let entry = cache.remove(index).expect("cached Mermaid entry exists");
        let rendered = entry.1.clone();
        cache.push_back(entry);
        return Ok(rendered);
    }

    // Incomplete streamed model output must not take down the desktop.
    let source = body.to_owned();
    let rendered = std::panic::catch_unwind(move || {
        let options = mermaid_rs_renderer::RenderOptions {
            theme,
            ..Default::default()
        };
        let scene = mermaid_rs_renderer::render_scene(&source, options)
            .map_err(|error| error.to_string())?;
        let native = Arc::new(crate::native_mermaid::NativeMermaid::new(&scene)?);
        Ok::<_, String>(RenderedMermaid { native })
    })
    .map_err(|_| "Mermaid renderer panicked".to_string())??;

    if let Ok(mut cache) = MERMAID_CACHE.lock() {
        cache.push_back((key, rendered.clone()));
        while cache.len() > MERMAID_CACHE_CAPACITY {
            cache.pop_front();
        }
    }
    Ok(rendered)
}

#[cfg(test)]
pub(crate) fn mermaid_natural_size(body: &str) -> (f32, f32) {
    let rendered = render_mermaid_scene(body, mermaid_theme(Theme::selected()))
        .expect("valid regression diagram");
    (rendered.native.width(), rendered.native.height())
}

pub(crate) type MediaPreviewHandler =
    std::rc::Rc<dyn Fn(Arc<gpui::Image>, &mut gpui::Window, &mut gpui::App)>;

/// Paint mmdr vector primitives directly into the native transcript. Incomplete streamed
/// diagrams retain the lightweight text representation until they become
/// valid, rather than flashing an error into the transcript.
fn mermaid_diagram(
    body: &str,
    key: SharedString,
    on_preview: Option<MediaPreviewHandler>,
) -> gpui::AnyElement {
    let theme = mermaid_theme(Theme::selected());
    if let Ok(rendered) = render_mermaid_scene(body, theme.clone()) {
        return div()
            .id(key)
            .debug_selector(|| "md-mermaid".into())
            .when_some(on_preview, |el, on_preview| {
                // The existing media viewer accepts an SVG. Generate that only
                // on explicit enlargement. Inline chat never creates an image.
                let source = body.to_owned();
                el.cursor_pointer().on_click(move |_, window, cx| {
                    let options = mermaid_rs_renderer::RenderOptions {
                        theme: theme.clone(),
                        ..Default::default()
                    };
                    if let Ok(Ok(svg)) = std::panic::catch_unwind(|| {
                        mermaid_rs_renderer::render_with_options(&source, options)
                    }) {
                        let image = Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Svg,
                            svg.into_bytes(),
                        ));
                        on_preview(image, window, cx);
                    }
                    cx.stop_propagation();
                })
            })
            .my_1()
            .w_full()
            .min_w_0()
            .child(rendered.native.element())
            .into_any_element();
    }

    let mut lines = body.lines().filter_map(mermaid_display_line).peekable();
    let rows: Vec<String> = lines.by_ref().collect();
    div()
        .debug_selector(|| "md-mermaid".into())
        .flex()
        .flex_col()
        .my_1()
        .w_full()
        .overflow_hidden()
        .rounded_md()
        .border_1()
        .border_color(Theme::global().PANEL_BORDER)
        .bg(Theme::global().QUOTE_BG)
        .child(
            div()
                .px_2p5()
                .py_1()
                .border_b_1()
                .border_color(Theme::global().PANEL_BORDER)
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Theme::global().TEXT_DIM)
                .child("DIAGRAM"),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1p5()
                .px_3()
                .py_2p5()
                .children(rows.into_iter().map(|line| {
                    let connector = line.trim_start().starts_with(['→', '←', '↔']);
                    div()
                        .max_w_full()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .when(!connector, |el| {
                            el.border_1()
                                .border_color(Theme::global().CODE_BORDER)
                                .bg(Theme::global().CODE_BG)
                        })
                        .font_family(Theme::global().FONT_MONO)
                        .text_size(px(12.0))
                        .text_color(if connector {
                            Theme::global().ACCENT_MUTED
                        } else {
                            Theme::global().TEXT
                        })
                        .child(line)
                })),
        )
        .into_any_element()
}

fn mermaid_display_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("%%") || line.starts_with("%%{") {
        return None;
    }
    let lower = line.to_ascii_lowercase();
    if [
        "graph ",
        "flowchart ",
        "sequencediagram",
        "classdiagram",
        "statediagram",
        "erdiagram",
        "gantt",
        "pie",
        "journey",
        "mindmap",
        "timeline",
    ]
    .iter()
    .any(|kind| lower.starts_with(kind))
    {
        return None;
    }
    let mut text = line
        .replace("<-->", " ↔ ")
        .replace("-->", " → ")
        .replace("==>", " ⇒ ")
        .replace("-.->", " ⇢ ")
        .replace("<--", " ← ")
        .replace("->>", " → ")
        .replace("-->>", " → ")
        .replace(":::", "  ");
    for (from, to) in [
        ("[", ""),
        ("]", ""),
        ("(", ""),
        (")", ""),
        ("{", ""),
        ("}", ""),
        ("\"", ""),
    ] {
        text = text.replace(from, to);
    }
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

// --- Rendering -------------------------------------------------------------

/// Render markdown into a column of GPUI elements.
pub fn render(
    source: &str,
    row: usize,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
) -> impl IntoElement {
    render_with_style(source, row, selection, window, cx, false, None)
}

/// Render transcript media with a panel-owned lightbox callback.
pub(crate) fn render_interactive(
    source: &str,
    row: usize,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
    reasoning: bool,
    on_preview: MediaPreviewHandler,
) -> impl IntoElement {
    render_with_style(
        source,
        row,
        selection,
        window,
        cx,
        reasoning,
        Some(on_preview),
    )
}

fn render_with_style(
    source: &str,
    row: usize,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
    reasoning: bool,
    on_preview: Option<MediaPreviewHandler>,
) -> impl IntoElement {
    render_document(
        source,
        row,
        &row.to_string(),
        selection,
        window,
        cx,
        reasoning,
        on_preview,
    )
}

fn render_document(
    source: &str,
    row: usize,
    key_prefix: &str,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
    reasoning: bool,
    on_preview: Option<MediaPreviewHandler>,
) -> gpui::AnyElement {
    let blocks = parse(source);
    let mut children: Vec<gpui::AnyElement> = Vec::with_capacity(blocks.len());
    let mut previous_was_list = false;

    for (block_index, block) in blocks.into_iter().enumerate() {
        let text_key = || -> SharedString { format!("{key_prefix}-{block_index}").into() };
        let is_list = matches!(block, Block::Bullet { .. } | Block::Numbered { .. });
        let tight = is_list && previous_was_list;
        previous_was_list = is_list;

        let element = match block {
            Block::Heading(level, text) => {
                let (size, weight) = match level {
                    _ if reasoning => (px(12.0), FontWeight::SEMIBOLD),
                    1 => (px(19.0), FontWeight::BOLD),
                    2 => (px(16.5), FontWeight::BOLD),
                    3 => (px(14.5), FontWeight::SEMIBOLD),
                    _ => (px(13.5), FontWeight::SEMIBOLD),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(!reasoning, |el| el.mt_2())
                    .child(
                        div()
                            .text_size(size)
                            .font_weight(weight)
                            .text_color(if reasoning {
                                Theme::global().REASONING
                            } else {
                                Theme::global().HEADING
                            })
                            .line_height(relative(1.35))
                            .child(styled_line(&text, selection, text_key(), window, cx)),
                    )
                    .when(!reasoning && level <= 2, |el| {
                        el.child(div().h(px(1.0)).w_full().bg(Theme::global().PANEL_BORDER))
                    })
                    .into_any_element()
            }
            Block::Reasoning(text) => div()
                .debug_selector(|| "restored-reasoning".into())
                .text_size(px(12.0))
                .text_color(Theme::global().REASONING)
                .line_height(relative(1.55))
                .child(render_document(
                    &text,
                    row,
                    &format!("{key_prefix}-{block_index}"),
                    selection,
                    window,
                    cx,
                    true,
                    on_preview.clone(),
                ))
                .into_any_element(),
            Block::Paragraph(text) => div()
                .line_height(relative(1.55))
                .child(styled_line(&text, selection, text_key(), window, cx))
                .into_any_element(),
            Block::Bullet { depth, text, task } => {
                let marker = match task {
                    Some(true) => "✓".to_string(),
                    Some(false) => "○".to_string(),
                    None => match depth {
                        0 => "•".into(),
                        1 => "◦".into(),
                        _ => "▪".into(),
                    },
                };
                let marker_color = match task {
                    _ if reasoning => Theme::global().REASONING,
                    Some(true) => Theme::global().OK,
                    Some(false) => Theme::global().TEXT_DIM,
                    None => Theme::global().ACCENT_MUTED,
                };
                list_row(
                    depth,
                    marker,
                    marker_color,
                    &text,
                    selection,
                    text_key(),
                    window,
                    cx,
                    tight,
                    task,
                )
            }
            Block::Numbered {
                depth,
                number,
                text,
            } => list_row(
                depth,
                format!("{number}."),
                if reasoning {
                    Theme::global().REASONING
                } else {
                    Theme::global().ACCENT_MUTED
                },
                &text,
                selection,
                text_key(),
                window,
                cx,
                tight,
                None,
            ),
            Block::Quote(lines) => div()
                .debug_selector(|| "md-quote".into())
                .flex()
                .flex_col()
                .my_1()
                .border_l_2()
                .border_color(Theme::global().PANEL_BORDER)
                .bg(Theme::global().QUOTE_BG)
                .rounded_r_md()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_2p5()
                        .py_1()
                        .flex_1()
                        .min_w_0()
                        .text_color(Theme::global().TEXT_DIM)
                        .italic()
                        .line_height(relative(1.5))
                        .children(
                            lines
                                .iter()
                                .filter(|line| !line.trim().is_empty())
                                .enumerate()
                                .map(|(line_index, line)| {
                                    div().child(styled_line(
                                        line,
                                        selection,
                                        format!("{row}-{block_index}-{line_index}").into(),
                                        window,
                                        cx,
                                    ))
                                }),
                        ),
                )
                .into_any_element(),
            Block::Code { lang, body }
                if lang.eq_ignore_ascii_case("diff") || lang.eq_ignore_ascii_case("patch") =>
            {
                crate::diff_block::DiffBlock::new(&body, text_key()).into_any_element()
            }
            Block::Code { lang, body } => code_block(&lang, &body, window),
            Block::HtmlPreview(body) if !reasoning => {
                crate::html_preview::HtmlPreview::new(body, row, block_index).into_any_element()
            }
            Block::HtmlPreview(body) => code_block("html-preview", &body, window),
            Block::Mermaid(body) => mermaid_diagram(&body, text_key(), on_preview.clone()),
            Block::Table { header, rows } => table(header, rows, selection, text_key(), window, cx),
            Block::Math(source) => math_block(&source, selection, text_key(), window, cx),
            Block::Rule => div()
                .h(px(1.0))
                .my_2()
                .w_full()
                .bg(Theme::global().PANEL_BORDER)
                .into_any_element(),
        };
        children.push(element);
    }

    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .children(children)
        .into_any_element()
}

fn list_row(
    depth: usize,
    marker: String,
    marker_color: gpui::Rgba,
    text: &str,
    selection: &gpui::Entity<TextSelection>,
    key: SharedString,
    window: &gpui::Window,
    cx: &gpui::App,
    tight: bool,
    task: Option<bool>,
) -> gpui::AnyElement {
    div()
        .flex()
        .flex_row()
        .gap_2()
        .when(tight, |el| el.mt_0())
        .pl(px(depth as f32 * 14.0))
        .child(
            div()
                .flex_none()
                .min_w(px(14.0))
                .text_color(marker_color)
                .text_size(px(12.5))
                .line_height(relative(1.55))
                .child(marker),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .line_height(relative(1.55))
                .when(task == Some(true), |el| {
                    el.text_color(Theme::global().TEXT_DIM)
                })
                .child(styled_line(text, selection, key, window, cx)),
        )
        .into_any_element()
}

fn table(
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    selection: &gpui::Entity<TextSelection>,
    key: SharedString,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui::AnyElement {
    let columns = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let cell = |text: &str, window: &gpui::Window| {
        div()
            .flex_1()
            .min_w_0()
            .px_2p5()
            .py_1p5()
            .line_height(relative(1.45))
            .child(styled_line(
                text,
                selection,
                format!("{key}-{:x}", hash(text)).into(),
                window,
                cx,
            ))
    };
    div()
        .flex()
        .flex_col()
        .my_1()
        .w_full()
        .overflow_hidden()
        .rounded_md()
        .border_1()
        .border_color(Theme::global().PANEL_BORDER)
        .text_size(px(12.5))
        .child(
            div()
                .flex()
                .flex_row()
                .bg(Theme::global().CODE_HEADER_BG)
                .border_b_1()
                .border_color(Theme::global().PANEL_BORDER)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Theme::global().HEADING)
                .children((0..columns).map(|index| {
                    cell(header.get(index).map(String::as_str).unwrap_or(""), window)
                })),
        )
        .children(rows.into_iter().enumerate().map(|(row_index, row)| {
            div()
                .flex()
                .flex_row()
                .when(row_index % 2 == 1, |el| el.bg(Theme::global().TABLE_STRIPE))
                .text_color(Theme::global().TEXT)
                .children(
                    (0..columns).map(|index| {
                        cell(row.get(index).map(String::as_str).unwrap_or(""), window)
                    }),
                )
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_inline_matches_scalar_at_every_streaming_boundary() {
        let examples = [
            "ordinary prose without markup. café 日本語 😀 hello",
            "**bold** *italic* __strong__ snake_case ~~gone~~ ***both***",
            r"escaped \*literal\* and `code` ``a ` b`` and trailing ",
            "[**nested** `code`](https://example.com) <https://example.org> http://a.b.",
            r"math $x_2$ and \(n \to \infty\) plus [broken]( and **unfinished",
            "h http https hhttp://example.com [link] ~ _ * ` \\ $ < 🦀",
        ];
        for source in examples {
            for end in source.char_indices().map(|(i, _)| i).chain([source.len()]) {
                let prefix = &source[..end];
                assert_eq!(
                    inline_spans_impl::<true>(prefix),
                    inline_spans_impl::<false>(prefix),
                    "{prefix:?}"
                );
            }
        }
        // Deterministic mixed syntax also exercises malformed nested delimiters.
        let atoms = [
            "a", "é", "😀", "h", "*", "_", "~", "`", "\\", "$", "[", "]", "(", ")", "<", ">", " ",
            ":", "/",
        ];
        let mut seed = 7u64;
        for _ in 0..1000 {
            let mut source = String::new();
            for _ in 0..48 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                source.push_str(atoms[(seed >> 32) as usize % atoms.len()]);
            }
            assert_eq!(
                inline_spans_impl::<true>(&source),
                inline_spans_impl::<false>(&source),
                "{source:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual paired same-binary inline parsing profiler"]
    fn inline_prose_profile() {
        use std::{hint::black_box, time::Instant};
        let source = "The desktop renders a streaming response with **important details**, `code`, and [a link](https://example.com). Unicode café 日本語 🦀 remains intact. ".repeat(128);
        for trial in 0..6 {
            let run = |bulk: bool| {
                let start = Instant::now();
                for _ in 0..100 {
                    if bulk {
                        black_box(inline_spans_impl::<true>(black_box(&source)));
                    } else {
                        black_box(inline_spans_impl::<false>(black_box(&source)));
                    }
                }
                start.elapsed().as_secs_f64() * 1000.0
            };
            let (bulk, scalar) = if trial % 2 == 0 {
                let scalar = run(false);
                (run(true), scalar)
            } else {
                let bulk = run(true);
                (bulk, run(false))
            };
            eprintln!(
                "inline trial={trial} bytes={} iterations=100 scalar_ms={scalar:.3} bulk_ms={bulk:.3} speedup={:.2}",
                source.len(),
                scalar / bulk
            );
        }
    }

    #[test]
    fn restored_reasoning_decodes_before_markdown_emphasis() {
        let source = format!(
            "{}{}\nNormal **answer**.",
            jcode_render_core::reasoning_line_markup("**Checking top live tabs**"),
            jcode_render_core::reasoning_line_markup("Use `code` and [docs](https://example.com)."),
        );
        let blocks = parse(&source);
        assert_eq!(blocks.len(), 2);
        let Block::Reasoning(heading) = &blocks[0] else {
            panic!("missing reasoning")
        };
        let inner = parse(heading);
        let Block::Paragraph(text) = &inner[0] else {
            panic!("missing paragraph")
        };
        let inline = inline_spans(text.split_once(" Use ").unwrap().0);
        assert_eq!(inline.plain, "Checking top live tabs");
        assert_eq!(inline.highlights[0].1.font_weight, Some(FontWeight::BOLD));
        let detail = inline_spans(heading.lines().nth(1).unwrap());
        assert_eq!(detail.plain, "Use code and docs.");
        assert_eq!(&detail.plain[detail.code_ranges[0].clone()], "code");
        assert_eq!(detail.links[0].1, "https://example.com");
        assert_eq!(blocks[1], Block::Paragraph("Normal **answer**.".into()));
    }

    #[test]
    fn restored_reasoning_preserves_intentional_escapes_and_fenced_examples() {
        let source = jcode_render_core::reasoning_line_markup(r"Keep \*literal\* and C:\work.");
        let Block::Reasoning(content) = &parse(&source)[0] else {
            panic!("missing reasoning")
        };
        let inline = inline_spans(content);
        assert_eq!(inline.plain, r"Keep *literal* and C:\work.");
        assert!(inline.highlights.is_empty());
        assert_eq!(
            parse(r"\*literal\*"),
            vec![Block::Paragraph(r"\*literal\*".into())]
        );
        let fenced = format!("```text\n{source}```");
        assert_eq!(
            parse(&fenced),
            vec![Block::Code {
                lang: "text".into(),
                body: source.trim_end_matches('\n').into(),
            }]
        );
    }

    #[test]
    fn restored_reasoning_preserves_multiline_markdown_blocks() {
        let original = "## Plan\n\n- Read `code`\n- Test\n\n```rust\nlet x = 1;\n```";
        let encoded: String = original
            .lines()
            .map(jcode_render_core::reasoning_line_markup)
            .collect();
        let blocks = parse(&format!("{encoded}\nDone."));
        assert_eq!(blocks.len(), 2);
        let Block::Reasoning(decoded) = &blocks[0] else {
            panic!("missing reasoning")
        };
        assert_eq!(decoded, original);
        assert_eq!(parse(decoded), parse(original));
        assert!(matches!(&parse(decoded)[3], Block::Code { lang, .. } if lang == "rust"));
        assert_eq!(blocks[1], Block::Paragraph("Done.".into()));
    }

    struct RestoredReasoningView {
        selection: gpui::Entity<TextSelection>,
        source: String,
    }

    impl gpui::Render for RestoredReasoningView {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(render_with_style(
                &self.source,
                0,
                &self.selection,
                window,
                cx,
                false,
                None,
            ))
        }
    }

    #[gpui::test]
    fn restored_reasoning_paints_and_copies_without_transport_markup(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, vcx) = cx.add_window_view(|_, cx| RestoredReasoningView {
            selection: cx.new(TextSelection::new),
            source: format!(
                "{}\nThe **answer**.",
                jcode_render_core::reasoning_line_markup("**Checking top live tabs**")
            ),
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("restored-reasoning").is_some());
        let heading = vcx.debug_bounds("selectable-text-0-0-0").unwrap();
        let answer = vcx.debug_bounds("selectable-text-0-1").unwrap();
        assert!(answer.top() >= heading.bottom());
        for (bounds, expected) in [(heading, "Checking top live tabs"), (answer, "The answer.")] {
            vcx.simulate_event(gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: bounds.center(),
                modifiers: gpui::Modifiers::default(),
                click_count: 4,
                first_mouse: false,
            });
            let selection = view.read_with(vcx, |view, _| view.selection.clone());
            vcx.update(|_, cx| selection.update(cx, |selection, cx| selection.copy(cx)));
            let copied = vcx
                .update(|_, cx| cx.read_from_clipboard())
                .and_then(|item| item.text());
            assert_eq!(copied.as_deref(), Some(expected));
        }
    }

    /// Nested inline spans produce overlapping highlight ranges. GPUI aborts
    /// the whole process in debug builds when highlight ranges overlap or run
    /// past the text, so the exact invariant `compute_runs` needs is checked
    /// here: sorted, disjoint, in-bounds segments that cover every styled
    /// byte on char boundaries.
    #[test]
    fn flattened_highlights_are_sorted_disjoint_and_in_bounds() {
        let sources = [
            "**bold with `code` inside**",
            "*italic [link](https://example.com) and `code`*",
            "~~strike **bold `code`** tail~~ plus **more *nesting* here**",
            "prefix **`n the`** suffix",
            "a **b *c `d` e* f** g [h **i**](https://example.com/x) j",
        ];
        for source in sources {
            let inline = inline_spans(source);
            let flattened = flatten_highlights(&inline.highlights);
            let mut previous_end = 0;
            for (range, _) in &flattened {
                assert!(range.start < range.end, "empty segment in {source:?}");
                assert!(
                    range.start >= previous_end,
                    "overlapping segments in {source:?}"
                );
                assert!(
                    range.end <= inline.plain.len(),
                    "segment out of bounds in {source:?}"
                );
                assert!(
                    inline.plain.is_char_boundary(range.start)
                        && inline.plain.is_char_boundary(range.end),
                    "segment splits a character in {source:?}"
                );
                previous_end = range.end;
            }
            // Every originally styled byte stays styled after flattening.
            for (range, _) in &inline.highlights {
                for offset in range.clone() {
                    assert!(
                        flattened.iter().any(|(r, _)| r.contains(&offset)),
                        "byte {offset} lost its style in {source:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn parses_blocks() {
        let blocks = parse("# Title\n\nBody text\n\n- item\n\n```rust\nfn main() {}\n```");
        assert_eq!(blocks[0], Block::Heading(1, "Title".into()));
        assert_eq!(blocks[1], Block::Paragraph("Body text".into()));
        assert_eq!(
            blocks[2],
            Block::Bullet {
                depth: 0,
                text: "item".into(),
                task: None
            }
        );
        assert_eq!(
            blocks[3],
            Block::Code {
                lang: "rust".into(),
                body: "fn main() {}".into()
            }
        );
    }

    #[test]
    fn parses_mermaid_fences_as_diagrams() {
        let blocks = parse("```mermaid\nflowchart LR\nA[Start] --> B[Done]\n```");
        assert_eq!(
            blocks,
            vec![Block::Mermaid("flowchart LR\nA[Start] --> B[Done]".into())]
        );
        assert_eq!(
            mermaid_display_line("A[Start] --> B[Done]").as_deref(),
            Some("AStart → BDone")
        );
    }

    #[test]
    fn html_previews_require_an_explicit_complete_fence() {
        assert_eq!(
            parse("```html-preview\n<h1>Hi</h1>\n```"),
            vec![Block::HtmlPreview("<h1>Hi</h1>".into())]
        );
        assert!(matches!(
            &parse("```html\n<h1>Hi</h1>\n```")[0],
            Block::Code { .. }
        ));
        assert!(matches!(
            &parse("```html-preview\n<h1>Streaming")[0],
            Block::Code { .. }
        ));
        assert!(matches!(
            &parse("```html-preview\n<h1>Hi</h1>\n~~~")[0],
            Block::Code { .. }
        ));
        assert!(matches!(
            &parse("```html-preview\n<h1>Hi</h1>\n```not-a-close")[0],
            Block::Code { .. }
        ));
        assert_eq!(
            parse("~~~html-preview\nhello\n~~~"),
            vec![Block::HtmlPreview("hello".into())]
        );
        assert!(matches!(
            &parse("<div>ordinary HTML</div>")[0],
            Block::Paragraph(_)
        ));
    }

    #[test]
    fn renders_mermaid_as_cached_native_scene() {
        let source = "flowchart TD\nA[Start] --> B[Done]";
        let light = mermaid_rs_renderer::Theme::modern();
        let first = render_mermaid_scene(source, light.clone()).expect("valid diagram");
        let second = render_mermaid_scene(source, light).expect("cached diagram");
        assert!(Arc::ptr_eq(&first.native, &second.native));
        assert!(first.native.width() > 0.0 && first.native.height() > 0.0);
        let dark =
            render_mermaid_scene(source, mermaid_rs_renderer::Theme::dark()).expect("dark diagram");
        assert!(
            !Arc::ptr_eq(&first.native, &dark.native),
            "palette is part of cache identity"
        );
    }

    #[test]
    fn mermaid_uses_semantic_desktop_palette() {
        let desktop = Theme::global();
        let diagram = mermaid_theme(desktop);
        assert_eq!(diagram.font_family, desktop.FONT_UI);
        assert_eq!(diagram.font_size, 14.0);
        assert_eq!(diagram.text_color, diagram.primary_text_color);
        assert_eq!(diagram.background, diagram.edge_label_background);
        assert_ne!(diagram.text_color, diagram.background);
    }

    #[test]
    fn parses_setext_headings() {
        assert_eq!(
            parse("Primary\n=======\n\nSecondary\n---"),
            vec![
                Block::Heading(1, "Primary".into()),
                Block::Heading(2, "Secondary".into())
            ]
        );
    }

    #[test]
    fn inline_code_font_ranges_survive_nested_styles_links_and_unicode() {
        let inline =
            inline_spans("é `one` **`two`** *`three`* ~~`four`~~ [`five`](https://example.com)");
        let code: Vec<_> = inline
            .code_ranges
            .iter()
            .map(|range| &inline.plain[range.clone()])
            .collect();
        assert_eq!(code, ["one", "two", "three", "four", "five"]);
        // GPUI overrides whole runs. Every code boundary must remain a run
        // boundary even when selection or outer emphasis overlaps it.
        let mut highlights = inline.highlights.clone();
        highlights.push((
            0..inline.plain.len(),
            HighlightStyle {
                background_color: Some(to_hsla(Theme::global().SELECTION)),
                ..Default::default()
            },
        ));
        let flattened = flatten_highlights(&highlights);
        for range in &inline.code_ranges {
            assert!(flattened.iter().any(|(run, _)| run.start == range.start));
            assert!(flattened.iter().any(|(run, _)| run.end == range.end));
        }
        assert!(
            inline_spans("plain prose and \\`escaped\\`")
                .code_ranges
                .is_empty()
        );
    }

    #[test]
    fn inline_code_and_bold() {
        let inline = inline_spans("use `cargo` and **run** it");
        assert_eq!(inline.plain, "use cargo and run it");
        assert_eq!(inline.highlights.len(), 2);
        assert_eq!(inline.highlights[0].0, 4..9);
        assert_eq!(inline.highlights[1].0, 14..17);
    }

    #[test]
    fn parses_display_math_and_does_not_misread_hashes() {
        let blocks = parse("\\[\ne^{i\\pi}+1=0\n\\]\n\n#hashtag");
        assert_eq!(blocks[0], Block::Math(r"e^{i\pi}+1=0".into()));
        assert_eq!(blocks[1], Block::Paragraph("#hashtag".into()));
    }

    #[test]
    fn renders_common_inline_math_as_unicode() {
        let inline = inline_spans("Euler: $e^{i\\pi}+1=0$ and $x_2$");
        assert_eq!(inline.plain, "Euler: e^(iπ)+1=0 and x₂");
        assert_eq!(inline.highlights.len(), 2);
    }

    #[test]
    fn renders_paren_delimited_inline_math() {
        let inline = inline_spans(r"limit \(n \to \infty\) holds");
        assert_eq!(inline.plain, "limit n → ∞ holds");
        assert_eq!(inline.highlights.len(), 1);
    }

    #[test]
    fn parses_single_line_bracket_math() {
        let blocks = parse(r"\[ e^{i\pi}+1=0 \]");
        assert_eq!(blocks[0], Block::Math(r"e^{i\pi}+1=0".into()));
    }

    #[test]
    fn preserves_unknown_latex_readably() {
        assert_eq!(latex_to_text(r"\\unknown{x}^2"), r"\\unknownx²");
        assert_eq!(latex_to_text(r"a \rightarrow \infty"), "a → ∞");
    }

    #[test]
    fn preserves_fraction_and_root_structure() {
        assert_eq!(
            latex_to_text(r"x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}"),
            "x=(-b±√(b²-4ac))/(2a)"
        );
        assert_eq!(latex_to_text(r"\frac{1}{\frac{x}{2}}"), "(1)/((x)/(2))");
    }

    #[test]
    fn transcript_equation_examples_remain_readable() {
        let source = concat!(
            r"\[E=mc^2\]",
            "\n\n",
            r"\[a^2+b^2=c^2\]",
            "\n\n",
            r"\[x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}\]",
            "\n\n",
            r"\[\int_a^b f(x)\,dx=F(b)-F(a)\]",
            "\n\n",
            r"\[e^{i\pi}+1=0\]",
        );
        assert_eq!(
            parse(source),
            vec![
                Block::Math("E=mc^2".into()),
                Block::Math("a^2+b^2=c^2".into()),
                Block::Math(r"x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}".into()),
                Block::Math(r"\int_a^b f(x)\,dx=F(b)-F(a)".into()),
                Block::Math(r"e^{i\pi}+1=0".into()),
            ]
        );
    }

    #[test]
    fn typesets_display_math_as_embedded_svg() {
        let rendered = render_math_svg(r"x=\frac{-b\pm\sqrt{b^2-4ac}}{2a}")
            .expect("common LaTeX should render");
        let svg = std::str::from_utf8(rendered.svg.as_ref()).expect("SVG is UTF-8");
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains("<path"), "glyphs should be embedded as paths");
        assert!(!svg.contains("<text"), "SVG must not depend on KaTeX fonts");
        assert!(
            svg.contains("currentColor"),
            "equations should follow the theme"
        );
        assert!(rendered.width > rendered.height);
    }

    #[test]
    fn strips_layout_commands_from_common_model_equations() {
        let quadratic =
            inline_spans(r"Quadratic: \(\displaystyle x = \frac{-b \pm \sqrt{b^2-4ac}}{2a}\)");
        assert_eq!(quadratic.plain, "Quadratic: x = (-b ± √(b²-4ac))/(2a)");

        let derivative = inline_spans(r"Derivative: \(\displaystyle \frac{d}{dx}x^n=nx^{n-1}\)");
        assert_eq!(derivative.plain, "Derivative: (d)/(dx)xⁿ=nxⁿ⁻¹");
    }

    #[test]
    fn unwraps_text_and_style_commands() {
        assert_eq!(latex_to_text(r"\boxed{\mathbf{x^2}}"), "x²");
        assert_eq!(latex_to_text(r"\text{area} = \pi r^2"), "area = π r²");
    }

    #[test]
    fn parses_nested_and_task_lists() {
        let blocks = parse("- top\n  - nested\n- [x] done\n- [ ] todo");
        assert_eq!(
            blocks[1],
            Block::Bullet {
                depth: 1,
                text: "nested".into(),
                task: None
            }
        );
        assert_eq!(
            blocks[2],
            Block::Bullet {
                depth: 0,
                text: "done".into(),
                task: Some(true)
            }
        );
        assert_eq!(
            blocks[3],
            Block::Bullet {
                depth: 0,
                text: "todo".into(),
                task: Some(false)
            }
        );
    }

    #[test]
    fn parses_tables() {
        let blocks = parse("| a | b |\n| --- | --- |\n| 1 | 2 |");
        assert_eq!(
            blocks[0],
            Block::Table {
                header: vec!["a".into(), "b".into()],
                rows: vec![vec!["1".into(), "2".into()]],
            }
        );
    }

    #[test]
    fn extracts_links() {
        let inline = inline_spans("see [docs](https://example.com) now");
        assert_eq!(inline.plain, "see docs now");
        assert_eq!(inline.links.len(), 1);
        assert_eq!(inline.links[0].1, "https://example.com");
        assert_eq!(inline.links[0].0, 4..8);
    }

    #[test]
    fn autolinks_bare_urls() {
        let inline = inline_spans("go to https://example.com.");
        assert_eq!(inline.plain, "go to https://example.com.");
        assert_eq!(inline.links[0].1, "https://example.com");
    }

    #[test]
    fn strikethrough_and_multiline_quote() {
        let inline = inline_spans("~~gone~~ here");
        assert_eq!(inline.plain, "gone here");
        let blocks = parse("> one\n> two\n\ntail");
        assert_eq!(blocks[0], Block::Quote(vec!["one".into(), "two".into()]));
    }

    #[test]
    fn highlights_code_keywords() {
        let (plain, highlights) = highlight_code("fn main() { let x = 1; }", "rust");
        assert_eq!(plain, "fn main() { let x = 1; }");
        assert!(!highlights.is_empty());
    }

    #[test]
    fn escapes_are_literal() {
        let inline = inline_spans(r"a \* b");
        assert_eq!(inline.plain, "a * b");
        assert!(inline.highlights.is_empty());
    }
}
