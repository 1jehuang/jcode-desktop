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

use crate::theme::{Theme, to_hsla};

#[derive(Debug, PartialEq)]
enum Block {
    Heading(u8, String),
    Paragraph(String),
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
        } else if trimmed.starts_with("\\[") && trimmed.ends_with("\\]") && trimmed.len() > 4 {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Math(latex_to_text(
                trimmed[2..trimmed.len() - 2].trim(),
            )));
        } else if let Some(rest) = trimmed
            .strip_prefix("```")
            .or_else(|| trimmed.strip_prefix("~~~"))
        {
            flush(&mut paragraph, &mut blocks);
            let lang = rest.trim().to_string();
            let mut body = String::new();
            for code_line in lines.by_ref() {
                let t = code_line.trim_start();
                if t.starts_with("```") || t.starts_with("~~~") {
                    break;
                }
                body.push_str(code_line);
                body.push('\n');
            }
            while body.ends_with('\n') {
                body.pop();
            }
            blocks.push(Block::Code { lang, body });
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

struct Inline {
    plain: String,
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    links: Vec<(std::ops::Range<usize>, String)>,
}

/// Inline spans: strip `code`, **bold**, *italic*, ~~strike~~ and link syntax,
/// returning plain text plus highlight ranges (byte offsets into the plain
/// text) and any link targets.
fn inline_spans(source: &str) -> Inline {
    let mut plain = String::with_capacity(source.len());
    let mut highlights = Vec::new();
    let mut links = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;

    let code_style = HighlightStyle {
        color: Some(to_hsla(Theme::CODE_TEXT)),
        background_color: Some(to_hsla(Theme::INLINE_CODE_BG)),
        ..Default::default()
    };

    while i < bytes.len() {
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
                        color: Some(to_hsla(Theme::CODE_TEXT)),
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
                        color: Some(to_hsla(Theme::CODE_TEXT)),
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
                    let nested = inline_spans(label);
                    let start = plain.len();
                    plain.push_str(&nested.plain);
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
                let nested = inline_spans(&source[i + 2..i + 2 + end]);
                let start = plain.len();
                plain.push_str(&nested.plain);
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
                            color: Some(to_hsla(Theme::TEXT_DIM)),
                        }),
                        color: Some(to_hsla(Theme::TEXT_DIM)),
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
                let nested = inline_spans(&source[i + marker.len()..i + marker.len() + end]);
                let start = plain.len();
                plain.push_str(&nested.plain);
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
                        let nested = inline_spans(inner);
                        let start = plain.len();
                        plain.push_str(&nested.plain);
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
        highlights,
        links,
    }
}

fn link_style() -> HighlightStyle {
    HighlightStyle {
        color: Some(to_hsla(Theme::LINK)),
        underline: Some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(to_hsla(Theme::LINK)),
            wavy: false,
        }),
        ..Default::default()
    }
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

/// A styled inline run. Links are clickable when any are present.
fn styled_line(source: &str, window: &gpui::Window) -> gpui::AnyElement {
    let inline = inline_spans(source);
    let style = window.text_style();
    let text = StyledText::new(inline.plain.clone())
        .with_default_highlights(&style, inline.highlights.clone());
    if inline.links.is_empty() {
        return text.into_any_element();
    }
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
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
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
        Token::Plain => Theme::CODE_TEXT,
        Token::Keyword => Theme::CODE_KEYWORD,
        Token::Str => Theme::CODE_STRING,
        Token::Comment => Theme::CODE_COMMENT,
        Token::Number => Theme::CODE_NUMBER,
        Token::Type => Theme::CODE_TYPE,
        Token::Punct => Theme::CODE_PUNCT,
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

fn highlight_code(
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

fn code_block(lang: &str, body: &str, window: &gpui::Window) -> gpui::AnyElement {
    let (plain, highlights) = highlight_code(body, lang);
    let line_count = plain.lines().count().max(1);
    let gutter = (1..=line_count)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut style = window.text_style();
    style.font_family = Theme::FONT_MONO.into();
    style.font_size = px(12.5).into();
    style.color = to_hsla(Theme::CODE_TEXT);
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
        .border_color(Theme::CODE_BORDER)
        .bg(Theme::CODE_BG)
        .when(show_header, |el| {
            el.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2p5()
                    .py_1()
                    .bg(Theme::CODE_HEADER_BG)
                    .border_b_1()
                    .border_color(Theme::CODE_BORDER)
                    .text_size(px(10.5))
                    .text_color(Theme::TEXT_DIM)
                    .font_family(Theme::FONT_MONO)
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
                                    .text_color(Theme::TEXT_FAINT)
                                    .hover(|el| el.text_color(Theme::TEXT))
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
                            .font_family(Theme::FONT_MONO)
                            .text_size(px(12.5))
                            .line_height(relative(1.5))
                            .text_color(Theme::CODE_GUTTER)
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

// --- Rendering -------------------------------------------------------------

/// Render markdown into a column of GPUI elements.
pub fn render(source: &str, window: &gpui::Window) -> impl IntoElement {
    let blocks = parse(source);
    let mut children: Vec<gpui::AnyElement> = Vec::with_capacity(blocks.len());
    let mut previous_was_list = false;

    for block in blocks {
        let is_list = matches!(block, Block::Bullet { .. } | Block::Numbered { .. });
        let tight = is_list && previous_was_list;
        previous_was_list = is_list;

        let element = match block {
            Block::Heading(level, text) => {
                let (size, weight) = match level {
                    1 => (px(19.0), FontWeight::BOLD),
                    2 => (px(16.5), FontWeight::BOLD),
                    3 => (px(14.5), FontWeight::SEMIBOLD),
                    _ => (px(13.5), FontWeight::SEMIBOLD),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .mt_2()
                    .child(
                        div()
                            .text_size(size)
                            .font_weight(weight)
                            .text_color(Theme::HEADING)
                            .line_height(relative(1.35))
                            .child(styled_line(&text, window)),
                    )
                    .when(level <= 2, |el| {
                        el.child(div().h(px(1.0)).w_full().bg(Theme::PANEL_BORDER))
                    })
                    .into_any_element()
            }
            Block::Paragraph(text) => div()
                .line_height(relative(1.55))
                .child(styled_line(&text, window))
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
                    Some(true) => Theme::OK,
                    Some(false) => Theme::TEXT_DIM,
                    None => Theme::ACCENT_MUTED,
                };
                list_row(depth, marker, marker_color, &text, window, tight, task)
            }
            Block::Numbered {
                depth,
                number,
                text,
            } => list_row(
                depth,
                format!("{number}."),
                Theme::ACCENT_MUTED,
                &text,
                window,
                tight,
                None,
            ),
            Block::Quote(lines) => div()
                .debug_selector(|| "md-quote".into())
                .flex()
                .flex_col()
                .my_1()
                .border_l_2()
                .border_color(Theme::PANEL_BORDER)
                .bg(Theme::QUOTE_BG)
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
                        .text_color(Theme::TEXT_DIM)
                        .italic()
                        .line_height(relative(1.5))
                        .children(
                            lines
                                .iter()
                                .filter(|line| !line.trim().is_empty())
                                .map(|line| div().child(styled_line(line, window))),
                        ),
                )
                .into_any_element(),
            Block::Code { lang, body } => code_block(&lang, &body, window),
            Block::Table { header, rows } => table(header, rows, window),
            Block::Math(text) => div()
                .w_full()
                .my_1()
                .px_3()
                .py_2p5()
                .rounded_md()
                .border_1()
                .border_color(Theme::CODE_BORDER)
                .bg(Theme::CODE_BG)
                .text_center()
                .text_size(px(16.0))
                .text_color(Theme::TEXT)
                .child(text)
                .into_any_element(),
            Block::Rule => div()
                .h(px(1.0))
                .my_2()
                .w_full()
                .bg(Theme::PANEL_BORDER)
                .into_any_element(),
        };
        children.push(element);
    }

    div().flex().flex_col().gap_1p5().children(children)
}

fn list_row(
    depth: usize,
    marker: String,
    marker_color: gpui::Rgba,
    text: &str,
    window: &gpui::Window,
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
                .when(task == Some(true), |el| el.text_color(Theme::TEXT_DIM))
                .child(styled_line(text, window)),
        )
        .into_any_element()
}

fn table(header: Vec<String>, rows: Vec<Vec<String>>, window: &gpui::Window) -> gpui::AnyElement {
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
            .child(styled_line(text, window))
    };
    div()
        .flex()
        .flex_col()
        .my_1()
        .w_full()
        .overflow_hidden()
        .rounded_md()
        .border_1()
        .border_color(Theme::PANEL_BORDER)
        .text_size(px(12.5))
        .child(
            div()
                .flex()
                .flex_row()
                .bg(Theme::CODE_HEADER_BG)
                .border_b_1()
                .border_color(Theme::PANEL_BORDER)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Theme::HEADING)
                .children((0..columns).map(|index| {
                    cell(header.get(index).map(String::as_str).unwrap_or(""), window)
                })),
        )
        .children(rows.into_iter().enumerate().map(|(row_index, row)| {
            div()
                .flex()
                .flex_row()
                .when(row_index % 2 == 1, |el| el.bg(Theme::TABLE_STRIPE))
                .text_color(Theme::TEXT)
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
        assert_eq!(blocks[0], Block::Math("e^(iπ)+1=0".into()));
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
        assert_eq!(blocks[0], Block::Math("e^(iπ)+1=0".into()));
    }

    #[test]
    fn preserves_unknown_latex_readably() {
        assert_eq!(latex_to_text(r"\\unknown{x}^2"), r"\\unknownx²");
        assert_eq!(latex_to_text(r"a \rightarrow \infty"), "a → ∞");
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
