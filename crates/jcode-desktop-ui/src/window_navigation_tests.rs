//! Workspace shortcuts must not depend on a mounted keyboard-focus target.
use super::*;

#[gpui::test]
fn navigation_without_mounted_focus(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        // Hot reload installs the keymap again. Each key must still move once.
        for _ in 0..3 {
            crate::bind_workspace_keys(cx);
        }
    });
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("left", cx);
        workspace.push_test_panel("right", cx);
        workspace
    });
    vcx.run_until_parked();
    let detached = vcx.update(|_, cx| cx.focus_handle());
    for missing_focus in [true, false] {
        for (chord, row, panel) in [
            ("super-k", 0, Some(0)),
            ("super-j", 1, None),
            ("super-j", 2, None),
            ("super-j", 3, None),
            ("super-j", 3, None),
            ("super-k", 2, None),
            ("super-k", 1, None),
            ("super-k", 0, Some(0)),
            ("super-l", 0, Some(1)),
            ("super-h", 0, Some(0)),
        ] {
            vcx.update(|window, cx| {
                if missing_focus {
                    window.blur();
                } else {
                    // A removed picker/composer can leave a live FocusHandle
                    // that no longer has a node in the rendered dispatch tree.
                    window.focus(&detached, cx);
                }
            });
            // No settling frame between focus loss and the first keypress.
            vcx.simulate_keystrokes(chord);
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let workspace = workspace.read(cx);
                assert_eq!(
                    workspace.active_row, row,
                    "{chord}, missing={missing_focus}"
                );
                assert_eq!(
                    workspace.navigation_state(window, cx)["keyboard_panel"],
                    serde_json::json!(panel),
                    "{chord}, missing={missing_focus}"
                );
                if panel.is_none() {
                    assert!(workspace.focus_handle.is_focused(window));
                }
            });
        }
    }
}

#[gpui::test]
fn navigation_fallback_respects_keymap_overrides(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        cx.bind_keys([
            gpui::KeyBinding::new("super-j", gpui::NoAction, None),
            gpui::KeyBinding::new("ctrl-alt-v", FocusDown, None),
        ]);
    });
    let (workspace, vcx) =
        cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
    vcx.update(|window, _| window.blur());
    vcx.simulate_keystrokes("super-j");
    workspace.read_with(vcx, |workspace, _| assert_eq!(workspace.active_row, 0));
    vcx.simulate_keystrokes("ctrl-alt-v");
    workspace.read_with(vcx, |workspace, _| assert_eq!(workspace.active_row, 1));
}

#[gpui::test]
fn navigation_survives_root_replacement(cx: &mut gpui::TestAppContext) {
    cx.update(crate::bind_workspace_keys);
    let (mut workspace, vcx) =
        cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
    let mut old_roots = Vec::new();
    for _ in 0..3 {
        vcx.update(|window, _| window.blur());
        vcx.simulate_keystrokes("super-j");
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| assert_eq!(workspace.active_row, 1));
        for old in &old_roots {
            let old: &Entity<Workspace> = old;
            old.read_with(vcx, |workspace, _| assert_eq!(workspace.active_row, 1));
        }
        // Keep the old entity alive, as a reload may do while capturing state.
        // Its handlers must nevertheless disappear with the old rendered root.
        old_roots.push(workspace);
        workspace = vcx.update(|window, cx| {
            crate::bind_workspace_keys(cx);
            window.replace_root(cx, |_, cx| Workspace::for_test(learning::Coach::new(), cx))
        });
        vcx.run_until_parked();
    }
}
