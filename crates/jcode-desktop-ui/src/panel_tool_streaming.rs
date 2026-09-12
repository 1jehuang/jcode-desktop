//! Read useful top-level fields without waiting for the entire argument object.
//! This is display-only: execution still requires complete, validated arguments.

use serde::de::{Deserializer as _, IgnoredAny, MapAccess, Visitor};

const PREFERRED: &[&str] = &[
    "intent",
    "command",
    "query",
    "file_path",
    "path",
    "pattern",
    "url",
    "prompt",
    "content",
    "task",
    "action",
];

pub(super) fn summary(input: &str) -> Option<String> {
    struct Fields<'a>(&'a mut Option<(usize, String)>);

    impl<'de> Visitor<'de> for Fields<'_> {
        type Value = ();

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a tool argument object")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
            while let Some(key) = map.next_key::<String>()? {
                let rank = PREFERRED.iter().position(|candidate| *candidate == key);
                if let Some(rank) = rank {
                    let value = map.next_value::<serde_json::Value>()?;
                    if let Some(text) = super::json_scalar(&value)
                        && !text.trim().is_empty()
                        && self.0.as_ref().is_none_or(|(best, _)| rank < *best)
                    {
                        *self.0 = Some((rank, text));
                        // Nothing outranks intent. Avoid scanning a potentially
                        // huge patch or file body on every streaming repaint.
                        if rank == 0 {
                            return Ok(());
                        }
                    }
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }

    let mut found = None;
    // EOF in a later field must not discard earlier, fully decoded values.
    let _ = serde_json::Deserializer::from_str(input).deserialize_map(Fields(&mut found));
    found.map(|(_, text)| text)
}

pub(super) fn fixture_items() -> Vec<super::Item> {
    use super::Item;
    vec![
        Item::User("Show tools as soon as their names arrive.".into()),
        Item::Assistant("The first tool has only a name. The second already has an intent, but its arguments are still arriving.".into()),
        Item::Tool {
            call_id: "name-only".into(),
            name: "agentgrep".into(),
            input: String::new(),
            output: String::new(),
            done: false,
            error: None,
        },
        Item::Tool {
            call_id: "partial-intent".into(),
            name: "bash".into(),
            input: r#"{"intent":"Check the build","command":"cargo"#.into(),
            output: String::new(),
            done: false,
            error: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::super::tool_summary;

    #[gpui::test]
    fn tool_streaming_paints_name_then_intent_before_done(cx: &mut gpui::TestAppContext) {
        use super::super::Item;
        use gpui::px;
        use jcode_sdk::ApiEvent;

        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::ReasoningDelta {
                    session_id: "session-a".into(),
                    text: "Check first.".into(),
                },
                cx,
            );
            panel.apply(
                &ApiEvent::ToolStart {
                    session_id: "session-a".into(),
                    call_id: "early".into(),
                    name: "bash".into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let header = vcx
            .debug_bounds("tool-header")
            .expect("name-only tool paints");
        assert!(header.size.width > px(0.) && header.size.height > px(0.));
        assert!(vcx.debug_bounds("tool-summary").is_none());
        panel.read_with(vcx, |panel, _| {
            assert!(matches!(&panel.items[0], Item::Reasoning(text) if text == "Check first."));
            assert!(
                matches!(&panel.items[1], Item::Tool { name, input, done: false, .. }
                if name == "bash" && input.is_empty())
            );
            assert!(panel.streaming_reasoning.is_empty());
        });

        for delta in [r#"{"intent":"Check the build""#, r#", "command":"cargo"#] {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::ToolInputDelta {
                        session_id: "session-a".into(),
                        call_id: "early".into(),
                        delta: delta.into(),
                    },
                    cx,
                )
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("tool-summary").is_some());
            panel.read_with(vcx, |panel, _| {
                assert_eq!(panel.items.len(), 2, "deltas update the same row");
                let Item::Tool { input, done, .. } = &panel.items[1] else {
                    panic!("missing tool")
                };
                assert!(!done);
                assert!(serde_json::from_str::<serde_json::Value>(input).is_err());
                assert_eq!(tool_summary(input), "Check the build");
            });
        }
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::ToolInputDelta {
                    session_id: "session-a".into(),
                    call_id: "early".into(),
                    delta: " test\"}".into(),
                },
                cx,
            );
            panel.apply(
                &ApiEvent::ToolDone {
                    session_id: "session-a".into(),
                    call_id: "early".into(),
                    name: "bash".into(),
                    output: "passed".into(),
                    error: None,
                },
                cx,
            );
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert_eq!(panel.items.len(), 2);
            assert!(matches!(&panel.items[1], Item::Tool { done: true, output, .. } if output == "passed"));
        });
    }

    #[test]
    fn intent_appears_before_arguments_finish() {
        for input in [
            r#"{"intent":"Check the build""#,
            r#"{"intent":"Check the build","command":"cargo"#,
            r#"{"intent":"Check the build","command":"cargo test"}"#,
        ] {
            assert_eq!(tool_summary(input), "Check the build");
        }
    }

    #[test]
    fn incomplete_strings_and_nested_intents_do_not_leak_json() {
        for input in [
            "",
            "{",
            r#"{"intent":"Check"#,
            r#"{"nested":{"intent":"wrong"},"#,
        ] {
            assert_eq!(tool_summary(input), "");
        }
    }

    #[test]
    fn fallback_upgrades_when_intent_arrives() {
        assert_eq!(
            tool_summary(r#"{"command":"cargo test","intent":"Check"#),
            "cargo test"
        );
        assert_eq!(
            tool_summary(r#"{"command":"cargo test","intent":"Check tests""#),
            "Check tests"
        );
    }

    #[test]
    fn decodes_escaped_and_unicode_values_without_reading_the_tail() {
        assert_eq!(
            tool_summary(r#"{"intent":"Read \"日本語\" \uD83D\uDE80","content":""#),
            "Read \"日本語\" 🚀"
        );
    }

    #[test]
    fn skips_empty_and_structured_fields_and_keeps_preference_order() {
        assert_eq!(
            tool_summary(r#"{"intent":"  ","content":"body","path":"src/main.rs","other":["#),
            "src/main.rs"
        );
        assert_eq!(
            tool_summary(r#"{"intent":{},"action":true,"other":"#),
            "true"
        );
    }
}
