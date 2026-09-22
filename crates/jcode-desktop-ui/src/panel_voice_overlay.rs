//! A bottom-of-window microphone meter, independent of the draft composer.
use super::*;

#[path = "panel_voice_pills.rs"]
mod pills;

fn meter_height(level: f32) -> f32 {
    // Speech RMS is far below full scale. Compress the visual range so quiet
    // speech is visible without making silence look like incoming audio.
    3.0 + (level.max(0.0) * 4.0).sqrt().min(1.0) * 19.0
}

impl Panel {
    pub(in crate::panel) fn render_voice_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        // Streaming words never resize, move, or replace the draft editor.
        self.input
            .update(cx, |input, cx| input.set_voice_preview(None, cx));
        if self.voice.trace.is_some() && !self.voice.trace_expanded {
            return Some(self.render_voice_pills(window, cx));
        }
        if !self.voice_active() && self.voice.error.is_none() {
            return None;
        }
        let theme = Theme::global();
        let phase = self.voice.phase;
        let title = match phase {
            Phase::Idle => "Voice unavailable",
            Phase::Checking => "Connecting…",
            Phase::Recording => "Listening",
            Phase::Transcribing => "Transcribing…",
            Phase::Routing => "Understanding…",
        };
        let viewport = window.viewport_size();
        let card = div()
            .id("voice-overlay")
            .debug_selector(|| "voice-overlay".into())
            .w((viewport.width - px(32.)).min(px(if phase == Phase::Idle { 360. } else { 196. })))
            .min_h(px(44.))
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded_full()
            .when(phase == Phase::Idle, |el| el.rounded_xl())
            .border_1()
            .border_color(theme.ACCENT.opacity(0.25))
            .bg(theme.PANEL_BG)
            .shadow_lg()
            .text_color(theme.TEXT)
            .text_size(px(11.))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .when(phase == Phase::Recording, |el| {
                el.child(
                    div()
                        .debug_selector(|| "voice-waveform".into())
                        .flex_1()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(2.))
                        .children(self.voice.levels.iter().map(|level| {
                            div()
                                .w(px(2.))
                                .h(px(meter_height(*level)))
                                .rounded_full()
                                .bg(theme.ACCENT.opacity(0.9))
                        })),
                )
            })
            .when(phase != Phase::Recording, |el| {
                el.child(
                    div()
                        .debug_selector(|| "voice-status".into())
                        .flex_1()
                        .min_w_0()
                        .text_color(theme.TEXT_DIM)
                        .child(self.voice.error.clone().unwrap_or_else(|| title.into())),
                )
            })
            .child(
                div()
                    .id("voice-cancel")
                    .debug_selector(|| "voice-cancel".into())
                    .size(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .text_color(theme.TEXT_DIM)
                    .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
                    .tooltip(|_, cx| {
                        cx.new(|_| VoiceTooltip("Cancel recording · Keep draft unchanged".into()))
                            .into()
                    })
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.cancel_voice(cx);
                        cx.stop_propagation();
                    }))
                    .child("×"),
            )
            .when(phase == Phase::Recording, |el| {
                el.child(
                    div()
                        .id("voice-stop")
                        .debug_selector(|| "voice-stop".into())
                        .size(px(24.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(theme.ACCENT_DIM)
                        .text_color(theme.ACCENT)
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                VoiceTooltip("Finish transcription · Insert into draft".into())
                            })
                            .into()
                        })
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.stop_voice(cx);
                            cx.stop_propagation();
                        }))
                        .child("■"),
                )
            });
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::point(px(0.), px(0.)))
                    .child(
                        div()
                            .w(viewport.width)
                            .h(viewport.height)
                            .pb(px(24.))
                            .flex()
                            .items_end()
                            .justify_center()
                            .child(card),
                    ),
            )
            .with_priority(90)
            .into_any_element(),
        )
    }

    pub(in crate::panel) fn render_voice_input_slot(&mut self, _cx: &App) -> gpui::AnyElement {
        self.input.clone().into_any_element()
    }

    pub(in crate::panel) fn seed_voice_preview(&mut self, state: PreviewState) {
        self.voice = VoiceState::default();
        self.voice.hold_capture = true;
        self.voice.phase = if state == PreviewState::VoiceConnecting {
            Phase::Checking
        } else {
            Phase::Recording
        };
        if state == PreviewState::VoiceListening {
            self.voice.started = Some(Instant::now());
            // Deterministic offline microphone fixture, never used in live capture.
            self.voice.levels =
                std::array::from_fn(|i| ((i as f32 * 0.7).sin().abs() * 0.2) + 0.002);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_tracks_silence_and_clamps_loud_audio() {
        assert_eq!(meter_height(0.), 3.);
        assert_eq!(meter_height(1.), 22.);
        assert_eq!(meter_height(2.), 22.);
        assert!(meter_height(0.1) > meter_height(0.01));
        assert!(meter_height(0.02) > 8.);
    }
    #[gpui::test]
    fn voice_overlay_stays_at_bottom_without_moving_composer(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        for (width, height) in [(240., 320.), (480., 600.), (1440., 1000.)] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
            panel.update(vcx, |panel, cx| panel.cancel_voice(cx));
            vcx.run_until_parked();
            let footer = vcx.debug_bounds("panel-meta").unwrap();
            let original_input = vcx.debug_bounds("prompt-input").unwrap();
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
                assert_eq!(vcx.debug_bounds("prompt-input").unwrap(), original_input);
                assert!(vcx.debug_bounds("voice-live-transcript").is_none());
                assert_eq!(
                    vcx.debug_bounds("voice-waveform").is_some(),
                    phase == Phase::Recording
                );
                assert!(card.size.width <= px(196.));
                assert!(card.size.height <= px(48.));
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
        assert!(vcx.debug_bounds("voice-live-transcript").is_none());
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
                "Keep my draft\nFinal words."
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
