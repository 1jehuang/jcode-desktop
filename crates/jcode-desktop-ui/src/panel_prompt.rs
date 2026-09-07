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

pub(super) fn user_prompt_label(items: &[Item], index: usize) -> String {
    let number = items
        .iter()
        .take(index.saturating_add(1))
        .filter(|item| matches!(item, Item::User(_)))
        .count();
    format!("you, {number}")
}

impl Panel {
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

    #[test]
    fn user_cards_are_numbered_including_consecutive_prompts() {
        let items = vec![
            Item::User("first".into()),
            Item::Assistant("reply".into()),
            Item::User("second".into()),
            Item::User("third".into()),
        ];
        assert_eq!(user_prompt_label(&items, 0), "you, 1");
        assert_eq!(user_prompt_label(&items, 2), "you, 2");
        assert_eq!(user_prompt_label(&items, 3), "you, 3");
    }

    #[gpui::test]
    fn native_scroll_paints_historical_numbered_prompt_cards(cx: &mut gpui::TestAppContext) {
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
            .debug_bounds("role-caption-you, 13")
            .expect("exact requested numbered label paints");
        assert!(caption.top() >= pinned.top() && caption.bottom() <= pinned.bottom());
        println!("ACCEPTANCE: live-end pinned card paints you, 13");

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
            if let (Some(pinned), Some(caption)) = (
                vcx.debug_bounds("pinned-latest-prompt"),
                vcx.debug_bounds("role-caption-you, 12"),
            ) {
                assert!(caption.top() >= pinned.top() && caption.bottom() <= pinned.bottom());
                assert!(
                    vcx.debug_bounds("role-caption-you, 13").is_none(),
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
        println!("ACCEPTANCE: native upward scrolling replaces pinned you, 13 with you, 12");
        panel.update(vcx, |panel, cx| {
            assert!(!panel.stick_to_bottom);
            panel
                .items
                .push(Item::User("A newer prompt arrives".into()));
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("role-caption-you, 12").is_some());
        assert!(vcx.debug_bounds("role-caption-you, 14").is_none());
        println!("ACCEPTANCE: arrival of prompt 14 leaves viewed prompt 12 pinned");
        for (delta, label) in [
            (180., "role-caption-you, 1"),
            (-180., "role-caption-you, 14"),
        ] {
            for _ in 0..250 {
                let position = vcx.debug_bounds("transcript").unwrap().center();
                vcx.simulate_event(gpui::ScrollWheelEvent {
                    position,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(delta))),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                });
                vcx.run_until_parked();
                if vcx.debug_bounds(label).is_some()
                    && vcx.debug_bounds("pinned-latest-prompt").is_none()
                {
                    break;
                }
            }
            assert!(
                vcx.debug_bounds(label).is_some(),
                "numbered transcript card {label} paints"
            );
            assert!(
                vcx.debug_bounds("pinned-latest-prompt").is_none(),
                "visible user card must not be duplicated"
            );
        }
        println!(
            "ACCEPTANCE: native top/bottom scrolling paints you, 1 and you, 14 without duplicate pinned cards"
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
        assert!(prompt.bottom() <= transcript.top());

        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(1600.)));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_none());
        assert!(vcx.debug_bounds("pinned-todo-card").is_some());
    }
}
