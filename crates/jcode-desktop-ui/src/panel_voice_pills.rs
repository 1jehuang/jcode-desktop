//! Compact concrete-action evidence, independent of the optional detailed trace.
use super::*;

// Keep the shared routing argmax order, including numeric (not lexical) candidates.
fn concrete_action_rank(id: &str) -> Option<usize> {
    match id {
        "uncertain" => Some(0),
        "coding_agent" => Some(1),
        "new_session" => Some(2),
        "next_session" => Some(3),
        "previous_session" => Some(4),
        _ => id
            .strip_prefix("candidate_")?
            .parse::<usize>()
            .ok()?
            .checked_add(5),
    }
}

fn action_label(id: &str) -> String {
    match id {
        "coding_agent" => "Coding agent".into(),
        "new_session" => "New conversation".into(),
        "next_session" => "Next conversation".into(),
        "previous_session" => "Previous conversation".into(),
        "uncertain" => "Uncertain".into(),
        _ => id.into(),
    }
}

struct ActionPill {
    id: String,
    label: String,
    score: String,
    winner: bool,
}

fn action_pills(trace: &VoiceTrace, waiting: bool, failed: bool) -> Vec<ActionPill> {
    let mut actions: Vec<_> = trace
        .questions
        .iter()
        .filter_map(|question| concrete_action_rank(&question.id).map(|rank| (rank, question)))
        .collect();
    actions.sort_by_key(|(rank, _)| *rank);
    let mut winner = None;
    let mut best = f64::NEG_INFINITY;
    if !waiting && !failed {
        for (_, question) in &actions {
            if let Some(answer) = trace.answers.iter().find(|answer| answer.id == question.id) {
                let probability = answer.probability as f64;
                if probability.is_finite() && probability > best {
                    best = probability;
                    winner = Some(question.id.as_str());
                }
            }
        }
    }
    actions
        .into_iter()
        .map(|(_, question)| {
            let score = trace
                .answers
                .iter()
                .find(|answer| answer.id == question.id)
                .filter(|answer| answer.probability.is_finite())
                .map(|answer| format!("{:.1}%", answer.probability * 100.))
                .unwrap_or_else(|| {
                    if waiting {
                        "Waiting…"
                    } else {
                        "Not returned"
                    }
                    .into()
                });
            ActionPill {
                id: question.id.clone(),
                label: question
                    .id
                    .strip_prefix("candidate_")
                    .and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| trace.candidates.get(index))
                    .map(|candidate| format!("Open: {}", candidate.title))
                    .unwrap_or_else(|| action_label(&question.id)),
                score,
                winner: winner == Some(question.id.as_str()),
            }
        })
        .collect()
}

impl Panel {
    pub(super) fn render_voice_pills(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let trace = self
            .voice
            .trace
            .as_ref()
            .expect("trace card requires a trace");
        let theme = Theme::global();
        let viewport = window.viewport_size();
        let pills = action_pills(
            trace,
            self.voice.phase == Phase::Routing,
            self.voice.error.is_some(),
        );
        let pill_row = div()
            .id("voice-action-pills")
            .debug_selector(|| "voice-action-pills".into())
            .min_h_0()
            .max_h(px(112.))
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(pills.into_iter().map(|pill| {
                let selector = format!("voice-action-{}", pill.id);
                let tooltip = format!(
                    "{} · {}{}",
                    pill.label,
                    pill.score,
                    if pill.winner {
                        " · Selected route"
                    } else {
                        ""
                    }
                );
                div()
                    .id(gpui::ElementId::Name(selector.clone().into()))
                    .debug_selector(move || selector.clone())
                    .max_w((viewport.width - px(44.)).max(px(1.)).min(px(280.)))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .border_1()
                    .border_color(if pill.winner {
                        theme.ACCENT
                    } else {
                        theme.ACCENT.opacity(0.2)
                    })
                    .bg(if pill.winner {
                        theme.ACCENT_DIM
                    } else {
                        theme.PANEL_BG
                    })
                    .text_color(if pill.winner {
                        theme.ACCENT
                    } else {
                        theme.TEXT_DIM
                    })
                    .tooltip(move |_, cx| cx.new(|_| VoiceTooltip(tooltip.clone())).into())
                    .when(pill.winner, |el| el.child(div().flex_shrink_0().child("✓")))
                    .child(div().min_w_0().truncate().child(pill.label))
                    .child(div().flex_shrink_0().child(pill.score))
            }));
        let heading = if self.voice.phase == Phase::Routing {
            "Jev · Choosing a route"
        } else if self.voice.error.is_some() {
            "Jev · Could not choose"
        } else {
            "Jev · Decision"
        };
        let card = div()
            .id("voice-overlay")
            .debug_selector(|| "voice-overlay".into())
            .w((viewport.width - px(24.)).max(px(1.)).min(px(960.)))
            .max_h((viewport.height - px(64.)).max(px(1.)).min(px(240.)))
            .p_2()
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
                            .child("Details"),
                    )
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
            .child(pill_row);
        gpui::deferred(
            gpui::anchored()
                .position(gpui::point(px(0.), px(0.)))
                .child(
                    div()
                        .w(viewport.width)
                        .h(viewport.height)
                        .pt(px(48.))
                        .flex()
                        .items_start()
                        .justify_center()
                        .child(card),
                ),
        )
        .with_priority(90)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trace_fixture() -> VoiceTrace {
        let candidates = (0..20)
            .map(|index| SessionCandidate {
                id: format!("session-{index}"),
                title: format!("Conversation {index}"),
                working_dir: None,
            })
            .collect::<Vec<_>>();
        VoiceTrace {
            transcript: "Open a conversation".into(),
            questions: voice_intent::describe_questions("Open a conversation", &candidates)
                .unwrap(),
            answers: Vec::new(),
            candidates,
        }
    }

    #[test]
    fn concrete_pills_show_actual_scores_and_exclude_diagnostic_families() {
        let mut trace = trace_fixture();
        trace.answers = vec![
            voice_intent::VoiceAnswer {
                id: "coding_agent".into(),
                probability: 0.876,
            },
            voice_intent::VoiceAnswer {
                id: "navigation".into(),
                probability: 0.99,
            },
            voice_intent::VoiceAnswer {
                id: "quick_action".into(),
                probability: 1.0,
            },
            voice_intent::VoiceAnswer {
                id: "candidate_19".into(),
                probability: 0.91,
            },
        ];
        let pills = action_pills(&trace, false, false);
        assert_eq!(pills.len(), 25);
        assert!(!pills
            .iter()
            .any(|p| p.id == "navigation" || p.id == "quick_action"));
        let coding = pills.iter().find(|p| p.id == "coding_agent").unwrap();
        assert_eq!(coding.label, "Coding agent");
        assert_eq!(coding.score, "87.6%");
        assert!(!coding.winner);
        let winners: Vec<_> = pills.iter().filter(|p| p.winner).collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(winners[0].id, "candidate_19");
        assert_eq!(winners[0].score, "91.0%");
        assert_eq!(winners[0].label, "Open: Conversation 19");
        assert_eq!(pills[2].label, "New conversation");
        assert_eq!(pills[3].label, "Next conversation");
        assert_eq!(pills[4].label, "Previous conversation");
    }

    #[test]
    fn pill_ties_use_shared_argmax_order_not_question_or_answer_order() {
        let mut trace = trace_fixture();
        trace.questions.reverse();
        let ids = [
            "uncertain",
            "coding_agent",
            "new_session",
            "next_session",
            "previous_session",
            "candidate_2",
            "candidate_10",
        ];
        for start in 0..ids.len() {
            trace.answers = ids[start..]
                .iter()
                .rev()
                .map(|id| voice_intent::VoiceAnswer {
                    id: (*id).into(),
                    probability: 0.8,
                })
                .collect();
            let pills = action_pills(&trace, false, false);
            assert_eq!(pills.iter().find(|p| p.winner).unwrap().id, ids[start]);
            assert_eq!(pills.iter().filter(|p| p.winner).count(), 1);
        }
    }

    #[test]
    fn waiting_and_error_pills_never_invent_confidence_or_a_winner() {
        let trace = trace_fixture();
        for (waiting, failed, label) in [(true, false, "Waiting…"), (false, true, "Not returned")]
        {
            let pills = action_pills(&trace, waiting, failed);
            assert_eq!(pills.len(), 25);
            assert!(pills.iter().all(|p| p.score == label && !p.winner));
        }
        let mut trace = trace;
        trace.answers.push(voice_intent::VoiceAnswer {
            id: "coding_agent".into(),
            probability: 0.9,
        });
        assert!(action_pills(&trace, true, false).iter().all(|p| !p.winner));
        assert!(action_pills(&trace, false, true).iter().all(|p| !p.winner));
    }

    #[gpui::test]
    fn compact_pills_stay_bounded_and_details_remain_available(cx: &mut gpui::TestAppContext) {
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
                for expanded in [false, true, false] {
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
                    assert!(card.size.width <= px(960.));
                    if !expanded {
                        assert_eq!(card.top(), px(48.));
                    }
                    assert!(card.size.height <= px(560.));
                    assert_eq!(vcx.debug_bounds("prompt-input").unwrap(), input);
                    if !expanded {
                        let pills = vcx.debug_bounds("voice-action-pills").unwrap();
                        assert!(pills.size.height <= px(112.));
                        assert!(pills.left() >= card.left() && pills.right() <= card.right());
                        assert!(vcx.debug_bounds("voice-action-candidate_19").is_some());
                        assert!(vcx.debug_bounds("voice-action-navigation").is_none());
                        assert!(vcx.debug_bounds("voice-action-quick_action").is_none());
                    }
                    assert_eq!(vcx.debug_bounds("voice-transcript").is_some(), expanded);
                    if expanded {
                        let scroll = vcx.debug_bounds("voice-trace-scroll").unwrap();
                        assert!(
                            scroll.top() >= card.top() && scroll.bottom() <= card.bottom(),
                            "scroll={scroll:?}, card={card:?}"
                        );
                    }
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
}
