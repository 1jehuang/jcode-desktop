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
