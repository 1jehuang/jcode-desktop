//! Live tab activity fill.
//!
//! The session emoji never moves. State is painted as a translucent wash over
//! the tab instead:
//!
//! - Working: a yellow fill grows left to right with completed todos, and a
//!   soft highlight sweeps across the tab so an unchanged count still reads
//!   as alive.
//! - Done: a green wash breathes gently until the session is viewed or a new
//!   turn starts.
//! - Error: a quiet red wash.
//! - Paused plan: a dim fill showing how far an interrupted plan got.
//!
//! Like the orb spinner, a painted fill arms at most one pending tick, so
//! clipped tabs stop animating on their own.
use std::time::{Duration, Instant};

use gpui::{
    Bounds, ContentMask, Context, Pixels, Render, Rgba, Task, Window, canvas, div,
    linear_color_stop, linear_gradient, point, prelude::*, px, size,
};

use crate::theme::Theme;

use super::MinimapSessionState;

/// 60 Hz on the shared animation grid, so every animated tab wakes in the
/// same frame instead of each re-rendering the header on its own.
const TICK: Duration = Duration::from_nanos(16_666_667);
/// Matches `rounded_md` on the tab so the fill hugs the real outline.
const RADIUS: f32 = 6.0;
/// Seconds for the working highlight to cross the tab once.
const SWEEP_SECONDS: f32 = 1.8;
/// Seconds for one breath of the done glow.
const BREATH_SECONDS: f32 = 2.6;
/// Time constant for progress changes. A completed todo grows the fill
/// instead of snapping it to its new width.
const PROGRESS_EASE: f32 = 0.18;

/// Seconds since a process-wide epoch. Every tab animates in lockstep, and
/// phase never depends on how many ticks were delivered.
fn clock() -> f32 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabMode {
    Working,
    /// Finished while out of view. Cleared once the session is seen.
    Done,
    Error,
    /// An interrupted plan, shown quietly.
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabRing {
    pub(crate) color: Rgba,
    /// Completed fraction of the latest todo list, if there is one.
    pub(crate) progress: Option<f32>,
    pub(crate) mode: TabMode,
}

impl TabRing {
    pub(crate) fn resolve(
        state: MinimapSessionState,
        todos: Option<(usize, usize)>,
    ) -> Option<Self> {
        let theme = Theme::global();
        let progress = todos
            .filter(|(_, total)| *total > 0)
            .map(|(done, total)| (done as f32 / total as f32).clamp(0.0, 1.0));
        let (color, mode) = match state {
            MinimapSessionState::Working | MinimapSessionState::Streaming => {
                (theme.WARN, TabMode::Working)
            }
            MinimapSessionState::Complete => (theme.OK, TabMode::Done),
            MinimapSessionState::Error => (theme.ERROR, TabMode::Error),
            MinimapSessionState::Idle if progress.is_some_and(|p| p < 1.0) => {
                (theme.TEXT_DIM, TabMode::Paused)
            }
            MinimapSessionState::Idle => return None,
        };
        Some(Self {
            color,
            progress: match mode {
                TabMode::Working | TabMode::Paused => progress,
                TabMode::Done | TabMode::Error => None,
            },
            mode,
        })
    }

    pub(crate) fn working(&self) -> bool {
        self.mode == TabMode::Working
    }

    fn animated(&self) -> bool {
        matches!(self.mode, TabMode::Working | TabMode::Done)
    }

    /// Faint background tint so state reads even on tabs too narrow to show
    /// much of the fill.
    pub(crate) fn tint(&self) -> Rgba {
        self.color.opacity(0.04)
    }
}

pub(super) struct TabOutline {
    panel: gpui::WeakEntity<super::Panel>,
    tick: Option<Task<()>>,
    /// Displayed fill, eased toward the todo progress target.
    shown_progress: Option<f32>,
    eased_at: Option<Instant>,
}

impl TabOutline {
    pub(super) fn new(panel: gpui::Entity<super::Panel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&panel, |_, _, cx| cx.notify()).detach();
        Self {
            panel: panel.downgrade(),
            tick: None,
            shown_progress: None,
            eased_at: None,
        }
    }

    /// Advance the displayed fill toward `target`. Returns whether it is
    /// still moving and therefore needs another frame.
    fn ease_progress(&mut self, target: Option<f32>, reduce_motion: bool) -> bool {
        let now = Instant::now();
        let dt = self
            .eased_at
            .replace(now)
            .map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f32())
            .min(0.1);
        match (target, self.shown_progress) {
            (Some(target), Some(shown)) if !reduce_motion => {
                let k = 1.0 - (-dt / PROGRESS_EASE).exp();
                let next = shown + (target - shown) * k;
                let settled = (target - next).abs() < 0.002;
                self.shown_progress = Some(if settled { target } else { next });
                !settled
            }
            // A fresh list grows in from empty rather than appearing full.
            (Some(target), None) if !reduce_motion && target > 0.0 => {
                self.shown_progress = Some(0.0);
                true
            }
            (target, _) => {
                self.shown_progress = target;
                false
            }
        }
    }

    fn arm(&mut self, animate: bool, cx: &mut Context<Self>) {
        if !animate {
            self.tick = None;
            return;
        }
        if self.tick.is_some() {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            crate::animation_clock::next_tick(cx.background_executor(), TICK).await;
            let _ = this.update(cx, |outline, cx| {
                outline.tick = None;
                cx.notify();
            });
        }));
    }
}

fn clipped(window: &mut Window, clip: Bounds<Pixels>, f: impl FnOnce(&mut Window)) {
    let clip = clip.intersect(&window.content_mask().bounds);
    if clip.size.width <= px(0.) || clip.size.height <= px(0.) {
        return;
    }
    window.with_content_mask(Some(ContentMask { bounds: clip }), f);
}

/// A soft vertical band centred at `center`, fading to nothing on both sides.
fn band(window: &mut Window, bounds: Bounds<Pixels>, center: f32, width: f32, peak: Rgba) {
    let clear = Rgba { a: 0., ..peak };
    let top = bounds.origin.y;
    let height = bounds.size.height;
    let half = width / 2.0;
    for (left, from, to) in [(center - half, clear, peak), (center, peak, clear)] {
        window.paint_quad(gpui::fill(
            Bounds::new(point(px(left), top), size(px(half), height)),
            linear_gradient(90., linear_color_stop(from, 0.), linear_color_stop(to, 1.)),
        ));
    }
}

fn paint(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    ring: TabRing,
    progress: Option<f32>,
    now: f32,
    reduce_motion: bool,
) {
    let radius = RADIUS.min(f32::from(bounds.size.height) / 2.0);
    let rounded = |color: Rgba| gpui::fill(bounds, color).corner_radii(px(radius));
    let outline = |color: Rgba| {
        gpui::quad(
            bounds,
            px(radius),
            gpui::transparent_black(),
            px(1.),
            color,
            gpui::BorderStyle::Solid,
        )
    };
    let left = f32::from(bounds.origin.x);
    let width = f32::from(bounds.size.width);
    match ring.mode {
        TabMode::Working => {
            if let Some(progress) = progress.filter(|p| *p > 0.0) {
                let filled = width * progress;
                let fill = Bounds::new(bounds.origin, size(px(filled), bounds.size.height));
                clipped(window, fill, |window| {
                    window.paint_quad(rounded(ring.color.opacity(0.20)));
                });
                // Brighter leading edge so the fill reads as a bar, not a tint.
                if progress < 1.0 {
                    band(
                        window,
                        bounds,
                        left + filled - 3.0,
                        6.0,
                        ring.color.opacity(0.28),
                    );
                }
            }
            if !reduce_motion {
                // The highlight enters from the left edge and leaves past the
                // right, then rests briefly before crossing again.
                let band_width = (width * 0.45).clamp(24.0, 120.0);
                let t = (now / SWEEP_SECONDS).fract();
                let travel = width + band_width * 2.0;
                let center = left - band_width + travel * (t * 1.25).min(1.0);
                clipped(window, bounds, |window| {
                    band(window, bounds, center, band_width, ring.color.opacity(0.22));
                });
            }
            window.paint_quad(outline(ring.color.opacity(0.55)));
        }
        TabMode::Done => {
            let breath = if reduce_motion {
                0.5
            } else {
                0.5 - 0.5 * (now / BREATH_SECONDS * std::f32::consts::TAU).cos()
            };
            window.paint_quad(rounded(ring.color.opacity(0.10 + 0.12 * breath)));
            window.paint_quad(outline(ring.color.opacity(0.45 + 0.45 * breath)));
        }
        TabMode::Error => {
            window.paint_quad(rounded(ring.color.opacity(0.12)));
            window.paint_quad(outline(ring.color.opacity(0.70)));
        }
        TabMode::Paused => {
            if let Some(progress) = progress.filter(|p| *p > 0.0) {
                let fill = Bounds::new(
                    bounds.origin,
                    size(px(width * progress), bounds.size.height),
                );
                clipped(window, fill, |window| {
                    window.paint_quad(rounded(ring.color.opacity(0.12)));
                });
            }
            window.paint_quad(outline(ring.color.opacity(0.40)));
        }
    }
}

impl Render for TabOutline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _render_scope = crate::render_stats::scope("TabOutline");
        let ring = self
            .panel
            .upgrade()
            .and_then(|panel| panel.read(cx).tab_ring());
        let outline = cx.entity().downgrade();
        div().size_full().when_some(ring, |el, ring| {
            el.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if !bounds.intersects(&window.content_mask().bounds) {
                            return;
                        }
                        let reduce_motion =
                            cx.reduce_motion() || crate::config::get().appearance.reduce_motion;
                        let Some(progress) = outline
                            .update(cx, |outline, cx| {
                                let easing = outline.ease_progress(ring.progress, reduce_motion);
                                outline.arm((ring.animated() && !reduce_motion) || easing, cx);
                                outline.shown_progress
                            })
                            .ok()
                        else {
                            return;
                        };
                        window.paint_layer(bounds, |window| {
                            paint(window, bounds, ring, progress, clock(), reduce_motion);
                        });
                    },
                )
                .size_full(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_color_codes_state_and_tracks_progress() {
        assert!(TabRing::resolve(MinimapSessionState::Idle, None).is_none());
        assert!(TabRing::resolve(MinimapSessionState::Idle, Some((2, 2))).is_none());
        let paused = TabRing::resolve(MinimapSessionState::Idle, Some((1, 4))).unwrap();
        assert_eq!(paused.progress, Some(0.25));
        assert_eq!(paused.mode, TabMode::Paused);
        let working = TabRing::resolve(MinimapSessionState::Working, Some((3, 4))).unwrap();
        assert_eq!(working.progress, Some(0.75));
        assert!(working.working());
        let sweep = TabRing::resolve(MinimapSessionState::Streaming, None).unwrap();
        assert_eq!(sweep.progress, None);
        assert!(sweep.working());
        let done = TabRing::resolve(MinimapSessionState::Complete, Some((4, 4))).unwrap();
        assert_eq!(done.mode, TabMode::Done);
        assert!(done.animated());
        let error = TabRing::resolve(MinimapSessionState::Error, None).unwrap();
        assert_eq!(error.mode, TabMode::Error);
        assert!(!error.animated());
    }

    #[test]
    fn fresh_todo_progress_grows_in_from_empty() {
        let mut outline = TabOutline {
            panel: gpui::WeakEntity::new_invalid(),
            tick: None,
            shown_progress: None,
            eased_at: None,
        };
        assert!(outline.ease_progress(Some(0.5), false));
        assert_eq!(outline.shown_progress, Some(0.0));
        outline.eased_at = Some(Instant::now() - Duration::from_secs(1));
        outline.ease_progress(Some(0.5), false);
        assert!(outline.shown_progress.unwrap() > 0.0);
        assert!(!outline.ease_progress(Some(0.5), true));
        assert_eq!(outline.shown_progress, Some(0.5));
    }
}
