//! Shared motion primitives and the transition coverage registry.
//!
//! Every user-visible state change belongs to a `Transition` below. Keeping the
//! registry exhaustive makes animation coverage reviewable and testable instead
//! of relying on scattered durations in render code.

use std::time::{Duration, Instant};

pub const STANDARD_DURATION: Duration = Duration::from_millis(150);
pub const MODAL_DURATION: Duration = Duration::from_millis(180);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transition {
    Focus,
    Row,
    PanelOrder,
    PanelOpen,
    PanelClose,
    PanelWidth,
    Overview,
    Hints,
    /// The learning coach's just-in-time hint toast.
    Coach,
    /// The automatic-update chip's activity pulse.
    Update,
    /// The panel footer's live session-status pulse.
    SessionStatus,
    Connection,
    Transcript,
    PromptDelivery,
    ToolArrival,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Motion {
    Animate,
    Continuous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Policy {
    pub transition: Transition,
    pub motion: Motion,
    pub duration: Duration,
}

pub const POLICIES: [Policy; 15] = [
    Policy {
        transition: Transition::Focus,
        motion: Motion::Animate,
        duration: STANDARD_DURATION,
    },
    Policy {
        transition: Transition::Row,
        motion: Motion::Animate,
        duration: STANDARD_DURATION,
    },
    Policy {
        transition: Transition::PanelOrder,
        motion: Motion::Animate,
        duration: STANDARD_DURATION,
    },
    Policy {
        transition: Transition::PanelOpen,
        motion: Motion::Animate,
        duration: STANDARD_DURATION,
    },
    Policy {
        transition: Transition::PanelClose,
        motion: Motion::Animate,
        duration: STANDARD_DURATION,
    },
    Policy {
        transition: Transition::PanelWidth,
        motion: Motion::Animate,
        duration: STANDARD_DURATION,
    },
    Policy {
        transition: Transition::Overview,
        motion: Motion::Animate,
        duration: MODAL_DURATION,
    },
    Policy {
        transition: Transition::Hints,
        motion: Motion::Animate,
        duration: MODAL_DURATION,
    },
    Policy {
        transition: Transition::Coach,
        motion: Motion::Animate,
        duration: MODAL_DURATION,
    },
    // The update chip pulses for as long as the updater is working, so its
    // policy is a repeating breath rather than a one-shot transition.
    Policy {
        transition: Transition::Update,
        motion: Motion::Animate,
        duration: Duration::from_millis(1400),
    },
    Policy {
        transition: Transition::SessionStatus,
        motion: Motion::Animate,
        duration: Duration::from_millis(1200),
    },
    // These states update incrementally from the runtime. Interpolating them
    // would add latency, so their deliberate policy is continuous rendering.
    Policy {
        transition: Transition::Connection,
        motion: Motion::Continuous,
        duration: Duration::ZERO,
    },
    Policy {
        transition: Transition::Transcript,
        motion: Motion::Continuous,
        duration: Duration::ZERO,
    },
    Policy {
        transition: Transition::PromptDelivery,
        motion: Motion::Animate,
        duration: Duration::from_millis(420),
    },
    Policy {
        transition: Transition::ToolArrival,
        motion: Motion::Animate,
        duration: MODAL_DURATION,
    },
];

pub fn policy(transition: Transition, reduce_motion: bool) -> Policy {
    let mut policy = *POLICIES
        .iter()
        .find(|policy| policy.transition == transition)
        .expect("transition missing from animation registry");
    if reduce_motion && policy.motion == Motion::Animate {
        policy.duration = Duration::ZERO;
    }
    policy
}

/// A retargetable scalar. Retargeting samples the in-flight value first, so
/// quickly reversing an overlay or resize never jumps.
#[derive(Clone, Copy, Debug)]
pub struct AnimatedValue {
    from: f32,
    value: f32,
    target: f32,
    started: Option<Instant>,
    duration: Duration,
}

impl AnimatedValue {
    pub fn new(value: f32, duration: Duration) -> Self {
        Self {
            from: value,
            value,
            target: value,
            started: None,
            duration,
        }
    }

    pub fn set(&mut self, target: f32, now: Instant) {
        self.sample(now);
        if (target - self.value).abs() < f32::EPSILON {
            self.target = target;
            self.started = None;
            return;
        }
        self.from = self.value;
        self.target = target;
        self.started = Some(now);
    }

    pub fn sample(&mut self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return self.value;
        };
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= self.duration || self.duration.is_zero() {
            self.value = self.target;
            self.started = None;
        } else {
            let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
            self.value = self.from + (self.target - self.from) * ease_out_cubic(t);
        }
        self.value
    }

    pub fn is_animating(&self) -> bool {
        self.started.is_some()
    }
}

/// A smooth ease-out curve for spatial movement.
///
/// Exponential easing completed more than half of the movement in the first
/// 16 ms at 60 Hz, leaving only a faint tail for the rest of the transition.
/// Cubic easing distributes that same 150 ms transition across more visible
/// frames while preserving a responsive start and a gentle stop.
pub fn ease_out_cubic(t: f32) -> f32 {
    if t >= 1.0 {
        1.0
    } else if t <= 0.0 {
        0.0
    } else {
        1.0 - (1.0 - t).powi(3)
    }
}

/// A restrained entrance for transcript cards that arrive from the runtime.
/// The row participates in layout immediately, then settles horizontally so
/// the transcript never changes height merely to produce motion.
pub fn arrival_motion(started_at: Instant, now: Instant, duration: Duration) -> (f32, f32, bool) {
    let elapsed = now.saturating_duration_since(started_at);
    if elapsed >= duration || duration.is_zero() {
        return (0.0, 1.0, false);
    }
    let progress = elapsed.as_secs_f32() / duration.as_secs_f32();
    let eased = ease_out_cubic(progress);
    (6.0 * (1.0 - eased), 0.55 + 0.45 * eased, true)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let duration = policy(Transition::ToolArrival, false).duration;
        assert_eq!(arrival_motion(start, start, duration), (6.0, 0.55, true));
        assert_eq!(
            arrival_motion(start, start + duration, duration),
            (0.0, 1.0, false)
        );
    }

    #[test]
    fn reduced_motion_preserves_policy_but_removes_animation_time() {
        let normal = policy(Transition::Focus, false);
        let reduced = policy(Transition::Focus, true);
        assert_eq!(normal.transition, reduced.transition);
        assert_eq!(normal.motion, reduced.motion);
        assert_eq!(reduced.duration, Duration::ZERO);
        assert_eq!(
            policy(Transition::Transcript, true).duration,
            Duration::ZERO
        );
    }
}
