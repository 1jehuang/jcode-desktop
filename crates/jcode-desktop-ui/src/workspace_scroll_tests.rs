//! A deliberate swipe selects one workspace, not every row in its momentum tail.
use super::*;
use gpui::TouchPhase::{Cancelled, Ended, Moved, Started};

// StripGesture integrates either direction. The workspace event routers apply
// the policy that downward row navigation is keyboard-only.
#[test]
fn workspace_swipe_requires_more_travel_and_latches_until_a_new_start() {
    let mut gesture = StripGesture::default();
    assert_eq!(gesture.feed_vertical(-130.0, Started), 0);
    assert_eq!(gesture.feed_vertical(-129.0, Moved), 0);
    assert_eq!(gesture.feed_vertical(-1.0, Moved), 1);
    for delta in [-900.0, -300.0, 900.0] {
        assert_eq!(gesture.feed_vertical(delta, Moved), 0);
    }
    assert_eq!(gesture.feed_vertical(-900.0, Ended), 0);
    assert_eq!(gesture.feed_vertical(-900.0, Moved), 0);
    assert_eq!(gesture.feed_vertical(STRIP_BREAK, Started), -1);
}

#[test]
fn workspace_swipe_cancellation_does_not_navigate_or_preserve_partial_pull() {
    let mut gesture = StripGesture::default();
    assert_eq!(gesture.feed_vertical(-100.0, Started), 0);
    assert_eq!(gesture.feed_vertical(-900.0, Cancelled), 0);
    assert_eq!(gesture.feed_vertical(-900.0, Moved), 0);
    assert_eq!(gesture.feed_vertical(-100.0, Started), 0);
    assert_eq!(gesture.feed_vertical(-160.0, Moved), 1);
}

fn swipe(
    workspace: &Entity<Workspace>,
    cx: &mut gpui::VisualTestContext,
    target: gpui::Point<gpui::Pixels>,
    dy: f32,
    phase: gpui::TouchPhase,
) {
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: target,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(dy))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: phase,
    });
    // Settle the row transition without advancing gesture time. Subsequent
    // native events must hit the destination row, not the departing row.
    workspace.update_in(cx, |w, window, cx| {
        w.row_progress
            .sample(Instant::now() + Duration::from_secs(1));
        cx.notify();
        window.simulate_next_frame(cx);
    });
    cx.run_until_parked();
}

fn assert_destination_latches(cx: &mut gpui::TestAppContext, destination: Option<bool>) {
    let (workspace, cx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.show_sidebar = false;
        // Start below the destination: downward workspace swipes are disabled.
        w.active_row = 3;
        w.push_test_panel("source", cx);
        if destination.is_some() {
            w.push_test_panel("destination", cx);
            w.slots[1].row = 2;
        }
        w.active = 0;
        for (index, slot) in w.slots.iter().enumerate() {
            slot.panel.update(cx, |panel, cx| {
                panel.items.clear();
                if index == 1 && destination == Some(true) {
                    for n in 0..80 {
                        panel
                            .items
                            .push(crate::panel::Item::Assistant(format!("message {n}")));
                    }
                }
                cx.notify();
            });
        }
        w
    });
    cx.run_until_parked();
    let target = cx.debug_bounds("panel-0").unwrap().center();
    swipe(&workspace, cx, target, 130.0, Started);
    assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 3);
    swipe(&workspace, cx, target, STRIP_BREAK - 130.0, Moved);
    assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 2);
    let panel = workspace.read_with(cx, |w, _| w.test_panel(1));
    let before = panel
        .as_ref()
        .map(|p| p.read_with(cx, |p, _| p.test_scroll_offset_y()));
    for index in 0..12 {
        let direction = if index % 2 == 0 { -1.0 } else { 1.0 };
        swipe(&workspace, cx, target, direction * STRIP_BREAK * 2.0, Moved);
        assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 2);
    }
    swipe(&workspace, cx, target, 0.0, Ended);
    swipe(&workspace, cx, target, STRIP_BREAK * 3.0, Moved);
    assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 2);
    let after = panel
        .as_ref()
        .map(|p| p.read_with(cx, |p, _| p.test_scroll_offset_y()));
    assert_eq!(
        before, after,
        "workspace momentum must not scroll the destination transcript"
    );
    if destination != Some(true) {
        swipe(&workspace, cx, target, STRIP_BREAK, Started);
        assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 1);
        cx.background_executor
            .advance_clock(GESTURE_RESET + Duration::from_millis(1));
        swipe(&workspace, cx, target, STRIP_BREAK, Moved);
        assert_eq!(
            workspace.read_with(cx, |w, _| w.active_row),
            0,
            "platforms without Started must re-arm after an idle gap"
        );
    } else {
        swipe(&workspace, cx, target, STRIP_BREAK, Started);
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 2);
        let fresh = panel
            .as_ref()
            .map(|p| p.read_with(cx, |p, _| p.test_scroll_offset_y()));
        assert_ne!(
            after, fresh,
            "a fresh gesture must scroll the destination normally"
        );
    }
}

#[gpui::test]
fn workspace_swipe_tail_cannot_skip_empty_rows(cx: &mut gpui::TestAppContext) {
    assert_destination_latches(cx, None);
}

#[gpui::test]
fn workspace_swipe_tail_cannot_skip_empty_panels(cx: &mut gpui::TestAppContext) {
    assert_destination_latches(cx, Some(false));
}

#[gpui::test]
fn workspace_swipe_tail_cannot_scroll_destination_conversation(cx: &mut gpui::TestAppContext) {
    assert_destination_latches(cx, Some(true));
}

#[gpui::test]
fn workspace_swipe_partial_pulls_expire_over_empty_panels(cx: &mut gpui::TestAppContext) {
    let (workspace, cx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.active_row = 2;
        w.push_test_panel("empty", cx);
        w.slots[0].panel.update(cx, |p, _| p.items.clear());
        w
    });
    cx.run_until_parked();
    let target = cx.debug_bounds("panel-0").unwrap().center();
    swipe(&workspace, cx, target, STRIP_BREAK * 0.6, Started);
    cx.background_executor
        .advance_clock(GESTURE_RESET + Duration::from_millis(1));
    swipe(&workspace, cx, target, STRIP_BREAK * 0.6, Moved);
    assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 2);
    swipe(&workspace, cx, target, STRIP_BREAK * 0.6, Moved);
    assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 1);
    swipe(&workspace, cx, target, STRIP_BREAK * 3.0, Cancelled);
    assert_eq!(workspace.read_with(cx, |w, _| w.active_row), 1);
}

#[gpui::test]
fn downward_workspace_swipes_never_preview_or_navigate(cx: &mut gpui::TestAppContext) {
    // Exercise the empty-row router, empty-panel router, and a horizontal pan
    // breaking vertically over a populated transcript. Start away from the
    // bottom edge so clamping cannot accidentally make the test pass.
    for source in [None, Some(false), Some(true)] {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_sidebar = false;
            w.active_row = 1;
            if let Some(populated) = source {
                w.push_test_panel("source", cx);
                if !populated {
                    w.slots[0].panel.update(cx, |p, _| p.items.clear());
                }
            }
            w
        });
        vcx.run_until_parked();
        let target = vcx.debug_bounds("workspace-canvas").unwrap().center();
        if source == Some(true) {
            vcx.simulate_event(gpui::ScrollWheelEvent {
                position: target,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(-40.0), px(0.0))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: Started,
            });
            vcx.run_until_parked();
            assert!(workspace.read_with(vcx, |w, _| w.gesture.axis == GestureAxis::Horizontal));
        }
        for (dy, phase) in [
            (
                -STRIP_BREAK * 0.5,
                if source == Some(true) { Moved } else { Started },
            ),
            (-STRIP_BREAK * 2.0, Moved),
            (0.0, Ended),
        ] {
            swipe(&workspace, vcx, target, dy, phase);
            workspace.read_with(vcx, |w, _| {
                assert_eq!(w.active_row, 1);
                assert_eq!(w.gesture.pull, 0.0);
                assert_eq!(w.workspace_pull.value, 0.0);
                assert!(w.outgoing_row.is_none());
            });
            assert!(vcx.debug_bounds("row-pull-neighbor").is_none());
        }
    }
}
