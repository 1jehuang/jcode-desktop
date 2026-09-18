//! Personal alpha only. The fixed local helper owns AWS policy and readiness.
use super::{Command, Panel, harness};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
    updates: Vec<(String, bool, bool)>,
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

// No shell, PATH lookup, user-supplied executable, or stop action.
fn run_helper(action: &'static str) -> Result<String, String> {
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
    let deadline = Instant::now() + Duration::from_secs(360);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
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
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Cloud {action} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
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
        let startup = request_id.as_deref() == Some(Panel::STARTUP_SESSION_ID);
        self.state.lock().unwrap().updates.push((
            format!("Waking {HOST} before connecting…"),
            false,
            startup,
        ));
        std::thread::spawn(move || {
            let _serial = lifecycle.wake_lock.lock().unwrap();
            let result = wake_then_connect(&bridge, request_id, || run_helper("wake").map(|_| ()));
            let mut state = lifecycle.state.lock().unwrap();
            let (message, failed) = match result {
                Ok(()) => (format!("{HOST} is awake. Connecting…"), false),
                Err(error) => (
                    format!("{error} No local fallback. Retry Connect when ready."),
                    true,
                ),
            };
            state.error = failed.then(|| message.clone());
            state.updates.push((message, failed, startup));
            state.last_refresh = None;
        });
    }

    pub(super) fn take_updates(&self) -> Vec<(String, bool, bool)> {
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
