//! `mcp` list calls render as a server directory, not tool text.
//!
//! A finished `mcp` call with `action: list` shows each connected server with
//! its tool count and tool names as pills, plus configured servers that are
//! not connected. It parses the tool's own output: both the compact format
//! (`## server (N tools)` then a comma-separated name line) and the older
//! format with `  - name: description` lines. Anything else keeps the
//! generic tool row.
use crate::text_selection::{self, TextSelection};
use crate::theme::Theme;
use gpui::{prelude::*, *};

/// Tool pills shown per server before the "+N more" toggle.
const TOOLS_COLLAPSED: usize = 10;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Server {
    pub name: String,
    pub tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Configured {
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct View {
    pub servers: Vec<Server>,
    pub configured: Vec<Configured>,
}

impl View {
    fn tool_count(&self) -> usize {
        self.servers.iter().map(|server| server.tools.len()).sum()
    }
}

/// `mcp__github__create_issue` reads as `create_issue` under its server.
fn short_tool_name(server: &str, name: &str) -> String {
    let name = name.trim();
    let prefix = format!("mcp__{}__", server.replace('-', "_"));
    name.strip_prefix(&prefix)
        .or_else(|| name.strip_prefix(&format!("mcp__{server}__")))
        .unwrap_or(name)
        .to_owned()
}

fn parse_list(output: &str) -> Option<View> {
    if !output.starts_with("Connected MCP servers:") {
        return None;
    }
    let mut view = View::default();
    let mut in_configured = false;
    // Compact output writes "## server (N tools)" then one name line. Older
    // output writes "  - name: description" whose descriptions can span lines,
    // so for those servers only bullet lines are tool names.
    let mut compact = false;
    for line in output.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            in_configured = false;
            // "server (12 tools)" or plain "server".
            let (name, counted) = match header.rsplit_once(" (") {
                Some((name, rest)) if rest.ends_with(')') && rest.contains("tool") => (name, true),
                _ => (header, false),
            };
            compact = counted;
            view.servers.push(Server {
                name: name.trim().to_owned(),
                tools: Vec::new(),
            });
            continue;
        }
        if line.starts_with("Configured but not connected") {
            in_configured = true;
            continue;
        }
        if in_configured {
            if let Some(entry) = line.trim_start().strip_prefix("- ") {
                let (name, rest) = entry.split_once(" (").unwrap_or((entry, ""));
                view.configured.push(Configured {
                    name: name.trim().to_owned(),
                    enabled: !rest.starts_with("disabled"),
                });
            }
            continue;
        }
        let Some(server) = view.servers.last_mut() else {
            continue;
        };
        let line = line.trim_end();
        if line.trim().is_empty() || line.trim() == "(no tools)" {
            continue;
        }
        if let Some(entry) = line.strip_prefix("  - ").filter(|_| !compact) {
            // Older format: "  - name: description".
            let name = entry.split_once(": ").map_or(entry, |(name, _)| name);
            server.tools.push(short_tool_name(&server.name, name));
        } else if compact && !line.starts_with(' ') && !line.starts_with("Use mcp_search") {
            server.tools.extend(
                line.split(", ")
                    .filter(|name| !name.trim().is_empty())
                    .map(|name| short_tool_name(&server.name, name)),
            );
        }
    }
    (!view.servers.is_empty() || !view.configured.is_empty()).then_some(view)
}

/// A card view of a finished `mcp` list call.
pub(super) fn parse(
    name: &str,
    input: &str,
    output: &str,
    done: bool,
    error: Option<&str>,
) -> Option<View> {
    if name.trim_start_matches("functions.") != "mcp" || !done || error.is_some() {
        return None;
    }
    let input: serde_json::Value = serde_json::from_str(input).ok()?;
    if input.get("action")?.as_str()? != "list" {
        return None;
    }
    parse_list(output.trim())
}

/// Whether any server has tools hidden behind "+N more".
pub(super) fn expandable(view: &View) -> bool {
    view.servers
        .iter()
        .any(|server| server.tools.len() > TOOLS_COLLAPSED + 1)
}

fn pill(theme: &Theme, text: String) -> Div {
    div()
        .flex_none()
        .px_2()
        .rounded_full()
        .bg(theme.INLINE_CODE_BG)
        .text_size(px(12.0))
        .line_height(px(20.0))
        .text_color(theme.TEXT_DIM)
        .child(text)
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

pub(super) fn render(
    index: usize,
    view: &View,
    tokens: Option<Div>,
    expanded: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    selection: &Entity<TextSelection>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global();
    let plain =
        |key: String, text: String| text_selection::plain(selection.clone(), key, text, window, cx);
    let on_toggle = std::rc::Rc::new(on_toggle);

    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .bg(theme.HEADER_BG)
        .child(crate::tool_icon::render_badge("mcp", true, false))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.TOOL_TEXT)
                .child("MCP servers"),
        )
        .child(
            div()
                .flex_none()
                .debug_selector(|| "mcp-list-summary".into())
                .text_size(px(12.0))
                .text_color(theme.TEXT_FAINT)
                .child(format!(
                    "{} connected · {}",
                    view.servers.len(),
                    plural(view.tool_count(), "tool")
                )),
        )
        .when_some(tokens, |el, tokens| el.child(tokens));

    let server_rows = view.servers.iter().enumerate().map(|(n, server)| {
        let hidden = if expanded || server.tools.len() <= TOOLS_COLLAPSED + 1 {
            0
        } else {
            server.tools.len() - TOOLS_COLLAPSED
        };
        let shown = server.tools.len() - hidden;
        let on_toggle = on_toggle.clone();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_1p5()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.TEXT)
                            .child(plain(
                                format!("mcp-server-{index}-{n}"),
                                server.name.clone(),
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(theme.TEXT_FAINT)
                            .child(plural(server.tools.len(), "tool")),
                    ),
            )
            .when(!server.tools.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_1()
                        .children(
                            server.tools[..shown]
                                .iter()
                                .map(|tool| pill(theme, tool.clone())),
                        )
                        .when(hidden > 0, |el| {
                            el.child(
                                pill(theme, format!("+{hidden} more"))
                                    .id(("mcp-list-more", index * 256 + n))
                                    .debug_selector(|| "mcp-list-more".into())
                                    .text_color(theme.LINK)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.ACCENT_MUTED))
                                    .on_click(move |event, window, cx| {
                                        on_toggle(event, window, cx)
                                    }),
                            )
                        }),
                )
            })
    });

    let toggle_less = {
        let on_toggle = on_toggle.clone();
        div()
            .id(("mcp-list-less", index))
            .debug_selector(|| "mcp-list-less".into())
            .mx_3()
            .mb_2()
            .text_size(px(12.0))
            .text_color(theme.TEXT_DIM)
            .cursor_pointer()
            .hover(|style| style.text_color(theme.TEXT))
            .on_click(move |event, window, cx| on_toggle(event, window, cx))
            .child("Show less")
    };

    let configured = (!view.configured.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_1p5()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.TEXT_FAINT)
                    .child("Configured, not connected"),
            )
            .child(div().flex().flex_row().flex_wrap().gap_1().children(
                view.configured.iter().map(|server| {
                    pill(
                        theme,
                        if server.enabled {
                            server.name.clone()
                        } else {
                            format!("{} · disabled", server.name)
                        },
                    )
                }),
            ))
    });

    div()
        .id(("mcp-list", index))
        .debug_selector(|| "mcp-list-card".into())
        .flex_none()
        .flex()
        .flex_col()
        .max_w(px(720.0))
        .my_1()
        .rounded_xl()
        .border_1()
        .border_color(theme.TOOL_BORDER)
        .bg(theme.TOOL_BG)
        .overflow_hidden()
        .text_size(px(13.0))
        .line_height(px(20.0))
        .child(header)
        .child(div().flex().flex_col().py_1().children(server_rows))
        .when_some(configured, |el, configured| el.child(configured))
        .when(expanded && expandable(view), |el| el.child(toggle_less))
        .into_any_element()
}

pub(super) fn fixture_items() -> Vec<super::Item> {
    use super::Item;
    let output = "Connected MCP servers: 3 (19 tools)\n\n## github (14 tools)\nmcp__github__actions_get, mcp__github__actions_list, mcp__github__add_issue_comment, mcp__github__create_branch, mcp__github__create_pull_request, mcp__github__get_file_contents, mcp__github__get_me, mcp__github__issue_read, mcp__github__issue_write, mcp__github__list_commits, mcp__github__list_issues, mcp__github__list_pull_requests, mcp__github__merge_pull_request, mcp__github__search_code\n\n## sheets (4 tools)\nmcp__sheets__info, mcp__sheets__read, mcp__sheets__write, mcp__sheets__append\n\n## cloudflare-docs (1 tool)\nmcp__cloudflare_docs__search_cloudflare_documentation\n\nUse mcp_search with a server or query for descriptions and input schemas.\n\nConfigured but not connected:\n  - gog (enabled; connect with {\"action\": \"connect\", \"server\": \"gog\"})\n  - playwright (disabled in config; connect on demand with {\"action\": \"connect\", \"server\": \"playwright\"})\n";
    vec![
        Item::User("Which MCPs do you have?".into()),
        Item::Tool {
            call_id: "mcp-list".into(),
            name: "mcp".into(),
            input: serde_json::json!({"intent": "List connected MCP servers", "action": "list"})
                .to_string(),
            output: output.into(),
            done: true,
            error: None,
        },
        Item::Assistant("Three servers are connected: GitHub, Sheets and Cloudflare docs.".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::{Configured, expandable, parse};

    const COMPACT: &str = "Connected MCP servers: 2 (3 tools)\n\n## cloudflare-docs (1 tool)\nmcp__cloudflare_docs__search\n\n## sheets (2 tools)\nmcp__sheets__info, mcp__sheets__read\n\nUse mcp_search with a server or query for descriptions and input schemas.\n\nConfigured but not connected:\n  - gog (enabled; connect with {\"action\": \"connect\", \"server\": \"gog\"})\n  - pw (disabled in config; connect on demand with {})\n";

    #[test]
    fn compact_list_becomes_servers() {
        let view = parse("mcp", r#"{"action":"list"}"#, COMPACT, true, None).unwrap();
        assert_eq!(view.servers.len(), 2);
        assert_eq!(view.servers[0].name, "cloudflare-docs");
        assert_eq!(view.servers[0].tools, vec!["search"]);
        assert_eq!(view.servers[1].tools, vec!["info", "read"]);
        assert_eq!(
            view.configured,
            vec![
                Configured {
                    name: "gog".into(),
                    enabled: true
                },
                Configured {
                    name: "pw".into(),
                    enabled: false
                },
            ]
        );
    }

    #[test]
    fn older_list_with_descriptions_still_parses() {
        let output = "Connected MCP servers: 1\n\n## rho\n  - mcp__rho__getCard: Get a card\n\nReturns one card: by id.\n  - mcp__rho__listCards: List cards\n\n";
        let view = parse("functions.mcp", r#"{"action":"list"}"#, output, true, None).unwrap();
        assert_eq!(view.servers[0].name, "rho");
        assert_eq!(view.servers[0].tools, vec!["getCard", "listCards"]);
    }

    #[test]
    fn only_finished_list_calls_become_cards() {
        let list = r#"{"action":"list"}"#;
        assert!(parse("mcp", list, COMPACT, false, None).is_none());
        assert!(parse("mcp", list, COMPACT, true, Some("boom")).is_none());
        assert!(parse("mcp", r#"{"action":"reload"}"#, COMPACT, true, None).is_none());
        assert!(parse("bash", list, COMPACT, true, None).is_none());
        assert!(parse("mcp", list, "No MCP servers connected.", true, None).is_none());
    }

    #[test]
    fn long_tool_lists_are_expandable() {
        let tools: Vec<String> = (0..20).map(|n| format!("mcp__big__t{n}")).collect();
        let output = format!(
            "Connected MCP servers: 1 (20 tools)\n\n## big (20 tools)\n{}\n",
            tools.join(", ")
        );
        let view = parse("mcp", r#"{"action":"list"}"#, &output, true, None).unwrap();
        assert_eq!(view.servers[0].tools.len(), 20);
        assert!(expandable(&view));
    }
}
