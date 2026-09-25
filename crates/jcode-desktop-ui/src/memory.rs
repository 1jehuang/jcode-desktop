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

/// Stop khugepaged from collapsing this process's heap into 2 MiB pages.
///
/// With THP set to `always`, glibc arenas that hold freed-but-retained pages
/// get collapsed into huge pages, so every partially used 2 MiB region is
/// charged in full and `malloc_trim` can no longer return its free parts.
/// On a private Xvfb fixture this added 36 MiB (198 vs 162 MiB RSS after
/// four minutes). The live host carried 90 to 135 MiB of anonymous huge
/// pages. A UI process gains nothing measurable from huge TLB entries.
/// Setting `JCODE_DESKTOP_THP=1` keeps the system default. Idempotent.
pub fn disable_transparent_huge_pages() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("JCODE_DESKTOP_THP").is_none_or(|value| value != "1") {
        // SAFETY: PR_SET_THP_DISABLE only sets a per-process mm flag.
        unsafe {
            libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0);
        }
    }
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

    #[cfg(target_os = "linux")]
    #[test]
    fn huge_pages_disabled_for_process() {
        if std::env::var_os("JCODE_DESKTOP_THP").is_some() {
            return;
        }
        disable_transparent_huge_pages();
        // SAFETY: PR_GET_THP_DISABLE reads the per-process flag.
        let flag = unsafe { libc::prctl(libc::PR_GET_THP_DISABLE, 0, 0, 0, 0) };
        assert_eq!(flag, 1);
    }
}
