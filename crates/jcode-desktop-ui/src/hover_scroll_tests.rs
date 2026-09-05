//! Wheel routing follows the painted panel, not the active keyboard target.
use super::*;

fn assert_hover_scroll(cx: &mut gpui::TestAppContext, precise: bool, active_empty: bool) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.show_sidebar = false;
        for name in ["active", "hovered"] {
            workspace.push_test_panel(name, cx);
        }
        for slot in &mut workspace.slots {
            slot.width_fraction = 0.5;
            slot.animated_width =
                AnimatedValue::new(0.5, transition::policy(Transition::PanelWidth).duration);
        }
        workspace.active = 0;
        workspace
    });
    let panels = workspace.read_with(vcx, |w, _| {
        [w.test_panel(0).unwrap(), w.test_panel(1).unwrap()]
    });
    for (index, panel) in panels.iter().enumerate() {
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            if index == 1 || !active_empty {
                for n in 0..80 {
                    panel.items.push(crate::panel::Item::Assistant(format!(
                        "Panel {index} message {n}"
                    )));
                }
            }
            cx.notify();
        });
    }
    vcx.run_until_parked();
    let before = panels
        .each_ref()
        .map(|panel| panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()));
    let target = vcx.debug_bounds("panel-1").unwrap().center();
    let camera_before = workspace.read_with(vcx, |w, _| w.camera_x);
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: target,
        delta: if precise {
            gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(80.)))
        } else {
            gpui::ScrollDelta::Lines(gpui::point(0., 3.))
        },
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Started,
    });
    vcx.run_until_parked();
    vcx.executor().advance_clock(Duration::from_millis(16));
    vcx.run_until_parked();
    let after = panels
        .each_ref()
        .map(|panel| panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()));
    assert_ne!(before[1], after[1], "the hovered panel must scroll");
    assert_eq!(before[0], after[0], "the active panel must not scroll");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.active, 0, "hover scrolling must not steal keyboard focus");
        assert_eq!(w.active_row, 0, "hover scrolling must not navigate strips");
        assert_eq!(w.camera_x, camera_before);
    });

    // Without clicking, move the wheel back to the active panel. An empty
    // panel retains workspace navigation; a populated one scrolls itself.
    let target = vcx.debug_bounds("panel-0").unwrap().center();
    let direction = if active_empty { -1.0 } else { 1.0 };
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: target,
        delta: if precise {
            gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(direction * STRIP_BREAK * 1.2)))
        } else {
            gpui::ScrollDelta::Lines(gpui::point(0., direction * 3.))
        },
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Started,
    });
    vcx.run_until_parked();
    vcx.executor().advance_clock(Duration::from_millis(16));
    vcx.run_until_parked();
    if active_empty {
        assert_eq!(workspace.read_with(vcx, |w, _| w.active_row), 1);
    } else {
        let active_after = panels[0].read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        assert_ne!(
            after[0], active_after,
            "moving the pointer retargets scrolling"
        );
        assert_eq!(workspace.read_with(vcx, |w, _| w.active_row), 0);
    }
}

#[gpui::test]
fn touchpad_scrolls_hovered_panel_when_active_panel_is_empty(cx: &mut gpui::TestAppContext) {
    assert_hover_scroll(cx, true, true);
}

#[gpui::test]
fn wheel_scrolls_hovered_panel_when_active_panel_is_empty(cx: &mut gpui::TestAppContext) {
    assert_hover_scroll(cx, false, true);
}

#[gpui::test]
fn touchpad_scrolls_only_hovered_populated_panel(cx: &mut gpui::TestAppContext) {
    assert_hover_scroll(cx, true, false);
}

#[gpui::test]
fn wheel_scrolls_only_hovered_populated_panel(cx: &mut gpui::TestAppContext) {
    assert_hover_scroll(cx, false, false);
}
