//! Return freed heap to the OS in the long-lived desktop host.
//!
//! glibc keeps pages freed by transient work (history JSON, markdown and
//! syntax layout, image decode) inside its arenas. In a single-panel host this
//! retained slack was larger than the live heap. A rate-limited `malloc_trim`
//! from housekeeping hands those pages back without touching the frame path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Minimum spacing between trims; `malloc_trim` walks every arena.
pub const TRIM_INTERVAL: Duration = Duration::from_secs(30);

static LAST_TRIM_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// Whether enough time has passed since the last trim in this process.
/// Claims the slot atomically, so many windows share one trim per interval.
pub fn claim_trim_slot() -> bool {
    let now = now_ms();
    let last = LAST_TRIM_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < TRIM_INTERVAL.as_millis() as u64 {
        return false;
    }
    LAST_TRIM_MS
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
}

/// Release free heap pages. Blocking; call from a background executor.
pub fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim is thread-safe and only releases free pages.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_trim_slot_per_interval() {
        LAST_TRIM_MS.store(0, Ordering::Relaxed);
        assert!(claim_trim_slot());
        assert!(!claim_trim_slot());
        trim();
    }
}
