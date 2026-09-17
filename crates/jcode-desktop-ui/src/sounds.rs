//! Original, opt-in desktop earcons. No audio assets or audio runtime required.
//!
//! `App` also accepts deref-coerced GPUI entity contexts. The preference is
//! session-local here; callers own persistence. Config must default to disabled.
use gpui::{App, Global};
use std::{
    f32::consts::TAU,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    Sent,
    Complete,
    Attention,
    Error,
    BackgroundComplete,
    PanelOpen,
    PanelClose,
}

const SAMPLE_RATE: u32 = 24_000;
const COOLDOWN: Duration = Duration::from_millis(400);
// Reserve time for termination/reaping within the three-second playback budget.
const PLAYBACK_BUDGET: Duration = Duration::from_millis(2_800);

struct Sounds {
    enabled: bool,
    last_started: Option<Instant>,
    last_cue: Option<Cue>,
    busy: Arc<AtomicBool>,
    cancel_epoch: Arc<AtomicU64>,
    pending: Option<Cue>,
    servicing: bool,
    #[cfg(test)]
    requested: Vec<Cue>,
}
impl Global for Sounds {}
impl Default for Sounds {
    fn default() -> Self {
        Self {
            enabled: crate::config::get().sounds.enabled,
            last_started: None,
            last_cue: None,
            busy: Arc::new(AtomicBool::new(false)),
            cancel_epoch: Arc::new(AtomicU64::new(0)),
            pending: None,
            servicing: false,
            #[cfg(test)]
            requested: Vec::new(),
        }
    }
}

/// Current preference, not effective audibility (fixtures/tests always mute).
pub fn enabled(cx: &App) -> bool {
    cx.try_global::<Sounds>().map_or_else(
        || crate::config::get().sounds.enabled,
        |state| state.enabled,
    )
}

/// Set the in-memory preference. Does not alter the user's persisted config.
pub fn set_enabled(value: bool, cx: &mut App) {
    let state = cx.default_global::<Sounds>();
    state.enabled = value;
    if !value {
        state.pending = None;
        state.cancel_epoch.fetch_add(1, Ordering::AcqRel);
    }
    cx.refresh_windows();
}

/// Record intent independently of mute/gate state for panel wiring tests.
#[cfg(test)]
pub(crate) fn drain_requested(cx: &mut App) -> Vec<Cue> {
    std::mem::take(&mut cx.default_global::<Sounds>().requested)
}

fn muted() -> bool {
    cfg!(test)
        || crate::harness::screenshot_mode()
        || std::env::var_os("JCODE_DESKTOP_SCREENSHOT").is_some()
        || std::env::var_os("JCODE_DESKTOP_FIXTURE").is_some()
        || std::env::var_os("JCODE_DESKTOP_MUTE_SOUNDS").is_some()
}

/// Pure gate: rejected cues never extend the cooldown.
fn may_start(enabled: bool, muted: bool, busy: bool, elapsed: Option<Duration>) -> bool {
    enabled && !muted && !busy && elapsed.is_none_or(|elapsed| elapsed >= COOLDOWN)
}

/// Fire and forget one bounded playback operation, never a persistent thread.
/// All panels share the app-global gate. Missing/broken players fail silently.
pub fn play(cue: Cue, cx: &mut App) {
    #[cfg(test)]
    cx.default_global::<Sounds>().requested.push(cue);
    request(cue, cx);
}

fn priority(cue: Cue) -> u8 {
    match cue {
        Cue::Error | Cue::Attention => 3,
        Cue::Complete => 2,
        Cue::BackgroundComplete => 1,
        Cue::Sent | Cue::PanelOpen | Cue::PanelClose => 0,
    }
}

fn pending_cue(current: Option<Cue>, incoming: Cue) -> Option<Cue> {
    if priority(incoming) == 0 {
        return current;
    }
    match current {
        Some(old) if priority(old) >= priority(incoming) => Some(old),
        _ => Some(incoming),
    }
}

fn defer_cue(current: Option<Cue>, last: Option<Cue>, incoming: Cue) -> Option<Cue> {
    if last == Some(incoming) {
        current
    } else {
        pending_cue(current, incoming)
    }
}

fn request(cue: Cue, cx: &mut App) {
    let now = Instant::now();
    let state = cx.default_global::<Sounds>();
    if !state.enabled || muted() {
        state.pending = None;
        return;
    }
    if !may_start(
        state.enabled,
        muted(),
        state.busy.load(Ordering::Acquire),
        state
            .last_started
            .map(|last| now.saturating_duration_since(last)),
    ) {
        // Two windows watching the same session may report the same transition.
        // Coalesce identical cues during playback/cooldown rather than echoing it.
        state.pending = defer_cue(state.pending, state.last_cue, cue);
        if state.pending.is_some() && !state.servicing {
            state.servicing = true;
            service_pending(cx);
        }
        return;
    }
    let cue = state
        .pending
        .take()
        .and_then(|pending| pending_cue(Some(pending), cue))
        .unwrap_or(cue);
    state.last_started = Some(now);
    state.last_cue = Some(cue);
    state.busy.store(true, Ordering::Release);
    let permit = PlaybackPermit(state.busy.clone());
    let cancel_epoch = state.cancel_epoch.clone();
    let epoch = cancel_epoch.load(Ordering::Acquire);
    // The host deliberately retains UI dylibs until process exit (reload.rs).
    // The permit is captured before spawn so cancellation also releases the gate.
    cx.background_executor()
        .spawn(async move {
            let _permit = permit;
            play_blocking(cue, &cancel_epoch, epoch);
        })
        .detach();
}

fn service_pending(cx: &mut App) {
    cx.spawn(async move |cx| {
        // One foreground waiter per app, no thread and no unbounded backlog.
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
            let finished = cx.update(|cx| {
                let state = cx.default_global::<Sounds>();
                if !state.enabled || muted() || Instant::now() >= deadline {
                    state.pending = None;
                }
                if state.pending.is_none() {
                    state.servicing = false;
                    return true;
                }
                if !may_start(
                    state.enabled,
                    false,
                    state.busy.load(Ordering::Acquire),
                    state.last_started.map(|last| last.elapsed()),
                ) {
                    return false;
                }
                let cue = state.pending.take().unwrap();
                state.servicing = false;
                request(cue, cx);
                true
            });
            if finished {
                break;
            }
        }
    })
    .detach();
}

struct PlaybackPermit(Arc<AtomicBool>);
impl Drop for PlaybackPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct Tone {
    start: f32,
    duration: f32,
    from_hz: f32,
    to_hz: f32,
    gain: f32,
}

fn tones(cue: Cue) -> Vec<Tone> {
    let note = |start, duration, from_hz, to_hz, gain| Tone {
        start,
        duration,
        from_hz,
        to_hz,
        gain,
    };
    match cue {
        Cue::Sent => vec![note(0.0, 0.11, 520.0, 730.0, 0.075)],
        Cue::Complete => vec![
            note(0.0, 0.18, 523.25, 523.25, 0.105),
            note(0.12, 0.23, 783.99, 783.99, 0.09),
        ],
        Cue::Attention => vec![
            note(0.0, 0.13, 659.25, 659.25, 0.10),
            note(0.19, 0.17, 659.25, 659.25, 0.085),
        ],
        Cue::Error => vec![
            note(0.0, 0.17, 392.0, 349.23, 0.105),
            note(0.13, 0.22, 293.66, 261.63, 0.09),
        ],
        Cue::BackgroundComplete => vec![note(0.0, 0.24, 440.0, 554.37, 0.065)],
        Cue::PanelOpen => vec![note(0.0, 0.075, 600.0, 850.0, 0.035)],
        Cue::PanelClose => vec![note(0.0, 0.075, 650.0, 430.0, 0.030)],
    }
}

/// Deterministic mono, signed 16-bit little-endian PCM WAV, exposed to crate tests.
/// Sine/chirp voices with zero-ended sin² envelopes avoid clicks and harsh highs.
pub(crate) fn synthesize(cue: Cue) -> Vec<u8> {
    let tones = tones(cue);
    let duration = tones
        .iter()
        .map(|tone| tone.start + tone.duration)
        .fold(0.0_f32, f32::max);
    let samples = (duration * SAMPLE_RATE as f32).ceil() as usize + 1;
    let data_len = (samples * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes()); // block alignment
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in 0..samples {
        let time = sample as f32 / SAMPLE_RATE as f32;
        let mut value = 0.0;
        for tone in &tones {
            let t = time - tone.start;
            if !(0.0..tone.duration).contains(&t) {
                continue;
            }
            let fraction = t / tone.duration;
            let envelope = (std::f32::consts::PI * fraction).sin().powi(2);
            let phase = TAU * (tone.from_hz * t + (tone.to_hz - tone.from_hz) * t * fraction * 0.5);
            value += tone.gain * envelope * phase.sin();
        }
        let pcm = (value.clamp(-0.25, 0.25) * i16::MAX as f32).round() as i16;
        wav.extend_from_slice(&pcm.to_le_bytes());
    }
    wav
}

fn player_commands(path: &Path) -> Vec<Command> {
    #[cfg(target_os = "linux")]
    {
        ["pw-play", "paplay", "aplay"]
            .into_iter()
            .map(|program| {
                let mut command = Command::new(program);
                command.arg(path);
                command
            })
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("afplay");
        command.arg(path);
        vec![command]
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("powershell.exe");
        // Pass the path as data, never interpolate it into PowerShell source.
        command.args(["-NoProfile", "-NonInteractive", "-Command",
            "$p = New-Object System.Media.SoundPlayer; try { $p.SoundLocation = $env:JCODE_CUE_WAV; $p.Load(); $p.PlaySync() } finally { $p.Dispose() }"])
            .env("JCODE_CUE_WAV", path)
            .creation_flags(0x08000000); // CREATE_NO_WINDOW
        vec![command]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = path;
        Vec::new()
    }
}

fn play_blocking(cue: Cue, cancel_epoch: &AtomicU64, epoch: u64) {
    // Defense in depth: even direct internal calls cannot sound during tests.
    let cancelled = || cancel_epoch.load(Ordering::Acquire) != epoch || muted();
    if cancelled() {
        return;
    }
    let deadline = Instant::now() + PLAYBACK_BUDGET;
    // tempfile is an existing dependency. Private random file, retained until the
    // player has exited; closing the write handle permits Windows SoundPlayer.
    let Ok(mut file) = tempfile::Builder::new()
        .prefix("jcode-cue-")
        .suffix(".wav")
        .tempfile()
    else {
        return;
    };
    if file.write_all(&synthesize(cue)).is_err() || file.flush().is_err() {
        return;
    }
    let path = file.into_temp_path();
    for mut command in player_commands(&path) {
        if Instant::now() >= deadline || cancelled() {
            break;
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let Ok(mut child) = command.spawn() else {
            continue;
        };
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return;
                    }
                    break; // Failed backend: try next installed player.
                }
                Ok(None) if Instant::now() < deadline && !cancelled() => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait(); // Reap before releasing the private WAV.
                    return; // A hung player must not multiply the timeout.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_cross_window_cues_coalesce_without_losing_priority_alerts() {
        assert_eq!(defer_cue(None, Some(Cue::Complete), Cue::Complete), None);
        assert_eq!(
            defer_cue(None, Some(Cue::Sent), Cue::Error),
            Some(Cue::Error)
        );
        assert_eq!(
            defer_cue(Some(Cue::Error), Some(Cue::Complete), Cue::Complete),
            Some(Cue::Error)
        );
        assert_eq!(defer_cue(None, Some(Cue::PanelOpen), Cue::PanelClose), None);
    }
    #[gpui::test]
    fn preference_and_request_capture(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(enabled(cx), crate::config::get().sounds.enabled);
            set_enabled(false, cx);
            assert!(!enabled(cx));
            play(Cue::Sent, cx);
            set_enabled(true, cx);
            assert!(enabled(cx));
            play(Cue::Complete, cx);
            assert_eq!(drain_requested(cx), vec![Cue::Sent, Cue::Complete]);
            assert!(drain_requested(cx).is_empty());
            let state = cx.default_global::<Sounds>();
            assert!(state.last_started.is_none());
            assert!(!state.busy.load(Ordering::Acquire));
            state.pending = Some(Cue::Error);
            let epoch = state.cancel_epoch.load(Ordering::Acquire);
            set_enabled(false, cx);
            let state = cx.default_global::<Sounds>();
            assert!(state.pending.is_none());
            assert_ne!(state.cancel_epoch.load(Ordering::Acquire), epoch);
            set_enabled(true, cx);
            assert!(cx.default_global::<Sounds>().pending.is_none());
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_backend_order_and_literal_path() {
        let path = Path::new("/private/sound with 'quotes'.wav");
        let commands = player_commands(path);
        for (command, expected) in commands.iter().zip(["pw-play", "paplay", "aplay"]) {
            assert_eq!(command.get_program(), expected);
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                vec![path.as_os_str()]
            );
        }
        assert_eq!(commands.len(), 3);
    }
    const ALL: [Cue; 7] = [
        Cue::Sent,
        Cue::Complete,
        Cue::Attention,
        Cue::Error,
        Cue::BackgroundComplete,
        Cue::PanelOpen,
        Cue::PanelClose,
    ];
    fn pcm(wav: &[u8]) -> Vec<i16> {
        wav[44..]
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]))
            .collect()
    }
    #[test]
    fn wav_format_gain_duration_and_distinctness() {
        let waves: Vec<_> = ALL.into_iter().map(synthesize).collect();
        for (index, wav) in waves.iter().enumerate() {
            assert_eq!(&wav[..4], b"RIFF");
            assert_eq!(
                u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
                wav.len() - 8
            );
            assert_eq!(&wav[8..16], b"WAVEfmt ");
            assert_eq!(&wav[16..24], &[16, 0, 0, 0, 1, 0, 1, 0]);
            assert_eq!(
                u32::from_le_bytes(wav[24..28].try_into().unwrap()),
                SAMPLE_RATE
            );
            assert_eq!(
                u32::from_le_bytes(wav[28..32].try_into().unwrap()),
                SAMPLE_RATE * 2
            );
            assert_eq!(&wav[32..40], &[2, 0, 16, 0, b'd', b'a', b't', b'a']);
            assert_eq!(
                u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize,
                wav.len() - 44
            );
            let samples = pcm(wav);
            let duration = samples.len() as f32 / SAMPLE_RATE as f32;
            assert!((0.07..=0.37).contains(&duration));
            let peak = samples.iter().map(|s| (*s as i32).abs()).max().unwrap();
            assert!(peak > 500 && peak < 6000, "cue {:?}: {peak}", ALL[index]);
            assert_eq!(samples.first(), Some(&0));
            assert_eq!(samples.last(), Some(&0));
            assert!(
                samples
                    .windows(2)
                    .all(|s| (s[1] as i32 - s[0] as i32).abs() < 1000)
            );
            for other in &waves[..index] {
                assert_ne!(wav, other);
            }
            assert_eq!(*wav, synthesize(ALL[index]));
        }
    }
    #[test]
    fn panel_cues_are_softer_than_task_cues() {
        let rms = |cue| {
            let samples = pcm(&synthesize(cue));
            (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
        };
        for panel in [Cue::PanelOpen, Cue::PanelClose] {
            for task in [
                Cue::Sent,
                Cue::Complete,
                Cue::Attention,
                Cue::Error,
                Cue::BackgroundComplete,
            ] {
                assert!(rms(panel) < rms(task) * 0.65);
            }
        }
    }
    #[test]
    fn cooldown_and_overlap_gate() {
        assert!(may_start(true, false, false, None));
        assert!(!may_start(false, false, false, None));
        assert!(!may_start(true, true, false, None));
        assert!(!may_start(true, false, true, Some(Duration::from_secs(10))));
        assert!(!may_start(
            true,
            false,
            false,
            Some(COOLDOWN - Duration::from_nanos(1))
        ));
        assert!(may_start(true, false, false, Some(COOLDOWN)));
        assert!(may_start(
            true,
            false,
            false,
            Some(COOLDOWN + Duration::from_nanos(1))
        ));
    }

    #[gpui::test]
    fn reapplying_mute_clears_existing_state_and_cancels_playback(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            set_enabled(true, cx);
            let state = cx.default_global::<Sounds>();
            state.pending = Some(Cue::Complete);
            let cancellation = Arc::clone(&state.cancel_epoch);
            let previous_epoch = cancellation.load(Ordering::Acquire);

            set_enabled(false, cx);

            assert!(!enabled(cx));
            assert!(cx.global::<Sounds>().pending.is_none());
            assert!(cancellation.load(Ordering::Acquire) > previous_epoch);
        });
    }

    #[test]
    fn pending_slot_prioritizes_and_coalesces_storms() {
        assert_eq!(pending_cue(None, Cue::Sent), None);
        let mut pending = pending_cue(None, Cue::BackgroundComplete);
        pending = pending_cue(pending, Cue::Complete);
        assert_eq!(pending, Some(Cue::Complete));
        pending = pending_cue(pending, Cue::Error);
        for _ in 0..10_000 {
            for cue in ALL {
                pending = pending_cue(pending, cue);
            }
        }
        assert_eq!(pending, Some(Cue::Error));
        assert_eq!(
            pending_cue(Some(Cue::Attention), Cue::Complete),
            Some(Cue::Attention)
        );
    }
    #[test]
    fn permits_release_and_tests_are_silent() {
        let busy = Arc::new(AtomicBool::new(true));
        drop(PlaybackPermit(busy.clone()));
        assert!(!busy.load(Ordering::Acquire));
        assert!(muted());
        // Returns before file creation or launching any process.
        play_blocking(Cue::Complete, &AtomicU64::new(0), 0);
    }
}
