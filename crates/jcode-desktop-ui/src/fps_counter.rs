//! Passive window draw-rate sampling, not monitor refresh rate. The last sample
//! stays visible at rest so the counter never needs an idle redraw loop.
use std::time::{Duration, Instant};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub(crate) struct FpsCounter {
    baseline: Option<(Instant, u64)>,
    fps: Option<u64>,
}

impl FpsCounter {
    pub(crate) fn label(&mut self, now: Instant, frames: impl FnOnce() -> u64) -> String {
        if self
            .baseline
            .is_none_or(|(at, _)| now.duration_since(at) >= SAMPLE_INTERVAL)
        {
            let count = frames();
            if let Some((at, previous)) = self.baseline {
                // A reset can occur when a profiler is cleared. Rebaseline rather
                // than underflowing or displaying an enormous FPS value.
                self.fps = count.checked_sub(previous).map(|delta| {
                    (delta as f64 / now.duration_since(at).as_secs_f64()).round() as u64
                });
            }
            self.baseline = Some((now, count));
        }
        self.fps
            .map_or_else(|| "— FPS".into(), |fps| format!("{fps} FPS"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_counter_samples_actual_draws_at_a_bounded_rate() {
        let mut counter = FpsCounter::default();
        let now = Instant::now();
        assert_eq!(counter.label(now, || 10), "— FPS");
        assert_eq!(
            counter.label(now + Duration::from_millis(499), || panic!(
                "no snapshot between samples"
            )),
            "— FPS"
        );
        assert_eq!(counter.label(now + SAMPLE_INTERVAL, || 70), "120 FPS");
        assert_eq!(
            counter.label(now + Duration::from_millis(700), || panic!(
                "retain last sample"
            )),
            "120 FPS"
        );
        assert_eq!(
            counter.label(now + Duration::from_secs(1), || 100),
            "60 FPS"
        );
    }

    #[test]
    fn fps_counter_handles_idle_and_profiler_reset() {
        let mut counter = FpsCounter::default();
        let now = Instant::now();
        counter.label(now, || 100);
        assert_eq!(
            counter.label(now + Duration::from_secs(10), || 100),
            "0 FPS"
        );
        assert_eq!(counter.label(now + Duration::from_secs(11), || 1), "— FPS");
        assert_eq!(
            counter.label(now + Duration::from_secs(12), || 61),
            "60 FPS"
        );
    }
}
