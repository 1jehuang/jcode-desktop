//! Folder surfaces remain joined while focus changes their top edge, not width.
use super::*;

#[gpui::test]
fn folder_panels_keep_canvas_space_and_transfer_the_raised_tab(cx: &mut gpui::TestAppContext) {
    cx.update(crate::bind_workspace_keys);
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for name in ["Left folder", "Focused folder", "Right folder"] {
            workspace.push_test_panel(name, cx);
        }
        for slot in &mut workspace.slots {
            slot.width_fraction = 1.0 / 3.0;
            slot.animated_width = AnimatedValue::new(
                slot.width_fraction,
                transition::policy(Transition::PanelWidth).duration,
            );
        }
        workspace.active = 1;
        workspace
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for (width, height) in [(800., 600.), (1440., 1000.)] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        for sidebar in [true, false] {
            workspace.update(vcx, |workspace, cx| {
                workspace.show_sidebar = sidebar;
                workspace.active = 1;
                workspace.camera_dirty.fill(true);
                cx.notify();
            });
            vcx.run_until_parked();
            for focused in [1, 0, 2] {
                if focused != 1 {
                    vcx.update(|window, cx| {
                        window.focus(&workspace.read(cx).focus_handle.clone(), cx);
                    });
                    vcx.simulate_keystrokes(if focused == 0 { "super-u" } else { "super-p" });
                    vcx.run_until_parked();
                }
                assert_eq!(workspace.read_with(vcx, |w, _| w.active), focused);
                let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
                let backing = vcx.debug_bounds("folder-backing-bridge").unwrap();
                let expected_left = if sidebar {
                    SIDEBAR_WIDTH + FOLDER_CONNECTOR_WIDTH
                } else {
                    0.0
                };
                assert_eq!(canvas.left(), px(expected_left));
                assert_eq!(
                    backing.left(),
                    px(if sidebar { SIDEBAR_WIDTH } else { 0.0 })
                );
                assert_eq!(backing.bottom(), canvas.bottom());
                let panels = ["panel-0", "panel-1", "panel-2"]
                    .into_iter()
                    .map(|id| vcx.debug_bounds(id).unwrap())
                    .collect::<Vec<_>>();
                for (index, panel) in panels.iter().enumerate() {
                    let inset = if index == focused {
                        0.
                    } else {
                        INACTIVE_PANEL_INSET
                    };
                    assert!(
                        (f32::from(panel.top() - canvas.top()) - STRIP_PADDING_Y - inset).abs()
                            < 1.
                    );
                    assert!(
                        (f32::from(canvas.bottom() - panel.bottom()) - STRIP_PADDING_Y).abs() < 1.
                    );
                    assert!(panel.size.height > px(200.));
                    assert_eq!(panel.bottom(), backing.top());
                }
                for pair in panels.windows(2) {
                    assert!(
                        (f32::from(pair[0].right() - pair[1].left())).abs() < 1.,
                        "folders must touch without gaps or overlap"
                    );
                    assert_eq!(
                        pair[0].bottom(),
                        pair[1].bottom(),
                        "folders share one baseline"
                    );
                }
            }
        }
    }
}
