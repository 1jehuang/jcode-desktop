//! A non-activating, OS-level voice status pill.
//!
//! The owner manages lifetime and closes this window when voice is inactive.
//! This module deliberately never falls back to a normal, focusable
//! application window. Backends:
//! * native Wayland: `zwlr_layer_shell_v1` overlay layer, bottom anchored, no
//!   keyboard interactivity. Compositors without layer shell (GNOME/Mutter)
//!   are rejected so the owner falls back to focused-only voice.
//! * X11 (including XWayland): `WindowKind::PopUp`, which GPUI maps to an
//!   override-redirect `_NET_WM_WINDOW_TYPE_NOTIFICATION` window.
//! * macOS: `WindowKind::PopUp`, a nonactivating `NSPanel` at
//!   `NSPopUpWindowLevel` that joins all spaces.
//! * Windows: `WindowKind::PopUp`, `WS_EX_TOOLWINDOW | WS_EX_TOPMOST`, shown
//!   with `SW_SHOWNOACTIVATE` because `focus` is false.
use crate::theme::Theme;
use gpui::{
    App, Bounds, Context, Pixels, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions, div, point, prelude::*, px, size,
};

/// Wide enough for a decision like "Jev → Previous session". The pill
/// itself hugs its content, so the rest of the surface stays transparent.
const SURFACE_WIDTH: f32 = 264.;
const SURFACE_HEIGHT: f32 = 44.;
/// Gap between the pill surface and the bottom of the usable display area.
const BOTTOM_MARGIN: f32 = 14.;
const PILL_WIDTH: f32 = 160.;
const PILL_MAX_WIDTH: f32 = SURFACE_WIDTH - 16.;
const PILL_HEIGHT: f32 = 28.;
const BAR_WIDTH: f32 = 2.;
const BAR_GAP: f32 = 1.;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    /// Visible status, including connecting, listening, transcribing or an error.
    pub title: String,
    /// The same 24 chronological RMS samples used by the panel voice meter.
    pub levels: Option<[f32; 24]>,
    /// The hold finished and `title` states what Jev decided (or why it did
    /// not act), so it reads as a result rather than a progress status.
    pub decided: bool,
}

pub(crate) struct VoiceOverlay {
    snapshot: Snapshot,
}

/// How the OS pill is realized for a given platform and GPUI backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// Native Wayland layer-shell surface. Fails on compositors without it.
    LayerShell,
    /// A platform "pop-up" window that never takes focus (X11, macOS, Windows).
    PopUp,
    /// No non-activating surface is known to exist (for example the headless
    /// or an unknown backend). Global voice must stay focused-only.
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Os {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Os {
    pub(crate) const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Other
        }
    }
}

/// Pure backend choice. `compositor` is GPUI's actual backend name, which is
/// "Wayland" or "X11" on Linux. WAYLAND_DISPLAY may be set while GPUI uses
/// XWayland, so never infer from the environment.
///
/// LayerShell code is compiled only on Linux, whose target dependencies always
/// enable GPUI's `wayland` feature (FreeBSD builds X11 only and is excluded).
pub(crate) fn backend_for(os: Os, compositor: &str) -> Backend {
    match os {
        Os::Linux if compositor == "Wayland" => Backend::LayerShell,
        Os::Linux if compositor == "X11" => Backend::PopUp,
        Os::MacOs | Os::Windows => Backend::PopUp,
        _ => Backend::Unsupported,
    }
}

pub(crate) fn current_backend(cx: &App) -> Backend {
    backend_for(Os::current(), cx.compositor_name())
}

/// Whether an OS pill can be attempted at all. A layer-shell compositor may
/// still reject the surface at open time, which the owner handles by
/// disabling global capture rather than recording invisibly.
pub(crate) fn available(cx: &App) -> bool {
    current_backend(cx) != Backend::Unsupported
}

/// Error for Wayland compositors that lack `zwlr_layer_shell_v1`.
#[derive(Debug)]
pub(crate) struct NoLayerShell;
impl std::fmt::Display for NoLayerShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "this Wayland compositor has no zwlr_layer_shell_v1 (for example GNOME/Mutter), \
             so no non-focusing voice pill can be shown; global voice stays focused-only",
        )
    }
}
impl std::error::Error for NoLayerShell {}

pub(crate) fn open(snapshot: Snapshot, cx: &mut App) -> anyhow::Result<WindowHandle<VoiceOverlay>> {
    let backend = current_backend(cx);
    let options = match backend {
        Backend::Unsupported => anyhow::bail!(
            "no non-activating voice overlay on the {:?} backend",
            cx.compositor_name()
        ),
        Backend::LayerShell => layer_shell_options()?,
        Backend::PopUp => {
            let display = cx.primary_display();
            let area = display.as_ref().map(|display| display.visible_bounds());
            popup_options(area, display.map(|display| display.id()))
        }
    };
    let result = cx.open_window(options, move |_, cx| cx.new(|_| VoiceOverlay { snapshot }));
    #[cfg(target_os = "linux")]
    if backend == Backend::LayerShell {
        return result.map_err(|error| {
            if error.is::<gpui::layer_shell::LayerShellNotSupportedError>() {
                anyhow::Error::new(NoLayerShell)
            } else {
                error
            }
        });
    }
    result
}

fn base_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: false,
        show: true,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        // Client mode plus no titlebar ensures we neither request server
        // decorations nor draw application decorations.
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some("jcode-voice-overlay".into()),
        ..Default::default()
    }
}

/// Bottom-center of the usable display area (above a taskbar or Dock).
fn popup_bounds(area: Option<Bounds<Pixels>>) -> Bounds<Pixels> {
    let surface = size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT));
    let Some(area) = area else {
        return Bounds { origin: point(px(0.), px(0.)), size: surface };
    };
    let x = area.origin.x + (area.size.width - surface.width) / 2.;
    let y = area.origin.y + area.size.height - surface.height - px(BOTTOM_MARGIN);
    Bounds { origin: point(x, y.max(area.origin.y)), size: surface }
}

fn popup_options(area: Option<Bounds<Pixels>>, display_id: Option<gpui::DisplayId>) -> WindowOptions {
    WindowOptions {
        kind: WindowKind::PopUp,
        display_id,
        ..base_options(popup_bounds(area))
    }
}

#[cfg(target_os = "linux")]
fn layer_shell_options() -> anyhow::Result<WindowOptions> {
    use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
    Ok(WindowOptions {
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: "jcode-voice-overlay".into(),
            layer: Layer::Overlay,
            // A bottom-only anchor centers a fixed-width surface horizontally.
            anchor: Anchor::BOTTOM,
            exclusive_zone: Some(px(0.)),
            exclusive_edge: None,
            margin: Some((px(0.), px(0.), px(BOTTOM_MARGIN), px(0.))),
            keyboard_interactivity: KeyboardInteractivity::None,
        }),
        ..base_options(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT)),
        })
    })
}

#[cfg(not(target_os = "linux"))]
fn layer_shell_options() -> anyhow::Result<WindowOptions> {
    Err(anyhow::Error::new(NoLayerShell))
}

const METER_MAX: f32 = 16.;
// The panel_voice_overlay RMS curve, scaled to the compact pill: 2px silence.
fn meter_height(level: f32) -> f32 {
    2.0 + (level.max(0.0) * 4.0).sqrt().min(1.0) * (METER_MAX - 2.0)
}

impl VoiceOverlay {
    pub(crate) fn set_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        if self.snapshot != snapshot {
            self.snapshot = snapshot;
            cx.notify();
        }
    }
}

impl Render for VoiceOverlay {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("global-voice-overlay")
                    .debug_selector(|| "global-voice-overlay".into())
                    .min_w(px(PILL_WIDTH))
                    .max_w(px(PILL_MAX_WIDTH))
                    .h(px(PILL_HEIGHT))
                    .px(px(12.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.ACCENT.opacity(0.25))
                    .bg(theme.PANEL_BG)
                    .shadow_md()
                    .text_color(theme.TEXT)
                    .text_size(px(11.))
                    // Retain the status even while the waveform is visible.
                    .child(
                        div()
                            .debug_selector(|| "global-voice-status".into())
                            .min_w_0()
                            .flex_shrink(1.)
                            .line_height(px(14.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if self.snapshot.decided {
                                theme.TEXT
                            } else {
                                theme.TEXT_DIM
                            })
                            .child(self.snapshot.title.clone()),
                    )
                    .when_some(self.snapshot.levels, |el, levels| {
                        el.child(
                            div()
                                .debug_selector(|| "global-voice-waveform".into())
                                .h(px(METER_MAX))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(px(BAR_GAP))
                                .children(levels.into_iter().map(|level| {
                                    div()
                                        .w(px(BAR_WIDTH))
                                        .h(px(meter_height(level)))
                                        .rounded_full()
                                        .bg(theme.ACCENT.opacity(0.9))
                                })),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_meter_matches_panel_and_handles_invalid_samples() {
        assert_eq!(meter_height(0.), 2.);
        assert_eq!(meter_height(-1.), 2.);
        assert_eq!(meter_height(f32::NAN), 2.);
        assert_eq!(meter_height(f32::NEG_INFINITY), 2.);
        assert_eq!(meter_height(f32::INFINITY), METER_MAX);
        assert_eq!(meter_height(1.), METER_MAX);
        assert_eq!(meter_height(2.), METER_MAX);
        assert!(meter_height(0.02) > 5.);
        assert!(meter_height(0.1) > meter_height(0.01));
        assert!(METER_MAX < PILL_HEIGHT - 2.);
    }

    #[test]
    fn backend_choice_per_platform_never_falls_back_to_a_normal_window() {
        assert_eq!(backend_for(Os::Linux, "Wayland"), Backend::LayerShell);
        assert_eq!(backend_for(Os::Linux, "X11"), Backend::PopUp);
        assert_eq!(backend_for(Os::Linux, "headless"), Backend::Unsupported);
        assert_eq!(backend_for(Os::Linux, ""), Backend::Unsupported);
        assert_eq!(backend_for(Os::MacOs, ""), Backend::PopUp);
        assert_eq!(backend_for(Os::Windows, ""), Backend::PopUp);
        assert_eq!(backend_for(Os::Other, "X11"), Backend::Unsupported);
    }

    fn assert_non_activating(options: &WindowOptions) {
        assert!(!options.focus, "overlay must never take focus");
        assert!(options.show);
        assert!(options.titlebar.is_none());
        assert!(!options.is_movable && !options.is_resizable && !options.is_minimizable);
        assert!(matches!(
            options.window_background,
            WindowBackgroundAppearance::Transparent
        ));
    }

    #[test]
    fn popup_is_non_activating_and_bottom_centered_in_usable_area() {
        let area = Bounds {
            origin: point(px(100.), px(30.)),
            size: size(px(1280.), px(770.)),
        };
        let options = popup_options(Some(area), None);
        assert_non_activating(&options);
        assert!(matches!(options.kind, WindowKind::PopUp));
        let Some(WindowBounds::Windowed(bounds)) = options.window_bounds else {
            panic!("expected fixed windowed bounds");
        };
        assert_eq!(bounds.size, size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT)));
        assert_eq!(bounds.center().x, area.center().x);
        assert_eq!(bounds.bottom(), area.bottom() - px(BOTTOM_MARGIN));
        // Tiny or missing displays still yield an on-screen fixed surface.
        let tiny = Bounds { origin: point(px(0.), px(0.)), size: size(px(20.), px(20.)) };
        let Some(WindowBounds::Windowed(bounds)) = popup_options(Some(tiny), None).window_bounds
        else {
            panic!()
        };
        assert!(bounds.top() >= px(0.));
        let Some(WindowBounds::Windowed(bounds)) = popup_options(None, None).window_bounds else {
            panic!()
        };
        assert_eq!(bounds.origin, point(px(0.), px(0.)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn layer_shell_options_never_request_focus_or_reserved_space() {
        use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
        let options = layer_shell_options().unwrap();
        assert_non_activating(&options);
        let WindowKind::LayerShell(layer) = options.kind else {
            panic!("must never fall back to a regular window");
        };
        assert_eq!(layer.layer, Layer::Overlay);
        assert_eq!(layer.anchor, Anchor::BOTTOM);
        assert_eq!(layer.keyboard_interactivity, KeyboardInteractivity::None);
        assert_eq!(layer.exclusive_zone, Some(px(0.)));
    }

    #[test]
    fn missing_layer_shell_error_is_actionable() {
        let message = NoLayerShell.to_string();
        assert!(message.contains("zwlr_layer_shell_v1") && message.contains("focused-only"));
    }

    // TestAppContext uses GPUI's fake platform, never a live native window.
    #[gpui::test]
    fn renders_status_and_meter_in_fixed_centered_pill(cx: &mut gpui::TestAppContext) {
        let (overlay, vcx) = cx.add_window_view(|_, _| VoiceOverlay {
            snapshot: Snapshot {
                title: "Connecting…".into(),
                levels: None,
                decided: false,
            },
        });
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT)));
        for (title, levels) in [
            ("Connecting…", None),
            ("Listening", Some([0.02; 24])),
            ("Transcribing…", None),
            ("Jev is choosing…", None),
            ("Jev → Previous session", None),
            ("Voice error: microphone unavailable", None),
        ] {
            overlay.update(vcx, |overlay, cx| {
                overlay.set_snapshot(
                    Snapshot {
                        title: title.into(),
                        levels,
                        decided: title.starts_with("Jev →"),
                    },
                    cx,
                );
            });
            vcx.run_until_parked();
            let pill = vcx.debug_bounds("global-voice-overlay").unwrap();
            assert_eq!(pill.size.height, px(PILL_HEIGHT));
            assert!(pill.size.width >= px(PILL_WIDTH) && pill.size.width <= px(PILL_MAX_WIDTH));
            assert_eq!(
                pill.center(),
                gpui::point(px(SURFACE_WIDTH / 2.), px(SURFACE_HEIGHT / 2.))
            );
            let status = vcx.debug_bounds("global-voice-status").unwrap();
            assert!(status.left() >= pill.left() && status.right() <= pill.right());
            assert!(status.top() >= pill.top() && status.bottom() <= pill.bottom());
            let meter = vcx.debug_bounds("global-voice-waveform");
            assert_eq!(meter.is_some(), levels.is_some());
            if let Some(meter) = meter {
                // Single row: label left of the meter, both inside the pill.
                assert!(status.right() <= meter.left(), "{title}: label overlaps meter");
                assert!(meter.left() >= pill.left() && meter.right() <= pill.right());
                assert!(meter.top() >= pill.top() && meter.bottom() <= pill.bottom());
            }
            overlay.read_with(vcx, |overlay, _| assert_eq!(overlay.snapshot.title, title));
        }
    }
}
