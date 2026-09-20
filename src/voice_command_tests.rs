//! Native host-to-UI action dispatch without any microphone or live windows.
use super::*;

gpui::actions!(host_voice_test, [VoiceRequest]);

struct VoiceRoot {
    requests: usize,
    focus: gpui::FocusHandle,
}

impl Render for VoiceRoot {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().track_focus(&self.focus).capture_action(cx.listener(
            |this, _: &VoiceRequest, _, cx| {
                this.requests += 1;
                cx.stop_propagation();
            },
        ))
    }
}

#[gpui::test]
fn named_voice_dispatch_targets_current_window_and_current_root(cx: &mut gpui::TestAppContext) {
    let (first, first_cx) = cx.add_window_view(|window, cx| {
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        VoiceRoot { requests: 0, focus }
    });
    let handle = first_cx.window_handle();
    first_cx.run_until_parked();
    first_cx.cx.update(|cx| {
        dispatch_ui_action(handle, "host_voice_test::VoiceRequest", cx).unwrap();
    });
    first_cx.run_until_parked();
    first.read_with(first_cx, |root, _| assert_eq!(root.requests, 1));
    let replacement = first_cx.update(|window, cx| {
        window.replace_root(cx, |window, cx| {
            let focus = cx.focus_handle().tab_stop(true);
            window.focus(&focus, cx);
            VoiceRoot { requests: 0, focus }
        })
    });
    first_cx.run_until_parked();
    first_cx.update(|window, _| window.blur());
    first_cx.cx.update(|cx| {
        dispatch_ui_action(handle, "host_voice_test::VoiceRequest", cx).unwrap();
        assert!(dispatch_ui_action(handle, "missing_ui::VoiceRequest", cx).is_err());
    });
    first_cx.run_until_parked();
    first.read_with(first_cx, |root, _| assert_eq!(root.requests, 1));
    replacement.read_with(first_cx, |root, _| assert_eq!(root.requests, 1));
}
