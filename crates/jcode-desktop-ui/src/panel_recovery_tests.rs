use super::*;

#[test]
fn recovery_classification_avoids_login_for_io_errors() {
    for message in [
        "I/O error: permission denied",
        "Connection closed",
        "Failed to read credentials file",
        "Unknown command",
        "I/O error reading /tmp/401/429.txt",
    ] {
        assert_eq!(recovery_kind(message), RecoveryKind::Other, "{message}");
    }
    for message in [
        "401 Unauthorized",
        "Authentication failed",
        "invalid_api_key",
    ] {
        assert_eq!(recovery_kind(message), RecoveryKind::Auth);
    }
    assert_eq!(
        recovery_kind("429 rate limit exceeded"),
        RecoveryKind::Quota
    );
    assert_eq!(recovery_kind("model does not exist"), RecoveryKind::Model);
}

#[test]
fn recovery_terminal_guidance_is_native_but_diagnostics_are_preserved() {
    let auth = "OpenAI token refresh failed; run /login to re-authenticate: invalid_grant";
    assert_eq!(recovery_kind(auth), RecoveryKind::Auth);
    let displayed = native_error_message(auth);
    assert!(!displayed.contains("/login"));
    assert!(displayed.contains("Log in / add account button"));
    assert!(displayed.contains("invalid_grant"));
    let model = "Model is not currently usable. Choose another model from `/model`.";
    assert_eq!(recovery_kind(model), RecoveryKind::Model);
    assert!(!native_error_message(model).contains("/model"));
}

fn error(message: &str) -> ApiEvent {
    ApiEvent::Error {
        code: jcode_sdk::api::ErrorCode::Internal,
        message: message.into(),
    }
}

#[gpui::test]
fn recovery_actual_clicks_switch_models_copy_errors_and_open_login(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.push_test_panel("session-a", cx);
        workspace
    });
    let panel = workspace
        .read_with(vcx, |workspace, _| workspace.test_panel(0))
        .unwrap();
    panel.update(vcx, |panel, cx| {
        panel.input.update(cx, |input, cx| {
            input.set_content("keep this draft".into(), cx)
        });
        panel.apply(
            &ApiEvent::RuntimeInfo {
                session_id: "session-a".into(),
                provider: None,
                model: None,
                reasoning_effort: None,
                routes: vec![
                    jcode_sdk::ModelRouteInfo {
                        model: "openai:test".into(),
                        provider: "openai".into(),
                        api_method: "openai-api-key".into(),
                        available: true,
                        detail: String::new(),
                    },
                    jcode_sdk::ModelRouteInfo {
                        model: "unavailable".into(),
                        provider: "openai".into(),
                        api_method: "openai-api-key".into(),
                        available: false,
                        detail: String::new(),
                    },
                ],
            },
            cx,
        );
        panel.apply(&error("429 rate limit exceeded"), cx);
    });
    vcx.run_until_parked();
    let row = vcx
        .debug_bounds("recovery-model-0-0")
        .expect("error automatically exposes a model choice");
    assert!(vcx.debug_bounds("recovery-model-0-1").is_none());
    vcx.simulate_click(row.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::SetModel { session_id, model }) if session_id == "session-a" && model == "openai:test")
    );
    assert!(
        commands.try_recv().is_err(),
        "recovery never automatically resends a prompt"
    );
    panel.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "keep this draft")
    });

    panel.update(vcx, |panel, cx| {
        panel.items.clear();
        panel.apply(&error("I/O error: permission denied"), cx);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("recovery-login").is_none());
    assert!(vcx.debug_bounds("recovery-model-0-0").is_none());
    let copy = vcx
        .debug_bounds("recovery-copy")
        .expect("generic errors have a relevant action");
    vcx.simulate_click(copy.center(), gpui::Modifiers::default());
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("I/O error: permission denied")
        )
    });

    panel.update(vcx, |panel, cx| panel.open_recovery_models(cx));
    vcx.run_until_parked();
    let row = vcx
        .debug_bounds("recovery-model-picker-0")
        .expect("native picker route");
    vcx.simulate_click(row.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::SetModel { model, .. }) if model == "openai:test")
    );
    assert!(vcx.debug_bounds("recovery-model-picker").is_none());

    panel.update(vcx, |panel, cx| {
        panel.items.clear();
        panel.apply(&error("401 Unauthorized"), cx);
    });
    vcx.run_until_parked();
    let login = vcx
        .debug_bounds("recovery-login")
        .expect("auth error login button");
    vcx.simulate_click(login.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, cx| {
        assert!(panel.login.is_some());
        assert_eq!(panel.input.read(cx).content.as_ref(), "keep this draft");
    });
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn recovery_empty_model_picker_offers_native_connect_and_refresh(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("empty-models".into(), None, None, bridge, cx));
    panel.update(vcx, |panel, cx| {
        panel.input.update(cx, |input, cx| {
            input.set_content("untouched draft".into(), cx)
        });
        panel.open_model_picker(cx);
        assert!(
            panel.items.is_empty(),
            "empty models no longer append slash-command instructions"
        );
    });
    vcx.run_until_parked();
    let refresh = vcx
        .debug_bounds("recovery-refresh-models")
        .expect("refresh button");
    vcx.simulate_click(refresh.center(), gpui::Modifiers::default());
    assert!(
        matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "empty-models")
    );
    let connect = vcx
        .debug_bounds("recovery-connect-account")
        .expect("connect button");
    vcx.simulate_click(connect.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, cx| {
        assert!(panel.login.is_some());
        assert!(!panel.recovery_picker_open);
        assert_eq!(panel.input.read(cx).content.as_ref(), "untouched draft");
    });
    assert!(commands.try_recv().is_err());
}
