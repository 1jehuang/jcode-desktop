//! Personal alpha only. The fixed local helper owns AWS policy and readiness.
use super::{Command, harness};
use serde::Deserialize;
use std::{
    io::Read,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime},
};

pub(super) const HOST: &str = "jcode-cloud-alpha";
const CUTOFF: &str = "Two-hour continuous cutoff, including active work. Monthly exhaustion or idle shutdown can stop it earlier. Save work before shutdown.";
// Full-policy receipts are bounded proofs, never VM lease renewals.
// Only read-only background checks may renew the proof while a panel is live.
const READY_TTL: Duration = Duration::from_secs(30);
const REUSING_READY: &str = "Reusing recently verified cloud VM...";

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalIdentity {
    home: PathBuf,
    config: (u64, SystemTime),
    helper: (u64, SystemTime),
}

impl LocalIdentity {
    fn read(home: &Path) -> Option<Self> {
        let helper = helper_paths(home).ok()?;
        let stamp = |path: &Path| {
            let metadata = std::fs::metadata(path).ok()?;
            Some((metadata.len(), metadata.modified().ok()?))
        };
        Some(Self {
            home: home.into(),
            config: stamp(&home.join(".config/jcode/cloud-alpha.json"))?,
            helper: stamp(&helper)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Receipt {
    instance_id: String,
    state: String,
    launch_time: String,
    lease_deadline: u64,
    lease_remaining_seconds: u64,
    allowance_remaining_minutes: u64,
    observed_at: u64,
    valid_for_seconds: u64,
    #[serde(default)]
    model_sync_supported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Boot {
    instance_id: String,
    launch_time: String,
}

struct Ready {
    verified_at: Instant,
    expires_at: Instant,
    checked_wall: SystemTime,
    expires_wall: SystemTime,
    identity: LocalIdentity,
    boot: Boot,
    model_sync_supported: bool,
}

impl Receipt {
    fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("Invalid cloud readiness receipt: {e}"))
    }

    fn into_ready(
        self,
        identity: LocalIdentity,
        started: Instant,
        now: Instant,
        wall: SystemTime,
    ) -> Result<Ready, String> {
        let invalid = || "Cloud readiness receipt is stale or unsafe.".to_owned();
        let wall = wall
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| invalid())?;
        let observed = Duration::from_secs(self.observed_at);
        let age = wall.checked_sub(observed).ok_or_else(invalid)?;
        if self.state != "running"
            || self.instance_id.is_empty()
            || self.launch_time.is_empty()
            || self.valid_for_seconds == 0
            || self.valid_for_seconds > READY_TTL.as_secs()
            || self.allowance_remaining_minutes == 0
        {
            return Err(invalid());
        }
        let lifetime = Duration::from_secs(
            self.valid_for_seconds
                .min(self.lease_remaining_seconds)
                .min(self.lease_deadline.saturating_sub(self.observed_at))
                .min(self.allowance_remaining_minutes.saturating_mul(60)),
        );
        // Charge both network time and wall receipt age, never extend from use
        // or from the time the UI happens to consume a completed check.
        let remaining = lifetime
            .checked_sub(age.max(now.saturating_duration_since(started)))
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(invalid)?;
        Ok(Ready {
            verified_at: started,
            expires_at: now + remaining,
            checked_wall: SystemTime::UNIX_EPOCH + wall,
            expires_wall: SystemTime::UNIX_EPOCH + wall + remaining,
            identity,
            boot: Boot {
                instance_id: self.instance_id,
                launch_time: self.launch_time,
            },
            model_sync_supported: self.model_sync_supported,
        })
    }
}

#[derive(Clone)]
struct Check {
    generation: u64,
    active_epoch: u64,
    identity: LocalIdentity,
    boot: Boot,
    started: Instant,
}

#[derive(Default)]
struct State {
    status: Option<(Status, Instant)>,
    error: Option<String>,
    refreshing: bool,
    last_refresh: Option<Instant>,
    updates: Vec<(String, bool, Option<String>)>,
    ready: Option<Ready>,
    ready_generation: u64,
    waking: bool,
    active_panels: Vec<String>,
    active_epoch: u64,
    checking: bool,
    last_check: Option<Instant>,
    check_result: Option<(Check, Result<Receipt, String>)>,
}

impl State {
    fn invalidate_ready(&mut self) {
        self.ready = None;
        self.ready_generation = self.ready_generation.wrapping_add(1);
    }

    fn recently_ready(&self, identity: Option<&LocalIdentity>, now: Instant) -> bool {
        self.recently_ready_at(identity, now, SystemTime::now())
    }

    fn recently_ready_at(
        &self,
        identity: Option<&LocalIdentity>,
        now: Instant,
        wall: SystemTime,
    ) -> bool {
        let Some(ready) = &self.ready else {
            return false;
        };
        if identity != Some(&ready.identity)
            || now >= ready.expires_at
            || wall < ready.checked_wall
            || wall >= ready.expires_wall
        {
            return false;
        }
        // A status obtained before this wake may describe the previous stopped
        // boot. Still honor a known running lease's deadline as it elapses.
        self.status.as_ref().is_none_or(|(status, fetched)| {
            (*fetched < ready.verified_at && status.state != "running")
                || status.allows_ready_reuse(now.saturating_duration_since(*fetched))
        })
    }

    fn set_active(&mut self, mut panels: Vec<String>) {
        panels.sort();
        panels.dedup();
        if self.active_panels != panels {
            self.active_epoch = self.active_epoch.wrapping_add(1);
            if panels.is_empty() {
                // Closing an old panel fences its background checks, but is
                // not a failure of a distinct foreground connect in progress.
                if self.waking {
                    self.ready = None;
                } else {
                    self.invalidate_ready();
                }
            }
            self.active_panels = panels;
        }
    }

    fn begin_check(&mut self, identity: Option<LocalIdentity>, now: Instant) -> Option<Check> {
        if self.active_panels.is_empty()
            || self.checking
            || self
                .last_check
                .is_some_and(|last| now.saturating_duration_since(last) < Duration::from_secs(10))
        {
            return None;
        }
        let ready = self.ready.as_ref()?;
        if identity.as_ref() != Some(&ready.identity) {
            self.invalidate_ready();
            return None;
        }
        let check = Check {
            generation: self.ready_generation,
            active_epoch: self.active_epoch,
            identity: ready.identity.clone(),
            boot: ready.boot.clone(),
            started: now,
        };
        self.checking = true;
        self.last_check = Some(now);
        Some(check)
    }

    fn finish_check(
        &mut self,
        check: Check,
        result: Result<Receipt, String>,
        identity: Option<LocalIdentity>,
        now: Instant,
        wall: SystemTime,
    ) {
        self.checking = false;
        if self.ready_generation != check.generation
            || self.active_epoch != check.active_epoch
            || self.active_panels.is_empty()
        {
            return;
        }
        if identity.as_ref() != Some(&check.identity) {
            self.invalidate_ready();
            return;
        }
        let ready =
            result.and_then(|receipt| receipt.into_ready(check.identity, check.started, now, wall));
        match ready {
            Ok(ready)
                if ready.boot == check.boot
                    && self
                        .ready
                        .as_ref()
                        .is_some_and(|old| old.boot == check.boot) =>
            {
                self.ready = Some(ready)
            }
            _ => self.invalidate_ready(),
        }
    }

    fn record_status_reply(
        &mut self,
        generation: u64,
        result: Result<Status, String>,
        now: Instant,
    ) {
        if self.ready_generation == generation {
            self.record_status(result, now);
        }
    }

    fn record_status(&mut self, result: Result<Status, String>, now: Instant) {
        match result {
            Ok(status) => {
                if !self.waking && !status.allows_ready_reuse(Duration::ZERO) {
                    self.invalidate_ready();
                }
                self.status = Some((status, now));
                self.error = None;
            }
            Err(error) => {
                if !self.waking {
                    self.invalidate_ready();
                }
                self.error = Some(error);
            }
        }
    }
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
    depleted: bool,
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

// Keep the leader unreaped until group cancellation. Reaping it first could
// allow PID/PGID reuse while descendants still hold output pipes open.
struct HelperGroup(Option<std::process::Child>);
impl HelperGroup {
    fn exited(&self) -> std::io::Result<bool> {
        let child = self.0.as_ref().expect("owned helper");
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { info.si_pid() } != 0)
    }

    fn finish(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let mut child = self.0.take().expect("owned helper");
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        child.wait()
    }
}
impl Drop for HelperGroup {
    fn drop(&mut self) {
        if self.0.is_some() {
            let _ = self.finish();
        }
    }
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
        .process_group(0)
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
    let mut group = HelperGroup(Some(child));
    let deadline =
        Instant::now() + Duration::from_secs(if action == "check-ready" { 15 } else { 360 });
    loop {
        for line in progress_rx.try_iter().take(16) {
            if matches!(action, "wake" | "wake-ready") {
                progress(line);
            }
        }
        match group.exited() {
            Ok(true) => break,
            Ok(false) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100))
            }
            result => {
                return Err(match result {
                    Err(e) => format!("Cloud helper failed: {e}"),
                    _ => "Cloud helper timed out. Check cloud status before retrying.".into(),
                });
            }
        }
    }
    // Also bound the wait for EOF if a helper descendant inherited its pipes.
    let stdout = stdout_rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "Cloud helper output timed out.".to_owned())?;
    let stderr = stderr_rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "Cloud helper output timed out.".to_owned())??;
    for line in progress_rx.try_iter().take(16) {
        if matches!(action, "wake" | "wake-ready") {
            progress(line);
        }
    }
    let status = group
        .finish()
        .map_err(|e| format!("Cannot reap cloud helper: {e}"))?;
    if !status.success() {
        return Err(format!(
            "Cloud {action} failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    let output = String::from_utf8(stdout?).map_err(|e| e.to_string())?;
    Ok(output)
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

fn runtime_identity() -> Option<LocalIdentity> {
    if harness::screenshot_mode() || cfg!(test) {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    LocalIdentity::read(Path::new(&home))
}

impl Lifecycle {
    /// Called when a cloud transport fails. A concurrent wake may finish, but
    /// cannot publish a cache entry over a newer failure or status invalidation.
    pub(super) fn invalidate_ready(&self) {
        self.state.lock().unwrap().invalidate_ready();
    }

    fn sync_reused_ready(
        &self,
        reused: bool,
        sync: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if reused {
            if let Err(error) = sync() {
                self.invalidate_ready();
                return Err(error);
            }
        }
        Ok(())
    }

    fn ensure_ready(
        &self,
        identity: impl Fn() -> Option<LocalIdentity>,
        mut progress: impl FnMut(String),
        wake: impl FnOnce(&mut dyn FnMut(String)) -> Result<Receipt, String>,
    ) -> Result<bool, String> {
        // Check *inside* the lock: queued requests reuse the first successful
        // wake rather than serially rerunning the entire AWS readiness sequence.
        let _serial = self.wake_lock.lock().unwrap();
        let before = identity();
        let generation = {
            let mut state = self.state.lock().unwrap();
            if state.recently_ready(before.as_ref(), Instant::now()) {
                drop(state);
                progress(REUSING_READY.into());
                return Ok(true);
            }
            state.invalidate_ready();
            state.waking = true;
            state.ready_generation
        };
        let result = wake(&mut progress);
        let after = identity();
        let mut state = self.state.lock().unwrap();
        let result = result.and_then(|receipt| {
            if state.ready_generation != generation || before != after {
                return Err("Cloud readiness changed while waking. Retry Connect.".into());
            }
            let identity = after.ok_or("Cloud readiness identity unavailable.")?;
            receipt.into_ready(identity, Instant::now(), Instant::now(), SystemTime::now())
        });
        state.waking = false;
        match result {
            Ok(ready) => {
                state.ready_generation = state.ready_generation.wrapping_add(1);
                state.ready = Some(ready);
                Ok(false)
            }
            Err(error) => {
                state.invalidate_ready();
                Err(error)
            }
        }
    }

    // The UI supplies the current panel set and consumes results before any
    // publication, so a completed network worker cannot revive a closed panel.
    pub(super) fn refresh_active(&self, panels: Vec<String>) {
        let mut state = self.state.lock().unwrap();
        state.set_active(panels);
        let identity = runtime_identity();
        if let Some((check, result)) = state.check_result.take() {
            state.finish_check(
                check,
                result,
                identity.clone(),
                Instant::now(),
                SystemTime::now(),
            );
        }
        if harness::screenshot_mode() || cfg!(test) {
            return;
        }
        let Some(check) = state.begin_check(identity, Instant::now()) else {
            return;
        };
        let lifecycle = self.clone();
        std::thread::spawn(move || {
            let result = run_helper("check-ready").and_then(|json| Receipt::parse(&json));
            lifecycle.state.lock().unwrap().check_result = Some((check, result));
        });
    }

    pub(super) fn connect(&self, bridge: harness::Bridge, request_id: Option<String>) {
        let lifecycle = self.clone();
        let update_request_id = request_id.clone();
        self.state.lock().unwrap().updates.push((
            format!("Waiting for shared VM {HOST}…"),
            false,
            update_request_id.clone(),
        ));
        std::thread::spawn(move || {
            let mut reused = false;
            let result = wake_then_connect(&bridge, request_id, || {
                reused = lifecycle.ensure_ready(
                    runtime_identity,
                    |message| {
                        let mut state = lifecycle.state.lock().unwrap();
                        // Retain only the newest pending phase for this request.
                        // Never relabel a stale request as a newer panel's update.
                        state.updates.retain(|(_, _, id)| id != &update_request_id);
                        state
                            .updates
                            .push((message, false, update_request_id.clone()));
                    },
                    |progress| {
                        run_helper_with_progress("wake-ready", progress)
                            .and_then(|json| Receipt::parse(&json))
                    },
                )?;
                // Readiness is cached, model credentials are not. The helper
                // hashes allowlisted local inputs, avoiding AWS/SSH when fresh.
                let sync_supported = lifecycle
                    .state
                    .lock()
                    .unwrap()
                    .ready
                    .as_ref()
                    .is_some_and(|ready| ready.model_sync_supported);
                if reused && sync_supported {
                    if let Err(error) = run_helper("sync-models") {
                        lifecycle.invalidate_ready();
                        return Err(error);
                    }
                }
                Ok(())
            });
            let mut state = lifecycle.state.lock().unwrap();
            let (message, failed) = match result {
                Ok(()) if reused => (REUSING_READY.into(), false),
                Ok(()) => (format!("{HOST} is awake. Connecting…"), false),
                Err(error) => (
                    format!("{error} No local fallback. Retry Connect when ready."),
                    true,
                ),
            };
            state.error = failed.then(|| message.clone());
            state.updates.retain(|(_, _, id)| id != &update_request_id);
            state.updates.push((message, failed, update_request_id));
            if !reused {
                state.last_refresh = None;
            }
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
        let generation = state.ready_generation;
        state.last_refresh = Some(Instant::now());
        let lifecycle = self.clone();
        std::thread::spawn(move || {
            let result = run_helper("status").and_then(|json| {
                serde_json::from_str::<Status>(&json)
                    .map_err(|e| format!("Invalid cloud status: {e}"))
            });
            let mut state = lifecycle.state.lock().unwrap();
            state.refreshing = false;
            state.record_status_reply(generation, result, Instant::now());
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
    fn allows_ready_reuse(&self, elapsed: Duration) -> bool {
        self.state == "running"
            && !self.depleted
            && self.used_minutes < self.allowance_minutes
            && self.remaining_lease(elapsed) != Some(0)
    }

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

    fn test_receipt() -> Receipt {
        receipt_at(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
    }

    fn receipt_at(observed_at: u64) -> Receipt {
        Receipt {
            instance_id: "i-test".into(),
            state: "running".into(),
            launch_time: "boot-1".into(),
            lease_deadline: observed_at + 600,
            lease_remaining_seconds: 600,
            allowance_remaining_minutes: 100,
            observed_at,
            valid_for_seconds: 30,
            model_sync_supported: false,
        }
    }

    fn wall(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn active_state(now: Instant) -> State {
        let mut state = State::default();
        state.ready = Some(
            receipt_at(1000)
                .into_ready(test_identity(), now, now, wall(1000))
                .unwrap(),
        );
        state.set_active(vec!["ssh://jcode-cloud-alpha/one".into()]);
        state
    }

    #[test]
    fn cloud_background_checks_maintain_proof_beyond_35_and_65_seconds() {
        let start = Instant::now();
        let mut state = active_state(start);
        for seconds in (0..=70).step_by(10) {
            let now = start + Duration::from_secs(seconds);
            let check = state.begin_check(Some(test_identity()), now).unwrap();
            assert!(
                state
                    .begin_check(Some(test_identity()), now + Duration::from_secs(20))
                    .is_none()
            );
            state.finish_check(
                check,
                Ok(receipt_at(1000 + seconds)),
                Some(test_identity()),
                now,
                wall(1000 + seconds),
            );
            assert!(state.recently_ready_at(
                Some(&test_identity()),
                now + Duration::from_secs(5),
                wall(1005 + seconds)
            ));
            assert!(
                state
                    .begin_check(Some(test_identity()), now + Duration::from_secs(9))
                    .is_none()
            );
        }
        assert!(!state.recently_ready_at(
            Some(&test_identity()),
            start + Duration::from_secs(100),
            wall(1100)
        ));
    }

    #[test]
    fn cloud_receipt_capability_defaults_off_and_numeric_fields_are_strict() {
        let json = r#"{"instance_id":"i-test","state":"running","launch_time":"boot-1","lease_deadline":1600,"lease_remaining_seconds":600,"allowance_remaining_minutes":100,"observed_at":1000,"valid_for_seconds":30}"#;
        assert!(!Receipt::parse(json).unwrap().model_sync_supported);
        for invalid in [
            json.replace("\"observed_at\":1000", "\"observed_at\":1000.5"),
            json.replace("\"valid_for_seconds\":30", "\"valid_for_seconds\":-1"),
            json.replace("\"lease_deadline\":1600", "\"lease_deadline\":null"),
        ] {
            assert!(Receipt::parse(&invalid).is_err());
        }
    }

    #[test]
    fn cloud_receipt_rejects_malformed_stopped_exhausted_and_expired_proofs() {
        assert!(Receipt::parse("{}").is_err());
        assert!(Receipt::parse("not JSON").is_err());
        let start = Instant::now();
        for mutate in [
            (|r: &mut Receipt| r.state = "stopped".into()) as fn(&mut Receipt),
            |r| r.state = "stopping".into(),
            |r| r.lease_remaining_seconds = 0,
            |r| r.lease_deadline = r.observed_at,
            |r| r.allowance_remaining_minutes = 0,
            |r| r.valid_for_seconds = 31,
            |r| r.valid_for_seconds = 0,
            |r| r.instance_id.clear(),
            |r| r.launch_time.clear(),
            |r| r.observed_at = 1001,
        ] {
            let mut receipt = receipt_at(1000);
            mutate(&mut receipt);
            assert!(
                receipt
                    .into_ready(test_identity(), start, start, wall(1000))
                    .is_err()
            );
        }
        assert!(
            receipt_at(1000)
                .into_ready(test_identity(), start, start, wall(1030))
                .is_err()
        );
        assert!(
            receipt_at(1000)
                .into_ready(
                    test_identity(),
                    start,
                    start + Duration::from_secs(30),
                    wall(1000)
                )
                .is_err()
        );
        let delayed = receipt_at(1000)
            .into_ready(
                test_identity(),
                start,
                start + Duration::from_secs(14),
                wall(1002),
            )
            .unwrap();
        assert_eq!(delayed.expires_at, start + READY_TTL);
        let aged = receipt_at(1000)
            .into_ready(
                test_identity(),
                start,
                start + Duration::from_secs(2),
                wall(1014),
            )
            .unwrap();
        assert_eq!(aged.expires_at, start + Duration::from_secs(18));
        let mut short = receipt_at(1000);
        short.lease_remaining_seconds = 4;
        short.lease_deadline = 1003;
        assert_eq!(
            short
                .into_ready(test_identity(), start, start, wall(1000))
                .unwrap()
                .expires_at,
            start + Duration::from_secs(3)
        );
    }

    #[test]
    fn cloud_background_failure_guard_budget_lease_and_boot_change_invalidate() {
        let start = Instant::now();
        for mutate in [
            (|r: &mut Receipt| r.state = "stopped".into()) as fn(&mut Receipt),
            |r| r.allowance_remaining_minutes = 0,
            |r| r.lease_remaining_seconds = 0,
            |r| r.launch_time = "new-boot".into(),
            |r| r.instance_id = "retarget".into(),
        ] {
            let mut state = active_state(start);
            let check = state.begin_check(Some(test_identity()), start).unwrap();
            let mut receipt = receipt_at(1000);
            mutate(&mut receipt);
            state.finish_check(check, Ok(receipt), Some(test_identity()), start, wall(1000));
            assert!(state.ready.is_none());
        }
        for error in [
            "guard stale",
            "identity denied",
            "helper timeout",
            "malformed JSON",
        ] {
            let mut state = active_state(start);
            let check = state.begin_check(Some(test_identity()), start).unwrap();
            state.finish_check(
                check,
                Err(error.into()),
                Some(test_identity()),
                start,
                wall(1000),
            );
            assert!(state.ready.is_none());
            assert!(
                state
                    .begin_check(Some(test_identity()), start + Duration::from_secs(20))
                    .is_none()
            );
        }
    }

    #[test]
    fn cloud_late_background_replies_cannot_revive_close_invalidation_retarget_or_new_wake() {
        let start = Instant::now();
        for mutation in 0..5 {
            let mut state = active_state(start);
            let check = state.begin_check(Some(test_identity()), start).unwrap();
            match mutation {
                0 => state.set_active(vec![]),
                1 => state.invalidate_ready(),
                2 => state.set_active(vec!["ssh://jcode-cloud-alpha/replacement".into()]),
                3 | 4 => {
                    state.invalidate_ready();
                    let mut receipt = receipt_at(1001);
                    receipt.launch_time = "new-wake".into();
                    state.ready = Some(
                        receipt
                            .into_ready(test_identity(), start, start, wall(1001))
                            .unwrap(),
                    );
                }
                _ => unreachable!(),
            }
            let before = state.ready.as_ref().map(|r| (r.boot.clone(), r.expires_at));
            state.finish_check(
                check,
                if mutation == 4 {
                    Err("late failure".into())
                } else {
                    Ok(receipt_at(1002))
                },
                Some(test_identity()),
                start + Duration::from_secs(2),
                wall(1002),
            );
            assert_eq!(
                state.ready.as_ref().map(|r| (r.boot.clone(), r.expires_at)),
                before
            );
            assert!(!state.checking);
        }
        let mut state = active_state(start);
        let check = state.begin_check(Some(test_identity()), start).unwrap();
        let mut changed = test_identity();
        changed.config.0 += 1;
        state.finish_check(
            check,
            Ok(receipt_at(1000)),
            Some(changed),
            start,
            wall(1000),
        );
        assert!(state.ready.is_none());
    }

    #[test]
    fn cloud_background_never_starts_without_live_panel_or_wake_proof() {
        let start = Instant::now();
        let mut state = State::default();
        state.record_status(Ok(running_status()), start);
        state.set_active(vec!["connected".into()]);
        assert!(state.begin_check(Some(test_identity()), start).is_none());
        state = active_state(start);
        state.set_active(vec![]);
        assert!(state.begin_check(Some(test_identity()), start).is_none());
        state = active_state(start);
        assert!(state.begin_check(None, start).is_none());
        assert!(state.ready.is_none());
    }

    #[test]
    fn cloud_background_does_not_hold_foreground_wake_lock() {
        let lifecycle = Lifecycle::default();
        let start = Instant::now();
        *lifecycle.state.lock().unwrap() = active_state(start);
        lifecycle.state.lock().unwrap().ready = Some(
            test_receipt()
                .into_ready(test_identity(), start, start, SystemTime::now())
                .unwrap(),
        );
        let check = lifecycle
            .state
            .lock()
            .unwrap()
            .begin_check(Some(test_identity()), start)
            .unwrap();
        assert!(
            lifecycle
                .ensure_ready(
                    || Some(test_identity()),
                    |_| {},
                    |_| panic!("fresh proof should not wake")
                )
                .unwrap()
        );
        lifecycle.state.lock().unwrap().finish_check(
            check,
            Err("check failed".into()),
            Some(test_identity()),
            start,
            wall(1000),
        );
        assert!(lifecycle.state.lock().unwrap().ready.is_none());
    }

    #[test]
    fn cloud_offline_readiness_actions_are_denied_even_with_injected_proof() {
        for action in ["wake-ready", "check-ready"] {
            assert!(run_helper(action).unwrap_err().contains("disabled"));
        }
        assert!(runtime_identity().is_none());
        let lifecycle = Lifecycle::default();
        *lifecycle.state.lock().unwrap() = active_state(Instant::now());
        lifecycle.refresh_active(vec!["ssh://jcode-cloud-alpha/one".into()]);
        assert!(!lifecycle.state.lock().unwrap().checking);
        assert!(
            !lifecycle
                .state
                .lock()
                .unwrap()
                .recently_ready(runtime_identity().as_ref(), Instant::now())
        );
    }

    #[test]
    fn cloud_helper_group_cleanup_closes_descendant_pipes() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 60 & printf 'ready\\n'; wait"])
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        std::io::BufRead::read_line(&mut stdout, &mut line).unwrap();
        assert_eq!(line.trim(), "ready");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(drain_output(stdout, None));
        });
        drop(HelperGroup(Some(child)));
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
    }

    #[test]
    fn cloud_cold_wake_ignores_intermediate_status_but_not_transport_invalidation() {
        for result in [
            Ok(Status {
                state: "stopped".into(),
                ..running_status()
            }),
            Err("temporary status failure".into()),
        ] {
            let lifecycle = Lifecycle::default();
            let mut status_generation = 0;
            lifecycle
                .ensure_ready(
                    || Some(test_identity()),
                    |_| {},
                    |_| {
                        let mut state = lifecycle.state.lock().unwrap();
                        status_generation = state.ready_generation;
                        state.record_status(result, Instant::now());
                        Ok(test_receipt())
                    },
                )
                .unwrap();
            let mut state = lifecycle.state.lock().unwrap();
            assert!(state.recently_ready(Some(&test_identity()), Instant::now()));
            state.record_status_reply(
                status_generation,
                Err("late stopped-boot status".into()),
                Instant::now(),
            );
            assert!(state.recently_ready(Some(&test_identity()), Instant::now()));
        }
        let lifecycle = Lifecycle::default();
        assert!(
            lifecycle
                .ensure_ready(
                    || Some(test_identity()),
                    |_| {},
                    |_| {
                        lifecycle.invalidate_ready();
                        Ok(test_receipt())
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn cloud_exited_helper_retains_group_ownership_until_descendant_cleanup() {
        let unrelated = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 60"])
            .process_group(0)
            .spawn()
            .unwrap();
        let unrelated = HelperGroup(Some(unrelated));
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 60 & exit 0"])
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut group = HelperGroup(Some(child));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !group.exited().unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        // WNOWAIT leaves the same zombie available on repeated observations.
        assert!(group.exited().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(drain_output(stdout, None));
        });
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
        assert!(group.finish().unwrap().success());
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
        assert!(!unrelated.exited().unwrap());
        drop(unrelated);
    }

    #[test]
    fn cloud_proof_expires_across_suspend_or_wall_clock_rollback() {
        let now = Instant::now();
        let state = active_state(now);
        assert!(state.recently_ready_at(Some(&test_identity()), now, wall(1000)));
        assert!(!state.recently_ready_at(Some(&test_identity()), now, wall(1031)));
        assert!(!state.recently_ready_at(Some(&test_identity()), now, wall(999)));
    }

    #[test]
    fn cloud_last_old_panel_close_does_not_cancel_distinct_foreground_wake() {
        let lifecycle = Lifecycle::default();
        lifecycle
            .state
            .lock()
            .unwrap()
            .set_active(vec!["old".into()]);
        lifecycle
            .ensure_ready(
                || Some(test_identity()),
                |_| {},
                |_| {
                    lifecycle.state.lock().unwrap().set_active(vec![]);
                    Ok(test_receipt())
                },
            )
            .unwrap();
        assert!(
            lifecycle
                .state
                .lock()
                .unwrap()
                .recently_ready(Some(&test_identity()), Instant::now())
        );
    }

    fn test_identity() -> LocalIdentity {
        LocalIdentity {
            home: PathBuf::from("/offline-cloud-test"),
            config: (2, SystemTime::UNIX_EPOCH),
            helper: (10, SystemTime::UNIX_EPOCH),
        }
    }

    fn running_status() -> Status {
        Status {
            state: "running".into(),
            used_minutes: 10,
            allowance_minutes: 3000,
            maximum_continuous_hours: 2,
            depleted: false,
            lease_remaining_seconds: Some(600),
        }
    }

    #[test]
    fn cloud_warm_model_sync_failure_blocks_create_and_invalidates_readiness() {
        let lifecycle = Lifecycle::default();
        lifecycle
            .ensure_ready(|| Some(test_identity()), |_| {}, |_| Ok(test_receipt()))
            .unwrap();
        let (bridge, commands) = harness::spawn_recording();
        let result = wake_then_connect(&bridge, Some("sync-failure".into()), || {
            lifecycle.sync_reused_ready(true, || Err("model sync failed".into()))
        });
        assert_eq!(result.unwrap_err(), "model sync failed");
        assert!(commands.try_recv().is_err());
        assert!(lifecycle.state.lock().unwrap().ready.is_none());
    }

    #[test]
    fn cloud_cold_wake_does_not_duplicate_model_sync() {
        let lifecycle = Lifecycle::default();
        lifecycle
            .sync_reused_ready(false, || panic!("cold wake already synchronizes"))
            .unwrap();
        let calls = std::cell::Cell::new(0);
        lifecycle
            .sync_reused_ready(true, || {
                calls.set(calls.get() + 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn cloud_warm_cache_is_success_only_and_does_not_slide() {
        let lifecycle = Lifecycle::default();
        let wakes = std::cell::Cell::new(0);
        let mut phases = Vec::new();
        let mut first_verified = None;
        for expected_reuse in [false, true, true] {
            assert_eq!(
                lifecycle
                    .ensure_ready(
                        || Some(test_identity()),
                        |phase| phases.push(phase),
                        |_| {
                            wakes.set(wakes.get() + 1);
                            Ok(test_receipt())
                        },
                    )
                    .unwrap(),
                expected_reuse,
            );
            let verified = lifecycle
                .state
                .lock()
                .unwrap()
                .ready
                .as_ref()
                .unwrap()
                .verified_at;
            assert_eq!(*first_verified.get_or_insert(verified), verified);
        }
        assert_eq!(wakes.get(), 1);
        assert_eq!(phases, [REUSING_READY, REUSING_READY]);
        let verified = lifecycle
            .state
            .lock()
            .unwrap()
            .ready
            .as_ref()
            .unwrap()
            .verified_at;
        lifecycle
            .state
            .lock()
            .unwrap()
            .record_status(Ok(running_status()), Instant::now());
        assert_eq!(
            lifecycle
                .state
                .lock()
                .unwrap()
                .ready
                .as_ref()
                .unwrap()
                .verified_at,
            verified
        );
        let state = lifecycle.state.lock().unwrap();
        assert!(state.recently_ready(
            Some(&test_identity()),
            state.ready.as_ref().unwrap().expires_at - Duration::from_nanos(1)
        ));
        assert!(!state.recently_ready(Some(&test_identity()), verified + READY_TTL));
        drop(state);
        lifecycle
            .state
            .lock()
            .unwrap()
            .ready
            .as_mut()
            .unwrap()
            .expires_at = Instant::now();
        assert!(
            !lifecycle
                .ensure_ready(|| Some(test_identity()), |_| {}, |_| Ok(test_receipt()))
                .unwrap()
        );
    }

    #[test]
    fn cloud_warm_cache_refuses_failures_missing_identity_and_changed_files() {
        let lifecycle = Lifecycle::default();
        for _ in 0..2 {
            assert!(
                lifecycle
                    .ensure_ready(
                        || Some(test_identity()),
                        |_| {},
                        |_| Err("guard stale".into())
                    )
                    .is_err()
            );
            assert!(lifecycle.state.lock().unwrap().ready.is_none());
            assert!(
                lifecycle
                    .ensure_ready(|| None, |_| {}, |_| Ok(test_receipt()))
                    .is_err()
            );
            assert!(lifecycle.state.lock().unwrap().ready.is_none());
        }
        assert!(
            !lifecycle
                .ensure_ready(|| Some(test_identity()), |_| {}, |_| Ok(test_receipt()))
                .unwrap()
        );
        let mut changed = test_identity();
        changed.config.0 += 1;
        assert!(
            !lifecycle
                .ensure_ready(|| Some(changed.clone()), |_| {}, |_| Ok(test_receipt()))
                .unwrap()
        );
        changed.helper.0 += 1;
        assert!(
            !lifecycle
                .ensure_ready(|| Some(changed.clone()), |_| {}, |_| Ok(test_receipt()))
                .unwrap()
        );
        lifecycle.invalidate_ready();
        assert!(lifecycle.state.lock().unwrap().ready.is_none());
    }

    #[test]
    fn cloud_warm_cache_invalidates_on_status_failure_stops_budget_and_cutoff() {
        let lifecycle = Lifecycle::default();
        let outcomes = [
            Err("status unavailable".into()),
            Ok(Status {
                state: "stopped".into(),
                ..running_status()
            }),
            Ok(Status {
                state: "stopping".into(),
                ..running_status()
            }),
            Ok(Status {
                depleted: true,
                ..running_status()
            }),
            Ok(Status {
                used_minutes: 3000,
                ..running_status()
            }),
            Ok(Status {
                lease_remaining_seconds: Some(0),
                ..running_status()
            }),
        ];
        for result in outcomes {
            lifecycle
                .ensure_ready(|| Some(test_identity()), |_| {}, |_| Ok(test_receipt()))
                .unwrap();
            let mut state = lifecycle.state.lock().unwrap();
            assert!(state.ready.is_some());
            let generation = state.ready_generation;
            state.record_status_reply(generation, result, Instant::now());
            assert!(state.ready.is_none());
        }
    }

    #[test]
    fn cloud_warm_cache_honors_known_lease_without_treating_old_stop_as_current() {
        let now = Instant::now();
        let mut state = State::default();
        state.record_status(
            Ok(Status {
                lease_remaining_seconds: Some(2),
                ..running_status()
            }),
            now,
        );
        state.ready = Some(Ready {
            verified_at: now + Duration::from_millis(1),
            identity: test_identity(),
            expires_at: now + READY_TTL,
            checked_wall: SystemTime::now(),
            expires_wall: SystemTime::now() + READY_TTL,
            boot: Boot {
                instance_id: "i-test".into(),
                launch_time: "boot-1".into(),
            },
            model_sync_supported: false,
        });
        assert!(state.recently_ready(Some(&test_identity()), now + Duration::from_secs(1)));
        assert!(!state.recently_ready(Some(&test_identity()), now + Duration::from_secs(2)));
        state.status.as_mut().unwrap().0.state = "stopped".into();
        assert!(state.recently_ready(Some(&test_identity()), now + Duration::from_secs(2)));
        // Reuse did not refresh status, lease, or wake timestamps.
        assert_eq!(state.status.as_ref().unwrap().1, now);
        assert_eq!(
            state.status.as_ref().unwrap().0.lease_remaining_seconds,
            Some(2)
        );
    }

    #[test]
    fn cloud_warm_cache_invalidation_during_wake_is_not_overwritten() {
        let lifecycle = Lifecycle::default();
        lifecycle
            .ensure_ready(
                || Some(test_identity()),
                |_| {},
                |_| {
                    lifecycle.invalidate_ready();
                    Ok(test_receipt())
                },
            )
            .unwrap_err();
        assert!(lifecycle.state.lock().unwrap().ready.is_none());
        let changed = std::cell::Cell::new(false);
        lifecycle
            .ensure_ready(
                || {
                    let mut identity = test_identity();
                    identity.config.0 += u64::from(changed.get());
                    Some(identity)
                },
                |_| {},
                |_| {
                    changed.set(true);
                    Ok(test_receipt())
                },
            )
            .unwrap_err();
        assert!(lifecycle.state.lock().unwrap().ready.is_none());
    }

    #[test]
    fn cloud_concurrent_warm_requests_share_wake_but_keep_distinct_creates() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let lifecycle = Lifecycle::default();
        let (bridge, commands) = harness::spawn_recording();
        let wakes = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|id| {
                let lifecycle = lifecycle.clone();
                let bridge = bridge.clone();
                let wakes = wakes.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    wake_then_connect(&bridge, Some(id.into()), || {
                        lifecycle
                            .ensure_ready(
                                || Some(test_identity()),
                                |_| {},
                                |_| {
                                    wakes.fetch_add(1, Ordering::SeqCst);
                                    Ok(test_receipt())
                                },
                            )
                            .map(|_| ())
                    })
                    .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        let mut ids: Vec<_> = commands
            .try_iter()
            .map(|command| match command {
                Command::CreateRemoteSession {
                    host,
                    request_id: Some(id),
                    ..
                } => {
                    assert_eq!(host, HOST);
                    id
                }
                _ => panic!("expected distinct remote create, never a local fallback"),
            })
            .collect();
        ids.sort();
        assert_eq!(ids, ["first", "second"]);
    }

    #[test]
    fn cloud_local_identity_requires_files_and_detects_metadata_change() {
        let home = tempfile::tempdir().unwrap();
        assert!(LocalIdentity::read(home.path()).is_none());
        let config = home.path().join(".config/jcode/cloud-alpha.json");
        let helper = home.path().join(".local/bin/jcode-cloud-alpha");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();
        std::fs::write(&helper, "helper").unwrap();
        let original = LocalIdentity::read(home.path()).unwrap();
        std::fs::write(&config, "{\"profile\":\"other\"}").unwrap();
        let changed = LocalIdentity::read(home.path()).unwrap();
        assert_ne!(original, changed);
        std::fs::write(&helper, "changed helper").unwrap();
        assert_ne!(changed, LocalIdentity::read(home.path()).unwrap());
        std::fs::remove_file(helper).unwrap();
        assert!(LocalIdentity::read(home.path()).is_none());
    }

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
        // Even a populated cache cannot bypass the offline operation guard.
        lifecycle.state.lock().unwrap().ready = Some(Ready {
            verified_at: Instant::now(),
            identity: test_identity(),
            expires_at: Instant::now() + READY_TTL,
            checked_wall: SystemTime::now(),
            expires_wall: SystemTime::now() + READY_TTL,
            boot: Boot {
                instance_id: "i-test".into(),
                launch_time: "boot-1".into(),
            },
            model_sync_supported: false,
        });
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
                depleted: false,
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
