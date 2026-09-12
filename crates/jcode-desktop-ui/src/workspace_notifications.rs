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

/// Share modifiers instead of repeating a wide Super/Shift chord twice.
fn compact_coach_keys(keys: &str) -> String {
    if let Some((first, second)) = keys.split_once(" / ")
        && let (Some((modifiers, _)), Some((other_modifiers, key))) =
            (first.rsplit_once('-'), second.rsplit_once('-'))
        && modifiers == other_modifiers
    {
        return format!("{first} / {key}");
    }
    keys.to_owned()
}

pub(super) fn coach_strip_height(compact: bool) -> f32 {
    if compact { 80.0 } else { 56.0 }
}

fn coach_visual(skill: &str) -> (&'static str, &'static str) {
    match skill {
        "focus_left_right" => ("Switch panels", "↔"),
        "focus_up_down" => ("Switch strips", "↕"),
        "focus_first_last" => ("Jump to ends", "↔"),
        "focus_previous" => ("Previous panel", "↶"),
        "overview" => ("Overview", "⊞"),
        "move_panel" => ("Reorder panel", "↔"),
        "move_panel_strip" => ("Move to strip", "↕"),
        "move_panel_end" => ("Move to end", "→"),
        "cycle_width" => ("Resize panel", "↔"),
        "maximize" => ("Fill / restore", "↔"),
        "width_presets" => ("Panel width", "↔"),
        "new_panel" => ("New session", "+"),
        "close_panel" => ("Close panel", "×"),
        _ => ("Shortcut", "→"),
    }
}

/// Tiny panel silhouettes keep the action visual without a paragraph of copy.
fn coach_diagram(skill: &str, symbol: &'static str) -> gpui::AnyElement {
    let theme = Theme::global();
    let vertical = matches!(skill, "focus_up_down" | "move_panel_strip");
    let sizing = matches!(skill, "cycle_width" | "maximize" | "width_presets");
    div()
        .debug_selector(|| "coach-diagram".into())
        .w(px(60.0))
        .h(px(34.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(3.0))
        .when(vertical, |el| el.flex_col())
        .children((0..3).map(|index| {
            let active = if skill == "focus_first_last" {
                index != 1
            } else {
                index == 1
            };
            div()
                .w(px(if vertical {
                    28.0
                } else if sizing && index == 1 {
                    28.0
                } else {
                    16.0
                }))
                .h(px(if vertical { 9.0 } else { 28.0 }))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.0))
                .border_1()
                .border_color(if active {
                    theme.ACCENT.opacity(0.7)
                } else {
                    theme.PANEL_BORDER
                })
                .bg(if active {
                    theme.ACCENT.opacity(0.12)
                } else {
                    theme.HEADER_BG
                })
                .text_size(px(if vertical { 10.0 } else { 14.0 }))
                .text_color(theme.ACCENT)
                .when(index == 1, |el| el.child(symbol))
        }))
        .into_any_element()
}

impl Workspace {
    pub(super) fn render_coach_toast(
        &self,
        hint: &learning::Hint,
        progress: f32,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let now = learning::now();
        let (label, symbol) = coach_visual(hint.skill_id);
        let label = if label == "Shortcut" { hint.label } else { label };
        div()
            .id("coach-toast")
            .debug_selector(|| "coach-toast".into())
            .occlude()
            .w_full()
            .h(px(coach_strip_height(compact)))
            .flex_none()
            .min_w_0()
            .overflow_hidden()
            .opacity(progress)
            .border_t_1()
            .border_color(theme.PANEL_BORDER)
            .bg(theme.PANEL_BG)
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(coach_diagram(hint.skill_id, symbol))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .when(compact, |el| el.flex_col().items_start())
                    .when(!compact, |el| el.items_center())
                    .gap(px(8.0))
                    .child(
                        div()
                            .debug_selector(|| "coach-title".into())
                            .text_size(px(12.0))
                            .text_color(theme.TEXT)
                            .child(label),
                    )
                    .child(
                        div()
                            .debug_selector(|| "coach-keys".into())
                            .child(shortcut_keys(&compact_coach_keys(hint.keys))),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "coach-countdown".into())
                    .w(px(28.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .font_family(theme.FONT_MONO)
                    .text_size(px(11.0))
                    .text_color(theme.TEXT_DIM)
                    .child(format!("{}s", hint.remaining_seconds(now)))
                    .child(
                        div()
                            .w_full()
                            .h(px(2.0))
                            .rounded_full()
                            .bg(theme.PANEL_BORDER)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(hint.remaining_fraction(now)))
                                    .bg(theme.ACCENT),
                            ),
                    ),
            )
            .child(
                div()
                    .id("coach-dismiss")
                    .debug_selector(|| "coach-dismiss".into())
                    .size(px(28.0))
                    .flex_none()
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
    fn coach_keycaps_share_only_matching_modifiers() {
        assert_eq!(compact_coach_keys("super-u / super-p"), "super-u / p");
        assert_eq!(
            compact_coach_keys("super-shift-h / super-shift-l"),
            "super-shift-h / l"
        );
        assert_eq!(compact_coach_keys("super-h / alt-l"), "super-h / alt-l");
        assert_eq!(compact_coach_keys("super-enter"), "super-enter");
        for skill in learning::SKILLS {
            assert_ne!(
                coach_visual(skill.id).0,
                "Shortcut",
                "{} needs a visual",
                skill.id
            );
        }
    }

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
