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

#[cfg(not(target_os = "macos"))]
const VOICE_SHORTCUT: &str = "Copilot key";
#[cfg(target_os = "macos")]
const VOICE_SHORTCUT: &str = "⌘⇧M (Command+Shift+M)";

fn voice_tooltip(action: &str, status: &str) -> String {
    format!(
        "{action} · {VOICE_SHORTCUT}\nHold {VOICE_SHORTCUT} to transcribe, release to finish. {status}. Ctrl+Shift+V toggles recording in the composer. Audio streams to Nari. Held speech always becomes a draft, never sent automatically. Click recording can also recognize requests to open a recent session."
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

#[derive(Default)]
pub(super) struct VoiceState {
    phase: Phase,
    // Attempt ownership persists through final transcription, even after key release.
    hold_capture: bool,
    recording: Option<NariRecording>,
    live_transcript: String,
    origin: Option<gpui::Bounds<gpui::Pixels>>,
    entrance: Option<Instant>,
    canceled: Arc<AtomicBool>,
    started: Option<Instant>,
    error: Option<String>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
    sessions: Option<Vec<jcode_sdk::SessionInfo>>,
    navigation: Option<gpui::Subscription>,
}

pub(crate) struct VoiceSessionRequested(pub jcode_sdk::SessionInfo);
impl gpui::EventEmitter<VoiceSessionRequested> for Panel {}

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

    pub(crate) fn begin_voice_hold(&mut self, cx: &mut Context<Self>) {
        if self.supports_voice() && self.voice.phase == Phase::Idle {
            self.start_voice_with_hold(true, cx);
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
        self.voice = VoiceState::default();
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
        self.prepare_voice_attempt(hold_capture);
        self.voice.origin = self.input.read(cx).voice_bounds();
        self.voice.entrance = Some(Instant::now());
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
                self.voice.phase = Phase::Recording;
                self.voice.recording = Some(recording);
                self.voice.started = Some(Instant::now());
                self.tick_voice(cx);
            }
            Err(error) => {
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
        self.voice.phase = Phase::Transcribing;
        cx.notify();
    }

    fn finish_voice(&mut self, result: Result<String, VoiceError>, cx: &mut Context<Self>) {
        let dictation_only = std::mem::take(&mut self.voice.hold_capture);
        self.voice.phase = Phase::Idle;
        self.voice.started = None;
        self.voice.recording = None;
        self.voice.live_transcript.clear();
        match result {
            Ok(text) if !text.trim().is_empty() => {
                if !dictation_only && self.voice.sessions.is_some() {
                    self.route_voice(text, cx);
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
        cx.notify();
    }

    fn route_voice(&mut self, text: String, cx: &mut Context<Self>) {
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
        let work = cx.background_executor().spawn(async move {
            tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)?
                .block_on(voice_intent::classify(&text, &candidates))
        });
        self.voice.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |panel, cx| {
                panel.finish_voice_routing(&attempt, result, cx);
            });
        }));
        cx.notify();
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
        let error = match result {
            Ok(VoiceIntent::Dictation) => None,
            Ok(VoiceIntent::OpenSession(id)) => {
                target = self.voice.sessions.as_ref()
                    .and_then(|sessions| sessions.iter().find(|s| s.session_id == id)).cloned();
                target.is_none().then(|| "Jev could not match a session in your last 20. Transcript kept in the draft.".to_string())
            }
            Ok(VoiceIntent::Uncertain) => Some("Jev couldn't confidently match a session in your last 20. Try its title or project name. Transcript kept in the draft.".into()),
            Err(error) => Some(format!("Jev session lookup unavailable: {error}. Transcript kept in the draft.")),
        };
        let text = std::mem::take(&mut self.voice.live_transcript);
        self.voice.error = error;
        if let Some(session) = target {
            // Emit only an exact member of the bounded snapshot, never a model-supplied path.
            cx.emit(VoiceSessionRequested(session));
        } else {
            self.input
                .update(cx, |input, cx| input.append_dictation(&text, cx));
        }
        cx.notify();
    }

    pub(super) fn render_voice_controls(
        &self,
        status: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let phase = self.voice.phase;
        let label = match phase {
            Phase::Idle => "Start voice",
            Phase::Checking => "Cancel microphone connection",
            Phase::Recording => "Stop voice",
            Phase::Transcribing | Phase::Routing => "Cancel voice request",
        };
        let active = self.activity_active();
        let tooltip_status = voice_tooltip(label, &status);
        div()
            .debug_selector(move || {
                if active {
                    "panel-voice-active".into()
                } else {
                    "panel-status-badge".into()
                }
            })
            .flex()
            .items_center()
            .gap_2()
            .justify_end()
            .flex_shrink_1()
            .min_w(px(160.))
            .text_size(px(10.5))
            .text_color(theme.TEXT_DIM)
            .when(!active, |el| {
                el.child(
                    div()
                        .debug_selector(|| "voice-ready-status".into())
                        .min_w_0()
                        .flex_shrink_1()
                        .truncate()
                        .child(status),
                )
            })
            .child(
                div()
                    .id("voice-toggle")
                    .debug_selector(|| "voice-toggle".into())
                    .tooltip(move |_, cx| cx.new(|_| VoiceTooltip(tooltip_status.clone())).into())
                    .flex_none()
                    .w(px(28.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(theme.HEADER_BG)
                    .text_color(if phase != Phase::Idle {
                        theme.ACCENT
                    } else {
                        theme.TEXT_DIM
                    })
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
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
                            .size(px(16.))
                            .child(
                                gpui::svg()
                                    .data(include_bytes!("../../../assets/icons/microphone.svg")
                                        as &'static [u8])
                                    .text_color(if phase != Phase::Idle {
                                        theme.ACCENT
                                    } else {
                                        theme.TEXT_DIM
                                    })
                                    .size(px(16.)),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn voice_hold_streams_then_appends_dictation_without_navigation_or_sending(
        cx: &mut gpui::TestAppContext,
    ) {
        let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(cx, |panel, cx| {
            panel.voice.sessions = Some(vec![
                serde_json::from_value(serde_json::json!({
                    "session_id": "target", "status": "idle", "title": "PDF renderer"
                }))
                .unwrap(),
            ]);
            panel.prepare_voice_attempt(true);
            panel.voice.phase = Phase::Recording;
            panel
                .input
                .update(cx, |input, cx| input.set_content("typed draft".into(), cx));
            panel.apply_voice_event(NariEvent::Transcript("open PDF".into()), cx);
            panel.apply_voice_event(NariEvent::Transcript("open PDF renderer".into()), cx);
            assert_eq!(panel.voice.live_transcript, "open PDF renderer");
            assert_eq!(panel.input.read(cx).content.as_ref(), "typed draft");
            assert!(
                !serde_json::to_string(&panel.snapshot(cx))
                    .unwrap()
                    .contains("open PDF renderer")
            );
            let attempt = panel.voice.canceled.clone();
            panel.begin_voice_hold(cx);
            assert!(Arc::ptr_eq(&attempt, &panel.voice.canceled));
            // Model stop_voice's post-stop phase without opening a live mic.
            panel.voice.phase = Phase::Transcribing;
            panel.end_voice_hold(cx);
            panel.end_voice_hold(cx);
            assert!(panel.voice.hold_capture);
            assert!(!attempt.load(Ordering::SeqCst));
            let before = panel.items.len();
            panel.apply_voice_event(NariEvent::Finished(Ok("open PDF renderer".into())), cx);
            assert!(
                panel.voice.phase == Phase::Idle,
                "must not enter voice routing"
            );
            assert!(!panel.voice.hold_capture);
            assert!(panel.voice.task.is_none(), "must not start a classifier");
            assert_eq!(
                panel.input.read(cx).content.as_ref(),
                "typed draft\nopen PDF renderer"
            );
            assert_eq!(panel.items.len(), before, "never sends");
            assert!(panel.voice.live_transcript.is_empty());
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
            panel.finish_voice_routing(&attempt, Ok(VoiceIntent::Dictation), cx);
            assert!(panel.input.read(cx).content.is_empty());
            panel.voice.phase = Phase::Routing;
            panel.voice.live_transcript = "new recording".into();
            panel.finish_voice_routing(&attempt, Ok(VoiceIntent::Dictation), cx);
            assert!(panel.input.read(cx).content.is_empty());
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
    fn voice_live_preview_renders_without_replacing_composer(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(vcx, |panel, cx| {
            panel.voice.phase = Phase::Recording;
            panel.apply_voice_event(NariEvent::Transcript("Streaming Nari words".into()), cx);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-live-transcript").is_some());
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
                let footer = vcx.debug_bounds("panel-meta").unwrap();
                let button = vcx.debug_bounds("voice-toggle").unwrap();
                let icon = vcx.debug_bounds("voice-microphone-icon").unwrap();
                assert!(vcx.debug_bounds("voice-shortcut").is_none());
                let status = vcx.debug_bounds("voice-ready-status").unwrap();
                assert!(
                    button.left() >= footer.left() && button.right() <= footer.right(),
                    "voice at {width}: {button:?} outside {footer:?}"
                );
                assert!(button.top() >= footer.top() && button.bottom() <= footer.bottom());
                assert!(icon.left() >= button.left() && icon.right() <= button.right());
                assert_eq!(icon.size.width, px(16.));
                assert_eq!(button.size.width, px(28.));
                assert!(
                    button.left() - status.right() >= px(7.),
                    "Ready needs breathing room"
                );
                assert_eq!(button.size.height, px(22.));
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
