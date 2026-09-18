//! Manual titles use the native SDK operation, never a desktop-only override.
use super::*;

pub(super) struct Editor {
    // Capture identity, not the active index: navigation may change while editing.
    target: Entity<Panel>,
    input: Entity<PromptInput>,
    error: Option<String>,
}

fn validated_title(title: &str) -> Result<String, &'static str> {
    let title = title.trim();
    if title.is_empty() {
        Err("Enter a session title.")
    } else if title.chars().any(char::is_control) {
        Err("Use a single line for the session title.")
    } else {
        Ok(title.to_owned())
    }
}

impl Workspace {
    pub(super) fn rename_target(&self, cx: &App) -> Option<Entity<Panel>> {
        self.slots
            .get(self.active)
            .filter(|slot| {
                let panel = slot.panel.read(cx);
                slot.row == self.active_row
                    && !slot.closing
                    && panel.can_fork()
                    && (!panel.session_id.contains("://") || panel.session_id.starts_with("ssh://"))
            })
            .map(|slot| slot.panel.clone())
    }

    pub(super) fn rename_session(
        &mut self,
        _: &RenameSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if let Some(editor) = &self.rename_editor {
            let focus = editor.input.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            return;
        }
        let Some(target) = self.rename_target(cx) else {
            return;
        };
        let title = crate::panel::folder_session_title(
            &target.read(cx).session_id,
            target.read(cx).title.as_ref(),
        )
        .to_string();
        let weak = cx.weak_entity();
        let cancel = weak.clone();
        let input = cx.new(|cx| {
            let mut input = PromptInput::new(cx, "Session title", move |title, _, _, app| {
                let _ = weak.update(app, |this, cx| this.save_session_title(&title, cx));
            })
            .without_command_completion()
            .with_on_overlay_cancel(move |app| {
                let _ = cancel.update(app, |this, cx| this.close_rename_editor(cx));
                true
            });
            input.set_content(title, cx);
            let mut snapshot = input.snapshot();
            snapshot.selection_start = 0;
            snapshot.selection_end = snapshot.content.len();
            input.restore(snapshot, cx);
            input
        });
        self.focus_pending = false;
        let focus = input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        self.rename_editor = Some(Editor {
            target,
            input,
            error: None,
        });
        cx.notify();
    }

    fn save_session_title(&mut self, title: &str, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.rename_editor else {
            return;
        };
        let title = match validated_title(title) {
            Ok(title) => title,
            Err(error) => {
                editor.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if !self
            .slots
            .iter()
            .any(|slot| slot.panel == editor.target && !slot.closing)
        {
            editor.error = Some("This session has been closed.".into());
            cx.notify();
            return;
        }
        if editor.target.read(cx).is_busy() {
            editor.error =
                Some("Wait for this session to finish working, then save your title.".into());
            cx.notify();
            return;
        }
        self.bridge.send(Command::SessionOperation {
            session_id: editor.target.read(cx).session_id.clone(),
            operation: harness::SessionOperation::Rename(Some(title)),
        });
        // SessionRenamed is authoritative and updates both tabs and history.
        // Transport failures use the existing visible session error path.
        self.close_rename_editor(cx);
    }

    pub(super) fn close_rename_editor(&mut self, cx: &mut Context<Self>) {
        self.rename_editor = None;
        self.focus_pending = true;
        cx.notify();
    }

    pub(super) fn render_rename_editor(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let editor = self.rename_editor.as_ref().unwrap();
        div()
            .id("rename-session-overlay")
            .debug_selector(|| "rename-session-overlay".into())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_rename_editor(cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .id("rename-session-dialog")
                    .debug_selector(|| "rename-session-dialog".into())
                    .w(px(420.0))
                    .max_w_full()
                    .mx_4()
                    .p_4()
                    .rounded_lg()
                    .bg(Theme::global().PANEL_BG)
                    .border_1()
                    .border_color(Theme::global().PANEL_BORDER)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .capture_action(cx.listener(|this, _: &crate::input::Submit, _, cx| {
                        cx.stop_propagation();
                        if let Some(editor) = &this.rename_editor {
                            let title = editor.input.read(cx).content.to_string();
                            this.save_session_title(&title, cx);
                        }
                    }))
                    .child(div().text_size(px(16.0)).child("Rename session"))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child("Enter to save · Esc to cancel · F2 to rename"),
                    )
                    .child(editor.input.clone())
                    .when_some(editor.error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(Theme::global().ERROR)
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("rename-session-cancel")
                                    .debug_selector(|| "rename-session-cancel".into())
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|el| el.bg(Theme::global().TOOL_BG))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_rename_editor(cx)),
                                    )
                                    .child("Cancel"),
                            )
                            .child(
                                div()
                                    .id("rename-session-save")
                                    .debug_selector(|| "rename-session-save".into())
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(Theme::global().TOOL_BG)
                                    .text_color(Theme::global().ACCENT)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(editor) = &this.rename_editor {
                                            let title = editor.input.read(cx).content.to_string();
                                            this.save_session_title(&title, cx);
                                        }
                                    }))
                                    .child("Save"),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "workspace_rename_tests.rs"]
mod tests;
