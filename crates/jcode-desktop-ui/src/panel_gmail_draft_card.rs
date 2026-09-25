//! Gmail compose calls render as an email, not as JSON.
//!
//! `gmail` calls with `action: draft` or `action: send` show the recipient,
//! subject and body as they stream in, then the saved draft or sent state
//! with a link to open it in Gmail. Other Gmail actions keep the generic row.
use crate::text_selection::{self, TextSelection};
use crate::theme::Theme;
use gpui::{prelude::*, *};

const BODY_LINES: usize = 14;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Compose {
    pub action: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<String>,
    pub reply: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Writing,
    Drafted {
        draft_id: Option<String>,
    },
    Sent {
        message_id: Option<String>,
    },
    /// The tool finished without composing, e.g. a missing attachment.
    Note(String),
}

/// Top-level string fields and string arrays, readable before the arguments
/// finish streaming. Unfinished values are returned as their decoded prefix.
fn fields(input: &str) -> Vec<(String, serde_json::Value)> {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(input) {
        return map.into_iter().collect();
    }
    let mut out = Vec::new();
    let Some(mut rest) = input.trim_start().strip_prefix('{') else {
        return out;
    };
    loop {
        rest = rest.trim_start();
        if !rest.starts_with('"') {
            break;
        }
        let (key, Some(consumed)) = super::tool_streaming::progressive_string(rest) else {
            break;
        };
        let Some(value) = rest[consumed..].trim_start().strip_prefix(':') else {
            break;
        };
        rest = value.trim_start();
        if rest.starts_with('"') {
            let (text, consumed) = super::tool_streaming::progressive_string(rest);
            out.push((key, serde_json::Value::String(text)));
            let Some(consumed) = consumed else { break };
            rest = &rest[consumed..];
        } else {
            let mut values =
                serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
            match values.next() {
                Some(Ok(value)) => {
                    out.push((key, value));
                    rest = &rest[values.byte_offset()..];
                }
                _ => break,
            }
        }
        let Some(next) = rest.trim_start().strip_prefix(',') else {
            break;
        };
        rest = next;
    }
    out
}

/// A compose view of `gmail` arguments, or `None` for non-compose actions.
pub(super) fn parse(name: &str, input: &str) -> Option<Compose> {
    if name.trim_start_matches("functions.") != "gmail" {
        return None;
    }
    let mut compose = Compose::default();
    for (key, value) in fields(input) {
        let text = || value.as_str().unwrap_or_default().to_owned();
        match key.as_str() {
            "action" => compose.action = text(),
            "to" => compose.to = text(),
            "cc" => compose.cc = text(),
            "subject" => compose.subject = text(),
            "body" => compose.body = text(),
            "in_reply_to" | "thread_id" => compose.reply |= !text().is_empty(),
            "attachments" => {
                compose.attachments = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|path| path.as_str().map(str::to_owned))
                    .collect()
            }
            _ => {}
        }
    }
    matches!(compose.action.as_str(), "draft" | "send").then_some(compose)
}

fn line_value(output: &str, label: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(super) fn outcome(output: &str, done: bool) -> Outcome {
    let output = output.trim();
    if !done {
        return Outcome::Writing;
    }
    if output.starts_with("Draft created") {
        Outcome::Drafted {
            draft_id: line_value(output, "Draft ID:"),
        }
    } else if output.starts_with("Email sent") {
        Outcome::Sent {
            message_id: line_value(output, "Message ID:"),
        }
    } else {
        Outcome::Note(output.lines().next().unwrap_or_default().to_owned())
    }
}

/// Where "Open in Gmail" goes. Sent mail opens the message itself. Drafts open
/// the Drafts folder, since the API draft id is not a Gmail web URL.
pub(super) fn gmail_url(outcome: &Outcome) -> Option<String> {
    match outcome {
        Outcome::Drafted { .. } => Some("https://mail.google.com/mail/u/0/#drafts".into()),
        Outcome::Sent {
            message_id: Some(id),
        } if id.chars().all(|ch| ch.is_ascii_alphanumeric()) => {
            Some(format!("https://mail.google.com/mail/u/0/#all/{id}"))
        }
        Outcome::Sent { .. } => Some("https://mail.google.com/mail/u/0/#sent".into()),
        _ => None,
    }
}

fn clip_body(body: &str) -> (String, usize) {
    let lines: Vec<&str> = body.trim_end().lines().collect();
    if lines.len() <= BODY_LINES {
        return (lines.join("\n"), 0);
    }
    (lines[..BODY_LINES].join("\n"), lines.len() - BODY_LINES)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    index: usize,
    compose: &Compose,
    outcome: &Outcome,
    error: Option<&str>,
    expanded: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    selection: &Entity<TextSelection>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global();
    let sending = compose.action == "send";
    let failed = error.is_some() || matches!(outcome, Outcome::Note(_));
    let (status, status_color) = match (outcome, error) {
        (_, Some(_)) => ("Failed", theme.ERROR),
        (Outcome::Writing, _) if sending => ("Sending…", theme.TEXT_FAINT),
        (Outcome::Writing, _) => ("Writing draft…", theme.TEXT_FAINT),
        (Outcome::Drafted { .. }, _) => ("Draft saved", theme.OK),
        (Outcome::Sent { .. }, _) => ("Sent", theme.OK),
        (Outcome::Note(_), _) => ("Not saved", theme.WARN),
    };
    let title = if compose.reply {
        if sending { "Reply" } else { "Reply draft" }
    } else if sending {
        "Email"
    } else {
        "Email draft"
    };
    let plain =
        |key: String, text: String| text_selection::plain(selection.clone(), key, text, window, cx);
    let field = |label: &'static str, value: String, key: &str, strong: bool| {
        div()
            .flex()
            .flex_row()
            .gap_3()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .w(px(56.0))
                    .text_color(theme.TEXT_FAINT)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if strong { theme.TEXT } else { theme.TEXT_DIM })
                    .when(strong, |el| el.font_weight(FontWeight::MEDIUM))
                    .child(plain(format!("gmail-{key}-{index}"), value)),
            )
    };
    let (body, hidden) = if expanded {
        (compose.body.trim_end().to_owned(), 0)
    } else {
        clip_body(&compose.body)
    };
    let writing = matches!(outcome, Outcome::Writing) && error.is_none();
    let url = gmail_url(outcome);

    div()
        .id(("gmail-compose", index))
        .debug_selector(|| "gmail-compose-card".into())
        .flex_none()
        .flex()
        .flex_col()
        .max_w(px(720.0))
        .my_1()
        .rounded_lg()
        .border_1()
        .border_color(if failed {
            theme.ERROR
        } else {
            theme.TOOL_BORDER
        })
        .bg(theme.TOOL_BG)
        .overflow_hidden()
        .text_size(px(13.0))
        .line_height(px(20.0))
        // Header: what this is and where it stands.
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .bg(theme.HEADER_BG)
                .child(crate::tool_icon::render_badge("gmail", !writing, failed))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.TOOL_TEXT)
                        .child(title),
                )
                .child(
                    div()
                        .debug_selector(|| "gmail-compose-status".into())
                        .text_size(px(12.0))
                        .text_color(status_color)
                        .child(status),
                )
                .when_some(url.clone(), |el, url| {
                    el.child(
                        div()
                            .id(("gmail-open", index))
                            .debug_selector(|| "gmail-compose-open".into())
                            .ml_auto()
                            .px_2()
                            .rounded_md()
                            .text_size(px(12.0))
                            .text_color(theme.LINK)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.INLINE_CODE_BG))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                if !crate::harness::screenshot_mode() {
                                    cx.open_url(&url);
                                }
                            })
                            .child("Open in Gmail"),
                    )
                }),
        )
        // Envelope.
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .px_3()
                .pt_2()
                .pb_2()
                .child(field(
                    "To",
                    if compose.to.is_empty() {
                        "…".into()
                    } else {
                        compose.to.clone()
                    },
                    "to",
                    false,
                ))
                .when(!compose.cc.is_empty(), |el| {
                    el.child(field("Cc", compose.cc.clone(), "cc", false))
                })
                .child(field(
                    "Subject",
                    if compose.subject.is_empty() {
                        if writing {
                            "…".into()
                        } else {
                            "(no subject)".into()
                        }
                    } else {
                        compose.subject.clone()
                    },
                    "subject",
                    true,
                )),
        )
        // Body, the part worth reading.
        .when(!body.trim().is_empty() || writing, |el| {
            el.child(
                div()
                    .debug_selector(|| "gmail-compose-body".into())
                    .mx_3()
                    .mb_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme.TOOL_BORDER)
                    .font_family(theme.FONT_AI)
                    .text_color(theme.TEXT)
                    .whitespace_normal()
                    .child(plain(format!("gmail-body-{index}"), body)),
            )
        })
        .when(
            hidden > 0 || (expanded && compose.body.lines().count() > BODY_LINES),
            |el| {
                el.child(
                    div()
                        .id(("gmail-body-toggle", index))
                        .debug_selector(|| "gmail-compose-more".into())
                        .mx_3()
                        .mb_2()
                        .text_size(px(12.0))
                        .text_color(theme.TEXT_DIM)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.TEXT))
                        .on_click(on_toggle)
                        .child(if expanded {
                            "Show less".to_owned()
                        } else {
                            format!(
                                "Show {hidden} more line{}",
                                if hidden == 1 { "" } else { "s" }
                            )
                        }),
                )
            },
        )
        .when(!compose.attachments.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .mx_3()
                    .mb_2()
                    .children(compose.attachments.iter().enumerate().map(|(n, path)| {
                        div()
                            .id(("gmail-attachment", index * 64 + n))
                            .px_2()
                            .rounded_md()
                            .bg(theme.INLINE_CODE_BG)
                            .text_size(px(12.0))
                            .text_color(theme.TEXT_DIM)
                            .child(format!("📎 {}", file_name(path)))
                    })),
            )
        })
        .when_some(
            match (error, outcome) {
                (Some(message), _) => Some(message.to_owned()),
                (None, Outcome::Note(note)) => Some(note.clone()),
                _ => None,
            },
            |el, message| {
                el.child(
                    div()
                        .debug_selector(|| "gmail-compose-error".into())
                        .px_3()
                        .pb_2()
                        .text_size(px(12.0))
                        .text_color(theme.ERROR)
                        .child(plain(format!("gmail-error-{index}"), message)),
                )
            },
        )
        .into_any_element()
}

pub(super) fn fixture_items() -> Vec<super::Item> {
    use super::Item;
    let body = "Hi Sam,\n\nThanks for sending over the Q3 numbers. I went through them this afternoon and they look solid.\n\nTwo small things before we share it more widely:\n\n1. The churn figure on page 3 uses August, not September.\n2. Could we add the Desktop beta signups next to the CLI installs?\n\nHappy to pair on it tomorrow if that helps.\n\nBest,\nJeremy";
    vec![
        Item::User("Draft a reply to Sam about the Q3 report. Mention the churn number and ask for Desktop signups.".into()),
        Item::Assistant("Here is a draft reply. It is saved in Gmail and has not been sent.".into()),
        Item::Tool {
            call_id: "gmail-draft".into(),
            name: "gmail".into(),
            input: serde_json::json!({
                "intent": "Draft reply to Sam",
                "action": "draft",
                "to": "sam@example.com",
                "subject": "Re: Q3 report",
                "body": body,
                "in_reply_to": "18c2f0a1b2c3d4e5",
                "attachments": ["/home/jeremy/reports/q3-notes.pdf"],
            })
            .to_string(),
            output: "Draft created successfully.\nDraft ID: r-812345\nTo: sam@example.com\nSubject: Re: Q3 report\nAttachments (1):\n  - /home/jeremy/reports/q3-notes.pdf\n\nTo send this draft, use action 'send_draft' with draft_id 'r-812345' and confirmed: true.".into(),
            done: true,
            error: None,
        },
        Item::User("Also start one to the team about Friday's demo.".into()),
        Item::Tool {
            call_id: "gmail-draft-streaming".into(),
            name: "gmail".into(),
            input: r#"{"intent":"Draft demo note","action":"draft","to":"team@example.com","subject":"Friday demo","body":"Hi all,\n\nQuick heads up: we will demo the new Gmail draft cards on Fri"#.into(),
            output: String::new(),
            done: false,
            error: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{BODY_LINES, Outcome, clip_body, file_name, gmail_url, outcome, parse};

    #[test]
    fn only_compose_actions_become_cards() {
        assert!(parse("gmail", r#"{"action":"search","query":"x"}"#).is_none());
        assert!(parse("bash", r#"{"action":"draft"}"#).is_none());
        let draft = parse("functions.gmail", r#"{"action":"draft","to":"a@b.c","subject":"Hi","body":"Yo","thread_id":"t","attachments":["/x/y.pdf"]}"#).unwrap();
        assert_eq!(draft.to, "a@b.c");
        assert_eq!(draft.subject, "Hi");
        assert!(draft.reply);
        assert_eq!(draft.attachments, vec!["/x/y.pdf"]);
        assert_eq!(
            parse("gmail", r#"{"action":"send","to":"a"}"#)
                .unwrap()
                .action,
            "send"
        );
    }

    #[test]
    fn streaming_arguments_fill_in_progressively() {
        assert!(parse("gmail", r#"{"intent":"x","act"#).is_none());
        let partial = parse("gmail", r#"{"action":"draft","to":"sam@ex","#).unwrap();
        assert_eq!(partial.to, "sam@ex");
        let partial = parse(
            "gmail",
            r#"{"action":"draft","to":"s","body":"Line one\nLine tw"#,
        )
        .unwrap();
        assert_eq!(partial.body, "Line one\nLine tw");
        assert!(partial.subject.is_empty());
    }

    #[test]
    fn outcomes_follow_the_tool_output() {
        assert_eq!(outcome("", false), Outcome::Writing);
        assert_eq!(
            outcome("Draft created successfully.\nDraft ID: r-1\nTo: a", true),
            Outcome::Drafted {
                draft_id: Some("r-1".into())
            }
        );
        assert_eq!(
            outcome(
                "Email sent successfully.\nMessage ID: 18abc\nThread ID: t",
                true
            ),
            Outcome::Sent {
                message_id: Some("18abc".into())
            }
        );
        assert_eq!(
            outcome("Attachment not found or not a file: /x", true),
            Outcome::Note("Attachment not found or not a file: /x".into())
        );
        assert_eq!(
            gmail_url(&Outcome::Sent {
                message_id: Some("18abc".into())
            })
            .unwrap(),
            "https://mail.google.com/mail/u/0/#all/18abc"
        );
        assert!(
            gmail_url(&Outcome::Drafted { draft_id: None })
                .unwrap()
                .ends_with("#drafts")
        );
        assert!(gmail_url(&Outcome::Writing).is_none());
    }

    #[test]
    fn long_bodies_clip_with_a_count() {
        let body: String = (0..20).map(|n| format!("l{n}\n")).collect();
        let (shown, hidden) = clip_body(&body);
        assert_eq!(hidden, 6);
        assert_eq!(shown.lines().count(), BODY_LINES);
        assert_eq!(file_name("/a/b/c.pdf"), "c.pdf");
    }
}
