//! Window-anchored speech feedback. It floats above panels without moving the draft.
use super::*;

impl Panel {
    pub(in crate::panel) fn render_voice_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.voice_active() && self.voice.error.is_none() {
            return None;
        }
        let theme = Theme::global();
        let phase = self.voice.phase;
        let title = match phase {
            Phase::Idle => "Voice unavailable",
            Phase::Checking => "Connecting microphone…",
            Phase::Recording => "Listening…",
            Phase::Transcribing => "Finishing transcript…",
            Phase::Routing => "Understanding your request…",
        };
        let detail = self.voice.error.clone().unwrap_or_else(|| match phase {
            Phase::Recording => "Audio streams to Nari · Copilot to stop".into(),
            Phase::Checking => "Connecting to Nari and checking microphone access".into(),
            Phase::Transcribing => "Your words will not be sent automatically".into(),
            Phase::Routing => "Jev is checking your last 20 sessions".into(),
            Phase::Idle => String::new(),
        });
        let viewport = window.viewport_size();
        let width = (viewport.width - px(32.)).max(px(160.)).min(px(560.));
        let transcript_height = (viewport.height * 0.25).min(px(160.));
        let card = div()
            .id("voice-overlay")
            .debug_selector(|| "voice-overlay".into())
            .w(width)
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_xl()
            .border_1()
            .border_color(theme.ACCENT.opacity(0.35))
            .bg(theme.PANEL_BG)
            .shadow_lg()
            .text_color(theme.TEXT)
            .text_size(px(13.))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(8.)).rounded_full().bg(theme.ACCENT))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when(phase == Phase::Recording, |el| {
                        let seconds = self.voice.started.map_or(0, |t| t.elapsed().as_secs());
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.TEXT_DIM)
                                .child(format!("{}:{:02}", seconds / 60, seconds % 60)),
                        )
                    }),
            )
            .when(self.voice_active(), |el| {
                el.child(
                    div()
                        .id("voice-live-transcript")
                        .debug_selector(|| "voice-live-transcript".into())
                        .min_h(px(40.))
                        .max_h(transcript_height)
                        .overflow_y_scroll()
                        .track_scroll(&self.voice.transcript_scroll)
                        .text_size(px(17.))
                        .text_color(if self.voice.live_transcript.is_empty() {
                            theme.TEXT_DIM
                        } else {
                            theme.TEXT
                        })
                        .child(if self.voice.live_transcript.is_empty() {
                            match phase {
                                Phase::Checking => "Your words will appear here…",
                                Phase::Recording => {
                                    "Speak now. Your words appear here as you talk…"
                                }
                                _ => "Waiting for the final transcript…",
                            }
                            .to_string()
                        } else {
                            self.voice.live_transcript.clone()
                        }),
                )
            })
            .child(
                div()
                    .debug_selector(|| "voice-status".into())
                    .text_size(px(11.))
                    .text_color(theme.TEXT_DIM)
                    .child(detail),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(
                        div()
                            .id("voice-cancel")
                            .debug_selector(|| "voice-cancel".into())
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme.TEXT_DIM)
                            .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.cancel_voice(cx);
                                cx.stop_propagation();
                            }))
                            .child(if phase == Phase::Idle {
                                "Dismiss"
                            } else {
                                "Cancel"
                            }),
                    )
                    .when(phase == Phase::Recording, |el| {
                        el.child(
                            div()
                                .id("voice-stop")
                                .debug_selector(|| "voice-stop".into())
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme.ACCENT_DIM)
                                .text_color(theme.ACCENT)
                                .hover(|el| el.bg(theme.ACCENT.opacity(0.25)))
                                .on_click(cx.listener(|panel, _, _, cx| {
                                    panel.stop_voice(cx);
                                    cx.stop_propagation();
                                }))
                                .child("Stop"),
                        )
                    }),
            );
        // Deferred painting escapes the transcript's clipping. Only the card
        // captures clicks, so the rest of the window stays usable while listening.
        Some(
            gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Anchor::BottomLeft)
                    .position(gpui::point(
                        (viewport.width - width) / 2.,
                        viewport.height - px(24.),
                    ))
                    .child(card),
            )
            .with_priority(90)
            .into_any_element(),
        )
    }

    pub(in crate::panel) fn seed_voice_preview(&mut self, state: PreviewState) {
        self.voice = VoiceState::default();
        self.voice.phase = if state == PreviewState::VoiceConnecting {
            Phase::Checking
        } else {
            Phase::Recording
        };
        if state == PreviewState::VoiceListening {
            self.voice.started = Some(Instant::now());
            self.voice.live_transcript =
                "Show me what I’m saying as I speak, right here on the screen.".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn voice_overlay_is_bottom_center_without_moving_composer(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        for (width, height) in [(240., 320.), (480., 600.), (1440., 1000.)] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
            panel.update(vcx, |panel, cx| panel.cancel_voice(cx));
            vcx.run_until_parked();
            let footer = vcx.debug_bounds("panel-meta").unwrap();
            for phase in [
                Phase::Checking,
                Phase::Recording,
                Phase::Transcribing,
                Phase::Routing,
            ] {
                panel.update(vcx, |panel, cx| {
                    panel.voice.phase = phase;
                    cx.notify();
                });
                vcx.run_until_parked();
                let card = vcx
                    .debug_bounds("voice-overlay")
                    .expect("visible before words arrive");
                assert!(
                    (card.center().x - px(width / 2.)).abs() <= px(1.),
                    "{card:?}"
                );
                assert!(
                    (card.bottom() - px(height - 24.)).abs() <= px(1.),
                    "{card:?}"
                );
                assert!(card.left() >= px(0.) && card.right() <= px(width));
                assert!(card.top() >= px(0.));
                assert_eq!(
                    vcx.debug_bounds("panel-meta").unwrap(),
                    footer,
                    "overlay must not reflow the composer"
                );
                let cancel = vcx.debug_bounds("voice-cancel").unwrap();
                assert!(cancel.bottom() <= card.bottom());
                assert_eq!(
                    vcx.debug_bounds("voice-stop").is_some(),
                    phase == Phase::Recording
                );
            }
        }
    }

    #[gpui::test]
    fn voice_overlay_follows_stream_revisions_and_closes_after_finishing(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, vcx) =
            cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::VoiceListening, cx));
        panel.update(vcx, |panel, cx| {
            panel.input.update(cx, |input, cx| {
                input.set_content("Keep my draft".into(), cx)
            });
            panel.apply_voice_event(NariEvent::Transcript("first partial".into()), cx);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-live-transcript").is_some());
        panel.update(vcx, |panel, cx| {
            panel.apply_voice_event(
                NariEvent::Transcript("Revised live words. ".repeat(100)),
                cx,
            );
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, cx| {
            assert_eq!(
                panel.voice.live_transcript,
                "Revised live words. ".repeat(100)
            );
            assert!(
                panel.voice.transcript_scroll.offset().y < px(0.),
                "latest words scroll into view"
            );
            assert_eq!(panel.input.read(cx).content.as_ref(), "Keep my draft");
            assert!(
                panel.voice.recording.is_none(),
                "fixtures never access the microphone"
            );
        });
        panel.update(vcx, |panel, cx| {
            panel.apply_voice_event(NariEvent::Finished(Ok("Final words.".into())), cx)
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-overlay").is_none());
        panel.read_with(vcx, |panel, cx| {
            assert_eq!(
                panel.input.read(cx).content.as_ref(),
                "Keep my draft Final words."
            );
        });
    }

    #[gpui::test]
    fn voice_button_shows_errors_in_overlay_and_dismisses_them(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        vcx.run_until_parked();
        let button = vcx.debug_bounds("voice-toggle").unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-overlay").is_some());
        assert!(vcx.debug_bounds("voice-stop").is_none());
        let cancel = vcx.debug_bounds("voice-cancel").unwrap();
        vcx.simulate_click(cancel.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-overlay").is_none());
        panel.read_with(vcx, |panel, _| assert!(panel.voice.error.is_none()));
    }
}
