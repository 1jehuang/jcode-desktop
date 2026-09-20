use super::*;
use std::{cell::Cell, rc::Rc};

/// The list's logical offset is an end sentinel while following the tail, not
/// the first visible row. Observe real painted bounds instead of that offset.
pub(super) fn visibility_marker(
    row: usize,
    first_visible: Rc<Cell<Option<(usize, usize)>>>,
    list: ListState,
) -> gpui::AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, _, _| {
            let viewport = list.viewport_bounds();
            if bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom() {
                first_visible.set(Some(
                    first_visible
                        .get()
                        .map_or((row, row), |(first, last)| (first.min(row), last.max(row))),
                ));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Pin only the prompt for the turn at the top of the painted viewport.
/// A prompt that is itself still visible needs no duplicate card.
fn prompt_for_viewport(
    prompt_rows: &[(usize, usize)],
    first_visible: Option<usize>,
) -> Option<usize> {
    let first = first_visible?;
    let &(row, index) = prompt_rows.iter().rev().find(|(row, _)| *row <= first)?;
    (row < first).then_some(index)
}

pub(super) fn is_pinnable_prompt(item: &Item) -> bool {
    let Item::User(text) = item else {
        return false;
    };
    let text = text.trim();
    // Background notifications can arrive in restored history as user-role
    // markdown. Keep them in the transcript, never in the pinned reminder.
    !text.is_empty()
        && ![
            "**Background task** `",
            "**Background task started** `",
            "**Background task progress** `",
            "**Background task stalled** `",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

impl Panel {
    /// Share text, selection, and acknowledgement behavior, but keep the pinned
    /// reminder unhighlighted against the transcript's normal backdrop.
    pub(super) fn render_user_prompt(
        &self,
        index: usize,
        text: &str,
        highlighted: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = Instant::now();
        let pending = self.pending_users.contains(&index);
        let (offset, opacity, animating) = self
            .accepted_users
            .get(&index)
            .map(|at| crate::ack::motion(*at, now))
            .unwrap_or((
                0.0,
                if pending {
                    crate::ack::PENDING_TONE
                } else {
                    1.0
                },
                false,
            ));
        if animating {
            window.request_animation_frame();
        }
        let card = div()
            .debug_selector(move || format!("user-prompt-{index}").into())
            .max_w_full()
            .flex_none()
            .flex()
            .flex_col()
            .ml(px(offset))
            .opacity(opacity)
            .when(highlighted, |card| card.bg(Theme::global().USER_BG))
            .rounded_md()
            .px_3()
            .py_2()
            .text_color(Theme::global().TEXT_USER)
            .child(markdown::render_interactive(
                text,
                index,
                &self.transcript_selection,
                window,
                cx,
                false,
                self.media_preview_handler(cx),
            ))
            .into_any_element();
        // Keep the row full width, but let its card use its intrinsic text width.
        // The maximum width still constrains long prompts and rich markdown.
        div()
            .flex()
            .flex_none()
            .flex_col()
            .items_start()
            .child(card)
            .into_any_element()
    }

    /// Keep the reminder out of flex layout. Inserting it above the list moves
    /// every visible row by its height at the pin boundary, and also changes
    /// the viewport used to decide whether the original prompt is visible.
    pub(super) fn render_pinned_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let index = self.offscreen_prompt?;
        let item = self.items.get(index)?.clone();
        if !is_pinnable_prompt(&item) {
            return None;
        }
        let Item::User(text) = &item else {
            return None;
        };
        Some(
            div()
                .id("pinned-latest-prompt")
                .debug_selector(|| "pinned-latest-prompt".into())
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .min_w_0()
                .max_h(px(
                    (f32::from(window.viewport_size().height) * 0.2).min(180.)
                ))
                .overflow_y_scroll()
                .bg(Theme::global().PANEL_BG)
                .occlude()
                .px_3()
                .pt_2p5()
                .pb_2()
                .text_size(px(13.5))
                .child(self.render_user_prompt(index, text, false, window, cx))
                .into_any_element(),
        )
    }

    /// Read visibility after the list paints. Measuring during render would
    /// still describe the previous scroll position, width, or transcript.
    pub(super) fn prompt_visibility_observer(
        &self,
        prompt_rows: Vec<(usize, usize)>,
        first_visible: Rc<Cell<Option<(usize, usize)>>>,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let panel = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, _, cx| {
                let visible = first_visible.take();
                let has_visible_prompt = visible.is_some_and(|(first, last)| {
                    prompt_rows
                        .iter()
                        .any(|(row, _)| *row >= first && *row <= last)
                });
                let offscreen = if has_visible_prompt {
                    None
                } else {
                    prompt_for_viewport(&prompt_rows, visible.map(|(first, _)| first))
                };
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |panel, cx| {
                        if panel.offscreen_prompt != offscreen {
                            panel.offscreen_prompt = offscreen;
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKGROUND_NOTICES: [&str; 4] = [
        "**Background task started** `task-42` · `Workspace tests`",
        "**Background task progress** `task-42` · `Workspace tests` (`bash`)\n\n35% · Running tests",
        "**Background task** `task-42` · `Workspace tests` (`bash`) · ✓ completed · 8.2s · exit 0",
        "**Background task stalled** `task-42` · `Workspace tests` (`bash`) · no output or progress for 30s (running 60s total)",
    ];

    #[test]
    fn background_notifications_are_never_pinnable_prompts() {
        for notice in BACKGROUND_NOTICES {
            assert!(!is_pinnable_prompt(&Item::User(format!("\n{notice}\n"))));
        }
        assert!(!is_pinnable_prompt(&Item::User("  ".into())));
        assert!(is_pinnable_prompt(&Item::User(
            "Explain background tasks".into()
        )));
        assert!(!is_pinnable_prompt(&Item::BackgroundTask {
            task_id: "task-42".into(),
            label: "Workspace tests".into(),
            summary: "Running tests".into(),
            percent: Some(35.),
            done: false,
        }));
    }

    #[gpui::test]
    fn background_notifications_stay_in_the_scroller(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "background-no-pin".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(400.)));
        for notice in BACKGROUND_NOTICES {
            for real_prompt in [false, true] {
                panel.update(vcx, |panel, cx| {
                    panel.items.clear();
                    if real_prompt {
                        panel.items.push(Item::User("Run the tests".into()));
                    }
                    panel.items.push(Item::User(notice.into()));
                    for i in 0..40 {
                        panel
                            .items
                            .push(Item::Assistant(format!("Response paragraph {i}")));
                    }
                    cx.notify();
                });
                vcx.run_until_parked();
                panel.read_with(vcx, |panel, _| {
                    assert_eq!(panel.offscreen_prompt, real_prompt.then_some(0));
                    assert!(
                        panel.transcript_render_rows().iter().any(|row| {
                            matches!(row.source, TranscriptRowSource::Settled(index)
                            if matches!(&panel.items[index], Item::User(text) if text == notice))
                        }),
                        "background output remains in transcript history"
                    );
                });
                assert_eq!(
                    vcx.debug_bounds("pinned-latest-prompt").is_some(),
                    real_prompt
                );
            }
        }
    }

    #[test]
    fn viewport_prompt_never_comes_from_a_future_turn() {
        let prompts = [(0, 0), (12, 16), (30, 40)];
        assert_eq!(prompt_for_viewport(&prompts, None), None);
        assert_eq!(prompt_for_viewport(&prompts, Some(0)), None);
        assert_eq!(prompt_for_viewport(&prompts, Some(5)), Some(0));
        assert_eq!(prompt_for_viewport(&prompts, Some(12)), None);
        assert_eq!(prompt_for_viewport(&prompts, Some(20)), Some(16));
        assert_eq!(prompt_for_viewport(&prompts, Some(35)), Some(40));
    }

    #[gpui::test]
    fn prompt_cards_fit_content_without_captions(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "compact-prompts".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(800.)));
        for width in [600., 320.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(800.)));
            for text in [
                "Hi".to_string(),
                "A longer prompt with **formatted text** that should wrap naturally. ".repeat(5),
            ] {
                panel.update(vcx, |panel, cx| {
                    panel.items = vec![Item::User(text.clone())];
                    cx.notify();
                });
                vcx.run_until_parked();
                let card = vcx.debug_bounds("user-prompt-0").expect("prompt paints");
                let viewport = vcx.debug_bounds("transcript").unwrap();
                assert!(card.left() >= viewport.left());
                assert!(
                    card.right() <= viewport.right(),
                    "prompt stays within chat width"
                );
                assert!(vcx.debug_bounds("role-caption-you, 1").is_none());
                if text == "Hi" {
                    assert!(
                        card.size.width < px(100.),
                        "short prompt hugs its content: {card:?}"
                    );
                } else {
                    assert!(card.size.height > px(50.), "long prompt wraps");
                }
            }
        }
    }

    #[gpui::test]
    fn native_scroll_paints_historical_prompt_cards(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "native-prompt-acceptance".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(500.)));
        panel.update(vcx, |panel, cx| {
            for number in 1..=13 {
                panel
                    .items
                    .push(Item::User(format!("Distinct prompt {number}")));
                for line in 0..40 {
                    panel.items.push(Item::Assistant(format!(
                        "Answer {number}, paragraph {line}"
                    )));
                }
            }
            cx.notify();
        });
        vcx.run_until_parked();
        let pinned = vcx
            .debug_bounds("pinned-latest-prompt")
            .expect("tail prompt paints");
        let caption = vcx
            .debug_bounds("user-prompt-492")
            .expect("prompt card paints");
        assert!(caption.top() >= pinned.top() && caption.bottom() <= pinned.bottom());
        println!("ACCEPTANCE: live-end pinned card paints prompt 13");

        // Enter through the actual scroll event handler, not ListState mutation.
        let mut found_historical = false;
        for _ in 0..80 {
            let position = vcx.debug_bounds("transcript").unwrap().center();
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(180.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            vcx.run_until_parked();
            scroll_momentum_tests::settle(vcx);
            if let (Some(pinned), Some(caption)) = (
                vcx.debug_bounds("pinned-latest-prompt"),
                vcx.debug_bounds("user-prompt-451"),
            ) {
                assert!(caption.top() >= pinned.top() && caption.bottom() <= pinned.bottom());
                assert!(
                    vcx.debug_bounds("user-prompt-492").is_none(),
                    "newest prompt must not replace historical context"
                );
                found_historical = true;
                break;
            }
        }
        assert!(
            found_historical,
            "native upward scrolling must paint the twelfth prompt"
        );
        println!("ACCEPTANCE: native upward scrolling replaces pinned prompt 13 with prompt 12");
        panel.update(vcx, |panel, cx| {
            assert!(!panel.stick_to_bottom);
            panel
                .items
                .push(Item::User("A newer prompt arrives".into()));
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("user-prompt-451").is_some());
        assert!(vcx.debug_bounds("user-prompt-533").is_none());
        println!("ACCEPTANCE: arrival of prompt 14 leaves viewed prompt 12 pinned");
        for (delta, label) in [(180., "user-prompt-0"), (-180., "user-prompt-533")] {
            for _ in 0..250 {
                let position = vcx.debug_bounds("transcript").unwrap().center();
                vcx.simulate_event(gpui::ScrollWheelEvent {
                    position,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(delta))),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                });
                vcx.run_until_parked();
                scroll_momentum_tests::settle(vcx);
                if vcx.debug_bounds(label).is_some()
                    && vcx.debug_bounds("pinned-latest-prompt").is_none()
                {
                    break;
                }
            }
            assert!(
                vcx.debug_bounds(label).is_some(),
                "transcript card {label} paints"
            );
            assert!(
                vcx.debug_bounds("pinned-latest-prompt").is_none(),
                "visible user card must not be duplicated"
            );
        }
        println!(
            "ACCEPTANCE: native top/bottom scrolling paints prompt 1 and prompt 14 without duplicate pinned cards"
        );
    }

    #[gpui::test]
    fn scrolling_history_pins_the_historical_turn(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "historical-prompts".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(vcx, |panel, cx| {
            for turn in 0..3 {
                panel.items.push(Item::User(format!("Prompt {turn}")));
                for line in 0..40 {
                    panel
                        .items
                        .push(Item::Assistant(format!("Turn {turn} response {line}")));
                }
            }
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, _| assert_eq!(panel.offscreen_prompt, Some(82)));
        for (row, expected) in [(50, Some(41)), (10, Some(0)), (0, None)] {
            panel.update(vcx, |panel, cx| {
                panel.stick_to_bottom = false;
                panel.transcript_list.scroll_to(gpui::ListOffset {
                    item_ix: row,
                    offset_in_item: px(0.),
                });
                cx.notify();
            });
            vcx.run_until_parked();
            panel.update(vcx, |panel, _| assert_eq!(panel.offscreen_prompt, expected));
        }
    }

    #[gpui::test]
    fn pinned_prompt_transition_does_not_move_the_transcript(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "prompt-transition".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(500.)));
        panel.update(vcx, |panel, cx| {
            panel.items.push(Item::User("First prompt".into()));
            for n in 0..40 {
                panel
                    .items
                    .push(Item::Assistant(format!("Response paragraph {n}")));
            }
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| {
            panel.stick_to_bottom = false;
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        let viewport = vcx.debug_bounds("transcript").unwrap();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_none());

        for (delta, expected_pin) in [(-4., true), (4., false)] {
            let mut crossed = false;
            for _ in 0..80 {
                let before = vcx.debug_bounds("transcript-row-1").unwrap().top();
                vcx.simulate_event(gpui::ScrollWheelEvent {
                    position: viewport.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(delta))),
                    modifiers: Default::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                });
                vcx.run_until_parked();
                scroll_momentum_tests::settle(vcx);
                assert_eq!(
                    vcx.debug_bounds("transcript").unwrap(),
                    viewport,
                    "pinning must not resize or move the scroll viewport"
                );
                let after = vcx.debug_bounds("transcript-row-1").unwrap().top();
                assert!(
                    (f32::from(after - before) - delta).abs() < 0.5,
                    "text must move only by the scroll delta, not the header height"
                );
                if vcx.debug_bounds("pinned-latest-prompt").is_some() == expected_pin {
                    crossed = true;
                    break;
                }
            }
            assert!(crossed, "native scrolling must cross the pin boundary");
            for _ in 0..4 {
                panel.update(vcx, |_, cx| cx.notify());
                vcx.run_until_parked();
                assert_eq!(vcx.debug_bounds("transcript").unwrap(), viewport);
                assert_eq!(
                    vcx.debug_bounds("pinned-latest-prompt").is_some(),
                    expected_pin
                );
            }
        }
    }

    #[gpui::test]
    fn pinned_prompt_tracks_scrolling_and_new_prompts(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "prompt-visibility".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::User("Keep the normal **prompt card**".into())];
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_none());

        // With no todos, a long response still pins the offscreen prompt.
        panel.update(vcx, |panel, cx| {
            for i in 0..40 {
                panel
                    .items
                    .push(Item::Assistant(format!("Response paragraph {i}")));
            }
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_some());

        // Scrolling back to even part of the original removes the reminder.
        panel.update(vcx, |panel, cx| {
            panel.stick_to_bottom = false;
            panel.transcript_list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(15.),
            });
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("transcript-row-0").is_some());
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_none());

        panel.update(vcx, |panel, cx| {
            panel.stick_to_bottom = true;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_some());

        panel.update(vcx, |panel, cx| {
            panel.items.push(Item::User("A new visible prompt".into()));
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_none());
    }

    #[gpui::test]
    fn pinned_prompt_card_is_below_todos_and_resize_aware(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "prompt-todos".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(400.)));
        panel.update(vcx, |panel, cx| {
            panel.items = vec![
                Item::User("Keep the prompt below the tasks".into()),
                Item::Todos(serde_json::from_value(serde_json::json!({
                    "todos": [{"id":"one", "content":"Verify visibility", "status":"in_progress", "priority":"high"}]
                })).unwrap()),
            ];
            for i in 0..20 {
                panel.items.push(Item::Assistant(format!("Response paragraph {i}")));
            }
            cx.notify();
        });
        vcx.run_until_parked();
        let todos = vcx.debug_bounds("pinned-todo-card").unwrap();
        let prompt = vcx.debug_bounds("pinned-latest-prompt").unwrap();
        let transcript = vcx.debug_bounds("transcript").unwrap();
        assert!(todos.bottom() <= prompt.top());
        assert_eq!(prompt.top(), transcript.top());
        assert!(prompt.bottom() < transcript.bottom());

        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(1600.)));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_none());
        assert!(vcx.debug_bounds("pinned-todo-card").is_some());
    }
}
