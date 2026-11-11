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
    assert!(displayed.contains("Log in again"));
    assert!(displayed.len() < 120);
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

fn failed_turn(panel: &mut Panel, message: &str, cx: &mut Context<Panel>) {
    panel.apply(
        &ApiEvent::TurnStopped {
            session_id: panel.session_id.clone(),
            reason: jcode_sdk::TurnStopReason::Failure,
            message: message.into(),
            provider_stop_reason: None,
        },
        cx,
    );
    panel.apply(&error(message), cx);
    panel.apply(
        &ApiEvent::TurnDone {
            session_id: panel.session_id.clone(),
            pending_soft_interrupts: None,
        },
        cx,
    );
    assert!(matches!(panel.items.last(), Some(Item::Stopped(notice)) if notice.detail == message));
    assert!(
        !panel
            .items
            .iter()
            .any(|item| matches!(item, Item::Error(_))),
        "duplicate terminal error is suppressed"
    );
}

#[gpui::test]
fn recovery_resume_click_sends_one_fresh_continuation_and_preserves_draft(
    cx: &mut gpui::TestAppContext,
) {
    for (status, stopped) in [("idle", false), ("connected", false), ("idle", true)] {
        let (bridge, commands) = crate::harness::spawn_recording();
        let (panel, vcx) =
            cx.add_window_view(|_, cx| Panel::new("resume-test".into(), None, None, bridge, cx));
        panel.update(vcx, |panel, cx| {
            panel
                .items
                .push(Item::User("Original request must not be replayed".into()));
            panel.input.update(cx, |input, cx| {
                input.set_content("untouched draft".into(), cx)
            });
            if stopped {
                failed_turn(panel, "503 Service Unavailable: connection reset", cx);
            } else {
                panel.apply(&error("503 Service Unavailable: connection reset"), cx);
            }
            panel.status = status.into();
        });
        vcx.run_until_parked();
        assert!(commands.try_recv().is_err(), "no automatic replay");
        let resume = vcx
            .debug_bounds("recovery-resume")
            .expect("explicit Resume pill");
        vcx.simulate_click(resume.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(
            matches!(commands.try_recv(), Ok(Command::Send { content, images, .. })
            if content.starts_with("Continue the task from where you left off.")
                && !content.contains("Original request") && images.is_empty())
        );
        assert!(commands.try_recv().is_err());
        assert!(vcx.debug_bounds("recovery-resume").is_none());
        panel.update(vcx, |panel, cx| {
            // A stale painted click cannot submit twice while acceptance is pending.
            panel.resume_failed_turn(1, "503 Service Unavailable: connection reset", cx);
            assert_eq!(panel.pending_users.len(), 1);
            assert_eq!(panel.input.read(cx).content.as_ref(), "untouched draft");
        });
        assert!(commands.try_recv().is_err());
    }
}

#[gpui::test]
fn recovery_resume_never_bypasses_active_retry_or_queued_user_requests(
    cx: &mut gpui::TestAppContext,
) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("resume-test".into(), None, None, bridge, cx));
    for message in [
        "401 Unauthorized: connection reset",
        "400 bad request",
        "usage limit reached",
    ] {
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            panel.apply(&error(message), cx);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("recovery-resume").is_none(), "{message}");
    }
    for provisional in [false, true] {
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            let mut notice = stop_reason::StopNotice::from_event(
                if provisional {
                    &jcode_sdk::TurnStopReason::Failure
                } else {
                    &jcode_sdk::TurnStopReason::Interrupted
                },
                "connection reset",
                None,
            );
            notice.provisional = provisional;
            panel.record_stop(notice);
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("recovery-resume").is_none(),
            "cancellation/provisional loss is not permission to resume"
        );
    }
    panel.update(vcx, |panel, cx| {
        panel.items.clear();
        panel.apply(&error("connection reset"), cx);
        panel.apply(
            &ApiEvent::ConnectionPhase {
                session_id: "resume-test".into(),
                phase: "retrying in 2s (1/3)".into(),
            },
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("recovery-resume").is_none());
    panel.update(vcx, |panel, cx| {
        panel.submit_or_queue("queued user priority".into(), vec![], true, cx);
        panel.apply(&error("connection reset"), cx);
        panel.resume_failed_turn(0, "connection reset", cx);
        assert!(panel.has_queued_prompts());
        assert!(panel.prompt_queue.paused);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("recovery-resume").is_none());
    assert!(
        commands.try_recv().is_err(),
        "a synthetic continuation cannot jump the queue"
    );
}

#[gpui::test]
fn recovery_failover_preserves_cost_and_requires_model_choice_consent(
    cx: &mut gpui::TestAppContext,
) {
    let offer = jcode_base::provider::ProviderFailoverPrompt {
        from_provider: "openai".into(),
        from_label: "OpenAI".into(),
        to_provider: "minimax".into(),
        to_label: "MiniMax".into(),
        target_route: Some("minimax:new-model".into()),
        reason: "503 Service Unavailable".into(),
        estimated_input_chars: 48000,
        estimated_input_tokens: 12000,
    }
    .to_error_message();
    let native = native_error_message(&offer);
    assert!(native.contains("OpenAI") && native.contains("MiniMax"));
    assert!(native.contains("12000 input tokens") && native.contains("48000 characters"));
    assert!(native.contains("confirm") && !native.contains("[jcode-provider-failover]"));
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("failover-test".into(), None, None, bridge, cx));
    vcx.update(|_, cx| Panel::connect_input(&panel, cx));
    panel.update(vcx, |panel, cx| {
        panel.available_models = vec!["minimax:new-model".into()];
        panel.input.update(cx, |input, cx| {
            input.set_command_models(panel.available_models.clone(), cx);
            input.set_content("keep draft".into(), cx);
        });
        failed_turn(panel, &offer, cx);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("recovery-resume").is_none());
    assert!(
        commands.try_recv().is_err(),
        "failover never switches providers automatically"
    );
    let copy = vcx.debug_bounds("recovery-copy").unwrap();
    vcx.simulate_click(copy.center(), gpui::Modifiers::default());
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(offer.as_str())
        )
    });
    let choose = vcx.debug_bounds("recovery-choose-model").unwrap();
    vcx.simulate_click(choose.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        commands.try_recv().is_err(),
        "opening choices is not consent to switch"
    );
    let route = vcx.debug_bounds("slash-command-row-0").unwrap();
    vcx.simulate_click(route.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::SetModel { model, .. }) if model == "minimax:new-model")
    );
    assert!(
        commands.try_recv().is_err(),
        "choosing a route does not replay the request"
    );
    panel.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "keep draft")
    });
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
    vcx.update(|_, cx| Panel::connect_input(&panel, cx));
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
                auth_method: None,
                account_label: None,
                routes: vec![
                    jcode_sdk::ModelRouteInfo {
                        usage: None,
                        model: "openai:test".into(),
                        provider: "openai".into(),
                        api_method: "openai-api-key".into(),
                        available: true,
                        detail: String::new(),
                    },
                    jcode_sdk::ModelRouteInfo {
                        usage: None,
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
    assert!(vcx.debug_bounds("recovery-model-0-0").is_none());
    let choose = vcx
        .debug_bounds("recovery-choose-model")
        .expect("compact recovery action");
    vcx.simulate_click(choose.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    let row = vcx
        .debug_bounds("slash-command-row-0")
        .expect("native model choice");
    assert!(vcx.debug_bounds("recovery-model-picker-1").is_none());
    vcx.simulate_click(row.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::SetModel { session_id, model }) if session_id == "session-a" && model == "openai-api:test")
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
        .debug_bounds("slash-command-row-0")
        .expect("native picker route");
    vcx.simulate_click(row.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::SetModel { model, .. }) if model == "openai-api:test")
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

const REFRESH_TOKEN_ERROR: &str = "openai refresh token was previously rejected by the provider and cannot be refreshed. Run `jcode login --provider openai` to mint a fresh token.";

#[test]
fn recovery_refresh_token_error_and_arbitrary_cli_guidance_use_native_summaries() {
    for original in [
        REFRESH_TOKEN_ERROR,
        "Authentication failed. Please execute some-other-cli auth login --token foo.",
        "Model unavailable. Run `jcode --model another-model` in your terminal.",
    ] {
        let native = native_error_message(original);
        assert!(!native.contains("jcode"));
        assert!(!native.contains("some-other-cli"));
        assert!(!native.contains("--"));
        assert!(native.len() < 120);
    }
    assert_eq!(recovery_kind(REFRESH_TOKEN_ERROR), RecoveryKind::Auth);
    assert_eq!(
        native_error_message("I/O failure: read only"),
        "I/O failure: read only"
    );
}

#[gpui::test]
fn recovery_many_routes_stay_compact_until_choose_model_is_clicked(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("compact".into(), None, None, bridge, cx));
    vcx.update(|_, cx| Panel::connect_input(&panel, cx));
    panel.update(vcx, |panel, cx| {
        panel.available_models = (0..80).map(|n| format!("minimax:model-{n:02}")).collect();
        panel.input.update(cx, |input, cx| {
            input.set_command_models(panel.available_models.clone(), cx);
            input.set_content("preserve draft".into(), cx)
        });
        panel.apply(&error(REFRESH_TOKEN_ERROR), cx);
    });
    vcx.run_until_parked();
    let card = vcx.debug_bounds("recovery-error").expect("error card");
    assert!(
        card.size.height < px(130.),
        "error should not grow with route count: {card:?}"
    );
    assert!(vcx.debug_bounds("recovery-model-choices").is_none());
    assert!(vcx.debug_bounds("recovery-login").is_some());
    let copy = vcx.debug_bounds("recovery-copy").unwrap();
    vcx.simulate_click(copy.center(), gpui::Modifiers::default());
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(REFRESH_TOKEN_ERROR)
        )
    });
    let choose = vcx.debug_bounds("recovery-choose-model").unwrap();
    vcx.simulate_click(choose.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    let picker = vcx
        .debug_bounds("slash-command-overlay")
        .expect("shared model menu");
    assert!(picker.top() >= px(0.), "menu must stay inside the viewport");
    assert!(picker.size.height <= px(490.));
    assert!(vcx.debug_bounds("recovery-model-picker").is_none());
    let row = vcx.debug_bounds("slash-command-row-0").unwrap();
    assert!(row.size.height >= px(20.));
    vcx.simulate_click(row.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::SetModel { model, .. }) if model == "minimax:model-00")
    );
    assert!(commands.try_recv().is_err());
    assert!(vcx.debug_bounds("recovery-model-picker").is_none());
    panel.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "preserve draft")
    });
}
