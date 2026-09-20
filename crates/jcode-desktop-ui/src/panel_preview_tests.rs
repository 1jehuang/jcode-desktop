use super::*;

fn assert_preview(state: PreviewState, cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(state, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("preview-badge").is_some());
    assert!(vcx.debug_bounds("preview-reset").is_some());
    panel.read_with(vcx, |panel, _| {
        assert_eq!(panel.preview_state, Some(state));
        assert!(!panel.can_fork());
        assert!(panel.session_id.starts_with("preview://"));
        match state {
            PreviewState::Empty => assert!(panel.items.is_empty()),
            PreviewState::Streaming => {
                assert_eq!(panel.status, "running");
                assert!(!panel.streaming_text.is_empty());
            }
            PreviewState::LoginDialogError => assert!(panel.login.is_some()),
            PreviewState::Interrupted | PreviewState::Crashed => {
                assert!(panel.items.iter().any(|item| matches!(item, Item::Stopped(_))));
                assert!(!panel.activity_active());
            }
            _ => assert!(
                panel
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::Error(_)))
            ),
        }
        if state == PreviewState::Disconnected {
            assert_eq!(panel.status, "disconnected");
        }
    });
    let (bridge, commands) = crate::harness::spawn_recording();
    panel.update(vcx, |panel, cx| {
        panel.bridge = bridge;
        for command in [
            "hello",
            "/terminal",
            "/gmail",
            "/todoist",
            "/file /etc/passwd",
            "/fork",
            "/clear",
            "/model test",
            "/login openai",
        ] {
            assert!(panel.handle_slash_command(command, cx));
        }
        panel.run_session_operation(SessionOperation::Clear, "not sent");
        panel.submit_command_prompt("not sent", cx);
        assert!(panel.terminal.is_none());
        assert!(panel.gmail_inbox.is_none());
        assert!(panel.todoist.is_none());
        assert!(panel.code_file.is_none());
        panel.close_login_picker(cx);
    });
    assert!(commands.try_recv().is_err());
    let id = panel.read_with(vcx, |panel, _| panel.session_id.clone());
    let reset = vcx.debug_bounds("preview-reset").unwrap();
    vcx.simulate_click(reset.center(), gpui::Modifiers::default());
    panel.read_with(vcx, |panel, _| {
        assert_eq!(panel.preview_state, Some(state));
        assert_eq!(panel.session_id, id);
    });
}

#[gpui::test]
fn preview_empty(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::Empty, cx);
}
#[gpui::test]
fn preview_interrupted(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::Interrupted, cx);
}
#[gpui::test]
fn preview_crashed(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::Crashed, cx);
}
#[gpui::test]
fn preview_streaming(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::Streaming, cx);
}
#[gpui::test]
fn preview_login_error(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::LoginError, cx);
}
#[gpui::test]
fn preview_model_access_error(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::ModelAccessError, cx);
}
#[gpui::test]
fn preview_rate_limit(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::RateLimit, cx);
}
#[gpui::test]
fn preview_disconnected(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::Disconnected, cx);
}
#[gpui::test]
fn preview_login_dialog_error(cx: &mut gpui::TestAppContext) {
    assert_preview(PreviewState::LoginDialogError, cx);
}

#[gpui::test]
fn preview_ids_are_unique(cx: &mut gpui::TestAppContext) {
    let a = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    let b = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    assert_ne!(
        a.read_with(cx, |p, _| p.session_id.clone()),
        b.read_with(cx, |p, _| p.session_id.clone())
    );
}

#[gpui::test]
fn preview_keyboard_and_model_recovery_stay_offline(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::ModelAccessError, cx));
    vcx.run_until_parked();
    let (bridge, commands) = crate::harness::spawn_recording();
    panel.update(vcx, |panel, _| panel.bridge = bridge);
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        let focus = panel.read(cx).input.read(cx).focus_handle.clone();
        focus.focus(window, cx);
    });
    for input in [
        "hello",
        "/terminal",
        "/gmail",
        "/todoist",
        "/file /etc/passwd",
    ] {
        vcx.simulate_input(input);
        vcx.simulate_keystrokes("enter");
    }
    let button = vcx.debug_bounds("recovery-choose-model").unwrap();
    vcx.simulate_click(button.center(), gpui::Modifiers::default());
    let button = vcx.debug_bounds("recovery-model-picker-0").unwrap();
    vcx.simulate_click(button.center(), gpui::Modifiers::default());
    panel.read_with(vcx, |panel, _| {
        assert_eq!(panel.model.as_deref(), Some("preview-model"));
        assert!(panel.items.iter().any(|item| matches!(item, Item::Assistant(text) if text.contains("Offline preview: selected"))));
        assert!(panel.terminal.is_none());
    });
    assert!(commands.try_recv().is_err());
}
