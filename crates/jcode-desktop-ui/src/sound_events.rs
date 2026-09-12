//! Per-panel sound policy. Snapshots and status alone never arm completion.

use std::collections::VecDeque;

use jcode_sdk::ApiEvent;

use crate::sounds::Cue;

const RECENT_IDS: usize = 128;

#[derive(Default)]
pub(crate) struct SoundEvents {
    live_turn: bool,
    cancelled: bool,
    error_reported: bool,
    permission_ids: VecDeque<(String, String)>,
    completed_tasks: VecDeque<(String, String)>,
}

impl SoundEvents {
    /// Silence completion when the panel cancels locally before the API reply.
    pub(crate) fn cancel(&mut self) {
        self.live_turn = false;
        self.cancelled = true;
    }

    pub(crate) fn observe(&mut self, event: &ApiEvent) -> Option<Cue> {
        match event {
            ApiEvent::MessageAccepted { .. } => {
                self.cancelled = false;
                self.live_turn = true;
                self.error_reported = false;
                None
            }
            ApiEvent::TextDelta { .. }
            | ApiEvent::ReasoningDelta { .. }
            | ApiEvent::ToolStart { .. }
            | ApiEvent::ToolInputDelta { .. }
            | ApiEvent::ToolExec { .. } => {
                if self.cancelled {
                    return None;
                }
                self.live_turn = true;
                self.error_reported = false;
                // Sent belongs to the local submit action, not its remote ack.
                None
            }
            ApiEvent::TurnDone { .. } => {
                self.cancelled = false;
                let complete = std::mem::take(&mut self.live_turn);
                complete.then_some(Cue::Complete)
            }
            ApiEvent::Error { .. } => {
                self.live_turn = false;
                let repeated = std::mem::replace(&mut self.error_reported, true);
                (!repeated).then_some(Cue::Error)
            }
            ApiEvent::SessionStatus { status, .. }
                if matches!(status.as_str(), "cancelled" | "canceled") =>
            {
                self.cancel();
                None
            }
            ApiEvent::Attached { .. } | ApiEvent::SessionForked { .. } => {
                // A newly attached session is not an observed live turn.
                self.live_turn = false;
                self.cancelled = false;
                self.error_reported = false;
                None
            }
            ApiEvent::PermissionRequest {
                session_id,
                request_id,
                ..
            } => {
                remember(&mut self.permission_ids, session_id, request_id).then_some(Cue::Attention)
            }
            ApiEvent::BackgroundProgress {
                session_id,
                task_id,
                done: true,
                ..
            } => remember(&mut self.completed_tasks, session_id, task_id)
                .then_some(Cue::BackgroundComplete),
            // Core currently emits attached/running/idle/cancelled. There is
            // no explicit waiting-for-user SessionStatus. PermissionRequest is
            // the attention signal. ToolDone errors are recoverable tool
            // results, not terminal turn failures, and do not make noise.
            _ => None,
        }
    }
}

/// Retain only a bounded recent window, keyed by session as well as request.
/// An ID replayed after eviction may sound again instead of growing forever.
fn remember(ids: &mut VecDeque<(String, String)>, session: &str, id: &str) -> bool {
    if ids.iter().any(|(s, i)| s == session && i == id) {
        return false;
    }
    if ids.len() == RECENT_IDS {
        ids.pop_front();
    }
    ids.push_back((session.to_owned(), id.to_owned()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn event(mut value: Value) -> ApiEvent {
        value["session_id"] = json!("session");
        serde_json::from_value(value).unwrap()
    }

    fn check(steps: Vec<(Value, Option<Cue>)>) {
        let mut sounds = SoundEvents::default();
        for (value, expected) in steps {
            let event = event(value);
            assert_eq!(sounds.observe(&event), expected, "event: {event:?}");
        }
    }

    #[test]
    fn live_activity_arms_exactly_one_completion_without_a_sent_cue() {
        let activity = [
            json!({"ev":"message_accepted"}),
            json!({"ev":"text_delta", "text":"Would you like me to continue?"}),
            json!({"ev":"reasoning_delta", "text":"thinking"}),
            json!({"ev":"tool_start", "call_id":"call", "name":"bash"}),
            json!({"ev":"tool_input_delta", "call_id":"call", "delta":"{}"}),
            json!({"ev":"tool_exec", "call_id":"call", "name":"bash"}),
        ];
        for active in activity {
            check(vec![
                (json!({"ev":"turn_done"}), None),
                (active, None),
                (json!({"ev":"turn_done"}), Some(Cue::Complete)),
                (json!({"ev":"turn_done"}), None),
            ]);
        }
    }

    #[test]
    fn snapshots_idle_and_unknown_events_are_silent() {
        for status in [
            "attached",
            "running",
            "idle",
            "generating",
            "waiting_for_user",
        ] {
            check(vec![
                (json!({"ev":"session_status", "status":status}), None),
                (json!({"ev":"turn_done"}), None),
            ]);
        }
        check(vec![
            (
                json!({"ev":"history", "messages":[{"role":"assistant", "content":"done"}]}),
                None,
            ),
            (json!({"ev":"unknown_future_event"}), None),
            (json!({"ev":"ok"}), None),
            (json!({"ev":"connection_phase", "phase":"streaming"}), None),
            (json!({"ev":"token_usage", "input":10, "output":20}), None),
            (json!({"ev":"turn_done"}), None),
        ]);
    }

    #[test]
    fn terminal_errors_suppress_completion_and_dedupe_until_new_activity() {
        let error = json!({"ev":"error", "code":"internal", "message":"failed"});
        check(vec![
            (json!({"ev":"message_accepted"}), None),
            (error.clone(), Some(Cue::Error)),
            (error.clone(), None),
            (json!({"ev":"turn_done"}), None),
            (error.clone(), None),
            (json!({"ev":"session_status", "status":"running"}), None),
            (error.clone(), None),
            (json!({"ev":"text_delta", "text":"retry"}), None),
            (error.clone(), Some(Cue::Error)),
            (json!({"ev":"turn_done"}), None),
            (json!({"ev":"message_accepted"}), None),
            (json!({"ev":"turn_done"}), Some(Cue::Complete)),
        ]);
    }

    #[test]
    fn cancellation_is_silent_and_does_not_complete() {
        check(vec![
            (json!({"ev":"text_delta", "text":"working"}), None),
            (json!({"ev":"session_status", "status":"cancelled"}), None),
            (json!({"ev":"session_status", "status":"idle"}), None),
            (json!({"ev":"turn_done"}), None),
            (json!({"ev":"turn_done"}), None),
            (json!({"ev":"message_accepted"}), None),
            (json!({"ev":"turn_done"}), Some(Cue::Complete)),
        ]);
    }

    #[test]
    fn permissions_and_background_completions_are_deduplicated_independently() {
        let permission = |id| json!({"ev":"permission_request", "request_id":id, "tool_name":"bash", "description":"allow?"});
        let background = |id, done| json!({"ev":"background_progress", "task_id":id, "label":"test", "summary":"status", "done":done});
        check(vec![
            (permission("1"), Some(Cue::Attention)),
            (permission("1"), None),
            (permission("2"), Some(Cue::Attention)),
            (background("1", false), None),
            (background("1", true), Some(Cue::BackgroundComplete)),
            (background("1", true), None),
            (background("1", false), None),
            (background("1", true), None),
            (background("2", true), Some(Cue::BackgroundComplete)),
            (json!({"ev":"turn_done"}), None),
        ]);
    }

    #[test]
    fn recoverable_tool_failure_does_not_replace_turn_completion() {
        check(vec![
            (
                json!({"ev":"tool_start", "call_id":"call", "name":"bash"}),
                None,
            ),
            (
                json!({"ev":"tool_done", "call_id":"call", "name":"bash", "output":"", "error":"exit 1"}),
                None,
            ),
            (json!({"ev":"turn_done"}), Some(Cue::Complete)),
        ]);
    }

    #[test]
    fn local_cancel_suppresses_late_streaming_and_attach_discards_live_state() {
        let mut sounds = SoundEvents::default();
        let active = event(json!({"ev":"text_delta", "text":"working"}));
        let done = event(json!({"ev":"turn_done"}));
        sounds.observe(&active);
        sounds.cancel();
        assert_eq!(sounds.observe(&active), None);
        assert_eq!(sounds.observe(&done), None);
        sounds.observe(&active);
        let attached = event(json!({"ev":"attached", "session": {
            "session_id":"session", "status":"processing", "archived":false
        }}));
        assert_eq!(sounds.observe(&attached), None);
        assert_eq!(sounds.observe(&done), None);
        sounds.observe(&active);
        assert_eq!(sounds.observe(&done), Some(Cue::Complete));
    }

    #[test]
    fn recent_ids_are_bounded_and_scoped_to_session() {
        let mut ids = VecDeque::new();
        assert!(remember(&mut ids, "one", "id"));
        assert!(!remember(&mut ids, "one", "id"));
        assert!(remember(&mut ids, "two", "id"));
        for n in 0..RECENT_IDS * 2 {
            assert!(remember(&mut ids, "one", &n.to_string()));
            assert!(ids.len() <= RECENT_IDS);
        }
        assert_eq!(ids.len(), RECENT_IDS);
        assert!(remember(&mut ids, "one", "id"));
    }
}
