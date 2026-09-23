//! Activity-selected upstream inline orbs, with a paint-only animation lease.
//! A timer invalidates geometry once, never rearms itself. Clipped/hidden views
//! therefore stop after at most one pending tick without visibility bookkeeping.

use std::time::{Duration, Instant};

use gpui::{Context, Render, Task, Window, canvas, div, prelude::*, px};
use gpui_thinking_orbs::{Frame, OrbSize, OrbState, Resolved, draw_mode_into, resolve_preset};

use crate::theme::Theme;

#[path = "activity_donut.rs"]
mod donut;

const MORPH_DURATION: Duration = Duration::from_millis(350);
const TICK: Duration = Duration::from_nanos(33_333_334);
const SIZE: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Activity {
    Idle,
    Working,
    Thinking,
    Responding,
    RunningTools,
}

impl Activity {
    pub(super) fn from_status_line(status: &str) -> Self {
        match status {
            "Thinking" => Self::Thinking,
            "Responding" => Self::Responding,
            "Running tools" => Self::RunningTools,
            _ => Self::Working,
        }
    }

    fn preset(self) -> Resolved {
        resolve_preset(
            match self {
                Self::Idle | Self::Working => OrbState::Working,
                Self::Thinking => OrbState::Reasoning,
                Self::Responding => OrbState::Composing,
                Self::RunningTools => OrbState::Solving,
            },
            OrbSize::Inline,
        )
    }
}

pub(super) struct Spinner {
    activity: Activity,
    sidebar_mark: bool,
    panel: Option<gpui::WeakEntity<super::Panel>>,
    elapsed: Duration,
    motion_elapsed: Duration,
    lease_started: Option<Instant>,
    resolved: Resolved,
    frame: Frame,
    target_frame: Frame,
    morph_from: Frame,
    morph_started: Option<Duration>,
    geometry_dirty: bool,
    reduced_motion: Option<bool>,
    tick: Option<Task<()>>,
}

impl Spinner {
    pub(super) fn new(_: &mut Context<Self>) -> Self {
        Self {
            activity: Activity::Idle,
            sidebar_mark: false,
            panel: None,
            elapsed: Duration::ZERO,
            motion_elapsed: Duration::ZERO,
            lease_started: None,
            resolved: resolve_preset(OrbState::Working, OrbSize::Inline),
            frame: Frame::new(),
            target_frame: Frame::new(),
            morph_from: Frame::new(),
            morph_started: None,
            geometry_dirty: true,
            reduced_motion: None,
            tick: None,
        }
    }

    pub(super) fn for_panel(panel: gpui::Entity<super::Panel>, cx: &mut Context<Self>) -> Self {
        // Observe the owner, not just its render: the sidebar may be visible
        // while the conversation panel is clipped or not rendered at all.
        cx.observe(&panel, |spinner, panel, cx| {
            let activity = panel.read(cx).orb_activity();
            spinner.set_activity(activity, cx);
        })
        .detach();
        let mut spinner = Self::new(cx);
        spinner.panel = Some(panel.downgrade());
        spinner
    }

    pub(super) fn for_sidebar(panel: gpui::Entity<super::Panel>, cx: &mut Context<Self>) -> Self {
        let mut spinner = Self::for_panel(panel, cx);
        spinner.sidebar_mark = true;
        spinner
    }

    pub(super) fn set_activity(&mut self, activity: Activity, cx: &mut Context<Self>) {
        if self.activity == activity {
            return;
        }
        if self.frame.dots.is_empty() {
            self.prepare(self.reduced_motion.unwrap_or(false));
        }
        self.morph_from.dots.clone_from(&self.frame.dots);
        self.morph_from.lines.clone_from(&self.frame.lines);
        self.morph_started = Some(self.elapsed);
        self.activity = activity;
        self.resolved = activity.preset();
        // Keep the retained frame and the existing paint lease. Changing
        // activity invalidates geometry but never starts a hidden timer.
        self.geometry_dirty = true;
        cx.notify();
    }

    fn prepare(&mut self, reduce_motion: bool) {
        if self.geometry_dirty || self.reduced_motion != Some(reduce_motion) {
            if self.activity == Activity::Idle {
                donut::draw_donut_into(SIZE, 0.6, &mut self.target_frame);
            } else {
                draw_mode_into(
                    self.resolved.mode,
                    SIZE,
                    animation_time(self.motion_elapsed, self.resolved.speed, reduce_motion),
                    &self.resolved.opts,
                    &mut self.target_frame,
                );
            }
            let progress = self.morph_started.map(|started| {
                self.elapsed.saturating_sub(started).as_secs_f32() / MORPH_DURATION.as_secs_f32()
            });
            if !reduce_motion && progress.is_some_and(|p| p < 1.0) {
                donut::morph_into(
                    &self.morph_from,
                    &self.target_frame,
                    progress.unwrap(),
                    &mut self.frame,
                );
            } else {
                self.frame.dots.clone_from(&self.target_frame.dots);
                self.frame.lines.clone_from(&self.target_frame.lines);
                self.morph_started = None;
            }
            self.geometry_dirty = false;
            self.reduced_motion = Some(reduce_motion);
        }
    }

    fn finish_lease(&mut self, now: Instant) {
        if let Some(started) = self.lease_started.take() {
            // Actual monotonic time, not +TICK or a frame index. Idle/reduced
            // intervals have no lease and cannot age or jump the animation.
            let delta = now.saturating_duration_since(started);
            self.elapsed += delta;
            // Freeze the destination pose throughout a morph. Depth-sorted
            // particle identities must not change under interpolation.
            if self.morph_started.is_none() {
                self.motion_elapsed += delta;
            }
        }
    }

    fn arm(&mut self, reduce_motion: bool, cx: &mut Context<Self>) {
        if reduce_motion || (self.activity == Activity::Idle && self.morph_started.is_none()) {
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
            let _ = this.update(cx, |spinner, cx| {
                spinner.finish_lease(Instant::now());
                spinner.tick = None;
                spinner.geometry_dirty = true;
                cx.notify();
            });
        }));
    }
}

fn animation_time(elapsed: Duration, speed: f32, reduce_motion: bool) -> f32 {
    // Match upstream's representative pose, which is not multiplied by speed.
    // Never derive phase from delivered ticks: slow paints must not slow motion.
    if reduce_motion {
        0.6
    } else {
        (elapsed.as_secs_f64() * speed as f64) as f32
    }
}

// Pulse only the ink, never the label's content or layout. Share the orb's
// paint lease so clipped labels cannot keep the transcript animating.
fn label_intensity(activity: Activity, elapsed: Duration, reduce_motion: bool) -> f32 {
    if reduce_motion || activity == Activity::Idle {
        return 0.0;
    }
    let period = match activity {
        Activity::Thinking => 2.8,
        Activity::Responding => 1.6,
        Activity::RunningTools => 1.2,
        _ => 2.2,
    };
    let phase = (elapsed.as_secs_f64() % period) / period;
    (0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()) as f32 * 0.45
}

fn ink_color(white: f32, alpha: f32, theme: &Theme) -> gpui::Rgba {
    let w = white.clamp(0.0, 1.0);
    let ink = theme.TEXT;
    let paper = theme.PANEL_BG;
    gpui::Rgba {
        r: ink.r + (paper.r - ink.r) * w,
        g: ink.g + (paper.g - ink.g) * w,
        b: ink.b + (paper.b - ink.b) * w,
        a: alpha.clamp(0.0, 1.0),
    }
}

// Geometry stays upstream. Theme ink is mapped above, and the native circle
// adapter replaces upstream's rounded quads.
// paint_quad snaps bounds to device pixels, visibly jittering these small dots.
// Keep fractional centers/radii, exact per-dot ink/alpha and back-to-front order.
// Do NOT merge contours: that changes GPUI's overlap compositing.
fn dot_path(
    dot: &gpui_thinking_orbs::Dot,
    r_min: f32,
    origin: gpui::Point<gpui::Pixels>,
) -> gpui::Path<gpui::Pixels> {
    let mut builder = gpui::PathBuilder::fill().with_style(gpui::PathStyle::Fill(
        gpui::FillOptions::default().with_tolerance(0.005),
    ));
    let r = dot.r.max(r_min);
    let k = r * 0.552_284_8;
    let p = |x: f32, y: f32| origin + gpui::point(px(dot.x + x), px(dot.y + y));
    builder.move_to(p(r, 0.0));
    builder.cubic_bezier_to(p(0.0, r), p(r, k), p(k, r));
    builder.cubic_bezier_to(p(-r, 0.0), p(-k, r), p(-r, k));
    builder.cubic_bezier_to(p(0.0, -r), p(-r, -k), p(-k, -r));
    builder.cubic_bezier_to(p(r, 0.0), p(k, -r), p(r, -k));
    builder.close();
    builder
        .build()
        .expect("bounded upstream orb circle tessellates")
}

impl Render for Spinner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut label = None;
        if let Some(panel) = self.panel.as_ref().and_then(|panel| panel.upgrade()) {
            let activity = panel.read(cx).orb_activity();
            if !self.sidebar_mark {
                label = Some(panel.read(cx).status_line());
            }
            self.set_activity(activity, cx);
        }
        let theme = Theme::global();
        let intensity = label_intensity(
            self.activity,
            self.elapsed,
            crate::config::get().appearance.reduce_motion,
        );
        let label_color = gpui::Rgba {
            r: theme.TEXT_DIM.r + (theme.TEXT.r - theme.TEXT_DIM.r) * intensity,
            g: theme.TEXT_DIM.g + (theme.TEXT.g - theme.TEXT_DIM.g) * intensity,
            b: theme.TEXT_DIM.b + (theme.TEXT.b - theme.TEXT_DIM.b) * intensity,
            a: theme.TEXT_DIM.a,
        };
        let spinner = cx.entity().downgrade();
        let selector = if self.sidebar_mark {
            "panel-sidebar-mark"
        } else {
            "panel-activity-spinner"
        };
        let orb = div()
            .debug_selector(move || selector.into())
            .relative()
            .flex_none()
            .size(px(SIZE))
            .overflow_hidden()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if bounds.intersects(&window.content_mask().bounds) {
                            let reduce_motion = crate::config::get().appearance.reduce_motion;
                            let _ = spinner.update(cx, |spinner, cx| {
                                spinner.prepare(reduce_motion);
                                let theme = Theme::global();
                                let r_min = spinner.resolved.opts.r_min.unwrap_or(0.3);
                                // Every selected preset is dot-only, including tool activity.
                                debug_assert!(spinner.frame.lines.is_empty());
                                window.paint_layer(bounds, |window| {
                                    for dot in &spinner.frame.dots {
                                        if dot.a >= 0.02 {
                                            window.paint_path(
                                                dot_path(dot, r_min, bounds.origin),
                                                ink_color(dot.white, dot.a, &theme),
                                            );
                                        }
                                    }
                                });
                                spinner.arm(reduce_motion, cx);
                            });
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(orb)
            .when_some(label, |row, label| {
                row.child(
                    div()
                        .debug_selector(|| "panel-activity-label".into())
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.0))
                        .text_color(label_color)
                        .child(label),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_pulse_is_bounded_state_aware_and_motion_safe() {
        for activity in [
            Activity::Working,
            Activity::Thinking,
            Activity::Responding,
            Activity::RunningTools,
        ] {
            assert_eq!(label_intensity(activity, Duration::ZERO, false), 0.0);
            for millis in 0..6000 {
                let elapsed = Duration::from_millis(millis);
                assert!((0.0..=0.45).contains(&label_intensity(activity, elapsed, false)));
                assert_eq!(label_intensity(activity, elapsed, true), 0.0);
                assert_eq!(label_intensity(Activity::Idle, elapsed, false), 0.0);
            }
        }
        let time = Duration::from_millis(600);
        assert_ne!(
            label_intensity(Activity::Thinking, time, false),
            label_intensity(Activity::RunningTools, time, false)
        );
    }

    fn signature(frame: &Frame) -> Vec<(f32, f32, f32, f32, f32, f32)> {
        frame
            .dots
            .iter()
            .map(|d| (d.x, d.y, d.z, d.r, d.white, d.a))
            .collect()
    }

    #[test]
    fn activity_presets_are_distinct_and_dot_only() {
        use gpui_thinking_orbs::ModeKey;
        for (label, activity, mode) in [
            ("Working", Activity::Working, ModeKey::Orbits),
            ("Thinking", Activity::Thinking, ModeKey::Gyroscope),
            ("Responding", Activity::Responding, ModeKey::Ribbon),
            ("Running tools", Activity::RunningTools, ModeKey::Rubik),
        ] {
            assert_eq!(Activity::from_status_line(label), activity);
            let preset = activity.preset();
            assert_eq!(preset.mode, mode);
            let mut frame = Frame::new();
            for i in 0..100 {
                draw_mode_into(preset.mode, SIZE, i as f32 / 30.0, &preset.opts, &mut frame);
                assert!(!frame.dots.is_empty());
                assert!(frame.lines.is_empty());
            }
        }
    }

    #[gpui::test]
    fn retained_spinner_morphs_reverses_and_settles_idle(cx: &mut gpui::TestAppContext) {
        let spinner = cx.new(Spinner::new);
        spinner.update(cx, |spinner, cx| {
            spinner.prepare(false);
            let idle = signature(&spinner.frame);
            spinner.arm(false, cx);
            assert!(spinner.tick.is_none(), "static idle does not schedule work");
            spinner.set_activity(Activity::Thinking, cx);
            spinner.prepare(false);
            assert_eq!(
                signature(&spinner.frame),
                idle,
                "morph begins from painted pose"
            );
            let frozen_target = signature(&spinner.target_frame);
            let start = Instant::now();
            spinner.lease_started = Some(start);
            spinner.finish_lease(start + MORPH_DURATION / 2);
            spinner.geometry_dirty = true;
            spinner.prepare(false);
            let midway = signature(&spinner.frame);
            assert_eq!(signature(&spinner.target_frame), frozen_target);
            assert_eq!(spinner.motion_elapsed, Duration::ZERO);
            assert_ne!(midway, idle);
            spinner.set_activity(Activity::Idle, cx);
            spinner.prepare(false);
            assert_eq!(signature(&spinner.frame), midway, "reversal never jumps");
            spinner.elapsed += MORPH_DURATION;
            spinner.geometry_dirty = true;
            spinner.prepare(false);
            spinner.arm(false, cx);
            assert_eq!(signature(&spinner.frame), idle);
            assert!(spinner.morph_started.is_none());
            assert!(spinner.tick.is_none());
            for activity in [
                Activity::Thinking,
                Activity::Responding,
                Activity::RunningTools,
                Activity::Working,
                Activity::Idle,
            ] {
                spinner.set_activity(activity, cx);
                spinner.prepare(true);
                spinner.arm(true, cx);
                assert!(spinner.morph_started.is_none());
                assert!(spinner.tick.is_none());
                let pose = signature(&spinner.frame);
                let buffer = spinner.frame.dots.as_ptr();
                let target_buffer = spinner.target_frame.dots.as_ptr();
                spinner.prepare(true);
                assert_eq!(signature(&spinner.frame), pose);
                spinner.geometry_dirty = true;
                spinner.prepare(true);
                assert_eq!(signature(&spinner.frame), pose);
                assert_eq!(spinner.frame.dots.as_ptr(), buffer);
                assert_eq!(spinner.target_frame.dots.as_ptr(), target_buffer);
            }
        });
    }

    fn assert_inline_status(vcx: &mut gpui::VisualTestContext) {
        let spinner = vcx
            .debug_bounds("panel-activity-spinner")
            .expect("spinner paints");
        let label = vcx
            .debug_bounds("panel-activity-label")
            .expect("status paints");
        assert!(
            label.left() > spinner.right(),
            "status sits to the right of the spinner"
        );
        assert_eq!(spinner.size, gpui::size(px(SIZE), px(SIZE)));
        assert!(
            f32::from(label.center().y - spinner.center().y).abs() < 1.0,
            "status and spinner are vertically centered"
        );
        assert!(
            vcx.debug_bounds("panel-status-badge").is_none(),
            "no duplicate footer status"
        );
    }

    #[test]
    fn upstream_inline_detail_and_retained_buffer_budget() {
        let preset = resolve_preset(OrbState::Working, OrbSize::Inline);
        assert_eq!(preset.mode, gpui_thinking_orbs::ModeKey::Orbits);
        assert_eq!(OrbSize::Inline.pixels(), SIZE);
        assert_eq!(preset.speed, 3.9);
        let mut frame = Frame::new();
        draw_mode_into(preset.mode, SIZE, 0.6, &preset.opts, &mut frame);
        let capacity = frame.dots.capacity();
        let buffer = frame.dots.as_ptr();
        let first = frame.dots.clone();
        for i in 0..300 {
            draw_mode_into(preset.mode, SIZE, i as f32 / 30.0, &preset.opts, &mut frame);
            assert_eq!(frame.dots.capacity(), capacity);
            assert_eq!(frame.dots.as_ptr(), buffer);
            assert!(frame.lines.is_empty());
            assert_eq!(frame.dots.len(), 39, "upstream inline sparse detail");
            assert!(frame.dots.windows(2).all(|d| d[0].z <= d[1].z));
            for d in &frame.dots {
                assert!(
                    [d.x, d.y, d.z, d.r, d.white, d.a]
                        .iter()
                        .all(|x| x.is_finite())
                );
                let r = d.r.max(preset.opts.r_min.unwrap_or(0.3));
                assert!(
                    r > 0.0
                        && d.x - r >= 0.0
                        && d.x + r <= SIZE
                        && d.y - r >= 0.0
                        && d.y + r <= SIZE,
                    "upstream dot exceeds inline footprint: {d:?}"
                );
            }
        }
        assert!(
            first
                .iter()
                .zip(&frame.dots)
                .any(|(a, b)| a.x != b.x || a.y != b.y)
        );
        assert!(capacity * std::mem::size_of::<gpui_thinking_orbs::Dot>() <= 4096);
    }

    #[test]
    #[ignore = "manual upstream geometry and native paint-preparation timing"]
    fn benchmark_upstream_geometry_and_fractional_dot_paths() {
        let preset = resolve_preset(OrbState::Working, OrbSize::Inline);
        let mut frame = Frame::new();
        let origin = gpui::point(px(100.25), px(200.125));
        let start = Instant::now();
        let mut max_vertices = 0;
        for i in 0..3_000 {
            draw_mode_into(
                preset.mode,
                SIZE,
                i as f32 / 30.0 * preset.speed,
                &preset.opts,
                &mut frame,
            );
            let mut vertices = 0;
            for dot in &frame.dots {
                let path = dot_path(dot, preset.opts.r_min.unwrap_or(0.3), origin);
                vertices += path.vertices.len();
                // Include GPUI's final device-scale path allocation/map.
                std::hint::black_box(path.scale(2.0));
            }
            max_vertices = max_vertices.max(vertices);
        }
        eprintln!(
            "3k upstream Working/Inline geometry + 39 fractional paths: {:.3} us/frame, max {max_vertices} vertices/frame, {} retained geometry bytes",
            start.elapsed().as_secs_f64() * 1_000_000.0 / 3_000.0,
            frame.dots.capacity() * std::mem::size_of::<gpui_thinking_orbs::Dot>()
        );
    }

    #[test]
    fn fractional_circles_preserve_upstream_radius_and_subpixel_translation() {
        let dot = gpui_thinking_orbs::Dot::new(10.0, 10.0, 0.0, 0.1, 0.4);
        let origin = gpui::point(px(123.25), px(456.125));
        let path = dot_path(&dot, 0.3, origin);
        assert!(!path.vertices.is_empty());
        for vertex in &path.vertices {
            let x = f32::from(vertex.xy_position.x - origin.x);
            let y = f32::from(vertex.xy_position.y - origin.y);
            assert!(x.is_finite() && y.is_finite());
            assert!((9.699..=10.301).contains(&x));
            assert!((9.699..=10.301).contains(&y));
        }
        let moved = dot_path(&dot, 0.3, origin + gpui::point(px(0.125), px(0.25)));
        assert_eq!(path.vertices.len(), moved.vertices.len());
        for (a, b) in path.vertices.iter().zip(&moved.vertices) {
            assert!((f32::from(b.xy_position.x - a.xy_position.x) - 0.125).abs() < 0.0001);
            assert!((f32::from(b.xy_position.y - a.xy_position.y) - 0.25).abs() < 0.0001);
        }
        assert!((f32::from(path.bounds.size.width) - 0.6).abs() < 0.001);
    }

    #[test]
    fn theme_ink_preserves_shading_and_alpha() {
        let theme = Theme::global();
        let ink = ink_color(-1.0, 2.0, theme);
        assert_eq!(
            (ink.r, ink.g, ink.b, ink.a),
            (theme.TEXT.r, theme.TEXT.g, theme.TEXT.b, 1.0)
        );
        let paper = ink_color(2.0, -1.0, theme);
        assert!((paper.r - theme.PANEL_BG.r).abs() < 1e-6);
        assert!((paper.g - theme.PANEL_BG.g).abs() < 1e-6);
        assert!((paper.b - theme.PANEL_BG.b).abs() < 1e-6);
        assert_eq!(paper.a, 0.0);
        let middle = ink_color(0.5, 0.37, theme);
        assert_eq!(middle.a, 0.37);
        assert!((middle.r - (theme.TEXT.r + theme.PANEL_BG.r) * 0.5).abs() < 1e-6);
    }

    #[gpui::test]
    fn lease_clock_counts_actual_elapsed_once_and_excludes_idle(cx: &mut gpui::TestAppContext) {
        let spinner = cx.new(|cx| {
            let mut spinner = Spinner::new(cx);
            spinner.activity = Activity::Working;
            spinner
        });
        spinner.update(cx, |spinner, _| {
            let start = Instant::now();
            spinner.lease_started = Some(start);
            spinner.finish_lease(start + Duration::from_millis(47));
            assert_eq!(spinner.elapsed, Duration::from_millis(47));
            spinner.finish_lease(start + Duration::from_secs(3600));
            assert_eq!(spinner.elapsed, Duration::from_millis(47));
            spinner.lease_started = Some(start + Duration::from_secs(3600));
            spinner.finish_lease(start + Duration::from_secs(3600) + Duration::from_millis(41));
            assert_eq!(spinner.elapsed, Duration::from_millis(88));
        });
    }

    #[test]
    fn cadence_elapsed_time_and_reduced_motion_match_upstream() {
        assert!((TICK.as_secs_f64() - 1.0 / 30.0).abs() < 1e-9);
        for elapsed in [
            Duration::ZERO,
            Duration::from_millis(17),
            Duration::from_secs(3600),
        ] {
            assert_eq!(animation_time(elapsed, 3.9, true), 0.6);
            assert_eq!(
                animation_time(elapsed, 3.9, false),
                (elapsed.as_secs_f64() * 3.9_f32 as f64) as f32
            );
        }
        assert!(animation_time(Duration::from_millis(17), 3.9, false) > 0.0);
    }

    #[gpui::test]
    fn activity_clock_is_bounded_and_respects_reduced_motion(cx: &mut gpui::TestAppContext) {
        let spinner = cx.new(|cx| {
            let mut spinner = Spinner::new(cx);
            spinner.activity = Activity::Working;
            spinner
        });
        spinner.update(cx, |spinner, cx| {
            spinner.prepare(false);
            spinner.arm(true, cx);
            assert!(spinner.tick.is_none());
            spinner.arm(false, cx);
            spinner.arm(false, cx);
            assert!(spinner.tick.is_some());
        });
        cx.run_until_parked();
        cx.executor().advance_clock(TICK - Duration::from_nanos(1));
        cx.run_until_parked();
        spinner.read_with(cx, |spinner, _| assert!(!spinner.geometry_dirty));
        cx.executor().advance_clock(Duration::from_nanos(1));
        cx.run_until_parked();
        spinner.update(cx, |spinner, _| {
            assert!(spinner.geometry_dirty);
            assert!(spinner.tick.is_none());
            spinner.prepare(false);
        });
        // No paint means no new timer, even after a long hidden interval.
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        spinner.read_with(cx, |spinner, _| assert!(!spinner.geometry_dirty));
    }

    #[gpui::test]
    fn activity_clock_cancels_and_retains_reduced_motion_frame(cx: &mut gpui::TestAppContext) {
        let spinner = cx.new(|cx| {
            let mut spinner = Spinner::new(cx);
            spinner.activity = Activity::Working;
            spinner
        });
        spinner.update(cx, |spinner, cx| spinner.arm(false, cx));
        cx.run_until_parked();
        spinner.update(cx, |spinner, cx| {
            spinner.prepare(true);
            spinner.arm(true, cx);
            assert!(spinner.tick.is_none());
            let representative = gpui_thinking_orbs::draw_mode(
                spinner.resolved.mode,
                SIZE,
                0.6,
                &spinner.resolved.opts,
            );
            for (a, b) in spinner.frame.dots.iter().zip(&representative.dots) {
                assert_eq!(
                    (a.x, a.y, a.z, a.r, a.white, a.a),
                    (b.x, b.y, b.z, b.r, b.white, b.a)
                );
            }
        });
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        spinner.read_with(cx, |spinner, _| {
            assert!(!spinner.geometry_dirty);
            assert!(spinner.tick.is_none());
        });
    }

    #[gpui::test]
    fn transcript_activity_follows_output_and_disappears_on_terminal_events(
        cx: &mut gpui::TestAppContext,
    ) {
        use jcode_sdk::ApiEvent;
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("tail-test", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        for terminal in ["done", "error", "failed", "idle", "cancelled"] {
            for event in [
                ApiEvent::SessionStatus {
                    session_id: "tail-test".into(),
                    status: "thinking".into(),
                },
                ApiEvent::ReasoningDelta {
                    session_id: "tail-test".into(),
                    text: "Checking the result".into(),
                },
                ApiEvent::TextDelta {
                    message_id: None,
                    session_id: "tail-test".into(),
                    text: "Here is the result".into(),
                },
                ApiEvent::ToolStart {
                    session_id: "tail-test".into(),
                    call_id: terminal.into(),
                    name: "read".into(),
                },
            ] {
                panel.update(vcx, |panel, cx| panel.apply(&event, cx));
                vcx.run_until_parked();
                let activity = vcx
                    .debug_bounds("transcript-activity")
                    .expect("activity paints");
                let content_rows = panel.read_with(vcx, |panel, _| {
                    assert_eq!(
                        panel.transcript_row_count,
                        panel.transcript_render_rows().len() + 1
                    );
                    panel.transcript_render_rows().len()
                });
                if content_rows > 0 {
                    let last = vcx
                        .debug_bounds(format!("transcript-row-{}", content_rows - 1).leak())
                        .expect("last content row paints");
                    assert!(
                        activity.top() >= last.bottom(),
                        "spinner needs its own tail line"
                    );
                }
                assert!(vcx.debug_bounds("panel-status-spinner").is_none());
                assert_inline_status(vcx);
            }
            panel.update(vcx, |panel, cx| match terminal {
                "done" => panel.apply(
                    &ApiEvent::TurnDone {
                        session_id: "tail-test".into(),
                    },
                    cx,
                ),
                "error" => panel.apply(
                    &ApiEvent::Error {
                        code: jcode_sdk::api::ErrorCode::Internal,
                        message: "Provider failed".into(),
                    },
                    cx,
                ),
                "failed" => panel.message_failed("Send failed".into(), cx),
                _ => panel.apply(
                    &ApiEvent::SessionStatus {
                        session_id: "tail-test".into(),
                        status: terminal.into(),
                    },
                    cx,
                ),
            });
            vcx.run_until_parked();
            assert!(
                vcx.debug_bounds("transcript-activity").is_none(),
                "{terminal}"
            );
            assert!(
                vcx.debug_bounds("panel-activity-spinner").is_none(),
                "{terminal}"
            );
            assert!(
                vcx.debug_bounds("panel-activity-label").is_none(),
                "{terminal}"
            );
            assert!(
                vcx.debug_bounds("panel-status-badge").is_some(),
                "non-active status stays in footer"
            );
            panel.read_with(vcx, |panel, _| {
                assert!(!panel.activity_active(), "{terminal}");
                assert!(panel.streaming_reasoning.is_empty());
                assert!(panel.streaming_text.is_empty());
                assert_eq!(
                    panel.transcript_row_count,
                    panel.transcript_render_rows().len()
                );
            });
        }
    }

    #[gpui::test]
    fn streaming_activity_paints_and_clears_through_session_lifecycle(
        cx: &mut gpui::TestAppContext,
    ) {
        use jcode_sdk::ApiEvent;
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("activity-test", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("panel-activity-spinner").is_none());

        // Providers send a transport phase before text or a status update.
        // A failed previous turn must not hide this new turn's activity.
        panel.update(vcx, |panel, cx| {
            panel.message_failed("Previous request failed".into(), cx);
            panel.apply(
                &ApiEvent::ConnectionPhase {
                    session_id: "activity-test".into(),
                    phase: "streaming".into(),
                },
                cx,
            );
            assert!(panel.activity_active());
            assert!(panel.sidebar_activity().is_some());
            assert_eq!(panel.status_line(), "Responding");
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("transcript-activity").is_some());
        assert_inline_status(vcx);
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TurnDone {
                    session_id: "activity-test".into(),
                },
                cx,
            );
            assert!(!panel.activity_active());
        });

        for (status, expected) in [
            ("running", "Working"),
            ("generating", "Working"),
            ("thinking", "Thinking"),
            ("streaming", "Responding"),
            ("running_tools", "Running tools"),
        ] {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::SessionStatus {
                        session_id: "activity-test".into(),
                        status: status.into(),
                    },
                    cx,
                );
                assert!(panel.activity_active());
                assert_eq!(panel.status_line(), expected);
            });
            vcx.run_until_parked();
            panel.read_with(vcx, |panel, cx| {
                let expected = Activity::from_status_line(expected);
                for spinner in [
                    &panel.activity_spinner,
                    &panel.latest_activity_spinner,
                    &panel.sidebar_spinner,
                ] {
                    assert_eq!(spinner.read(cx).activity, expected);
                }
            });
            assert!(vcx.debug_bounds("panel-activity-spinner").is_some());
            assert!(vcx.debug_bounds("panel-session-title").is_none());
            assert_inline_status(vcx);
            let spinner = vcx.debug_bounds("transcript-activity").unwrap();
            let transcript = vcx.debug_bounds("transcript").unwrap();
            assert!(spinner.top() >= transcript.top());
            assert!(spinner.bottom() <= transcript.bottom());
            assert!(vcx.debug_bounds("panel-status-spinner").is_none());
        }
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::ConnectionPhase {
                    session_id: "activity-test".into(),
                    phase: "streaming".into(),
                },
                cx,
            );
            assert_eq!(panel.status_line(), "Running tools");
            panel.apply(
                &ApiEvent::ReasoningDelta {
                    session_id: "activity-test".into(),
                    text: "Let me check".into(),
                },
                cx,
            );
            assert_eq!(panel.status_line(), "Thinking");
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: "activity-test".into(),
                    text: "Here is the result".into(),
                },
                cx,
            );
            assert_eq!(panel.status_line(), "Responding");
            panel.apply(
                &ApiEvent::SessionStatus {
                    session_id: "activity-test".into(),
                    status: "idle".into(),
                },
                cx,
            );
            assert!(!panel.activity_active());
            assert_eq!(panel.status_line(), "Ready");
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("panel-activity-spinner").is_none());
        assert!(vcx.debug_bounds("panel-status-spinner").is_none());
        assert!(vcx.debug_bounds("transcript-activity").is_none());
        assert!(vcx.debug_bounds("panel-activity-label").is_none());
        panel.update(vcx, |panel, _| {
            for status in [
                "attached",
                "connected",
                "connecting",
                "lost: disconnected",
                "error",
                "crashed",
            ] {
                panel.status = status.into();
                assert!(!panel.activity_active(), "{status} should not animate");
            }
        });
    }
}
