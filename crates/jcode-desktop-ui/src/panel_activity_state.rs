//! Turn activity outlives individual text/reasoning buffers. A block ending or
//! a tool completing is not a turn completion. Keep the latest semantic state
//! until a real terminal event settles it in `Panel::finish_response`.
use super::*;

impl Panel {
    pub(super) fn observe_activity_state(&mut self, event: &ApiEvent) {
        let status = match event {
            ApiEvent::MessageAccepted { .. } if !self.activity_active() => Some("running"),
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
            self.status = status.into();
            // Actual output supersedes a stale waiting/auth/retry transport
            // phase, including providers that omit a streaming phase event.
            self.connection_phase.clear();
        }
    }
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
