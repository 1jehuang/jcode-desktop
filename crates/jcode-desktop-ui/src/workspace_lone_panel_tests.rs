//! Width allocation follows live columns, not surfaces retained for closing fades.
use super::*;

#[gpui::test]
fn lone_default_panel_fills_canvas_after_its_neighbor_closes(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.push_test_panel("left", cx);
        w.push_test_panel("right", cx);
        w.active = 1;
        w
    });
    vcx.run_until_parked();
    workspace.update_in(vcx, |w, window, cx| {
        w.close_panel(&ClosePanel, window, cx);
        assert_eq!(w.slots[0].width_fraction, 1.0);
        assert_eq!(w.active, 0);
        assert!(w.slots[1].closing);
        // Settle the same animations as the live host without wall-clock sleeps.
        w.remove_finished_closing_panels(Instant::now() + Duration::from_secs(60), window, cx);
        w.slots[0].animated_width = AnimatedValue::new(1.0, Duration::ZERO);
        cx.notify();
    });
    vcx.run_until_parked();
    let panel = vcx.debug_bounds("panel-0").unwrap();
    let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
    assert!(
        panel.size.width > canvas.size.width * 0.9,
        "lone panel must fill the canvas: {panel:?} in {canvas:?}"
    );
}

#[gpui::test]
fn reopening_during_last_panel_fade_still_gets_full_width(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.push_test_panel("closing", cx);
        w.slots[0].close_progress = AnimatedValue::new(1.0, Duration::from_secs(60));
        w
    });
    workspace.update_in(vcx, |w, window, cx| {
        w.close_panel(&ClosePanel, window, cx);
        w.open_local_draft(None, cx);
        assert_eq!(w.slots.len(), 2, "old surface stays mounted for its fade");
        let live = w
            .row_indices(0)
            .filter(|&i| !w.slots[i].closing)
            .collect::<Vec<_>>();
        assert_eq!(live.len(), 1);
        assert_eq!(w.slots[live[0]].width_fraction, 1.0);
        w.open_local_draft(None, cx);
        let live = w
            .row_indices(0)
            .filter(|&i| !w.slots[i].closing)
            .collect::<Vec<_>>();
        assert_eq!(live.len(), 2);
        for i in live {
            assert_eq!(
                w.slots[i].width_fraction, DEFAULT_WIDTH,
                "only live columns share the canvas"
            );
        }
    });
}

#[gpui::test]
fn closing_neighbor_preserves_custom_lone_panel_width(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) =
        cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
    for width in [0.25, 0.75, 1.0] {
        workspace.update_in(vcx, |w, window, cx| {
            w.slots.clear();
            w.push_test_panel("custom", cx);
            w.push_test_panel("neighbor", cx);
            w.slots[0].width_fraction = width;
            w.active = 1;
            w.close_panel(&ClosePanel, window, cx);
            assert_eq!(w.slots[0].width_fraction, width);
        });
    }
}
