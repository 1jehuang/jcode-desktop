//! Time-based wheel momentum. Touchpads and thumb dragging remain direct input.
use std::time::Duration;

#[derive(Debug, Default)]
pub(super) struct WheelGlide {
    pub(super) remaining: f32,
}

impl WheelGlide {
    // About 95% of the travel in 255 ms: responsive without a long, slippery tail.
    const DECAY_SECONDS: f32 = 0.085;
    pub(super) const SETTLE: f32 = 0.1;

    pub(super) fn push(&mut self, pixels: f32) {
        if !pixels.is_finite() || pixels == 0.0 {
            return;
        }
        // A reversal is a brake, not a fight against queued travel.
        if self.remaining.signum() != pixels.signum() {
            self.remaining = 0.0;
        }
        self.remaining += pixels;
    }

    pub(super) fn take_step(&mut self, elapsed: Duration) -> Option<f32> {
        if self.remaining == 0.0 || elapsed.is_zero() {
            return None;
        }
        let mut step = self.remaining * -(-elapsed.as_secs_f32() / Self::DECAY_SECONDS).exp_m1();
        if (self.remaining - step).abs() <= Self::SETTLE {
            step = self.remaining;
        }
        self.remaining -= step;
        Some(step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn travel(hz: u32, millis: u64) -> f32 {
        let mut glide = WheelGlide::default();
        glide.push(120.0);
        for _ in 0..(hz as u64 * millis / 1000) {
            glide.take_step(Duration::from_secs_f64(1.0 / hz as f64));
        }
        120.0 - glide.remaining
    }

    #[test]
    fn wheel_momentum_is_refresh_rate_independent() {
        let expected = travel(60, 500);
        for hz in [30, 120, 144, 240] {
            assert!((travel(hz, 500) - expected).abs() < 0.01);
        }
    }

    #[test]
    fn wheel_momentum_has_a_responsive_start_and_visible_coast() {
        let mut glide = WheelGlide::default();
        glide.push(120.0);
        let first = glide.take_step(Duration::from_millis(16)).unwrap();
        assert!((15.0..25.0).contains(&first));
        glide.take_step(Duration::from_millis(84));
        assert!((30.0..40.0).contains(&glide.remaining));
        let tail = glide.take_step(Duration::from_secs(1)).unwrap();
        assert!(tail > 0.0);
        assert_eq!(glide.remaining, 0.0);
    }

    #[test]
    fn wheel_momentum_accumulates_but_reverses_immediately() {
        let mut glide = WheelGlide::default();
        glide.push(80.0);
        glide.push(40.0);
        assert_eq!(glide.remaining, 120.0);
        glide.take_step(Duration::from_millis(16));
        glide.push(-30.0);
        assert_eq!(glide.remaining, -30.0);
        assert!(glide.take_step(Duration::from_millis(16)).unwrap() < 0.0);
    }

    #[test]
    fn wheel_momentum_preserves_small_deltas_and_ignores_invalid_input() {
        let mut glide = WheelGlide::default();
        glide.push(0.05);
        glide.push(0.0);
        glide.push(f32::NAN);
        glide.push(f32::INFINITY);
        assert_eq!(glide.take_step(Duration::ZERO), None);
        assert_eq!(glide.take_step(Duration::from_millis(16)), Some(0.05));
        assert_eq!(glide.take_step(Duration::from_millis(16)), None);
    }
}
