//! Frame-paced scrolling. Precise input uses a short filter, not extra inertia.
use std::time::Duration;

#[derive(Debug, Default)]
pub(super) struct WheelGlide {
    pub(super) remaining: f32,
    precise: bool,
}

impl WheelGlide {
    // About 95% of the travel in 255 ms: responsive without a long, slippery tail.
    const DECAY_SECONDS: f32 = 0.085;
    // Smooth event bursts over a few frames without fighting native touchpad
    // inertia. 95% of each delta is delivered within 75 ms.
    const PRECISE_DECAY_SECONDS: f32 = 0.025;
    pub(super) const SETTLE: f32 = 0.1;

    pub(super) fn push(&mut self, pixels: f32) {
        self.push_input(pixels, false);
    }

    pub(super) fn push_input(&mut self, pixels: f32, precise: bool) {
        if !pixels.is_finite() || pixels == 0.0 {
            return;
        }
        // A reversal is a brake, not a fight against queued travel.
        if self.precise != precise || self.remaining.signum() != pixels.signum() {
            self.remaining = 0.0;
        }
        self.precise = precise;
        self.remaining += pixels;
    }

    pub(super) fn take_step(&mut self, elapsed: Duration) -> Option<f32> {
        if self.remaining == 0.0 || elapsed.is_zero() {
            return None;
        }
        let decay = if self.precise {
            Self::PRECISE_DECAY_SECONDS
        } else {
            Self::DECAY_SECONDS
        };
        let mut step = self.remaining * -(-elapsed.as_secs_f32() / decay).exp_m1();
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

    #[test]
    fn precise_scroll_smooths_bursts_with_a_short_tail_and_exact_distance() {
        let mut glide = WheelGlide::default();
        glide.push_input(30.0, true);
        glide.push_input(50.0, true);
        let first = glide.take_step(Duration::from_millis(16)).unwrap();
        assert!((35.0..40.0).contains(&first));
        let next = glide.take_step(Duration::from_millis(59)).unwrap();
        assert!(glide.remaining < 4.0, "95% delivered within 75 ms");
        let tail = glide.take_step(Duration::from_millis(200)).unwrap();
        assert!((first + next + tail - 80.0).abs() < 0.001);
        assert_eq!(glide.remaining, 0.0);
    }

    #[test]
    fn precise_scroll_is_refresh_rate_independent() {
        for hz in [30, 60, 120, 144, 240] {
            let mut glide = WheelGlide::default();
            glide.push_input(120.0, true);
            for _ in 0..hz / 6 {
                glide.take_step(Duration::from_secs_f64(1.0 / hz as f64));
            }
            let expected = 120.0 * (-1.0 / (6.0 * WheelGlide::PRECISE_DECAY_SECONDS)).exp();
            assert!((glide.remaining - expected).abs() < 0.001);
        }
    }

    #[test]
    fn fractional_and_changing_refresh_cadence_preserves_elapsed_time_motion() {
        // Monitor moves, VRR and missed callbacks must not turn the number of
        // frames into a speed control. Compare identical elapsed time, not a
        // rounded integer frame count, at both above-120 and fractional Hz.
        for precise in [false, true] {
            for rates in [
                [59.94, 59.94, 59.94, 59.94],
                [165.0, 165.0, 165.0, 165.0],
                [240.0, 60.0, 144.0, 30.0],
            ] {
                let mut paced = WheelGlide::default();
                let mut single_step = WheelGlide::default();
                paced.push_input(120.0, precise);
                single_step.push_input(120.0, precise);
                let mut elapsed = Duration::ZERO;
                let mut traveled = 0.0;
                for hz in rates {
                    let interval = Duration::from_secs_f64(1.0 / hz);
                    elapsed += interval;
                    traveled += paced.take_step(interval).unwrap();
                }
                single_step.take_step(elapsed);
                assert!(
                    (paced.remaining - single_step.remaining).abs() < 0.001,
                    "precise={precise}, rates={rates:?}"
                );
                assert!((traveled + paced.remaining - 120.0).abs() < 0.001);
                traveled += paced.take_step(Duration::from_secs(1)).unwrap();
                assert_eq!(paced.remaining, 0.0);
                assert!((traveled - 120.0).abs() < 0.001);
            }
        }
    }

    #[test]
    fn changing_input_source_or_direction_discards_old_travel() {
        let mut glide = WheelGlide::default();
        glide.push(120.0);
        glide.push_input(20.0, true);
        assert_eq!(glide.remaining, 20.0);
        glide.push_input(-5.0, true);
        assert_eq!(glide.remaining, -5.0);
        glide.push(-40.0);
        assert_eq!(glide.remaining, -40.0);
    }
}
