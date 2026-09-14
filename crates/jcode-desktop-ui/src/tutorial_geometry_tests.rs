//! End-to-end layout and interaction checks for the opt-in Learn panel.
use super::*;

fn overlap_area(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> f32 {
    f32::from(a.right().min(b.right()) - a.left().max(b.left())).max(0.0)
        * f32::from(a.bottom().min(b.bottom()) - a.top().max(b.top())).max(0.0)
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"));
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

const LESSONS: &[&str] = &[
    "tutorial-nav-left",
    "tutorial-nav-down",
    "tutorial-nav-up",
    "tutorial-nav-right",
    "tutorial-new",
    "tutorial-close",
    "tutorial-move-left",
    "tutorial-move-down",
    "tutorial-move-up",
    "tutorial-move-right",
    "tutorial-width-presets",
    "tutorial-resize",
    "tutorial-maximize",
    "tutorial-overview",
];

fn assert_all_lessons(vcx: &mut gpui::VisualTestContext) {
    for id in LESSONS {
        assert!(vcx.debug_bounds(*id).is_some(), "missing lesson {id}");
    }
    for id in ["tutorial-next", "tutorial-back", "tutorial-stage"] {
        assert!(
            vcx.debug_bounds(id).is_none(),
            "pagination must not return: {id}"
        );
    }
}

fn scroll_to_bottom(vcx: &mut gpui::VisualTestContext) {
    let content = vcx.debug_bounds("tutorial-content").unwrap();
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: content.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-2000.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    vcx.run_until_parked();
}

#[gpui::test]
fn tutorial_tab_lists_all_lessons_without_resizing_canvas(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) =
        cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
    vcx.run_until_parked();
    let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
    assert!(vcx.debug_bounds("tutorial-guides").is_none());
    click(vcx, "sidebar-learn-tab");
    assert_all_lessons(vcx);
    assert_eq!(vcx.debug_bounds("workspace-canvas").unwrap(), canvas);
    click(vcx, "sidebar-sessions-tab");
    assert!(vcx.debug_bounds("tutorial-guides").is_none());
    click(vcx, "sidebar-learn-tab");
    assert_all_lessons(vcx);
}

#[gpui::test]
fn tutorial_legacy_pages_restore_as_complete_reference(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) =
        cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
    vcx.run_until_parked();
    click(vcx, "sidebar-learn-tab");
    for page in 0..3 {
        let mut snapshot =
            vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        snapshot.tutorial_page = page;
        let bytes = snapshot.encode().unwrap();
        workspace.update(vcx, |workspace, cx| {
            workspace.sidebar_view = SidebarView::Sessions;
            workspace.apply_snapshot(WorkspaceSnapshot::decode(&bytes).unwrap(), cx);
            cx.notify();
        });
        vcx.run_until_parked();
        assert_all_lessons(vcx);
        let mut old: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        old.as_object_mut().unwrap().remove("tutorial_page");
        old.as_object_mut().unwrap().remove("sidebar_view");
        let decoded = WorkspaceSnapshot::decode(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(decoded.sidebar_view, SidebarView::Sessions);
        assert_eq!(decoded.tutorial_page, 0);
    }
}

#[gpui::test]
fn tutorial_list_scrolls_to_width_and_overview_actions(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("Practice", cx);
        workspace
    });
    vcx.run_until_parked();
    click(vcx, "sidebar-learn-tab");
    let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
    scroll_to_bottom(vcx);
    let content = vcx.debug_bounds("tutorial-content").unwrap();
    for id in ["tutorial-width-2", "tutorial-maximize", "tutorial-overview"] {
        let bounds = vcx.debug_bounds(id).unwrap();
        assert!(
            bounds.top() >= content.top() && bounds.bottom() <= content.bottom(),
            "{id} must be reachable by scrolling"
        );
    }
    click(vcx, "tutorial-width-2");
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.slots[0].width_fraction, 0.5);
        assert!(workspace.coach.trace("width_presets").practiced());
    });
    click(vcx, "tutorial-maximize");
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.slots[0].width_fraction, 1.0);
        assert!(workspace.coach.trace("maximize").practiced());
    });
    click(vcx, "tutorial-maximize");
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.slots[0].width_fraction, 0.5)
    });
    click(vcx, "tutorial-overview");
    workspace.read_with(vcx, |workspace, _| {
        assert!(workspace.overview);
        assert!(workspace.coach.trace("overview").practiced());
    });
    click(vcx, "tutorial-overview");
    workspace.read_with(vcx, |workspace, _| assert!(!workspace.overview));
    assert_eq!(vcx.debug_bounds("workspace-canvas").unwrap(), canvas);
}

#[gpui::test]
fn onboarding_geometry_acceptance(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("Previous prompt visibility", cx);
        workspace.slots[0].width_fraction = 1.0;
        workspace.slots[0].animated_width = AnimatedValue::new(1.0, transition::policy(Transition::PanelWidth).duration);
        workspace.slots[0].panel.update(cx, |panel, cx| {
            panel.items = vec![
                crate::panel::Item::User("Keep the previous prompt visible alongside the shortcut reference".into()),
                crate::panel::Item::Tool {
                    call_id: "geometry-todo".into(), name: "todo".into(), input: "{}".into(),
                    output: r#"[{"id":"check","content":"Verify tutorial layout without covering this text","status":"in_progress","priority":"high"}]"#.into(),
                    done: true, error: None,
                },
            ];
            cx.notify();
        });
        workspace
    });
    // Measure the settled layout without depending on entrance-animation timing.
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let handle = vcx.update(|window, _| window.window_handle());
    let mut cases = 0;
    let mut controls = 0;
    for (width, height) in [
        (640., 480.),
        (800., 600.),
        (917., 700.),
        (918., 700.),
        (1440., 1000.),
    ] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        for stage in 0..3 {
            workspace.update(vcx, |workspace, cx| {
                workspace.sidebar_view = SidebarView::Learn;
                workspace.show_sidebar = true;
                workspace.tutorial_page = stage;
                workspace.compact_sidebar_open = responsive::is_compact(width);
                cx.set_reduce_motion(true);
                cx.notify();
            });
            vcx.run_until_parked();
            // GPUI's cached paint reuses the visible scene, but this pinned
            // version does not copy debug_bounds for reused subtrees. Repaint
            // once to collect selectors before measuring unchanged geometry.
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            let guide = vcx.debug_bounds("tutorial-guides").unwrap();
            let content = vcx.debug_bounds("tutorial-content").unwrap();
            let panel = vcx.debug_bounds("panel-0").unwrap();
            let prompt = vcx.debug_bounds("pinned-latest-prompt")
                .or_else(|| vcx.debug_bounds("transcript-row-0"))
                .expect("prompt should be visible in the transcript or pinned");
            let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
            if responsive::is_compact(width) {
                assert!(vcx.debug_bounds("compact-sidebar-overlay").is_some());
            } else {
                assert_eq!(overlap_area(guide, panel), 0.0);
                assert_eq!(overlap_area(guide, prompt), 0.0);
                assert_eq!(overlap_area(guide, canvas), 0.0);
            }
            assert!(guide.right() <= px(SIDEBAR_WIDTH));
            assert!(
                canvas.top() < px(40.0),
                "no reserved tutorial strip at the top"
            );
            let mut regions = Vec::new();
            for id in LESSONS.iter().copied().chain(["tutorial-heading"]) {
                if let Some(bounds) = vcx.debug_bounds(id) {
                    assert!(
                        bounds.right() <= guide.right() && bounds.left() >= guide.left(),
                        "{id} must fit the Learn panel"
                    );
                    if !responsive::is_compact(width) {
                        assert_eq!(overlap_area(bounds, panel), 0.0, "{id} covers the session");
                    }
                    regions.push((id, bounds));
                }
            }
            assert_eq!(regions.len(), LESSONS.len() + 1, "every lesson is rendered");
            assert_all_lessons(vcx);
            for (i, (a_id, a)) in regions.iter().enumerate() {
                for (b_id, b) in &regions[i + 1..] {
                    assert_eq!(
                        overlap_area(*a, *b),
                        0.0,
                        "tutorial rows {a_id} and {b_id} overlap at {width}x{height}"
                    );
                }
            }
            assert!(content.size.height > px(0.0));
            assert!(content.bottom() <= px(height));
            scroll_to_bottom(vcx);
            let overview = vcx.debug_bounds("tutorial-overview").unwrap();
            assert!(
                overview.bottom() <= content.bottom() && overview.top() >= content.top(),
                "last lesson must be reachable at {width}x{height}"
            );
            controls += regions.len();
            cases += 1;
            workspace.update(vcx, |workspace, cx| {
                workspace.show_sidebar = false;
                cx.notify();
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("tutorial-guides").is_none());
        }
    }
    println!(
        "LEARN_GEOMETRY cases={cases} controls={controls} wide_panel_collisions=0 wide_prompt_collisions=0 control_collisions=0 top_reserved_pixels=0"
    );
}
