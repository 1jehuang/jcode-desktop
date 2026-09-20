use super::*;
use std::{cell::Cell, rc::Rc};

/// Observe the last row's bottom, not whether auto-follow happens to be enabled.
/// Short transcripts and manually scrolled tails need no catch-up button.
pub(super) fn end_marker(visible: Rc<Cell<bool>>, list: ListState) -> gpui::AnyElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, _, _| {
            let viewport = list.viewport_bounds();
            visible.set(
                viewport.size.height > px(0.)
                    && bounds.bottom() > viewport.top()
                    && bounds.bottom() <= viewport.bottom() + px(1.),
            );
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

impl Panel {
    pub(super) fn transcript_end_observer(
        &self,
        visible: Rc<Cell<bool>>,
        row_count: usize,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let panel = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, _, cx| {
                // This paints after the virtual list, including its activity row.
                // An unpainted last row is offscreen. No estimated scroll height
                // or previous-layout metrics are used to decide visibility.
                let visible = visible.replace(false) || row_count == 0;
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |panel, cx| {
                        if panel.transcript_end_visible != visible {
                            panel.transcript_end_visible = visible;
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

    #[gpui::test]
    fn latest_updates_after_resize_and_streaming_activity(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "latest-resize".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(400.)));
        panel.update(vcx, |panel, cx| {
            panel.items = (0..12)
                .map(|n| Item::Assistant(format!("message {n}")))
                .collect();
            panel.stick_to_bottom = false;
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: "latest-resize".into(),
                    text: "Streaming reply".into(),
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| {
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("jump-to-latest").is_some());
        assert!(vcx.debug_bounds("latest-activity").is_some());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(1600.)));
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("jump-to-latest").is_none(),
            "resizing to reveal the stream and activity row hides Latest"
        );
        assert!(vcx.debug_bounds("latest-activity").is_none());
        vcx.simulate_window_resize(handle, gpui::size(px(600.), px(400.)));
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("jump-to-latest").is_some(),
            "resizing to clip the end shows Latest again"
        );
        assert!(vcx.debug_bounds("latest-activity").is_some());
    }

    #[gpui::test]
    fn latest_shows_live_activity_while_scrolled_up(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new(
                "latest-activity".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        });
        panel.update(vcx, |panel, cx| {
            panel.items = (0..80)
                .map(|n| Item::Assistant(format!("message {n}")))
                .collect();
            panel.stick_to_bottom = false;
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| {
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("jump-to-latest").is_some());
        assert!(vcx.debug_bounds("latest-activity").is_none());

        for (status, label) in [
            ("thinking", "Thinking"),
            ("streaming", "Responding"),
            ("running_tools", "Running tools"),
        ] {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::SessionStatus {
                        session_id: "latest-activity".into(),
                        status: status.into(),
                    },
                    cx,
                );
                assert_eq!(panel.status_line(), label);
            });
            vcx.run_until_parked();
            let activity = vcx
                .debug_bounds("latest-activity")
                .expect("pinned activity paints");
            let chip = vcx.debug_bounds("jump-to-latest").unwrap();
            let transcript = vcx.debug_bounds("transcript").unwrap();
            assert!(activity.left() >= chip.left() && activity.right() <= chip.right());
            assert!(activity.top() >= transcript.top() && activity.bottom() <= transcript.bottom());
            assert!(
                vcx.debug_bounds("transcript-activity").is_none(),
                "tail is offscreen"
            );
            assert!(!panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
        }

        let chip = vcx.debug_bounds("jump-to-latest").unwrap();
        vcx.simulate_click(chip.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
        assert!(vcx.debug_bounds("latest-activity").is_none());
        assert!(vcx.debug_bounds("transcript-activity").is_some());

        for terminal in ["done", "idle", "cancelled", "error"] {
            panel.update(vcx, |panel, cx| {
                panel.stick_to_bottom = false;
                panel.transcript_list.scroll_to(gpui::ListOffset::default());
                panel.apply(
                    &ApiEvent::SessionStatus {
                        session_id: "latest-activity".into(),
                        status: "thinking".into(),
                    },
                    cx,
                );
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("latest-activity").is_some());
            panel.update(vcx, |panel, cx| {
                let event = match terminal {
                    "done" => ApiEvent::TurnDone {
                        session_id: "latest-activity".into(),
                    },
                    "error" => ApiEvent::Error {
                        code: jcode_sdk::api::ErrorCode::Internal,
                        message: "Provider failed".into(),
                    },
                    status => ApiEvent::SessionStatus {
                        session_id: "latest-activity".into(),
                        status: status.into(),
                    },
                };
                panel.apply(&event, cx);
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("latest-activity").is_none(), "{terminal}");
            assert!(
                vcx.debug_bounds("jump-to-latest").is_some(),
                "catch-up remains available"
            );
        }
    }

    #[gpui::test]
    fn latest_tracks_visible_end_not_follow_mode(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Assistant("Short reply".into())];
            panel.stick_to_bottom = false;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("jump-to-latest").is_none(),
            "the whole short transcript is visible even without follow mode"
        );

        panel.update(vcx, |panel, cx| {
            panel.items = (0..80)
                .map(|n| Item::Assistant(format!("message {n}")))
                .collect();
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("jump-to-latest").is_some(),
            "the end is below the viewport"
        );

        // Scroll manually to the end without enabling follow mode or clicking Latest.
        panel.update(vcx, |panel, cx| {
            panel.transcript_list.scroll_to_end();
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(!panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
        assert!(
            vcx.debug_bounds("jump-to-latest").is_none(),
            "manual catch-up hides Latest too"
        );

        // Content shrinking to fit must hide the chip even while detached.
        panel.update(vcx, |panel, cx| {
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("jump-to-latest").is_some());
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Assistant("Short again".into())];
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("jump-to-latest").is_none());

        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("jump-to-latest").is_none(),
            "empty transcripts never offer Latest"
        );
    }

    #[gpui::test]
    fn latest_requires_the_bottom_of_a_tall_last_row(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::User("A long prompt line\n".repeat(1000))];
            panel.stick_to_bottom = false;
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| {
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("transcript-row-0").is_some());
        let chip = vcx
            .debug_bounds("jump-to-latest")
            .expect("a visible last row is not enough when its bottom is clipped");
        vcx.simulate_click(chip.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("jump-to-latest").is_none());
        assert!(panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
    }
}
