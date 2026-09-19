//! Personal alpha only. The fixed local helper owns AWS policy and readiness.
use super::{Command, harness};
use serde::Deserialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

pub(super) const HOST: &str = "jcode-cloud-alpha";
const CUTOFF: &str = "Two-hour continuous cutoff, including active work. Monthly exhaustion or idle shutdown can stop it earlier. Save work before shutdown.";

#[derive(Default)]
struct State {
    status: Option<(Status, Instant)>,
    error: Option<String>,
    refreshing: bool,
    last_refresh: Option<Instant>,
    updates: Vec<(String, bool, Option<String>)>,
}

#[derive(Clone, Default)]
pub(super) struct Lifecycle {
    state: Arc<Mutex<State>>,
    // Concurrent new-panel requests retain their individual request IDs, but
    // never race two wake operations against the configured instance.
    wake_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Deserialize)]
struct Status {
    state: String,
    used_minutes: u64,
    allowance_minutes: u64,
    maximum_continuous_hours: u64,
    #[serde(default)]
    lease_remaining_seconds: Option<u64>,
}

fn helper_paths(home: &Path) -> Result<PathBuf, String> {
    if !home.join(".config/jcode/cloud-alpha.json").is_file() {
        return Err("Personal cloud is not configured: ~/.config/jcode/cloud-alpha.json is required. No local session was opened.".into());
    }
    let helper = home.join(".local/bin/jcode-cloud-alpha");
    if !helper.is_file() {
        return Err("Personal cloud helper is missing: ~/.local/bin/jcode-cloud-alpha".into());
    }
    Ok(helper)
}

const OUTPUT_LIMIT: usize = 64 * 1024;
const PROGRESS_LINE_LIMIT: usize = 512;

// Drain even after reaching the retained-output limit, so a noisy helper cannot
// block on a full pipe. Progress uses a bounded channel and never blocks reading.
fn drain_output(
    mut reader: impl Read,
    progress: Option<mpsc::SyncSender<String>>,
) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut line = Vec::new();
    let mut overflow = false;
    let mut buffer = [0; 4096];
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Cannot read cloud helper output: {error}")),
        };
        let retain = count.min(OUTPUT_LIMIT - output.len());
        output.extend_from_slice(&buffer[..retain]);
        overflow |= retain < count;
        if let Some(progress) = &progress {
            for byte in &buffer[..count] {
                if matches!(byte, b'\n' | b'\r') {
                    send_progress(progress, &mut line);
                } else if line.len() < PROGRESS_LINE_LIMIT {
                    line.push(*byte);
                }
            }
        }
    }
    if let Some(progress) = &progress {
        send_progress(progress, &mut line);
    }
    // Status JSON must never be silently truncated and treated as complete.
    if overflow && progress.is_none() {
        Err("Cloud helper output exceeded the safety limit.".into())
    } else {
        Ok(output)
    }
}

fn send_progress(sender: &mpsc::SyncSender<String>, line: &mut Vec<u8>) {
    let text: String = String::from_utf8_lossy(line)
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if !text.trim().is_empty() {
        let _ = sender.try_send(text.trim().to_owned());
    }
    line.clear();
}

// No shell, PATH lookup, user-supplied executable, or stop action.
fn run_helper(action: &'static str) -> Result<String, String> {
    run_helper_with_progress(action, |_| {})
}

fn run_helper_with_progress(
    action: &'static str,
    mut progress: impl FnMut(String),
) -> Result<String, String> {
    if harness::screenshot_mode() || cfg!(test) {
        return Err("Cloud operations are disabled in offline tests and screenshots.".into());
    }
    let home = std::env::var_os("HOME").ok_or("Cannot find home directory")?;
    let helper = helper_paths(Path::new(&home))?;
    let mut child = std::process::Command::new(helper)
        .arg(action)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot run cloud helper: {e}"))?;
    let (progress_tx, progress_rx) = mpsc::sync_channel(16);
    let (stdout_tx, stdout_rx) = mpsc::sync_channel(1);
    let (stderr_tx, stderr_rx) = mpsc::sync_channel(1);
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        let _ = stdout_tx.send(drain_output(stdout, None));
    });
    std::thread::spawn(move || {
        let _ = stderr_tx.send(drain_output(stderr, Some(progress_tx)));
    });
    let deadline = Instant::now() + Duration::from_secs(360);
    let status = loop {
        for line in progress_rx.try_iter().take(16) {
            if action == "wake" {
                progress(line);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => format!("Cloud helper failed: {e}"),
                    _ => "Cloud helper timed out. Check cloud status before retrying.".into(),
                });
            }
        }
    };
    // Also bound the wait for EOF if a helper descendant inherited its pipes.
    let stdout = stdout_rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "Cloud helper output timed out.".to_owned())?;
    let stderr = stderr_rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "Cloud helper output timed out.".to_owned())??;
    for line in progress_rx.try_iter().take(16) {
        if action == "wake" {
            progress(line);
        }
    }
    if !status.success() {
        return Err(format!(
            "Cloud {action} failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    String::from_utf8(stdout?).map_err(|e| e.to_string())
}

fn wake_then_connect(
    bridge: &harness::Bridge,
    request_id: Option<String>,
    wake: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    wake()?;
    bridge.send(Command::CreateRemoteSession {
        host: HOST.into(),
        working_dir: None,
        request_id,
    });
    Ok(())
}

impl Lifecycle {
    pub(super) fn connect(&self, bridge: harness::Bridge, request_id: Option<String>) {
        let lifecycle = self.clone();
        let update_request_id = request_id.clone();
        self.state.lock().unwrap().updates.push((
            format!("Waiting for shared VM {HOST}…"),
            false,
            update_request_id.clone(),
        ));
        std::thread::spawn(move || {
            let _serial = lifecycle.wake_lock.lock().unwrap();
            let result = wake_then_connect(&bridge, request_id, || {
                run_helper_with_progress("wake", |message| {
                    let mut state = lifecycle.state.lock().unwrap();
                    // Retain only the newest pending phase for this request.
                    // Never relabel a stale request as a newer panel's update.
                    state.updates.retain(|(_, _, id)| id != &update_request_id);
                    state
                        .updates
                        .push((message, false, update_request_id.clone()));
                })
                .map(|_| ())
            });
            let mut state = lifecycle.state.lock().unwrap();
            let (message, failed) = match result {
                Ok(()) => (format!("{HOST} is awake. Connecting…"), false),
                Err(error) => (
                    format!("{error} No local fallback. Retry Connect when ready."),
                    true,
                ),
            };
            state.error = failed.then(|| message.clone());
            state.updates.retain(|(_, _, id)| id != &update_request_id);
            state.updates.push((message, failed, update_request_id));
            state.last_refresh = None;
        });
    }

    pub(super) fn take_updates(&self) -> Vec<(String, bool, Option<String>)> {
        std::mem::take(&mut self.state.lock().unwrap().updates)
    }

    pub(super) fn refresh(&self) {
        let mut state = self.state.lock().unwrap();
        if state.refreshing
            || state
                .last_refresh
                .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        state.refreshing = true;
        state.last_refresh = Some(Instant::now());
        let lifecycle = self.clone();
        std::thread::spawn(move || {
            let result = run_helper("status").and_then(|json| {
                serde_json::from_str::<Status>(&json)
                    .map_err(|e| format!("Invalid cloud status: {e}"))
            });
            let mut state = lifecycle.state.lock().unwrap();
            state.refreshing = false;
            match result {
                Ok(status) => {
                    state.status = Some((status, Instant::now()));
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
        });
    }

    /// Kept in the persistent Machines switcher, so active work gets a warning
    /// without opening the management panel. This never requests a stop.
    pub(super) fn lease_warning(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        let (status, fetched) = state.status.as_ref()?;
        if status.state != "running" {
            return None;
        }
        let monthly = status.allowance_minutes.saturating_sub(status.used_minutes);
        if monthly <= 10 {
            return Some(format!("Cloud allowance ≤{monthly}m. Save work!"));
        }
        let seconds = status.remaining_lease(fetched.elapsed())?;
        (seconds <= 600).then(|| format!("Cloud cutoff ≤{}m. Save work!", seconds.div_ceil(60)))
    }

    pub(super) fn summary(&self) -> String {
        let state = self.state.lock().unwrap();
        let mut text = state
            .status
            .as_ref()
            .map(|(status, fetched)| status.summary(fetched.elapsed()))
            .unwrap_or_else(|| format!("Cloud status not yet available. {CUTOFF}"));
        if let Some(error) = &state.error {
            text.push_str(&format!(" Status unavailable: {error}"));
        }
        text
    }
}

impl Status {
    fn remaining_lease(&self, elapsed: Duration) -> Option<u64> {
        self.lease_remaining_seconds
            .filter(|_| self.state == "running")
            .map(|seconds| seconds.saturating_sub(elapsed.as_secs()))
    }

    fn summary(&self, elapsed: Duration) -> String {
        let remaining = self.allowance_minutes.saturating_sub(self.used_minutes);
        let mut text = format!(
            "{} · Monthly remaining: {}h {}m of {}h {}m (last checked). Maximum continuous lease: {}h. {CUTOFF}",
            self.state,
            remaining / 60,
            remaining % 60,
            self.allowance_minutes / 60,
            self.allowance_minutes % 60,
            self.maximum_continuous_hours
        );
        if let Some(seconds) = self.remaining_lease(elapsed) {
            text.push_str(&format!(
                " Two-hour cutoff in ≤{}m.{}",
                seconds.div_ceil(60),
                if seconds <= 600 {
                    " Warning: lease ends soon and active agents will stop."
                } else {
                    ""
                }
            ));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_progress_is_streamed_before_eof() {
        use std::io::Write;
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::sync_channel(16);
        let drain = std::thread::spawn(move || drain_output(reader, Some(tx)));
        writer
            .write_all(b"Checking budget\nWaiting for VM\n")
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "Checking budget"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "Waiting for VM"
        );
        // The writer remains open when both progress messages arrive.
        drop(writer);
        assert_eq!(
            drain.join().unwrap().unwrap(),
            b"Checking budget\nWaiting for VM\n"
        );
    }

    #[test]
    fn cloud_output_and_progress_are_bounded_without_blocking_drain() {
        let mut input = vec![b'x'; OUTPUT_LIMIT * 3];
        input.extend_from_slice(b"\n\0Done\t\rLast phase");
        let (tx, rx) = mpsc::sync_channel(1);
        let output = drain_output(input.as_slice(), Some(tx)).unwrap();
        assert_eq!(output.len(), OUTPUT_LIMIT);
        let line = rx.recv().unwrap();
        assert_eq!(line.len(), PROGRESS_LINE_LIMIT);
        assert!(rx.try_recv().is_err());
        assert!(
            drain_output(input.as_slice(), None)
                .unwrap_err()
                .contains("safety limit")
        );
    }

    #[test]
    fn cloud_progress_filters_controls_and_flushes_final_line() {
        let (tx, rx) = mpsc::sync_channel(16);
        drain_output(&b"\0Checking\t\r\nReady"[..], Some(tx)).unwrap();
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), ["Checking", "Ready"]);
    }

    #[test]
    fn cloud_status_output_remains_exact_json() {
        let json = br#"{"state":"running","used_minutes":1}"#;
        assert_eq!(drain_output(&json[..], None).unwrap(), json);
    }

    #[test]
    fn cloud_test_guard_prevents_helper_and_progress_callback() {
        assert!(
            run_helper_with_progress("wake", |_| panic!("must not run"))
                .unwrap_err()
                .contains("disabled")
        );
        assert!(run_helper("status").unwrap_err().contains("disabled"));
    }

    #[test]
    fn cloud_initial_and_failure_updates_preserve_independent_request_ids() {
        let lifecycle = Lifecycle::default();
        let (bridge, commands) = harness::spawn_recording();
        let serial = lifecycle.wake_lock.lock().unwrap();
        lifecycle.connect(bridge.clone(), Some("panel-old".into()));
        lifecycle.connect(bridge.clone(), Some("panel-new".into()));
        let initial = lifecycle.take_updates();
        assert_eq!(initial.len(), 2);
        assert!(
            initial
                .iter()
                .all(|(text, failed, _)| text.contains("shared VM") && !failed)
        );
        assert_eq!(initial[0].2.as_deref(), Some("panel-old"));
        assert_eq!(initial[1].2.as_deref(), Some("panel-new"));
        drop(serial);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut final_updates = Vec::new();
        while final_updates.len() < 2 && Instant::now() < deadline {
            final_updates.extend(lifecycle.take_updates());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(final_updates.len(), 2);
        assert!(final_updates.iter().all(|(_, failed, _)| *failed));
        let mut ids: Vec<_> = final_updates
            .into_iter()
            .map(|(_, _, id)| id.unwrap())
            .collect();
        ids.sort();
        assert_eq!(ids, ["panel-new", "panel-old"]);
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn cloud_wake_failure_never_sends_remote_or_local_command() {
        let (bridge, commands) = harness::spawn_recording();
        assert!(
            wake_then_connect(&bridge, Some("startup".into()), || Err("exhausted".into())).is_err()
        );
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn cloud_wake_precedes_remote_creation_and_preserves_request() {
        let (bridge, commands) = harness::spawn_recording();
        wake_then_connect(&bridge, Some("startup".into()), || {
            assert!(commands.try_recv().is_err());
            Ok(())
        })
        .unwrap();
        assert!(
            matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {host, working_dir: None, request_id: Some(id)}) if host == HOST && id == "startup")
        );
    }

    #[test]
    fn cloud_config_is_required_before_helper_resolution() {
        let home = tempfile::tempdir().unwrap();
        assert!(
            helper_paths(home.path())
                .unwrap_err()
                .contains("cloud-alpha.json")
        );
        std::fs::create_dir_all(home.path().join(".config/jcode")).unwrap();
        std::fs::write(home.path().join(".config/jcode/cloud-alpha.json"), "{}").unwrap();
        assert!(
            helper_paths(home.path())
                .unwrap_err()
                .contains("helper is missing")
        );
    }

    #[test]
    fn cloud_lease_warning_is_available_with_management_panel_closed() {
        let lifecycle = Lifecycle::default();
        assert!(lifecycle.lease_warning().is_none());
        lifecycle.state.lock().unwrap().status = Some((
            Status {
                state: "running".into(),
                used_minutes: 10,
                allowance_minutes: 3000,
                maximum_continuous_hours: 2,
                lease_remaining_seconds: Some(599),
            },
            Instant::now(),
        ));
        assert!(lifecycle.lease_warning().unwrap().contains("Save work!"));
        lifecycle
            .state
            .lock()
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .0
            .state = "stopped".into();
        assert!(lifecycle.lease_warning().is_none());
    }

    #[test]
    fn cloud_status_supports_old_helper_and_warns_at_cutoff() {
        let old: Status = serde_json::from_str(r#"{"state":"running","used_minutes":65,"allowance_minutes":120,"maximum_continuous_hours":2}"#).unwrap();
        assert!(
            old.summary(Duration::ZERO)
                .contains("Monthly remaining: 0h 55m")
        );
        assert!(
            old.summary(Duration::ZERO)
                .contains("including active work")
        );
        let status = Status {
            lease_remaining_seconds: Some(650),
            ..old
        };
        assert!(!status.summary(Duration::ZERO).contains("Warning:"));
        assert!(status.summary(Duration::from_secs(60)).contains("Warning:"));
        assert!(
            status
                .summary(Duration::from_secs(700))
                .contains("cutoff in ≤0m")
        );
        let depleted = Status {
            used_minutes: 999,
            ..status
        };
        assert!(
            depleted
                .summary(Duration::ZERO)
                .contains("Monthly remaining: 0h 0m")
        );
    }
}
