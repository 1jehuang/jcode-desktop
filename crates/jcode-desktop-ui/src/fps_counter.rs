//! Passive animation presentation rate. Idle time is not a slow frame.
use std::time::{Duration, Instant};

use gpui::profiler::FrameDurationSnapshot;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct FpsCounter {
    baseline: Option<(Instant, FrameDurationSnapshot)>,
    fps: Option<u64>,
}

impl FpsCounter {
    pub(crate) fn label(
        &mut self,
        now: Instant,
        snapshot: impl FnOnce() -> FrameDurationSnapshot,
    ) -> String {
        if self
            .baseline
            .as_ref()
            .is_none_or(|(at, _)| now.duration_since(*at) >= SAMPLE_INTERVAL)
        {
            let current = snapshot();
            if let Some((_, previous)) = &self.baseline {
                // GPUI records these intervals only when consecutive presented
                // frames were requested by animation. Unlike draws / wall time,
                // this excludes deliberate idle gaps, but retains slow frames.
                let mut intervals = current.present_interval_histogram.clone();
                self.fps = if intervals
                    .subtract(&previous.present_interval_histogram)
                    .is_ok()
                    && !intervals.is_empty()
                    && intervals.mean() > 0.0
                {
                    Some((1_000_000_000.0 / intervals.mean()).round() as u64)
                } else {
                    None
                };
            }
            self.baseline = Some((now, current));
        }
        self.fps
            .map_or_else(|| "FPS · idle".into(), |fps| format!("{fps} FPS"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hdrhistogram::Histogram;

    fn snapshot(intervals_ns: &[u64]) -> FrameDurationSnapshot {
        let mut present_interval_histogram = Histogram::new(3).unwrap();
        for interval in intervals_ns {
            present_interval_histogram.record(*interval).unwrap();
        }
        FrameDurationSnapshot {
            draw_duration_histogram: Histogram::new(3).unwrap(),
            present_interval_histogram,
        }
    }

    #[test]
    fn fps_counter_excludes_idle_time_and_limits_snapshot_work() {
        let mut counter = FpsCounter::default();
        let now = Instant::now();
        assert_eq!(counter.label(now, || snapshot(&[])), "FPS · idle");
        assert_eq!(
            counter.label(now + Duration::from_millis(100), || panic!(
                "no snapshot between samples"
            )),
            "FPS · idle"
        );
        // Even after ten seconds idle, three actual 120 Hz animation frames
        // should report 120 FPS, not their count divided by ten seconds.
        let intervals = [8_333_333; 3];
        assert_eq!(
            counter.label(now + Duration::from_secs(10), || snapshot(&intervals)),
            "120 FPS"
        );
        assert_eq!(
            counter.label(now + Duration::from_secs(11), || snapshot(&intervals)),
            "FPS · idle"
        );
    }

    #[test]
    fn fps_counter_retains_slow_frames_and_uses_only_new_samples() {
        let mut counter = FpsCounter::default();
        let now = Instant::now();
        counter.label(now, || snapshot(&[8_333_333]));
        assert_eq!(
            counter.label(now + SAMPLE_INTERVAL, || snapshot(&[
                8_333_333, 50_000_000, 50_000_000
            ])),
            "20 FPS"
        );
        assert_eq!(
            counter.label(now + SAMPLE_INTERVAL * 2, || snapshot(&[
                8_333_333, 50_000_000, 50_000_000, 16_666_667
            ])),
            "60 FPS"
        );
    }

    #[test]
    fn fps_counter_rebaselines_after_profiler_reset() {
        let mut counter = FpsCounter::default();
        let now = Instant::now();
        counter.label(now, || snapshot(&[8_333_333]));
        assert_eq!(
            counter.label(now + SAMPLE_INTERVAL, || snapshot(&[])),
            "FPS · idle"
        );
        assert_eq!(
            counter.label(now + SAMPLE_INTERVAL * 2, || snapshot(&[16_666_667])),
            "60 FPS"
        );
    }
}
