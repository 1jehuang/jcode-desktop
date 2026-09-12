//! Real painted-list regression coverage for momentum and direct manipulation.
use super::*;

fn fixture(cx: &mut gpui::TestAppContext) -> (Entity<Panel>, &mut gpui::VisualTestContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("scroll-momentum", cx);
        workspace
    });
    let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    panel.update(vcx, |panel, cx| {
        panel.items = (0..100)
            .map(|n| Item::Assistant(format!("Message {n}")))
            .collect();
        cx.notify();
    });
    vcx.run_until_parked();
    panel.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    (panel, vcx)
}

fn wheel(vcx: &mut gpui::VisualTestContext, precise: bool, delta: f32) {
    let position = vcx.debug_bounds("transcript").unwrap().center();
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: if precise {
            gpui::ScrollDelta::Pixels(point(px(0.), px(delta)))
        } else {
            gpui::ScrollDelta::Lines(point(0., delta))
        },
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    vcx.run_until_parked();
}

fn frame(vcx: &mut gpui::VisualTestContext, millis: u64) {
    vcx.executor().advance_clock(Duration::from_millis(millis));
    vcx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    vcx.run_until_parked();
}

pub(super) fn settle(vcx: &mut gpui::VisualTestContext) {
    for _ in 0..16 {
        frame(vcx, 16);
    }
}

#[gpui::test]
fn scroll_momentum_coasts_on_frames_then_stops_requesting_frames(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = fixture(cx);
    let before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
    wheel(vcx, false, 3.0);
    assert_eq!(
        before,
        panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y())
    );
    frame(vcx, 16);
    let first = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
    assert!(first > before);
    frame(vcx, 16);
    assert!(panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()) > first);
    for _ in 0..60 {
        frame(vcx, 16);
    }
    panel.read_with(vcx, |panel, _| {
        assert_eq!(panel.transcript_wheel_glide.remaining, 0.0);
        assert!(panel.transcript_wheel_frame.is_none());
        assert!(!panel.transcript_wheel_frame_pending);
    });
}

#[gpui::test]
fn scroll_momentum_switches_to_short_touchpad_smoothing_and_latest_cancels(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = fixture(cx);
    wheel(vcx, false, 3.0);
    frame(vcx, 16);
    wheel(vcx, true, 20.0);
    let start = panel.read_with(vcx, |panel, _| {
        assert_eq!(panel.transcript_wheel_glide.remaining, -20.0);
        panel.test_scroll_offset_y()
    });
    frame(vcx, 16);
    let first = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
    assert!(first > start && first < start + px(20.0));
    frame(vcx, 16);
    assert!(panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()) > first);
    wheel(vcx, false, 3.0);
    let chip = vcx.debug_bounds("jump-to-latest").unwrap();
    vcx.simulate_click(chip.center(), Default::default());
    frame(vcx, 16);
    panel.read_with(vcx, |panel, _| {
        assert!(panel.transcript_wheel_frame.is_none());
        assert_eq!(panel.transcript_wheel_glide.remaining, 0.0);
        assert!(panel.stick_to_bottom);
    });
}

#[gpui::test]
fn scroll_momentum_stops_at_edges_and_discards_suspended_frames(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = fixture(cx);
    // Already at the bottom. A downward wheel must not build up hidden debt.
    wheel(vcx, false, -100.0);
    frame(vcx, 16);
    assert!(panel.read_with(vcx, |panel, _| panel.transcript_wheel_frame.is_none()));
    wheel(vcx, false, 3.0);
    let before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
    frame(vcx, 500);
    panel.read_with(vcx, |panel, _| {
        assert_eq!(before, panel.test_scroll_offset_y());
        assert!(panel.transcript_wheel_frame.is_none());
    });
}

#[gpui::test]
fn scroll_momentum_reduced_motion_is_direct(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = fixture(cx);
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
    wheel(vcx, false, 3.0);
    panel.read_with(vcx, |panel, _| {
        assert!(panel.test_scroll_offset_y() > before);
        assert!(panel.transcript_wheel_frame.is_none());
    });
    // The virtual list's scrollbar estimate may change as new rows are measured.
    // Assert the actual reading position instead of that estimate.
    let before = panel.read_with(vcx, |panel, _| panel.transcript_list.logical_scroll_top());
    wheel(vcx, true, 20.0);
    panel.read_with(vcx, |panel, _| {
        let after = panel.transcript_list.logical_scroll_top();
        assert_eq!(before.item_ix, after.item_ix);
        assert!((f32::from(before.offset_in_item - after.offset_in_item) - 20.0).abs() < 0.1);
        assert!(panel.transcript_wheel_frame.is_none());
    });
}

#[gpui::test]
fn precise_scroll_moves_down_smoothly_and_settles_without_drift(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = fixture(cx);
    // Leave room to scroll down without hitting the live tail.
    panel.update(vcx, |panel, cx| panel.scroll_transcript_direct(300.0, cx));
    vcx.run_until_parked();
    let start = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
    wheel(vcx, true, -80.0);
    assert_eq!(
        start,
        panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y())
    );
    let mut previous = start;
    for _ in 0..16 {
        frame(vcx, 16);
        let current = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        assert!(current <= previous && current >= start - px(80.1));
        previous = current;
    }
    panel.read_with(vcx, |panel, _| {
        assert!((f32::from(panel.test_scroll_offset_y() - start) + 80.0).abs() < 0.1);
        assert!(panel.transcript_wheel_frame.is_none());
        assert!(!panel.transcript_wheel_frame_pending);
    });
}

#[gpui::test]
fn scroll_momentum_thumb_grab_cancels_pending_travel(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = fixture(cx);
    wheel(vcx, false, 3.0);
    frame(vcx, 16);
    let thumb = vcx.debug_bounds("transcript-scrollbar").unwrap();
    vcx.simulate_event(gpui::MouseDownEvent {
        position: thumb.center(),
        button: gpui::MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    let grabbed = panel.read_with(vcx, |panel, _| {
        assert!(panel.transcript_wheel_frame.is_none());
        assert!(!panel.stick_to_bottom);
        panel.test_scroll_offset_y()
    });
    frame(vcx, 16);
    assert_eq!(
        grabbed,
        panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y())
    );
    vcx.simulate_event(gpui::MouseUpEvent {
        position: thumb.center(),
        button: gpui::MouseButton::Left,
        modifiers: Default::default(),
        click_count: 1,
    });
}
