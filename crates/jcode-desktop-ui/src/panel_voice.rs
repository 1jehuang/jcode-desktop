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

const VOICE_SHORTCUT: &str = "Copilot";

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
    recording: Option<NariRecording>,
    live_transcript: String,
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
        div().max_w(px(300.)).p_2().rounded_md().bg(Theme::global().HEADER_BG)
            .text_size(px(12.)).text_color(Theme::global().TEXT)
            .child(format!("{} · {VOICE_SHORTCUT} starts/stops voice (or cancels a pending request). Ctrl+Shift+V also works in the composer. Audio streams to Nari. Jev checks the transcript against your last 20 sessions to recognize requests to open one. Other speech becomes a draft, never sent automatically.", self.0))
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
        self.voice = VoiceState::default();
        cx.notify();
    }

    fn start_voice(&mut self, cx: &mut Context<Self>) {
        if self.preview_state.is_some() || crate::harness::screenshot_mode() {
            self.voice.error = Some("Microphone access is disabled in offline previews.".into());
            cx.notify();
            return;
        }
        if self.voice.phase != Phase::Idle {
            return;
        }
        self.voice = VoiceState::default();
        self.voice.phase = Phase::Checking;
        let token = self.voice.canceled.clone();
        let attempt = token.clone();
        let work = cx.background_executor().spawn(async move {
            let key = voice::nari_api_key().ok_or(VoiceError::NariNotConfigured)?;
            NariRecording::start_cancellable(token, &key)
        });
        self.voice.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |panel, cx| {
                if !Arc::ptr_eq(&attempt, &panel.voice.canceled) {
                    return;
                }
                match result {
                    Ok(recording) => {
                        panel.voice.phase = Phase::Recording;
                        panel.voice.recording = Some(recording);
                        panel.voice.started = Some(Instant::now());
                        panel.tick_voice(cx);
                    }
                    Err(error) => {
                        panel.voice.phase = Phase::Idle;
                        panel.voice.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
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
        self.voice.phase = Phase::Idle;
        self.voice.started = None;
        self.voice.recording = None;
        self.voice.live_transcript.clear();
        match result {
            Ok(text) if !text.trim().is_empty() => {
                if self.voice.sessions.is_some() {
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

    pub(super) fn render_voice_status(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let detail = self
            .voice
            .error
            .clone()
            .or_else(|| match self.voice.phase {
                Phase::Idle => None,
                Phase::Checking => {
                    Some("Connecting to Nari and checking microphone access…".into())
                }
                Phase::Recording => {
                    let secs = self.voice.started.map_or(0, |t| t.elapsed().as_secs());
                    Some(format!(
                        "Streaming to Nari {}:{:02} · Copilot to stop · 5 min maximum",
                        secs / 60,
                        secs % 60
                    ))
                }
                Phase::Transcribing => Some(
                    "Finishing Nari transcript. Nothing is sent to the conversation.".into(),
                ),
                Phase::Routing => Some("Jev is checking your request against your last 20 sessions… Copilot to cancel.".into()),
            })?;
        Some(
            div()
                .debug_selector(|| "voice-status".into())
                .flex_none()
                .px_3()
                .py_1()
                .text_size(px(11.))
                .text_color(Theme::global().TEXT_DIM)
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(detail)
                        .when(!self.voice.live_transcript.is_empty(), |el| {
                            el.child(
                                div()
                                    .id("voice-live-transcript")
                                    .debug_selector(|| "voice-live-transcript".into())
                                    .max_h(px(100.))
                                    .overflow_y_scroll()
                                    .text_color(Theme::global().TEXT)
                                    .child(self.voice.live_transcript.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .id("voice-cancel")
                        .debug_selector(|| "voice-cancel".into())
                        .flex_none()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|el| el.bg(Theme::global().ACCENT_DIM))
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.cancel_voice(cx);
                            cx.stop_propagation();
                        }))
                        .child(if self.voice.phase == Phase::Idle {
                            "Dismiss"
                        } else {
                            "Cancel"
                        }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_voice_controls(
        &self,
        status: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let phase = self.voice.phase;
        let label = match phase {
            Phase::Idle => "Voice",
            Phase::Checking => "Cancel",
            Phase::Recording => "Stop",
            Phase::Transcribing | Phase::Routing => "Cancel",
        };
        let active = self.activity_active();
        let tooltip_status = status.clone();
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
                    .px_2()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .bg(theme.HEADER_BG)
                    .text_color(if phase == Phase::Recording {
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
                    .child(label)
                    .child(
                        div()
                            .debug_selector(|| "voice-shortcut".into())
                            .text_size(px(10.))
                            .child(VOICE_SHORTCUT),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                assert_eq!(panel.input.read(cx).content.as_ref(), "typed spoken words");
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
                "typed draft Hello world."
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
    fn voice_footer_keeps_shortcut_and_spacing_at_all_widths(cx: &mut gpui::TestAppContext) {
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
                let shortcut = vcx.debug_bounds("voice-shortcut").unwrap();
                let status = vcx.debug_bounds("voice-ready-status").unwrap();
                assert!(
                    button.left() >= footer.left() && button.right() <= footer.right(),
                    "voice at {width}: {button:?} outside {footer:?}"
                );
                assert!(button.top() >= footer.top() && button.bottom() <= footer.bottom());
                assert!(shortcut.left() >= button.left() && shortcut.right() <= button.right());
                assert!(
                    shortcut.size.width > px(30.),
                    "shortcut must not be clipped"
                );
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
                "Typed while transcribing dictated words"
            );
            assert_eq!(
                panel.items.len(),
                items,
                "dictation must never submit a prompt"
            );
            panel.finish_voice(Err(VoiceError::Network), cx);
            assert_eq!(
                panel.input.read(cx).snapshot().content,
                "Typed while transcribing dictated words"
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
