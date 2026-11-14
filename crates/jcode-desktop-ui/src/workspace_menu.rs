//! Native-menu entry points reuse the workspace's existing settings and sessions.
use super::*;
use crate::ui_actions::OpenSettings;

impl Workspace {
    pub(super) fn open_settings(
        &mut self,
        _: &OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_view = SidebarView::Settings;
        if !self.single_panel {
            self.show_sidebar = true;
            self.compact_sidebar_open =
                responsive::is_compact(f32::from(window.viewport_size().width));
        }
        // Menu actions must work even when the composer, rather than the
        // workspace, owned focus. Nothing replaces the current chat entity.
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    pub(super) fn close_single_panel_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_view = SidebarView::Sessions;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(super) fn render_single_panel_settings(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .debug_selector(|| "standalone-settings".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::global().PANEL_BG)
            .text_color(Theme::global().TEXT)
            .font_family(Theme::global().FONT_UI)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::new_panel))
            .on_action(cx.listener(|this, _: &ClosePanel, window, cx| {
                this.close_single_panel_settings(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_single_panel_settings(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div().p_3().child(
                    div()
                        .id("standalone-settings-back")
                        .debug_selector(|| "standalone-settings-back".into())
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .bg(Theme::global().TOOL_BG)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_single_panel_settings(window, cx);
                        }))
                        .child("Back to chat"),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.render_settings(cx)))
            .child(
                div()
                    .relative()
                    .h(px(28.0))
                    .child(self.render_version_footer(360.0, cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn menu_settings_reveals_hidden_sidebar_without_replacing_chat(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.show_sidebar = false;
            workspace.open_startup_draft(cx);
            workspace
        });
        vcx.run_until_parked();
        let original = workspace.read_with(vcx, |w, _| w.slots[0].panel.entity_id());
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| w.focus_active(window, cx));
            window.dispatch_action(Box::new(OpenSettings), cx);
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            assert!(w.show_sidebar);
            assert_eq!(w.sidebar_view, SidebarView::Settings);
            assert_eq!(w.slots[0].panel.entity_id(), original);
        });
        assert!(vcx.debug_bounds("workspace-settings").is_some());
    }

    #[gpui::test]
    fn standalone_new_session_opens_window_without_mutating_current_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        single_panel::NEW_SESSION_WINDOWS.with(|count| count.set(0));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.single_panel = true;
            workspace.open_startup_draft(cx);
            workspace
        });
        vcx.run_until_parked();
        let original = workspace.read_with(vcx, |w, _| w.slots[0].panel.entity_id());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| w.focus_active(window, cx));
            window.dispatch_action(Box::new(NewPanel), cx);
        });
        vcx.run_until_parked();
        single_panel::NEW_SESSION_WINDOWS.with(|count| assert_eq!(count.get(), 1));
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.slots[0].panel.entity_id(), original);
        });
    }

    #[gpui::test]
    fn standalone_settings_returns_to_same_chat(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.single_panel = true;
            workspace.open_startup_draft(cx);
            workspace
        });
        vcx.run_until_parked();
        let original = workspace.read_with(vcx, |w, _| w.slots[0].panel.entity_id());
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| w.focus_active(window, cx));
            window.dispatch_action(Box::new(OpenSettings), cx);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("standalone-settings").is_some());
        assert!(vcx.debug_bounds("settings-minimap").is_none());
        assert!(vcx.debug_bounds("settings-folder-tabs").is_none());
        assert!(vcx.debug_bounds("settings-machines").is_none());
        let back = vcx.debug_bounds("standalone-settings-back").unwrap();
        vcx.simulate_click(back.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("standalone-settings").is_none());
        assert!(vcx.debug_bounds("single-panel-root").is_some());
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.slots[0].panel.entity_id(), original);
        });
    }
}
