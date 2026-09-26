//! Global hold while a Jcode CLI (TUI) terminal is focused.
//!
//! Desktop records the hold, shows the same non-focusing OS pill, and hands
//! the final transcript to that CLI session through the shared server, exactly
//! like `jcode transcript --session <id>`. The CLI then submits it as a prompt.
//! Any other focused app keeps the existing behavior (a new voice window).
use super::*;
use jcode_base::voice::{self, NariEvent, NariRecording, VoiceError};
use std::sync::atomic::Ordering;

const METER: Duration = Duration::from_millis(50);
const RESULT_VISIBLE: Duration = Duration::from_secs(4);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Recording,
    Transcribing,
    Sending,
    Done,
}

pub(super) struct CliCapture {
    session_id: String,
    attempt: Arc<AtomicBool>,
    phase: Phase,
    recording: Option<NariRecording>,
    levels: [f32; 24],
    last_level: Instant,
    result: Option<String>,
    finished_at: Option<Instant>,
    send_task: Option<Task<()>>,
}

impl Drop for CliCapture {
    fn drop(&mut self) {
        self.attempt.store(true, Ordering::SeqCst);
    }
}

impl CliCapture {
    pub(super) fn recording(&self) -> bool {
        matches!(self.phase, Phase::Recording | Phase::Transcribing)
    }

    fn snapshot(&self) -> Snapshot {
        let (title, decided) = match self.phase {
            Phase::Recording => ("Listening".to_string(), false),
            Phase::Transcribing => ("Transcribing…".to_string(), false),
            Phase::Sending => ("Sending to CLI…".to_string(), false),
            Phase::Done => (self.result.clone().unwrap_or_default(), true),
        };
        Snapshot {
            title,
            levels: (self.phase == Phase::Recording).then_some(self.levels),
            decided,
        }
    }

    fn finish(&mut self, result: impl Into<String>) {
        self.recording = None;
        self.phase = Phase::Done;
        self.result = Some(result.into());
        self.finished_at = Some(Instant::now());
    }
}

/// Short OS-pill label for a failure. Never includes transcript content.
fn failure_label(error: &VoiceError) -> String {
    match error {
        VoiceError::NariNotConfigured => "Voice not configured".into(),
        other => {
            let text = other.to_string();
            if text.chars().count() > 28 {
                "Voice failed".into()
            } else {
                text
            }
        }
    }
}

impl Workspace {
    /// Unfocused press: target the focused Jcode CLI if there is one,
    /// otherwise open a new voice window as before. Resolution runs off the UI
    /// thread (it asks the compositor and reads /proc).
    pub(super) fn global_press_unfocused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.global_voice.held = true;
        self.global_voice.press_serial = self.global_voice.press_serial.wrapping_add(1);
        let serial = self.global_voice.press_serial;
        let resolve = cx.background_executor().spawn(async {
            let session = jcode_base::dictation::focused_cli_session().ok().flatten()?;
            let allowed = crate::global_voice_session::permitted().await;
            Some((session, allowed))
        });
        self.global_voice.cli_resolve = Some(cx.spawn_in(window, async move |this, cx| {
            let result = resolve.await;
            let _ = this.update_in(cx, |this, _window, cx| {
                if this.global_voice.press_serial != serial {
                    return;
                }
                this.global_voice.cli_resolve = None;
                match result {
                    None => {
                        this.global_voice.held = false;
                        spawn_voice_window();
                    }
                    Some((_, false)) => {
                        eprintln!("global voice: denied by session check");
                        this.global_voice.held = false;
                    }
                    // A release before resolution was a tap too short to record.
                    Some((session, true)) if this.global_voice.held => {
                        this.start_cli_capture(session, cx)
                    }
                    Some(_) => {}
                }
            });
        }));
    }

    fn start_cli_capture(&mut self, session_id: String, cx: &mut Context<Self>) {
        // Replaces any lingering result pill from an earlier hold.
        self.global_voice.cli = None;
        self.global_voice.close_overlay(cx);
        let attempt = Arc::new(AtomicBool::new(false));
        let mut capture = CliCapture {
            session_id,
            attempt: attempt.clone(),
            phase: Phase::Recording,
            recording: None,
            levels: [0.; 24],
            last_level: Instant::now(),
            result: None,
            finished_at: None,
            send_task: None,
        };
        // Prove a visible indicator exists before opening the microphone.
        if !self.show_global_voice_overlay(capture.snapshot(), cx) {
            return;
        }
        voice::timing::begin();
        match voice::nari_api_key()
            .ok_or(VoiceError::NariNotConfigured)
            .and_then(|key| NariRecording::start_cancellable(attempt, &key))
        {
            Ok(recording) => {
                eprintln!("global voice: recording for a focused Jcode CLI");
                capture.recording = Some(recording);
            }
            Err(error) => {
                eprintln!("global voice: recording failed to start: {error}");
                capture.finish(failure_label(&error));
            }
        }
        self.global_voice.cli = Some(capture);
    }

    /// Physical key-up. Stops the microphone and waits for the final text.
    pub(super) fn cli_release(&mut self) {
        self.global_voice.held = false;
        if let Some(capture) = self.global_voice.cli.as_mut()
            && capture.phase == Phase::Recording
        {
            if let Some(recording) = capture.recording.as_ref() {
                voice::timing::release();
                recording.stop();
            }
            capture.phase = Phase::Transcribing;
        }
    }

    /// Drive the CLI capture and its OS pill. Returns whether one is active,
    /// in which case it owns the overlay this poll.
    pub(super) fn poll_cli_capture(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(capture) = self.global_voice.cli.as_mut() else {
            return false;
        };
        if capture.phase == Phase::Recording && capture.last_level.elapsed() >= METER {
            capture.last_level = Instant::now();
            let level = capture
                .recording
                .as_ref()
                .map_or(0., NariRecording::audio_level);
            capture.levels.rotate_left(1);
            capture.levels[23] = level;
        }
        let mut finished = None;
        if let Some(recording) = capture.recording.as_ref() {
            let mut drain = || {
                while let Some(event) = recording.try_event() {
                    if let NariEvent::Finished(result) = event {
                        return Some(result);
                    }
                }
                None
            };
            finished = drain();
            if finished.is_none() && recording.is_finished() {
                // Re-check after observing worker completion so a final event
                // published just after the first drain is not lost.
                finished = Some(drain().unwrap_or(Err(VoiceError::CaptureFailed)));
            }
        }
        match finished {
            Some(Ok(text)) if !text.trim().is_empty() => {
                capture.recording = None;
                capture.phase = Phase::Sending;
                let session = capture.session_id.clone();
                let attempt = capture.attempt.clone();
                // Length only. Never log transcript content.
                eprintln!(
                    "global voice: sending {} transcript chars to a Jcode CLI",
                    text.chars().count()
                );
                let send = cx.background_executor().spawn(async move {
                    if !crate::global_voice_session::permitted().await {
                        anyhow::bail!("denied by session check");
                    }
                    jcode_base::dictation::send_transcript_blocking(
                        text.trim(),
                        jcode_base::protocol::TranscriptMode::Send,
                        Some(session),
                        SEND_TIMEOUT,
                    )
                });
                capture.send_task = Some(cx.spawn(async move |this, cx| {
                    let result = send.await;
                    let _ = this.update(cx, |this, cx| {
                        let Some(capture) = this.global_voice.cli.as_mut() else {
                            return;
                        };
                        if !Arc::ptr_eq(&capture.attempt, &attempt) {
                            return;
                        }
                        match result {
                            Ok(()) => capture.finish("Sent to CLI"),
                            Err(error) => {
                                eprintln!("global voice: CLI delivery failed: {error:#}");
                                capture.finish("Couldn't reach CLI");
                            }
                        }
                        cx.notify();
                    });
                }));
            }
            Some(Ok(_)) => capture.finish("No speech detected"),
            Some(Err(error)) => {
                eprintln!("global voice: finished without text: {error}");
                capture.finish(failure_label(&error));
            }
            None => {}
        }
        if capture
            .finished_at
            .is_some_and(|at| at.elapsed() >= RESULT_VISIBLE)
        {
            self.global_voice.cli = None;
            self.global_voice.hide_overlay(cx);
            return true;
        }
        let snapshot = capture.snapshot();
        let shown = match self.global_voice.overlay {
            Some(handle) => handle
                .update(cx, |overlay, _, cx| overlay.set_snapshot(snapshot, cx))
                .is_ok(),
            None => self.show_global_voice_overlay(snapshot, cx),
        };
        if !shown {
            // Never keep recording without a visible indicator.
            self.global_voice.cli = None;
            self.global_voice.hide_overlay(cx);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(phase: Phase) -> CliCapture {
        CliCapture {
            session_id: "session_fox_1".into(),
            attempt: Arc::new(AtomicBool::new(false)),
            phase,
            recording: None,
            levels: [0.25; 24],
            last_level: Instant::now(),
            result: None,
            finished_at: None,
            send_task: None,
        }
    }

    #[test]
    fn cli_pill_shows_levels_only_while_listening_and_result_as_decided() {
        let listening = capture(Phase::Recording).snapshot();
        assert_eq!(listening.title, "Listening");
        assert!(listening.levels.is_some() && !listening.decided);
        let sending = capture(Phase::Sending).snapshot();
        assert_eq!(sending.title, "Sending to CLI…");
        assert!(sending.levels.is_none());
        let mut done = capture(Phase::Recording);
        done.finish("Sent to CLI");
        let snapshot = done.snapshot();
        assert_eq!(snapshot.title, "Sent to CLI");
        assert!(snapshot.decided && done.finished_at.is_some());
    }

    #[test]
    fn dropping_a_cli_capture_cancels_its_attempt() {
        let capture = capture(Phase::Recording);
        let attempt = capture.attempt.clone();
        drop(capture);
        assert!(attempt.load(Ordering::SeqCst));
    }

    #[test]
    fn failure_labels_fit_the_pill() {
        assert_eq!(
            failure_label(&VoiceError::NariNotConfigured),
            "Voice not configured"
        );
        assert!(failure_label(&VoiceError::CaptureFailed).chars().count() <= 28);
    }
}
