use super::*;
use std::{cell::Cell, rc::Rc};

// The card sits below the row's inter-message gap. The sticky boundary is
// the card top, not the row top and not the disappearance of its bottom.
pub(super) const PROMPT_TOP_PADDING: f32 = 10.;

#[derive(Clone, Copy, Debug)]
pub(super) struct VisibleRows {
    first: usize,
    last: usize,
    first_top: gpui::Pixels,
    next_prompt_top: Option<(usize, gpui::Pixels)>,
}

/// The list's logical offset is an end sentinel while following the tail, not
/// the first visible row. Observe real painted bounds instead of that offset.
pub(super) fn visibility_marker(
    row: usize,
    is_prompt: bool,
    first_visible: Rc<Cell<Option<VisibleRows>>>,
    list: ListState,
) -> gpui::AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, _, _| {
            let viewport = list.viewport_bounds();
            if bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom() {
                let top = bounds.top() - viewport.top();
                let mut visible = first_visible.get().unwrap_or(VisibleRows {
                    first: row,
                    last: row,
                    first_top: top,
                    next_prompt_top: None,
                });
                if row <= visible.first {
                    visible.first = row;
                    visible.first_top = top;
                }
                let card_top = top + px(PROMPT_TOP_PADDING);
                if is_prompt
                    && card_top > px(0.)
                    && visible.next_prompt_top.is_none_or(|(next, _)| row < next)
                {
                    visible.next_prompt_top = Some((row, card_top));
                }
                visible.last = visible.last.max(row);
                first_visible.set(Some(visible));
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Use painted row geometry, including partially clipped rows. Logical list
/// offsets are an end sentinel during tail following and cannot identify this.
fn prompt_for_viewport(
    prompt_rows: &[(usize, usize)],
    visible: Option<VisibleRows>,
    at_live_end: bool,
) -> Option<usize> {
    let visible = visible?;
    // A freshly submitted prompt at the live end supersedes the old reminder.
    // While browsing history, a later prompt entering the bottom must not unpin
    // the current turn before that later card reaches the top.
    if at_live_end
        && prompt_rows
            .last()
            .is_some_and(|(row, _)| *row > visible.first && *row <= visible.last)
    {
        return None;
    }
    prompt_rows
        .iter()
        .rev()
        .find(|(row, _)| {
            *row < visible.first
                || (*row == visible.first && visible.first_top + px(PROMPT_TOP_PADDING) <= px(0.))
        })
        .map(|(_, index)| *index)
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

/// Count user turns rather than transcript rows. Tool output, assistant text,
/// todo snapshots and restored background notices must not age prompt colors.
fn prompt_distance(items: &[Item], index: usize) -> Option<usize> {
    items.get(index).filter(|item| is_pinnable_prompt(item))?;
    Some(
        items
            .iter()
            .skip(index + 1)
            .filter(|item| is_pinnable_prompt(item))
            .count(),
    )
}

/// Oldest genuine prompt is one. Later arrivals never renumber history.
fn prompt_number(items: &[Item], index: usize) -> Option<usize> {
    items.get(index).filter(|item| is_pinnable_prompt(item))?;
    Some(
        items[..=index]
            .iter()
            .filter(|item| is_pinnable_prompt(item))
            .count(),
    )
}

impl Panel {
    /// Inline and sticky cards use exactly the same content, colors, width,
    /// padding, selection, and acknowledgement animation. Only placement differs.
    pub(super) fn render_user_prompt(
        &self,
        index: usize,
        text: &str,
        pinned: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = Instant::now();
        // Animate position only. Fading the whole card exposes transcript text
        // underneath a sticky prompt during pending/accept acknowledgement.
        let (offset, animating) = self
            .accepted_users
            .get(&index)
            .map(|at| {
                let (offset, _, animating) = crate::ack::motion(*at, now);
                (offset, animating)
            })
            .unwrap_or((0.0, false));
        if animating {
            window.request_animation_frame();
        }
        let number = prompt_number(&self.items, index);
        // Keep the number outside the card in a compact circular badge. Wider
        // turn numbers grow into a pill without taking space from card padding.
        let number_width = number.map_or(0., |number| {
            (number.to_string().len() as f32 * 7. + 8.).max(20.)
        });
        let background = Theme::global()
            .prompt_background(prompt_distance(&self.items, index).unwrap_or(usize::MAX));
        let card = div()
            .relative()
            .debug_selector(move || format!("user-prompt-{index}").into())
            .max_w_full()
            .min_w_0()
            .flex_shrink_1()
            .flex()
            .flex_col()
            .bg(background)
            .rounded_md()
            .px_3()
            .py_2()
            .text_color(Theme::global().TEXT_USER)
            .child(
                div()
                    .debug_selector(move || format!("prompt-content-{index}").into())
                    .child(markdown::render_interactive(
                        text,
                        index,
                        &self.transcript_selection,
                        window,
                        cx,
                        false,
                        self.media_preview_handler(cx),
                    )),
            )
            .into_any_element();
        // Animate and hide the complete row so inline and sticky badges always
        // travel with their cards. Shrink only the card when markdown wraps.
        div()
            .flex()
            .flex_none()
            .w_full()
            .min_w_0()
            .items_start()
            .relative()
            .left(px(offset))
            .when(!pinned && self.offscreen_prompt == Some(index), |row| {
                row.invisible()
            })
            .when_some(number, |row, number| {
                row.gap(px(6.)).child(
                    div()
                        .debug_selector(move || format!("prompt-number-{index}-{number}").into())
                        .flex_none()
                        .mt(px(8.))
                        .w(px(number_width))
                        .h(px(20.))
                        .rounded_full()
                        .bg(background)
                        .font_family(Theme::global().FONT_MONO)
                        .text_center()
                        .text_size(px(10.))
                        .line_height(px(20.))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(number.to_string()),
                )
            })
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
                .max_h(
                    self.offscreen_prompt_clip
                        .unwrap_or(self.transcript_list.viewport_bounds().size.height)
                        .min(self.transcript_list.viewport_bounds().size.height),
                )
                .overflow_y_scroll()
                .occlude()
                .px_3()
                .text_size(px(13.5))
                .child(self.render_user_prompt(index, text, true, window, cx))
                .into_any_element(),
        )
    }

    /// Read visibility after the list paints. Measuring during render would
    /// still describe the previous scroll position, width, or transcript.
    pub(super) fn prompt_visibility_observer(
        &self,
        prompt_rows: Vec<(usize, usize)>,
        first_visible: Rc<Cell<Option<VisibleRows>>>,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let panel = cx.entity().downgrade();
        let following_tail = self.stick_to_bottom;
        let row_count = self.transcript_list.item_count();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, _, cx| {
                let visible = first_visible.take();
                let at_live_end =
                    following_tail || visible.is_some_and(|rows| rows.last + 1 == row_count);
                let offscreen = prompt_for_viewport(&prompt_rows, visible, at_live_end);
                // The next card slides in front of the old reminder. Clip only
                // the old overlay, never the list or either card's layout.
                let clip = offscreen.and_then(|_| visible?.next_prompt_top.map(|(_, top)| top));
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |panel, cx| {
                        if panel.offscreen_prompt != offscreen
                            || panel.offscreen_prompt_clip != clip
                        {
                            panel.offscreen_prompt = offscreen;
                            panel.offscreen_prompt_clip = clip;
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
    fn prompt_numbers_ignore_notices_and_remain_stable_on_append() {
        let mut items = vec![Item::User("First".into()), Item::Assistant("Answer".into())];
        for notice in BACKGROUND_NOTICES {
            items.push(Item::User(notice.into()));
            assert_eq!(prompt_number(&items, items.len() - 1), None);
        }
        items.push(Item::User("  ".into()));
        assert_eq!(prompt_number(&items, items.len() - 1), None);
        assert_eq!(prompt_number(&items, 1), None);
        assert_eq!(prompt_number(&items, items.len()), None);
        for number in 2..=15 {
            items.push(Item::User(format!("Prompt {number}")));
            assert_eq!(prompt_number(&items, items.len() - 1), Some(number));
            assert_eq!(prompt_number(&items, 0), Some(1));
        }
    }

    fn assert_number_geometry(vcx: &mut gpui::VisualTestContext, card: gpui::Bounds<gpui::Pixels>) {
        let number = vcx
            .debug_bounds("prompt-number-0-1")
            .expect("genuine prompt is numbered");
        assert_eq!(
            card.left() - number.right(),
            px(6.),
            "badge sits left of card"
        );
        assert_eq!(number.top(), card.top() + px(8.));
        assert_eq!(number.size, gpui::size(px(20.), px(20.)));
        assert!(number.left() >= vcx.debug_bounds("transcript").unwrap().left());
        let content = vcx.debug_bounds("prompt-content-0").unwrap();
        assert!(
            number.right() < content.left(),
            "number never overlaps prompt content"
        );
        assert_eq!(
            card.size.height,
            content.size.height + px(16.),
            "number adds no footer height"
        );
        assert_eq!(
            card.size.width,
            content.size.width + px(24.),
            "number does not change card padding"
        );
    }

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

    #[test]
    fn only_real_user_turns_age_prompt_card_backgrounds() {
        let mut items = vec![Item::User("First prompt".into())];
        assert_eq!(prompt_distance(&items, 0), Some(0));
        items.push(Item::Assistant("Response".into()));
        items.push(Item::Tool {
            call_id: "call".into(),
            name: "bash".into(),
            input: "{}".into(),
            output: "Done".into(),
            done: true,
            error: None,
        });
        for notice in BACKGROUND_NOTICES {
            items.push(Item::User(notice.into()));
            assert_eq!(prompt_distance(&items, items.len() - 1), None);
        }
        items.push(Item::BackgroundTask {
            task_id: "task".into(),
            label: "Tests".into(),
            summary: "Running".into(),
            percent: None,
            done: false,
        });
        assert_eq!(prompt_distance(&items, 0), Some(0));
        items.push(Item::User("Second prompt".into()));
        assert_eq!(prompt_distance(&items, 0), Some(1));
        assert_eq!(prompt_distance(&items, items.len() - 1), Some(0));
        items.push(Item::User("Third prompt".into()));
        assert_eq!(prompt_distance(&items, 0), Some(2));
        assert_eq!(prompt_distance(&items, items.len() - 1), Some(0));
        assert_eq!(prompt_distance(&items, items.len()), None);
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

    fn visible(first: usize, last: usize, first_top: f32) -> Option<VisibleRows> {
        Some(VisibleRows {
            first,
            last,
            first_top: px(first_top),
            next_prompt_top: None,
        })
    }

    #[test]
    fn viewport_prompt_never_comes_from_a_future_turn() {
        let prompts = [(0, 0), (12, 16), (30, 40)];
        assert_eq!(prompt_for_viewport(&prompts, None, false), None);
        for (first, expected) in [
            (0, None),
            (5, Some(0)),
            (12, Some(0)),
            (20, Some(16)),
            (35, Some(40)),
        ] {
            assert_eq!(
                prompt_for_viewport(&prompts, visible(first, first + 5, 0.), false),
                expected
            );
        }
    }

    #[test]
    fn pin_threshold_is_the_card_top_even_while_its_row_is_visible() {
        let prompts = [(0, 0), (12, 16)];
        for (top, expected) in [
            (0., None),
            (-9.99, None),
            (-10., Some(0)),
            (-10.01, Some(0)),
            (-35., Some(0)),
        ] {
            assert_eq!(
                prompt_for_viewport(&prompts, visible(0, 5, top), false),
                expected
            );
        }
        // The previous card survives the incoming row's ten-pixel top gap,
        // in both directions, until the incoming card itself reaches zero.
        for (top, expected) in [
            (0., 0),
            (-9.5, 0),
            (-10., 16),
            (-10.5, 16),
            (-9.5, 0),
            (0., 0),
        ] {
            assert_eq!(
                prompt_for_viewport(&prompts, visible(12, 15, top), false),
                Some(expected)
            );
        }
        // A subsequent prompt down-screen does not remove historical context.
        assert_eq!(
            prompt_for_viewport(&prompts, visible(3, 12, -5.), false),
            Some(0)
        );
        // At the live end, a newly visible submitted prompt supersedes it.
        assert_eq!(
            prompt_for_viewport(&prompts, visible(3, 12, -5.), true),
            None
        );
        assert_eq!(
            prompt_for_viewport(&prompts, visible(12, 15, -10.), false),
            Some(16)
        );
    }

    #[gpui::test]
    fn sticky_card_keeps_original_geometry_at_partial_scroll_threshold(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "sticky-geometry".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [600., 320.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(800.)));
            for text in [
                "Hi".to_string(),
                "A **formatted** prompt that wraps naturally. ".repeat(12),
            ] {
                panel.update(vcx, |panel, cx| {
                    panel.items = vec![Item::User(text.clone())];
                    for n in 0..40 {
                        panel
                            .items
                            .push(Item::Assistant(format!("Response paragraph {n}")));
                    }
                    panel.stick_to_bottom = false;
                    panel.transcript_list.scroll_to(gpui::ListOffset::default());
                    cx.notify();
                });
                vcx.run_until_parked();
                panel.update(vcx, |panel, cx| {
                    panel.stick_to_bottom = false;
                    panel.transcript_list.scroll_to(gpui::ListOffset::default());
                    cx.notify();
                });
                vcx.run_until_parked();
                let original = vcx.debug_bounds("user-prompt-0").unwrap();
                assert_number_geometry(vcx, original);
                let viewport = vcx.debug_bounds("transcript").unwrap();
                assert_eq!(original.top(), viewport.top() + px(PROMPT_TOP_PADDING));
                // Cross in both directions, including an exactly aligned card,
                // fractional pixel offsets, and a still mostly visible row.
                for offset in [9.5, 10., 10.5, 15., 10., 9.5, 0.] {
                    panel.update(vcx, |panel, cx| {
                        panel.transcript_list.scroll_to(gpui::ListOffset {
                            item_ix: 0,
                            offset_in_item: px(offset),
                        });
                        cx.notify();
                    });
                    vcx.run_until_parked();
                    let card = vcx
                        .debug_bounds("user-prompt-0")
                        .expect("card never disappears");
                    assert_number_geometry(vcx, card);
                    assert_eq!(
                        card.size, original.size,
                        "sticky markdown has identical size at width {width}, offset {offset}"
                    );
                    assert_eq!(card.left(), original.left(), "no horizontal jump");
                    assert_eq!(
                        card.top(),
                        (original.top() - px(offset)).max(viewport.top()),
                        "card clamps exactly to transcript top"
                    );
                    assert_eq!(vcx.debug_bounds("transcript").unwrap(), viewport);
                    assert_eq!(
                        vcx.debug_bounds("pinned-latest-prompt").is_some(),
                        offset >= PROMPT_TOP_PADDING
                    );
                    if let Some(pinned) = vcx.debug_bounds("pinned-latest-prompt") {
                        assert_eq!(pinned.top(), card.top(), "no banner padding above the card");
                        assert_eq!(
                            pinned.size.height, card.size.height,
                            "sticky wrapper must neither clip the card to a banner nor add bottom padding"
                        );
                    }
                    assert!(
                        vcx.debug_bounds("transcript-row-0").is_some(),
                        "threshold must be reached before the prompt row disappears"
                    );
                }
            }
        }
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
                assert_number_geometry(vcx, card);
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
    fn multi_digit_prompt_numbers_stay_in_left_badges(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "prompt-number-gutter".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [600., 320.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(800.)));
            for text in [
                "Hi".to_string(),
                "A **formatted** prompt with wrapping content. ".repeat(8),
            ] {
                for (count, card_selector, content_selector, number_selector) in [
                    (
                        99,
                        "user-prompt-98",
                        "prompt-content-98",
                        "prompt-number-98-99",
                    ),
                    (
                        100,
                        "user-prompt-99",
                        "prompt-content-99",
                        "prompt-number-99-100",
                    ),
                ] {
                    panel.update(vcx, |panel, cx| {
                        panel.items = (1..count).map(|_| Item::User("Earlier".into())).collect();
                        panel.items.push(Item::User(text.clone()));
                        panel.stick_to_bottom = true;
                        panel.transcript_list.scroll_to_end();
                        cx.notify();
                    });
                    vcx.run_until_parked();
                    let card = vcx.debug_bounds(card_selector).unwrap();
                    let content = vcx.debug_bounds(content_selector).unwrap();
                    let number = vcx.debug_bounds(number_selector).unwrap();
                    assert!(
                        number.right() < content.left(),
                        "multi-digit number never overlaps markdown"
                    );
                    assert_eq!(number.right() + px(6.), card.left());
                    assert_eq!(number.top(), card.top() + px(8.));
                    assert_eq!(number.size.width, px(if count == 99 { 22. } else { 29. }));
                    assert!(number.left() >= vcx.debug_bounds("transcript").unwrap().left());
                    assert_eq!(card.size.width, content.size.width + px(24.));
                    assert_eq!(
                        card.size.height,
                        content.size.height + px(16.),
                        "no numbered footer"
                    );
                    assert!(card.right() <= vcx.debug_bounds("transcript").unwrap().right());
                }
            }
        }
    }

    #[gpui::test]
    fn newer_prompt_slides_over_old_sticky_card_before_taking_its_place(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "prompt-stack".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(600.)));
        panel.update(vcx, |panel, cx| {
            panel.items = vec![
                Item::User("Old prompt line\n\n".repeat(5)),
                Item::Assistant("Short answer".into()),
                Item::User("Newer prompt line\n\n".repeat(3)),
            ];
            for n in 0..40 {
                panel.items.push(Item::Assistant(format!("Response {n}")));
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
        let old_size = vcx.debug_bounds("user-prompt-0").unwrap().size;
        let new_size = vcx.debug_bounds("user-prompt-2").unwrap().size;
        let mut overlapped = false;
        let mut switched = false;
        for _ in 0..150 {
            let viewport = vcx.debug_bounds("transcript").unwrap();
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-6.))),
                modifiers: Default::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            vcx.run_until_parked();
            scroll_momentum_tests::settle(vcx);
            let pinned_index = panel.read_with(vcx, |panel, _| panel.offscreen_prompt);
            if overlapped {
                assert!(
                    pinned_index.is_some(),
                    "no missing reminder at the incoming row padding"
                );
            }
            if pinned_index == Some(0) {
                let old = vcx.debug_bounds("user-prompt-0").unwrap();
                let newer = vcx.debug_bounds("user-prompt-2").unwrap();
                assert_eq!(old.size, old_size, "clipping never resizes old content");
                assert_eq!(newer.size, new_size);
                if newer.top() < old.bottom() {
                    overlapped = true;
                    let overlay = vcx.debug_bounds("pinned-latest-prompt").unwrap();
                    assert_eq!(
                        overlay.bottom(),
                        newer.top(),
                        "old overlay stops at incoming card"
                    );
                    assert!(
                        newer.top() > viewport.top(),
                        "old stays pinned until newer reaches top"
                    );
                }
            } else if pinned_index == Some(2) {
                assert!(
                    overlapped,
                    "incoming prompt passes in front before switching"
                );
                let newer = vcx.debug_bounds("user-prompt-2").unwrap();
                assert_eq!(newer.top(), viewport.top());
                assert_eq!(newer.size, new_size);
                switched = true;
                break;
            }
        }
        assert!(
            overlapped && switched,
            "native scroll exercises overlap and handoff"
        );
        let mut restored = false;
        for _ in 0..100 {
            let viewport = vcx.debug_bounds("transcript").unwrap();
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: viewport.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(6.))),
                modifiers: Default::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            vcx.run_until_parked();
            scroll_momentum_tests::settle(vcx);
            if panel.read_with(vcx, |panel, _| panel.offscreen_prompt) == Some(0) {
                let old = vcx.debug_bounds("user-prompt-0").unwrap();
                let newer = vcx.debug_bounds("user-prompt-2").unwrap();
                assert_eq!(old.size, old_size);
                assert_eq!(newer.size, new_size);
                assert!(newer.top() > viewport.top() && newer.top() < old.bottom());
                assert_eq!(
                    vcx.debug_bounds("pinned-latest-prompt").unwrap().bottom(),
                    newer.top()
                );
                restored = true;
                break;
            }
        }
        assert!(
            restored,
            "reverse native scroll restores old card behind newer card"
        );
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
                panel.read_with(vcx, |panel, _| {
                    assert_eq!(
                        panel.offscreen_prompt,
                        Some(451),
                        "newest prompt must not replace historical context"
                    );
                });
                if let Some(newer) = vcx.debug_bounds("user-prompt-492") {
                    assert!(
                        newer.top() > pinned.bottom(),
                        "a newer inline prompt can enter below the historical sticky card"
                    );
                }
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

        // A partially scrolled original remains sticky once its card top crosses.
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
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_some());

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
        assert_eq!(
            prompt.top() - todos.bottom(),
            px(8.),
            "todo breathing room belongs above the whole viewport"
        );
        assert_eq!(prompt.top(), transcript.top());
        assert!(prompt.bottom() < transcript.bottom());

        panel.update(vcx, |panel, cx| {
            panel.pinned_todo_expanded = true;
            cx.notify();
        });
        vcx.run_until_parked();
        let todos = vcx.debug_bounds("pinned-todo-card").unwrap();
        let prompt = vcx.debug_bounds("pinned-latest-prompt").unwrap();
        assert_eq!(
            prompt.top() - todos.bottom(),
            px(8.),
            "expanded todo has the same gap"
        );
        assert_eq!(prompt.top(), vcx.debug_bounds("transcript").unwrap().top());

        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(1600.)));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-latest-prompt").is_none());
        assert!(vcx.debug_bounds("pinned-todo-card").is_some());
    }
}
