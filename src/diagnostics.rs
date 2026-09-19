//! Process-wide persistent diagnostics for startup failures and UI lag.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const HIGH_CPU_PERCENT: f64 = 50.0;

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
            loop {
                std::thread::sleep(SAMPLE_INTERVAL);
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

#[cfg(test)]
mod tests {
    #[test]
    fn linux_stat_tick_fields_are_readable() {
        #[cfg(target_os = "linux")]
        assert!(super::read_process_ticks().is_some());
    }
}
