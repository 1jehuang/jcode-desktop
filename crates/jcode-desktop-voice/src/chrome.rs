//! The voice button's chrome: the platform shortcut keycap, its tooltip,
//! short pill labels for voice outcomes, and the idle microphone/shortcut
//! crossfade. Holds no panel state, so it is testable without a Panel.
use crate::theme::Theme;
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Task, Window, div, prelude::*, px};
use std::time::{Duration, Instant};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const VOICE_SHORTCUT: &str = "Copilot key";
// Registered globally by the host (RegisterHotKey), so it works unfocused too.
#[cfg(target_os = "windows")]
pub const VOICE_SHORTCUT: &str = "Ctrl+Shift+Space";
#[cfg(target_os = "macos")]
pub const VOICE_SHORTCUT: &str = "⌘⇧M (Command+Shift+M)";

/// Compact keycap shown inside the voice pill so the shortcut is visible
/// without hovering. Linux draws the Copilot key glyph instead of text.
#[cfg(target_os = "windows")]
const VOICE_SHORTCUT_KEYCAP: &str = "Ctrl+Shift+Space";
#[cfg(target_os = "macos")]
const VOICE_SHORTCUT_KEYCAP: &str = "⌘⇧M";

/// Linux: a small outlined keycap holding the Copilot key glyph.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn voice_shortcut_keycap(color: gpui::Hsla, _theme: &Theme) -> gpui::AnyElement {
    div()
        .debug_selector(|| "voice-shortcut".into())
        .flex_none()
        .size(px(15.))
        .rounded(px(3.5))
        .border_1()
        .border_color(color.opacity(0.7))
        .flex()
        .items_center()
        .justify_center()
        .child(
            gpui::svg()
                .debug_selector(|| "voice-shortcut-copilot-icon".into())
                .data(include_bytes!("../../../assets/icons/copilot.svg") as &'static [u8])
                .text_color(color)
                .size(px(9.)),
        )
        .into_any_element()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn voice_shortcut_keycap(color: gpui::Hsla, theme: &Theme) -> gpui::AnyElement {
    div()
        .debug_selector(|| "voice-shortcut".into())
        .flex_none()
        .whitespace_nowrap()
        .text_size(px(10.5))
        .font_family(theme.FONT_MONO)
        .text_color(color)
        .child(VOICE_SHORTCUT_KEYCAP)
        .into_any_element()
}

pub fn voice_tooltip(action: &str, status: &str) -> String {
    format!(
        "{action} · {VOICE_SHORTCUT}\nHold {VOICE_SHORTCUT} to transcribe, release to finish. {status}. Ctrl+Shift+V toggles recording in the composer. Audio streams to Nari. After transcription, Jev chooses a coding agent or a quick navigation action."
    )
}

pub struct VoiceTooltip(pub String);
impl Render for VoiceTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let _render_scope = crate::render_stats::scope("VoiceTooltip");
        div()
            .debug_selector(|| "voice-shortcut-tooltip".into())
            .max_w(px(300.))
            .p_2()
            .rounded_md()
            .bg(Theme::global().HEADER_BG)
            .text_size(px(12.))
            .text_color(Theme::global().TEXT)
            .child(self.0.clone())
    }
}

/// Compact OS-pill label for Jev's decision, e.g. "Jev → Coding agent".
pub fn global_pill_decision(decision: &str) -> String {
    let Some(decision) = decision.strip_prefix("Jev chose: ") else {
        return decision.to_string();
    };
    let (route, detail) = decision.split_once(" · ").unwrap_or((decision, ""));
    match route {
        "Quick action" if !detail.is_empty() && detail != "Navigation" => format!("Jev → {detail}"),
        route => format!("Jev → {route}"),
    }
}

/// Compact OS-pill label for a finished attempt that did not act.
pub fn global_pill_error(error: &str) -> String {
    if error.starts_with("Jev could not match") {
        "No matching session · Kept in draft".into()
    } else if error.starts_with("Jev routing unavailable") {
        "Jev unavailable · Kept in draft".into()
    } else if error.contains("Navigation unavailable") {
        "Jev → Quick action · Kept in draft".into()
    } else {
        short_voice_status(error)
    }
}

/// Short, pill-sized wording for a voice failure. Full errors stay in logs.
pub fn short_voice_status(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        "Transcription timed out".into()
    } else if lower.starts_with("no speech") {
        "No speech detected".into()
    } else if lower.contains("not configured") {
        "Voice not configured".into()
    } else if error.chars().count() > 40 {
        "Voice failed".into()
    } else {
        error.trim_end_matches('.').to_string()
    }
}
/// Height (and minimum width) of the in-box voice button.
pub const VOICE_BUTTON_SIZE: f32 = 26.;
/// One full microphone, shortcut, microphone cycle.
const VOICE_SWAP_CYCLE: Duration = Duration::from_millis(7000);
/// Frame interval while the voice button crossfades.
const VOICE_SWAP_FRAME: Duration = Duration::from_nanos(33_333_334);
/// Portion of each half cycle spent crossfading.
const VOICE_SWAP_FADE: f32 = 0.07;

pub fn voice_microphone_face(color: gpui::Hsla) -> gpui::Div {
    div()
        .debug_selector(|| "voice-microphone-icon".into())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            gpui::svg()
                .data(include_bytes!("../../../assets/icons/microphone.svg") as &'static [u8])
                .text_color(color)
                .size(px(12.)),
        )
}

pub fn voice_shortcut_face(color: gpui::Hsla) -> gpui::Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(voice_shortcut_keycap(color, &Theme::global()))
}

/// The voice button's two faces, alternating on their own clock, so a
/// crossfade frame redraws only this view. The panel passes the colors.
pub struct VoiceSwap {
    epoch: Instant,
    _clock: Task<()>,
    /// Microphone and keycap colors, set by the panel that owns the button.
    pub colors: Option<(gpui::Hsla, gpui::Hsla)>,
}

impl VoiceSwap {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let epoch = cx.background_executor().now();
        let id = cx.entity_id();
        // The clock sleeps through the holds and ticks while the faces move.
        // It only notifies this view by id: updating it would count as a
        // change for the panel around it, and build that again every tick.
        let clock = cx.spawn(async move |this, cx| {
            loop {
                let now = cx.background_executor().now();
                let t = voice_swap_phase(epoch, now);
                let idle = VOICE_SWAP_CYCLE.mul_f32(voice_swap_idle(t));
                if idle > VOICE_SWAP_FRAME {
                    cx.background_executor().timer(idle).await;
                } else {
                    crate::animation_clock::next_tick(cx.background_executor(), VOICE_SWAP_FRAME)
                        .await;
                }
                if this.upgrade().is_none() {
                    break;
                }
                cx.update(|cx| cx.notify(id));
            }
        });
        Self {
            epoch,
            _clock: clock,
            colors: None,
        }
    }
}

/// Where in the swap cycle `now` falls, from 0 to 1.
fn voice_swap_phase(epoch: Instant, now: Instant) -> f32 {
    (now.saturating_duration_since(epoch).as_secs_f32() / VOICE_SWAP_CYCLE.as_secs_f32()).fract()
}

impl Render for VoiceSwap {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _render_scope = crate::render_stats::scope("VoiceSwap");
        let t = voice_swap_phase(self.epoch, cx.background_executor().now());
        let (mic_opacity, mic_offset) = voice_swap_frame(t, false);
        let (key_opacity, key_offset) = voice_swap_frame(t, true);
        let (icon, keycap) = self.colors.unwrap_or_default();
        div()
            .absolute()
            .inset_0()
            .child(
                voice_microphone_face(icon)
                    .opacity(mic_opacity)
                    .top(px(mic_offset))
                    .bottom(px(-mic_offset)),
            )
            .child(
                voice_shortcut_face(keycap)
                    .opacity(key_opacity)
                    .top(px(key_offset))
                    .bottom(px(-key_offset)),
            )
    }
}

/// Cycle fraction from `t` until the voice button next moves. Zero while a
/// crossfade is running.
fn voice_swap_idle(t: f32) -> f32 {
    if t < 0.5 - VOICE_SWAP_FADE {
        0.5 - VOICE_SWAP_FADE - t
    } else if t < 0.5 {
        0.
    } else if t < 1. - VOICE_SWAP_FADE {
        1. - VOICE_SWAP_FADE - t
    } else {
        0.
    }
}

/// Opacity and vertical offset for one face of the voice button at cycle
/// progress `t`. The microphone holds for most of the first half, then the
/// shortcut rises in while the microphone rises out, and back again.
fn voice_swap_frame(t: f32, shortcut: bool) -> (f32, f32) {
    const FADE: f32 = VOICE_SWAP_FADE;
    const RISE: f32 = 6.;
    let ease = |x: f32| {
        let x = x.clamp(0., 1.);
        x * x * (3. - 2. * x)
    };
    // 0 shows the microphone, 1 the shortcut.
    let (mix, rising) = if t < 0.5 - FADE {
        (0., true)
    } else if t < 0.5 {
        (ease((t - (0.5 - FADE)) / FADE), true)
    } else if t < 1. - FADE {
        (1., false)
    } else {
        (1. - ease((t - (1. - FADE)) / FADE), false)
    };
    let visible = if shortcut { mix } else { 1. - mix };
    // Faces enter from below and leave upward.
    let offset = if shortcut == rising {
        (1. - visible) * RISE
    } else {
        -(1. - visible) * RISE
    };
    (visible, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_button_sleeps_through_holds_and_ticks_only_while_crossfading() {
        assert!((voice_swap_idle(0.) - (0.5 - VOICE_SWAP_FADE)).abs() < 1e-6);
        assert_eq!(voice_swap_idle(0.47), 0.);
        assert!((voice_swap_idle(0.5) - (0.5 - VOICE_SWAP_FADE)).abs() < 1e-6);
        assert_eq!(voice_swap_idle(0.97), 0.);
        // Faces are exactly still wherever the scheduler sleeps.
        for (t, still) in [
            (0., 0.2),
            (0.1, 0.2),
            (0.42, 0.2),
            (0.5, 0.7),
            (0.7, 0.7),
            (0.92, 0.7),
        ] {
            assert!(voice_swap_idle(t) > 0.);
            for shortcut in [false, true] {
                assert_eq!(
                    voice_swap_frame(t, shortcut),
                    voice_swap_frame(still, shortcut)
                );
            }
        }
    }

    #[test]
    fn voice_button_alternates_microphone_and_shortcut() {
        // Mostly one face at a time, with a brief crossfade at each swap.
        let (mic, _) = voice_swap_frame(0.1, false);
        let (key, _) = voice_swap_frame(0.1, true);
        assert_eq!((mic, key), (1., 0.));
        let (mic, _) = voice_swap_frame(0.7, false);
        let (key, _) = voice_swap_frame(0.7, true);
        assert_eq!((mic, key), (0., 1.));
        for t in [0., 0.2, 0.45, 0.47, 0.5, 0.8, 0.95, 0.99] {
            let (mic, _) = voice_swap_frame(t, false);
            let (key, _) = voice_swap_frame(t, true);
            assert!((mic + key - 1.).abs() < 1e-5, "t={t}");
        }
        // The cycle loops seamlessly.
        assert_eq!(voice_swap_frame(0., false), voice_swap_frame(1., false));
    }
}
