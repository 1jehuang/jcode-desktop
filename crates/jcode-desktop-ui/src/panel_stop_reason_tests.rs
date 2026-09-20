use super::*;

fn active_panel(cx: &mut Context<Panel>) -> Panel {
    let mut panel = Panel::new(
        "stop-test".into(),
        None,
        None,
        crate::harness::spawn_inert(),
        cx,
    );
    panel.history_loaded = true;
    panel.status = "running".into();
    panel.connection_phase = "streaming".into();
    panel.items = vec![
        Item::User("Continue the task".into()),
        Item::Assistant("Already received output".into()),
        Item::Tool {
            call_id: "pending-tool".into(),
            name: "bash".into(),
            input: "{}".into(),
            output: "Partial tool output".into(),
            done: false,
            error: None,
        },
    ];
    panel.streaming_text = "Unfinished answer".into();
    panel.streaming_reasoning = "Unfinished reasoning".into();
    panel
}

fn stopped(reason: TurnStopReason, message: &str) -> ApiEvent {
    ApiEvent::TurnStopped {
        session_id: "stop-test".into(),
        reason,
        message: message.into(),
        provider_stop_reason: Some("provider_finish_reason".into()),
    }
}

fn notices(panel: &Panel) -> Vec<&StopNotice> {
    panel
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Stopped(notice) => Some(notice),
            _ => None,
        })
        .collect()
}

fn assert_settled_partial_output(panel: &Panel) {
    for expected in ["Already received output", "Unfinished answer"] {
        assert!(
            panel
                .items
                .iter()
                .any(|item| { matches!(item, Item::Assistant(text) if text == expected) }),
            "missing preserved assistant text: {expected}"
        );
    }
    assert!(
        panel.items.iter().any(|item| {
            matches!(item, Item::Reasoning(text) if text == "Unfinished reasoning")
        })
    );
    assert!(panel.streaming_text.is_empty());
    assert!(panel.streaming_reasoning.is_empty());
    assert!(panel.connection_phase.is_empty());
    assert!(!panel.activity_active());
    assert!(panel.sidebar_activity().is_none());
}

#[gpui::test]
fn all_stop_categories_preserve_partial_output_settle_tools_and_pause_queue(
    cx: &mut gpui::TestAppContext,
) {
    for (reason, title) in [
        (TurnStopReason::Interrupted, "Response interrupted"),
        (TurnStopReason::Failure, "Response stopped: error"),
        (TurnStopReason::Crash, "Response stopped: session crashed"),
        (
            TurnStopReason::ProviderGuardrail,
            "Response stopped: provider guardrail",
        ),
        (
            TurnStopReason::LimitReached,
            "Response stopped: limit reached",
        ),
        (TurnStopReason::Unknown, "Response stopped"),
    ] {
        let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
        panel.update(vcx, |panel, cx| {
            panel.submit_or_queue("Do not send automatically".into(), vec![], true, cx);
            let waiting = serde_json::to_value(&panel.prompt_queue).unwrap()["prompts"].clone();
            assert_eq!(waiting.as_array().unwrap().len(), 1);
            panel.apply(&stopped(reason, "Runtime supplied stop detail"), cx);
            assert_settled_partial_output(panel);
            assert!(panel.prompt_queue.paused);
            assert!(panel.items.iter().any(|item| matches!(item,
                Item::Tool { output, done: true, error: Some(_), .. }
                if output == "Partial tool output"
            )));
            let notice = notices(panel);
            assert_eq!(notice.len(), 1);
            assert_eq!(notice[0].title, title);
            assert_eq!(notice[0].detail, "Runtime supplied stop detail");
            assert_eq!(
                notice[0].provider_stop_reason.as_deref(),
                Some("provider_finish_reason")
            );
            assert!(!notice[0].provisional);
            // Compatibility completion/status events must not drain a stopped queue.
            panel.apply(
                &ApiEvent::TurnDone {
                    session_id: "stop-test".into(),
                },
                cx,
            );
            panel.apply(
                &ApiEvent::SessionStatus {
                    session_id: "stop-test".into(),
                    status: "idle".into(),
                },
                cx,
            );
            assert!(panel.prompt_queue.paused);
            assert_eq!(
                serde_json::to_value(&panel.prompt_queue).unwrap()["prompts"],
                waiting
            );
            assert_eq!(notices(panel).len(), 1);
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("response-stop-notice").is_some(),
            "{reason:?}"
        );
        assert!(vcx.debug_bounds("transcript-activity").is_none());
        assert!(vcx.debug_bounds("panel-activity-spinner").is_none());
    }
}

#[gpui::test]
fn natural_done_preserves_output_without_a_stop_notice(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        // A normal turn has already received its tool result.
        if let Item::Tool { done, .. } = &mut panel.items[2] {
            *done = true;
        }
        panel.apply(
            &ApiEvent::TurnDone {
                session_id: "stop-test".into(),
            },
            cx,
        );
        assert_settled_partial_output(panel);
        assert!(notices(panel).is_empty());
        assert!(!panel.prompt_queue.paused);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("response-stop-notice").is_none());
    assert!(vcx.debug_bounds("transcript-activity").is_none());
    assert!(vcx.debug_bounds("panel-activity-spinner").is_none());
}

#[gpui::test]
fn legacy_error_after_structured_stop_does_not_duplicate_the_message(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        panel.apply(
            &stopped(TurnStopReason::Failure, "The provider request failed"),
            cx,
        );
        let count = panel.items.len();
        panel.apply(
            &ApiEvent::Error {
                code: jcode_sdk::api::ErrorCode::Internal,
                message: "The provider request failed".into(),
            },
            cx,
        );
        assert_eq!(panel.items.len(), count);
        assert_eq!(notices(panel).len(), 1);
        assert!(
            !panel
                .items
                .iter()
                .any(|item| matches!(item, Item::Error(_)))
        );
        assert_settled_partial_output(panel);
        assert!(panel.prompt_queue.paused);
    });
}

#[gpui::test]
fn user_cancel_notice_is_replaced_by_confirmed_interruption(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        window.focus(&panel.read(cx).input.read(cx).focus_handle.clone(), cx);
    });
    vcx.simulate_keystrokes("escape");
    panel.update(vcx, |panel, cx| {
        let notice = notices(panel);
        assert_eq!(notice.len(), 1);
        assert_eq!(notice[0].title, "Stop requested by you");
        assert!(notice[0].provisional);
        assert_settled_partial_output(panel);
        panel.apply(
            &stopped(
                TurnStopReason::Interrupted,
                "Cancellation confirmed by runtime",
            ),
            cx,
        );
        let notice = notices(panel);
        assert_eq!(notice.len(), 1);
        assert_eq!(notice[0].title, "Response interrupted");
        assert_eq!(notice[0].detail, "Cancellation confirmed by runtime");
        assert!(!notice[0].provisional);
        assert!(panel.prompt_queue.paused);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("response-stop-notice").is_some());
}

#[gpui::test]
fn legacy_cancelled_and_crashed_statuses_record_explicit_stops(cx: &mut gpui::TestAppContext) {
    for (status, title) in [
        ("cancelled", "Response interrupted"),
        ("canceled", "Response interrupted"),
        ("crashed", "Response stopped: session crashed"),
    ] {
        let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
        panel.update(vcx, |panel, cx| {
            let event = ApiEvent::SessionStatus {
                session_id: "stop-test".into(),
                status: status.into(),
            };
            panel.apply(&event, cx);
            panel.apply(&event, cx);
            assert_settled_partial_output(panel);
            assert!(panel.prompt_queue.paused);
            let notice = notices(panel);
            assert_eq!(notice.len(), 1);
            assert_eq!(notice[0].title, title);
            assert!(!notice[0].provisional);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("response-stop-notice").is_some());
    }
}

#[gpui::test]
fn active_disconnect_reports_unknown_outcome_not_a_confirmed_crash(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        panel.connection_lost("Socket closed", cx);
        assert_settled_partial_output(panel);
        assert!(panel.prompt_queue.paused);
        let notice = notices(panel);
        assert_eq!(notice.len(), 1);
        assert_eq!(notice[0].title, "Connection lost: response outcome unknown");
        assert!(notice[0].detail.contains("Socket closed"));
        assert!(notice[0].detail.contains("not a confirmed crash"));
        assert!(!notice[0].failure);
        assert!(notice[0].provisional);
        assert_ne!(panel.status, "crashed");
        panel.apply(
            &stopped(TurnStopReason::Interrupted, "Confirmed after reconnect"),
            cx,
        );
        let notice = notices(panel);
        assert_eq!(notice.len(), 1);
        assert_eq!(notice[0].detail, "Confirmed after reconnect");
        assert!(!notice[0].provisional);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("response-stop-notice").is_some());
    assert!(vcx.debug_bounds("transcript-activity").is_none());
    assert!(vcx.debug_bounds("panel-activity-spinner").is_none());
}

#[gpui::test]
fn idle_disconnect_does_not_insert_a_transcript_notice(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        let mut panel = Panel::new(
            "stop-test".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        );
        panel.history_loaded = true;
        panel.status = "idle".into();
        panel.items = vec![Item::Assistant("Completed answer".into())];
        panel
    });
    panel.update(vcx, |panel, cx| {
        panel.connection_lost("Socket closed", cx);
        assert_eq!(panel.items.len(), 1);
        assert!(matches!(&panel.items[0], Item::Assistant(text) if text == "Completed answer"));
        assert!(notices(panel).is_empty());
        assert!(!panel.activity_active());
        assert_ne!(panel.status, "crashed");
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("response-stop-notice").is_none());
}

#[gpui::test]
fn stop_settles_only_current_turn_tools_and_preserves_detached_background_work(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        panel.items.insert(
            0,
            Item::Tool {
                call_id: "historical-tool".into(),
                name: "bash".into(),
                input: "{}".into(),
                output: "Historical output".into(),
                done: false,
                error: None,
            },
        );
        panel.items.push(Item::BackgroundTask {
            task_id: "detached-job".into(),
            label: "Build".into(),
            summary: "Still running independently".into(),
            percent: Some(25.0),
            done: false,
        });
        panel.apply(
            &stopped(TurnStopReason::Interrupted, "Cancelled foreground response"),
            cx,
        );
        assert!(matches!(
            &panel.items[0],
            Item::Tool {
                done: false,
                error: None,
                ..
            }
        ));
        assert!(panel.items.iter().any(|item| matches!(item,
            Item::Tool { call_id, done: true, error: Some(_), .. } if call_id == "pending-tool"
        )));
        assert!(panel.items.iter().any(|item| matches!(item,
            Item::BackgroundTask { task_id, done: false, percent: Some(percent), .. }
            if task_id == "detached-job" && *percent == 25.0
        )));
        assert_settled_partial_output(panel);
    });
}

#[gpui::test]
fn compatibility_status_and_later_disconnect_preserve_confirmed_stop_details(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        panel.apply(
            &stopped(
                TurnStopReason::ProviderGuardrail,
                "Specific provider explanation",
            ),
            cx,
        );
        for status in ["failed", "error", "crashed", "cancelled"] {
            panel.apply(
                &ApiEvent::SessionStatus {
                    session_id: "stop-test".into(),
                    status: status.into(),
                },
                cx,
            );
        }
        panel.connection_lost("Socket closed after terminal event", cx);
        let notice = notices(panel);
        assert_eq!(notice.len(), 1);
        assert_eq!(notice[0].title, "Response stopped: provider guardrail");
        assert_eq!(notice[0].detail, "Specific provider explanation");
        assert_eq!(
            notice[0].provider_stop_reason.as_deref(),
            Some("provider_finish_reason")
        );
        assert!(!notice[0].provisional);
    });
}
