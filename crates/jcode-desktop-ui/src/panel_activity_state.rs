//! Turn activity outlives individual text/reasoning buffers. A block ending or
//! a tool completing is not a turn completion. Keep the latest semantic state
//! until a real terminal event settles it in `Panel::finish_response`.
use super::*;

impl Panel {
    pub(super) fn observe_activity_state(&mut self, event: &ApiEvent) {
        // The harness explicitly clears this phase when an automatic retry is
        // cancelled. A countdown made the panel busy even though no request was
        // in flight, so clearing only its label would leave an endless spinner.
        // Once real output arrives it clears the retry phase below, which also
        // prevents a late transport clear from ending a live response.
        if matches!(event, ApiEvent::ConnectionPhase { phase, .. } if phase.is_empty())
            && is_retry_phase(&self.connection_phase)
        {
            // Cancelled or exhausted: the failure is final after all.
            self.set_auto_retry(stop_reason::AutoRetry::None);
            self.finish_response();
        }
        if matches!(event, ApiEvent::ConnectionPhase { phase, .. } if is_retry_phase(phase)) {
            // "Response stopped" beside a retry countdown is a contradiction.
            self.set_auto_retry(stop_reason::AutoRetry::Pending);
        }
        if matches!(event, ApiEvent::Error { .. }) {
            // A rejected send never receives MessageAccepted. Keep its text in
            // the transcript, but do not leave it pending forever or block the
            // next explicitly resumed follow-up behind a stale acknowledgement.
            self.pending_users.clear();
        }
        let status = match event {
            ApiEvent::MessageAccepted { .. }
                if !self.activity_active() || is_retry_phase(&self.connection_phase) =>
            {
                Some("running")
            }
            ApiEvent::ConnectionPhase { phase, .. } if !self.activity_active() => {
                if phase == "streaming" {
                    Some("streaming")
                } else if is_request_phase(phase) {
                    Some("running")
                } else {
                    None
                }
            }
            ApiEvent::TextDelta { .. } => Some("streaming"),
            ApiEvent::ReasoningDelta { .. } => Some("thinking"),
            ApiEvent::ToolStart { .. } | ApiEvent::ToolInputDelta { .. } => Some("running_tools"),
            ApiEvent::ToolDone { call_id, .. } if self.activity_active() => {
                let other_tools_running = self.items.iter().any(|item| {
                    matches!(item, Item::Tool { call_id: id, done: false, .. } if id != call_id)
                });
                Some(if other_tools_running {
                    "running_tools"
                } else {
                    "running"
                })
            }
            _ => None,
        };
        if let Some(status) = status {
            if !matches!(event, ApiEvent::ConnectionPhase { .. }) {
                self.set_auto_retry(stop_reason::AutoRetry::Recovered);
            }
            self.status = status.into();
            // Actual output supersedes a stale waiting/auth/retry transport
            // phase, including providers that omit a streaming phase event.
            self.connection_phase.clear();
        }
    }
}

pub(super) fn is_retry_phase(phase: &str) -> bool {
    phase.trim().to_ascii_lowercase().starts_with("retrying")
}

pub(super) fn is_request_phase(phase: &str) -> bool {
    let phase = phase.trim().replace('_', " ").to_ascii_lowercase();
    matches!(
        phase.as_str(),
        "authenticating" | "connecting" | "sending request" | "waiting for response"
    ) || phase.starts_with("retrying")
}

pub(super) fn phase_label(phase: &str) -> String {
    let phase = phase.replace('_', " ");
    match phase.as_str() {
        "authenticating" => "Authenticating".into(),
        "connecting" => "Connecting".into(),
        "sending request" => "Sending request".into(),
        "waiting for response" => "Waiting for model".into(),
        _ if phase.starts_with("retrying") => phase.replacen("retrying", "Retrying", 1),
        _ => phase,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(mut value: serde_json::Value) -> ApiEvent {
        value["session_id"] = json!("activity-lifecycle");
        serde_json::from_value(value).unwrap()
    }

    #[gpui::test]
    fn clearing_retry_settles_countdown_but_not_an_accepted_or_streaming_turn(
        cx: &mut gpui::TestAppContext,
    ) {
        for live in [
            None,
            Some(json!({"ev":"message_accepted"})),
            Some(json!({"ev":"text_delta", "text":"Recovered"})),
        ] {
            let panel = cx.new(|cx| {
                Panel::new(
                    "activity-lifecycle".into(),
                    None,
                    None,
                    crate::harness::spawn_inert(),
                    cx,
                )
            });
            panel.update(cx, |panel, cx| {
                panel.apply(
                    &event(json!({"ev":"connection_phase", "phase":"retrying in 5s (1/3)"})),
                    cx,
                );
                assert!(panel.activity_active());
                assert_eq!(panel.status_line(), "Retrying in 5s (1/3)");
                if let Some(live) = &live {
                    panel.apply(&event(live.clone()), cx);
                }
                panel.apply(&event(json!({"ev":"connection_phase", "phase":""})), cx);
                assert_eq!(panel.activity_active(), live.is_some());
                if live.is_none() {
                    assert_eq!(panel.status_line(), "Ready");
                    assert!(!panel.is_busy());
                }
            });
        }
    }

    #[gpui::test]
    fn failure_being_retried_is_not_reported_as_a_stopped_response(cx: &mut gpui::TestAppContext) {
        let failed = json!({"ev":"turn_stopped", "reason":"failure",
            "message":"Anthropic API error (503 Service Unavailable): credential validation failed"});
        for (outcome, title, error) in [
            (None, "Request failed. Retrying automatically", false),
            (
                Some(json!({"ev":"text_delta", "text":"Recovered"})),
                "Request failed. Retried automatically",
                false,
            ),
            (
                Some(json!({"ev":"connection_phase", "phase":""})),
                "Response stopped: error",
                true,
            ),
        ] {
            let panel = cx.new(|cx| {
                Panel::new(
                    "activity-lifecycle".into(),
                    None,
                    None,
                    crate::harness::spawn_inert(),
                    cx,
                )
            });
            panel.update(cx, |panel, cx| {
                panel.apply(&event(json!({"ev":"text_delta", "text":"Partial"})), cx);
                panel.apply(&event(failed.clone()), cx);
                let notice = |panel: &Panel| {
                    panel
                        .items
                        .iter()
                        .find_map(|item| match item {
                            Item::Stopped(notice) => Some(notice.clone()),
                            _ => None,
                        })
                        .unwrap()
                };
                assert_eq!(notice(panel).title(), "Response stopped: error");
                assert!(notice(panel).shows_error());
                for phase in ["retrying in 8s (3/3)", "retrying in 7s (3/3)"] {
                    panel.apply(&event(json!({"ev":"connection_phase", "phase":phase})), cx);
                }
                if let Some(outcome) = &outcome {
                    panel.apply(&event(outcome.clone()), cx);
                }
                assert_eq!(notice(panel).title(), title);
                assert_eq!(notice(panel).shows_error(), error);
            });
        }
    }

    #[gpui::test]
    fn rejected_or_cancelled_sends_do_not_leave_permanent_pending_markers(
        cx: &mut gpui::TestAppContext,
    ) {
        for terminal in [
            json!({"ev":"error", "code":"internal", "message":"request rejected"}),
            json!({"ev":"session_status", "status":"cancelled"}),
            json!({"ev":"session_status", "status":"failed"}),
            json!({"ev":"turn_stopped", "reason":"interrupted", "message":"Cancelled"}),
        ] {
            let panel = cx.new(|cx| {
                Panel::new(
                    "activity-lifecycle".into(),
                    None,
                    None,
                    crate::harness::spawn_inert(),
                    cx,
                )
            });
            panel.update(cx, |panel, cx| {
                panel.submit_or_queue("Keep my prompt".into(), vec![], false, cx);
                assert_eq!(panel.pending_users.len(), 1);
                panel.apply(&event(terminal), cx);
                assert!(panel.pending_users.is_empty());
                assert!(panel.accepted_users.is_empty(), "rejection is not acceptance");
                assert!(matches!(panel.items.first(), Some(Item::User(text)) if text == "Keep my prompt"));
                assert!(!panel.activity_active());
            });
        }
    }

    #[gpui::test]
    fn ambiguous_disconnect_keeps_pending_send_until_confirmed_stop(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            Panel::new(
                "activity-lifecycle".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(cx, |panel, cx| {
            panel.submit_or_queue("Possibly delivered".into(), vec![], false, cx);
            panel.connection_lost("connection reset", cx);
            assert_eq!(panel.pending_users.len(), 1);
            assert!(panel.accepted_users.is_empty());
            panel.apply(
                &event(json!({"ev":"session_status", "status":"cancelled"})),
                cx,
            );
            assert!(panel.pending_users.is_empty());
        });
    }

    #[gpui::test]
    fn activity_paints_continuously_across_blocks_tools_and_provider_requests(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("activity-lifecycle", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        // Deliberately no coarse `running` status and no streaming transport
        // notification. Neither is guaranteed for every provider/attachment.
        for (value, label) in [
            (json!({"ev":"message_accepted"}), "Working"),
            (
                json!({"ev":"connection_phase", "phase":"authenticating"}),
                "Authenticating",
            ),
            (
                json!({"ev":"connection_phase", "phase":"connecting"}),
                "Connecting",
            ),
            (
                json!({"ev":"connection_phase", "phase":"sending request"}),
                "Sending request",
            ),
            (
                json!({"ev":"connection_phase", "phase":"waiting for response"}),
                "Waiting for model",
            ),
            (
                json!({"ev":"reasoning_delta", "text":"Checking"}),
                "Thinking",
            ),
            (json!({"ev":"reasoning_done"}), "Thinking"),
            (
                json!({"ev":"session_status", "status":"attached"}),
                "Thinking",
            ),
            (
                json!({"ev":"session_status", "status":"connected"}),
                "Thinking",
            ),
            (
                json!({"ev":"text_delta", "text":"I'll inspect that."}),
                "Responding",
            ),
            (json!({"ev":"text_done"}), "Responding"),
            (
                json!({"ev":"tool_start", "call_id":"one", "name":"read"}),
                "Running tools",
            ),
            (
                json!({"ev":"tool_start", "call_id":"two", "name":"read"}),
                "Running tools",
            ),
            (
                json!({"ev":"tool_done", "call_id":"one", "name":"read", "output":"ok"}),
                "Running tools",
            ),
            (
                json!({"ev":"tool_done", "call_id":"two", "name":"read", "output":"ok"}),
                "Working",
            ),
            (
                json!({"ev":"connection_phase", "phase":"waiting for response"}),
                "Waiting for model",
            ),
            (
                json!({"ev":"connection_phase", "phase":"retrying (1/3)"}),
                "Retrying (1/3)",
            ),
            (
                json!({"ev":"text_delta", "text":"Full answer."}),
                "Responding",
            ),
            (json!({"ev":"text_done"}), "Responding"),
        ] {
            let event = event(value);
            panel.update(vcx, |panel, cx| {
                panel.apply(&event, cx);
                assert!(panel.activity_active(), "{event:?}");
                assert!(panel.is_busy(), "{event:?}");
                assert_eq!(panel.status_line(), label, "{event:?}");
            });
            vcx.run_until_parked();
            assert!(
                vcx.debug_bounds("transcript-activity").is_some(),
                "{event:?}"
            );
            assert!(
                vcx.debug_bounds("panel-activity-label").is_some(),
                "{event:?}"
            );
        }
        panel.update(vcx, |panel, cx| {
            panel.apply(&event(json!({"ev":"turn_done"})), cx);
            assert!(!panel.activity_active());
            assert!(!panel.is_busy());
            assert_eq!(panel.status_line(), "Ready");
            assert!(panel.streaming_text.is_empty());
            assert!(
                panel
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::Assistant(text) if text == "Full answer."))
            );
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("transcript-activity").is_none());
    }

    #[gpui::test]
    fn request_activity_survives_transport_bookkeeping_and_reload_then_stops(
        cx: &mut gpui::TestAppContext,
    ) {
        let panel = cx.new(|cx| {
            Panel::new(
                "activity-lifecycle".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        for terminal in [
            json!({"ev":"turn_done"}),
            json!({"ev":"session_status", "status":"idle"}),
            json!({"ev":"session_status", "status":"cancelled"}),
            json!({"ev":"error", "code":"internal", "message":"request failed"}),
        ] {
            panel.update(cx, |panel, cx| {
                panel.apply(
                    &event(json!({"ev":"connection_phase", "phase":"waiting for response"})),
                    cx,
                );
                assert!(panel.activity_active());
                assert_eq!(panel.status_line(), "Waiting for model");
                panel.apply(
                    &event(json!({"ev":"connection_phase", "phase":"connected"})),
                    cx,
                );
                assert!(panel.activity_active());
                panel.apply(
                    &event(json!({"ev":"reasoning_delta", "text":"Checking"})),
                    cx,
                );
                panel.apply(&event(json!({"ev":"reasoning_done"})), cx);
                let snapshot = panel.transcript_snapshot();
                panel.finish_response();
                panel.restore_transcript(snapshot);
                assert!(panel.activity_active());
                assert_eq!(panel.status_line(), "Thinking");
                panel.apply(&event(terminal), cx);
                assert!(!panel.activity_active());
                assert_eq!(panel.status_line(), "Ready");
                // Late tool completions do not restart a settled turn.
                panel.apply(
                    &event(
                        json!({"ev":"tool_done", "call_id":"late", "name":"read", "output":"ok"}),
                    ),
                    cx,
                );
                assert!(!panel.activity_active());
            });
        }
    }
}
