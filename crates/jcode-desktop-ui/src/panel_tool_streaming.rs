//! Read useful top-level fields without waiting for the entire argument object.
//! This is display-only: execution still requires complete, validated arguments.

use serde::de::IgnoredAny;

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

// Return only decoded string content. An unfinished escape (including either
// half of a surrogate pair) is withheld until it can be decoded atomically.
pub(super) fn progressive_string(input: &str) -> (String, Option<usize>) {
    debug_assert!(input.starts_with('"'));
    let mut text = String::new();
    let mut offset = 1;
    while offset < input.len() {
        let ch = input[offset..].chars().next().unwrap();
        match ch {
            '"' => return (text, Some(offset + 1)),
            '\\' => {
                let rest = &input[offset..];
                let Some(escape) = rest.as_bytes().get(1) else {
                    break;
                };
                let mut len = if *escape == b'u' { 6 } else { 2 };
                if *escape == b'u' {
                    let Some(hex) = rest.get(2..6) else { break };
                    let Ok(unit) = u16::from_str_radix(hex, 16) else {
                        break;
                    };
                    if (0xD800..=0xDBFF).contains(&unit) {
                        len = 12;
                    }
                }
                let Some(encoded) = rest.get(..len) else {
                    break;
                };
                let Ok(decoded) = serde_json::from_str::<String>(&format!("\"{encoded}\"")) else {
                    break;
                };
                text.push_str(&decoded);
                offset += len;
            }
            ch if ch < ' ' => break,
            ch => {
                text.push(ch);
                offset += ch.len_utf8();
            }
        }
    }
    (text, None)
}

pub(super) fn summary(input: &str) -> Option<String> {
    let mut rest = input.trim_start().strip_prefix('{')?;
    let mut found: Option<(usize, String)> = None;
    loop {
        rest = rest.trim_start();
        if !rest.starts_with('"') {
            break;
        }
        // Keys must be complete, unlike display values. Never interpret nested
        // fields or punctuation inside a string as top-level summary fields.
        let (key, Some(consumed)) = progressive_string(rest) else {
            break;
        };
        let Some(value) = rest[consumed..].trim_start().strip_prefix(':') else {
            break;
        };
        rest = value.trim_start();
        let rank = PREFERRED.iter().position(|candidate| *candidate == key);
        let (text, consumed) = if rank.is_some() && rest.starts_with('"') {
            let (text, consumed) = progressive_string(rest);
            (Some(text), consumed)
        } else if rank.is_some() {
            let mut values =
                serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
            match values.next() {
                Some(Ok(value)) => (super::json_scalar(&value), Some(values.byte_offset())),
                _ => break,
            }
        } else {
            let mut values = serde_json::Deserializer::from_str(rest).into_iter::<IgnoredAny>();
            match values.next() {
                Some(Ok(_)) => (None, Some(values.byte_offset())),
                _ => break,
            }
        };
        if let (Some(rank), Some(text)) = (rank, text)
            && !text.trim().is_empty()
            && found.as_ref().is_none_or(|(best, _)| rank < *best)
        {
            found = Some((rank, text));
            // Nothing outranks intent. Avoid scanning large trailing bodies.
            if rank == 0 {
                break;
            }
        }
        let Some(consumed) = consumed else { break };
        let Some(next) = rest[consumed..].trim_start().strip_prefix(',') else {
            break;
        };
        rest = next;
    }
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
        assert!(vcx.debug_bounds("tool-type-icon").is_some());
        assert!(vcx.debug_bounds("tool-icon-running").is_some());
        panel.read_with(vcx, |panel, _| {
            assert!(matches!(&panel.items[0], Item::Reasoning(text) if text == "Check first."));
            assert!(
                matches!(&panel.items[1], Item::Tool { name, input, done: false, .. }
                if name == "bash" && input.is_empty())
            );
            assert!(panel.streaming_reasoning.is_empty());
        });

        for (delta, expected) in [
            (r#"{"intent":"Check"#, "Check"),
            (" the build", "Check the build"),
            (r#"", "command":"cargo"#, "Check the build"),
        ] {
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
                assert_eq!(tool_summary(input), expected);
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
            let Item::Tool {
                input,
                done,
                output,
                ..
            } = &panel.items[1]
            else {
                panic!("missing finished tool")
            };
            assert!(*done);
            assert_eq!(output, "passed");
            let arguments: serde_json::Value = serde_json::from_str(input).unwrap();
            assert_eq!(arguments["command"], "cargo test");
            assert_eq!(tool_summary(input), "Check the build");
        });
    }

    #[gpui::test]
    fn parallel_tools_keep_interleaved_arguments_and_completion_isolated(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::super::Item;
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
        for (call_id, name) in [("call-a", "bash"), ("call-b", "agentgrep")] {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::ToolStart {
                        session_id: "session-a".into(),
                        call_id: call_id.into(),
                        name: name.into(),
                    },
                    cx,
                );
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("tool-header").is_some());
            assert!(vcx.debug_bounds("tool-type-icon").is_some());
            assert!(vcx.debug_bounds("tool-icon-running").is_some());
            panel.read_with(vcx, |panel, _| {
                assert!(panel.items.iter().any(|item| matches!(item,
                    Item::Tool { call_id: id, name: actual, input, done: false, .. }
                    if id == call_id && actual == name && input.is_empty()
                )));
            });
        }
        for (call_id, delta, expected_a, expected_b) in [
            ("call-a", r#"{"intent":"Build"#, "Build", ""),
            ("call-b", r#"{"query":"Find"#, "Build", "Find"),
            (
                "call-a",
                r#" project","command":"cargo test"}"#,
                "Build project",
                "Find",
            ),
            ("call-b", r#" matches"}"#, "Build project", "Find matches"),
        ] {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::ToolInputDelta {
                        session_id: "session-a".into(),
                        call_id: call_id.into(),
                        delta: delta.into(),
                    },
                    cx,
                );
            });
            vcx.run_until_parked();
            panel.read_with(vcx, |panel, _| {
                assert_eq!(panel.items.len(), 2);
                for (index, expected) in [(0, expected_a), (1, expected_b)] {
                    let Item::Tool { input, done, .. } = &panel.items[index] else {
                        panic!("missing parallel tool")
                    };
                    assert!(!done);
                    assert_eq!(tool_summary(input), expected);
                }
            });
        }
        // Finish B before A, keeping A's running state and input unchanged.
        for (call_id, name, expected_done) in [
            ("call-b", "agentgrep", [false, true]),
            ("call-a", "bash", [true, true]),
        ] {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::ToolDone {
                        session_id: "session-a".into(),
                        call_id: call_id.into(),
                        name: name.into(),
                        output: format!("finished {call_id}"),
                        error: None,
                    },
                    cx,
                );
            });
            vcx.run_until_parked();
            panel.read_with(vcx, |panel, _| {
                assert_eq!(panel.items.len(), 2);
                for (index, expected) in ["Build project", "Find matches"].iter().enumerate() {
                    let Item::Tool {
                        call_id,
                        input,
                        output,
                        done,
                        ..
                    } = &panel.items[index]
                    else {
                        panic!("missing parallel tool")
                    };
                    assert_eq!(*done, expected_done[index]);
                    assert_eq!(tool_summary(input), *expected);
                    assert!(serde_json::from_str::<serde_json::Value>(input).is_ok());
                    if *done {
                        assert_eq!(output, &format!("finished {call_id}"));
                    } else {
                        assert!(output.is_empty());
                    }
                }
            });
        }
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
    fn nested_intents_and_incomplete_structure_do_not_leak_json() {
        for input in ["", "{", r#"{"nested":{"intent":"wrong"},"#] {
            assert_eq!(tool_summary(input), "");
        }
    }

    #[test]
    fn fallback_upgrades_when_intent_arrives() {
        assert_eq!(
            tool_summary(r#"{"command":"cargo test","intent":"Check"#),
            "Check"
        );
        assert_eq!(
            tool_summary(r#"{"command":"cargo test","intent":"Check tests""#),
            "Check tests"
        );
    }

    #[test]
    fn partial_strings_decode_only_complete_escapes_and_unicode() {
        for (input, expected) in [
            (r#"{"intent":"Read"#, "Read"),
            (r#"{"intent":"Read \"#, "Read "),
            (r#"{"intent":"Read \u"#, "Read "),
            (r#"{"intent":"Read \u65"#, "Read "),
            (r#"{"intent":"Read \u65e5"#, "Read 日"),
            (r#"{"intent":"Read \uD83D"#, "Read "),
            (r#"{"intent":"Read \uD83D\uDE"#, "Read "),
            (r#"{"intent":"Read \uD83D\uDE80"#, "Read 🚀"),
            (
                r#"{"intent":"Read \"日本語\"\nnext"#,
                "Read \"日本語\"\nnext",
            ),
            (r#"{"intent":"Read \q","query":"wrong"}"#, "Read "),
            (r#"{"intent":"Read \uDE80","query":"wrong"}"#, "Read "),
        ] {
            assert_eq!(super::summary(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn every_string_prefix_is_safe_and_finishes_with_the_full_value() {
        let expected = "Read \"日本語\" 🚀\nnext\tpath\\file";
        let input = serde_json::json!({"intent": expected}).to_string();
        for end in (0..=input.len()).filter(|end| input.is_char_boundary(*end)) {
            if let Some(text) = super::summary(&input[..end]) {
                assert!(
                    expected.starts_with(&text),
                    "unsafe prefix at {end}: {text:?}"
                );
            }
        }
        assert_eq!(super::summary(&input).as_deref(), Some(expected));
    }

    #[test]
    fn partial_fallback_priority_and_nested_values_are_safe() {
        for (input, expected) in [
            (r#"{"path":"src/"#, "src/"),
            (r#"{"path":"src/main.rs","command":"cargo"#, "cargo"),
            (r#"{"command":"cargo test","intent":""#, "cargo test"),
            (
                r#"{"command":"cargo test","intent":"   \uD83D"#,
                "cargo test",
            ),
            (r#"{"command":"cargo test","path":"lower"#, "cargo test"),
            (r#"{"nested":{"intent":"wrong"},"query":"right"#, "right"),
            (r#"{"intent":{"query":"wrong"},"query":"right"#, "right"),
            (r#"{"query":"right","intent":{"query":"wrong"#, "right"),
        ] {
            assert_eq!(tool_summary(input), expected, "{input}");
        }
        for input in [
            r#"{"nested":{"intent":"wrong"#,
            r#"{"intent":["wrong"#,
            r#"{"intent":{"query":"wrong"#,
            r#"{"unrecognized":"\"intent\":\"wrong"#,
            r#"[{"intent":"wrong"}]"#,
            r#"{"intent"#,
            r#"{"intent":"\uD83D"#,
        ] {
            assert_eq!(super::summary(input), None, "{input}");
        }
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
