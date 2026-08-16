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
    Connection,
    Transcript,
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

pub const POLICIES: [Policy; 11] = [
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
];

pub fn policy(transition: Transition) -> &'static Policy {
    POLICIES
        .iter()
        .find(|policy| policy.transition == transition)
        .expect("transition missing from animation registry")
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
            self.value = self.from + (self.target - self.from) * ease_out_expo(t);
        }
        self.value
    }

    pub fn is_animating(&self) -> bool {
        self.started.is_some()
    }
}

pub fn ease_out_expo(t: f32) -> f32 {
    if t >= 1.0 {
        1.0
    } else if t <= 0.0 {
        0.0
    } else {
        1.0 - 2.0_f32.powf(-10.0 * t)
    }
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
            Transition::Connection,
            Transition::Transcript,
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
}
