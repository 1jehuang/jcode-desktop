//! Count real Panel renders, not a surrogate cache implementation.
use super::*;
use std::cell::RefCell;

thread_local! {
    static RENDERS: RefCell<std::collections::HashMap<gpui::EntityId, usize>> = RefCell::new(Default::default());
}

pub(crate) fn record_render(id: gpui::EntityId) {
    RENDERS.with_borrow_mut(|renders| *renders.entry(id).or_default() += 1);
}

fn count(panel: &gpui::Entity<Panel>) -> usize {
    RENDERS.with_borrow(|renders| renders.get(&panel.entity_id()).copied().unwrap_or(0))
}

#[gpui::test]
fn panel_cache_reuses_transcript_on_workspace_notifications(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("cached-panel", cx);
        workspace
    });
    let panel = workspace.read_with(vcx, |w, _| w.test_panel(0).unwrap());
    vcx.run_until_parked();
    // Settle camera and focus invalidations before measuring chrome-only work.
    for _ in 0..3 {
        workspace.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    let before = count(&panel);
    assert!(before > 0);
    for _ in 0..20 {
        workspace.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    assert_eq!(
        count(&panel),
        before,
        "chrome redraws must reuse the real transcript subtree"
    );
    panel.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        count(&panel) > before,
        "panel notifications must invalidate the cached subtree"
    );
}

#[gpui::test]
fn panel_cache_preserves_streaming_and_resize(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("cache-stream", cx);
        workspace
    });
    let panel = workspace.read_with(vcx, |w, _| w.test_panel(0).unwrap());
    vcx.run_until_parked();
    let before = count(&panel);
    panel.update(vcx, |panel, cx| {
        panel.apply(
            &jcode_sdk::ApiEvent::TextDelta {
                session_id: "cache-stream".into(),
                text: "A visible streaming response".into(),
            },
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(
        count(&panel) > before,
        "streaming must invalidate cached content"
    );
    let before = count(&panel);
    vcx.simulate_resize(gpui::size(px(900.), px(700.)));
    vcx.run_until_parked();
    assert!(
        count(&panel) > before,
        "resizing must relayout cached content"
    );
}

#[gpui::test]
fn panel_cache_descendant_composer_notifications_remain_live(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("cache-input", cx);
        workspace
    });
    let panel = workspace.read_with(vcx, |w, _| w.test_panel(0).unwrap());
    vcx.run_until_parked();
    vcx.update(|window, cx| window.focus(&panel.read(cx).input_focus_handle(cx), cx));
    vcx.run_until_parked();
    let before = count(&panel);
    vcx.simulate_keystrokes("cache");
    vcx.simulate_keystrokes("space");
    vcx.simulate_keystrokes("invalidation");
    vcx.run_until_parked();
    assert!(
        count(&panel) > before,
        "a descendant composer change must invalidate its cached parent"
    );
    assert_eq!(
        panel.read_with(vcx, |panel, cx| panel.snapshot(cx).draft.content),
        "cache invalidation"
    );
}
