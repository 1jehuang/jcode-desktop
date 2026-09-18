use super::*;

#[gpui::test]
fn title_actions_fit_and_rename_does_not_select_or_close(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.push_test_panel("session_fox_original", cx);
        w.push_test_panel("session_owl_neighbor", cx);
        w
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for width in [1440., 800., 640.] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
        vcx.run_until_parked();
        let tab = vcx.debug_bounds("live-session-tab-0").unwrap();
        let title = vcx.debug_bounds("live-session-tab-0-title").unwrap();
        let rename = vcx.debug_bounds("rename-session-button").unwrap();
        let close = vcx.debug_bounds("close-session-button-0").unwrap();
        assert!(title.right() <= rename.left());
        assert!(rename.right() <= close.left());
        assert!(close.right() <= tab.right());
        vcx.update(|window, cx| window.simulate_mouse_move(tab.center(), cx));
        vcx.run_until_parked();
        vcx.simulate_click(rename.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.active, 0);
            assert_eq!(w.slots.len(), 2);
            assert!(w.slots.iter().all(|slot| !slot.closing));
            assert!(w.rename_editor.is_some());
        });
        vcx.update(|_, cx| workspace.update(cx, |w, cx| w.close_rename_editor(cx)));
        vcx.run_until_parked();
    }
}

#[gpui::test]
fn hovered_inactive_tab_close_targets_that_tab_only(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.push_test_panel("session_fox_original", cx);
        w.push_test_panel("session_owl_neighbor", cx);
        w
    });
    vcx.run_until_parked();
    let close = vcx.debug_bounds("close-session-button-1").unwrap();
    vcx.update(|window, cx| window.simulate_mouse_move(close.center(), cx));
    vcx.run_until_parked();
    vcx.simulate_click(close.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        let surviving: Vec<_> = w
            .slots
            .iter()
            .filter(|slot| !slot.closing)
            .map(|slot| slot.panel.read(cx).session_id.as_str())
            .collect();
        assert_eq!(surviving, ["session_fox_original"]);
        assert!(w.rename_editor.is_none());
    });
}
