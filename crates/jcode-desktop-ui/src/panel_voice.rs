//! Explicit Nari streaming dictation. Audio and interim text are never snapshotted.
use super::*;
use jcode_base::voice::{self, NariEvent, NariRecording, VoiceError};
use jcode_base::voice_intent::{self, SessionCandidate, VoiceIntent};
use std::sync::atomic::{AtomicBool, Ordering};

gpui::actions!(panel_voice, [ToggleVoice]);

pub(crate) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([gpui::KeyBinding::new(
        "ctrl-shift-v",
        ToggleVoice,
        Some("ChatPanel"),
    )]);
}

#[path = "panel_voice_overlay.rs"]
mod overlay;

#[cfg(test)]
#[path = "panel_global_voice_tests.rs"]
mod global_tests;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const VOICE_SHORTCUT: &str = "Copilot key";
// Registered globally by the host (RegisterHotKey), so it works unfocused too.
#[cfg(target_os = "windows")]
const VOICE_SHORTCUT: &str = "Ctrl+Shift+Space";
#[cfg(target_os = "macos")]
const VOICE_SHORTCUT: &str = "⌘⇧M (Command+Shift+M)";

/// Compact keycap shown inside the voice pill so the shortcut is visible
/// without hovering. Linux draws the Copilot key glyph instead of text.
#[cfg(target_os = "windows")]
const VOICE_SHORTCUT_KEYCAP: &str = "Ctrl+Shift+Space";
#[cfg(target_os = "macos")]
const VOICE_SHORTCUT_KEYCAP: &str = "⌘⇧M";

/// Linux: a small outlined keycap holding the Copilot key glyph.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn voice_shortcut_keycap(color: gpui::Hsla, _theme: &Theme) -> gpui::AnyElement {
    div()
        .debug_selector(|| "voice-shortcut".into())
        .flex_none()
        .size(px(15.))
        .rounded(px(3.5))
        .border_1()
        .border_color(color.opacity(0.7))
        .flex()
        .items_center()
        .justify_center()
        .child(
            gpui::svg()
                .debug_selector(|| "voice-shortcut-copilot-icon".into())
                .data(include_bytes!("../../../assets/icons/copilot.svg") as &'static [u8])
                .text_color(color)
                .size(px(9.)),
        )
        .into_any_element()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn voice_shortcut_keycap(color: gpui::Hsla, theme: &Theme) -> gpui::AnyElement {
    div()
        .debug_selector(|| "voice-shortcut".into())
        .flex_none()
        .whitespace_nowrap()
        .text_size(px(10.5))
        .font_family(theme.FONT_MONO)
        .text_color(color)
        .child(VOICE_SHORTCUT_KEYCAP)
        .into_any_element()
}

fn voice_tooltip(action: &str, status: &str) -> String {
    format!(
        "{action} · {VOICE_SHORTCUT}\nHold {VOICE_SHORTCUT} to transcribe, release to finish. {status}. Ctrl+Shift+V toggles recording in the composer. Audio streams to Nari. After transcription, Jev chooses a coding agent or a quick navigation action. Uncertain requests stay in the draft."
    )
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Recording,
    Transcribing,
    Routing,
}

/// Ephemeral routing evidence. Never included in persisted panel snapshots.
#[derive(Clone)]
pub(crate) struct VoiceTrace {
    transcript: String,
    candidates: Vec<SessionCandidate>,
    questions: Vec<voice_intent::VoiceQuestion>,
    answers: Vec<voice_intent::VoiceAnswer>,
    /// Streamed microphone audio, the Nari billing unit. Measured locally.
    audio: Option<Duration>,
    /// Provider-reported Jev usage. `None` while routing or when unreported.
    usage: Option<voice_intent::VoiceUsage>,
}

#[derive(Default)]
pub(super) struct VoiceState {
    phase: Phase,
    // Attempt ownership persists through final transcription, even after key release.
    hold_capture: bool,
    // Unfocused capture driven by the global hold. It routes through Jev like a
    // focused hold, but its status and Jev's decision are mirrored in the OS pill.
    global_capture: bool,
    recording: Option<NariRecording>,
    live_transcript: String,
    levels: [f32; 24],
    canceled: Arc<AtomicBool>,
    started: Option<Instant>,
    // Audio length at stop, so final transcription latency is not billed as audio.
    audio: Option<Duration>,
    error: Option<String>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
    sessions: Option<Vec<jcode_sdk::SessionInfo>>,
    navigation: Option<gpui::Subscription>,
    actions: Option<gpui::Subscription>,
    decision: Option<String>,
    trace: Option<VoiceTrace>,
    trace_expanded: bool,
    trace_preview: bool,
}

pub(crate) struct VoiceSessionRequested(pub jcode_sdk::SessionInfo, pub String);
impl gpui::EventEmitter<VoiceSessionRequested> for Panel {}

pub(crate) struct VoiceActionRequested(pub voice_intent::QuickAction, pub String);
impl gpui::EventEmitter<VoiceActionRequested> for Panel {}

impl Drop for VoiceState {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::SeqCst);
        if let Some(recording) = self.recording.take() {
            // NariRecording signals cancellation without joining the native worker.
            drop(recording);
        }
    }
}

struct VoiceTooltip(String);
impl Render for VoiceTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "voice-shortcut-tooltip".into())
            .max_w(px(300.))
            .p_2()
            .rounded_md()
            .bg(Theme::global().HEADER_BG)
            .text_size(px(12.))
            .text_color(Theme::global().TEXT)
            .child(self.0.clone())
    }
}

/// Compact OS-pill label for Jev's decision, e.g. "Jev → Coding agent".
fn global_pill_decision(decision: &str) -> String {
    let decision = decision.strip_prefix("Jev chose: ").unwrap_or(decision);
    let (route, detail) = decision.split_once(" · ").unwrap_or((decision, ""));
    match route {
        "Quick action" if !detail.is_empty() && detail != "Navigation" => format!("Jev → {detail}"),
        route => format!("Jev → {route}"),
    }
}

/// Compact OS-pill label for a finished attempt that did not act.
fn global_pill_error(error: &str) -> String {
    if error.starts_with("Jev is unsure") {
        "Jev unsure · Kept in draft".into()
    } else if error.starts_with("Jev could not match") {
        "No matching session · Kept in draft".into()
    } else if error.starts_with("Jev routing unavailable") {
        "Jev unavailable · Kept in draft".into()
    } else if error.contains("Navigation unavailable") {
        "Jev → Quick action · Kept in draft".into()
    } else {
        error.to_string()
    }
}

impl Panel {
    #[cfg(test)]
    pub(crate) fn resolve_voice_for_test(
        &mut self,
        text: &str,
        result: anyhow::Result<VoiceIntent>,
        cx: &mut Context<Self>,
    ) {
        self.voice.phase = Phase::Routing;
        self.voice.live_transcript = text.into();
        let attempt = self.voice.canceled.clone();
        self.finish_voice_routing(&attempt, result, cx);
    }

    pub(crate) fn configure_voice_navigation(
        &mut self,
        sessions: Vec<jcode_sdk::SessionInfo>,
        subscription: gpui::Subscription,
    ) {
        self.voice.sessions = Some(sessions);
        self.voice.navigation = Some(subscription);
    }

    pub(crate) fn configure_voice_actions(&mut self, subscription: gpui::Subscription) {
        self.voice.actions = Some(subscription);
    }

    pub(crate) fn show_voice_decision(&mut self, message: String, cx: &mut Context<Self>) {
        self.voice.error = None;
        self.voice.decision = Some(message);
        cx.notify();
    }

    /// Refine the decision shown by an unfocused hold's OS pill. Focused
    /// holds already show it on the destination panel.
    pub(crate) fn set_global_voice_decision(&mut self, message: String, cx: &mut Context<Self>) {
        if self.voice.global_capture && self.voice.error.is_none() {
            self.voice.decision = Some(message);
            cx.notify();
        }
    }

    pub(crate) fn voice_trace(&self) -> Option<VoiceTrace> {
        self.voice.trace.clone()
    }

    pub(crate) fn show_voice_trace(&mut self, trace: Option<VoiceTrace>, cx: &mut Context<Self>) {
        self.voice.trace = trace;
        self.voice.trace_expanded = false;
        self.voice.trace_preview = false;
        cx.notify();
    }

    pub(crate) fn keep_voice_draft(&mut self, text: &str, cx: &mut Context<Self>) {
        self.voice.decision = None;
        self.voice.error = Some(
            "Jev chose: Quick action · Navigation unavailable. Transcript kept in the draft."
                .into(),
        );
        self.input
            .update(cx, |input, cx| input.append_dictation(text, cx));
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn voice_decision_for_test(&self) -> Option<&str> {
        self.voice.decision.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn set_voice_checking_for_test(&mut self) {
        self.voice = VoiceState::default();
        self.voice.phase = Phase::Checking;
    }

    #[cfg(test)]
    pub(crate) fn set_voice_hold_checking_for_test(&mut self) {
        self.prepare_voice_attempt(true);
    }

    pub(crate) fn supports_voice(&self) -> bool {
        (!self.session_id.contains("://")
            || self.is_pending_session()
            || self.session_id.starts_with("preview://"))
            && self.terminal.is_none()
            && self.code_file.is_none()
            && !self.is_side_document()
            && self.gmail_inbox.is_none()
            && self.gmail_message.is_none()
            && self.todoist.is_none()
    }

    pub(crate) fn voice_active(&self) -> bool {
        self.voice.phase != Phase::Idle
    }

    pub(crate) fn global_voice_snapshot(
        &self,
        attempt: &Arc<AtomicBool>,
    ) -> Option<crate::global_voice_overlay::Snapshot> {
        if !Arc::ptr_eq(attempt, &self.voice.canceled) || !self.voice.global_capture {
            return None;
        }
        use crate::global_voice_overlay::Snapshot;
        let (title, decided) = match self.voice.phase {
            Phase::Idle => match (&self.voice.error, &self.voice.decision) {
                (Some(error), _) => (global_pill_error(error), true),
                (None, Some(decision)) => (global_pill_decision(decision), true),
                (None, None) => ("Added to draft".into(), true),
            },
            Phase::Checking => ("Connecting…".into(), false),
            Phase::Recording => ("Listening".into(), false),
            Phase::Transcribing => ("Transcribing…".into(), false),
            Phase::Routing => ("Jev is choosing…".into(), false),
        };
        Some(Snapshot {
            title,
            levels: (self.voice.phase == Phase::Recording).then_some(self.voice.levels),
            decided,
        })
    }

    pub(crate) fn cancel_global_voice(
        &mut self,
        attempt: &Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) {
        if Arc::ptr_eq(attempt, &self.voice.canceled) && self.voice.global_capture {
            self.cancel_voice(cx);
        }
    }

    pub(crate) fn begin_voice_hold(&mut self, cx: &mut Context<Self>) {
        if self.supports_voice() && self.voice.phase == Phase::Idle {
            self.start_voice_with_hold(true, cx);
        }
    }

    pub(crate) fn begin_global_voice_hold(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Arc<AtomicBool>> {
        if self.supports_voice() && !self.voice_active() {
            self.start_voice_with_hold(true, cx);
            self.voice.global_capture = true;
            return Some(self.voice.canceled.clone());
        }
        None
    }

    pub(crate) fn end_global_voice_hold(
        &mut self,
        attempt: &Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) {
        if Arc::ptr_eq(attempt, &self.voice.canceled) && self.voice.global_capture {
            self.end_voice_hold(cx);
        }
    }

    pub(crate) fn end_voice_hold(&mut self, cx: &mut Context<Self>) {
        if !self.voice.hold_capture {
            return;
        }
        match self.voice.phase {
            Phase::Checking => self.cancel_voice(cx),
            Phase::Recording => self.stop_voice(cx),
            Phase::Idle | Phase::Transcribing | Phase::Routing => {}
        }
    }

    pub(crate) fn toggle_voice(&mut self, cx: &mut Context<Self>) {
        if !self.supports_voice() {
            return;
        }
        match self.voice.phase {
            Phase::Idle => self.start_voice(cx),
            Phase::Recording => self.stop_voice(cx),
            Phase::Checking | Phase::Transcribing | Phase::Routing => self.cancel_voice(cx),
        }
    }

    fn cancel_voice(&mut self, cx: &mut Context<Self>) {
        self.reset_voice_attempt();
        cx.notify();
    }

    fn reset_voice_attempt(&mut self) {
        // Session configuration belongs to the panel, not an individual capture.
        let sessions = self.voice.sessions.take();
        let navigation = self.voice.navigation.take();
        let actions = self.voice.actions.take();
        self.voice = VoiceState::default();
        self.voice.actions = actions;
        self.voice.sessions = sessions;
        self.voice.navigation = navigation;
    }

    fn prepare_voice_attempt(&mut self, hold_capture: bool) {
        self.reset_voice_attempt();
        self.voice.phase = Phase::Checking;
        self.voice.hold_capture = hold_capture;
    }

    fn start_voice(&mut self, cx: &mut Context<Self>) {
        self.start_voice_with_hold(false, cx);
    }

    fn start_voice_with_hold(&mut self, hold_capture: bool, cx: &mut Context<Self>) {
        if self.preview_state.is_some() || crate::harness::screenshot_mode() {
            self.voice.error = Some("Microphone access is disabled in offline previews.".into());
            cx.notify();
            return;
        }
        if self.voice.phase != Phase::Idle {
            return;
        }
        jcode_base::voice::timing::begin();
        self.prepare_voice_attempt(hold_capture);
        let token = self.voice.canceled.clone();
        let attempt = token.clone();
        let work = cx.background_executor().spawn(async move {
            let key = voice::nari_api_key().ok_or(VoiceError::NariNotConfigured)?;
            NariRecording::start_cancellable(token, &key)
        });
        self.voice.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |panel, cx| {
                panel.finish_voice_startup(&attempt, result, cx);
            });
        }));
        cx.notify();
    }

    fn finish_voice_startup(
        &mut self,
        attempt: &Arc<AtomicBool>,
        result: Result<NariRecording, VoiceError>,
        cx: &mut Context<Self>,
    ) {
        if !Arc::ptr_eq(attempt, &self.voice.canceled)
            || attempt.load(Ordering::SeqCst)
            || self.voice.phase != Phase::Checking
        {
            // Dropping a late successful recording cancels its native worker.
            return;
        }
        match result {
            Ok(recording) => {
                jcode_base::voice::timing::mark("ui shows recording");
                if self.voice.global_capture {
                    eprintln!("global voice: recording started");
                }
                self.voice.phase = Phase::Recording;
                self.voice.recording = Some(recording);
                self.voice.started = Some(Instant::now());
                self.tick_voice(cx);
            }
            Err(error) => {
                if self.voice.global_capture {
                    eprintln!("global voice: recording failed to start: {error}");
                }
                self.voice.phase = Phase::Idle;
                self.voice.hold_capture = false;
                self.voice.error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    fn tick_voice(&mut self, cx: &mut Context<Self>) {
        let attempt = self.voice.canceled.clone();
        self.voice.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let keep_ticking = this
                    .update(cx, |panel, cx| {
                        if !Arc::ptr_eq(&attempt, &panel.voice.canceled) || !panel.voice_active() {
                            return false;
                        }
                        if panel.voice.phase == Phase::Recording {
                            let level = panel
                                .voice
                                .recording
                                .as_ref()
                                .map_or(0., NariRecording::audio_level);
                            panel.voice.levels.rotate_left(1);
                            panel.voice.levels[23] = level;
                        }
                        // Drain a bounded batch so a fast provider cannot monopolize the UI.
                        for _ in 0..64 {
                            let event = panel
                                .voice
                                .recording
                                .as_ref()
                                .and_then(NariRecording::try_event);
                            let Some(event) = event else { break };
                            panel.apply_voice_event(event, cx);
                            if !panel.voice_active() || panel.voice.phase == Phase::Routing {
                                return false;
                            }
                        }
                        if panel
                            .voice
                            .recording
                            .as_ref()
                            .is_some_and(NariRecording::is_finished)
                        {
                            // Re-check after observing worker completion so a final event
                            // published just after the first drain is not lost.
                            while let Some(event) = panel
                                .voice
                                .recording
                                .as_ref()
                                .and_then(NariRecording::try_event)
                            {
                                panel.apply_voice_event(event, cx);
                                if !panel.voice_active() || panel.voice.phase == Phase::Routing {
                                    return false;
                                }
                            }
                            panel.finish_voice(Err(VoiceError::CaptureFailed), cx);
                            return false;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_ticking {
                    break;
                }
            }
        }));
    }

    fn apply_voice_event(&mut self, event: NariEvent, cx: &mut Context<Self>) {
        match event {
            NariEvent::Started => {}
            NariEvent::Transcript(text) => {
                // This separate preview can be revised without replacing anything the
                // user types. Only Finished inserts one undoable edit into the draft.
                self.voice.live_transcript = text;
                cx.notify();
            }
            NariEvent::Finished(result) => self.finish_voice(result, cx),
        }
    }

    fn stop_voice(&mut self, cx: &mut Context<Self>) {
        let Some(recording) = self.voice.recording.as_ref() else {
            return;
        };
        recording.stop();
        self.voice.audio = self.voice.started.map(|started| started.elapsed());
        self.voice.phase = Phase::Transcribing;
        cx.notify();
    }

    fn finish_voice(&mut self, result: Result<String, VoiceError>, cx: &mut Context<Self>) {
        self.voice.hold_capture = false;
        self.voice.phase = Phase::Idle;
        let audio = self
            .voice
            .audio
            .take()
            .or_else(|| self.voice.started.map(|started| started.elapsed()));
        self.voice.started = None;
        self.voice.recording = None;
        self.voice.live_transcript.clear();
        match result {
            Ok(text) if !text.trim().is_empty() => {
                if self.voice.global_capture {
                    self.voice.phase = Phase::Transcribing;
                    let attempt = self.voice.canceled.clone();
                    let check = cx
                        .background_executor()
                        .spawn(crate::global_voice_session::permitted());
                    self.voice.task = Some(cx.spawn(async move |this, cx| {
                        let allowed = check.await;
                        let _ = this.update(cx, |panel, cx| {
                            panel.finish_global_voice_text(&attempt, text, audio, allowed, cx);
                        });
                    }));
                    cx.notify();
                    return;
                }
                if self.voice.sessions.is_some() {
                    self.route_voice(text, audio, cx);
                    return;
                }
                self.input.update(cx, |input, cx| {
                    input.append_dictation(&text, cx);
                });
                self.voice.error = None;
            }
            Ok(_) => self.voice.error = Some("No speech was detected. Try recording again.".into()),
            Err(error) => self.voice.error = Some(error.to_string()),
        }
        if self.voice.global_capture {
            if let Some(error) = &self.voice.error {
                eprintln!("global voice: finished without text: {error}");
            }
        }
        cx.notify();
    }

    fn finish_global_voice_text(
        &mut self,
        attempt: &Arc<AtomicBool>,
        text: String,
        audio: Option<Duration>,
        allowed: bool,
        cx: &mut Context<Self>,
    ) {
        if !Arc::ptr_eq(attempt, &self.voice.canceled)
            || attempt.load(Ordering::SeqCst)
            || !self.voice.global_capture
            || self.voice.phase != Phase::Transcribing
        {
            return;
        }
        if allowed && self.voice.sessions.is_some() {
            // Length only. Never log transcript content.
            eprintln!(
                "global voice: routing {} transcript chars through Jev",
                text.chars().count()
            );
            self.route_voice(text, audio, cx);
        } else if allowed {
            eprintln!(
                "global voice: inserted {} transcript chars",
                text.chars().count()
            );
            self.input
                .update(cx, |input, cx| input.append_dictation(&text, cx));
            self.voice.phase = Phase::Idle;
            self.voice.error = None;
            cx.notify();
        } else {
            self.cancel_global_voice(attempt, cx);
        }
    }

    fn route_voice(&mut self, text: String, audio: Option<Duration>, cx: &mut Context<Self>) {
        self.voice.phase = Phase::Routing;
        self.voice.live_transcript = text.clone();
        self.voice.error = None;
        let attempt = self.voice.canceled.clone();
        let candidates = self
            .voice
            .sessions
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|s| SessionCandidate {
                id: s.session_id.clone(),
                title: match jcode_core::id::extract_session_name(&s.session_id) {
                    Some(name) => format!("{name}: {}", s.title.as_deref().unwrap_or_default()),
                    None => s.title.clone().unwrap_or_default(),
                },
                working_dir: s.working_dir.clone(),
            })
            .collect::<Vec<_>>();
        let questions = match voice_intent::describe_questions(&text, &candidates) {
            Ok(questions) => questions,
            Err(error) => {
                self.finish_voice_routing(&attempt, Err(error), cx);
                return;
            }
        };
        self.voice.trace = Some(VoiceTrace {
            transcript: text.clone(),
            candidates: candidates.clone(),
            questions,
            answers: Vec::new(),
            audio,
            usage: None,
        });
        let work = cx.background_executor().spawn(async move {
            tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)?
                .block_on(voice_intent::classify_with_report(&text, &candidates))
        });
        self.voice.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |panel, cx| {
                panel.finish_voice_report(&attempt, result, cx);
            });
        }));
        cx.notify();
    }

    fn finish_voice_report(
        &mut self,
        attempt: &Arc<AtomicBool>,
        result: anyhow::Result<voice_intent::VoiceClassification>,
        cx: &mut Context<Self>,
    ) {
        // A canceled or superseded request cannot overwrite the visible evidence.
        if !Arc::ptr_eq(attempt, &self.voice.canceled)
            || attempt.load(Ordering::SeqCst)
            || self.voice.phase != Phase::Routing
        {
            return;
        }
        let intent = result.map(|report| {
            if let Some(trace) = &mut self.voice.trace {
                trace.answers = report.answers;
                trace.usage = report.usage;
            }
            report.intent
        });
        self.finish_voice_routing(attempt, intent, cx);
    }

    fn finish_voice_routing(
        &mut self,
        attempt: &Arc<AtomicBool>,
        result: anyhow::Result<VoiceIntent>,
        cx: &mut Context<Self>,
    ) {
        if !Arc::ptr_eq(attempt, &self.voice.canceled)
            || attempt.load(Ordering::SeqCst)
            || self.voice.phase != Phase::Routing
        {
            return;
        }
        self.voice.phase = Phase::Idle;
        let mut target = None;
        let mut quick_action = None;
        let mut coding_agent = false;
        let error = match result {
            Ok(VoiceIntent::CodingAgent) => {
                coding_agent = true;
                None
            }
            Ok(VoiceIntent::QuickAction(action)) => {
                if self.voice.actions.is_some() {
                    quick_action = Some(action);
                    None
                } else {
                    Some("Jev chose: Quick action · Navigation unavailable. Transcript kept in the draft.".into())
                }
            }
            Ok(VoiceIntent::Dictation) => None,
            Ok(VoiceIntent::OpenSession(id)) => {
                target = self
                    .voice
                    .sessions
                    .as_ref()
                    .and_then(|sessions| sessions.iter().find(|s| s.session_id == id))
                    .cloned();
                target.is_none().then(|| "Jev could not match a session in your last 20. Transcript kept in the draft.".to_string())
            }
            Ok(VoiceIntent::Uncertain) => {
                Some("Jev is unsure which route to choose. Transcript kept in the draft.".into())
            }
            Err(error) => Some(format!(
                "Jev routing unavailable: {error}. Transcript kept in the draft."
            )),
        };
        let text = std::mem::take(&mut self.voice.live_transcript);
        self.voice.error = error;
        if coding_agent {
            // Send only this utterance, never text or attachments already in the composer.
            // Send ASAP like Enter: an active turn is steered, not queued behind.
            self.voice.decision = Some("Jev chose: Coding agent · Sent now".into());
            self.submit_or_queue(text, Vec::new(), false, cx);
        } else if let Some(action) = quick_action {
            self.voice.decision = Some("Jev chose: Quick action · Navigation".into());
            cx.emit(VoiceActionRequested(action, text));
        } else if let Some(session) = target {
            self.voice.decision = Some("Jev chose: Quick action · Open session".into());
            // Emit only an exact member of the bounded snapshot, never a model-supplied path.
            cx.emit(VoiceSessionRequested(session, text));
        } else {
            self.input
                .update(cx, |input, cx| input.append_dictation(&text, cx));
        }
        cx.notify();
    }

    /// Footer status text. Active work is shown by the transcript activity
    /// indicator instead, so the footer does not duplicate it.
    pub(super) fn render_voice_status(&self, status: String) -> Option<gpui::AnyElement> {
        if self.activity_active() {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "panel-status-badge".into())
                .flex()
                .items_center()
                .flex_shrink_1()
                .min_w(px(40.))
                .text_color(Theme::global().TEXT_DIM)
                .child(
                    div()
                        .debug_selector(|| "voice-ready-status".into())
                        .min_w_0()
                        .flex_shrink_1()
                        .truncate()
                        .child(status),
                )
                .into_any_element(),
        )
    }

    /// Voice pill on the right of the composer's pill row: microphone plus its
    /// keybinding, so the shortcut is visible without hovering. While
    /// voice is active it fills with the accent. No glow or halo.
    pub(super) fn render_voice_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let phase = self.voice.phase;
        let active = phase != Phase::Idle;
        let label = match phase {
            Phase::Idle => "Start voice",
            Phase::Checking => "Cancel microphone connection",
            Phase::Recording => "Stop voice",
            Phase::Transcribing | Phase::Routing => "Cancel voice request",
        };
        let tooltip_status = voice_tooltip(label, &self.status_line());
        let icon_color = if active { theme.BG } else { theme.TEXT_DIM };
        let size = super::composer::TAB_HEIGHT;
        let keycap_color = if active {
            theme.BG.opacity(0.8)
        } else {
            theme.TEXT_FAINT
        };
        let button = div()
            .id("voice-toggle")
            .debug_selector(|| "voice-toggle".into())
            .relative()
            .flex_none()
            .h(px(size))
            .pl(px(8.))
            .pr(px(9.))
            .gap(px(6.))
            .rounded_full()
            .flex()
            .items_center()
            .cursor_pointer()
            .bg(if active {
                theme.ACCENT
            } else {
                theme.prompt_background(usize::MAX)
            })
            .when(!active, |el| el.hover(|el| el.bg(theme.USER_BG)))
            .tooltip(move |_, cx| cx.new(|_| VoiceTooltip(tooltip_status.clone())).into())
            .on_click(cx.listener(|panel, _, window, cx| {
                // All controls use the same action path. Workspace captures
                // this before the panel fallback to stop the existing owner,
                // even if this button belongs to another conversation.
                panel.focus_input(window, cx);
                window.dispatch_action(Box::new(ToggleVoice), cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .debug_selector(|| "voice-microphone-icon".into())
                    .flex_none()
                    .size(px(12.))
                    .child(
                        gpui::svg()
                            .data(include_bytes!("../../../assets/icons/microphone.svg")
                                as &'static [u8])
                            .text_color(icon_color)
                            .size(px(12.)),
                    ),
            )
            .child(voice_shortcut_keycap(keycap_color.into(), &theme));
        div()
            .flex_none()
            .h(px(size))
            .child(button)
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "panel_voice_live_tests.rs"]
mod live_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn voice_report_retains_actual_answers_and_keeps_uncertain_text(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            let text = "maybe switch somewhere";
            panel
                .input
                .update(cx, |input, cx| input.set_content("typed work".into(), cx));
            panel.voice.phase = Phase::Routing;
            panel.voice.live_transcript = text.into();
            panel.voice.trace = Some(VoiceTrace {
                transcript: text.into(),
                candidates: Vec::new(),
                questions: voice_intent::describe_questions(text, &[]).unwrap(),
                answers: Vec::new(),
                audio: None,
                usage: None,
            });
            let attempt = panel.voice.canceled.clone();
            panel.finish_voice_report(
                &attempt,
                Ok(voice_intent::VoiceClassification {
                    intent: VoiceIntent::Uncertain,
                    answers: vec![voice_intent::VoiceAnswer {
                        id: "uncertain".into(),
                        probability: 0.91,
                    }],
                    usage: Some(voice_intent::VoiceUsage {
                        input_tokens: 1200,
                        output_tokens: 7,
                        requests: 1,
                    }),
                }),
                cx,
            );
            let trace = panel.voice.trace.as_ref().unwrap();
            assert_eq!(trace.transcript, text);
            assert_eq!(trace.questions.len(), 7);
            assert_eq!(trace.answers[0].probability, 0.91);
            assert_eq!(trace.usage.unwrap().input_tokens, 1200);
            assert_eq!(
                panel.input.read(cx).content.as_ref(),
                "typed work\nmaybe switch somewhere"
            );
            assert!(panel.voice.error.as_ref().unwrap().contains("unsure"));
            // A duplicate completion cannot replace the evidence or append again.
            panel.finish_voice_report(&attempt, Err(anyhow::anyhow!("late failure")), cx);
            assert_eq!(
                panel.voice.trace.as_ref().unwrap().answers[0].probability,
                0.91
            );
            panel.cancel_voice(cx);
            assert!(panel.voice.trace.is_none());
            panel.finish_voice_report(
                &attempt,
                Ok(voice_intent::VoiceClassification {
                    intent: VoiceIntent::CodingAgent,
                    answers: Vec::new(),
                    usage: None,
                }),
                cx,
            );
            assert!(panel.voice.trace.is_none());
            assert_eq!(
                panel.input.read(cx).content.as_ref(),
                "typed work\nmaybe switch somewhere"
            );
        });
    }

    #[gpui::test]
    fn voice_report_failure_keeps_questions_without_fabricating_answers(
        cx: &mut gpui::TestAppContext,
    ) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.voice.phase = Phase::Routing;
            panel.voice.live_transcript = "keep this".into();
            panel.voice.trace = Some(VoiceTrace {
                transcript: "keep this".into(),
                candidates: Vec::new(),
                questions: voice_intent::describe_questions("keep this", &[]).unwrap(),
                answers: Vec::new(),
                audio: None,
                usage: None,
            });
            let attempt = panel.voice.canceled.clone();
            panel.finish_voice_report(&attempt, Err(anyhow::anyhow!("provider unavailable")), cx);
            assert_eq!(panel.voice.trace.as_ref().unwrap().questions.len(), 7);
            assert!(panel.voice.trace.as_ref().unwrap().answers.is_empty());
            assert!(
                panel
                    .voice
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("provider unavailable")
            );
            assert_eq!(panel.input.read(cx).content.as_ref(), "keep this");
        });
    }

    #[gpui::test]
    fn voice_coding_agent_sends_only_transcript_once_and_preserves_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| {
            let mut panel = Panel::new("voice-agent".into(), None, None, bridge, cx);
            panel.history_loaded = true;
            panel.status = "idle".into();
            panel
        });
        panel.update(cx, |panel, cx| {
            panel.input.update(cx, |input, cx| {
                input.set_content("unfinished typed work".into(), cx)
            });
            panel.resolve_voice_for_test("debug this failure", Ok(VoiceIntent::CodingAgent), cx);
            assert_eq!(
                panel.input.read(cx).content.as_ref(),
                "unfinished typed work"
            );
            let attempt = panel.voice.canceled.clone();
            panel.finish_voice_routing(&attempt, Ok(VoiceIntent::CodingAgent), cx);
        });
        assert!(
            matches!(commands.try_recv(), Ok(Command::Send { session_id, content, images })
            if session_id == "voice-agent" && content == "debug this failure" && images.is_empty())
        );
        assert!(
            commands.try_recv().is_err(),
            "a duplicate completion must never send twice"
        );
    }

    #[gpui::test]
    fn voice_coding_agent_steers_while_busy(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = crate::harness::spawn_recording();
        let panel = cx.new(|cx| {
            let mut panel = Panel::new("voice-agent".into(), None, None, bridge, cx);
            panel.history_loaded = true;
            panel.status = "running".into();
            panel
        });
        panel.update(cx, |panel, cx| {
            panel.resolve_voice_for_test("then add tests", Ok(VoiceIntent::CodingAgent), cx);
        });
        // Sent while still running, never held for the turn to finish. The
        // harness delivers a Send during an active turn as an urgent steer.
        assert!(
            matches!(commands.try_recv(), Ok(Command::Send { content, .. }) if content == "then add tests")
        );
    }

    #[gpui::test]
    fn voice_hold_repeat_and_early_release_cancel_only_owned_startup(
        cx: &mut gpui::TestAppContext,
    ) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel
                .input
                .update(cx, |input, cx| input.set_content("keep".into(), cx));
            panel.voice.sessions = Some(Vec::new());
            // Seed the same state as production after its offline-preview guard.
            panel.prepare_voice_attempt(true);
            let attempt = panel.voice.canceled.clone();
            panel.begin_voice_hold(cx);
            panel.begin_voice_hold(cx);
            assert!(Arc::ptr_eq(&attempt, &panel.voice.canceled));
            assert!(panel.voice.phase == Phase::Checking);
            assert!(panel.voice.error.is_none());
            panel.end_voice_hold(cx);
            panel.end_voice_hold(cx);
            assert!(attempt.load(Ordering::SeqCst));
            assert!(panel.voice.phase == Phase::Idle);
            assert!(!panel.voice.hold_capture);
            assert!(panel.voice.sessions.is_some());
            assert_eq!(panel.input.read(cx).content.as_ref(), "keep");

            // Late startup completion cannot resurrect a canceled capture or
            // change a newer attempt. No recording or microphone is constructed.
            panel.finish_voice_startup(&attempt, Err(VoiceError::Cancelled), cx);
            assert!(panel.voice.phase == Phase::Idle);
            assert!(panel.voice.error.is_none());
            panel.prepare_voice_attempt(true);
            let current = panel.voice.canceled.clone();
            panel.finish_voice_startup(&attempt, Err(VoiceError::Network), cx);
            assert!(Arc::ptr_eq(&current, &panel.voice.canceled));
            assert!(panel.voice.phase == Phase::Checking);
            assert!(panel.voice.error.is_none());
            current.store(true, Ordering::SeqCst);
            panel.finish_voice_startup(&current, Err(VoiceError::Network), cx);
            assert!(panel.voice.phase == Phase::Checking);
            assert!(panel.voice.error.is_none());
        });
    }

    #[gpui::test]
    fn voice_hold_does_not_take_over_click_recording_or_routing(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            for phase in [
                Phase::Checking,
                Phase::Recording,
                Phase::Transcribing,
                Phase::Routing,
            ] {
                panel.voice.phase = phase;
                panel.voice.hold_capture = false;
                panel.voice.live_transcript = "existing click request".into();
                let attempt = panel.voice.canceled.clone();
                panel.begin_voice_hold(cx);
                panel.end_voice_hold(cx);
                assert!(panel.voice.phase == phase);
                assert!(Arc::ptr_eq(&attempt, &panel.voice.canceled));
                assert!(!attempt.load(Ordering::SeqCst));
                assert_eq!(panel.voice.live_transcript, "existing click request");
            }
        });
    }

    #[gpui::test]
    fn voice_hold_routes_final_transcription_without_touching_typed_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.voice.sessions = Some(Vec::new());
            panel.prepare_voice_attempt(true);
            panel.voice.phase = Phase::Recording;
            panel
                .input
                .update(cx, |input, cx| input.set_content("typed draft".into(), cx));
            panel.apply_voice_event(NariEvent::Transcript("fix the bug".into()), cx);
            assert_eq!(panel.input.read(cx).content.as_ref(), "typed draft");
            assert!(panel.voice.phase == Phase::Recording);
            panel.apply_voice_event(NariEvent::Finished(Ok("fix the bug".into())), cx);
            assert!(panel.voice.phase == Phase::Routing);
            assert!(!panel.voice.hold_capture);
            // Replace the unpolled task with a deterministic typed Jev response.
            panel.voice.task = None;
            let attempt = panel.voice.canceled.clone();
            panel.finish_voice_routing(&attempt, Ok(VoiceIntent::CodingAgent), cx);
            assert_eq!(panel.input.read(cx).content.as_ref(), "typed draft");
            assert!(
                panel
                    .voice
                    .decision
                    .as_ref()
                    .unwrap()
                    .contains("Coding agent")
            );
            assert!(
                panel
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::User(text) if text == "fix the bug"))
            );
            assert!(panel.voice.live_transcript.is_empty());
            assert!(!panel.voice_active());
        });
    }

    #[gpui::test]
    fn voice_hold_rejects_unsupported_and_offline_panels(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.session_id = "terminal://1".into();
            panel.begin_voice_hold(cx);
            assert!(!panel.voice_active());
            assert!(panel.voice.error.is_none());
            panel.session_id = "preview://chat".into();
            panel.begin_voice_hold(cx);
            assert!(!panel.voice_active());
            assert!(!panel.voice.hold_capture);
            assert!(
                panel
                    .voice
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("offline previews")
            );
            assert!(panel.voice.recording.is_none());
            assert!(panel.voice.task.is_none());
        });
    }

    #[gpui::test]
    fn voice_microphone_hover_shows_platform_shortcut(cx: &mut gpui::TestAppContext) {
        let (_panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        vcx.run_until_parked();
        let button = vcx.debug_bounds("voice-toggle").unwrap();
        vcx.update(|window, cx| window.simulate_mouse_move(button.center(), cx));
        vcx.run_until_parked();
        vcx.executor().advance_clock(Duration::from_secs(1));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-shortcut-tooltip").is_some());
        let text = voice_tooltip("Start voice", "Ready");
        assert!(text.starts_with(&format!("Start voice · {VOICE_SHORTCUT}")));
        assert!(text.contains("Ctrl+Shift+V toggles"));
        assert!(text.contains(&format!(
            "Hold {VOICE_SHORTCUT} to transcribe, release to finish"
        )));
        #[cfg(target_os = "macos")]
        assert!(text.contains("Command+Shift+M") && !text.contains("Copilot"));
        #[cfg(not(target_os = "macos"))]
        assert!(text.contains("Copilot key"));
    }

    #[gpui::test]
    fn voice_routing_non_navigation_retains_draft_and_never_sends(cx: &mut gpui::TestAppContext) {
        for result in [
            Ok(VoiceIntent::Dictation),
            Ok(VoiceIntent::Uncertain),
            Ok(VoiceIntent::OpenSession("not-offered".into())),
            Err(anyhow::anyhow!("offline")),
        ] {
            let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
            panel.update(cx, |panel, cx| {
                panel
                    .input
                    .update(cx, |input, cx| input.set_content("typed".into(), cx));
                panel.voice.sessions = Some(Vec::new());
                let before = panel.items.len();
                panel.resolve_voice_for_test("spoken words", result, cx);
                assert_eq!(panel.input.read(cx).content.as_ref(), "typed\nspoken words");
                assert_eq!(panel.items.len(), before);
                assert!(!panel.voice_active());
                assert!(panel.voice.live_transcript.is_empty());
            });
        }
    }

    #[gpui::test]
    fn canceled_and_superseded_jev_results_cannot_edit_the_draft(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.voice.phase = Phase::Routing;
            panel.voice.live_transcript = "discard".into();
            let attempt = panel.voice.canceled.clone();
            panel.cancel_voice(cx);
            panel.finish_voice_routing(&attempt, Ok(VoiceIntent::CodingAgent), cx);
            assert!(panel.input.read(cx).content.is_empty());
            assert!(panel.items.is_empty());
            panel.voice.phase = Phase::Routing;
            panel.voice.live_transcript = "new recording".into();
            panel.finish_voice_routing(&attempt, Ok(VoiceIntent::CodingAgent), cx);
            assert!(panel.input.read(cx).content.is_empty());
            assert!(panel.items.is_empty());
            assert_eq!(panel.voice.live_transcript, "new recording");
            assert!(
                !serde_json::to_string(&panel.snapshot(cx))
                    .unwrap()
                    .contains("new recording")
            );
        });
    }

    #[gpui::test]
    fn jev_navigation_preserves_composer_draft(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.input.update(cx, |input, cx| {
                input.set_content("keep my code request".into(), cx)
            });
            let session = serde_json::from_value(serde_json::json!({
                "session_id": "target", "status": "idle", "title": "PDF renderer"
            }))
            .unwrap();
            panel.voice.sessions = Some(vec![session]);
            panel.resolve_voice_for_test(
                "open PDF renderer",
                Ok(VoiceIntent::OpenSession("target".into())),
                cx,
            );
            assert_eq!(
                panel.input.read(cx).content.as_ref(),
                "keep my code request"
            );
            assert!(panel.voice.error.is_none());
        });
    }

    #[gpui::test]
    fn voice_streaming_revisions_preserve_typed_draft_and_stay_out_of_snapshots(
        cx: &mut gpui::TestAppContext,
    ) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.voice.phase = Phase::Recording;
            panel
                .input
                .update(cx, |input, cx| input.set_content("typed draft".into(), cx));
            panel.apply_voice_event(NariEvent::Transcript("hello wor".into()), cx);
            panel.apply_voice_event(NariEvent::Transcript("Hello world.".into()), cx);
            assert_eq!(panel.voice.live_transcript, "Hello world.");
            assert_eq!(panel.input.read(cx).content.as_ref(), "typed draft");
            let snapshot = serde_json::to_string(&panel.snapshot(cx)).unwrap();
            assert!(!snapshot.contains("Hello world"));
            assert!(!snapshot.contains("live_transcript"));
            let before = panel.items.len();
            panel.apply_voice_event(NariEvent::Finished(Ok("Hello world.".into())), cx);
            assert_eq!(
                panel.input.read(cx).content.as_ref(),
                "typed draft\nHello world."
            );
            assert_eq!(panel.items.len(), before, "never auto-submit");
            assert!(panel.voice.live_transcript.is_empty());
            assert!(!panel.voice_active());
        });
    }

    #[gpui::test]
    fn voice_cancel_discards_partial_without_changing_draft(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.voice.phase = Phase::Recording;
            panel
                .input
                .update(cx, |input, cx| input.set_content("keep".into(), cx));
            panel.apply_voice_event(NariEvent::Transcript("discard me".into()), cx);
            let token = panel.voice.canceled.clone();
            panel.cancel_voice(cx);
            assert!(token.load(Ordering::SeqCst));
            assert!(panel.voice.live_transcript.is_empty());
            assert_eq!(panel.input.read(cx).content.as_ref(), "keep");
        });
    }

    #[gpui::test]
    fn voice_only_targets_chat_composers(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            for id in ["session_chat", "startup://draft", "startup://draft/next"] {
                panel.session_id = id.into();
                assert!(panel.supports_voice(), "{id}");
            }
            for id in [
                "terminal://1",
                "gmail://inbox",
                "settings://machines",
                "desktop://changelog",
                "file:///note.md",
            ] {
                panel.session_id = id.into();
                assert!(!panel.supports_voice(), "{id}");
                panel.toggle_voice(cx);
                assert!(!panel.voice_active());
                assert!(panel.voice.error.is_none());
            }
        });
    }

    #[gpui::test]
    fn voice_meter_renders_without_streaming_into_composer(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(vcx, |panel, cx| {
            panel.voice.phase = Phase::Recording;
            panel.apply_voice_event(NariEvent::Transcript("Streaming Nari words".into()), cx);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-live-transcript").is_none());
        assert!(vcx.debug_bounds("voice-waveform").is_some());
        panel.read_with(vcx, |panel, cx| {
            assert!(panel.input.read(cx).content.is_empty())
        });
    }

    #[gpui::test]
    fn voice_shortcut_toggles_focused_composer_without_sending(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::input::bind_keys(cx);
            crate::bind_workspace_keys(cx);
            // Reloading the keymap must not dispatch the toggle twice.
            crate::bind_workspace_keys(cx);
        });
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(vcx, |panel, cx| {
            panel.input.update(cx, |input, cx| {
                input.set_content("keep my draft".into(), cx)
            });
        });
        vcx.update(|window, cx| {
            let focus = panel.read(cx).input.read(cx).focus_handle.clone();
            focus.focus(window, cx);
        });
        vcx.simulate_keystrokes("ctrl-shift-v");
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, cx| {
            assert!(
                panel
                    .voice
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("offline previews")
            );
            assert_eq!(panel.input.read(cx).content.as_ref(), "keep my draft");
            assert!(panel.voice.recording.is_none());
        });
        for phase in [Phase::Checking, Phase::Transcribing] {
            panel.update(vcx, |panel, cx| {
                panel.voice.error = None;
                panel.voice.phase = phase;
                cx.notify();
            });
            vcx.simulate_keystrokes("ctrl-shift-v");
            panel.read_with(vcx, |panel, cx| {
                assert!(panel.voice.phase == Phase::Idle);
                assert!(panel.voice.error.is_none(), "toggle must fire only once");
                assert_eq!(panel.input.read(cx).content.as_ref(), "keep my draft");
            });
        }
    }

    #[gpui::test]
    fn voice_footer_keeps_microphone_and_spacing_at_all_widths(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [240., 320., 480., 800., 1440.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            for phase in [
                Phase::Idle,
                Phase::Checking,
                Phase::Recording,
                Phase::Transcribing,
            ] {
                panel.update(vcx, |panel, cx| {
                    panel.model = Some("gpt-6-astra".into());
                    panel.provider = Some("openai".into());
                    panel.voice.phase = phase;
                    cx.notify();
                });
                vcx.run_until_parked();
                let input = vcx.debug_bounds("prompt-input").unwrap();
                let button = vcx.debug_bounds("voice-toggle").unwrap();
                let icon = vcx.debug_bounds("voice-microphone-icon").unwrap();
                let shortcut = vcx.debug_bounds("voice-shortcut").unwrap();
                // The microphone is a round button detached above the input.
                assert!(
                    button.left() >= input.left() && button.right() <= input.right(),
                    "voice at {width}: {button:?} outside {input:?}"
                );
                assert!(
                    button.bottom() < input.top(),
                    "voice is detached from the input"
                );
                assert!(icon.left() >= button.left() && icon.right() <= button.right());
                assert_eq!(icon.size.width, px(12.));
                assert_eq!(button.size.height, px(crate::panel::composer::TAB_HEIGHT));
                // The keybinding sits inside the pill, right of the icon.
                assert!(icon.right() <= shortcut.left());
                assert!(shortcut.right() <= button.right());
                if phase == Phase::Idle {
                    let status = vcx.debug_bounds("voice-ready-status").unwrap();
                    assert!(
                        status.top() >= input.bottom(),
                        "status sits in the bottom bar"
                    );
                }
            }
        }
    }

    #[gpui::test]
    fn voice_controls_render_and_cancel_without_network(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(240.), px(320.)));
        vcx.run_until_parked();
        let button = vcx
            .debug_bounds("voice-toggle")
            .expect("voice button in fresh composer");
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert!(
                panel
                    .voice
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("offline previews")
            );
            assert!(panel.voice.task.is_none());
        });
        let dismiss = vcx.debug_bounds("voice-cancel").expect("dismiss error");
        vcx.simulate_click(dismiss.center(), gpui::Modifiers::default());
        for phase in [Phase::Checking, Phase::Recording, Phase::Transcribing] {
            panel.update(vcx, |panel, cx| {
                panel
                    .items
                    .push(Item::Assistant("Existing conversation".into()));
                panel.voice.phase = phase;
                panel.voice.started = Some(Instant::now());
                cx.notify();
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("voice-toggle").is_some());
            let cancel = vcx
                .debug_bounds("voice-cancel")
                .expect("cancel visible during work");
            vcx.simulate_click(cancel.center(), gpui::Modifiers::default());
            panel.read_with(vcx, |panel, _| assert!(panel.voice.phase == Phase::Idle));
        }
    }

    #[gpui::test]
    fn voice_insertion_is_undoable(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(vcx, |panel, cx| {
            panel
                .input
                .update(cx, |input, cx| input.set_content("original".into(), cx));
            panel.finish_voice(Ok("dictated".into()), cx);
        });
        vcx.update(|window, cx| {
            let focus = panel.read(cx).input.read(cx).focus_handle.clone();
            focus.focus(window, cx);
        });
        vcx.simulate_keystrokes("ctrl-z");
        panel.read_with(vcx, |panel, cx| {
            assert_eq!(panel.input.read(cx).content.as_ref(), "original")
        });
    }

    #[gpui::test]
    fn voice_appends_to_current_draft_without_sending(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            Panel::new(
                "voice-test".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(cx, |panel, cx| {
            panel.input.update(cx, |input, cx| {
                input.set_content("Typed while transcribing".into(), cx)
            });
            let items = panel.items.len();
            panel.finish_voice(Ok("  dictated words  ".into()), cx);
            assert_eq!(
                panel.input.read(cx).snapshot().content,
                "Typed while transcribing\ndictated words"
            );
            assert_eq!(
                panel.items.len(),
                items,
                "dictation must never submit a prompt"
            );
            panel.finish_voice(Err(VoiceError::Network), cx);
            assert_eq!(
                panel.input.read(cx).snapshot().content,
                "Typed while transcribing\ndictated words"
            );
            assert!(panel.voice.error.is_some());
        });
    }

    #[gpui::test]
    fn voice_cancel_invalidates_pending_work_and_preserves_draft(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            Panel::new(
                "voice-test".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(cx, |panel, cx| {
            let token = panel.voice.canceled.clone();
            panel.voice.phase = Phase::Checking;
            panel
                .input
                .update(cx, |input, cx| input.set_content("keep me".into(), cx));
            panel.cancel_voice(cx);
            assert!(token.load(Ordering::SeqCst));
            assert!(!Arc::ptr_eq(&token, &panel.voice.canceled));
            assert_eq!(panel.input.read(cx).snapshot().content, "keep me");
            assert!(panel.voice.phase == Phase::Idle);
        });
    }

    #[gpui::test]
    fn voice_preview_never_opens_microphone(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.start_voice(cx);
            assert!(panel.voice.phase == Phase::Idle);
            assert!(panel.voice.recording.is_none());
            assert!(panel.voice.task.is_none());
            assert!(
                panel
                    .voice
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("offline previews")
            );
        });
    }
}
