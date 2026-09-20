use super::*;

#[gpui::test]
fn fps_and_new_session_share_the_tab_row_without_a_top_header(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("fps-layout", cx);
        workspace
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for width in [1440.0, 1000.0, 800.0, 640.0, 420.0, 360.0] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(700.0)));
        for layout in [
            crate::config::LayoutMode::FolderTabs,
            crate::config::LayoutMode::Normal,
        ] {
            for sidebar in [true, false] {
                for minimap in [true, false] {
                    workspace.update(vcx, |workspace, cx| {
                        workspace.layout_mode = layout;
                        workspace.show_sidebar = sidebar;
                        workspace.show_minimap = minimap;
                        cx.notify();
                    });
                    vcx.run_until_parked();
                    let row = vcx.debug_bounds("workspace-tab-row").unwrap();
                    let counter = vcx.debug_bounds("fps-counter").unwrap();
                    let tabs = vcx.debug_bounds("live-session-tabs").unwrap();
                    let plus = vcx.debug_bounds("tab-new-session").unwrap();
                    let body = vcx.debug_bounds("workspace-body").unwrap();
                    let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
                    assert_eq!(
                        vcx.debug_bounds("minimap").is_some(),
                        minimap && live_tabs::minimap_fits_header(f32::from(canvas.size.width)),
                        "minimap yields to navigation without changing its saved preference"
                    );
                    assert_eq!(workspace.read_with(vcx, |w, _| w.show_minimap), minimap);
                    assert!(vcx.debug_bounds("fps-header").is_none());
                    assert!(vcx.debug_bounds("edge-new-session").is_none());
                    assert_eq!(body.top(), px(0.0));
                    assert_eq!(row.top(), canvas.top() + px(STRIP_PADDING_TOP));
                    assert_eq!(row.size.height, px(FOLDER_CONTENT_INSET));
                    assert!(counter.left() >= row.left() && counter.right() <= tabs.left());
                    assert!(counter.top() >= row.top() && counter.bottom() <= row.bottom());
                    assert!((counter.center().y - plus.center().y).abs() < px(1.0));
                    assert!(
                        plus.left() >= tabs.right() && plus.right() <= row.right(),
                        "width={width}, sidebar={sidebar}, minimap={minimap}, plus={plus:?}, tabs={tabs:?}, row={row:?}"
                    );
                    assert_eq!(plus.top(), tabs.top());
                    assert!(
                        tabs.size.width > px(0.0),
                        "width={width}, sidebar={sidebar}, minimap={minimap}"
                    );
                    if let Some(version) = vcx.debug_bounds("workspace-version") {
                        assert!(version.left() >= tabs.right());
                        assert!(version.right() <= plus.left());
                        assert!(tabs.size.width >= px(208.0));
                    }
                    if sidebar {
                        assert_eq!(
                            vcx.debug_bounds(if responsive::is_compact(width) {
                                "compact-sidebar"
                            } else {
                                "sidebar"
                            })
                            .unwrap()
                            .top(),
                            body.top()
                        );
                    }
                }
            }
        }
    }
}
