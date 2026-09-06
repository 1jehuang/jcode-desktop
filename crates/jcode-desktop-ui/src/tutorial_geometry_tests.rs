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

#[gpui::test]
fn tutorial_tab_is_opt_in_and_stages_do_not_resize_the_canvas(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) =
        cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
    vcx.run_until_parked();
    let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
    assert!(
        vcx.debug_bounds("tutorial-guides").is_none(),
        "no onboarding banner by default"
    );
    let tab = vcx.debug_bounds("sidebar-learn-tab").unwrap();
    assert!(
        tab.right() <= px(SIDEBAR_WIDTH),
        "Learn is visible without scrolling the tabs"
    );
    click(vcx, "sidebar-learn-tab");
    assert!(vcx.debug_bounds("tutorial-new").is_some());
    for stage in 0..3 {
        workspace.read_with(vcx, |workspace, _| {
            assert_eq!(workspace.tutorial_page, stage)
        });
        assert_eq!(
            vcx.debug_bounds("workspace-canvas").unwrap(),
            canvas,
            "stages must not reserve canvas space"
        );
        if stage == 1 {
            assert!(vcx.debug_bounds("tutorial-width-presets").is_some());
        }
        if stage == 2 {
            assert!(vcx.debug_bounds("tutorial-overview").is_some());
        }
        click(vcx, "tutorial-next");
    }
    assert!(
        vcx.debug_bounds("tutorial-guides").is_none(),
        "Done returns to Chat"
    );
    assert_eq!(vcx.debug_bounds("workspace-canvas").unwrap(), canvas);
    click(vcx, "sidebar-learn-tab");
    click(vcx, "tutorial-back");
    workspace.read_with(vcx, |workspace, _| assert_eq!(workspace.tutorial_page, 1));
    click(vcx, "sidebar-sessions-tab");
    assert!(vcx.debug_bounds("tutorial-guides").is_none());
    click(vcx, "sidebar-learn-tab");
    workspace.read_with(vcx, |workspace, _| assert_eq!(workspace.tutorial_page, 1));
}

#[gpui::test]
fn tutorial_page_and_selected_tab_survive_snapshot_reload(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) =
        cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
    vcx.run_until_parked();
    click(vcx, "sidebar-learn-tab");
    click(vcx, "tutorial-next");
    let snapshot = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
    let bytes = snapshot.encode().unwrap();
    workspace.update(vcx, |workspace, cx| {
        workspace.sidebar_view = SidebarView::Sessions;
        workspace.tutorial_page = 0;
        workspace.apply_snapshot(WorkspaceSnapshot::decode(&bytes).unwrap(), cx);
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("tutorial-width-presets").is_some());
    // The additive state remains compatible with snapshots from before Learn.
    let mut old: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    old.as_object_mut().unwrap().remove("tutorial_page");
    old.as_object_mut().unwrap().remove("sidebar_view");
    let decoded = WorkspaceSnapshot::decode(&serde_json::to_vec(&old).unwrap()).unwrap();
    assert_eq!(decoded.sidebar_view, SidebarView::Sessions);
    assert_eq!(decoded.tutorial_page, 0);
}

#[gpui::test]
fn tutorial_later_stages_dispatch_width_and_overview_actions(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("Practice", cx);
        workspace
    });
    vcx.run_until_parked();
    click(vcx, "sidebar-learn-tab");
    click(vcx, "tutorial-next");
    click(vcx, "tutorial-width-2");
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.slots[0].width_fraction, 0.5);
        assert!(workspace.coach.trace("width_presets").practiced());
    });
    click(vcx, "tutorial-next");
    click(vcx, "tutorial-overview");
    workspace.read_with(vcx, |workspace, _| {
        assert!(workspace.overview);
        assert!(workspace.coach.trace("overview").practiced());
    });
    click(vcx, "tutorial-overview");
    workspace.read_with(vcx, |workspace, _| assert!(!workspace.overview));
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
                crate::panel::Item::User("Keep the previous prompt visible alongside the staged tutorial".into()),
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
                cx.notify();
            });
            vcx.run_until_parked();
            let guide = vcx.debug_bounds("tutorial-guides").unwrap();
            let panel = vcx.debug_bounds("panel-0").unwrap();
            let prompt = vcx.debug_bounds("pinned-latest-prompt")
                .or_else(|| vcx.debug_bounds("transcript-row-0"))
                .expect("prompt should be visible in the transcript or pinned");
            let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
            assert_eq!(overlap_area(guide, panel), 0.0);
            assert_eq!(overlap_area(guide, prompt), 0.0);
            assert_eq!(overlap_area(guide, canvas), 0.0);
            assert!(guide.right() <= px(SIDEBAR_WIDTH));
            assert!(
                canvas.top() < px(40.0),
                "no reserved tutorial strip at the top"
            );
            let mut regions = Vec::new();
            for id in [
                "tutorial-stage",
                "tutorial-nav-left",
                "tutorial-nav-down",
                "tutorial-nav-up",
                "tutorial-nav-right",
                "tutorial-new",
                "tutorial-close",
                "tutorial-width-presets",
                "tutorial-resize",
                "tutorial-overview",
                "tutorial-next",
            ] {
                if let Some(bounds) = vcx.debug_bounds(id) {
                    assert!(
                        bounds.right() <= guide.right() && bounds.left() >= guide.left(),
                        "{id} must fit the Learn panel"
                    );
                    assert_eq!(overlap_area(bounds, panel), 0.0, "{id} covers the session");
                    regions.push(bounds);
                }
            }
            assert_eq!(
                regions.len(),
                [8, 7, 4][stage],
                "every stage control is rendered"
            );
            for (i, a) in regions.iter().enumerate() {
                for b in &regions[i + 1..] {
                    assert_eq!(overlap_area(*a, *b), 0.0, "tutorial rows must not overlap");
                }
            }
            let next = vcx.debug_bounds("tutorial-next").unwrap();
            assert!(
                next.bottom() <= px(height),
                "stage navigation remains visible"
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
        "LEARN_GEOMETRY cases={cases} controls={controls} panel_collisions=0 prompt_collisions=0 control_collisions=0 top_reserved_pixels=0"
    );
}
