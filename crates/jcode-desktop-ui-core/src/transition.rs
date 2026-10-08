//! UI-facing motion policy adapter.
//!
//! The frame-hot, UI-independent interpolation code lives in the separately
//! optimized `jcode-desktop-motion` crate. This adapter keeps user configuration
//! in the reloadable UI crate without pulling GPUI into the motion core.

#[cfg(test)]
use jcode_desktop_motion::POLICIES;
pub use jcode_desktop_motion::{
    AnimatedValue, Policy, STANDARD_DURATION, Transition, arrival_motion, ease_out_cubic,
};

pub fn policy(transition: Transition) -> Policy {
    jcode_desktop_motion::policy(transition, crate::config::get().appearance.reduce_motion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn every_visible_transition_has_exactly_one_policy() {
        let all = [
            Transition::Focus,
            Transition::Row,
            Transition::PanelOrder,
            Transition::PanelOpen,
            Transition::PanelClose,
            Transition::PanelWidth,
            Transition::Overview,
            Transition::Hints,
            Transition::Coach,
            Transition::Update,
            Transition::SessionStatus,
            Transition::Connection,
            Transition::Transcript,
            Transition::PromptDelivery,
            Transition::ToolArrival,
        ];
        for transition in all {
            assert_eq!(
                POLICIES
                    .iter()
                    .filter(|p| p.transition == transition)
                    .count(),
                1,
                "{transition:?}"
            );
        }
    }

    #[test]
    fn retargeting_preserves_the_in_flight_value() {
        let start = Instant::now();
        let mut value = AnimatedValue::new(0.0, Duration::from_millis(100));
        value.set(1.0, start);
        let midway = value.sample(start + Duration::from_millis(50));
        value.set(0.0, start + Duration::from_millis(50));
        assert!((value.sample(start + Duration::from_millis(50)) - midway).abs() < 0.001);
        assert_eq!(value.sample(start + Duration::from_millis(150)), 0.0);
    }

    #[test]
    fn arrival_motion_settles_at_full_opacity() {
        let start = Instant::now();
        let duration = policy(Transition::ToolArrival).duration;
        assert_eq!(arrival_motion(start, start, duration), (6.0, 0.55, true));
        assert_eq!(
            arrival_motion(start, start + duration, duration),
            (0.0, 1.0, false)
        );
    }
}
