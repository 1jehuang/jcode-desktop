use super::*;

gpui::actions!(panel_shortcuts, [JumpToLatest]);

pub(crate) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([gpui::KeyBinding::new(
        "shift-space",
        JumpToLatest,
        Some("ChatPanel"),
    )]);
}

impl Panel {
    pub(super) fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.cancel_transcript_momentum();
        self.release_startup_preview();
        self.stick_to_bottom = true;
        self.transcript_list.scroll_to_end();
        cx.notify();
    }
}

/// Outlined keycaps for the Shift+Space jump binding, drawn as icons so the
/// latest pill shows the shortcut instead of a text label.
pub(super) fn jump_to_latest_keycaps(color: gpui::Hsla) -> gpui::AnyElement {
    macro_rules! icon {
        ($paths:literal) => {
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
                $paths,
                "</svg>"
            )
            .as_bytes()
        };
    }
    const SHIFT: &[u8] = icon!(r#"<path d="m12 3 9 9h-5v9H8v-9H3z"/>"#);
    const SPACE: &[u8] = icon!(r#"<path d="M3 9v6h18V9"/>"#);
    let keycap = |selector: &'static str, data: &'static [u8], width: f32| {
        div()
            .debug_selector(move || selector.into())
            .flex_none()
            .h(px(15.))
            .w(px(width))
            .rounded(px(3.5))
            .border_1()
            .border_color(color.opacity(0.7))
            .flex()
            .items_center()
            .justify_center()
            .child(gpui::svg().data(data).text_color(color).size(px(9.)))
    };
    div()
        .debug_selector(|| "jump-to-latest-shortcut".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(3.))
        .child(keycap("jump-to-latest-shift", SHIFT, 15.))
        .child(keycap("jump-to-latest-space", SPACE, 24.))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn shift_space_jumps_to_latest_from_composer(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        cx.update(crate::input::bind_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::User("A long prompt line\n".repeat(1000))];
            panel.stick_to_bottom = false;
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| {
            panel.transcript_list.scroll_to(gpui::ListOffset::default());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("jump-to-latest").is_some());
        assert!(vcx.debug_bounds("jump-to-latest-shift").is_some());
        assert!(vcx.debug_bounds("jump-to-latest-space").is_some());
        vcx.update(|window, cx| {
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();
        vcx.simulate_input("keep this draft");
        vcx.simulate_keystrokes("shift-space");
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
        assert!(vcx.debug_bounds("jump-to-latest").is_none());
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).content.to_string()),
            "keep this draft"
        );
    }
}
