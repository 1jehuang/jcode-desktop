//! Startup layout must survive scroll, history timing, reload, and chrome changes.
use super::*;

fn history(prompt: &str) -> Vec<jcode_sdk::HistoryMessage> {
    vec![jcode_sdk::HistoryMessage {
        role: "user".into(),
        content: prompt.into(),
    }]
}

fn focused_startup_panel(
    cx: &mut gpui::TestAppContext,
) -> (Entity<Panel>, &mut gpui::VisualTestContext) {
    cx.update(crate::input::bind_keys);
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(
            "startup-lifecycle".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        )
    });
    vcx.update(|window, cx| {
        Panel::connect_input(&panel, cx);
        let focus = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    });
    vcx.run_until_parked();
    (panel, vcx)
}

#[gpui::test]
fn startup_tall_preview_manual_down_scroll_detaches_without_jumping_to_tail(
    cx: &mut gpui::TestAppContext,
) {
    for discrete in [false, true] {
        let (panel, vcx) = focused_startup_panel(cx);
        vcx.simulate_input(&format!(
            "FIRST LINE\n\n{}LAST LINE",
            "middle line\n\n".repeat(100)
        ));
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert_eq!(panel.transcript_list.logical_scroll_top().item_ix, 0);
            assert_eq!(
                panel.transcript_list.logical_scroll_top().offset_in_item,
                px(0.)
            );
        });
        panel.update(vcx, |panel, cx| {
            if discrete {
                panel.glide_transcript_wheel(40., cx);
                // Exercise the same glide steps without wall-clock timer waits.
                for _ in 0..128 {
                    if !panel.advance_transcript_wheel(cx) {
                        break;
                    }
                }
            } else {
                panel.scroll_transcript_direct(-40., cx);
            }
        });
        vcx.run_until_parked();
        let offset = panel.read_with(vcx, |panel, _| {
            let offset = -f32::from(panel.transcript_list.scroll_px_offset_for_scrollbar().y);
            let max = f32::from(panel.transcript_list.max_offset_for_scrollbar().y);
            assert!(
                !panel.stick_to_bottom,
                "manual scrolling must detach first-prompt preview"
            );
            assert!(
                offset >= 30. && offset <= 45.,
                "40px scroll became {offset}px (discrete={discrete})"
            );
            assert!(
                offset < max / 2.,
                "first downward gesture jumped to the tail"
            );
            offset
        });
        panel.update(vcx, |panel, cx| {
            panel.streaming_text = "Answer arrived while reading the beginning".into();
            cx.notify();
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert!(!panel.stick_to_bottom);
            let after = -f32::from(panel.transcript_list.scroll_px_offset_for_scrollbar().y);
            assert!(
                (after - offset).abs() < 1.,
                "response stole detached scroll position"
            );
        });
    }
}

#[gpui::test]
fn startup_existing_history_has_identical_layout_before_or_after_welcome_paint(
    cx: &mut gpui::TestAppContext,
) {
    let mut bounds = Vec::new();
    for delayed in [false, true] {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let mut panel = Panel::new(
                "existing-history".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            );
            if !delayed {
                panel.load_history(history("Existing conversation"), Vec::new(), cx);
            }
            panel
        });
        vcx.run_until_parked();
        if delayed {
            assert!(vcx.debug_bounds("fresh-session").is_some());
            panel.update(vcx, |panel, cx| {
                panel.load_history(history("Existing conversation"), Vec::new(), cx);
            });
            vcx.run_until_parked();
        }
        assert!(vcx.debug_bounds("fresh-session").is_none());
        panel.read_with(vcx, |panel, _| {
            assert!(
                panel.startup_layout.is_none(),
                "history timing armed a fresh composer"
            );
        });
        bounds.push((
            vcx.debug_bounds("prompt-input").unwrap(),
            vcx.debug_bounds("transcript").unwrap(),
            vcx.debug_bounds("transcript-row-0").unwrap(),
        ));
    }
    assert_eq!(
        bounds[0], bounds[1],
        "existing history layout depends on arrival timing"
    );
}

#[gpui::test]
fn resumed_transcript_keeps_cards_clear_of_the_composer_footer(cx: &mut gpui::TestAppContext) {
    for (width, height) in [(800., 600.), (1440., 1000.), (640., 480.)] {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let mut panel = Panel::new(
                "composer-spacing".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            );
            panel.load_history(history(&"Older message\n\n".repeat(100)), Vec::new(), cx);
            panel.items.push(Item::Error("Last card".into()));
            panel
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        vcx.run_until_parked();
        let viewport = panel.read_with(vcx, |panel, _| panel.transcript_list.viewport_bounds());
        let last_row = vcx.debug_bounds("transcript-row-1").unwrap();
        let footer = vcx.debug_bounds("panel-meta").unwrap();
        let input = vcx.debug_bounds("prompt-input").unwrap();
        assert!(last_row.bottom() <= viewport.bottom() + px(1.));
        assert!(
            footer.top() - viewport.bottom() >= px(12.),
            "last card must not crowd the footer: {viewport:?}, {footer:?}"
        );
        assert!(input.top() >= footer.bottom());
        assert!(input.bottom() <= px(height));
    }
}

#[gpui::test]
fn startup_committed_layout_round_trips_reload_and_empty_history_loading_frame(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = focused_startup_panel(cx);
    let fresh = vcx.debug_bounds("prompt-input").unwrap();
    vcx.simulate_input("Short first prompt");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(vcx.debug_bounds("prompt-input"), Some(fresh));
    vcx.simulate_input("Unsent followup survives reload");
    let snapshot = panel.read_with(vcx, |panel, cx| panel.snapshot(cx));
    let serialized = serde_json::to_vec(&snapshot).unwrap();
    let snapshot: PanelSnapshot = serde_json::from_slice(&serialized).unwrap();
    let (restored, vcx) = cx.add_window_view(|_, cx| {
        let mut panel = Panel::new(
            "startup-lifecycle".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        );
        panel.restore_snapshot(snapshot, cx);
        panel
    });
    // The bridge has not supplied history yet. This frame must not overwrite
    // the committed reservation with a newly centered welcome measurement.
    vcx.run_until_parked();
    restored.update(vcx, |panel, cx| {
        panel.load_history(history("Short first prompt"), Vec::new(), cx);
    });
    vcx.run_until_parked();
    assert_eq!(vcx.debug_bounds("prompt-input"), Some(fresh));
    let row = vcx.debug_bounds("transcript-row-0").unwrap();
    let transcript = vcx.debug_bounds("transcript").unwrap();
    assert!((f32::from(row.top() - transcript.top())).abs() < 1.);
    restored.read_with(vcx, |panel, cx| {
        assert_eq!(
            panel.input.read(cx).content.as_ref(),
            "Unsent followup survives reload"
        );
        assert!(panel.startup_layout.is_some());
        assert_eq!(
            panel.transcript_list.logical_scroll_top().offset_in_item,
            px(0.)
        );
    });
}

#[gpui::test]
fn startup_resize_and_repeated_notifications_preserve_settled_geometry(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = focused_startup_panel(cx);
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_input("Keep the input stable while the answer grows");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    // Response sizes around the pinned-prompt threshold exercise the feedback
    // loop which previously alternated list height and pin visibility forever.
    for (width, height, paragraphs) in [
        (800., 600., 8),
        (800., 600., 10),
        (800., 600., 12),
        (640., 480., 12),
        (1440., 1000., 12),
        (800., 600., 12),
    ] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        panel.update(vcx, |panel, cx| {
            panel.streaming_text = (0..paragraphs)
                .map(|i| format!("Response paragraph {i}\n\n"))
                .collect();
            cx.notify();
        });
        vcx.run_until_parked();
        let input = vcx.debug_bounds("prompt-input").unwrap();
        let transcript = vcx.debug_bounds("transcript-with-response").unwrap();
        let pin = vcx.debug_bounds("pinned-latest-prompt");
        let meta = vcx.debug_bounds("panel-meta").unwrap();
        assert!(input.top() >= px(0.) && input.bottom() <= meta.top());
        assert!(input.left() >= px(0.) && input.right() <= px(width));
        assert!(meta.bottom() <= px(height));
        assert!(transcript.bottom() <= input.top());
        for _ in 0..8 {
            panel.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
            assert_eq!(vcx.debug_bounds("prompt-input"), Some(input));
            assert_eq!(
                vcx.debug_bounds("transcript-with-response"),
                Some(transcript)
            );
            assert_eq!(vcx.debug_bounds("pinned-latest-prompt"), pin);
        }
    }
}

#[gpui::test]
fn startup_each_native_submission_paints_the_newest_row(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = focused_startup_panel(cx);
    for index in 0..20 {
        let text = format!(
            "Growth message {index:02}. {}",
            "Keep the composer steady until the conversation needs more room. ".repeat(3)
        );
        vcx.simulate_input(&text);
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert_eq!(panel.items.len(), index + 1);
            assert!(
                matches!(panel.items.last(), Some(Item::User(actual)) if actual == text.trim())
            );
        });
        let row = vcx
            .debug_bounds(Box::leak(
                format!("transcript-row-{index}").into_boxed_str(),
            ))
            .expect("newest row must paint immediately after each submission");
        let transcript = vcx.debug_bounds("transcript").unwrap();
        let input = vcx.debug_bounds("prompt-input").unwrap();
        assert!(
            row.bottom() <= input.top() + px(1.),
            "newest row painted behind composer: {row:?}, {input:?}"
        );
        assert!(
            row.top() < transcript.bottom() && row.bottom() <= transcript.bottom() + px(1.),
            "newest row not in viewport: {row:?}, {transcript:?}"
        );
    }
}
