//! Exercise the real Todos panel inside workspace gesture routing, without API calls.
use super::*;

fn install_tasks(panel: &Entity<Panel>, count: usize, cx: &mut gpui::VisualTestContext) {
    panel.update(cx, |panel, cx| {
        panel.items.clear();
        panel.todoist = Some(TodoistPanelState::fixture(count));
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
}

#[gpui::test]
fn todos_scroll_with_wheel_and_touchpad_while_chrome_stays_fixed(cx: &mut gpui::TestAppContext) {
    let (workspace, cx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("todos-scroll", cx);
        workspace
    });
    let panel = workspace.read_with(cx, |w, _| w.test_panel(0)).unwrap();
    let window = cx.update(|window, _| window.window_handle());
    for (width, height) in [(1440., 1000.), (800., 600.)] {
        cx.simulate_window_resize(window, gpui::size(px(width), px(height)));
        install_tasks(&panel, 80, cx);
        let viewport = cx.debug_bounds("todoist-task-list").unwrap();
        let header = cx.debug_bounds("todoist-header").unwrap();
        let composer = cx.debug_bounds("todoist-composer").unwrap();
        let first = cx.debug_bounds("todoist-task-0").unwrap();
        assert!(
            first.size.height > px(30.),
            "rows must not shrink to fit the viewport"
        );
        assert!(header.bottom() <= viewport.top());
        assert!(viewport.bottom() <= composer.top());
        assert!(composer.bottom() <= px(height));
        assert!(
            cx.debug_bounds("todoist-scrollbar").is_some(),
            "scrollbar appears before interaction"
        );
        let scroll = panel.read_with(cx, |p, _| p.todoist.as_ref().unwrap().scroll.clone());
        assert!(scroll.max_offset().y > px(0.));
        for delta in [
            gpui::ScrollDelta::Lines(point(0., -3.)),
            gpui::ScrollDelta::Pixels(point(px(0.), px(-120.))),
        ] {
            let before = scroll.offset().y;
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: viewport.center(),
                delta,
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            cx.run_until_parked();
            assert!(
                scroll.offset().y < before,
                "input must move the task list downward"
            );
            assert_eq!(header, cx.debug_bounds("todoist-header").unwrap());
            assert_eq!(composer, cx.debug_bounds("todoist-composer").unwrap());
        }
        assert!(cx.debug_bounds("todoist-task-0").unwrap().top() < first.top());
        let offset = scroll.offset();
        panel.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(
            scroll.offset(),
            offset,
            "redraw preserves the reading position"
        );
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-100_000.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        let last = cx.debug_bounds("todoist-task-79").unwrap();
        assert!(last.top() >= viewport.top());
        assert!(
            last.bottom() <= viewport.bottom(),
            "last task can be reached"
        );
    }
}

#[gpui::test]
fn todos_short_and_empty_lists_do_not_show_a_scrollbar(cx: &mut gpui::TestAppContext) {
    let (workspace, cx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("todos-short", cx);
        workspace
    });
    let panel = workspace.read_with(cx, |w, _| w.test_panel(0)).unwrap();
    for count in [0, 2] {
        install_tasks(&panel, count, cx);
        assert!(cx.debug_bounds("todoist-scrollbar").is_none());
        assert_eq!(
            panel.read_with(cx, |p, _| p.todoist.as_ref().unwrap().scroll.max_offset().y),
            px(0.)
        );
    }
}

#[gpui::test]
fn todos_scrollbar_updates_after_loading_and_filtering(cx: &mut gpui::TestAppContext) {
    let (workspace, cx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("todos-load", cx);
        workspace
    });
    let panel = workspace.read_with(cx, |w, _| w.test_panel(0)).unwrap();
    install_tasks(&panel, 0, cx);
    assert!(cx.debug_bounds("todoist-scrollbar").is_none());
    panel.update(cx, |panel, cx| {
        panel.todoist.as_mut().unwrap().tasks = TodoistPanelState::fixture(80).tasks;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("todoist-scrollbar").is_some());
    panel.update(cx, |panel, cx| {
        panel.todoist.as_mut().unwrap().selected_project = Some("empty-project".into());
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("todoist-scrollbar").is_none());
}
