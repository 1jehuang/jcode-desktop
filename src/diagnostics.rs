//! Process-wide persistent diagnostics for startup failures and UI lag.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const HIGH_CPU_PERCENT: f64 = 50.0;
/// Routine memory samples, so long-running leaks leave a growth history.
const MEMORY_LOG_EVERY: u32 = 120; // 120 * 5s = 10 minutes
const MIB: u64 = 1024 * 1024;
/// Warn when resident memory first crosses this level, then per doubling.
const HIGH_RSS_BYTES: u64 = 1024 * MIB;
/// Warn when resident memory grows this much within one routine interval.
const FAST_GROWTH_BYTES: u64 = 256 * MIB;
/// Window, GPU, and UI initialization legitimately allocate hundreds of MiB.
/// Track the baseline silently for the first 30 seconds.
const STARTUP_TICKS: u32 = 6;

pub fn install() -> io::Result<PathBuf> {
    let path = log_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_if_needed(&path)?;
    redirect_stderr(&path)?;
    eprintln!(
        "{} jcode-desktop started pid={} version={}",
        timestamp(),
        std::process::id(),
        jcode_desktop_ui::build_version()
    );
    spawn_cpu_monitor();
    Ok(path)
}

fn log_path() -> PathBuf {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(state).join("jcode-desktop/jcode-desktop.log");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".local/state/jcode-desktop/jcode-desktop.log")
}

fn rotate_if_needed(path: &Path) -> io::Result<()> {
    if path.metadata().map(|meta| meta.len()).unwrap_or(0) < MAX_LOG_BYTES {
        return Ok(());
    }
    let previous = path.with_extension("log.previous");
    let _ = fs::remove_file(&previous);
    fs::rename(path, previous)
}

#[cfg(unix)]
fn redirect_stderr(path: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // SAFETY: both descriptors are valid here. dup2 keeps its own reference to
    // the open file description, so `file` can be dropped after the call.
    if unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn redirect_stderr(path: &Path) -> io::Result<()> {
    // Keep the file discoverable on unsupported hosts even though stderr cannot
    // yet be redirected without platform-specific APIs.
    OpenOptions::new().create(true).append(true).open(path)?;
    Ok(())
}

fn timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "[unix {}.{:03}]",
        duration.as_secs(),
        duration.subsec_millis()
    )
}

#[cfg(target_os = "linux")]
fn spawn_cpu_monitor() {
    std::thread::Builder::new()
        .name("jcode-lag-monitor".into())
        .spawn(|| {
            let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
            if ticks_per_second <= 0.0 {
                return;
            }
            let mut previous = read_process_ticks();
            let mut high_windows = 0_u32;
            let mut memory = MemoryMonitor::default();
            if let Some(sample) = MemorySample::read() {
                eprintln!("{} memory startup: {}", timestamp(), sample);
                memory.last_logged_rss = sample.rss;
            }
            loop {
                std::thread::sleep(SAMPLE_INTERVAL);
                memory.tick();
                let current = read_process_ticks();
                let (Some(before), Some(after)) = (previous, current) else {
                    previous = current;
                    continue;
                };
                previous = Some(after);
                let cpu = after.saturating_sub(before) as f64
                    / ticks_per_second
                    / SAMPLE_INTERVAL.as_secs_f64()
                    * 100.0;
                if cpu >= HIGH_CPU_PERCENT {
                    high_windows += 1;
                    // Log immediately, then every 30 seconds while the problem
                    // persists. This is enough evidence without flooding disk.
                    if high_windows == 1 || high_windows % 6 == 0 {
                        eprintln!(
                            "{} lag warning: process CPU {:.1}% for {}s (pid={})",
                            timestamp(),
                            cpu,
                            high_windows as u64 * SAMPLE_INTERVAL.as_secs(),
                            std::process::id()
                        );
                    }
                } else if high_windows > 0 {
                    eprintln!(
                        "{} lag recovered: process CPU {:.1}% after {}s high",
                        timestamp(),
                        cpu,
                        high_windows as u64 * SAMPLE_INTERVAL.as_secs()
                    );
                    high_windows = 0;
                }
            }
        })
        .expect("spawn lag monitor");
}

#[cfg(target_os = "linux")]
fn read_process_ticks() -> Option<u64> {
    let stat = fs::read_to_string("/proc/self/stat").ok()?;
    // The command name is parenthesized and may contain spaces. Fields after
    // the final ')' begin with field 3; utime/stime are fields 14 and 15.
    let fields = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

#[cfg(not(target_os = "linux"))]
fn spawn_cpu_monitor() {}

/// Process memory breakdown from `/proc/self/status` and `smaps_rollup`.
/// Anonymous memory is the heap and GPUI allocations, file memory is mapped
/// code (including retained hot-reload libraries), and shmem covers tmpfs and
/// GPU buffers shared with the compositor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MemorySample {
    rss: u64,
    anon: u64,
    file: u64,
    shmem: u64,
    swap: u64,
    threads: u64,
}

impl MemorySample {
    #[cfg(target_os = "linux")]
    fn read() -> Option<Self> {
        Some(Self::parse_status(&fs::read_to_string("/proc/self/status").ok()?))
    }

    #[cfg(not(target_os = "linux"))]
    fn read() -> Option<Self> {
        None
    }

    fn parse_status(status: &str) -> Self {
        let mut sample = Self::default();
        for line in status.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let number = value
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0);
            match key {
                "VmRSS" => sample.rss = number * 1024,
                "RssAnon" => sample.anon = number * 1024,
                "RssFile" => sample.file = number * 1024,
                "RssShmem" => sample.shmem = number * 1024,
                "VmSwap" => sample.swap = number * 1024,
                "Threads" => sample.threads = number,
                _ => {}
            }
        }
        sample
    }
}

impl std::fmt::Display for MemorySample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "rss={}MiB anon={}MiB file={}MiB shmem={}MiB swap={}MiB threads={} pid={}",
            self.rss / MIB,
            self.anon / MIB,
            self.file / MIB,
            self.shmem / MIB,
            self.swap / MIB,
            self.threads,
            std::process::id()
        )
    }
}

#[derive(Default)]
struct MemoryMonitor {
    ticks: u32,
    last_logged_rss: u64,
    next_warning_rss: u64,
}

impl MemoryMonitor {
    fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        let Some(sample) = MemorySample::read() else {
            return;
        };
        for line in self.observe(sample) {
            eprintln!("{} {line}", timestamp());
        }
    }

    /// Returns log lines for this sample. Kept pure for unit tests.
    fn observe(&mut self, sample: MemorySample) -> Vec<String> {
        let mut lines = Vec::new();
        if self.next_warning_rss == 0 {
            self.next_warning_rss = HIGH_RSS_BYTES;
        }
        if sample.rss >= self.next_warning_rss {
            lines.push(format!(
                "memory warning: resident memory above {}MiB: {sample}",
                self.next_warning_rss / MIB
            ));
            while self.next_warning_rss <= sample.rss {
                self.next_warning_rss = self.next_warning_rss.saturating_mul(2);
            }
        }
        let growth = sample.rss.saturating_sub(self.last_logged_rss);
        if self.ticks <= STARTUP_TICKS {
            self.last_logged_rss = self.last_logged_rss.max(sample.rss);
        } else if growth >= FAST_GROWTH_BYTES {
            lines.push(format!(
                "memory warning: grew {}MiB since last sample: {sample}",
                growth / MIB
            ));
            self.last_logged_rss = sample.rss;
        } else if self.ticks % MEMORY_LOG_EVERY == 0 {
            lines.push(format!("memory: {sample}"));
            self.last_logged_rss = sample.rss;
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(rss_mib: u64) -> MemorySample {
        MemorySample {
            rss: rss_mib * MIB,
            ..MemorySample::default()
        }
    }

    #[test]
    fn status_memory_fields_parse() {
        let parsed = MemorySample::parse_status(
            "Name:\tjcode\nVmRSS:\t  2048 kB\nRssAnon:\t1024 kB\nRssFile:\t512 kB\nRssShmem:\t512 kB\nVmSwap:\t0 kB\nThreads:\t38\n",
        );
        assert_eq!(parsed.rss, 2 * MIB);
        assert_eq!(parsed.anon, MIB);
        assert_eq!(parsed.file, MIB / 2);
        assert_eq!(parsed.threads, 38);
        #[cfg(target_os = "linux")]
        assert!(MemorySample::read().unwrap().rss > 0);
    }

    #[test]
    fn memory_monitor_warns_on_threshold_doubling_and_fast_growth() {
        let mut monitor = MemoryMonitor {
            last_logged_rss: 200 * MIB,
            ..MemoryMonitor::default()
        };
        monitor.ticks = 1;
        // Startup allocation establishes the baseline without warnings.
        assert!(monitor.observe(sample(300)).is_empty());
        monitor.ticks = STARTUP_TICKS + 1;
        assert!(monitor.observe(sample(310)).is_empty());
        let lines = monitor.observe(sample(1100));
        assert!(lines.iter().any(|l| l.contains("above 1024MiB")));
        assert!(lines.iter().any(|l| l.contains("grew 800MiB")));
        // No repeated threshold warning until the next doubling.
        assert!(monitor.observe(sample(1200)).is_empty());
        assert!(monitor.observe(sample(2100)).iter().any(|l| l.contains("above 2048MiB")));
        monitor.ticks = MEMORY_LOG_EVERY;
        assert_eq!(monitor.observe(sample(2150)), vec![format!("memory: {}", sample(2150))]);
    }

    #[test]
    fn linux_stat_tick_fields_are_readable() {
        #[cfg(target_os = "linux")]
        assert!(super::read_process_ticks().is_some());
    }
}
