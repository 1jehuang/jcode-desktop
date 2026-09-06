//! Keyboard navigation must ignore dismissed panels while their surfaces fade.
use super::*;

fn check_navigation_across_closing_panels(cx: &mut gpui::TestAppContext, right: bool) {
    cx.update(crate::bind_workspace_keys);
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for name in ["left", "closing-one", "closing-two", "right"] {
            workspace.push_test_panel(name, cx);
        }
        // Keep dismissal in flight independently of test executor wall time.
        for slot in &mut workspace.slots {
            slot.close_progress = AnimatedValue::new(1.0, Duration::from_secs(60));
        }
        workspace.set_active(1, cx);
        workspace
    });
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| workspace.focus_active(window, cx));
    });
    vcx.run_until_parked();
    for _ in 0..2 {
        vcx.simulate_keystrokes("super-q");
        vcx.run_until_parked();
    }
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.active, 3);
        assert!(workspace.slots[1].closing);
        assert!(workspace.slots[2].closing);
    });
    if right {
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.set_active(0, cx);
                workspace.focus_active(window, cx);
            });
        });
        vcx.run_until_parked();
    }
    let (chord, expected) = if right {
        ("super-l", 3)
    } else {
        ("super-h", 0)
    };
    vcx.simulate_keystrokes(chord);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let workspace = workspace.read(cx);
        assert_eq!(
            workspace.active, expected,
            "{chord} must skip both closing slots"
        );
        assert!(!workspace.slots[workspace.active].closing);
        assert_eq!(
            workspace.navigation_state(window, cx)["keyboard_panel"],
            expected
        );
    });
    // Repeating into the outer edge remains a no-op, not a wraparound.
    vcx.simulate_keystrokes(chord);
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| assert_eq!(workspace.active, expected));
}

#[gpui::test]
fn focus_left_skips_closing_panels(cx: &mut gpui::TestAppContext) {
    check_navigation_across_closing_panels(cx, false);
}

#[gpui::test]
fn focus_right_skips_closing_panels(cx: &mut gpui::TestAppContext) {
    check_navigation_across_closing_panels(cx, true);
}
