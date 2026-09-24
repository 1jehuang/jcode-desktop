//! A bottom-of-window microphone meter, independent of the draft composer.
use super::*;

#[path = "panel_voice_pills.rs"]
mod pills;

fn meter_height(level: f32) -> f32 {
    // Speech RMS is far below full scale. Compress the visual range so quiet
    // speech is visible without making silence look like incoming audio.
    2.0 + (level.max(0.0) * 4.0).sqrt().min(1.0) * 16.0
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
        // A global hold's status is shown by the OS pill while this window is
        // unfocused. Never show a second, in-panel copy at the same time.
        if self.voice.global_capture && !window.is_window_active() {
            return None;
        }
        if self.voice.trace.is_some() && !self.voice.trace_expanded {
            return Some(self.render_voice_pills(window, cx));
        }
        if !self.voice_active()
            && self.voice.error.is_none()
            && self.voice.decision.is_none()
            && self.voice.trace.is_none()
        {
            return None;
        }
        if self.voice.trace.is_some() {
            return Some(self.render_voice_trace(window, cx));
        }
        let theme = Theme::global();
        let phase = self.voice.phase;
        let title = match phase {
            Phase::Idle => "Voice unavailable",
            Phase::Checking => "Connecting…",
            Phase::Recording => "Listening",
            Phase::Transcribing => "Transcribing…",
            Phase::Routing => "Jev is choosing…",
        };
        let viewport = window.viewport_size();
        let card = div()
            .id("voice-overlay")
            .debug_selector(|| "voice-overlay".into())
            .when(phase == Phase::Idle, |el| {
                el.w((viewport.width - px(32.)).min(px(360.)))
                    .min_h(px(44.))
                    .px_3()
                    .py_2()
            })
            // Active capture hugs its content: waveform or status plus controls.
            .when(phase != Phase::Idle, |el| {
                el.max_w(viewport.width - px(32.))
                    .h(px(30.))
                    .pl(px(12.))
                    .pr(px(4.))
            })
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .rounded_full()
            .when(phase == Phase::Idle, |el| el.rounded_xl())
            .border_1()
            .border_color(theme.ACCENT.opacity(0.25))
            .bg(theme.PANEL_BG)
            .text_color(theme.TEXT)
            .text_size(px(11.))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .when(phase == Phase::Recording, |el| {
                el.child(
                    div()
                        .debug_selector(|| "voice-waveform".into())
                        .flex_none()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(1.5))
                        .children(self.voice.levels.iter().map(|level| {
                            div()
                                .w(px(2.))
                                .h(px(meter_height(*level)))
                                .rounded_full()
                                .bg(theme.ACCENT.opacity(0.9))
                        })),
                )
            })
            .when(
                phase == Phase::Idle && self.voice.error.is_none() && self.voice.decision.is_some(),
                |el| {
                    let decision = self.voice.decision.as_deref().unwrap_or_default();
                    let (route, detail) = decision.split_once(" · ").unwrap_or((decision, ""));
                    el.child(
                        div()
                            .debug_selector(|| "voice-decision".into())
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.TEXT)
                                    .child(route.to_string()),
                            )
                            .child(div().text_color(theme.TEXT_DIM).child(detail.to_string())),
                    )
                },
            )
            .when(
                phase != Phase::Recording
                    && (phase != Phase::Idle
                        || self.voice.error.is_some()
                        || self.voice.decision.is_none()),
                |el| {
                    el.child(
                        div()
                            .debug_selector(|| "voice-status".into())
                            .when(phase == Phase::Idle, |el| el.flex_1())
                            .min_w_0()
                            .whitespace_nowrap()
                            .truncate()
                            .text_color(theme.TEXT_DIM)
                            .child(
                                self.voice
                                    .error
                                    .clone()
                                    .or_else(|| self.voice.decision.clone())
                                    .unwrap_or_else(|| title.into()),
                            ),
                    )
                },
            )
            .when(phase == Phase::Recording, |el| {
                // While listening, the only control finishes the capture and
                // sends the transcription, so a mouse-started recording can end.
                el.child(
                    div()
                        .id("voice-stop")
                        .debug_selector(|| "voice-stop".into())
                        .size(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(theme.ACCENT.opacity(0.15))
                        .hover(|el| el.bg(theme.ACCENT.opacity(0.3)))
                        .tooltip(|_, cx| {
                            cx.new(|_| VoiceTooltip("Stop and send transcription".into()))
                                .into()
                        })
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.stop_voice(cx);
                            cx.stop_propagation();
                        }))
                        .child(
                            div()
                                .size(px(8.))
                                .rounded(px(2.))
                                .bg(theme.ACCENT),
                        ),
                )
            })
            .when(phase != Phase::Recording, |el| {
                el.child(
                    div()
                        .id("voice-cancel")
                        .debug_selector(|| "voice-cancel".into())
                        .size(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .text_color(theme.TEXT_DIM)
                        .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
                        .tooltip(|_, cx| {
                            cx.new(|_| VoiceTooltip("Dismiss or cancel voice request".into()))
                                .into()
                        })
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.cancel_voice(cx);
                            cx.stop_propagation();
                        }))
                        .child("×"),
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

    fn render_voice_trace(&self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let trace = self
            .voice
            .trace
            .as_ref()
            .expect("trace card requires a trace");
        let theme = Theme::global();
        let viewport = window.viewport_size();
        let expanded = self.voice.trace_expanded;
        let heading = if self.voice.phase == Phase::Routing {
            "Jev · Choosing a route"
        } else if self.voice.error.is_some() {
            "Jev · Could not choose"
        } else {
            "Jev · Decision"
        };
        let mut content = div()
            .id("voice-trace-scroll")
            .debug_selector(|| "voice-trace-scroll".into())
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_color(theme.TEXT_DIM).child("You said"))
                    .child(
                        div()
                            .debug_selector(|| "voice-transcript".into())
                            .child(trace.transcript.clone()),
                    ),
            )
            .child(
                div()
                    .text_color(theme.TEXT_DIM)
                    .child("Independent yes probabilities, not shares of a total."),
            );
        let mut questions: Vec<_> = trace.questions.iter().collect();
        questions.sort_by_key(|question| match question.id.as_str() {
            "coding_agent" => 0,
            "quick_action" => 1,
            "navigation" => 2,
            "new_session" => 3,
            "next_session" => 4,
            "previous_session" => 5,
            id => {
                6 + id
                    .strip_prefix("candidate_")
                    .and_then(|index| index.parse::<usize>().ok())
                    .unwrap_or(100)
            }
        });
        for question in questions {
            let answer = trace.answers.iter().find(|answer| answer.id == question.id);
            let label = match question.id.as_str() {
                "coding_agent" => "Coding agent".to_string(),
                "navigation" => "Open an existing conversation".to_string(),
                "quick_action" => "Quick action".to_string(),
                "new_session" => "New conversation".to_string(),
                "next_session" => "Next conversation".to_string(),
                "previous_session" => "Previous conversation".to_string(),
                id => id
                    .strip_prefix("candidate_")
                    .and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| trace.candidates.get(index))
                    .map(|candidate| format!("Open: {}", candidate.title))
                    .unwrap_or_else(|| id.replace('_', " ")),
            };
            let score = answer
                .map(|answer| format!("{:.1}% yes", answer.probability * 100.))
                .unwrap_or_else(|| {
                    if self.voice.phase == Phase::Routing {
                        "Waiting…"
                    } else {
                        "Not returned"
                    }
                    .into()
                });
            let mut row = div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(theme.ACCENT_DIM.opacity(0.35))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(label))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(theme.TEXT_DIM)
                                .child(score),
                        ),
                );
            if !expanded {
                row = row.child(
                    div().text_color(theme.TEXT_DIM).child(
                        question
                            .instructions
                            .rsplit_once('\n')
                            .map(|(_, question)| question)
                            .unwrap_or(&question.instructions)
                            .to_string(),
                    ),
                );
            }
            if expanded {
                row = row
                    .child(
                        div()
                            .text_color(theme.TEXT_DIM)
                            .child(format!("Question ID: {}", question.id)),
                    )
                    .child(div().child(question.instructions.clone()))
                    .child(
                        div()
                            .text_color(theme.TEXT_DIM)
                            .child(format!("Yes: {}", question.yes)),
                    )
                    .child(
                        div()
                            .text_color(theme.TEXT_DIM)
                            .child(format!("No: {}", question.no)),
                    );
            }
            content = content.child(row);
        }
        if expanded {
            content = content.child(
                div()
                    .text_color(theme.TEXT_DIM)
                    .child("Offered conversations (newest first)"),
            );
            for (index, candidate) in trace.candidates.iter().enumerate() {
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(format!("candidate_{index}: {}", candidate.title))
                        .child(
                            div()
                                .text_color(theme.TEXT_DIM)
                                .child(format!("Session: {}", candidate.id)),
                        )
                        .child(
                            div().text_color(theme.TEXT_DIM).child(
                                candidate
                                    .working_dir
                                    .clone()
                                    .unwrap_or_else(|| "No working directory".into()),
                            ),
                        ),
                );
            }
            if trace.candidates.is_empty() {
                content = content.child("No existing conversations were offered.");
            }
        }
        let card = div()
            .id("voice-overlay")
            .debug_selector(|| "voice-overlay".into())
            .w((viewport.width - px(32.)).max(px(1.)).min(px(440.)))
            .max_h((viewport.height - px(48.)).max(px(1.)).min(px(560.)))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_xl()
            .border_1()
            .border_color(theme.ACCENT.opacity(0.25))
            .bg(theme.PANEL_BG)
            .shadow_lg()
            .text_color(theme.TEXT)
            .text_size(px(11.))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(div().flex_1().min_w_0().text_size(px(12.)).child(heading))
                    .when(self.voice.trace_preview, |el| {
                        el.child(div().text_color(theme.TEXT_DIM).child("Preview"))
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
                            .hover(|el| el.bg(theme.ACCENT_DIM))
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.cancel_voice(cx);
                                cx.stop_propagation();
                            }))
                            .child("×"),
                    ),
            )
            .child(
                div()
                    .id("voice-trace-expand")
                    .debug_selector(|| "voice-trace-expand".into())
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_color(theme.ACCENT)
                    .on_click(cx.listener(|panel, _, _, cx| {
                        panel.voice.trace_expanded = !panel.voice.trace_expanded;
                        cx.notify();
                        cx.stop_propagation();
                    }))
                    .child(if expanded {
                        "Hide full questions and candidates"
                    } else {
                        "Show full questions and candidates"
                    }),
            )
            .when_some(self.voice.error.clone(), |el, error| {
                el.child(
                    div()
                        .debug_selector(|| "voice-status".into())
                        .flex_shrink_0()
                        .child(error),
                )
            })
            .when_some(self.voice.decision.clone(), |el, decision| {
                el.child(
                    div()
                        .debug_selector(|| "voice-decision".into())
                        .flex_shrink_0()
                        .text_color(theme.ACCENT)
                        .child(decision),
                )
            })
            .child(content)
            .child(self.render_voice_costs(trace));
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
        .into_any_element()
    }

    pub(in crate::panel) fn render_voice_input_slot(&mut self, _cx: &App) -> gpui::AnyElement {
        self.input.clone().into_any_element()
    }

    pub(in crate::panel) fn seed_voice_preview(&mut self, state: PreviewState) {
        self.voice = VoiceState::default();
        if matches!(
            state,
            PreviewState::VoiceRouting
                | PreviewState::VoiceCodingAgent
                | PreviewState::VoiceQuickAction
        ) {
            let candidates = vec![
                SessionCandidate {
                    id: "preview-terminal".into(),
                    title: "Terminal rendering".into(),
                    working_dir: Some("/projects/jcode-desktop".into()),
                },
                SessionCandidate {
                    id: "preview-settings".into(),
                    title: "Settings and shortcuts".into(),
                    working_dir: Some("/projects/jcode-desktop".into()),
                },
            ];
            let coding = state == PreviewState::VoiceCodingAgent;
            let transcript = if coding {
                "Fix the terminal rendering when I resize the window."
            } else {
                "Open the terminal rendering conversation."
            }
            .to_string();
            let questions = voice_intent::describe_questions(&transcript, &candidates)
                .expect("valid deterministic voice preview");
            let answers = if state == PreviewState::VoiceRouting {
                vec![]
            } else {
                questions
                    .iter()
                    .map(|question| voice_intent::VoiceAnswer {
                        id: question.id.clone(),
                        probability: match (coding, question.id.as_str()) {
                            (true, "coding_agent") => 0.97,
                            (false, "navigation") => 0.98,
                            (false, "candidate_0") => 0.96,
                            _ => 0.02,
                        },
                    })
                    .collect()
            };
            let routed = state != PreviewState::VoiceRouting;
            self.voice.trace = Some(VoiceTrace {
                transcript,
                questions,
                answers,
                candidates,
                // Deterministic offline fixture values, never live measurements.
                audio: Some(Duration::from_millis(3400)),
                usage: routed.then_some(voice_intent::VoiceUsage {
                    input_tokens: 4_812,
                    output_tokens: 9,
                    requests: 1,
                }),
            });
            self.voice.trace_preview = true;
            if state == PreviewState::VoiceRouting {
                self.voice.phase = Phase::Routing;
            } else {
                self.voice.decision = Some(
                    if coding {
                        "Jev chose: Coding agent · Sent now"
                    } else {
                        "Jev chose: Quick action · Open Terminal rendering"
                    }
                    .into(),
                );
            }
            return;
        }
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

    #[gpui::test]
    fn jev_decision_is_visible_until_dismissed_without_reflow(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [240., 480., 1440.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            panel.update(vcx, |panel, cx| panel.cancel_voice(cx));
            vcx.run_until_parked();
            let input = vcx.debug_bounds("prompt-input").unwrap();
            for state in [
                PreviewState::VoiceCodingAgent,
                PreviewState::VoiceQuickAction,
            ] {
                panel.update(vcx, |panel, cx| {
                    panel.seed_voice_preview(state);
                    cx.notify();
                });
                vcx.run_until_parked();
                let card = vcx
                    .debug_bounds("voice-overlay")
                    .expect("decision remains visible");
                assert!(card.left() >= px(0.) && card.right() <= px(width));
                assert_eq!(vcx.debug_bounds("prompt-input").unwrap(), input);
                assert!(vcx.debug_bounds("voice-decision").is_some());
                let dismiss = vcx.debug_bounds("voice-cancel").unwrap();
                vcx.simulate_click(dismiss.center(), gpui::Modifiers::default());
                vcx.run_until_parked();
                assert!(vcx.debug_bounds("voice-overlay").is_none());
            }
        }
    }

    #[gpui::test]
    fn global_sent_pill_is_in_panel_only_while_focused(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        panel.update(vcx, |panel, cx| {
            panel.voice.global_capture = true;
            panel.voice.decision = Some("Sent to agent".into());
            cx.notify();
        });
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("voice-overlay").is_some(),
            "focused: in-panel pill"
        );
        vcx.deactivate_window();
        panel.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("voice-overlay").is_none(),
            "unfocused: only the OS pill is shown"
        );
    }

    #[gpui::test]
    fn trace_card_expands_and_stays_bounded_without_composer_reflow(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        for (width, height) in [(240., 320.), (480., 600.), (1440., 1000.)] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
            panel.update(vcx, |panel, cx| panel.cancel_voice(cx));
            vcx.run_until_parked();
            let input = vcx.debug_bounds("prompt-input").unwrap();
            for state in [
                PreviewState::VoiceRouting,
                PreviewState::VoiceCodingAgent,
                PreviewState::VoiceQuickAction,
            ] {
                panel.update(vcx, |panel, cx| {
                    panel.seed_voice_preview(state);
                    panel.voice.trace_expanded = true;
                    let trace = panel.voice.trace.as_mut().unwrap();
                    trace.candidates = (0..20)
                        .map(|index| SessionCandidate {
                            id: format!("preview-session-{index}"),
                            title: format!(
                                "Conversation {index}: terminal rendering and keyboard shortcuts"
                            ),
                            working_dir: Some("/projects/jcode-desktop".into()),
                        })
                        .collect();
                    trace.questions =
                        voice_intent::describe_questions(&trace.transcript, &trace.candidates)
                            .unwrap();
                    cx.notify();
                });
                vcx.run_until_parked();
                for expanded in [true] {
                    panel.read_with(vcx, |panel, _| {
                        assert_eq!(panel.voice.trace_expanded, expanded)
                    });
                    let card = vcx.debug_bounds("voice-overlay").unwrap();
                    assert!(
                        card.left() >= px(0.) && card.right() <= px(width),
                        "{card:?}"
                    );
                    assert!(
                        card.top() >= px(0.) && card.bottom() <= px(height),
                        "{card:?}"
                    );
                    assert!(card.size.width <= px(440.));
                    assert!(card.size.height <= px(560.));
                    assert_eq!(vcx.debug_bounds("prompt-input").unwrap(), input);
                    let scroll = vcx.debug_bounds("voice-trace-scroll").unwrap();
                    assert!(
                        scroll.top() >= card.top() && scroll.bottom() <= card.bottom(),
                        "scroll={scroll:?}, card={card:?}"
                    );
                    assert!(vcx.debug_bounds("voice-transcript").is_some());
                    let toggle = vcx.debug_bounds("voice-trace-expand").unwrap();
                    vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
                    vcx.run_until_parked();
                }
                let dismiss = vcx.debug_bounds("voice-cancel").unwrap();
                vcx.simulate_click(dismiss.center(), gpui::Modifiers::default());
                vcx.run_until_parked();
                assert!(vcx.debug_bounds("voice-overlay").is_none());
                panel.read_with(vcx, |panel, _| assert!(panel.voice.trace.is_none()));
            }
        }
    }

    #[gpui::test]
    fn trace_error_preserves_questions_without_inventing_answers(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) =
            cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::VoiceRouting, cx));
        panel.update(vcx, |panel, cx| {
            panel.voice.phase = Phase::Idle;
            panel.voice.error = Some("Jev request failed. Transcript kept in draft.".into());
            panel.voice.trace_expanded = true;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("voice-overlay").is_some());
        assert!(vcx.debug_bounds("voice-status").is_some());
        assert!(vcx.debug_bounds("voice-transcript").is_some());
        panel.read_with(vcx, |panel, _| {
            let trace = panel.voice.trace.as_ref().unwrap();
            assert!(!trace.questions.is_empty());
            assert!(trace.answers.is_empty());
        });
    }

    #[test]
    fn meter_tracks_silence_and_clamps_loud_audio() {
        assert_eq!(meter_height(0.), 2.);
        assert_eq!(meter_height(1.), 18.);
        assert_eq!(meter_height(2.), 18.);
        assert!(meter_height(0.1) > meter_height(0.01));
        assert!(meter_height(0.02) > 6.);
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
                // While listening, the overlay's single control stops and
                // sends. Other phases keep the dismiss button.
                let recording = phase == Phase::Recording;
                let control = vcx
                    .debug_bounds(if recording { "voice-stop" } else { "voice-cancel" })
                    .unwrap();
                assert!(control.bottom() <= card.bottom());
                assert_eq!(vcx.debug_bounds("voice-cancel").is_some(), !recording);
                assert_eq!(vcx.debug_bounds("voice-stop").is_some(), recording);
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
            assert_eq!(panel.input.read(cx).content.as_ref(), "Keep my draft");
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
