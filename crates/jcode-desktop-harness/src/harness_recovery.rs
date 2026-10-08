//! A recovery directive belongs to one attachment, not every history refresh.
use super::{ApiEvent, is_already_processing_error, update_turn_activity};

#[derive(Default)]
pub(super) struct Recovery {
    consumed: bool,
    awaiting_acceptance: bool,
}

impl Recovery {
    pub(super) fn supersede(&mut self) {
        self.consumed = true;
    }

    pub(super) fn claim(&mut self, message: &str, busy: bool) -> bool {
        if self.consumed || busy || message.trim().is_empty() {
            return false;
        }
        self.consumed = true;
        self.awaiting_acceptance = true;
        true
    }

    pub(super) fn finish_submission(&mut self) {
        self.awaiting_acceptance = false;
    }

    /// Returns true only for a redundant recovery rejected by a busy runtime.
    pub(super) fn observe(&mut self, event: &ApiEvent) -> bool {
        let redundant = self.awaiting_acceptance
            && matches!(event, ApiEvent::Error { message, .. } if is_already_processing_error(message));
        if matches!(
            event,
            ApiEvent::MessageAccepted { .. } | ApiEvent::Error { .. } | ApiEvent::TurnDone { .. }
        ) {
            self.finish_submission();
        }
        let mut active = false;
        update_turn_activity(event, &mut active);
        if active
            || matches!(event, ApiEvent::TurnDone { .. })
            || matches!(event, ApiEvent::SessionStatus { status, .. } if matches!(status.as_str(), "cancelled" | "canceled"))
        {
            self.supersede();
        }
        redundant
    }
}
