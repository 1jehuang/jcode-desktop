//! Lightweight, self-development-only UI latency telemetry.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::profiler::{FrameDurationSnapshot, InputLatencySnapshot};
use serde::Serialize;

const SAMPLE_LIMIT: usize = 240;
const WAKE_WARN_MS: f64 = 12.0;
const RENDER_WARN_MS: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Good,
    Degraded,
    Bad,
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub wake_p95_ms: f64,
    pub render_p95_ms: f64,
    pub worst_ms: f64,
    pub animation_frame_p95_ms: f64,
    pub animation_fps: f64,
    pub missed_animation_frames: usize,
    pub health: Health,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuiSnapshot {
    pub draw_p95_ms: f64,
    pub present_p95_ms: f64,
    pub input_to_frame_p95_ms: f64,
    pub input_events_per_frame_p95: u64,
    pub mid_draw_inputs: u64,
}

struct PendingAction {
    name: &'static str,
    started: Instant,
    frame_baseline: FrameDurationSnapshot,
    input_baseline: InputLatencySnapshot,
    frames: usize,
    missed_frames: usize,
    previous_frame: Option<Instant>,
}

/// A deliberately small, synchronous recorder used only when explicitly opted
/// in. One short line is written after an action, never on each animation frame.
pub struct ActionCapture {
    path: PathBuf,
    pending: Option<PendingAction>,
}

#[derive(Serialize)]
struct ActionRecord {
    action: &'static str,
    elapsed_ms: f64,
    draw_p95_ms: f64,
    present_p95_ms: f64,
    input_p95_ms: f64,
    presented_frame_count: u64,
    frame_count: usize,
    missed_frames: usize,
}

impl ActionCapture {
    pub fn from_env() -> Option<Self> {
        std::env::var_os("JCODE_DESKTOP_PERF_ACTIONS")
            .filter(|path| !path.is_empty())
            .map(|path| Self {
                path: path.into(),
                pending: None,
            })
    }

    pub fn begin(
        &mut self,
        name: &'static str,
        now: Instant,
        frame_baseline: FrameDurationSnapshot,
        input_baseline: InputLatencySnapshot,
    ) {
        self.pending = Some(PendingAction {
            name,
            started: now,
            frame_baseline,
            input_baseline,
            frames: 0,
            missed_frames: 0,
            previous_frame: None,
        });
    }

    pub fn observe(
        &mut self,
        now: Instant,
        settled: bool,
        current_frame: FrameDurationSnapshot,
        current_input: InputLatencySnapshot,
    ) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        pending.frames += 1;
        if pending
            .previous_frame
            .replace(now)
            .is_some_and(|previous| now.saturating_duration_since(previous).as_secs_f64() > 0.0175)
        {
            pending.missed_frames += 1;
        }
        if !settled {
            return;
        }
        let pending = self.pending.take().unwrap();
        let mut draw = current_frame.draw_duration_histogram;
        let mut present = current_frame.present_interval_histogram;
        let mut input = current_input.latency_histogram;
        // GPUI exposes cumulative window histograms. Subtract the snapshots
        // taken at action dispatch so unrelated history cannot skew this
        // action's distribution. Subtracting cumulative percentiles would be
        // mathematically invalid.
        let _ = draw.subtract(&pending.frame_baseline.draw_duration_histogram);
        let _ = present.subtract(&pending.frame_baseline.present_interval_histogram);
        let _ = input.subtract(&pending.input_baseline.latency_histogram);
        let milliseconds = |nanoseconds: u64| nanoseconds as f64 / 1_000_000.0;
        let record = ActionRecord {
            action: pending.name,
            elapsed_ms: now.saturating_duration_since(pending.started).as_secs_f64() * 1_000.0,
            draw_p95_ms: milliseconds(draw.value_at_quantile(0.95)),
            present_p95_ms: milliseconds(present.value_at_quantile(0.95)),
            input_p95_ms: milliseconds(input.value_at_quantile(0.95)),
            presented_frame_count: present.len(),
            frame_count: pending.frames,
            missed_frames: pending.missed_frames,
        };
        if let Ok(line) = serde_json::to_string(&record)
            && let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

#[derive(Default)]
pub struct Profile {
    wake_lag_ms: VecDeque<f64>,
    render_ms: VecDeque<f64>,
    animation_frame_ms: VecDeque<f64>,
    last_animation_frame: Option<Instant>,
}

impl Profile {
    pub fn observe_wake_lag(&mut self, lag: Duration) {
        push(&mut self.wake_lag_ms, lag.as_secs_f64() * 1_000.0);
    }

    pub fn observe_render(&mut self, elapsed: Duration) {
        push(&mut self.render_ms, elapsed.as_secs_f64() * 1_000.0);
    }

    /// Record UI construction cadence while spatial motion is active. This is
    /// deliberately separate from compositor presentation telemetry: it tells
    /// us whether Jcode supplied a fresh animation state for each frame budget.
    pub fn observe_frame(&mut self, now: Instant, animation_active: bool) {
        if !animation_active {
            self.last_animation_frame = None;
            return;
        }
        if let Some(previous) = self.last_animation_frame.replace(now) {
            push(
                &mut self.animation_frame_ms,
                now.saturating_duration_since(previous).as_secs_f64() * 1_000.0,
            );
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let wake = percentile(&self.wake_lag_ms, 0.95);
        let render = percentile(&self.render_ms, 0.95);
        let animation_frame = percentile(&self.animation_frame_ms, 0.95);
        let ratio = (wake / WAKE_WARN_MS).max(render / RENDER_WARN_MS);
        let health = if ratio >= 2.0 {
            Health::Bad
        } else if ratio >= 1.0 {
            Health::Degraded
        } else {
            Health::Good
        };
        Snapshot {
            wake_p95_ms: wake,
            render_p95_ms: render,
            worst_ms: self
                .wake_lag_ms
                .iter()
                .chain(self.render_ms.iter())
                .copied()
                .fold(0.0, f64::max),
            animation_frame_p95_ms: animation_frame,
            animation_fps: if animation_frame > 0.0 {
                1_000.0 / animation_frame
            } else {
                0.0
            },
            missed_animation_frames: self
                .animation_frame_ms
                .iter()
                .filter(|interval| **interval > 17.5)
                .count(),
            health,
        }
    }
}

pub fn enabled(_arguments: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> bool {
    std::env::var_os("JCODE_DESKTOP_PERF_ACTIONS").is_some()
        || std::env::var("JCODE_DESKTOP_PERF")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "on"))
}

fn push(samples: &mut VecDeque<f64>, value: f64) {
    if samples.len() == SAMPLE_LIMIT {
        samples.pop_front();
    }
    samples.push_back(value);
}

fn percentile(samples: &VecDeque<f64>, percentile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.iter().copied().collect::<Vec<_>>();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_classifies_sustained_latency_and_keeps_tail_samples() {
        let mut profile = Profile::default();
        for millis in 1..=300 {
            profile.observe_wake_lag(Duration::from_millis(millis));
        }
        let snapshot = profile.snapshot();
        assert!(snapshot.wake_p95_ms >= 288.0);
        assert_eq!(snapshot.health, Health::Bad);
    }

    #[test]
    fn healthy_render_and_wake_times_stay_good() {
        let mut profile = Profile::default();
        for _ in 0..20 {
            profile.observe_wake_lag(Duration::from_millis(2));
            profile.observe_render(Duration::from_millis(3));
        }
        assert_eq!(profile.snapshot().health, Health::Good);
    }

    #[test]
    fn animation_cadence_reports_fps_and_missed_budgets() {
        let mut profile = Profile::default();
        let start = Instant::now();
        profile.observe_frame(start, true);
        profile.observe_frame(start + Duration::from_millis(16), true);
        profile.observe_frame(start + Duration::from_millis(32), true);
        profile.observe_frame(start + Duration::from_millis(57), true);
        let snapshot = profile.snapshot();
        assert_eq!(snapshot.animation_frame_p95_ms, 25.0);
        assert_eq!(snapshot.animation_fps, 40.0);
        assert_eq!(snapshot.missed_animation_frames, 1);

        profile.observe_frame(start + Duration::from_millis(60), false);
        profile.observe_frame(start + Duration::from_millis(100), true);
        assert_eq!(profile.snapshot().missed_animation_frames, 1);
    }

    #[test]
    fn profile_requires_an_explicit_opt_in() {
        assert!(!enabled(["jcode-desktop", "--hot-reload"]));
        assert!(!enabled(["jcode-desktop"]));
    }

    #[test]
    fn action_capture_appends_one_record_when_settled() {
        let path = std::env::temp_dir().join(format!(
            "jcode-action-perf-{}-{}.jsonl",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let mut capture = ActionCapture {
            path: path.clone(),
            pending: None,
        };
        let start = Instant::now();
        let mut historical_draw = hdrhistogram::Histogram::<u64>::new(3).unwrap();
        historical_draw.record(100_000_000).unwrap();
        let mut historical_present = hdrhistogram::Histogram::<u64>::new(3).unwrap();
        historical_present.record(100_000_000).unwrap();
        let frame_baseline = FrameDurationSnapshot {
            draw_duration_histogram: historical_draw,
            present_interval_histogram: historical_present,
        };
        let mut historical_input = hdrhistogram::Histogram::<u64>::new(3).unwrap();
        historical_input.record(100_000_000).unwrap();
        let input_baseline = InputLatencySnapshot {
            latency_histogram: historical_input,
            events_per_frame_histogram: hdrhistogram::Histogram::<u64>::new(3).unwrap(),
            mid_draw_events_dropped: 0,
        };
        capture.begin(
            "focus_left",
            start,
            frame_baseline.clone(),
            input_baseline.clone(),
        );
        capture.observe(
            start + Duration::from_millis(16),
            false,
            frame_baseline.clone(),
            input_baseline.clone(),
        );
        let mut current_frame = frame_baseline;
        current_frame
            .draw_duration_histogram
            .record(2_500_000)
            .unwrap();
        current_frame
            .present_interval_histogram
            .record(16_000_000)
            .unwrap();
        current_frame
            .present_interval_histogram
            .record(24_000_000)
            .unwrap();
        let mut current_input = input_baseline;
        current_input.latency_histogram.record(5_000_000).unwrap();
        capture.observe(
            start + Duration::from_millis(40),
            true,
            current_frame,
            current_input,
        );
        let line = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(value["action"], "focus_left");
        assert_eq!(value["frame_count"], 2);
        assert_eq!(value["missed_frames"], 1);
        assert!(value["draw_p95_ms"].as_f64().unwrap() < 3.0);
        assert!(value["input_p95_ms"].as_f64().unwrap() < 6.0);
        assert_eq!(value["presented_frame_count"], 2);
        let _ = std::fs::remove_file(path);
    }
}
