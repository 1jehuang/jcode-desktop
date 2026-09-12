//! App-wide opt-in sound controls. The setting is persisted, not a window snapshot.
use super::*;
use crate::sounds::{self, Cue};

impl Workspace {
    pub(super) fn render_sound_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let enabled = sounds::enabled(cx);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .mt_2()
            .child(div().text_color(Theme::global().TEXT_DIM).child("Sounds · all windows"))
            .child(
                div()
                    .id("settings-sounds")
                    .debug_selector(|| "settings-sounds".into())
                    .px_2()
                    .py_2()
                    .rounded_sm()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .cursor_pointer()
                    .bg(Theme::global().PANEL_BG)
                    .hover(|el| el.bg(Theme::global().TOOL_BG))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                        let enabled = !sounds::enabled(cx);
                        if let Err(error) = crate::config::persist_sounds_enabled(enabled) {
                            this.status = format!("Could not save sound setting: {error}");
                            cx.notify();
                            return;
                        }
                        sounds::set_enabled(enabled, cx);
                        if enabled {
                            sounds::play(Cue::Complete, cx);
                        }
                        cx.notify();
                    }))
                    .child("Sound effects")
                    .child(div().text_color(if enabled { Theme::global().ACCENT } else { Theme::global().TEXT_DIM }).child(if enabled { "On" } else { "Off" })),
            )
            .child(div().text_color(Theme::global().TEXT_DIM).child("Soft cues for messages, attention, errors, finished tasks, and opening or closing panels. Off by default."))
            .when(enabled, |el| el.child(
                div()
                    .id("settings-sounds-preview")
                    .debug_selector(|| "settings-sounds-preview".into())
                    .px_2()
                    .py_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(Theme::global().PANEL_BG)
                    .hover(|el| el.bg(Theme::global().TOOL_BG))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, cx| {
                        sounds::play(Cue::Complete, cx);
                    }))
                    .child("Play preview"),
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn settings_sounds_toggle_is_global_and_preview_is_only_available_when_enabled(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            sounds::set_enabled(false, cx);
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.sidebar_view = SidebarView::Settings;
            workspace
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("settings-sounds-preview").is_none());
        let button = vcx.debug_bounds("settings-sounds").unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(workspace.read_with(vcx, |_, cx| sounds::enabled(cx)));
        let button = vcx.debug_bounds("settings-sounds-preview").unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.update(vcx, |_, cx| {
            assert_eq!(sounds::drain_requested(cx), [Cue::Complete, Cue::Complete]);
        });
        let button = vcx.debug_bounds("settings-sounds").unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(!workspace.read_with(vcx, |_, cx| sounds::enabled(cx)));
        assert!(vcx.debug_bounds("settings-sounds-preview").is_none());
    }
}
