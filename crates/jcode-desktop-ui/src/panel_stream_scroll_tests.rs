use super::*;

#[gpui::test]
fn streaming_chunks_preserve_touchpad_reading_position(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(
            "scroll-stream".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        )
    });
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, gpui::size(px(600.), px(500.)));
    panel.update(vcx, |panel, cx| {
        panel.items = (0..30)
            .map(|n| Item::Assistant(format!("History {n}")))
            .collect();
        panel.apply(
            &ApiEvent::TextDelta {
                message_id: None,
                session_id: "scroll-stream".into(),
                text: "A streaming paragraph.\n\n".repeat(80),
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let transcript = vcx
        .debug_bounds("transcript-with-response")
        .or_else(|| vcx.debug_bounds("transcript"))
        .unwrap();
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: transcript.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(240.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    vcx.run_until_parked();
    scroll_momentum_tests::settle(vcx);
    let before = panel.read_with(vcx, |panel, _| {
        assert!(!panel.stick_to_bottom);
        panel.transcript_list.logical_scroll_top()
    });
    for n in 0..8 {
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: "scroll-stream".into(),
                    text: format!("Chunk {n}.\n\n"),
                },
                cx,
            )
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            let after = panel.transcript_list.logical_scroll_top();
            assert!(!panel.stick_to_bottom);
            assert_eq!(
                before.item_ix, after.item_ix,
                "chunk {n} changed the reading row"
            );
            assert_eq!(
                before.offset_in_item, after.offset_in_item,
                "chunk {n} changed the reading offset"
            );
        });
    }
}

#[gpui::test]
fn streaming_reasoning_settlement_preserves_history_position(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(
            "scroll-settle".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        )
    });
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, gpui::size(px(600.), px(500.)));
    panel.update(vcx, |panel, cx| {
        panel.items = (0..80)
            .map(|n| Item::Assistant(format!("History {n}")))
            .collect();
        panel.items.push(Item::Reasoning("First thought".into()));
        panel.apply(
            &ApiEvent::ReasoningDelta {
                session_id: "scroll-settle".into(),
                text: "More thinking".into(),
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let transcript = vcx
        .debug_bounds("transcript-with-response")
        .or_else(|| vcx.debug_bounds("transcript"))
        .unwrap();
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: transcript.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(300.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    vcx.run_until_parked();
    scroll_momentum_tests::settle(vcx);
    let before = panel.read_with(vcx, |panel, _| panel.transcript_list.logical_scroll_top());
    panel.update(vcx, |panel, cx| {
        panel.apply(
            &ApiEvent::ReasoningDone {
                session_id: "scroll-settle".into(),
                duration_secs: None,
            },
            cx,
        )
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, _| {
        let after = panel.transcript_list.logical_scroll_top();
        assert!(!panel.stick_to_bottom);
        assert_eq!(
            before.item_ix, after.item_ix,
            "settling a streamed row must not reset history"
        );
        assert_eq!(before.offset_in_item, after.offset_in_item);
    });
}

#[gpui::test]
fn upward_touchpad_between_chunk_and_paint_moves_from_visible_position(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(
            "scroll-race".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        )
    });
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, gpui::size(px(600.), px(500.)));
    panel.update(vcx, |panel, cx| {
        panel.items = vec![Item::Assistant("History paragraph.\n\n".repeat(80))];
        panel.apply(
            &ApiEvent::TextDelta {
                message_id: None,
                session_id: "scroll-race".into(),
                text: "Live reply".into(),
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let transcript = vcx
        .debug_bounds("transcript-with-response")
        .or_else(|| vcx.debug_bounds("transcript"))
        .unwrap();
    let before = panel.read_with(vcx, |panel, _| panel.transcript_list.logical_scroll_top());
    vcx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: "scroll-race".into(),
                    text: " more".into(),
                },
                cx,
            );
        });
        // Dispatch within the same update so test helpers cannot flush the
        // queued render between the chunk and the native input event.
        window.dispatch_event(
            gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                position: transcript.center(),
                delta: gpui::ScrollDelta::Pixels(point(px(0.), px(40.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            }),
            cx,
        );
    });
    vcx.run_until_parked();
    scroll_momentum_tests::settle(vcx);
    panel.read_with(vcx, |panel, _| {
        let after = panel.transcript_list.logical_scroll_top();
        assert!(!panel.stick_to_bottom);
        assert_eq!(before.item_ix, after.item_ix);
        assert!(
            (f32::from(before.offset_in_item - after.offset_in_item) - 40.0).abs() < 0.01,
            "a streamed chunk must not eat an upward touchpad movement"
        );
    });
}
