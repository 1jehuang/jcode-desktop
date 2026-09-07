//! Quiet, consistently styled transient feedback. Only explicit controls dismiss
//! a tip, so clicking its text never unexpectedly changes the workspace below.
use super::*;

fn key_label(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "super" => if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Super"
        }
        .into(),
        "shift" => "Shift".into(),
        "ctrl" | "control" => "Ctrl".into(),
        "alt" => "Alt".into(),
        "enter" => "Enter".into(),
        "tab" => "Tab".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "escape" | "esc" => "Esc".into(),
        _ => key.to_uppercase(),
    }
}

fn shortcut_keys(shortcut: &str) -> gpui::AnyElement {
    let theme = Theme::global();
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(4.0))
        .children(shortcut.replace(" + ", "-").split_whitespace().map(|part| {
            if matches!(part, "/" | ".." | "→" | "then" | "or") {
                div()
                    .px_1()
                    .text_size(px(11.0))
                    .text_color(theme.TEXT_DIM)
                    .child(if part == "/" { "or" } else { part }.to_owned())
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .children(
                        part.split(['-', '+'])
                            .filter(|key| !key.is_empty())
                            .map(|key| {
                                div()
                                    .min_w(px(24.0))
                                    .h(px(24.0))
                                    .px(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(5.0))
                                    .bg(theme.HEADER_BG)
                                    .border_1()
                                    .border_b_2()
                                    .border_color(theme.PANEL_BORDER)
                                    .font_family(theme.FONT_MONO)
                                    .text_size(px(11.0))
                                    .text_color(theme.TEXT)
                                    .child(key_label(key))
                                    .into_any_element()
                            }),
                    )
                    .into_any_element()
            }
        }))
        .into_any_element()
}

impl Workspace {
    pub(super) fn render_coach_toast(
        &self,
        hint: &learning::Hint,
        progress: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        div()
            .absolute()
            .top(px(MINIMAP_TOP
                + if self.show_minimap {
                    MINIMAP_HEIGHT + COACH_TOAST_GAP
                } else {
                    0.0
                }))
            .right(px(MINIMAP_RIGHT))
            .w(px(COACH_TOAST_WIDTH))
            .max_w_full()
            .min_w_0()
            .opacity(progress)
            .child(
                div()
                    .id("coach-toast")
                    .debug_selector(|| "coach-toast".into())
                    .occlude()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .bg(theme.PANEL_BG)
                    .border_1()
                    .border_color(theme.PANEL_BORDER)
                    .rounded(px(12.0))
                    .shadow_md()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pl(px(16.0))
                            .pr(px(8.0))
                            .pt(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .child(div().size(px(5.0)).rounded_full().bg(theme.ACCENT))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(theme.TEXT_DIM)
                                            .child("Shortcut tip"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("coach-dismiss")
                                    .debug_selector(|| "coach-dismiss".into())
                                    .size(px(28.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .text_size(px(18.0))
                                    .text_color(theme.TEXT_DIM)
                                    .hover(|s| s.bg(theme.HEADER_BG).text_color(theme.TEXT))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.dismiss_coach_hint(cx);
                                        }),
                                    )
                                    .child("×"),
                            ),
                    )
                    .child(
                        div()
                            .px(px(16.0))
                            .pt(px(4.0))
                            .pb(px(14.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .debug_selector(|| "coach-title".into())
                                    .text_size(px(14.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.TEXT)
                                    .child(hint.label),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme.TEXT_DIM)
                                    .child(hint.because.clone()),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "coach-keys".into())
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_t_1()
                            .border_color(theme.PANEL_BORDER)
                            .bg(theme.HEADER_BG.opacity(0.4))
                            .child(shortcut_keys(hint.keys)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_showcase_cue(&self, cue: &ShowcaseCue) -> gpui::AnyElement {
        let theme = Theme::global();
        div()
            .id("showcase-shortcut")
            .debug_selector(|| "showcase-shortcut".into())
            .absolute()
            .left(px(16.0))
            .right(px(16.0))
            .bottom(px(56.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .id("showcase-card")
                    .debug_selector(|| "showcase-card".into())
                    .occlude()
                    .max_w_full()
                    .px(px(14.0))
                    .py(px(10.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(px(12.0))
                    .rounded(px(10.0))
                    .bg(theme.PANEL_BG)
                    .border_1()
                    .border_color(theme.PANEL_BORDER)
                    .shadow_md()
                    .child(
                        div()
                            .id("showcase-action")
                            .debug_selector(|| "showcase-action".into())
                            .text_size(px(12.0))
                            .text_color(theme.TEXT)
                            .child(cue.action),
                    )
                    .child(
                        div()
                            .id("showcase-key")
                            .debug_selector(|| "showcase-key".into())
                            .child(shortcut_keys(&cue.shortcut)),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycaps_use_readable_labels() {
        assert_eq!(key_label("shift"), "Shift");
        assert_eq!(key_label("h"), "H");
        assert_eq!(key_label("enter"), "Enter");
        assert_eq!(key_label("CTRL"), "Ctrl");
        assert_eq!(
            key_label("super"),
            if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Super"
            }
        );
    }
}
