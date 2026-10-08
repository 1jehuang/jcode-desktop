//! Shared phase grid for decorative animation ticks.
//!
//! Every independently armed 30 Hz timer used to start at the moment its
//! element last painted, so four animations per working session woke at four
//! unrelated phases. In GPUI a child's `notify` dirties its cached ancestors,
//! so each desynchronized wake re-rendered the whole panel and workspace as a
//! separate frame. Aligning deadlines to one process-wide grid lets every
//! animation that is due wake in the same executor turn and share one frame.
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::BackgroundExecutor;

/// Delay from `now` until the next boundary of a `period` grid anchored at
/// `epoch`. Always strictly positive, so a tick can never spin.
pub fn delay_to_boundary(epoch: Instant, now: Instant, period: Duration) -> Duration {
    let period_ns = period.as_nanos().max(1);
    let into = now.saturating_duration_since(epoch).as_nanos() % period_ns;
    Duration::from_nanos((period_ns - into) as u64)
}

/// Wait until the next shared `period` boundary on `executor`'s clock.
pub async fn next_tick(executor: &BackgroundExecutor, period: Duration) {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let now = executor.now();
    // Test executors use a fake clock, so anchor there rather than on a real
    // Instant captured by an unrelated test in the same process.
    let epoch = if cfg!(any(test, feature = "test-support")) {
        now
    } else {
        *EPOCH.get_or_init(|| now)
    };
    executor.timer(delay_to_boundary(epoch, now, period)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlines_share_one_grid_regardless_of_arm_time() {
        let epoch = Instant::now();
        let period = Duration::from_millis(33);
        for offset_ms in [0u64, 1, 10, 32, 33, 34, 100] {
            let now = epoch + Duration::from_millis(offset_ms);
            let delay = delay_to_boundary(epoch, now, period);
            assert!(delay > Duration::ZERO && delay <= period);
            let wake = now + delay - epoch;
            assert_eq!(wake.as_nanos() % period.as_nanos(), 0, "offset {offset_ms}");
        }
    }
}
