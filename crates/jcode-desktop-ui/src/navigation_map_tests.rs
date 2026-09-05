//! Cross-cutting keyboard-navigation and minimap consistency regressions.
//!
//! This is a child module of `workspace`, so the tests deliberately inspect the
//! workspace's private slot order and active-row state as well as the rendered
//! map.  Keeping both assertions in the same test catches state/render drift.

use super::*;

fn focused_workspace_with_map<'a>(
    cx: &'a mut gpui::TestAppContext,
    names: &[&str],
) -> (gpui::Entity<Workspace>, &'a mut gpui::VisualTestContext) {
    cx.update(|cx| crate::bind_workspace_keys(cx));
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.enable_test_minimap();
        for name in names {
            workspace.push_test_panel(name, cx);
        }
        workspace
    });
    vcx.update(|window, cx| {
        window.focus(&workspace.read(cx).focus_handle.clone(), cx);
    });
    vcx.run_until_parked();
    (workspace, vcx)
}

fn assert_pin_inside(vcx: &mut gpui::VisualTestContext, panel_index: usize) {
    let (panel_selector, focused_selector) = match panel_index {
        0 => ("minimap-panel-0", "minimap-panel-0-focused"),
        1 => ("minimap-panel-1", "minimap-panel-1-focused"),
        2 => ("minimap-panel-2", "minimap-panel-2-focused"),
        3 => ("minimap-panel-3", "minimap-panel-3-focused"),
        _ => panic!("test selector missing for minimap panel {panel_index}"),
    };
    let panel = vcx
        .debug_bounds(panel_selector)
        .expect("focused panel should be painted on the minimap");
    let pin = vcx
        .debug_bounds("minimap-you-pin")
        .expect("a populated active row should paint its focus pin");
    assert!(
        panel.contains(&pin.center()),
        "focus pin must be inside minimap panel {panel_index}: panel={panel:?}, pin={pin:?}"
    );
    assert!(
        vcx.debug_bounds(focused_selector).is_some(),
        "the panel containing the pin must own the focused marker"
    );
}

#[gpui::test]
fn repeated_key_binding_still_moves_exactly_one_panel_and_keeps_map_in_sync(
    cx: &mut gpui::TestAppContext,
) {
    // Plugin reloads may install the workspace keymap again. One physical key
    // press must still produce one navigation action.
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        crate::bind_workspace_keys(cx);
        crate::bind_workspace_keys(cx);
    });
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.enable_test_minimap();
        for name in ["one", "two", "three", "four"] {
            workspace.push_test_panel(name, cx);
        }
        workspace.active = 1;
        workspace
    });
    vcx.update(|window, cx| window.focus(&workspace.read(cx).focus_handle.clone(), cx));
    vcx.run_until_parked();

    vcx.simulate_keystrokes("super-l");
    vcx.run_until_parked();

    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.test_focus_position(), Some(2));
        assert_eq!(workspace.active, 2);
    });
    assert_pin_inside(vcx, 2);
}

#[gpui::test]
fn keyboard_focus_and_reorder_keep_slot_order_and_map_pin_aligned(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = focused_workspace_with_map(cx, &["one", "two", "three"]);

    vcx.simulate_keystrokes("super-l super-shift-l super-h");
    vcx.run_until_parked();

    workspace.read_with(vcx, |workspace, cx| {
        let order = workspace
            .row_indices(0)
            .map(|index| workspace.slots[index].panel.read(cx).session_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(order, ["one", "three", "two"]);
        assert_eq!(workspace.test_focus_position(), Some(1));
        assert_eq!(
            workspace.slots[workspace.active].panel.read(cx).session_id,
            "three"
        );
    });
    assert_pin_inside(vcx, 1);
}

#[gpui::test]
fn empty_active_row_has_no_stale_focused_panel_or_pin(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = focused_workspace_with_map(cx, &["one", "two"]);
    assert_pin_inside(vcx, 0);

    let empty_row = vcx
        .debug_bounds("minimap-row-2")
        .expect("every workspace row should be represented on the minimap");
    vcx.simulate_click(empty_row.center(), gpui::Modifiers::default());
    vcx.run_until_parked();

    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.active_row, 2);
        assert!(workspace.row_indices(2).next().is_none());
    });
    assert!(
        vcx.debug_bounds("minimap-you-pin").is_none(),
        "an empty active row cannot have a panel focus pin"
    );
    for (index, selector) in [
        (0, "minimap-panel-0-focused"),
        (1, "minimap-panel-1-focused"),
    ] {
        assert!(
            vcx.debug_bounds(selector).is_none(),
            "panel {index} on an inactive row must not retain a focused marker"
        );
    }
}

#[gpui::test]
fn clicking_minimap_panel_restores_row_focus_and_pin_to_that_rectangle(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = focused_workspace_with_map(cx, &["one", "two", "three"]);
    vcx.simulate_keystrokes("super-j");
    vcx.run_until_parked();
    assert_eq!(
        workspace.read_with(vcx, |workspace, _| workspace.active_row),
        1
    );

    let target = vcx
        .debug_bounds("minimap-panel-0")
        .expect("first panel should be clickable from another active row");
    vcx.simulate_click(target.center(), gpui::Modifiers::default());
    vcx.run_until_parked();

    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.active_row, 0);
        assert_eq!(workspace.test_focus_position(), Some(0));
    });
    assert_pin_inside(vcx, 0);
}
