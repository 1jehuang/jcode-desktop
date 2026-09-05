//! Quantitative acceptance check that can also run against the pre-dock UI.
//! Deliberately depends only on existing controls and panel selectors, not on
//! the dock or its sizing formulas, so before/after measurements are comparable.
use super::*;

fn overlap_area(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> f32 {
    let width = f32::from(a.right().min(b.right()) - a.left().max(b.left())).max(0.0);
    let height = f32::from(a.bottom().min(b.bottom()) - a.top().max(b.top())).max(0.0);
    width * height
}

#[gpui::test]
fn onboarding_geometry_acceptance(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("Previous prompt visibility", cx);
        workspace.slots[0].width_fraction = 1.0;
        workspace.slots[0].animated_width =
            AnimatedValue::new(1.0, transition::policy(Transition::PanelWidth).duration);
        workspace.slots[0].panel.update(cx, |panel, cx| {
            panel.items = vec![
                crate::panel::Item::User("Keep the previous prompt visible while onboarding teaches the navigation controls".into()),
                crate::panel::Item::Tool {
                    call_id: "geometry-todo".into(),
                    name: "todo".into(),
                    input: "{}".into(),
                    output: r#"[{"id":"check","content":"Verify tutorial layout without covering this text","status":"in_progress","priority":"high"}]"#.into(),
                    done: true,
                    error: None,
                },
            ];
            cx.notify();
        });
        workspace
    });
    let handle = vcx.update(|window, _| window.window_handle());
    let mut cases = 0;
    let mut controls_checked = 0;
    let mut panel_collisions = 0;
    let mut prompt_collisions = 0;
    let mut control_collisions = 0;
    let mut offscreen_controls = 0;
    let mut panel_overlap_pixels = 0.0;
    for (width, height) in [
        (640., 480.),
        (800., 600.),
        (917., 700.),
        (918., 700.),
        (1440., 1000.),
    ] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        for sidebar in [false, true] {
            for stage in 1..=3 {
                workspace.update(vcx, |workspace, cx| {
                    workspace.show_sidebar = sidebar;
                    workspace.showcase_mode = true;
                    workspace.showcase_cue = None;
                    workspace.coach = learning::Coach::new();
                    for skill in ONBOARDING_SKILLS.iter().take(match stage {
                        1 => 0,
                        2 => 4,
                        _ => 7,
                    }) {
                        workspace.coach.used_shortcut(skill, learning::now());
                    }
                    cx.notify();
                });
                vcx.run_until_parked();
                let panel = vcx.debug_bounds("panel-0").expect("session panel");
                let prompt = vcx
                    .debug_bounds("pinned-latest-prompt")
                    .expect("pinned previous prompt");
                let mut controls = Vec::new();
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
                ] {
                    if let Some(bounds) = vcx.debug_bounds(id) {
                        let overlap = overlap_area(bounds, panel);
                        panel_collisions += usize::from(overlap > 0.0);
                        panel_overlap_pixels += overlap;
                        prompt_collisions += usize::from(overlap_area(bounds, prompt) > 0.0);
                        offscreen_controls += usize::from(
                            bounds.left() < px(0.)
                                || bounds.top() < px(0.)
                                || bounds.right() > px(width)
                                || bounds.bottom() > px(height),
                        );
                        controls.push(bounds);
                    }
                }
                assert_eq!(
                    controls.len(),
                    match stage {
                        1 => 7,
                        2 => 6,
                        _ => 3,
                    }
                );
                for (i, bounds) in controls.iter().enumerate() {
                    for other in &controls[i + 1..] {
                        control_collisions += usize::from(overlap_area(*bounds, *other) > 0.0);
                    }
                }
                controls_checked += controls.len();
                cases += 1;
            }
        }
    }
    println!(
        "ONBOARDING_GEOMETRY cases={cases} controls={controls_checked} panel_collisions={panel_collisions} prompt_collisions={prompt_collisions} control_collisions={control_collisions} offscreen_controls={offscreen_controls} panel_overlap_pixels={panel_overlap_pixels:.0}"
    );
    assert_eq!(
        panel_collisions, 0,
        "tutorial content covers session panels"
    );
    assert_eq!(
        prompt_collisions, 0,
        "tutorial content covers the previous prompt"
    );
    assert_eq!(
        control_collisions, 0,
        "tutorial controls overlap each other"
    );
    assert_eq!(
        offscreen_controls, 0,
        "tutorial controls extend outside the window"
    );
}
