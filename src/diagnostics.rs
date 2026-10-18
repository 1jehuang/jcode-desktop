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
/// Warn when RSS (or larger macOS footprint) crosses this level, then per doubling.
const HIGH_RSS_BYTES: u64 = 1024 * MIB;
/// Warn when the tracked memory metric grows this much within one routine interval.
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
    // Say where diagnostics go while stderr still reaches the launching
    // terminal, so `RUST_LOG=debug jcode-desktop` users know where to look.
    eprintln!("desktop diagnostics: {}", path.display());
    redirect_stderr(&path)?;
    install_logger();
    eprintln!(
        "{} jcode-desktop started pid={} version={} os={} arch={}",
        timestamp(),
        std::process::id(),
        jcode_desktop_ui::build_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    spawn_cpu_monitor();
    Ok(path)
}

/// `$XDG_STATE_HOME/jcode-desktop`, `%LOCALAPPDATA%\Jcode Desktop` on
/// Windows, `~/Library/Application Support/Jcode` on macOS, and
/// `~/.local/state/jcode-desktop` elsewhere. Windows sets no `HOME`, which
/// used to put this log under `%TEMP%\.local\state` (#5).
fn log_path() -> PathBuf {
    jcode_desktop_model::platform_dirs::state_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("jcode-desktop"))
        .join("jcode-desktop.log")
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

#[cfg(windows)]
fn redirect_stderr(path: &Path) -> io::Result<()> {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};

    // A GUI-subsystem process has no console, so stderr went nowhere and the
    // log stayed empty (#5). Rust's stderr resolves the process handle on
    // every write, so pointing STD_ERROR_HANDLE at the file captures every
    // eprintln!, panic message, and `log` record from here on, exactly like
    // the dup2 on Unix.
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // Leak the handle on purpose: it must stay valid for the process lifetime.
    let handle = file.into_raw_handle();
    // SAFETY: `handle` is a valid, owned file handle that is never closed.
    unsafe {
        if SetStdHandle(STD_ERROR_HANDLE, handle as _) == 0 {
            return Err(io::Error::last_os_error());
        }
        // stdout carries nothing useful for a windowed app, but stray
        // println! output is better in the log than silently dropped.
        SetStdHandle(STD_OUTPUT_HANDLE, handle as _);
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn redirect_stderr(path: &Path) -> io::Result<()> {
    OpenOptions::new().create(true).append(true).open(path)?;
    Ok(())
}

/// Route `log` records (GPUI's platform backends, the SDK) into stderr, which
/// is the diagnostics log. Without a logger, `RUST_LOG=debug` did nothing.
/// The level comes from `RUST_LOG` (`error`..`trace`, or `off`), default warn.
fn install_logger() {
    let level = log_level(std::env::var("RUST_LOG").ok().as_deref());
    if log::set_logger(&StderrLogger).is_ok() {
        log::set_max_level(level);
    }
}

/// Accepts plain levels and `env_logger`-style directives, taking the most
/// verbose level mentioned (`gpui=debug,warn` enables debug).
fn log_level(spec: Option<&str>) -> log::LevelFilter {
    let Some(spec) = spec.map(str::trim).filter(|spec| !spec.is_empty()) else {
        return log::LevelFilter::Warn;
    };
    spec.split(',')
        .filter_map(|directive| {
            let level = directive.rsplit('=').next()?.trim();
            level.parse::<log::LevelFilter>().ok()
        })
        .max()
        .unwrap_or(log::LevelFilter::Warn)
}

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "{} {} {}: {}",
                timestamp(),
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
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

#[cfg(target_os = "macos")]
fn spawn_cpu_monitor() {
    // Keep memory sampling independent of the UI event loop, including when
    // the main thread is stalled under memory pressure. No subprocesses or
    // retained sample history, and failure to start diagnostics is nonfatal.
    if let Err(error) = std::thread::Builder::new()
        .name("jcode-memory-monitor".into())
        .spawn(|| {
            let mut memory = MemoryMonitor::default();
            if let Some(sample) = MemorySample::read() {
                eprintln!("{} memory startup: {}", timestamp(), sample);
                memory.last_logged_rss = sample.pressure_bytes();
            } else {
                for line in memory.observe_read(None) {
                    eprintln!("{} {line}", timestamp());
                }
            }
            loop {
                std::thread::sleep(SAMPLE_INTERVAL);
                memory.tick();
            }
        })
    {
        eprintln!("{} memory monitor unavailable: {error}", timestamp());
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn spawn_cpu_monitor() {}

/// Linux breakdown from `/proc/self/status`, or macOS RSS/physical footprint
/// from libproc. Linux anonymous memory includes heap/GPUI allocations, file
/// memory includes mapped code, and shmem includes compositor-shared buffers.
/// macOS footprint accounts for pressure not represented by resident pages.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct MemorySample {
    rss: u64,
    /// macOS physical footprint is distinct from resident memory. Never
    /// substitute zero-valued Linux breakdowns for unavailable macOS fields.
    physical_footprint: Option<u64>,
    anon: u64,
    file: u64,
    shmem: u64,
    swap: u64,
    threads: u64,
}

impl MemorySample {
    #[cfg(target_os = "linux")]
    fn read() -> Option<Self> {
        Some(Self::parse_status(
            &fs::read_to_string("/proc/self/status").ok()?,
        ))
    }

    #[cfg(target_os = "macos")]
    fn read() -> Option<Self> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
        // SAFETY: V2 selects exactly rusage_info_v2. libproc's historical
        // pointer-to-void-pointer signature takes the struct buffer itself,
        // not a pointer to a separately allocated pointer. Read it only on
        // success. Querying our own PID needs no elevated privileges.
        let result = unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V2,
                usage.as_mut_ptr().cast::<libc::rusage_info_t>(),
            )
        };
        if result != 0 {
            return Self::from_macos_usage(Err(result));
        }
        // SAFETY: the successful V2 call initialized the provided buffer.
        let usage = unsafe { usage.assume_init() };
        Self::from_macos_usage(Ok((usage.ri_resident_size, usage.ri_phys_footprint)))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn read() -> Option<Self> {
        None
    }

    #[cfg(any(target_os = "macos", test))]
    fn from_macos_usage(result: Result<(u64, u64), i32>) -> Option<Self> {
        let (rss, physical_footprint) = result.ok()?;
        Some(Self {
            rss,
            physical_footprint: Some(physical_footprint),
            ..Self::default()
        })
    }

    fn pressure_bytes(self) -> u64 {
        self.rss.max(self.physical_footprint.unwrap_or(0))
    }

    fn pressure_label(self) -> &'static str {
        if self
            .physical_footprint
            .is_some_and(|bytes| bytes >= self.rss)
        {
            "physical footprint"
        } else {
            "resident memory"
        }
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
        if let Some(footprint) = self.physical_footprint {
            return write!(
                f,
                "rss={}MiB footprint={}MiB rss_bytes={} footprint_bytes={} pid={}",
                self.rss / MIB,
                footprint / MIB,
                self.rss,
                footprint,
                std::process::id()
            );
        }
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
    // On macOS these track max(RSS, physical footprint), on Linux RSS.
    last_logged_rss: u64,
    next_warning_rss: u64,
    unavailable: bool,
}

impl MemoryMonitor {
    fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        for line in self.observe_read(MemorySample::read()) {
            eprintln!("{} {line}", timestamp());
        }
    }

    fn observe_read(&mut self, sample: Option<MemorySample>) -> Vec<String> {
        let Some(sample) = sample else {
            if std::mem::replace(&mut self.unavailable, true) {
                return Vec::new();
            }
            return vec!["memory sampling unavailable (will retry)".into()];
        };
        let recovered = std::mem::replace(&mut self.unavailable, false);
        let mut lines = self.observe(sample);
        if recovered {
            lines.insert(0, format!("memory sampling recovered: {sample}"));
        }
        lines
    }

    /// Returns log lines for this sample. Kept pure for unit tests.
    fn observe(&mut self, sample: MemorySample) -> Vec<String> {
        let mut lines = Vec::new();
        if self.next_warning_rss == 0 {
            self.next_warning_rss = HIGH_RSS_BYTES;
        }
        let pressure = sample.pressure_bytes();
        if pressure >= self.next_warning_rss && self.next_warning_rss < u64::MAX {
            lines.push(format!(
                "memory warning: {} above {}MiB: {sample}",
                sample.pressure_label(),
                self.next_warning_rss / MIB
            ));
            while self.next_warning_rss <= pressure && self.next_warning_rss < u64::MAX {
                self.next_warning_rss = self.next_warning_rss.saturating_mul(2);
            }
        }
        let growth = pressure.saturating_sub(self.last_logged_rss);
        if self.ticks <= STARTUP_TICKS {
            self.last_logged_rss = self.last_logged_rss.max(pressure);
        } else if growth >= FAST_GROWTH_BYTES {
            lines.push(format!(
                "memory warning: grew {}MiB since last sample: {sample}",
                growth / MIB
            ));
            self.last_logged_rss = pressure;
        } else if self.ticks % MEMORY_LOG_EVERY == 0 {
            lines.push(format!("memory: {sample}"));
            self.last_logged_rss = pressure;
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
        assert!(
            monitor
                .observe(sample(2100))
                .iter()
                .any(|l| l.contains("above 2048MiB"))
        );
        monitor.ticks = MEMORY_LOG_EVERY;
        assert_eq!(
            monitor.observe(sample(2150)),
            vec![format!("memory: {}", sample(2150))]
        );
    }

    #[test]
    fn macos_usage_preserves_bytes_and_distinguishes_unavailable_fields() {
        let sample = MemorySample::from_macos_usage(Ok((250 * MIB + 7, 1500 * MIB + 9)))
            .expect("successful macOS sample");
        assert_eq!(sample.rss, 250 * MIB + 7);
        assert_eq!(sample.physical_footprint, Some(1500 * MIB + 9));
        assert_eq!(sample.pressure_bytes(), 1500 * MIB + 9);
        let text = sample.to_string();
        assert!(text.contains("rss=250MiB footprint=1500MiB"));
        assert!(text.contains(&format!("rss_bytes={}", 250 * MIB + 7)));
        assert!(text.contains(&format!("footprint_bytes={}", 1500 * MIB + 9)));
        for unavailable in ["anon=", "file=", "shmem=", "swap=", "threads="] {
            assert!(!text.contains(unavailable), "misleading field: {text}");
        }
        assert!(MemorySample::from_macos_usage(Err(-1)).is_none());
    }

    #[test]
    fn memory_monitor_tracks_footprint_even_when_rss_is_small() {
        let mut monitor = MemoryMonitor {
            ticks: STARTUP_TICKS + 1,
            last_logged_rss: 300 * MIB,
            ..MemoryMonitor::default()
        };
        let sample = MemorySample::from_macos_usage(Ok((200 * MIB, 1500 * MIB))).unwrap();
        let lines = monitor.observe(sample);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("physical footprint above 1024MiB"))
        );
        assert!(lines.iter().any(|line| line.contains("grew 1200MiB")));
        assert!(
            monitor.observe(sample).is_empty(),
            "unchanged sample is quiet"
        );
        // Also retain the RSS warning if resident pages exceed footprint.
        let larger_rss = MemorySample::from_macos_usage(Ok((2200 * MIB, 1500 * MIB))).unwrap();
        assert!(
            monitor
                .observe(larger_rss)
                .iter()
                .any(|line| { line.contains("resident memory above 2048MiB") })
        );
    }

    #[test]
    fn memory_sampling_failure_logs_only_transitions_and_recovers() {
        let mut monitor = MemoryMonitor::default();
        assert_eq!(monitor.observe_read(None).len(), 1);
        for _ in 0..1000 {
            assert!(monitor.observe_read(None).is_empty());
        }
        let lines = monitor.observe_read(Some(sample(100)));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("memory sampling recovered:"));
        assert!(monitor.observe_read(Some(sample(100))).is_empty());
        assert_eq!(monitor.observe_read(None).len(), 1);
    }

    #[test]
    fn memory_threshold_saturates_without_looping_or_repeating() {
        let mut monitor = MemoryMonitor {
            ticks: STARTUP_TICKS + 1,
            ..MemoryMonitor::default()
        };
        let sample = MemorySample {
            rss: u64::MAX,
            ..MemorySample::default()
        };
        assert_eq!(monitor.observe(sample).len(), 2);
        assert_eq!(monitor.next_warning_rss, u64::MAX);
        assert!(monitor.observe(sample).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_current_process_memory_is_readable() {
        let sample = MemorySample::read().expect("current process rusage");
        assert!(sample.rss > 0);
        assert!(sample.physical_footprint.is_some_and(|bytes| bytes > 0));
    }

    #[test]
    fn rust_log_specs_pick_the_most_verbose_level() {
        use log::LevelFilter::*;
        assert_eq!(log_level(None), Warn);
        assert_eq!(log_level(Some("")), Warn);
        assert_eq!(log_level(Some("debug")), Debug);
        assert_eq!(log_level(Some("OFF")), Off);
        assert_eq!(log_level(Some("gpui_windows=trace,info")), Trace);
        assert_eq!(log_level(Some("nonsense")), Warn);
    }

    #[test]
    fn log_path_uses_the_platform_state_dir() {
        let path = log_path();
        assert!(path.ends_with("jcode-desktop.log"));
        assert!(path.is_absolute(), "{}", path.display());
    }

    #[test]
    fn linux_stat_tick_fields_are_readable() {
        #[cfg(target_os = "linux")]
        assert!(super::read_process_ticks().is_some());
    }
}
