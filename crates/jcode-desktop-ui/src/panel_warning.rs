//! Transcript warnings: expected refusals the user should notice but that are
//! not failures, such as `/compact` with nothing to summarize. They render as
//! a compact pill in the warning tone, without the error card's recovery
//! actions or error sound.
use super::*;

impl Panel {
    /// Append a warning row. Unlike an error it plays no sound and does not
    /// mark the session as failed in the minimap.
    pub(crate) fn push_warning(&mut self, message: String, cx: &mut Context<Self>) {
        self.items.push(Item::Warning(message));
        cx.notify();
    }

    pub(super) fn render_warning_notice(
        &self,
        index: usize,
        message: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        div()
            .debug_selector(|| "warning-notice".into())
            .flex()
            .items_center()
            .map(|el| {
                if crate::config::display().compact_notifications {
                    el.gap_1().px_2().py_1()
                } else {
                    el.gap_2().px_3().py_2()
                }
            })
            .rounded_full()
            .bg(theme.HEADER_BG)
            .text_size(px(11.5))
            .text_color(theme.WARN)
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child("⚠"))
            .child(div().min_w_0().flex_1().child(text_selection::plain(
                self.transcript_selection.clone(),
                format!("{index}-warning"),
                message.to_owned(),
                window,
                cx,
            )))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn command_warning_renders_as_a_notice_not_an_error(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("warn", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            panel.history_loaded = true;
            panel.push_warning("Nothing to compact yet.".into(), cx);
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert_ne!(panel.minimap_state(), MinimapSessionState::Error);
            assert!(matches!(
                panel.items.last(),
                Some(Item::Warning(text)) if text == "Nothing to compact yet."
            ));
            assert!(
                !panel
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::Error(_)))
            );
        });
        assert!(vcx.debug_bounds("warning-notice").is_some());
        assert!(vcx.debug_bounds("recovery-error").is_none());
    }
}
