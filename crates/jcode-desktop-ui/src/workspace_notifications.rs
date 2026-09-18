//! Quiet, consistently styled transient feedback. Only explicit controls dismiss
//! a tip, so clicking its text never unexpectedly changes the workspace below.
use super::*;

const SHOWCASE_PRESS: Duration = Duration::from_millis(160);
pub(super) const SHOWCASE_FADE: Duration = Duration::from_millis(280);

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
