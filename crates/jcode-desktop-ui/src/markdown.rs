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
    parse_with_line_breaks(source, false)
}

fn parse_with_line_breaks(source: &str, preserve_line_breaks: bool) -> Vec<Block> {
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
                paragraph.push(if preserve_line_breaks { '\n' } else { ' ' });
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
    styled_line_with_avatar(source, selection, key, _window, cx, false)
}

// A layout-only spacer participates in native shaping/wrapping, not paragraph
// padding: later visual lines start at the original left edge. Use ASCII
// spaces because GPUI's wrapper recognizes these as leading whitespace and
// does not create a word-break opportunity between them and the first word.
// Selection and clipboard offsets exclude the prefix entirely.
const AVATAR_PREFIX: &str = "    ";

fn assistant_avatar() -> impl IntoElement {
    gpui::svg()
        .debug_selector(|| "assistant-avatar".into())
        .data(crate::accounts::logo("jcode").expect("vendored Jcode logo"))
        .size(px(20.0))
        .text_color(Theme::global().TEXT)
}

fn styled_line_with_avatar(
    source: &str,
    selection: &gpui::Entity<TextSelection>,
    key: SharedString,
    _window: &gpui::Window,
    cx: &gpui::App,
    avatar: bool,
) -> gpui::AnyElement {
    styled_line_layout(source, selection, key, _window, cx, avatar).0
}

thread_local! {
    /// Bytes of freshly streamed text still fading in, for the document being
    /// rendered. Set by the panel around a live row and consumed by the last
    /// prose block, so the tail of a streaming response eases into view.
    static STREAM_FADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The fade handed to the next inline text leaf of the final block.
    static LEAF_FADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Render `f` with the trailing `bytes` of its markdown fading in.
pub(crate) fn with_stream_fade<R>(bytes: usize, f: impl FnOnce() -> R) -> R {
    STREAM_FADE.set(bytes);
    let result = f();
    STREAM_FADE.set(0);
    LEAF_FADE.set(0);
    result
}

/// Steps of the fading tail. More steps read as a smoother gradient, while
/// each step is only a highlight run, so layout and shaping are unaffected.
const FADE_STEPS: usize = 8;

/// Alpha ramp over the trailing `fade` bytes of `text`: oldest bytes almost
/// opaque, newest nearly transparent. Ranges fall on char boundaries.
pub(crate) fn fade_tail_highlights(
    text: &str,
    fade: usize,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let fade = fade.min(text.len());
    if fade == 0 {
        return Vec::new();
    }
    let start = text.floor_char_boundary(text.len() - fade);
    let span = text.len() - start;
    let mut ranges = Vec::with_capacity(FADE_STEPS);
    let mut from = start;
    for step in 0..FADE_STEPS {
        let to = if step + 1 == FADE_STEPS {
            text.len()
        } else {
            text.floor_char_boundary(start + span * (step + 1) / FADE_STEPS)
        };
        if to > from {
            let progress = (step as f32 + 1.0) / FADE_STEPS as f32;
            ranges.push((
                from..to,
                HighlightStyle {
                    fade_out: Some(0.9 * progress * progress),
                    ..Default::default()
                },
            ));
            from = to;
        }
    }
    ranges
}

/// Retain the native layout alongside the interactive leaf, so geometry tests
/// exercise exactly the same shaping, highlighting and links as the renderer.
fn styled_line_layout(
    source: &str,
    selection: &gpui::Entity<TextSelection>,
    key: SharedString,
    _window: &gpui::Window,
    cx: &gpui::App,
    avatar: bool,
) -> (gpui::AnyElement, gpui::TextLayout) {
    let inline = inline_spans(source);
    let mut highlights = inline.highlights.clone();
    highlights.extend(fade_tail_highlights(&inline.plain, LEAF_FADE.take()));
    if let Some(highlight) = selection.read(cx).highlight(&key, inline.plain.len()) {
        highlights.push(highlight);
    }
    let prefix = if avatar { AVATAR_PREFIX } else { "" };
    let shift =
        |range: std::ops::Range<usize>| range.start + prefix.len()..range.end + prefix.len();
    let display: SharedString = format!("{prefix}{}", inline.plain).into();
    // Resolve the base style during layout, inside the surrounding element.
    // Capturing window.text_style() here bypasses the dimmed reasoning color
    // (and heading weight), since the parent has not been laid out yet.
    // Give the spacer its own monospace run. Four proportional spaces can be
    // narrower than the icon when the user changes the assistant font.
    let prefix_highlight = avatar.then(|| (0..prefix.len(), HighlightStyle::default()));
    let prefix_font = avatar.then(|| (0..prefix.len(), Theme::global().FONT_MONO.into()));
    let text = StyledText::new(display)
        .with_highlights(
            prefix_highlight.into_iter().chain(
                flatten_highlights(&highlights)
                    .into_iter()
                    .map(|(range, style)| (shift(range), style)),
            ),
        )
        .with_font_family_overrides(
            prefix_font.into_iter().chain(
                inline
                    .code_ranges
                    .iter()
                    .cloned()
                    .map(|range| (shift(range), Theme::global().FONT_MONO.into())),
            ),
        );
    let layout = text.layout().clone();
    let child = if inline.links.is_empty() {
        text.into_any_element()
    } else {
        let ranges: Vec<_> = inline
            .links
            .iter()
            .map(|(range, _)| shift(range.clone()))
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
    let child = crate::markdown_inline_code::wrap(
        child,
        layout.clone(),
        inline.code_ranges.iter().cloned().map(shift).collect(),
    );
    let selectable = text_selection::selectable_with_prefix(
        selection.clone(),
        key,
        inline.plain,
        layout.clone(),
        child,
        prefix.len(),
        cx,
    );
    let element = div()
        .relative()
        .child(selectable)
        .when(avatar, |el| {
            el.child(div().absolute().top_0().left_0().child(assistant_avatar()))
        })
        .into_any_element();
    (element, layout)
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

#[path = "markdown_syntax.rs"]
mod syntax;

pub(crate) fn highlight_code(
    body: &str,
    lang: &str,
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    syntax::highlight(body, lang)
}

pub(crate) fn code_block(lang: &str, body: &str, window: &gpui::Window) -> gpui::AnyElement {
    code_block_with_selection(lang, body, window, None)
}

fn code_block_with_selection(
    lang: &str,
    body: &str,
    window: &gpui::Window,
    selection: Option<(&gpui::Entity<TextSelection>, SharedString, &gpui::App)>,
) -> gpui::AnyElement {
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
    let mut highlights = highlights;
    if let Some((model, key, cx)) = &selection {
        if let Some(highlight) = model.read(cx).highlight(key, plain.len()) {
            highlights.push(highlight);
        }
    }
    let styled = StyledText::new(plain.clone())
        .with_default_highlights(&style, flatten_highlights(&highlights));
    let code = if let Some((model, key, cx)) = selection {
        let layout = styled.layout().clone();
        text_selection::selectable(model.clone(), key, plain, layout, styled, cx)
    } else {
        styled.into_any_element()
    };
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
                        .child(code),
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

/// Logical reading order for transcript selection without laying out offscreen rows.
/// Keys match selectable render leaves, including reasoning namespaces and padded
/// table cells. List markers and rules are decorations, not selectable text.
/// Diff/patch, rendered math, HTML previews, and Mermaid use synthetic block-key
/// entries containing their readable source: these rich/media views do not expose
/// a corresponding selectable leaf (HTML in reasoning does render as code).
pub(crate) fn selection_segments(
    source: &str,
    row: usize,
    reasoning: bool,
    prompt: bool,
) -> Vec<(SharedString, SharedString)> {
    let mut segments = Vec::new();
    collect_selection_segments(source, &row.to_string(), reasoning, prompt, &mut segments);
    segments
}

fn collect_selection_segments(
    source: &str,
    key_prefix: &str,
    reasoning: bool,
    prompt: bool,
    segments: &mut Vec<(SharedString, SharedString)>,
) {
    for (block_index, block) in parse_with_line_breaks(source, prompt)
        .into_iter()
        .enumerate()
    {
        let key = format!("{key_prefix}-{block_index}");
        match block {
            Block::Paragraph(text) => {
                let text = if reasoning {
                    reasoning_section_title(&text).unwrap_or(&text)
                } else {
                    &text
                };
                segments.push((key.into(), inline_spans(text).plain.into()));
            }
            Block::Heading(_, text) | Block::Bullet { text, .. } | Block::Numbered { text, .. } => {
                segments.push((key.into(), inline_spans(&text).plain.into()));
            }
            Block::Reasoning(text) => {
                collect_selection_segments(&text, &key, true, prompt, segments);
            }
            // Quotes currently render individual nonempty inline lines, not a
            // recursive markdown document. Preserve that exact leaf structure.
            Block::Quote(lines) => {
                for (line_index, line) in lines
                    .iter()
                    .filter(|line| !line.trim().is_empty())
                    .enumerate()
                {
                    segments.push((
                        format!("{key}-{line_index}").into(),
                        inline_spans(line).plain.into(),
                    ));
                }
            }
            Block::Table { header, rows } => {
                let columns = header
                    .len()
                    .max(rows.iter().map(Vec::len).max().unwrap_or(0));
                for (row_index, row) in std::iter::once(&header).chain(rows.iter()).enumerate() {
                    for column in 0..columns {
                        let text = row.get(column).map(String::as_str).unwrap_or("");
                        segments.push((
                            table_cell_key(&key, row_index, column),
                            inline_spans(text).plain.into(),
                        ));
                    }
                }
            }
            // Highlighting preserves code's text. Avoid syntax highlighting and
            // media rendering here, so offscreen extraction stays inexpensive.
            Block::Code { body, .. }
            | Block::HtmlPreview(body)
            | Block::Mermaid(body)
            | Block::Math(body) => {
                segments.push((key.into(), body.into()));
            }
            Block::Rule => {}
        }
    }
}

/// Header is row zero, followed by body rows. Content never participates in IDs.
fn table_cell_key(key: &str, row: usize, column: usize) -> SharedString {
    format!("{key}-{row}-{column}").into()
}

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

/// Render user prose with preserved newlines and a rounded, line-fitting fill.
pub(crate) fn render_prompt(
    source: &str,
    row: usize,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
    on_preview: MediaPreviewHandler,
    background: gpui::Rgba,
) -> gpui::AnyElement {
    render_prompt_with_prefix(
        source, row, &row.to_string(), selection, window, cx, on_preview, background,
    )
}

/// Render a duplicate prompt in its own selection namespace. Pinned overlays
/// must not reuse the transcript's logical row keys or replace its text leaves.
pub(crate) fn render_prompt_with_prefix(
    source: &str,
    row: usize,
    key_prefix: &str,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
    on_preview: MediaPreviewHandler,
    background: gpui::Rgba,
) -> gpui::AnyElement {
    render_document_with_prompt_background(
        source,
        row,
        key_prefix,
        selection,
        window,
        cx,
        false,
        Some(on_preview),
        false,
        Some(background),
    )
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
    render_interactive_with_avatar(
        source, row, selection, window, cx, reasoning, on_preview, false,
    )
}

pub(crate) fn render_interactive_with_avatar(
    source: &str,
    row: usize,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
    reasoning: bool,
    on_preview: MediaPreviewHandler,
    avatar: bool,
) -> impl IntoElement {
    render_document(
        source,
        row,
        &row.to_string(),
        selection,
        window,
        cx,
        reasoning,
        Some(on_preview),
        avatar,
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
        false,
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
    avatar: bool,
) -> gpui::AnyElement {
    render_document_with_prompt_background(
        source, row, key_prefix, selection, window, cx, reasoning, on_preview, avatar, None,
    )
}

// Providers often send reasoning section titles as standalone bold paragraphs
// rather than ATX headings. Do not promote ordinary prose or partial emphasis.
fn reasoning_section_title(text: &str) -> Option<&str> {
    let text = text.trim();
    let title = text.strip_prefix("**")?.strip_suffix("**")?;
    (!title.trim().is_empty()
        && !title.contains('\n')
        && !title.contains("**")
        && title.chars().count() <= 100)
        .then_some(title)
}

fn render_document_with_prompt_background(
    source: &str,
    row: usize,
    key_prefix: &str,
    selection: &gpui::Entity<TextSelection>,
    window: &gpui::Window,
    cx: &gpui::App,
    reasoning: bool,
    on_preview: Option<MediaPreviewHandler>,
    avatar: bool,
    prompt_background: Option<gpui::Rgba>,
) -> gpui::AnyElement {
    let blocks = if prompt_background.is_some() {
        parse_with_line_breaks(source, true)
    } else {
        parse(source)
    };
    let mut children: Vec<gpui::AnyElement> = Vec::with_capacity(blocks.len());
    let mut previous_was_list = false;
    let stream_fade = STREAM_FADE.take();
    let last_block = blocks.len().saturating_sub(1);

    for (block_index, block) in blocks.into_iter().enumerate() {
        // Only prose leaves fade. Table cells and code keep full contrast.
        let prose = matches!(
            block,
            Block::Heading(..)
                | Block::Paragraph(..)
                | Block::Bullet { .. }
                | Block::Numbered { .. }
                | Block::Quote(..)
        );
        LEAF_FADE.set(if prose && block_index == last_block { stream_fade } else { 0 });
        let block = match block {
            Block::Paragraph(ref text) if reasoning => reasoning_section_title(text)
                .map(|title| Block::Heading(3, title.to_owned()))
                .unwrap_or(block),
            block => block,
        };
        let text_key = || -> SharedString { format!("{key_prefix}-{block_index}").into() };
        let is_list = matches!(block, Block::Bullet { .. } | Block::Numbered { .. });
        let tight = is_list && previous_was_list;
        previous_was_list = is_list;

        let first_line_avatar = avatar && block_index == 0;
        let fitted_background = matches!(
            &block,
            Block::Heading(..) | Block::Paragraph(..) | Block::Reasoning(..)
        );
        let inline_text = |text: &str| {
            let (child, layout) =
                styled_line_layout(text, selection, text_key(), window, cx, first_line_avatar);
            match prompt_background {
                Some(color) => crate::prompt_background::wrap(child, layout, color),
                None => child,
            }
        };
        // Structured non-inline blocks get a separate mark, never an overlay
        // on code, table cells, diagrams, or media.
        let inline_avatar = matches!(
            &block,
            Block::Heading(..)
                | Block::Paragraph(..)
                | Block::Reasoning(..)
                | Block::Bullet { .. }
                | Block::Numbered { .. }
                | Block::Quote(..)
        );
        if first_line_avatar && !inline_avatar {
            children.push(
                div()
                    .h(px(22.0))
                    .child(assistant_avatar())
                    .into_any_element(),
            );
        }
        let element = match block {
            Block::Heading(level, text) => {
                let (size, weight) = match level {
                    _ if reasoning => (px(16.5), FontWeight::SEMIBOLD),
                    1 => (px(19.0), FontWeight::BOLD),
                    2 => (px(16.5), FontWeight::BOLD),
                    3 => (px(14.5), FontWeight::SEMIBOLD),
                    _ => (px(13.5), FontWeight::SEMIBOLD),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(reasoning, |el| el.mt_1().mb_1())
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
                            .child(inline_text(&text)),
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
                .child(render_document_with_prompt_background(
                    &text,
                    row,
                    &format!("{key_prefix}-{block_index}"),
                    selection,
                    window,
                    cx,
                    true,
                    on_preview.clone(),
                    first_line_avatar,
                    prompt_background,
                ))
                .into_any_element(),
            Block::Paragraph(text) => div()
                .line_height(relative(1.55))
                .child(inline_text(&text))
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
                    first_line_avatar,
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
                first_line_avatar,
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
                        .children({
                            let fade = LEAF_FADE.take();
                            let lines: Vec<&String> =
                                lines.iter().filter(|line| !line.trim().is_empty()).collect();
                            let last_line = lines.len().saturating_sub(1);
                            lines
                                .into_iter()
                                .enumerate()
                                .map(move |(line_index, line)| {
                                    LEAF_FADE.set(if line_index == last_line { fade } else { 0 });
                                    div().child(styled_line_with_avatar(
                                        line,
                                        selection,
                                        format!("{key_prefix}-{block_index}-{line_index}").into(),
                                        window,
                                        cx,
                                        first_line_avatar && line_index == 0,
                                    ))
                                })
                        }),
                )
                .into_any_element(),
            Block::Code { lang, body }
                if lang.eq_ignore_ascii_case("diff") || lang.eq_ignore_ascii_case("patch") =>
            {
                crate::diff_block::DiffBlock::new(&body, text_key()).into_any_element()
            }
            Block::Code { lang, body } => code_block_with_selection(
                &lang, &body, window, Some((selection, text_key(), cx)),
            ),
            Block::HtmlPreview(body) if !reasoning => {
                crate::html_preview::HtmlPreview::new(body, row, block_index).into_any_element()
            }
            Block::HtmlPreview(body) => code_block_with_selection(
                "html-preview", &body, window, Some((selection, text_key(), cx)),
            ),
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
        // Code, tables, and diagrams do not consume the fade. Never leak it.
        LEAF_FADE.set(0);
        // Structured blocks retain their native rectangular geometry. Prose
        // instead gets a contour from the actual shaped visual line widths.
        children.push(match prompt_background {
            Some(color) if !fitted_background => {
                crate::prompt_background::wrap_block(element, color)
            }
            _ => element,
        });
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
    avatar: bool,
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
                .child(styled_line_with_avatar(
                    text, selection, key, window, cx, avatar,
                )),
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
    let cell = |text: &str, row: usize, column: usize, window: &gpui::Window| {
        div()
            .flex_1()
            .min_w_0()
            .px_2p5()
            .py_1p5()
            .line_height(relative(1.45))
            .child(styled_line(
                text,
                selection,
                table_cell_key(&key, row, column),
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
                    cell(
                        header.get(index).map(String::as_str).unwrap_or(""),
                        0,
                        index,
                        window,
                    )
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
                        cell(
                            row.get(index).map(String::as_str).unwrap_or(""),
                            row_index + 1,
                            index,
                            window,
                        )
                    }),
                )
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn pinned_prompt_selection_namespace_is_independent(cx: &mut gpui::TestAppContext) {
        struct PromptCopies {
            selection: gpui::Entity<TextSelection>,
        }
        impl gpui::Render for PromptCopies {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                let preview: MediaPreviewHandler = std::rc::Rc::new(|_, _, _| {});
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(render_prompt(
                        "Full prompt",
                        4,
                        &self.selection,
                        window,
                        cx,
                        preview.clone(),
                        Theme::global().QUOTE_BG,
                    ))
                    .child(render_prompt_with_prefix(
                        "Pinned preview",
                        4,
                        "pinned-4",
                        &self.selection,
                        window,
                        cx,
                        preview,
                        Theme::global().QUOTE_BG,
                    ))
            }
        }
        let (view, vcx) = cx.add_window_view(|_, cx| PromptCopies {
            selection: cx.new(TextSelection::new),
        });
        vcx.run_until_parked();
        for (selector, expected) in [
            ("selectable-text-4-0", "Full prompt"),
            ("selectable-text-pinned-4-0", "Pinned preview"),
        ] {
            let bounds = vcx.debug_bounds(selector).expect("independent prompt leaf");
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
        assert_eq!(
            selection_segments("Full prompt", 4, false, true)[0]
                .0
                .as_ref(),
            "4-0"
        );
    }

    fn extracted(source: &str, row: usize, reasoning: bool, prompt: bool) -> Vec<(String, String)> {
        selection_segments(source, row, reasoning, prompt)
            .into_iter()
            .map(|(key, text)| (key.to_string(), text.to_string()))
            .collect()
    }

    #[test]
    fn selection_segments_preserve_leaf_order_and_plain_text() {
        let source = "# **Heading**\n\nProse [link](https://example.com)\ncontinued.\n\n- [x] `done`\n2. *next*\n\n> **quote**\n>\n> > nested\n\n***\n\n```rust\n  let café = 1;\n\t// text\n```";
        let expected = [
            ("9-0", "Heading"),
            ("9-1", "Prose link continued."),
            ("9-2", "done"),
            ("9-3", "next"),
            ("9-4-0", "quote"),
            ("9-4-1", "> nested"),
            ("9-6", "  let café = 1;\n\t// text"),
        ];
        assert_eq!(
            extracted(source, 9, false, false),
            expected.map(|(k, t)| (k.into(), t.into()))
        );
        assert_eq!(
            extracted("a\nb", 0, false, true),
            [("0-0".into(), "a\nb".into())]
        );
        assert_eq!(
            extracted("**`Title`**", 0, true, false),
            [("0-0".into(), "Title".into())]
        );
        assert!(selection_segments("\n***\n", 0, false, false).is_empty());
    }

    #[test]
    fn selection_segments_tables_are_positional_unique_and_padded() {
        let source = "| same | same |\n| --- | --- |\n| same | same | same |\n| same |";
        let segments = extracted(source, 3, false, false);
        assert_eq!(segments.len(), 9);
        let keys: std::collections::HashSet<_> = segments.iter().map(|(key, _)| key).collect();
        assert_eq!(keys.len(), segments.len());
        for (index, (key, _)) in segments.iter().enumerate() {
            assert_eq!(key, &format!("3-0-{}-{}", index / 3, index % 3));
        }
        assert_eq!(segments[2].1, "");
        assert_eq!(segments[7].1, "");
        assert_eq!(segments[8].1, "");
    }

    #[test]
    fn selection_segments_nested_reasoning_quotes_keep_their_namespace() {
        let inner: String = "> **nested**\n>\n> last"
            .lines()
            .map(jcode_render_core::reasoning_line_markup)
            .collect();
        let source: String = inner
            .lines()
            .map(jcode_render_core::reasoning_line_markup)
            .collect();
        assert_eq!(
            extracted(&source, 7, false, false),
            [
                ("7-0-0-0-0".into(), "nested".into()),
                ("7-0-0-0-1".into(), "last".into()),
            ]
        );
    }

    #[test]
    fn selection_segments_code_and_media_use_readable_bodies() {
        for lang in [
            "rust",
            "text",
            "unknown",
            "diff",
            "patch",
            "html-preview",
            "mermaid",
            "mmd",
        ] {
            let body = "  α <b> & `literal`\n\tsecond";
            let source = format!("```{lang}\n{body}\n```");
            assert_eq!(
                extracted(&source, 4, false, false),
                [("4-0".into(), body.into())]
            );
            if !matches!(lang, "diff" | "patch" | "html-preview" | "mermaid" | "mmd") {
                assert_eq!(highlight_code(body, lang).0, body);
            }
        }
        assert_eq!(
            extracted("$$x^2$$", 4, false, false),
            [("4-0".into(), "x^2".into())]
        );
    }

    #[gpui::test]
    fn selection_segments_match_rendered_leaf_keys_and_copy(cx: &mut gpui::TestAppContext) {
        let nested: String = ["> **quoted**", ">", "> second"]
            .into_iter()
            .map(jcode_render_core::reasoning_line_markup)
            .collect();
        // Separate small documents keep every tested leaf inside the test viewport.
        for source in [
            "# **Heading**\n\nProse `code`\n\n- [x] done\n2. next".to_owned(),
            "```rust\n  let x = 1;\n\t// café\n```".to_owned(),
            nested,
            "| same | same |\n| --- | --- |\n| same | same |".to_owned(),
        ] {
            let expected = selection_segments(&source, 0, false, false);
            let (view, vcx) = cx.add_window_view(|_, cx| RestoredReasoningView {
                selection: cx.new(TextSelection::new),
                source,
            });
            vcx.run_until_parked();
            for (key, text) in expected {
                let bounds = vcx
                    .debug_bounds(Box::leak(format!("selectable-text-{key}").into_boxed_str()))
                    .unwrap_or_else(|| panic!("missing rendered leaf {key}"));
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
                assert_eq!(copied.as_deref(), Some(text.as_ref()), "leaf {key}");
            }
        }
    }

    #[test]
    fn prompt_line_breaks_are_preserved_without_changing_assistant_markdown() {
        let source = "A longer first line\nShort.\n\nAnother paragraph.";
        assert_eq!(parse_with_line_breaks(source, true), vec![
            Block::Paragraph("A longer first line\nShort.".into()),
            Block::Paragraph("Another paragraph.".into()),
        ]);
        assert_eq!(parse(source), vec![
            Block::Paragraph("A longer first line Short.".into()),
            Block::Paragraph("Another paragraph.".into()),
        ]);
    }

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

    struct AvatarTextView {
        selection: gpui::Entity<TextSelection>,
        source: String,
        width: f32,
        layout: gpui::TextLayout,
        continuation: gpui::TextLayout,
    }

    impl gpui::Render for AvatarTextView {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let (first, layout) = styled_line_layout(
                &self.source,
                &self.selection,
                "first".into(),
                window,
                cx,
                true,
            );
            let (next, continuation) = styled_line_layout(
                "Subsequent streaming segment",
                &self.selection,
                "next".into(),
                window,
                cx,
                false,
            );
            self.layout = layout;
            self.continuation = continuation;
            div()
                .w(px(self.width))
                .text_size(px(14.0))
                .line_height(px(22.0))
                .flex()
                .flex_col()
                .child(first)
                .child(next)
        }
    }

    #[gpui::test]
    fn avatar_indents_only_first_visual_line_and_preserves_copy_and_links(
        cx: &mut gpui::TestAppContext,
    ) {
        let source = "[Linked](https://example.com) **bold βeta** and `code` then enough words to wrap over several visual lines without indenting the remainder of the paragraph.";
        let plain = inline_spans(source).plain;
        let (view, vcx) = cx.add_window_view(|_, cx| AvatarTextView {
            selection: cx.new(TextSelection::new),
            source: source.into(),
            width: 180.0,
            layout: gpui::TextLayout::default(),
            continuation: gpui::TextLayout::default(),
        });
        vcx.run_until_parked();
        let (layout, continuation, selection) = view.read_with(vcx, |view, _| {
            (
                view.layout.clone(),
                view.continuation.clone(),
                view.selection.clone(),
            )
        });
        let first = layout.position_for_index(AVATAR_PREFIX.len()).unwrap();
        let avatar = vcx.debug_bounds("assistant-avatar").unwrap();
        assert!(
            first.x >= avatar.right(),
            "first text must clear the icon: {first:?} {avatar:?}"
        );
        assert_eq!(
            first.y,
            layout.bounds().top(),
            "spacer must not wrap onto its own line"
        );
        assert!(
            first.x - layout.bounds().left() <= px(50.0),
            "first-line spacer must stay compact"
        );
        // GPUI gives a byte at a wrap boundary upstream affinity (the end
        // of the preceding line). Hit-test each actual following line's left
        // edge instead, and require its first glyph, not a padded gutter.
        let lines = layout.line_layouts();
        let wrapped = &lines[0];
        assert!(wrapped.wrap_boundaries.len() >= 2);
        for (line_index, boundary) in wrapped.wrap_boundaries.iter().enumerate() {
            let expected =
                wrapped.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
            let position = gpui::point(
                layout.bounds().left() + px(0.1),
                first.y + layout.line_height() * (line_index + 1) as f32 + px(5.0),
            );
            assert_eq!(
                layout.index_for_position(position),
                Ok(expected),
                "wrapped line must begin at normal margin"
            );
            assert!(
                position.y - px(5.0) >= avatar.bottom(),
                "icon must not overlap second line"
            );
        }
        assert_eq!(
            continuation.position_for_index(0).unwrap().x,
            layout.bounds().left()
        );
        let click = first + gpui::point(px(2.0), px(5.0));
        vcx.simulate_click(click, gpui::Modifiers::default());
        assert_eq!(vcx.opened_url().as_deref(), Some("https://example.com"));
        for (click_count, expected) in [(2, "Linked"), (4, plain.as_str())] {
            vcx.simulate_event(gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: click,
                modifiers: gpui::Modifiers::default(),
                click_count,
                first_mouse: false,
            });
            vcx.update(|_, cx| selection.update(cx, |selection, cx| selection.copy(cx)));
            assert_eq!(
                vcx.update(|_, cx| cx.read_from_clipboard())
                    .and_then(|item| item.text())
                    .as_deref(),
                Some(expected)
            );
        }
        // Shift-select across a soft wrap and mixed UTF-8/style runs. The
        // selected bytes must be source bytes, not the display-only prefix.
        vcx.simulate_click(click, gpui::Modifiers::default());
        let end = plain.find("code").unwrap() + "code".len();
        let end_position = layout
            .position_for_index(AVATAR_PREFIX.len() + end)
            .unwrap();
        vcx.simulate_click(
            end_position + gpui::point(px(0.1), px(5.0)),
            gpui::Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        vcx.update(|_, cx| selection.update(cx, |selection, cx| selection.copy(cx)));
        assert_eq!(
            vcx.update(|_, cx| cx.read_from_clipboard())
                .and_then(|item| item.text())
                .as_deref(),
            Some(&plain[..end])
        );
        // Repaint selection highlights, including UTF-8, link, bold and code
        // ranges shifted by the layout prefix, then resize and append live text.
        view.update(vcx, |view, cx| {
            view.width = 260.0;
            view.source.push_str(" Streamed **tail**.");
            cx.notify();
        });
        vcx.run_until_parked();
        let layout = view.read_with(vcx, |view, _| view.layout.clone());
        assert_eq!(
            layout.position_for_index(AVATAR_PREFIX.len()).unwrap().y,
            layout.bounds().top()
        );
    }

    #[gpui::test]
    fn avatar_markdown_blocks_keep_their_normal_left_margin(cx: &mut gpui::TestAppContext) {
        struct Document {
            selection: gpui::Entity<TextSelection>,
        }
        impl gpui::Render for Document {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                div().w(px(190.0)).child(render_document(
                    "# A heading that wraps across multiple visual lines\n\nThe next paragraph stays at its normal margin.\n\n- A list item", 0, "avatar-document", &self.selection, window, cx, false, None, true,
                ))
            }
        }
        let (_, vcx) = cx.add_window_view(|_, cx| Document {
            selection: cx.new(TextSelection::new),
        });
        vcx.run_until_parked();
        let first = vcx
            .debug_bounds("selectable-text-avatar-document-0")
            .unwrap();
        let next = vcx
            .debug_bounds("selectable-text-avatar-document-1")
            .unwrap();
        let avatar = vcx.debug_bounds("assistant-avatar").unwrap();
        assert_eq!(first.left(), next.left());
        assert_eq!(avatar.left(), first.left());
        assert!(next.top() >= first.bottom());
    }

    struct RestoredReasoningView {
        selection: gpui::Entity<TextSelection>,
        source: String,
    }

    #[gpui::test]
    fn fenced_code_selection_copies_source_without_gutters(cx: &mut gpui::TestAppContext) {
        cx.update(crate::text_selection::bind_keys);
        struct CodeDocument {
            selection: gpui::Entity<TextSelection>,
            source: String,
        }
        impl gpui::Render for CodeDocument {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                let selection = self.selection.clone();
                div()
                    .w(px(500.))
                    .key_context(TextSelection::key_context())
                    .track_focus(&selection.read(cx).focus_handle())
                    .on_action(move |_: &text_selection::Copy, _, cx| {
                        selection.update(cx, |selection, cx| selection.copy(cx));
                    })
                    .child(render(&self.source, 0, &self.selection, window, cx))
            }
        }
        let (view, vcx) = cx.add_window_view(|_, cx| {
            let selection = cx.new(TextSelection::new);
            cx.observe(&selection, |_, _, cx| cx.notify()).detach();
            CodeDocument {
                selection,
                source: "```rust\nlet βeta = 1;\nprintln!(\"hello\");\n```".into(),
            }
        });
        for (source, expected, header) in [
            ("```rust\nlet βeta = 1;\nprintln!(\"hello\");\n```", "let βeta = 1;\nprintln!(\"hello\");", true),
            ("```\nβeta\n```", "βeta", false),
        ] {
            view.update(vcx, |view, cx| {
                view.source = source.into();
                cx.notify();
            });
            vcx.run_until_parked();
            assert_eq!(vcx.debug_bounds("code-copy").is_some(), header);
            let bounds = vcx.debug_bounds("selectable-text-0-0").unwrap();
            let start = gpui::point(bounds.left() + px(0.1), bounds.top() + px(5.));
            let end = gpui::point(bounds.right() + px(10.), bounds.bottom() - px(2.));
            vcx.simulate_event(gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: start,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            });
            vcx.simulate_event(gpui::MouseMoveEvent {
                position: end,
                pressed_button: Some(gpui::MouseButton::Left),
                modifiers: Default::default(),
            });
            vcx.simulate_event(gpui::MouseUpEvent {
                button: gpui::MouseButton::Left,
                position: end,
                modifiers: Default::default(),
                click_count: 1,
            });
            vcx.run_until_parked();
            view.read_with(vcx, |view, cx| {
                assert!(view.selection.read(cx).highlight("0-0", expected.len()).is_some());
            });
            vcx.simulate_keystrokes("ctrl-c");
            assert_eq!(
                vcx.update(|_, cx| cx.read_from_clipboard()).and_then(|item| item.text()).as_deref(),
                Some(expected),
            );
            if let Some(copy) = vcx.debug_bounds("code-copy") {
                vcx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("old".into())));
                vcx.simulate_event(gpui::MouseDownEvent {
                    button: gpui::MouseButton::Left,
                    position: copy.center(),
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                });
                assert_eq!(
                    vcx.update(|_, cx| cx.read_from_clipboard()).and_then(|item| item.text()).as_deref(),
                    Some(expected),
                );
            }
        }
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
                "{}\n{}\n{}\nThe **answer**.",
                jcode_render_core::reasoning_line_markup("**Checking top live tabs**"),
                jcode_render_core::reasoning_line_markup(""),
                jcode_render_core::reasoning_line_markup("Quiet reasoning body.")
            ),
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("restored-reasoning").is_some());
        let heading = vcx.debug_bounds("selectable-text-0-0-0").unwrap();
        let body = vcx.debug_bounds("selectable-text-0-0-1").unwrap();
        assert!(heading.size.height > body.size.height);
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

    #[test]
    fn reasoning_section_titles_only_promote_short_standalone_emphasis() {
        assert_eq!(
            reasoning_section_title("**Classifying voice support**"),
            Some("Classifying voice support")
        );
        for source in [
            "ordinary text",
            "**bold** and prose",
            "**one** and **two**",
            "****",
            "**two\nlines**",
            "**unfinished",
        ] {
            assert_eq!(reasoning_section_title(source), None, "{source}");
        }
        assert_eq!(
            reasoning_section_title(&format!("**{}**", "x".repeat(101))),
            None
        );
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
