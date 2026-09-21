//! Lift the owning composer into the window center while speech is streaming.
use super::*;

fn lift_geometry(
    viewport: gpui::Size<gpui::Pixels>,
    origin: Option<gpui::Bounds<gpui::Pixels>>,
    target_width: gpui::Pixels,
    progress: f32,
) -> (gpui::Pixels, gpui::Point<gpui::Pixels>) {
    let remaining = 1.0 - crate::transition::ease_out_cubic(progress);
    let width = target_width
        + origin.map_or(px(0.), |b| {
            b.size.width.min(viewport.width - px(32.)) - target_width
        }) * remaining;
    let offset = origin.map_or(gpui::point(px(0.), px(0.)), |b| {
        gpui::point(
            (b.center().x - viewport.width / 2.) * remaining,
            (b.center().y - viewport.height / 2.) * remaining,
        )
    });
    (width, offset)
}

impl Panel {
    pub(in crate::panel) fn render_voice_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.input.update(cx, |input, cx| {
            input.set_voice_preview(
                self.voice_active()
                    .then(|| self.voice.live_transcript.clone()),
                cx,
            );
        });
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
            Phase::Recording if self.voice.hold_capture => {
                format!("Audio streams to Nari · Release {VOICE_SHORTCUT} to finish transcription")
            }
            Phase::Recording => "Audio streams to Nari · Click Stop to finish".into(),
            Phase::Checking => "Connecting to Nari and checking microphone access".into(),
            Phase::Transcribing => "Your words will not be sent automatically".into(),
            Phase::Routing => "Jev is checking your last 20 sessions".into(),
            Phase::Idle => String::new(),
        });
        let viewport = window.viewport_size();
        let target_width = (viewport.width - px(32.)).max(px(160.)).min(px(640.));
        let duration = jcode_desktop_motion::policy(
            crate::transition::Transition::PanelOpen,
            crate::config::get().appearance.reduce_motion || cx.reduce_motion(),
        )
        .duration;
        let progress = self.voice.entrance.map_or(1.0, |started| {
            if duration.is_zero() {
                1.0
            } else {
                (started.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.0)
            }
        });
        if progress < 1.0 {
            window.request_animation_frame();
        }
        let (width, offset) = lift_geometry(viewport, self.voice.origin, target_width, progress);
        let card = div()
            .id("voice-overlay")
            .debug_selector(|| "voice-overlay".into())
            .relative()
            .left(offset.x)
            .top(offset.y)
            .w(width)
            .max_h((viewport.height - px(32.)).max(px(160.)))
            .overflow_y_scroll()
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
            .when(self.voice_active(), |el| el.child(self.input.clone()))
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
        // Only the lifted composer captures input. The original slot retains
        // its space so the transcript does not jump underneath the animation.
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::point(px(0.), px(0.)))
                    .child(
                        div()
                            .w(viewport.width)
                            .h(viewport.height)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(card),
                    ),
            )
            .with_priority(90)
            .into_any_element(),
        )
    }

    pub(in crate::panel) fn render_voice_input_slot(&mut self, cx: &App) -> gpui::AnyElement {
        if self.voice_active() {
            if self.voice.origin.is_none() {
                self.voice.origin = self.input.read(cx).voice_bounds();
            }
            div()
                .debug_selector(|| "voice-input-origin".into())
                .w_full()
                .h(self.voice.origin.map_or(px(100.), |b| b.size.height))
                .into_any_element()
        } else {
            self.input.clone().into_any_element()
        }
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

    #[test]
    fn voice_lift_starts_at_input_and_settles_at_window_center() {
        let viewport = gpui::size(px(1440.), px(1000.));
        let origin = gpui::Bounds::new(
            gpui::point(px(300.), px(800.)),
            gpui::size(px(900.), px(100.)),
        );
        let (width, offset) = lift_geometry(viewport, Some(origin), px(640.), 0.);
        assert_eq!(width, origin.size.width);
        assert_eq!(offset, gpui::point(px(30.), px(350.)));
        let (_, halfway) = lift_geometry(viewport, Some(origin), px(640.), 0.5);
        assert!(halfway.y > px(0.) && halfway.y < offset.y);
        assert_eq!(
            lift_geometry(viewport, Some(origin), px(640.), 1.),
            (px(640.), gpui::point(px(0.), px(0.)))
        );
        assert_eq!(
            lift_geometry(viewport, None, px(640.), 0.),
            (px(640.), gpui::point(px(0.), px(0.)))
        );
    }

    #[gpui::test]
    fn voice_overlay_centers_the_actual_composer_without_moving_footer(
        cx: &mut gpui::TestAppContext,
    ) {
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
                    (card.center().y - px(height / 2.)).abs() <= px(1.),
                    "{card:?}"
                );
                assert!(card.left() >= px(0.) && card.right() <= px(width));
                assert!(card.top() >= px(0.));
                let input = vcx
                    .debug_bounds("prompt-input")
                    .expect("actual composer is lifted");
                assert!(input.left() >= card.left() && input.right() <= card.right());
                assert!(input.top() >= card.top() && input.bottom() <= card.bottom());
                assert!(vcx.debug_bounds("voice-input-origin").is_some());
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
