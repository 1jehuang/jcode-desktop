//! Gmail reads render as email, not as tool text.
//!
//! A finished `gmail` call with `action: read` shows the message it read
//! (sender, date, subject, labels and body). `action: thread` shows the
//! conversation as a stack of messages. Both parse the tool's own output, so
//! the card never touches the network. In-flight or failed calls, and output
//! that does not parse, keep the generic tool row.
use crate::text_selection::{self, TextSelection};
use crate::theme::Theme;
use gpui::{prelude::*, *};

const BODY_LINES: usize = 12;
const THREAD_COLLAPSED: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Message {
    pub id: String,
    pub from: String,
    pub date: String,
    pub subject: String,
    pub snippet: String,
    pub labels: Vec<String>,
    pub attachments: Vec<String>,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum View {
    Read(Message),
    Thread { id: String, messages: Vec<Message> },
}

fn action(input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    Some(value.get("action")?.as_str()?.to_owned())
}

/// Parse "Key: value" header lines plus an attachment list into `message`.
fn fill(message: &mut Message, block: &str) {
    let mut in_attachments = false;
    for line in block.lines() {
        if in_attachments {
            if let Some(name) = line.strip_prefix("  - ") {
                message.attachments.push(name.trim().to_owned());
                continue;
            }
            in_attachments = false;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key {
            "ID" => message.id = value,
            "From" => message.from = value,
            "Date" => message.date = value,
            "Subject" => message.subject = value,
            "Snippet" => message.snippet = value,
            "Labels" => {
                message.labels = value
                    .split(',')
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
            key if key.starts_with("Attachments") => in_attachments = true,
            _ => {}
        }
    }
}

fn parse_read(output: &str) -> Option<Message> {
    if !output.starts_with("From:") {
        return None;
    }
    let (head, body) = match output.split_once("\n\n--- Body ---\n") {
        Some((head, body)) => (head, body),
        None => (output, ""),
    };
    let mut message = Message::default();
    fill(&mut message, head);
    message.body = decode_entities(body.trim_end());
    (!message.id.is_empty()).then_some(message)
}

fn parse_thread(output: &str) -> Option<View> {
    let first = output.lines().next()?;
    let id = first
        .strip_prefix("Thread ")?
        .split_whitespace()
        .next()?
        .to_owned();
    let messages: Vec<Message> = output
        .split("--- Message ")
        .skip(1)
        .map(|block| {
            let mut message = Message::default();
            fill(
                &mut message,
                block.split_once('\n').map_or("", |(_, rest)| rest),
            );
            message.snippet = decode_entities(&message.snippet);
            message
        })
        .collect();
    (!messages.is_empty()).then_some(View::Thread { id, messages })
}

/// A card view of a finished `gmail` read or thread call.
pub(super) fn parse(
    name: &str,
    input: &str,
    output: &str,
    done: bool,
    error: Option<&str>,
) -> Option<View> {
    if name.trim_start_matches("functions.") != "gmail" || !done || error.is_some() {
        return None;
    }
    let output = output.trim();
    match action(input)?.as_str() {
        "read" => parse_read(output).map(View::Read),
        "thread" => parse_thread(output),
        _ => None,
    }
}

/// Gmail snippets arrive HTML-escaped.
fn decode_entities(text: &str) -> String {
    text.replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// "Sam Lee <sam@example.com>" reads as "Sam Lee".
pub(super) fn sender_name(from: &str) -> String {
    let from = from.trim();
    match from.split_once('<') {
        Some((name, address)) => {
            let name = name.trim().trim_matches('"').trim();
            if name.is_empty() {
                address.trim_end_matches('>').trim().to_owned()
            } else {
                name.to_owned()
            }
        }
        None if from.is_empty() => "(unknown)".into(),
        None => from.to_owned(),
    }
}

/// "Tue, 23 Sep 2025 14:05:11 -0700 (PDT)" reads as "Tue, 23 Sep 2025 14:05".
pub(super) fn short_date(date: &str) -> String {
    let parts: Vec<&str> = date.split_whitespace().collect();
    if let Some(index) = parts.iter().position(|part| part.matches(':').count() == 2) {
        let time = parts[index]
            .rsplit_once(':')
            .map_or(parts[index], |(hm, _)| hm);
        let mut out = parts[..index].join(" ");
        out.push(' ');
        out.push_str(time);
        return out;
    }
    date.trim().to_owned()
}

/// Labels worth showing: user labels and a few meaningful system ones.
fn visible_labels(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .filter_map(|label| match label.as_str() {
            "UNREAD" => Some("Unread".into()),
            "IMPORTANT" => Some("Important".into()),
            "STARRED" => Some("Starred".into()),
            "SENT" => Some("Sent".into()),
            "DRAFT" => Some("Draft".into()),
            label if label.starts_with("CATEGORY_") || label == "INBOX" => None,
            label if label.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') => None,
            label if label.starts_with("Label_") => None,
            label => Some(label.to_owned()),
        })
        .collect()
}

fn safe_id(id: &str) -> Option<&str> {
    (!id.is_empty() && id.chars().all(|ch| ch.is_ascii_alphanumeric())).then_some(id)
}

pub(super) fn gmail_url(view: &View) -> Option<String> {
    let id = match view {
        View::Read(message) => safe_id(&message.id)?,
        View::Thread { id, .. } => safe_id(id)?,
    };
    Some(format!("https://mail.google.com/mail/u/0/#all/{id}"))
}

fn clip_body(body: &str) -> (String, usize) {
    let lines: Vec<&str> = body.trim_end().lines().collect();
    if lines.len() <= BODY_LINES {
        return (lines.join("\n"), 0);
    }
    (lines[..BODY_LINES].join("\n"), lines.len() - BODY_LINES)
}

/// Whether the card has anything to expand.
pub(super) fn expandable(view: &View) -> bool {
    match view {
        View::Read(message) => clip_body(&message.body).1 > 0,
        View::Thread { messages, .. } => messages.len() > THREAD_COLLAPSED + 1,
    }
}

fn pill(theme: &Theme, text: String) -> Div {
    div()
        .flex_none()
        .px_2()
        .rounded_full()
        .bg(theme.INLINE_CODE_BG)
        .text_size(px(11.0))
        .line_height(px(18.0))
        .text_color(theme.TEXT_DIM)
        .child(text)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    index: usize,
    view: &View,
    expanded: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    selection: &Entity<TextSelection>,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global();
    let plain =
        |key: String, text: String| text_selection::plain(selection.clone(), key, text, window, cx);
    let url = gmail_url(view);
    let (subject, status) = match view {
        View::Read(message) => (message.subject.clone(), "Read".to_owned()),
        View::Thread { messages, .. } => (
            messages
                .iter()
                .map(|message| message.subject.as_str())
                .find(|subject| !subject.is_empty())
                .unwrap_or_default()
                .to_owned(),
            format!(
                "{} message{}",
                messages.len(),
                if messages.len() == 1 { "" } else { "s" }
            ),
        ),
    };
    let subject = if subject.is_empty() {
        "(no subject)".to_owned()
    } else {
        subject
    };

    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .bg(theme.HEADER_BG)
        .child(crate::tool_icon::render_badge("gmail", true, false))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.TOOL_TEXT)
                .child(plain(format!("gmail-read-subject-{index}"), subject)),
        )
        .child(
            div()
                .flex_none()
                .debug_selector(|| "gmail-read-status".into())
                .text_size(px(12.0))
                .text_color(theme.TEXT_FAINT)
                .child(status),
        )
        .when_some(url, |el, url| {
            el.child(
                div()
                    .id(("gmail-read-open", index))
                    .debug_selector(|| "gmail-read-open".into())
                    .flex_none()
                    .px_2()
                    .rounded_full()
                    .bg(theme.INLINE_CODE_BG)
                    .text_size(px(12.0))
                    .text_color(theme.LINK)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.ACCENT_MUTED))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        if !crate::harness::screenshot_mode() {
                            cx.open_url(&url);
                        }
                    })
                    .child("Open in Gmail"),
            )
        });

    let on_toggle = std::rc::Rc::new(on_toggle);
    let toggle = |label: String| {
        let on_toggle = on_toggle.clone();
        div()
            .id(("gmail-read-toggle", index))
            .debug_selector(|| "gmail-read-more".into())
            .mx_3()
            .mb_2()
            .text_size(px(12.0))
            .text_color(theme.TEXT_DIM)
            .cursor_pointer()
            .hover(|style| style.text_color(theme.TEXT))
            .on_click(move |event, window, cx| on_toggle(event, window, cx))
            .child(label)
    };

    let content = match view {
        View::Read(message) => {
            let labels = visible_labels(&message.labels);
            let (body, hidden) = if expanded {
                (message.body.trim_end().to_owned(), 0)
            } else {
                clip_body(&message.body)
            };
            let body = if body.trim().is_empty() {
                decode_entities(&message.snippet)
            } else {
                body
            };
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .pt_2()
                        .min_w_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.TEXT)
                                .font_weight(FontWeight::MEDIUM)
                                .child(plain(
                                    format!("gmail-read-from-{index}"),
                                    message.from.clone(),
                                )),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.0))
                                .text_color(theme.TEXT_FAINT)
                                .child(short_date(&message.date)),
                        ),
                )
                .when(
                    !labels.is_empty() || !message.attachments.is_empty(),
                    |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap_1()
                                .px_3()
                                .pt_1()
                                .children(labels.into_iter().map(|label| pill(theme, label)))
                                .children(
                                    message
                                        .attachments
                                        .iter()
                                        .map(|name| pill(theme, format!("📎 {name}"))),
                                ),
                        )
                    },
                )
                .when(!body.trim().is_empty(), |el| {
                    el.child(
                        div()
                            .debug_selector(|| "gmail-read-body".into())
                            .mx_3()
                            .mt_2()
                            .mb_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(theme.TOOL_BORDER)
                            .font_family(theme.FONT_AI)
                            .text_color(theme.TEXT)
                            .whitespace_normal()
                            .child(plain(format!("gmail-read-body-{index}"), body)),
                    )
                })
                .when(hidden > 0, |el| {
                    el.child(toggle(format!(
                        "Show {hidden} more line{}",
                        if hidden == 1 { "" } else { "s" }
                    )))
                })
                .when(expanded && expandable(view), |el| {
                    el.child(toggle("Show less".into()))
                })
                .into_any_element()
        }
        View::Thread { messages, .. } => {
            // Keep the newest messages visible, hiding the oldest middle ones.
            let hidden = if expanded || messages.len() <= THREAD_COLLAPSED + 1 {
                0
            } else {
                messages.len() - THREAD_COLLAPSED
            };
            let last = messages.len().saturating_sub(1);
            let row = |n: usize, message: &Message| {
                let latest = n == last;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .mx_2()
                    .px_2()
                    .py_1p5()
                    .rounded_lg()
                    .when(latest, |el| el.bg(theme.INLINE_CODE_BG))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.TEXT)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(plain(
                                        format!("gmail-thread-from-{index}-{n}"),
                                        sender_name(&message.from),
                                    )),
                            )
                            .when(!message.attachments.is_empty(), |el| {
                                el.child(pill(theme, format!("📎 {}", message.attachments.len())))
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.0))
                                    .text_color(theme.TEXT_FAINT)
                                    .child(short_date(&message.date)),
                            ),
                    )
                    .when(!message.snippet.is_empty(), |el| {
                        el.child(
                            div()
                                .text_color(if latest { theme.TEXT } else { theme.TEXT_DIM })
                                .font_family(theme.FONT_AI)
                                .whitespace_normal()
                                .child(plain(
                                    format!("gmail-thread-snippet-{index}-{n}"),
                                    message.snippet.clone(),
                                )),
                        )
                    })
                    .into_any_element()
            };
            let mut rows: Vec<AnyElement> = Vec::new();
            if hidden > 0 {
                rows.push(row(0, &messages[0]));
                rows.push(
                    toggle(format!(
                        "Show {hidden} earlier message{}",
                        if hidden == 1 { "" } else { "s" }
                    ))
                    .mx_4()
                    .my_1()
                    .into_any_element(),
                );
                let start = messages.len() - (THREAD_COLLAPSED - 1);
                for (n, message) in messages.iter().enumerate().skip(start) {
                    rows.push(row(n, message));
                }
            } else {
                for (n, message) in messages.iter().enumerate() {
                    rows.push(row(n, message));
                }
            }
            div()
                .debug_selector(|| "gmail-thread-messages".into())
                .flex()
                .flex_col()
                .gap_1()
                .py_2()
                .children(rows)
                .when(expanded && expandable(view), |el| {
                    el.child(toggle("Show fewer".into()).mt_1().mb_0())
                })
                .into_any_element()
        }
    };

    div()
        .id(("gmail-read", index))
        .debug_selector(|| "gmail-read-card".into())
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
        .child(content)
        .into_any_element()
}

pub(super) fn fixture_items() -> Vec<super::Item> {
    use super::Item;
    let read_output = "From: Sam Lee <sam@example.com>\nSubject: Q3 report\nDate: Tue, 23 Sep 2025 14:05:11 -0700\nLabels: UNREAD, IMPORTANT, CATEGORY_PERSONAL, INBOX, Finance\nSnippet: Hi Jeremy, attached are the Q3 numbers.\nID: 18c2f0a1b2c3d4e5\nAttachments (1):\n  - q3-report.pdf (application/pdf, 412.0 KB)\n\n--- Body ---\nHi Jeremy,\n\nAttached are the Q3 numbers. Revenue is up 18% quarter over quarter, mostly from the Desktop beta.\n\nCould you take a look before Thursday? I would like to share them with the team on Friday.\n\nThanks,\nSam";
    let thread_output = "Thread 18c2f0a1b2c3d4e0 (5 messages):\n\n--- Message 1 ---\nID: 18c2f0a1b2c3d4e0\nFrom: Priya Shah <priya@example.com>\nDate: Mon, 22 Sep 2025 09:12:40 -0700\nSubject: Friday demo plan\nSnippet: Proposing we demo the new Gmail cards and the sidebar on Friday. Thoughts?\n\n--- Message 2 ---\nID: 18c2f0a1b2c3d4e1\nFrom: Jeremy Huang <jeremy@example.com>\nDate: Mon, 22 Sep 2025 10:03:02 -0700\nSubject: Re: Friday demo plan\nSnippet: Sounds good. I can cover the transcript cards.\n\n--- Message 3 ---\nID: 18c2f0a1b2c3d4e2\nFrom: Alex Kim <alex@example.com>\nDate: Mon, 22 Sep 2025 11:47:19 -0700\nSubject: Re: Friday demo plan\nSnippet: I&#39;ll take voice. Do we have a room booked?\n\n--- Message 4 ---\nID: 18c2f0a1b2c3d4e3\nFrom: Priya Shah <priya@example.com>\nDate: Tue, 23 Sep 2025 08:30:55 -0700\nSubject: Re: Friday demo plan\nSnippet: Booked Room 4 from 2 to 3pm. Slides attached.\nAttachments (1):\n  - demo-slides.key (application/octet-stream, 3.2 MB)\n\n--- Message 5 ---\nID: 18c2f0a1b2c3d4e4\nFrom: Sam Lee <sam@example.com>\nDate: Tue, 23 Sep 2025 15:21:08 -0700\nSubject: Re: Friday demo plan\nSnippet: Perfect, I will bring the Q3 numbers so we can close with them.";
    vec![
        Item::User("What did Sam send about Q3?".into()),
        Item::Tool {
            call_id: "gmail-read".into(),
            name: "gmail".into(),
            input: serde_json::json!({"intent": "Read Sam's Q3 email", "action": "read", "message_id": "18c2f0a1b2c3d4e5"}).to_string(),
            output: read_output.into(),
            done: true,
            error: None,
        },
        Item::Assistant("Sam sent the Q3 report and wants your review before Thursday.".into()),
        Item::User("And catch me up on the demo thread.".into()),
        Item::Tool {
            call_id: "gmail-thread".into(),
            name: "gmail".into(),
            input: serde_json::json!({"intent": "Show demo thread", "action": "thread", "thread_id": "18c2f0a1b2c3d4e0"}).to_string(),
            output: thread_output.into(),
            done: true,
            error: None,
        },
        Item::Assistant("The demo is Friday 2 to 3pm in Room 4. Priya shared slides and Sam will close with Q3.".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        Message, THREAD_COLLAPSED, View, expandable, gmail_url, parse, sender_name, short_date,
        visible_labels,
    };

    const READ: &str = "From: \"Sam Lee\" <sam@example.com>\nSubject: Q3\nDate: Tue, 23 Sep 2025 14:05:11 -0700 (PDT)\nLabels: UNREAD, INBOX, CATEGORY_UPDATES, Label_9, Finance\nSnippet: Hi\nID: 18abc\nAttachments (2):\n  - a.pdf (application/pdf, 1.0 KB)\n  - b.png\n\n--- Body ---\nHello\n\nBye\n";

    #[test]
    fn read_output_becomes_a_message() {
        let View::Read(message) = parse(
            "gmail",
            r#"{"action":"read","message_id":"18abc"}"#,
            READ,
            true,
            None,
        )
        .unwrap() else {
            panic!("expected read view");
        };
        assert_eq!(message.id, "18abc");
        assert_eq!(sender_name(&message.from), "Sam Lee");
        assert_eq!(short_date(&message.date), "Tue, 23 Sep 2025 14:05");
        assert_eq!(
            message.attachments,
            vec!["a.pdf (application/pdf, 1.0 KB)", "b.png"]
        );
        assert_eq!(message.body, "Hello\n\nBye");
        assert_eq!(visible_labels(&message.labels), vec!["Unread", "Finance"]);
    }

    #[test]
    fn only_finished_read_and_thread_calls_become_cards() {
        let read = r#"{"action":"read"}"#;
        assert!(parse("gmail", read, READ, false, None).is_none());
        assert!(parse("gmail", read, READ, true, Some("boom")).is_none());
        assert!(parse("gmail", read, "Gmail is not connected.", true, None).is_none());
        assert!(parse("gmail", r#"{"action":"search"}"#, READ, true, None).is_none());
        assert!(parse("bash", read, READ, true, None).is_none());
        assert!(parse("functions.gmail", read, READ, true, None).is_some());
    }

    #[test]
    fn thread_output_becomes_a_conversation() {
        let output = "Thread t1 (2 messages):\n\n--- Message 1 ---\nID: m1\nFrom: A <a@x>\nDate: Mon, 22 Sep 2025 09:12:40 -0700\nSubject: Plan\nSnippet: I&#39;m in\nAttachments (1):\n  - s.key\n\n--- Message 2 ---\nID: m2\nFrom: b@x\nDate: \nSubject: Re: Plan\nSnippet: ok";
        let view = parse(
            "gmail",
            r#"{"action":"thread","thread_id":"t1"}"#,
            output,
            true,
            None,
        )
        .unwrap();
        let View::Thread { id, messages } = &view else {
            panic!("expected thread")
        };
        assert_eq!(id, "t1");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].snippet, "I'm in");
        assert_eq!(messages[0].attachments, vec!["s.key"]);
        assert_eq!(sender_name(&messages[1].from), "b@x");
        assert_eq!(
            gmail_url(&view).unwrap(),
            "https://mail.google.com/mail/u/0/#all/t1"
        );
        assert!(!expandable(&view));
        assert!(
            parse(
                "gmail",
                r#"{"action":"thread"}"#,
                "Thread has no messages.",
                true,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn long_bodies_and_threads_are_expandable() {
        let body: String = (0..20).map(|n| format!("l{n}\n")).collect();
        let message = Message {
            body,
            ..Message::default()
        };
        assert!(expandable(&View::Read(message)));
        let messages = vec![Message::default(); THREAD_COLLAPSED + 1];
        assert!(!expandable(&View::Thread {
            id: "t".into(),
            messages
        }));
        let messages = vec![Message::default(); THREAD_COLLAPSED + 2];
        assert!(expandable(&View::Thread {
            id: "t".into(),
            messages
        }));
    }
}
