use super::*;
use crate::sounds::{Cue, drain_requested};
use serde_json::{Value, json};

fn event(mut value: Value) -> ApiEvent {
    value["session_id"] = json!("sound-test");
    serde_json::from_value(value).unwrap()
}

fn live_panel(cx: &mut gpui::TestAppContext) -> Entity<Panel> {
    let (bridge, _commands) = crate::harness::spawn_recording();
    cx.new(|cx| Panel::new("sound-test".into(), None, None, bridge, cx))
}

fn apply_steps(
    panel: &Entity<Panel>,
    cx: &mut gpui::TestAppContext,
    steps: &[(Value, Option<Cue>)],
) {
    for (value, expected) in steps {
        let event = event(value.clone());
        panel.update(cx, |panel, cx| {
            panel.apply(&event, cx);
            assert_eq!(
                drain_requested(cx),
                expected.iter().copied().collect::<Vec<_>>(),
                "{event:?}"
            );
        });
    }
}

#[gpui::test]
fn live_turn_completion_and_terminal_error_cues(cx: &mut gpui::TestAppContext) {
    let panel = live_panel(cx);
    apply_steps(
        &panel,
        cx,
        &[
            (json!({"ev":"turn_done"}), None),
            (json!({"ev":"message_accepted"}), None),
            (json!({"ev":"text_delta", "text":"done"}), None),
            (json!({"ev":"turn_done"}), Some(Cue::Complete)),
            (json!({"ev":"turn_done"}), None),
            (json!({"ev":"message_accepted"}), None),
            (
                json!({"ev":"error", "code":"internal", "message":"failed"}),
                Some(Cue::Error),
            ),
            (
                json!({"ev":"error", "code":"internal", "message":"failed"}),
                None,
            ),
            (json!({"ev":"turn_done"}), None),
        ],
    );
}

#[gpui::test]
fn attention_and_background_cues_are_once_per_id(cx: &mut gpui::TestAppContext) {
    let panel = live_panel(cx);
    let permission = json!({"ev":"permission_request", "request_id":"p", "tool_name":"bash", "description":"allow?"});
    let progress = |done| json!({"ev":"background_progress", "task_id":"b", "label":"build", "summary":"building", "done":done});
    apply_steps(
        &panel,
        cx,
        &[
            (permission.clone(), Some(Cue::Attention)),
            (permission, None),
            (progress(false), None),
            (progress(true), Some(Cue::BackgroundComplete)),
            (progress(true), None),
            (json!({"ev":"turn_done"}), None),
        ],
    );
}

#[gpui::test]
fn history_status_and_preview_are_silent(cx: &mut gpui::TestAppContext) {
    let panel = live_panel(cx);
    apply_steps(
        &panel,
        cx,
        &[
            (
                json!({"ev":"history", "messages":[{"role":"assistant", "content":"past response"}]}),
                None,
            ),
            (json!({"ev":"session_status", "status":"running"}), None),
            (json!({"ev":"session_status", "status":"idle"}), None),
            (json!({"ev":"turn_done"}), None),
            (json!({"ev":"future_soundless_event"}), None),
        ],
    );
    let preview = cx.new(|cx| Panel::new_preview(PreviewState::Streaming, cx));
    cx.update(|cx| assert!(drain_requested(cx).is_empty()));
    apply_steps(
        &preview,
        cx,
        &[
            (json!({"ev":"text_delta", "text":"preview"}), None),
            (json!({"ev":"turn_done"}), None),
            (
                json!({"ev":"error", "code":"internal", "message":"preview error"}),
                None,
            ),
            (
                json!({"ev":"permission_request", "request_id":"preview", "tool_name":"bash", "description":"preview"}),
                None,
            ),
            (
                json!({"ev":"background_progress", "task_id":"preview", "label":"test", "summary":"done", "done":true}),
                None,
            ),
        ],
    );
    preview.update(cx, |panel, cx| {
        panel.message_failed("preview local error".into(), cx);
        assert!(drain_requested(cx).is_empty());
    });
}

#[gpui::test]
fn local_send_failure_and_both_cancel_statuses_prevent_completion(cx: &mut gpui::TestAppContext) {
    let panel = live_panel(cx);
    panel.update(cx, |panel, cx| {
        panel.apply(&event(json!({"ev":"message_accepted"})), cx);
        panel.message_failed("local send failed".into(), cx);
        assert_eq!(drain_requested(cx), vec![Cue::Error]);
    });
    apply_steps(&panel, cx, &[(json!({"ev":"turn_done"}), None)]);
    for status in ["cancelled", "canceled"] {
        apply_steps(
            &panel,
            cx,
            &[
                (json!({"ev":"message_accepted"}), None),
                (json!({"ev":"session_status", "status":status}), None),
                (json!({"ev":"text_delta", "text":"late queued delta"}), None),
                (json!({"ev":"turn_done"}), None),
            ],
        );
    }
}

#[gpui::test]
fn actual_prompt_submit_sounds_sent_and_escape_cancels_silently(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("sound-test".into(), None, None, bridge, cx));
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        panel
            .read(cx)
            .input
            .read(cx)
            .focus_handle
            .clone()
            .focus(window, cx);
        assert!(drain_requested(cx).is_empty());
    });
    vcx.simulate_input("hello sound test");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    vcx.update(|_, cx| assert_eq!(drain_requested(cx), vec![Cue::Sent]));
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "hello sound test")
    );
    panel.update(vcx, |panel, cx| {
        panel.apply(&event(json!({"ev":"message_accepted"})), cx);
        panel.apply(&event(json!({"ev":"text_delta", "text":"working"})), cx);
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(matches!(commands.try_recv(), Ok(Command::Cancel { .. })));
    panel.update(vcx, |panel, cx| {
        panel.apply(&event(json!({"ev":"text_delta", "text":"late delta"})), cx);
        panel.apply(&event(json!({"ev":"turn_done"})), cx);
        assert!(drain_requested(cx).is_empty());
    });
}
