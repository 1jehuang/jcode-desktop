//! Highlight complete, independent old/new hunk sources, then project onto display rows.
use crate::markdown::highlight_code;
use gpui::HighlightStyle;
use std::ops::Range;

pub(super) type StyledLine = (String, Vec<(Range<usize>, HighlightStyle)>);

pub(super) fn language(path: &str) -> String {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" | "containerfile" => return "dockerfile".into(),
        "makefile" | "gnumakefile" => return "makefile".into(),
        "gemfile" | "rakefile" => return "ruby".into(),
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => return "bash".into(),
        _ => {}
    }
    let extension = name.rsplit_once('.').map_or(name.as_str(), |(_, ext)| ext);
    match extension {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "sh" | "zsh" => "bash",
        "rb" => "ruby",
        "yml" => "yaml",
        "h" | "cc" | "cpp" | "hpp" | "cxx" => "cpp",
        "txt" | "md" | "mdx" | "rst" => "text",
        // Let the grammar bundle resolve all other extensions, not a finite
        // desktop-only allowlist (Lua, C#, PHP, Elixir, etc.). Unknowns remain plain.
        _ if name.contains('.') => extension,
        _ => "text",
    }
    .into()
}

/// Matches compact_code exactly while retaining original UTF-8 byte boundaries.
fn compact(text: &str, indent: usize, spans: &[(Range<usize>, HighlightStyle)]) -> StyledLine {
    let mut plain = String::new();
    let mut map = Vec::new();
    let (mut column, mut leading) = (0, true);
    for (count, (offset, ch)) in text.char_indices().enumerate() {
        if count == 1024 {
            plain.push('…');
            break;
        }
        let start = plain.len();
        if ch == '\t' {
            for _ in 0..2 - column % 2 {
                if !leading || column >= indent {
                    plain.push(' ');
                }
                column += 1;
            }
        } else {
            if ch != ' ' {
                leading = false;
            }
            if !leading || column >= indent {
                plain.push(ch);
            }
            column += 1;
        }
        map.push((offset..offset + ch.len_utf8(), start..plain.len()));
    }
    let highlights = spans
        .iter()
        .filter_map(|(span, style)| {
            let mut ranges = map
                .iter()
                .filter(|(source, visible)| {
                    source.start < span.end && source.end > span.start && !visible.is_empty()
                })
                .map(|(_, visible)| visible);
            let first = ranges.next()?;
            let end = ranges.last().unwrap_or(first).end;
            Some((first.start..end, *style))
        })
        .collect();
    (plain, highlights)
}

pub(super) fn prepare(lines: &[String], path: &str, indent: usize) -> Vec<StyledLine> {
    let lang = language(path);
    let mut result: Vec<_> = lines
        .iter()
        .map(|line| {
            let text = line.strip_prefix(['+', '-', ' ']).unwrap_or(line);
            compact(text, indent, &[])
        })
        .collect();
    let mut start = 0;
    while start < lines.len() {
        if lines[start].starts_with("@@") {
            start += 1;
            continue;
        }
        let end = (start..lines.len())
            .find(|&i| lines[i].starts_with("@@"))
            .unwrap_or(lines.len());
        for old in [true, false] {
            let mut source = String::new();
            let mut offsets = Vec::new();
            for (i, line) in lines.iter().enumerate().take(end).skip(start) {
                if line.starts_with('\\') || line.starts_with(if old { '+' } else { '-' }) {
                    continue;
                }
                let text = line.strip_prefix(['+', '-', ' ']).unwrap_or(line);
                let offset = source.len();
                source.push_str(text);
                offsets.push((i, offset..source.len()));
                source.push('\n');
            }
            let (_, highlights) = highlight_code(&source, &lang);
            for (i, range) in offsets {
                let first = highlights.partition_point(|(span, _)| span.end <= range.start);
                let spans = highlights[first..]
                    .iter()
                    .take_while(|(span, _)| span.start < range.end)
                    .filter_map(|(span, style)| {
                        let lo = span.start.max(range.start);
                        let hi = span.end.min(range.end);
                        (lo < hi).then_some((
                            lo.saturating_sub(range.start)..hi.saturating_sub(range.start),
                            *style,
                        ))
                    })
                    .collect::<Vec<_>>();
                result[i] = compact(&source[range], indent, &spans);
            }
        }
        start = end;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, to_hsla};
    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }
    fn color_at(line: &StyledLine, needle: &str) -> Option<gpui::Hsla> {
        let start = line.0.find(needle).unwrap();
        line.1
            .iter()
            .find(|(range, _)| range.contains(&start))
            .and_then(|(_, style)| style.color)
    }
    #[test]
    fn resolves_paths_and_aliases() {
        for (path, expected) in [
            ("src/FILE.RS", "rust"),
            ("C:\\src\\index.TSX", "tsx"),
            ("a/model.pyi", "python"),
            ("x/test.mjs", "javascript"),
            ("src/.bashrc", "bash"),
            ("dir.with.dots/README", "text"),
            ("src/config.yml", "yaml"),
            ("src/plugin.lua", "lua"),
        ] {
            assert_eq!(language(path), expected);
        }
    }
    #[test]
    fn independent_old_new_contexts_and_hunk_reset() {
        let rows = prepare(
            &lines(
                "@@ -1,3 +1,3 @@\n-/* old\n-old comment\n-*/\n+let text = r#\"new\n+new string\n+\"#;\n@@ -20 +20 @@\n+let x = 1;",
            ),
            "src/a.rs",
            0,
        );
        assert_eq!(
            color_at(&rows[2], "old"),
            Some(to_hsla(Theme::global().CODE_COMMENT))
        );
        assert_eq!(
            color_at(&rows[5], "new"),
            Some(to_hsla(Theme::global().CODE_STRING))
        );
        assert_eq!(
            color_at(&rows[8], "let"),
            Some(to_hsla(Theme::global().CODE_KEYWORD))
        );
    }
    #[test]
    fn context_rows_use_new_side_without_contaminating_removed_rows() {
        let rows = prepare(
            &lines(" /* begin\n-still comment\n+*/\n let x = 7;"),
            "a.rs",
            0,
        );
        assert_eq!(
            color_at(&rows[1], "still"),
            Some(to_hsla(Theme::global().CODE_COMMENT))
        );
        assert_eq!(
            color_at(&rows[3], "let"),
            Some(to_hsla(Theme::global().CODE_KEYWORD))
        );
    }
    #[test]
    fn python_triples_and_typescript_templates_span_lines() {
        for (path, text, needle) in [
            (
                "a.py",
                "+value = \"\"\"first\n+second # still string\n+\"\"\"",
                "second",
            ),
            (
                "a.ts",
                "+const value = `first\n+second // still string\n+`;",
                "second",
            ),
        ] {
            let rows = prepare(&lines(text), path, 0);
            assert_eq!(
                color_at(&rows[1], needle),
                Some(to_hsla(Theme::global().CODE_STRING))
            );
        }
    }
    #[test]
    fn spans_follow_unicode_tabs_indent_and_truncation() {
        let source = format!("\t\tlet text = \"é🙂{}\";", "x".repeat(1100));
        let rows = prepare(&[format!("+{source}")], "a.rs", 4);
        assert_eq!(rows[0].0, super::super::compact_code(&source, 4));
        assert_eq!(
            color_at(&rows[0], "é"),
            Some(to_hsla(Theme::global().CODE_STRING))
        );
        assert!(rows[0].0.ends_with('…'));
        for (range, _) in &rows[0].1 {
            assert!(rows[0].0.is_char_boundary(range.start));
            assert!(rows[0].0.is_char_boundary(range.end));
            assert!(range.end <= rows[0].0.len() - '…'.len_utf8());
        }
    }
    #[test]
    fn metadata_does_not_break_multiline_context() {
        let rows = prepare(
            &lines("+/* first\n\\ No newline at end of file\n+second */"),
            "a.rs",
            0,
        );
        assert_eq!(
            color_at(&rows[2], "second"),
            Some(to_hsla(Theme::global().CODE_COMMENT))
        );
    }
}
