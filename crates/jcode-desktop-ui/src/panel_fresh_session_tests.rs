use super::*;

#[gpui::test]
fn fresh_session_composer_is_centered_spacious_and_stable_while_typing(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.push_test_panel("session-a", cx);
        workspace
    });
    let panel = workspace
        .read_with(vcx, |workspace, _| workspace.test_panel(0))
        .unwrap();
    vcx.update(|window, cx| {
        let focus = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for (width, height) in [(800., 600.), (1440., 1000.), (640., 480.)] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        vcx.run_until_parked();
        let fresh = vcx
            .debug_bounds("fresh-session")
            .expect("fresh session paints");
        let input = vcx.debug_bounds("prompt-input").expect("input paints");
        assert!(input.size.height >= px(112.));
        assert!(input.size.width <= px(760.));
        assert!(input.left() >= fresh.left() && input.right() <= fresh.right());
        assert!((f32::from(input.center().x - fresh.center().x)).abs() < 1.);
        assert!((f32::from(input.center().y - fresh.center().y)).abs() < 70.);
        assert!(input.bottom() < fresh.bottom() - px(40.));
    }
    let input = vcx.debug_bounds("prompt-input").unwrap();
    vcx.simulate_input("Plan a small project");
    vcx.run_until_parked();
    assert_eq!(vcx.debug_bounds("prompt-input"), Some(input));

    // A real keyboard submission preserves the same editor bounds and focus.
    vcx.simulate_keystrokes("enter");
    assert!(matches!(
        commands.recv_timeout(std::time::Duration::from_millis(100)),
        Ok(Command::Send { content, .. }) if content == "Plan a small project"
    ));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("fresh-session").is_none());
    let submitted = vcx.debug_bounds("prompt-input").expect("input paints");
    assert_eq!(
        submitted, input,
        "submission must not move or shrink the editor"
    );
    let row = vcx.debug_bounds("transcript-row-0").expect("prompt paints");
    let transcript = vcx.debug_bounds("transcript").unwrap();
    assert!((f32::from(row.top() - transcript.top())).abs() < 1.);
    assert!(row.bottom() < submitted.top());
    vcx.simulate_input("Next message");
    panel.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "Next message");
    });
}

#[gpui::test]
fn fresh_session_slash_palette_stays_above_the_larger_composer(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("session-a", cx);
        workspace
    });
    let panel = workspace
        .read_with(vcx, |workspace, _| workspace.test_panel(0))
        .unwrap();
    vcx.update(|window, cx| {
        let focus = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    });
    vcx.simulate_input("/help");
    vcx.run_until_parked();
    let input = vcx.debug_bounds("prompt-input").unwrap();
    let palette = vcx
        .debug_bounds("slash-command-overlay")
        .expect("palette paints");
    assert!(palette.bottom() <= input.top());
    assert!(vcx.debug_bounds("fresh-session").is_some());
}

#[gpui::test]
fn fresh_session_response_spends_space_before_moving_input(cx: &mut gpui::TestAppContext) {
    for (width, height) in [(800., 600.), (1440., 1000.), (640., 480.)] {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "startup-growth".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        vcx.run_until_parked();
        let initial = vcx.debug_bounds("prompt-input").unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items.push(Item::User("FIRST PROMPT".into()));
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(vcx.debug_bounds("prompt-input"), Some(initial));
        panel.update(vcx, |panel, cx| {
            panel.streaming_text = "Small response".into();
            cx.notify();
        });
        vcx.run_until_parked();
        // Streaming now includes a separate activity row. Keep the editor
        // fixed while all rows plus the breathing gap fit, then move it only
        // by the exhausted space (4.5px at 640x480 with the default metrics).
        let activity = vcx.debug_bounds("transcript-activity").unwrap();
        let mut expected = initial;
        expected.origin.y = initial
            .top()
            .max(activity.bottom() + px(TRANSCRIPT_BOTTOM_GAP));
        let small = vcx.debug_bounds("prompt-input").unwrap();
        assert_eq!(small, expected);
        let first_row = vcx.debug_bounds("transcript-row-0").unwrap();
        let viewport = panel.read_with(vcx, |panel, _| panel.transcript_list.viewport_bounds());
        assert_eq!(
            first_row.top(),
            viewport.top(),
            "small response must not scroll"
        );
        let mut previous = small.top();
        for paragraphs in [10, 20, 40] {
            panel.update(vcx, |panel, cx| {
                panel.streaming_text = (0..paragraphs)
                    .map(|i| format!("Response paragraph {i}\n\n"))
                    .collect();
                cx.notify();
            });
            vcx.run_until_parked();
            let input = vcx.debug_bounds("prompt-input").unwrap();
            assert!(
                input.top() >= previous,
                "growing response pulled input upward"
            );
            assert!(input.bottom() <= px(height));
            let viewport = panel.read_with(vcx, |panel, _| panel.transcript_list.viewport_bounds());
            let last_row = vcx.debug_bounds("transcript-row-1").unwrap();
            assert!(last_row.bottom() <= viewport.bottom() + px(1.));
            assert!(
                input.top() - viewport.bottom() >= px(12.),
                "transcript must leave breathing room above the composer: {viewport:?}, {input:?}"
            );
            previous = input.top();
        }
        let grown = vcx.debug_bounds("prompt-input").unwrap();
        assert!(grown.top() > initial.top());
        let meta = vcx.debug_bounds("panel-meta").unwrap();
        assert!((f32::from(grown.bottom() - meta.top())).abs() < 1.);
        assert!(
            panel
                .read_with(vcx, |panel, _| panel.transcript_list.is_scrolled_to_end())
                .unwrap()
        );
    }
}

#[gpui::test]
fn fresh_session_tall_prompt_starts_at_top_then_follows_response(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(
            "startup-long".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        )
    });
    vcx.run_until_parked();
    panel.update(vcx, |panel, cx| {
        panel.items.push(Item::User(format!(
            "FIRST LINE\n\n{}LAST LINE",
            "middle line\n\n".repeat(100)
        )));
        // Long prompts collapse by default. This covers an expanded tall one.
        panel.expanded_prompts.insert((0, false));
        panel.transcript_list.scroll_to_end(); // submit normally requests the tail
        cx.notify();
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, _| {
        let offset = panel.transcript_list.logical_scroll_top();
        assert_eq!(offset.item_ix, 0);
        assert_eq!(offset.offset_in_item, px(0.));
    });
    let row = vcx.debug_bounds("transcript-row-0").unwrap();
    let transcript = vcx.debug_bounds("transcript").unwrap();
    assert!((f32::from(row.top() - transcript.top())).abs() < 1.);
    assert!(row.bottom() > transcript.bottom());
    panel.update(vcx, |panel, cx| {
        panel.streaming_text = "RESPONSE ARRIVED".into();
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(
        panel
            .read_with(vcx, |panel, _| panel.transcript_list.is_scrolled_to_end())
            .unwrap()
    );
    assert!(vcx.debug_bounds("transcript-row-1").is_some());
}
