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

#[gpui::test]
fn returning_to_bottom_resumes_following_streamed_output(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new(
            "scroll-follow".into(),
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
        cx.notify();
    });
    vcx.run_until_parked();
    for mode in 0..3 {
        panel.update(vcx, |panel, cx| {
            if mode == 2 {
                panel.scroll_transcript_direct(300.0, cx);
            } else {
                panel.glide_transcript_input(-300.0, mode == 0, cx);
            }
        });
        vcx.run_until_parked();
        scroll_momentum_tests::settle(vcx);
        panel.read_with(vcx, |panel, _| {
            assert!(!panel.stick_to_bottom, "scrolling up releases follow mode");
            assert!(!panel.transcript_end_visible);
        });
        panel.update(vcx, |panel, cx| {
            if mode == 2 {
                panel.scroll_transcript_direct(-100_000.0, cx);
            } else {
                panel.glide_transcript_input(100_000.0, mode == 0, cx);
            }
        });
        vcx.run_until_parked();
        scroll_momentum_tests::settle(vcx);
        panel.read_with(vcx, |panel, _| {
            assert!(panel.transcript_end_visible);
            assert!(panel.stick_to_bottom, "manual catch-up resumes follow mode");
        });
        for n in 0..4 {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::TextDelta {
                        message_id: None,
                        session_id: "scroll-follow".into(),
                        text: format!("Chunk {n}.\n\n").repeat(20),
                    },
                    cx,
                )
            });
            vcx.run_until_parked();
            panel.read_with(vcx, |panel, _| {
                assert!(panel.stick_to_bottom);
                assert!(
                    panel.transcript_end_visible,
                    "streamed growth remains visible"
                );
            });
        }
    }
}

#[gpui::test]
fn native_scrollbar_catch_up_resumes_following_streamed_output(cx: &mut gpui::TestAppContext) {
    // Normal mouse-up, missed mouse-up recovered by hover, and track paging.
    for mode in 0..3 {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "scrollbar-follow".into(),
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
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| panel.scroll_transcript_direct(300., cx));
        vcx.run_until_parked();
        assert!(!panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
        let thumb = vcx.debug_bounds("transcript-scrollbar").unwrap();
        if mode == 2 {
            let viewport = panel.read_with(vcx, |panel, _| panel.transcript_list.viewport_bounds());
            let track_bottom = point(thumb.center().x, viewport.bottom() - px(5.));
            assert!(track_bottom.y > thumb.bottom(), "click must hit the track");
            vcx.simulate_click(track_bottom, Default::default());
        } else {
            vcx.simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, Default::default());
            vcx.run_until_parked();
            let destination = point(thumb.center().x, thumb.center().y + px(100_000.));
            vcx.simulate_mouse_move(destination, gpui::MouseButton::Left, Default::default());
            vcx.run_until_parked();
            panel.read_with(vcx, |panel, _| {
                assert!(panel.transcript_list.is_scrollbar_dragging());
                assert!(!panel.stick_to_bottom, "do not follow while thumb is held");
            });
            if mode == 0 {
                vcx.simulate_mouse_up(destination, gpui::MouseButton::Left, Default::default());
            } else {
                vcx.simulate_mouse_move(destination, None, Default::default());
            }
        }
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert!(!panel.transcript_list.is_scrollbar_dragging());
            assert!(
                panel.stick_to_bottom,
                "scrollbar mode {mode} must resume follow"
            );
            assert!(panel.transcript_end_visible);
        });
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: "scrollbar-follow".into(),
                    text: "New streamed paragraph.\n\n".repeat(80),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert!(panel.stick_to_bottom);
            assert!(
                panel.transcript_end_visible,
                "scrollbar mode {mode} must follow growth"
            );
        });
    }
}
