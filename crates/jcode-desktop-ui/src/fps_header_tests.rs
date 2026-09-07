use super::*;

#[gpui::test]
fn fps_header_is_in_flow_above_tabs_and_canvas(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.layout_mode = crate::config::LayoutMode::FolderTabs;
        workspace.push_test_panel("fps-layout", cx);
        workspace
    });
    for (layout, sidebar) in [
        (crate::config::LayoutMode::FolderTabs, true),
        (crate::config::LayoutMode::FolderTabs, false),
        (crate::config::LayoutMode::Normal, true),
        (crate::config::LayoutMode::Normal, false),
    ] {
        workspace.update(vcx, |workspace, cx| {
            workspace.layout_mode = layout;
            workspace.show_sidebar = sidebar;
            cx.notify();
        });
        vcx.run_until_parked();
        let header = vcx.debug_bounds("fps-header").unwrap();
        let counter = vcx.debug_bounds("fps-counter").unwrap();
        let body = vcx.debug_bounds("workspace-body").unwrap();
        let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
        assert_eq!(header.size.height, px(FPS_HEADER_HEIGHT));
        assert_eq!(header.bottom(), body.top());
        assert!(canvas.top() >= header.bottom());
        assert!((f32::from(header.center().x - counter.center().x)).abs() < 1.0);
        assert!(counter.top() >= header.top() && counter.bottom() <= header.bottom());
        if sidebar {
            assert!(vcx.debug_bounds("sidebar").unwrap().top() >= header.bottom());
        }
    }
}
