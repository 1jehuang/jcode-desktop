//! Explicit, subscription-backed microphone dictation. Audio is never snapshotted.
use super::*;
use jcode_base::voice::{self, MicrophoneRecording, VoiceError};
use std::sync::atomic::{AtomicBool, Ordering};

gpui::actions!(panel_voice, [ToggleVoice]);

pub(crate) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([gpui::KeyBinding::new(
        "ctrl-shift-v",
        ToggleVoice,
        Some("ChatPanel"),
    )]);
}

const VOICE_SHORTCUT: &str = "Ctrl+Shift+V";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Recording,
    Transcribing,
}

#[derive(Default)]
pub(super) struct VoiceState {
    phase: Phase,
    recording: Option<MicrophoneRecording>,
    canceled: Arc<AtomicBool>,
    started: Option<Instant>,
    error: Option<String>,
    task: Option<Task<()>>,
    timer: Option<Task<()>>,
}

impl Drop for VoiceState {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::SeqCst);
        if let Some(recording) = self.recording.take() {
            // Native stream teardown must not block the UI or hot reload.
            std::thread::spawn(move || recording.cancel());
        }
    }
}

struct VoiceTooltip(String);
impl Render for VoiceTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().max_w(px(300.)).p_2().rounded_md().bg(Theme::global().HEADER_BG)
            .text_size(px(12.)).text_color(Theme::global().TEXT)
            .child(format!("{} · {VOICE_SHORTCUT} starts/stops voice dictation (or cancels a pending request). Voice dictation is included with an active Jcode subscription. Audio is sent to Groq when you stop. Review the transcript before sending.", self.0))
    }
}

async fn canceled(token: &AtomicBool) {
    while !token.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl Panel {
    pub(super) fn toggle_voice(&mut self, cx: &mut Context<Self>) {
        match self.voice.phase {
            Phase::Idle => self.start_voice(cx),
            Phase::Recording => self.stop_voice(cx),
            Phase::Checking | Phase::Transcribing => self.cancel_voice(cx),
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
            let runtime = tokio::runtime::Runtime::new().map_err(|_| VoiceError::Unavailable)?;
            let available = runtime.block_on(async {
                tokio::select! {
                    available = voice::subscription_voice_available() => available,
                    _ = canceled(&token) => false,
                }
            });
            if !available || token.load(Ordering::SeqCst) {
                return Err(VoiceError::Unavailable);
            }
            MicrophoneRecording::start_cancellable(token)
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
                        panel.voice.error = Some(if error == VoiceError::Unavailable {
                            "Voice needs an active Jcode subscription and an available transcription service. Sign in through Accounts, then try again.".into()
                        } else { error.to_string() });
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn tick_voice(&mut self, cx: &mut Context<Self>) {
        self.voice.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let keep_ticking = this
                    .update(cx, |panel, cx| {
                        if panel.voice.phase != Phase::Recording {
                            return false;
                        }
                        if panel
                            .voice
                            .recording
                            .as_ref()
                            .is_some_and(MicrophoneRecording::is_finished)
                        {
                            panel.stop_voice(cx);
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

    fn stop_voice(&mut self, cx: &mut Context<Self>) {
        let Some(recording) = self.voice.recording.take() else {
            return;
        };
        self.voice.phase = Phase::Transcribing;
        let token = self.voice.canceled.clone();
        let attempt = token.clone();
        let work = cx.background_executor().spawn(async move {
            let wav = recording.stop()?;
            if token.load(Ordering::SeqCst) {
                return Err(VoiceError::Unavailable);
            }
            let runtime = tokio::runtime::Runtime::new().map_err(|_| VoiceError::Unavailable)?;
            runtime.block_on(async {
                tokio::select! {
                    result = voice::transcribe_wav(wav, None) => result,
                    _ = canceled(&token) => Err(VoiceError::Unavailable),
                }
            })
        });
        self.voice.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |panel, cx| {
                if !Arc::ptr_eq(&attempt, &panel.voice.canceled) {
                    return;
                }
                panel.finish_voice(result, cx);
            });
        }));
        cx.notify();
    }

    fn finish_voice(&mut self, result: Result<String, VoiceError>, cx: &mut Context<Self>) {
        self.voice.phase = Phase::Idle;
        self.voice.started = None;
        match result {
            Ok(text) if !text.trim().is_empty() => {
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

    pub(super) fn render_voice_status(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let detail = self.voice.error.clone().or_else(|| match self.voice.phase {
            Phase::Idle => None,
            Phase::Checking => Some("Checking subscription and microphone access…".into()),
            Phase::Recording => {
                let secs = self.voice.started.map_or(0, |t| t.elapsed().as_secs());
                Some(format!("Recording {}:{:02} · Stop sends audio to Groq for transcription · 5 min / 10 MiB maximum", secs / 60, secs % 60))
            }
            Phase::Transcribing => Some("Transcribing with Groq. The result will be added to this draft, not sent.".into()),
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
                .child(div().min_w_0().flex_1().child(detail))
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
            Phase::Transcribing => "Cancel",
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
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.toggle_voice(cx);
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
                    shortcut.size.width > px(60.),
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
