//! Quiet, consistently styled transient feedback. Only explicit controls dismiss
//! a tip, so clicking its text never unexpectedly changes the workspace below.
use super::*;

const SHOWCASE_PRESS: Duration = Duration::from_millis(160);
pub(super) const SHOWCASE_FADE: Duration = Duration::from_millis(280);

/// Embedded pictograms keep showcase feedback compact and independent of font
/// glyph coverage. Focus uses arrows, while moving includes the panel outline.
fn showcase_action_icon(action: &str) -> &'static [u8] {
    macro_rules! icon {
        ($paths:literal) => {
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"#,
                $paths,
                "</svg>"
            ).as_bytes()
        };
    }
    match action {
        "Focus left" => icon!(r#"<path d="M20 12H4m6-6-6 6 6 6"/>"#),
        "Focus right" => icon!(r#"<path d="M4 12h16m-6-6 6 6-6 6"/>"#),
        "Focus strip above" => icon!(r#"<path d="M12 20V4m-6 6 6-6 6 6"/>"#),
        "Focus strip below" => icon!(r#"<path d="M12 4v16m-6-6 6 6 6-6"/>"#),
        "Focus first panel" => icon!(r#"<path d="M4 5v14m16-7H8m6-6-6 6 6 6"/>"#),
        "Focus last panel" => icon!(r#"<path d="M20 5v14M4 12h12m-6-6 6 6-6 6"/>"#),
        "Return to previous panel" => icon!(r#"<path d="m8 4-5 5 5 5M3 9h11a5 5 0 0 1 0 10h-3"/>"#),
        "Move panel left" => icon!(
            r#"<rect x="12" y="4" width="9" height="16" rx="2"/><path d="M16 12H3m4-4-4 4 4 4"/>"#
        ),
        "Move panel right" => icon!(
            r#"<rect x="3" y="4" width="9" height="16" rx="2"/><path d="M8 12h13m-4-4 4 4-4 4"/>"#
        ),
        "Move panel up" => icon!(
            r#"<rect x="4" y="12" width="16" height="9" rx="2"/><path d="M12 16V3m-4 4 4-4 4 4"/>"#
        ),
        "Move panel down" => icon!(
            r#"<rect x="4" y="3" width="16" height="9" rx="2"/><path d="M12 8v13m-4-4 4 4 4-4"/>"#
        ),
        "Move panel to start" => icon!(
            r#"<rect x="14" y="4" width="7" height="16" rx="2"/><path d="M3 5v14m14-7H7m4-4-4 4 4 4"/>"#
        ),
        "Move panel to end" => icon!(
            r#"<rect x="3" y="4" width="7" height="16" rx="2"/><path d="M21 5v14M7 12h10m-4-4 4 4-4 4"/>"#
        ),
        "New session" => icon!(
            r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M7 12h10m-5-5v10"/>"#
        ),
        "Close panel" => icon!(
            r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="m8 8 8 8m0-8-8 8"/>"#
        ),
        "Toggle overview" => icon!(
            r#"<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>"#
        ),
        "Show all shortcuts" => icon!(
            r#"<rect x="2" y="5" width="20" height="14" rx="2"/><path d="M6 9h1m4 0h1m4 0h1M6 12h1m4 0h1m4 0h1M7 16h10"/>"#
        ),
        "Panel width 25%" => icon!(
            r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M7 4v16M3 8h3m-3 4h3m-3 4h3"/>"#
        ),
        "Panel width 50%" => icon!(
            r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M12 4v16M3 8h8m-8 4h8m-8 4h8"/>"#
        ),
        "Panel width 75%" => icon!(
            r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M17 4v16M3 8h13m-13 4h13m-13 4h13"/>"#
        ),
        "Panel width 100%" => icon!(
            r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M3 8h18M3 12h18M3 16h18"/>"#
        ),
        "Cycle panel width" => {
            icon!(r#"<path d="M8 3H3v18h5m8-18h5v18h-5M6 12h12m-9-3-3 3 3 3m6-6 3 3-3 3"/>"#)
        }
        "Maximize or restore panel" => icon!(
            r#"<path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5M3 3l6 6m12-6-6 6M3 21l6-6m12 6-6-6"/>"#
        ),
        // Showcase enabled, and a neutral visual for future actions.
        _ => icon!(
            r#"<rect x="3" y="3" width="18" height="14" rx="2"/><path d="m10 7 5 3-5 3V7m2 10v4m-4 0h8"/>"#
        ),
    }
}

/// A short press acknowledgement, a quiet reading interval, then a soft exit.
/// The pulse restarts even when the same shortcut is pressed repeatedly.
fn showcase_motion(elapsed: Duration, reduce_motion: bool) -> (f32, f32, bool) {
    if reduce_motion {
        return (1.0, 0.0, false);
    }
    let press = 1.0
        - transition::ease_out_cubic(
            (elapsed.as_secs_f32() / SHOWCASE_PRESS.as_secs_f32()).min(1.0),
        );
    let fade_start = SHOWCASE_DURATION - SHOWCASE_FADE;
    let fade =
        (elapsed.saturating_sub(fade_start).as_secs_f32() / SHOWCASE_FADE.as_secs_f32()).min(1.0);
    let opacity = (1.0 - 0.15 * press) * (1.0 - fade * fade * (3.0 - 2.0 * fade));
    (
        opacity,
        press,
        elapsed < SHOWCASE_PRESS || (elapsed >= fade_start && elapsed < SHOWCASE_DURATION),
    )
}

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

/// Draw special keys instead of relying on font coverage or spelled-out names.
/// Letter and number keys keep their actual key legends.
fn showcase_key_icon(key: &str) -> Option<&'static [u8]> {
    macro_rules! icon {
        ($paths:literal) => {
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"#,
                $paths,
                "</svg>"
            ).as_bytes()
        };
    }
    Some(match key.to_ascii_lowercase().as_str() {
        "cmd" | "command" => icon!(r#"<path d="M8 8h8v8H8zM8 8H5a3 3 0 1 1 3-3v3m8 0V5a3 3 0 1 1 3 3h-3m0 8h3a3 3 0 1 1-3 3v-3m-8 0v3a3 3 0 1 1-3-3h3"/>"#),
        "super" if cfg!(target_os = "macos") => return showcase_key_icon("cmd"),
        "super" => icon!(r#"<rect x="4" y="4" width="6" height="6"/><rect x="14" y="4" width="6" height="6"/><rect x="4" y="14" width="6" height="6"/><rect x="14" y="14" width="6" height="6"/>"#),
        "shift" => icon!(r#"<path d="m12 3 9 9h-5v9H8v-9H3z"/>"#),
        "ctrl" | "control" => icon!(r#"<path d="m5 15 7-7 7 7"/>"#),
        "alt" | "option" => icon!(r#"<path d="M3 5h6l6 14h6M15 5h6"/>"#),
        "enter" | "return" => icon!(r#"<path d="M20 4v10H4m6-6-6 6 6 6"/>"#),
        "tab" => icon!(r#"<path d="M3 12h15m-6-6 6 6-6 6M21 5v14"/>"#),
        "home" => icon!(r#"<path d="M6 18V6h12M6 6l12 12"/>"#),
        "end" => icon!(r#"<path d="M6 6v12h12M6 18 18 6"/>"#),
        "escape" | "esc" => icon!(r#"<circle cx="12" cy="12" r="9"/><path d="M8 16V8h8M8 8l8 8"/>"#),
        _ => return None,
    })
}

fn shortcut_keys(shortcut: &str) -> gpui::AnyElement {
    render_shortcut_keys(shortcut, false)
}

fn render_shortcut_keys(shortcut: &str, icons: bool) -> gpui::AnyElement {
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
                                    .child(match icons.then(|| showcase_key_icon(key)).flatten() {
                                        Some(data) => gpui::svg()
                                            .debug_selector(|| "showcase-key-icon".into())
                                            .data(data)
                                            .size(px(16.0))
                                            .text_color(theme.TEXT)
                                            .into_any_element(),
                                        None => div().child(key_label(key)).into_any_element(),
                                    })
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

pub(super) const COACH_CHIP_WIDTH: f32 = 300.0;

fn chip_keys(keys: &str) -> String {
    compact_coach_keys(keys)
        .split_whitespace()
        .map(|part| {
            if matches!(part, "/" | "..") {
                part.to_owned()
            } else {
                part.split('-').map(key_label).collect::<Vec<_>>().join("+")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

struct CoachDetails(learning::Hint);

impl Render for CoachDetails {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global();
        div()
            .debug_selector(|| "coach-details".into())
            .max_w(px(360.0))
            .p_3()
            .rounded_md()
            .bg(theme.PANEL_BG)
            .border_1()
            .border_color(theme.PANEL_BORDER)
            .text_size(px(12.0))
            .text_color(theme.TEXT)
            .flex()
            .flex_col()
            .gap_2()
            .child(self.0.label)
            .child(shortcut_keys(&self.0.keys))
            .child(
                div()
                    .text_color(theme.TEXT_DIM)
                    .child(self.0.because.clone()),
            )
    }
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

impl Workspace {
    pub(super) fn render_coach_chip(
        &self,
        hint: &learning::Hint,
        progress: f32,
        left: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let (label, _) = coach_visual(hint.skill_id);
        let label = if label == "Shortcut" {
            hint.label
        } else {
            label
        };
        let slide = if transition::policy(Transition::Coach).duration.is_zero() {
            0.0
        } else {
            -6.0 * (1.0 - progress)
        };
        div()
            .id("coach-toast")
            .debug_selector(|| "coach-toast".into())
            .absolute()
            .left(px(left))
            .top(px(2.0 + slide))
            .w(px(COACH_CHIP_WIDTH))
            .h(px(28.0))
            .rounded_md()
            .bg(theme.PANEL_BG.opacity(0.65))
            .opacity(progress)
            .occlude()
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.coach.hover_hint(*hovered, learning::now());
                this.after_coach_update(cx);
            }))
            .tooltip({
                let hint = hint.clone();
                move |_, cx| cx.new(|_| CoachDetails(hint.clone())).into()
            })
            .child(
                div()
                    .debug_selector(|| "coach-title".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.0))
                    .text_color(theme.TEXT_DIM)
                    .child(label),
            )
            .child(
                div()
                    .debug_selector(|| "coach-keys".into())
                    .flex_none()
                    .font_family(theme.FONT_MONO)
                    .text_size(px(10.0))
                    .text_color(theme.TEXT)
                    .child(chip_keys(hint.keys)),
            )
            .child(
                div()
                    .id("coach-dismiss")
                    .debug_selector(|| "coach-dismiss".into())
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_size(px(14.0))
                    .text_color(theme.TEXT_DIM)
                    .hover(|s| s.bg(theme.HEADER_BG).text_color(theme.TEXT))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.dismiss_coach_hint(cx);
                        }),
                    )
                    .child("×"),
            )
            .into_any_element()
    }

    pub(super) fn render_showcase_cue(
        &self,
        cue: &ShowcaseCue,
        window: &mut Window,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let (opacity, press, animating) = cue.started_at.map_or((1.0, 0.0, false), |start| {
            showcase_motion(
                start.elapsed(),
                crate::config::get().appearance.reduce_motion,
            )
        });
        if animating {
            window.request_animation_frame();
        }
        div()
            .id("showcase-shortcut")
            .debug_selector(|| "showcase-shortcut".into())
            .absolute()
            .left(px(16.0))
            .right(px(16.0))
            .bottom(px(56.0 - 4.0 * press))
            .opacity(opacity)
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
                    .border_color(theme.PANEL_BORDER.blend(theme.ACCENT.opacity(0.45 * press)))
                    .shadow_md()
                    .child(
                        div()
                            .id("showcase-action")
                            .debug_selector(|| "showcase-action".into())
                            .size(px(24.0))
                            .flex_shrink_0()
                            .child(
                                gpui::svg()
                                    .debug_selector(|| "showcase-action-icon".into())
                                    .data(showcase_action_icon(cue.action))
                                    .size(px(24.0))
                                    .text_color(theme.TEXT),
                            ),
                    )
                    .child(
                        div()
                            .id("showcase-key")
                            .debug_selector(|| "showcase-key".into())
                            .child(render_shortcut_keys(&cue.shortcut, true)),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn showcase_actions_have_distinct_text_free_icons() {
        let actions = [
            "Focus left",
            "Focus right",
            "Focus strip above",
            "Focus strip below",
            "Focus first panel",
            "Focus last panel",
            "Return to previous panel",
            "Move panel left",
            "Move panel right",
            "Move panel up",
            "Move panel down",
            "Move panel to start",
            "Move panel to end",
            "New session",
            "Close panel",
            "Toggle overview",
            "Show all shortcuts",
            "Panel width 25%",
            "Panel width 50%",
            "Panel width 75%",
            "Panel width 100%",
            "Cycle panel width",
            "Maximize or restore panel",
            "Showcase mode on",
        ];
        let mut icons = std::collections::HashSet::new();
        for action in actions {
            let data = showcase_action_icon(action);
            let svg = std::str::from_utf8(data).unwrap();
            assert!(
                svg.starts_with("<svg ") && svg.ends_with("</svg>"),
                "{action}"
            );
            assert!(!svg.contains("<text") && !svg.contains(action), "{action}");
            assert!(
                icons.insert(data),
                "{action} must have a distinct pictogram"
            );
        }
        let source = include_str!("workspace_notifications.rs");
        let renderer = source
            .split("pub(super) fn render_showcase_cue(")
            .nth(1)
            .unwrap()
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(renderer.contains("showcase_action_icon(cue.action)"));
        assert!(renderer.contains("render_shortcut_keys(&cue.shortcut, true)"));
        assert!(!renderer.contains(".child(cue.action)"));
    }

    #[test]
    fn showcase_special_keys_have_text_free_icons() {
        for key in [
            "Super", "Cmd", "Command", "Shift", "Ctrl", "Control", "Alt", "Option",
            "Enter", "Return", "Tab", "Home", "End", "Escape", "Esc",
        ] {
            let svg = std::str::from_utf8(showcase_key_icon(key).expect(key)).unwrap();
            assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>"), "{key}");
            assert!(!svg.contains("<text"), "{key}");
            assert_eq!(showcase_key_icon(key), showcase_key_icon(&key.to_lowercase()));
        }
        for key in ["H", "J", "K", "L", "S", "1", "2", "3", "4", "/"] {
            assert!(showcase_key_icon(key).is_none(), "preserve the {key} key legend");
        }
    }

    #[test]
    fn showcase_press_settles_then_fades_without_animating_the_hold() {
        assert_eq!(showcase_motion(Duration::ZERO, false), (0.85, 1.0, true));
        let (opacity, press, animating) = showcase_motion(SHOWCASE_PRESS / 2, false);
        assert!(opacity > 0.85 && opacity < 1.0);
        assert!(press > 0.0 && press < 1.0 && animating);
        assert_eq!(showcase_motion(SHOWCASE_PRESS, false), (1.0, 0.0, false));
        assert_eq!(
            showcase_motion(SHOWCASE_DURATION - SHOWCASE_FADE, false),
            (1.0, 0.0, true)
        );
        let (opacity, press, animating) =
            showcase_motion(SHOWCASE_DURATION - SHOWCASE_FADE / 2, false);
        assert!((opacity - 0.5).abs() < 0.001);
        assert_eq!((press, animating), (0.0, true));
        assert_eq!(showcase_motion(SHOWCASE_DURATION, false), (0.0, 0.0, false));
        let mut previous = 1.0;
        for ms in 0..=SHOWCASE_FADE.as_millis() as u64 {
            let (opacity, _, _) = showcase_motion(
                SHOWCASE_DURATION - SHOWCASE_FADE + Duration::from_millis(ms),
                false,
            );
            assert!(opacity <= previous && opacity >= 0.0);
            previous = opacity;
        }
    }

    #[test]
    fn showcase_reduced_motion_stays_readable_without_requesting_frames() {
        for elapsed in [Duration::ZERO, SHOWCASE_PRESS, SHOWCASE_DURATION] {
            assert_eq!(showcase_motion(elapsed, true), (1.0, 0.0, false));
        }
    }

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
