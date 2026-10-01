use super::*;

gpui::actions!(
    panel_shortcuts,
    [JumpToLatest, JumpToLatestIfEmpty, BackgroundRunningTool]
);

/// Alt+B/Ctrl+B move the running tool to the background, matching the TUI.
/// Both chords are also composer word-motion keys, so the handler propagates
/// when no tool is running. Registered from `input::bind_keys` right after the
/// composer's own bindings so it outranks them at the same context depth.
pub(crate) fn background_tool_bindings() -> [gpui::KeyBinding; 4] {
    [
        gpui::KeyBinding::new("alt-b", BackgroundRunningTool, Some("ChatPanel")),
        gpui::KeyBinding::new("ctrl-b", BackgroundRunningTool, Some("ChatPanel")),
        gpui::KeyBinding::new(
            "alt-b",
            BackgroundRunningTool,
            Some("ChatPanel > PromptInput"),
        ),
        gpui::KeyBinding::new(
            "ctrl-b",
            BackgroundRunningTool,
            Some("ChatPanel > PromptInput"),
        ),
    ]
}

pub(crate) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        gpui::KeyBinding::new("shift-space", JumpToLatest, Some("ChatPanel")),
        // Plain space jumps only while the composer is empty. With a draft the
        // handler propagates so the space is typed normally.
        gpui::KeyBinding::new(
            "space",
            JumpToLatestIfEmpty,
            Some("ChatPanel > PromptInput"),
        ),
    ]);
}

impl Panel {
    /// Whether a tool call is in flight and could be moved to the background.
    pub(super) fn has_running_tool(&self) -> bool {
        self.preview_state.is_none()
            && !self.is_pending_session()
            && self
                .items
                .iter()
                .rev()
                .any(|item| matches!(item, Item::Tool { done: false, error: None, .. }))
    }

    pub(super) fn background_running_tool_if_any(&mut self, cx: &mut Context<Self>) {
        if self.has_running_tool() {
            self.background_running_tool(cx);
        } else {
            cx.propagate();
        }
    }

    pub(super) fn background_running_tool(&mut self, cx: &mut Context<Self>) {
        self.bridge.send(Command::BackgroundTool {
            session_id: self.session_id.clone(),
        });
        cx.notify();
    }

    pub(super) fn jump_to_latest_if_empty(&mut self, cx: &mut Context<Self>) {
        if self.input.read(cx).content.is_empty() {
            self.jump_to_latest(cx);
        } else {
            cx.propagate();
        }
    }

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

    fn scrolled_up_panel(
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::Entity<Panel>, &mut gpui::VisualTestContext) {
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
            *panel.items = vec![Item::User("A long prompt line\n".repeat(1000))];
            panel.expanded_prompts.insert((0, false));
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
        (panel, vcx)
    }

    #[gpui::test]
    fn space_jumps_to_latest_only_when_composer_is_empty(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = scrolled_up_panel(cx);
        vcx.simulate_input("draft");
        vcx.simulate_keystrokes("space");
        vcx.run_until_parked();
        assert!(!panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).content.to_string()),
            "draft "
        );
        panel.update(vcx, |panel, cx| {
            panel
                .input
                .update(cx, |input, cx| input.set_content(String::new(), cx));
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("space");
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.stick_to_bottom));
        assert!(vcx.debug_bounds("jump-to-latest").is_none());
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).content.to_string()),
            ""
        );
    }

    #[gpui::test]
    fn shift_space_jumps_to_latest_from_composer(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = scrolled_up_panel(cx);
        assert!(vcx.debug_bounds("jump-to-latest-shift").is_some());
        assert!(vcx.debug_bounds("jump-to-latest-space").is_some());
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

    #[gpui::test]
    fn alt_b_and_button_background_a_running_tool_but_move_words_when_idle(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::bind_workspace_keys);
        cx.update(crate::input::bind_keys);
        let (bridge, commands) = crate::harness::spawn_recording();
        let (panel, vcx) =
            cx.add_window_view(|_, cx| Panel::new("bg-test".into(), None, None, bridge, cx));
        vcx.update(|window, cx| {
            Panel::connect_input(&panel, cx);
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            handle.focus(window, cx);
        });
        let drain = |commands: &std::sync::mpsc::Receiver<crate::harness::Command>| {
            commands
                .try_iter()
                .filter(|c| matches!(c, Command::BackgroundTool { .. }))
                .count()
        };

        // Idle: Alt+B stays a composer word motion.
        vcx.simulate_input("hello world");
        vcx.simulate_keystrokes("alt-b");
        vcx.run_until_parked();
        assert_eq!(drain(&commands), 0);
        assert!(vcx.debug_bounds("tool-background").is_none());

        panel.update(vcx, |panel, cx| {
            panel.items.push(Item::Tool {
                call_id: "call-1".into(),
                name: "bash".into(),
                input: r#"{"command":"sleep 100"}"#.into(),
                output: String::new(),
                done: false,
                error: None,
            });
            cx.notify();
        });
        vcx.run_until_parked();

        for chord in ["alt-b", "ctrl-b"] {
            vcx.simulate_keystrokes(chord);
            vcx.run_until_parked();
            assert_eq!(drain(&commands), 1, "{chord}");
        }
        let pill = vcx
            .debug_bounds("tool-background")
            .expect("running tool shows a Background pill");
        vcx.simulate_click(pill.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        assert_eq!(drain(&commands), 1);
    }
}
