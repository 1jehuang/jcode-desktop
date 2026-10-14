use super::*;

#[test]
fn screenshot_selection_excludes_tool_metadata_but_keeps_user_content() {
    let item = Item::Tool {
        call_id: "private-call".into(),
        name: "agentgrep".into(),
        input: "private/path".into(),
        output: "private-output".into(),
        done: true,
        error: None,
    };
    let mut display = crate::config::DisplayConfig::default();
    assert!(!text_document::segments_with_display(&item, 0, false, true, &display).is_empty());
    display.screenshot_mode = true;
    assert!(text_document::segments_with_display(&item, 0, false, true, &display).is_empty());
    let user = Item::User("User content is not automatically redacted".into());
    assert!(!text_document::segments_with_display(&user, 1, false, false, &display).is_empty());
}

#[test]
fn compact_notification_selection_preserves_full_detail() {
    let notice = Item::Stopped(stop_reason::StopNotice::cancel_requested());
    let display = crate::config::DisplayConfig::default();
    let collapsed = text_document::segments_with_display(&notice, 0, false, false, &display);
    let expanded = text_document::segments_with_display(&notice, 0, false, true, &display);
    assert_eq!(collapsed, expanded);
    assert!(expanded[0].1.contains("Waiting for the runtime"));
}

#[gpui::test]
fn display_thinking_modes_filter_rows_without_mutating_history(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let panel = cx.new(|cx| {
            let mut panel = Panel::new(
                "display-test".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            );
            *panel.items = vec![
                Item::Reasoning("old private reasoning".into()),
                Item::Assistant("answer".into()),
            ];
            panel.streaming_reasoning = "current reasoning".into();
            panel
        });
        panel.read_with(cx, |panel, _| {
            let mut display = crate::config::DisplayConfig::default();
            assert_eq!(panel.transcript_render_rows_with_display(&display).len(), 3);
            display.thinking_display = crate::config::ThinkingDisplay::Current;
            let current = panel.transcript_render_rows_with_display(&display);
            assert_eq!(current.len(), 2);
            assert_eq!(current[1].index, usize::MAX - 1);
            display.thinking_display = crate::config::ThinkingDisplay::Off;
            assert_eq!(panel.transcript_render_rows_with_display(&display).len(), 1);
            assert_eq!(panel.items.len(), 2);
            assert!(
                matches!(&panel.items[0], Item::Reasoning(text) if text == "old private reasoning")
            );
            display.thinking_display = crate::config::ThinkingDisplay::Full;
            assert_eq!(panel.transcript_render_rows_with_display(&display).len(), 3);
        });
    });
}

#[test]
fn every_display_preference_participates_in_derived_cache_key() {
    let base = TranscriptDerivedKey {
        items_revision: 1,
        applet_rows_version: 2,
        streaming: 3,
        expansions: 4,
        display: crate::config::DisplayConfig::default(),
    };
    for setting in [
        "alignment",
        "compact_notifications",
        "show_agentgrep_output",
        "tool_call_details",
        "thinking_display",
        "diff_display",
        "debug_visual",
        "screenshot_mode",
    ] {
        let mut next = base.clone();
        match setting {
            "alignment" => next.display.alignment = crate::config::TextAlignment::Right,
            "compact_notifications" => next.display.compact_notifications = false,
            "show_agentgrep_output" => next.display.show_agentgrep_output = true,
            "tool_call_details" => next.display.tool_call_details = true,
            "thinking_display" => {
                next.display.thinking_display = crate::config::ThinkingDisplay::Off
            }
            "diff_display" => next.display.diff_display = crate::config::DiffDisplay::File,
            "debug_visual" => next.display.debug_visual = true,
            "screenshot_mode" => next.display.screenshot_mode = true,
            _ => unreachable!(),
        }
        assert!(base != next, "{setting} must invalidate derived rows");
    }
}
