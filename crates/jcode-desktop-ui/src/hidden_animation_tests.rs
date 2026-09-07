use super::*;

/// Seed expired transitions without wall-clock sleeps, then draw the real
/// overview. Its content path must settle the strips it no longer renders.
#[gpui::test]
fn hidden_strip_animations_stop_requesting_frames_in_overview(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("hidden-animation", cx);
        workspace.show_sidebar = false;
        workspace
    });
    vcx.run_until_parked();
    workspace.update(vcx, |workspace, cx| {
        let old = Instant::now() - Duration::from_secs(2);
        workspace.overview = true;
        workspace.overview_progress = AnimatedValue::new(1.0, Duration::ZERO);
        workspace.slots[0].animated_width = AnimatedValue::new(0.5, CAMERA_DURATION);
        workspace.slots[0].animated_width.set(0.75, old);
        workspace.slots[0].width_fraction = 0.75;
        workspace.slots[0].order_offset = AnimatedValue::new(1.0, CAMERA_DURATION);
        workspace.slots[0].order_offset.set(0.0, old);
        workspace.camera_from[0] = 0.0;
        workspace.camera_target[0] = 125.0;
        workspace.camera_started[0] = Some(old);
        workspace.camera_touch_pan[0] = true;
        cx.notify();
    });
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        assert!(workspace.overview);
        assert!(
            !workspace.animation_active(),
            "expired hidden transitions must not perpetually arm the 8 ms frame timer"
        );
        assert!(!workspace.slots[0].animated_width.is_animating());
        assert!(!workspace.slots[0].order_offset.is_animating());
        assert!(workspace.camera_started[0].is_none());
        assert_eq!(workspace.camera_x[0], 125.0);
        assert!(!workspace.camera_touch_pan[0]);
    });
}

#[gpui::test]
fn hidden_strip_sampling_preserves_visible_rows_and_natural_deadlines(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for name in ["incoming", "outgoing", "hidden"] {
            workspace.push_test_panel(name, cx);
        }
        workspace
    });
    workspace.update(vcx, |workspace, _| {
        let start = Instant::now();
        let duration = Duration::from_millis(150);
        workspace.active_row = 0;
        workspace.outgoing_row = Some(1);
        for (row, slot) in workspace.slots.iter_mut().enumerate() {
            slot.row = row;
            slot.animated_width = AnimatedValue::new(0.5, duration);
            slot.animated_width.set(0.75, start);
            slot.order_offset = AnimatedValue::new(1.0, duration);
            slot.order_offset.set(0.0, start);
            workspace.camera_started[row] = Some(start);
            workspace.camera_target[row] = 100.0 + row as f32;
            workspace.camera_touch_pan[row] = false;
        }
        workspace.advance_hidden_strip_animations(start, false);
        assert!(workspace.slots[2].animated_width.is_animating());
        assert_eq!(workspace.camera_started[2], Some(start));
        workspace.advance_hidden_strip_animations(start + Duration::from_secs(1), false);
        for row in [0, 1] {
            assert!(
                workspace.slots[row].animated_width.is_animating(),
                "rendered rows retain their own sampling"
            );
            assert!(workspace.slots[row].order_offset.is_animating());
            assert_eq!(workspace.camera_started[row], Some(start));
        }
        assert!(!workspace.slots[2].animated_width.is_animating());
        assert!(!workspace.slots[2].order_offset.is_animating());
        assert_eq!(workspace.slots[2].animated_width.sample(start), 0.75);
        assert_eq!(workspace.slots[2].order_offset.sample(start), 0.0);
        assert!(workspace.camera_started[2].is_none());
        assert_eq!(workspace.camera_x[2], 102.0);

        // Once a row transition ends, its outgoing row becomes hidden too.
        workspace.outgoing_row = None;
        workspace.advance_hidden_strip_animations(start + Duration::from_secs(1), false);
        assert!(!workspace.slots[1].animated_width.is_animating());
        assert!(workspace.camera_started[1].is_none());
        assert!(workspace.slots[0].animated_width.is_animating());
    });
}
