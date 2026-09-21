//! The same syntect engine used by jcode-tui-markdown, with VS Code-style Desktop semantic colors.
//! Two-face adds maintained TS/TSX grammars absent from syntect's default bundle.
use crate::theme::{Theme, to_hsla};
use gpui::HighlightStyle;
use std::{
    collections::VecDeque,
    ops::Range,
    sync::{Arc, LazyLock, Mutex},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Color, StyleModifier, Theme as SyntaxTheme, ThemeItem},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
// Encode semantic categories as tiny color IDs, not actual colors. Cached parse
// results therefore survive palette changes and animated theme transitions.
static SEMANTICS: LazyLock<SyntaxTheme> = LazyLock::new(|| {
    let mut theme = SyntaxTheme::default();
    theme.settings.foreground = Some(color_id(0));
    // Keep categories separate even where a grammar only supplies lexical scopes.
    // In particular, functions must not collapse into the teal type category.
    for (scope, id) in [
        ("keyword, storage", 1),
        ("string", 2),
        ("comment", 3),
        ("constant.numeric", 4),
        ("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, entity.name.namespace, support.type, support.class", 5),
        ("punctuation", 6),
        ("entity.name.function, support.function, variable.function", 7),
        ("variable, meta.object-literal.key, support.variable", 8),
        ("keyword.control", 9),
        ("constant.language, variable.language", 1),
        ("constant.other, variable.other.constant, entity.name.constant", 10),
        ("entity.name.tag", 11),
        ("entity.other.attribute-name", 12),
        ("keyword.operator", 6),
        ("keyword.operator.word, keyword.operator.new, keyword.operator.expression", 1),
        ("string punctuation", 2),
        ("comment punctuation", 3),
    ] {
        theme.scopes.push(ThemeItem {
            scope: scope.parse().expect("static syntax scope"),
            style: StyleModifier {
                foreground: Some(color_id(id)),
                ..Default::default()
            },
        });
    }
    theme
});
fn color_id(id: u8) -> Color {
    Color {
        r: id,
        g: 0,
        b: 0,
        a: 255,
    }
}
type Tokens = Arc<Vec<(Range<usize>, u8)>>;
struct Entry {
    body: String,
    lang: String,
    tokens: Tokens,
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));
const CACHE_BYTES: usize = 2 * 1024 * 1024;

fn tokens(body: &str, lang: &str) -> Tokens {
    {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = cache
            .entries
            .iter()
            .position(|entry| entry.lang == lang && entry.body == body)
        {
            let entry = cache.entries.remove(index).unwrap();
            let tokens = entry.tokens.clone();
            cache.entries.push_back(entry);
            return tokens;
        }
    }
    let syntax = SYNTAXES
        .find_syntax_by_token(lang)
        .or_else(|| SYNTAXES.find_syntax_by_extension(lang))
        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let mut parser = HighlightLines::new(syntax, &SEMANTICS);
    let mut tokens = Vec::new();
    let mut offset = 0;
    for line in LinesWithEndings::from(body) {
        if let Ok(spans) = parser.highlight_line(line, &SYNTAXES) {
            let mut start = offset;
            for (style, text) in spans {
                let end = start + text.len();
                if style.foreground.r != 0 && start < end {
                    tokens.push((start..end, style.foreground.r));
                }
                start = end;
            }
        }
        offset += line.len();
    }
    let tokens = Arc::new(tokens);
    let cost = body.len() + tokens.len() * std::mem::size_of::<(Range<usize>, u8)>();
    if cost <= CACHE_BYTES {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        while cache.bytes + cost > CACHE_BYTES || cache.entries.len() >= 128 {
            let Some(entry) = cache.entries.pop_front() else {
                break;
            };
            cache.bytes -=
                entry.body.len() + entry.tokens.len() * std::mem::size_of::<(Range<usize>, u8)>();
        }
        cache.bytes += cost;
        cache.entries.push_back(Entry {
            body: body.into(),
            lang: lang.into(),
            tokens: tokens.clone(),
        });
    }
    tokens
}

pub(super) fn highlight(body: &str, lang: &str) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    highlight_with_theme(body, lang, Theme::global())
}

fn highlight_with_theme(
    body: &str,
    lang: &str,
    theme: &Theme,
) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
    let palette = [
        theme.CODE_TEXT,
        theme.CODE_KEYWORD,
        theme.CODE_STRING,
        theme.CODE_COMMENT,
        theme.CODE_NUMBER,
        theme.CODE_TYPE,
        theme.CODE_PUNCT,
        theme.CODE_FUNCTION,
        theme.CODE_VARIABLE,
        theme.CODE_CONTROL,
        theme.CODE_CONSTANT,
        theme.CODE_TAG,
        theme.CODE_ATTRIBUTE,
    ];
    let spans = tokens(body, &lang.to_ascii_lowercase())
        .iter()
        .map(|(range, id)| {
            (
                range.clone(),
                HighlightStyle {
                    color: Some(to_hsla(palette[*id as usize])),
                    ..Default::default()
                },
            )
        })
        .collect();
    (body.to_owned(), spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn category_at(body: &str, lang: &str, needle: &str) -> u8 {
        let offset = body.find(needle).unwrap();
        tokens(body, lang)
            .iter()
            .find(|(range, _)| range.contains(&offset))
            .map_or(0, |(_, id)| *id)
    }
    #[test]
    fn rust_raw_strings_lifetimes_chars_and_nested_comments() {
        let body = "fn borrow<'a>(v: &'a str) { let c = 'é'; let s = r##\"first\n\"quoted\" # second\"##; /* outer /* inner */ still comment */ }";
        assert_ne!(category_at(body, "rust", "'a"), 2);
        assert_eq!(category_at(body, "rust", "é"), 2);
        assert_eq!(category_at(body, "rust", "quoted"), 2);
        assert_eq!(category_at(body, "rust", "still comment"), 3);
    }
    #[test]
    fn real_tsx_grammar_and_interpolation() {
        let syntax = SYNTAXES.find_syntax_by_token("tsx").unwrap();
        assert_ne!(syntax.name, "Plain Text");
        let body =
            "const value: string = `hello ${42}`;\nconst element = <Widget title={value} />;";
        assert_eq!(category_at(body, "tsx", "const"), 1);
        assert_eq!(category_at(body, "tsx", "hello"), 2);
        assert_eq!(category_at(body, "tsx", "42"), 4);
        assert!(!tokens(body, "tsx").is_empty());
    }
    #[test]
    fn token_cache_reuses_parsing_and_is_theme_independent() {
        let body = "let cached_unique_marker = 91234;";
        let first = tokens(body, "rust");
        let second = tokens(body, "rust");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &tokens(body, "text")));
        assert!(tokens(body, "unknown-format").is_empty());
        let (_, spans) = highlight(body, "rust");
        assert_eq!(
            spans[0].1.color,
            Some(to_hsla(Theme::global().CODE_KEYWORD))
        );
    }
    #[test]
    fn palette_changes_recolor_without_reparsing() {
        let body = "let palette_unique_marker = 414;";
        let parsed = tokens(body, "rust");
        let mut alternate = Theme::global().clone();
        alternate.CODE_KEYWORD = gpui::rgb(0xff00ff);
        let (_, changed) = highlight_with_theme(body, "rust", &alternate);
        assert_eq!(changed[0].1.color, Some(to_hsla(alternate.CODE_KEYWORD)));
        assert!(Arc::ptr_eq(&parsed, &tokens(body, "rust")));
        let (_, current) = highlight_with_theme(body, "rust", Theme::global());
        assert_eq!(
            current[0].1.color,
            Some(to_hsla(Theme::global().CODE_KEYWORD))
        );
    }

    #[test]
    fn vscode_categories_distinguish_functions_types_and_control_flow() {
        let rust = "struct Widget { count: u32 }\nfn build() { if true { return; } }";
        assert_eq!(category_at(rust, "rust", "Widget"), 5);
        assert_eq!(category_at(rust, "rust", "build"), 7);
        assert_eq!(category_at(rust, "rust", "if"), 9);
        assert_eq!(category_at(rust, "rust", "true"), 1);
        let ts = "function greet(name: string) { return name + \"hello\"; }";
        assert_eq!(category_at(ts, "typescript", "greet"), 7);
        assert_eq!(category_at(ts, "typescript", "name"), 8);
        assert_eq!(category_at(ts, "typescript", "return"), 9);
        let html = "<div class=\"hello\">world</div>";
        assert_eq!(category_at(html, "html", "div"), 11);
        assert_eq!(category_at(html, "html", "class"), 12);
        assert_eq!(category_at(html, "html", "hello"), 2);
    }

    #[test]
    fn utf8_ranges_are_valid_with_escaped_unicode_and_crlf() {
        let body = "let s = \"\\é🙂\";\r\n// café\r\nlet x = 1;";
        let (plain, spans) = highlight(body, "rust");
        assert_eq!(plain, body);
        for (range, _) in spans {
            assert!(body.get(range).is_some());
        }
    }
}
