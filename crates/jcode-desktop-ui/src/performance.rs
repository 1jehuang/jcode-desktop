//! Lightweight, self-development-only UI latency telemetry.

use std::collections::VecDeque;
use std::time::Duration;

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
    pub health: Health,
}

#[derive(Default)]
pub struct Profile {
    wake_lag_ms: VecDeque<f64>,
    render_ms: VecDeque<f64>,
}

impl Profile {
    pub fn observe_wake_lag(&mut self, lag: Duration) {
        push(&mut self.wake_lag_ms, lag.as_secs_f64() * 1_000.0);
    }

    pub fn observe_render(&mut self, elapsed: Duration) {
        push(&mut self.render_ms, elapsed.as_secs_f64() * 1_000.0);
    }

    pub fn snapshot(&self) -> Snapshot {
        let wake = percentile(&self.wake_lag_ms, 0.95);
        let render = percentile(&self.render_ms, 0.95);
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
            health,
        }
    }
}

pub fn enabled(arguments: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> bool {
    let override_enabled = std::env::var("JCODE_DESKTOP_PERF")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "on"));
    override_enabled
        || arguments
            .into_iter()
            .any(|argument| argument.as_ref() == "--hot-reload")
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
    fn hot_reload_enables_the_profile() {
        assert!(enabled(["jcode-desktop", "--hot-reload"]));
        assert!(!enabled(["jcode-desktop"]));
    }
}
