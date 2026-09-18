//! Frame-delta-driven edit preview timing, independent of the UI toolkit.
use std::time::Duration;

pub const COUNTDOWN_DURATION: Duration = Duration::from_secs(5);
pub const COLLAPSE_DURATION: Duration = Duration::from_millis(240);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditPreviewState {
    Running,
    Countdown,
    PinnedOpen,
    Collapsing,
    Collapsed,
}

#[derive(Clone, Debug)]
pub struct EditPreviewTimer {
    state: EditPreviewState,
    done: bool,
    countdown_elapsed: Duration,
    collapse_elapsed: Duration,
}

impl Default for EditPreviewTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl EditPreviewTimer {
    pub fn new() -> Self {
        Self {
            state: EditPreviewState::Running,
            done: false,
            countdown_elapsed: Duration::ZERO,
            collapse_elapsed: Duration::ZERO,
        }
    }

    /// Advance by time since the previous update, not total time since creation.
    /// The caller measures this delta with `Instant`. The completion-edge delta
    /// is ignored because it may include time spent executing the tool.
    /// A return to running resets automatic timing but preserves a manual pin.
    pub fn update(&mut self, done: bool, elapsed: Duration) {
        let completion_edge = done && !self.done;
        self.done = done;
        if !done {
            self.countdown_elapsed = Duration::ZERO;
            self.collapse_elapsed = Duration::ZERO;
            if self.state != EditPreviewState::PinnedOpen {
                self.state = EditPreviewState::Running;
            }
            return;
        }
        if completion_edge {
            if self.state == EditPreviewState::Running {
                self.state = EditPreviewState::Countdown;
            }
            return;
        }
        let mut collapse_delta = elapsed;
        if self.state == EditPreviewState::Countdown {
            let remaining = COUNTDOWN_DURATION.saturating_sub(self.countdown_elapsed);
            if elapsed < remaining {
                self.countdown_elapsed += elapsed;
                return;
            }
            self.countdown_elapsed = COUNTDOWN_DURATION;
            self.state = EditPreviewState::Collapsing;
            collapse_delta = elapsed.saturating_sub(remaining);
        }
        if self.state == EditPreviewState::Collapsing {
            self.collapse_elapsed = self
                .collapse_elapsed
                .saturating_add(collapse_delta)
                .min(COLLAPSE_DURATION);
            if self.collapse_elapsed == COLLAPSE_DURATION {
                self.state = EditPreviewState::Collapsed;
            }
        }
    }

    /// Reopen or keep open indefinitely, including before tool completion.
    pub fn expand(&mut self) {
        self.state = EditPreviewState::PinnedOpen;
        self.collapse_elapsed = Duration::ZERO;
    }

    /// Begin a smooth manual collapse. Running tools remain open. Repeated
    /// collapse requests do not restart an in-flight collapse animation.
    pub fn collapse(&mut self) {
        if !self.done
            || matches!(
                self.state,
                EditPreviewState::Collapsing | EditPreviewState::Collapsed
            )
        {
            return;
        }
        self.state = EditPreviewState::Collapsing;
        self.countdown_elapsed = COUNTDOWN_DURATION;
        self.collapse_elapsed = Duration::ZERO;
    }

    pub fn state(&self) -> EditPreviewState {
        self.state
    }

    /// Countdown width in [0, 1]. Zero outside automatic countdown.
    pub fn remaining_fraction(&self) -> f32 {
        if self.state != EditPreviewState::Countdown {
            return 0.0;
        }
        1.0 - self.countdown_elapsed.as_secs_f32() / COUNTDOWN_DURATION.as_secs_f32()
    }

    /// Expanded height/opacity factor in [0, 1], using smoothstep easing.
    pub fn expansion_fraction(&self) -> f32 {
        match self.state {
            EditPreviewState::Collapsed => 0.0,
            EditPreviewState::Collapsing => {
                let t = self.collapse_elapsed.as_secs_f32() / COLLAPSE_DURATION.as_secs_f32();
                1.0 - t * t * (3.0 - 2.0 * t)
            }
            _ => 1.0,
        }
    }

    /// Whether the caller should schedule another animation frame.
    pub fn needs_animation(&self) -> bool {
        matches!(
            self.state,
            EditPreviewState::Countdown | EditPreviewState::Collapsing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed() -> EditPreviewTimer {
        let mut timer = EditPreviewTimer::new();
        timer.update(true, Duration::from_secs(99));
        timer
    }

    #[test]
    fn running_stays_open_and_completion_starts_full_countdown() {
        let mut timer = EditPreviewTimer::new();
        timer.update(false, Duration::MAX);
        timer.collapse();
        assert_eq!(timer.state(), EditPreviewState::Running);
        assert_eq!(timer.expansion_fraction(), 1.0);
        assert!(!timer.needs_animation());
        timer.update(true, Duration::MAX);
        assert_eq!(timer.state(), EditPreviewState::Countdown);
        assert_eq!(timer.remaining_fraction(), 1.0);
        assert!(timer.needs_animation());
    }

    #[test]
    fn countdown_reaches_zero_before_smooth_collapse() {
        let mut timer = completed();
        timer.update(true, Duration::from_millis(2500));
        assert_eq!(timer.remaining_fraction(), 0.5);
        assert_eq!(timer.expansion_fraction(), 1.0);
        timer.update(true, Duration::from_millis(2500));
        assert_eq!(timer.state(), EditPreviewState::Collapsing);
        assert_eq!(timer.remaining_fraction(), 0.0);
        assert_eq!(timer.expansion_fraction(), 1.0);
        timer.update(true, Duration::from_millis(60));
        assert_eq!(timer.expansion_fraction(), 0.84375);
        timer.update(true, Duration::from_millis(60));
        assert_eq!(timer.expansion_fraction(), 0.5);
        timer.update(true, Duration::from_millis(120));
        assert_eq!(timer.state(), EditPreviewState::Collapsed);
        assert_eq!(timer.expansion_fraction(), 0.0);
        assert!(!timer.needs_animation());
    }

    #[test]
    fn large_frames_carry_over_and_saturate() {
        let mut timer = completed();
        timer.update(true, COUNTDOWN_DURATION + Duration::from_millis(120));
        assert_eq!(timer.expansion_fraction(), 0.5);
        timer.update(true, Duration::MAX);
        assert_eq!(timer.state(), EditPreviewState::Collapsed);
        timer.update(true, Duration::MAX);
        assert_eq!(timer.expansion_fraction(), 0.0);
    }

    #[test]
    fn pin_before_completion_disables_countdown() {
        let mut timer = EditPreviewTimer::new();
        timer.expand();
        timer.update(false, Duration::MAX);
        timer.update(true, Duration::ZERO);
        timer.update(true, Duration::MAX);
        assert_eq!(timer.state(), EditPreviewState::PinnedOpen);
        assert_eq!(timer.expansion_fraction(), 1.0);
        assert_eq!(timer.remaining_fraction(), 0.0);
        assert!(!timer.needs_animation());
    }

    #[test]
    fn manual_collapse_is_animated_and_reopen_pins() {
        let mut timer = completed();
        timer.expand();
        timer.collapse();
        timer.update(true, Duration::from_millis(120));
        timer.collapse();
        assert_eq!(timer.expansion_fraction(), 0.5);
        timer.expand();
        timer.update(true, Duration::MAX);
        assert_eq!(timer.state(), EditPreviewState::PinnedOpen);
        assert_eq!(timer.expansion_fraction(), 1.0);
        timer.collapse();
        timer.update(true, COLLAPSE_DURATION);
        timer.expand();
        assert_eq!(timer.state(), EditPreviewState::PinnedOpen);
    }

    #[test]
    fn running_again_resets_automatic_timer() {
        let mut timer = completed();
        timer.update(true, Duration::MAX);
        timer.update(false, Duration::ZERO);
        assert_eq!(timer.state(), EditPreviewState::Running);
        assert_eq!(timer.expansion_fraction(), 1.0);
        timer.update(true, Duration::MAX);
        assert_eq!(timer.remaining_fraction(), 1.0);
    }

    #[test]
    fn zero_delta_does_not_restart_or_advance_countdown() {
        let mut timer = completed();
        timer.update(true, Duration::from_secs(1));
        let remaining = timer.remaining_fraction();
        timer.update(true, Duration::ZERO);
        assert_eq!(timer.remaining_fraction(), remaining);
    }
}
