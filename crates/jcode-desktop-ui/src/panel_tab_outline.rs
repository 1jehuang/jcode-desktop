//! The tab outline doubles as a state ring and a todo progress bar.
//!
//! The session emoji never moves. Activity is shown by the outline instead:
//! the ring fills clockwise from the top center with completed todos, is
//! color-coded by session state, and a short comet circles it while the
//! session works without a todo list. Like the orb spinner, a painted outline
//! arms at most one pending tick, so clipped tabs stop animating on their own.
use std::time::{Duration, Instant};

use gpui::{Context, Pixels, Point, Render, Rgba, Task, Window, canvas, div, point, prelude::*, px};

use crate::theme::Theme;

use super::MinimapSessionState;

const TICK: Duration = Duration::from_nanos(33_333_334);
/// Matches `rounded_md` on the tab so the ring hugs the real outline.
const RADIUS: f32 = 6.0;
const STROKE: f32 = 1.5;
const ORBIT_SECONDS: f32 = 2.2;
const COMET_FRACTION: f32 = 0.22;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabRing {
    pub(crate) color: Rgba,
    /// Completed fraction of the latest todo list, if there is one.
    pub(crate) progress: Option<f32>,
    pub(crate) working: bool,
}

impl TabRing {
    pub(crate) fn resolve(state: MinimapSessionState, todos: Option<(usize, usize)>) -> Option<Self> {
        let theme = Theme::global();
        let progress = todos
            .filter(|(_, total)| *total > 0)
            .map(|(done, total)| (done as f32 / total as f32).clamp(0.0, 1.0));
        let (color, working) = match state {
            MinimapSessionState::Working | MinimapSessionState::Streaming => (theme.ACCENT, true),
            MinimapSessionState::Complete => (theme.OK, false),
            MinimapSessionState::Error => (theme.ERROR, false),
            // An interrupted plan still shows how far it got, but quietly.
            MinimapSessionState::Idle if progress.is_some_and(|p| p < 1.0) => {
                (theme.TEXT_DIM, false)
            }
            MinimapSessionState::Idle => return None,
        };
        Some(Self {
            color,
            progress: if state == MinimapSessionState::Complete {
                Some(1.0)
            } else {
                progress
            },
            working,
        })
    }

    /// Subtle background tint so state reads even on narrow, crowded tabs.
    pub(crate) fn tint(&self) -> Rgba {
        self.color.opacity(if self.working { 0.10 } else { 0.07 })
    }
}

pub(super) struct TabOutline {
    panel: gpui::WeakEntity<super::Panel>,
    elapsed: Duration,
    lease_started: Option<Instant>,
    tick: Option<Task<()>>,
}

impl TabOutline {
    pub(super) fn new(panel: gpui::Entity<super::Panel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&panel, |_, _, cx| cx.notify()).detach();
        Self {
            panel: panel.downgrade(),
            elapsed: Duration::ZERO,
            lease_started: None,
            tick: None,
        }
    }

    fn finish_lease(&mut self, now: Instant) {
        if let Some(started) = self.lease_started.take() {
            self.elapsed += now.saturating_duration_since(started);
        }
    }

    fn arm(&mut self, animate: bool, cx: &mut Context<Self>) {
        if !animate {
            self.finish_lease(Instant::now());
            self.tick = None;
            return;
        }
        if self.tick.is_some() {
            return;
        }
        self.lease_started = Some(Instant::now());
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TICK).await;
            let _ = this.update(cx, |outline, cx| {
                outline.finish_lease(Instant::now());
                outline.tick = None;
                cx.notify();
            });
        }));
    }
}

/// Rounded-rectangle perimeter, parameterized by arc length clockwise from
/// the top center. `inset` is the distance of the stroke center from the edge.
struct Perimeter {
    origin: Point<Pixels>,
    w: f32,
    h: f32,
    r: f32,
}

impl Perimeter {
    fn new(origin: Point<Pixels>, w: f32, h: f32, inset: f32) -> Option<Self> {
        let w = w - 2.0 * inset;
        let h = h - 2.0 * inset;
        if w <= 1.0 || h <= 1.0 {
            return None;
        }
        Some(Self {
            origin: origin + point(px(inset), px(inset)),
            w,
            h,
            r: (RADIUS - inset).clamp(0.0, w.min(h) / 2.0),
        })
    }

    fn straight(&self) -> (f32, f32) {
        (self.w - 2.0 * self.r, self.h - 2.0 * self.r)
    }

    fn len(&self) -> f32 {
        let (sw, sh) = self.straight();
        2.0 * (sw + sh) + std::f32::consts::TAU * self.r
    }

    fn at(&self, s: f32) -> Point<Pixels> {
        let (sw, sh) = self.straight();
        let arc = std::f32::consts::FRAC_PI_2 * self.r;
        let (w, h, r) = (self.w, self.h, self.r);
        let mut s = s.rem_euclid(self.len());
        let corner = |cx: f32, cy: f32, start: f32, s: f32| {
            let a = start + if r > 0.0 { s / r } else { 0.0 };
            (cx + r * a.cos(), cy + r * a.sin())
        };
        let half = sw / 2.0;
        let (x, y) = 'found: {
            // Top edge, center to right.
            if s <= half {
                break 'found (w / 2.0 + s, 0.0);
            }
            s -= half;
            if s <= arc {
                break 'found corner(w - r, r, -std::f32::consts::FRAC_PI_2, s);
            }
            s -= arc;
            if s <= sh {
                break 'found (w, r + s);
            }
            s -= sh;
            if s <= arc {
                break 'found corner(w - r, h - r, 0.0, s);
            }
            s -= arc;
            if s <= sw {
                break 'found (w - r - s, h);
            }
            s -= sw;
            if s <= arc {
                break 'found corner(r, h - r, std::f32::consts::FRAC_PI_2, s);
            }
            s -= arc;
            if s <= sh {
                break 'found (0.0, h - r - s);
            }
            s -= sh;
            if s <= arc {
                break 'found corner(r, r, std::f32::consts::PI, s);
            }
            s -= arc;
            (r + s.min(half), 0.0)
        };
        self.origin + point(px(x), px(y))
    }

    fn stroke(&self, from: f32, to: f32, window: &mut Window, color: Rgba) {
        let span = to - from;
        if span <= 0.05 {
            return;
        }
        let steps = ((span / 2.0).ceil() as usize).clamp(2, 400);
        let mut path = gpui::PathBuilder::stroke(px(STROKE));
        for step in 0..=steps {
            let p = self.at(from + span * step as f32 / steps as f32);
            if step == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

impl Render for TabOutline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ring = self.panel.upgrade().and_then(|panel| panel.read(cx).tab_ring());
        let outline = cx.entity().downgrade();
        div().size_full().when_some(ring, |el, ring| {
            el.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if !bounds.intersects(&window.content_mask().bounds) {
                            return;
                        }
                        let reduce_motion = cx.reduce_motion()
                            || crate::config::get().appearance.reduce_motion;
                        let Some(elapsed) = outline
                            .update(cx, |outline, cx| {
                                outline.arm(ring.working && !reduce_motion, cx);
                                outline.elapsed
                            })
                            .ok()
                        else {
                            return;
                        };
                        let Some(perimeter) = Perimeter::new(
                            bounds.origin,
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            STROKE / 2.0,
                        ) else {
                            return;
                        };
                        let len = perimeter.len();
                        let phase = if reduce_motion {
                            0.0
                        } else {
                            (elapsed.as_secs_f32() / ORBIT_SECONDS).fract()
                        };
                        window.paint_layer(bounds, |window| {
                            perimeter.stroke(0.0, len, window, ring.color.opacity(0.28));
                            match ring.progress {
                                Some(progress) => {
                                    let end = len * progress;
                                    perimeter.stroke(0.0, end, window, ring.color);
                                    if ring.working && progress < 1.0 {
                                        // Breathe a short leading edge so an
                                        // unchanged count still reads as alive.
                                        let pulse = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
                                        let head = (len * 0.08).min(len - end);
                                        perimeter.stroke(
                                            end,
                                            end + head,
                                            window,
                                            ring.color.opacity(0.25 + 0.55 * pulse),
                                        );
                                    }
                                }
                                None if ring.working => {
                                    let head = len * phase;
                                    let tail = len * COMET_FRACTION;
                                    let segments = 6;
                                    for i in 0..segments {
                                        let a = i as f32 / segments as f32;
                                        let b = (i + 1) as f32 / segments as f32;
                                        perimeter.stroke(
                                            head - tail * (1.0 - a),
                                            head - tail * (1.0 - b),
                                            window,
                                            ring.color.opacity(0.15 + 0.85 * b),
                                        );
                                    }
                                }
                                None => perimeter.stroke(0.0, len, window, ring.color),
                            }
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
    fn perimeter_starts_at_top_center_and_runs_clockwise() {
        let p = Perimeter::new(point(px(0.0), px(0.0)), 100.0, 30.0, 0.0).unwrap();
        let start = p.at(0.0);
        assert!((f32::from(start.x) - 50.0).abs() < 0.01);
        assert!(f32::from(start.y).abs() < 0.01);
        // A quarter of the way should be on the right edge, half on the bottom.
        assert!(f32::from(p.at(p.len() * 0.25).x) > 90.0);
        let half = p.at(p.len() * 0.5);
        assert!((f32::from(half.x) - 50.0).abs() < 0.5);
        assert!((f32::from(half.y) - 30.0).abs() < 0.01);
        let end = p.at(p.len() - 0.001);
        assert!((f32::from(end.x) - 50.0).abs() < 0.1);
    }

    #[test]
    fn ring_color_codes_state_and_tracks_progress() {
        // Other tests may switch the global theme concurrently, so compare
        // roles against each other rather than against a sampled palette.
        assert!(TabRing::resolve(MinimapSessionState::Idle, None).is_none());
        assert!(TabRing::resolve(MinimapSessionState::Idle, Some((2, 2))).is_none());
        let paused = TabRing::resolve(MinimapSessionState::Idle, Some((1, 4))).unwrap();
        assert_eq!(paused.progress, Some(0.25));
        assert!(!paused.working);
        let working = TabRing::resolve(MinimapSessionState::Working, Some((3, 4))).unwrap();
        assert_eq!(working.progress, Some(0.75));
        assert!(working.working);
        let comet = TabRing::resolve(MinimapSessionState::Streaming, None).unwrap();
        assert_eq!(comet.progress, None);
        assert!(comet.working);
        let done = TabRing::resolve(MinimapSessionState::Complete, Some((4, 4))).unwrap();
        assert_eq!(done.progress, Some(1.0));
        assert!(!done.working);
        let error = TabRing::resolve(MinimapSessionState::Error, None).unwrap();
        assert!(!error.working);
        assert_eq!(error.progress, None);
    }
}
