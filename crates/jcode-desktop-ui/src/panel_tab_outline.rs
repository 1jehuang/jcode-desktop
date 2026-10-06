//! The tab outline doubles as a state ring and a todo progress bar.
//!
//! The session emoji never moves. Activity is shown by the outline instead:
//! the ring fills clockwise from the top center with completed todos, is
//! color-coded by session state, and a short comet circles it while the
//! session works without a todo list. Like the orb spinner, a painted outline
//! arms at most one pending tick, so clipped tabs stop animating on their own.
use std::time::{Duration, Instant};

use gpui::{
    Context, Pixels, Point, Render, Rgba, Task, Window, canvas, div, point, prelude::*, px,
};

use crate::theme::Theme;

use super::MinimapSessionState;

/// 60 Hz on the shared animation grid, so every ringed tab wakes in the
/// same frame instead of each re-rendering the header on its own.
const TICK: Duration = Duration::from_nanos(16_666_667);
/// Matches `rounded_md` on the tab so the ring hugs the real outline.
const RADIUS: f32 = 6.0;
const STROKE: f32 = 1.5;
const ORBIT_SECONDS: f32 = 2.4;
const COMET_FRACTION: f32 = 0.30;
/// Segments along the comet tail. Enough that the alpha taper reads as a
/// continuous gradient rather than visible steps.
const COMET_SEGMENTS: usize = 28;
/// Time constant for progress changes. A completed todo sweeps forward
/// instead of snapping the fill to its new length.
const PROGRESS_EASE: f32 = 0.18;

/// Orbit phase from one process-wide clock. Every working tab circles in
/// lockstep, and phase never depends on how many ticks were delivered.
fn orbit_phase() -> f32 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    (epoch.elapsed().as_secs_f32() / ORBIT_SECONDS).fract()
}

/// Smooth ease-in-out so the comet glides through corners instead of moving
/// at a mechanical constant rate.
fn ease_orbit(t: f32) -> f32 {
    // Mostly linear with a gentle sine modulation: continuous speed at the
    // wrap point, never stopping, but visibly softer than constant motion.
    t - 0.035 * (t * std::f32::consts::TAU).sin()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabRing {
    pub(crate) color: Rgba,
    /// Completed fraction of the latest todo list, if there is one.
    pub(crate) progress: Option<f32>,
    pub(crate) working: bool,
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
    /// Displayed fill, eased toward the todo progress target.
    shown_progress: Option<f32>,
    eased_at: Option<Instant>,
}

impl TabOutline {
    pub(super) fn new(panel: gpui::Entity<super::Panel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&panel, |_, _, cx| cx.notify()).detach();
        Self {
            panel: panel.downgrade(),
            elapsed: Duration::ZERO,
            lease_started: None,
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
            (target, _) => {
                self.shown_progress = target;
                false
            }
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
            crate::animation_clock::next_tick(cx.background_executor(), TICK).await;
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
        self.stroke_width(from, to, STROKE, window, color);
    }

    fn stroke_width(&self, from: f32, to: f32, width: f32, window: &mut Window, color: Rgba) {
        let span = to - from;
        if span <= 0.05 {
            return;
        }
        // Tessellating the full-length track and the progress fill is the
        // costly part of painting the ring, and both repeat unchanged from
        // frame to frame while the comet orbits. Tessellate them once, at
        // the origin, and move them into place. Comet segments are a few
        // pixels long and at a new phase every frame, so they are built
        // fresh.
        if span < CACHED_STROKE_MIN {
            if let Some(path) = self.ribbon(from, to, width) {
                window.paint_path(path, color);
            }
            return;
        }
        let key = StrokeKey::new(self, from, to, width);
        let path = STROKES.with_borrow_mut(|cache| {
            if let Some(path) = cache.get(&key) {
                return path.clone();
            }
            let origin = Perimeter {
                origin: point(px(0.), px(0.)),
                ..*self
            };
            let path = origin.tessellate(from, to, width);
            if cache.len() >= STROKE_CACHE_LIMIT {
                cache.clear();
            }
            cache.insert(key, path.clone());
            path
        });
        if let Some(path) = path {
            window.paint_path(translated(path, self.origin), color);
        }
    }

    fn tessellate(&self, from: f32, to: f32, width: f32) -> Option<gpui::Path<Pixels>> {
        let span = to - from;
        // About one vertex per pixel keeps the rounded corners smooth.
        let steps = (span.ceil() as usize).clamp(2, 800);
        let mut path = gpui::PathBuilder::stroke(px(width));
        for step in 0..=steps {
            let p = self.at(from + span * step as f32 / steps as f32);
            if step == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        path.build().ok()
    }

    /// A short stroke as a strip of quads along the perimeter. Comet segments
    /// are a few pixels long, two dozen of them are painted every frame, and
    /// running lyon's stroker for each was most of the outline's cost. A
    /// butt-capped ribbon is what the stroker produced for them anyway.
    fn ribbon(&self, from: f32, to: f32, width: f32) -> Option<gpui::Path<Pixels>> {
        let span = to - from;
        let steps = (span.ceil() as usize).clamp(1, 64);
        let half = width / 2.0;
        let at = |s: f32| {
            let p = self.at(s);
            (f32::from(p.x), f32::from(p.y))
        };
        // Offsets either side of the centre line, from the local direction.
        let edge = |s: f32| {
            let (x, y) = at(s);
            let (ax, ay) = at(s - 0.25);
            let (bx, by) = at(s + 0.25);
            let (dx, dy) = (bx - ax, by - ay);
            let length = (dx * dx + dy * dy).sqrt();
            if length <= f32::EPSILON {
                return None;
            }
            let (nx, ny) = (-dy / length * half, dx / length * half);
            Some((point(px(x + nx), px(y + ny)), point(px(x - nx), px(y - ny))))
        };
        let st = (point(0., 1.), point(0., 1.), point(0., 1.));
        let mut previous = edge(from)?;
        let mut path = gpui::Path::new(previous.0);
        for step in 1..=steps {
            let Some(next) = edge(from + span * step as f32 / steps as f32) else {
                continue;
            };
            path.push_triangle((previous.0, previous.1, next.0), st);
            path.push_triangle((previous.1, next.1, next.0), st);
            previous = next;
        }
        (!path.vertices.is_empty()).then_some(path)
    }
}

/// Strokes shorter than this are cheap to build and rarely repeat.
const CACHED_STROKE_MIN: f32 = 24.0;
/// Long strokes kept per thread: a track and a fill per tab size and
/// progress step.
const STROKE_CACHE_LIMIT: usize = 256;

fn translated(mut path: gpui::Path<Pixels>, by: Point<Pixels>) -> gpui::Path<Pixels> {
    path.bounds.origin = path.bounds.origin + by;
    for vertex in &mut path.vertices {
        vertex.xy_position = vertex.xy_position + by;
    }
    path
}

thread_local! {
    static STROKES: std::cell::RefCell<std::collections::HashMap<StrokeKey, Option<gpui::Path<Pixels>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// A stroke along a perimeter, in hundredths of a pixel.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StrokeKey([i32; 6]);

impl StrokeKey {
    fn new(perimeter: &Perimeter, from: f32, to: f32, width: f32) -> Self {
        let q = |v: f32| (v * 100.0).round() as i32;
        Self([
            q(perimeter.w),
            q(perimeter.h),
            q(perimeter.r),
            q(from.rem_euclid(perimeter.len())),
            q(to - from),
            q(width),
        ])
    }
}

/// A tapered streak ending at `head`: alpha and width fall off smoothly along
/// the tail, with a faint wider glow under the bright end.
fn comet(
    perimeter: &Perimeter,
    head: f32,
    tail: f32,
    color: Rgba,
    strength: f32,
    window: &mut Window,
) {
    let seg = tail / COMET_SEGMENTS as f32;
    for i in 0..COMET_SEGMENTS {
        // t runs 0 at the tail tip to 1 at the head.
        let t = (i + 1) as f32 / COMET_SEGMENTS as f32;
        let ease = t * t * (3.0 - 2.0 * t);
        let from = head - tail + seg * i as f32;
        // Overlap a hair so the joins between segments never show seams.
        let to = from + seg + 0.35;
        if t > 0.55 {
            perimeter.stroke_width(
                from,
                to,
                STROKE + 2.5 * ease,
                window,
                color.opacity(0.10 * ease * strength),
            );
        }
        perimeter.stroke_width(
            from,
            to,
            STROKE * (0.6 + 0.4 * ease) + 0.5 * ease,
            window,
            color.opacity(ease * strength),
        );
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
                                outline.arm((ring.working || easing) && !reduce_motion, cx);
                                outline.shown_progress
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
                        let phase = if reduce_motion { 0.0 } else { orbit_phase() };
                        let tau = std::f32::consts::TAU;
                        window.paint_layer(bounds, |window| {
                            // Quiet track, so the ring reads as one outline.
                            perimeter.stroke(0.0, len, window, ring.color.opacity(0.22));
                            match progress {
                                Some(progress) => {
                                    let end = len * progress;
                                    perimeter.stroke(0.0, end, window, ring.color);
                                    if ring.working && progress < 1.0 {
                                        // A soft glow breathes at the leading
                                        // edge so an unchanged count still
                                        // reads as alive, without moving.
                                        let pulse = 0.5 - 0.5 * (phase * tau).cos();
                                        let head = (len * 0.10).min(len - end);
                                        comet(
                                            &perimeter,
                                            end + head,
                                            head,
                                            ring.color,
                                            0.30 + 0.50 * pulse,
                                            window,
                                        );
                                    }
                                }
                                None if ring.working => {
                                    let head = len * ease_orbit(phase);
                                    comet(
                                        &perimeter,
                                        head,
                                        len * COMET_FRACTION,
                                        ring.color,
                                        1.0,
                                        window,
                                    );
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
    fn translated_stroke_matches_one_built_in_place() {
        let at = |x: f32, y: f32| Perimeter::new(point(px(x), px(y)), 120.0, 28.0, 0.75).unwrap();
        let placed = at(37.5, 212.25);
        let direct = placed.tessellate(0.0, placed.len(), STROKE).unwrap();
        let moved = translated(
            at(0.0, 0.0).tessellate(0.0, placed.len(), STROKE).unwrap(),
            point(px(37.5), px(212.25)),
        );
        assert_eq!(direct.bounds, moved.bounds);
        assert_eq!(direct.vertices.len(), moved.vertices.len());
        for (a, b) in direct.vertices.iter().zip(&moved.vertices) {
            assert!((f32::from(a.xy_position.x) - f32::from(b.xy_position.x)).abs() < 0.01);
            assert!((f32::from(a.xy_position.y) - f32::from(b.xy_position.y)).abs() < 0.01);
        }
    }

    #[test]
    fn comet_ribbons_cover_what_the_stroker_covered() {
        let p = Perimeter::new(point(px(10.0), px(20.0)), 120.0, 28.0, 0.75).unwrap();
        let area = |path: &gpui::Path<Pixels>| -> f32 {
            path.vertices
                .chunks_exact(3)
                .map(|t| {
                    let (a, b, c) = (t[0].xy_position, t[1].xy_position, t[2].xy_position);
                    let (ax, ay) = (f32::from(a.x), f32::from(a.y));
                    let (bx, by) = (f32::from(b.x), f32::from(b.y));
                    let (cx, cy) = (f32::from(c.x), f32::from(c.y));
                    ((bx - ax) * (cy - ay) - (cx - ax) * (by - ay)).abs() / 2.0
                })
                .sum()
        };
        // Straight runs, a corner, and the wrap through the top centre.
        for (from, span, width) in [(3.0, 4.0, 1.5), (55.0, 9.0, 3.0), (p.len() - 2.0, 5.0, 2.0)] {
            let lyon = p.tessellate(from, from + span, width).unwrap();
            let ribbon = p.ribbon(from, from + span, width).unwrap();
            let (expected, actual) = (area(&lyon), area(&ribbon));
            assert!(
                (expected - actual).abs() <= expected * 0.08,
                "stroke {from}+{span} w{width}: lyon {expected}, ribbon {actual}"
            );
            let grow = |b: gpui::Bounds<Pixels>| b.dilate(px(0.6));
            assert!(grow(lyon.bounds).contains(&ribbon.bounds.origin), "{from}");
            assert!(grow(ribbon.bounds).contains(&lyon.bounds.origin), "{from}");
        }
    }

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
