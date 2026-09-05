//! Keep this measurement independent of tutorial internals so it also runs on
//! pre-Learn revisions for like-for-like rendered layout comparisons.
use super::*;

#[gpui::test]
fn tutorial_minimalism_measurement(cx: &mut gpui::TestAppContext) {
    let (_workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("Previous prompt", cx);
        workspace
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for (width, height) in [(640., 480.), (1440., 1000.)] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        vcx.run_until_parked();
        let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
        let default_guides = vcx.debug_bounds("tutorial-guides");
        let tab = vcx.debug_bounds("sidebar-learn-tab");
        let controls = [
            "tutorial-nav-left",
            "tutorial-nav-down",
            "tutorial-nav-up",
            "tutorial-nav-right",
            "tutorial-new",
            "tutorial-close",
        ]
        .iter()
        .filter(|id| vcx.debug_bounds(**id).is_some())
        .count();
        println!(
            "MINIMAL_UI width={width} height={height} canvas_top={} canvas_height={} default_controls={controls} default_guide_height={} learn_tab={}",
            f32::from(canvas.top()),
            f32::from(canvas.size.height),
            default_guides
                .map(|b| f32::from(b.size.height))
                .unwrap_or(0.0),
            tab.is_some(),
        );
        if let Some(tab) = tab {
            vcx.simulate_click(tab.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            let guide = vcx.debug_bounds("tutorial-guides").unwrap();
            assert_eq!(vcx.debug_bounds("workspace-canvas").unwrap(), canvas);
            assert!(guide.right() <= canvas.left());
            println!(
                "MINIMAL_UI_OPEN width={width} canvas_unchanged=true tutorial_outside_canvas=true"
            );
            let chat = vcx.debug_bounds("sidebar-sessions-tab").unwrap();
            vcx.simulate_click(chat.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
        }
    }
}
