//! Headless regressions for native canvas pinch routing and the spatial map.
//! Registered as a child of `map_gesture`, so private gesture state is testable.
use super::*;
use gpui::TouchPhase::{Cancelled, Ended, Moved, Started};

fn workspace_fixture(
    cx: &mut gpui::TestAppContext,
) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.show_sidebar = false;
        for name in ["first", "second", "lower"] {
            workspace.push_test_panel(name, cx);
        }
        workspace.slots[2].row = 1;
        workspace.active = 0;
        workspace.active_row = 0;
        workspace
    });
    workspace.update_in(vcx, |workspace, window, cx| {
        workspace.focus_active(window, cx);
    });
    vcx.run_until_parked();
    (workspace, vcx)
}

fn pinch(vcx: &mut gpui::VisualTestContext, delta: f32, phase: gpui::TouchPhase) {
    let position = vcx.debug_bounds("workspace-canvas").unwrap().center();
    vcx.simulate_event(gpui::PinchEvent {
        position,
        delta,
        modifiers: Default::default(),
        phase,
    });
    vcx.run_until_parked();
}

/// Workspace animation uses std::time::Instant, not the executor's fake clock.
/// Rebase the live transition into the past without sampling it here: render
/// must observe its completion and perform the actual focus-restoration path.
fn finish_overview_animation(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) {
    workspace.update_in(vcx, |workspace, window, cx| {
        let target = if workspace.overview { 1.0 } else { 0.0 };
        workspace.overview_progress = AnimatedValue::new(1.0 - target, Duration::from_millis(1));
        workspace
            .overview_progress
            .set(target, Instant::now() - Duration::from_secs(1));
        cx.notify();
        window.simulate_next_frame(cx);
    });
    vcx.run_until_parked();
    assert!(!workspace.read_with(vcx, |workspace, _| {
        workspace.overview_progress.is_animating()
    }));
}

#[gpui::test]
fn native_pinch_enters_and_leaves_overview_restoring_composer_focus(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = workspace_fixture(cx);
    let first = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    assert!(vcx.update(|window, cx| first.read(cx).input_focus_handle(cx).is_focused(window)));

    pinch(vcx, -0.25, Started);
    assert!(workspace.read_with(vcx, |workspace, _| workspace.overview));
    finish_overview_animation(&workspace, vcx);
    assert!(vcx.debug_bounds("overview-panel-0").is_some());
    assert!(vcx.update(|window, cx| workspace.read(cx).focus_handle.is_focused(window)));
    pinch(vcx, 0.0, Ended);

    pinch(vcx, 0.25, Started);
    assert!(!workspace.read_with(vcx, |workspace, _| workspace.overview));
    pinch(vcx, 0.0, Ended);
    finish_overview_animation(&workspace, vcx);
    assert!(vcx.debug_bounds("overview-panel-0").is_none());
    assert!(vcx.update(|window, cx| first.read(cx).input_focus_handle(cx).is_focused(window)));
    vcx.simulate_input("back in the active composer");
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, cx| {
        assert_eq!(workspace.active, 0);
        assert_eq!(workspace.active_row, 0);
        assert_eq!(
            first.read(cx).input.read(cx).content.as_ref(),
            "back in the active composer"
        );
        assert_eq!(
            workspace
                .test_panel(1)
                .unwrap()
                .read(cx)
                .input
                .read(cx)
                .content
                .as_ref(),
            ""
        );
    });
}

#[gpui::test]
fn native_small_pinch_deltas_accumulate_before_switching(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = workspace_fixture(cx);
    for phase in [Started, Moved, Moved] {
        pinch(vcx, -0.05, phase);
        assert!(!workspace.read_with(vcx, |workspace, _| workspace.overview));
    }
    // 0.95^3 > 0.82, but 0.95^4 < 0.82.
    pinch(vcx, -0.05, Moved);
    assert!(workspace.read_with(vcx, |workspace, _| workspace.overview));
    pinch(vcx, 0.0, Ended);
    finish_overview_animation(&workspace, vcx);
    for phase in [Started, Moved, Moved] {
        pinch(vcx, 0.05, phase);
        assert!(workspace.read_with(vcx, |workspace, _| workspace.overview));
    }
    pinch(vcx, 0.05, Moved);
    assert!(!workspace.read_with(vcx, |workspace, _| workspace.overview));
}

#[gpui::test]
fn cancelled_pinch_discards_partial_scale_and_rearms_latch(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = workspace_fixture(cx);
    pinch(vcx, -0.10, Started);
    // A cancellation carries no zoom, even if the platform supplies a delta.
    pinch(vcx, -0.90, Cancelled);
    workspace.read_with(vcx, |workspace, _| {
        assert!(!workspace.overview);
        assert_eq!(workspace.map_gesture.scale, 1.0);
        assert!(!workspace.map_gesture.switched);
    });
    // Moved deliberately has no preceding Started, to prove cancellation itself
    // cleared the accumulator rather than relying on a later start to reset it.
    pinch(vcx, -0.10, Moved);
    assert!(!workspace.read_with(vcx, |workspace, _| workspace.overview));
    pinch(vcx, -0.10, Moved);
    assert!(workspace.read_with(vcx, |workspace, _| workspace.overview));
    pinch(vcx, 0.90, Cancelled);
    workspace.read_with(vcx, |workspace, _| {
        assert!(workspace.overview);
        assert_eq!(workspace.map_gesture.scale, 1.0);
        assert!(!workspace.map_gesture.switched);
    });
    pinch(vcx, 0.25, Started);
    assert!(!workspace.read_with(vcx, |workspace, _| workspace.overview));
}

#[gpui::test]
fn one_native_pinch_cannot_toggle_twice_when_direction_reverses(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = workspace_fixture(cx);
    pinch(vcx, -0.25, Started);
    finish_overview_animation(&workspace, vcx);
    for delta in [0.90, 0.90, -0.90, 0.90] {
        pinch(vcx, delta, Moved);
        assert!(workspace.read_with(vcx, |workspace, _| workspace.overview));
    }
    pinch(vcx, 0.0, Ended);
    pinch(vcx, 0.25, Started);
    finish_overview_animation(&workspace, vcx);
    for delta in [-0.90, -0.90, 0.90, -0.90] {
        pinch(vcx, delta, Moved);
        assert!(!workspace.read_with(vcx, |workspace, _| workspace.overview));
    }
}

#[gpui::test]
fn overview_cards_keep_workspace_rows_and_native_scroll_pans_without_activation(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = workspace_fixture(cx);
    pinch(vcx, -0.25, Started);
    pinch(vcx, 0.0, Ended);
    finish_overview_animation(&workspace, vcx);
    let first = vcx.debug_bounds("overview-panel-0").unwrap();
    let second = vcx.debug_bounds("overview-panel-1").unwrap();
    let lower = vcx.debug_bounds("overview-panel-2").unwrap();
    assert_eq!(first.top(), second.top());
    assert_eq!(second.left() - first.left(), px(316.0));
    assert_eq!(first.left(), lower.left());
    assert_eq!(lower.top() - first.top(), px(206.0));

    let position = vcx.debug_bounds("workspace-canvas").unwrap().center();
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(-40.0), px(-30.0))),
        modifiers: Default::default(),
        touch_phase: Started,
    });
    vcx.run_until_parked();
    for (selector, before) in [
        ("overview-panel-0", first),
        ("overview-panel-1", second),
        ("overview-panel-2", lower),
    ] {
        let after = vcx.debug_bounds(selector).unwrap();
        assert_eq!(after.left() - before.left(), px(-40.0));
        assert_eq!(after.top() - before.top(), px(-30.0));
    }
    workspace.read_with(vcx, |workspace, _| {
        assert!(workspace.overview);
        assert_eq!(workspace.active, 0);
        assert_eq!(workspace.active_row, 0);
        assert_eq!(workspace.map_gesture.pan, (-40.0, -30.0));
    });
}

#[test]
fn invalid_pinch_deltas_do_not_poison_scale_or_switch() {
    let mut gesture = MapGesture::default();
    for delta in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, -2.0] {
        assert!(!gesture.update(delta, false));
        assert_eq!(gesture.scale, 1.0);
        assert!(!gesture.switched);
    }
    assert!(gesture.update(-0.25, false));
}

#[gpui::test]
fn clicking_a_map_card_activates_its_row_and_restores_its_composer(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = workspace_fixture(cx);
    pinch(vcx, -0.25, Started);
    pinch(vcx, 0.0, Ended);
    finish_overview_animation(&workspace, vcx);
    let target = vcx.debug_bounds("overview-panel-2").unwrap().center();
    vcx.simulate_click(target, Default::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        assert!(!workspace.overview);
        assert_eq!(workspace.active, 2);
        assert_eq!(workspace.active_row, 1);
    });
    finish_overview_animation(&workspace, vcx);
    let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(2).unwrap());
    assert!(vcx.update(|window, cx| panel.read(cx).input_focus_handle(cx).is_focused(window)));
    vcx.simulate_input("selected from map");
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "selected from map");
    });
}

#[gpui::test]
fn dragging_map_background_pans_cards_without_changing_active_session(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = workspace_fixture(cx);
    pinch(vcx, -0.25, Started);
    pinch(vcx, 0.0, Ended);
    finish_overview_animation(&workspace, vcx);
    let first = vcx.debug_bounds("overview-panel-0").unwrap();
    let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
    // Above the cards, so the press cannot activate a session.
    let start = gpui::point(canvas.left() + px(200.0), canvas.top() + px(40.0));
    let end = start + gpui::point(px(-40.0), px(-30.0));
    vcx.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(end, gpui::MouseButton::Left, Default::default());
    vcx.run_until_parked();
    let after = vcx.debug_bounds("overview-panel-0").unwrap();
    assert_eq!(after.left() - first.left(), px(-40.0));
    assert_eq!(after.top() - first.top(), px(-30.0));
    vcx.simulate_mouse_up(end, gpui::MouseButton::Left, Default::default());
    // Motion after releasing must stop dragging and must not pan any further.
    vcx.simulate_mouse_move(start, None, Default::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        assert!(workspace.overview);
        assert_eq!(workspace.active, 0);
        assert_eq!(workspace.active_row, 0);
        assert_eq!(workspace.map_gesture.pan, (-40.0, -30.0));
        assert!(workspace.map_gesture.drag.is_none());
    });
}
