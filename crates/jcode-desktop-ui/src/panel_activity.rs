//! A small activity clock, armed only when its spinner is actually painted.
//!
//! Never request a display-rate animation for a long-running agent. Eight steps
//! per second are enough for this cached native torus. A clipped/hidden spinner
//! stops rearming, and reduced motion keeps the same static activity marker.

use std::time::Duration;

use gpui::{Context, Render, Task, Window, canvas, div, prelude::*, px};

use crate::theme::Theme;

#[path = "panel_activity_donut.rs"]
mod donut;

const TICK: Duration = Duration::from_millis(125);

pub(super) struct Spinner {
    step: usize,
    tick: Option<Task<()>>,
}

impl Spinner {
    pub(super) fn new(_: &mut Context<Self>) -> Self {
        Self {
            step: 0,
            tick: None,
        }
    }

    fn arm(&mut self, reduce_motion: bool, cx: &mut Context<Self>) {
        if reduce_motion {
            self.tick = None;
            return;
        }
        if self.tick.is_some() {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TICK).await;
            let _ = this.update(cx, |spinner, cx| {
                spinner.tick = None;
                spinner.step = (spinner.step + 1) % donut::FRAME_COUNT;
                cx.notify();
            });
        }));
    }
}

fn visible_step(step: usize, reduce_motion: bool) -> usize {
    if reduce_motion { 0 } else { step }
}

// Filled triangles need only XY in the cache. The other 24 bytes in a GPUI
// vertex are the same ST coordinate and default content mask for every vertex.
struct DonutPath {
    bounds: gpui::Bounds<gpui::Pixels>,
    vertices: Box<[gpui::Point<gpui::Pixels>]>,
}

impl DonutPath {
    fn at(&self, origin: gpui::Point<gpui::Pixels>) -> gpui::Path<gpui::Pixels> {
        let mut path = gpui::Path::new(origin);
        path.bounds = self.bounds;
        path.bounds.origin += origin;
        path.vertices = self
            .vertices
            .iter()
            .map(|&position| gpui::PathVertex {
                xy_position: position + origin,
                st_position: gpui::point(0.0, 1.0),
                content_mask: Default::default(),
            })
            .collect();
        path
    }
}

// Cache each frame lazily, so the first paint does not build the whole cycle.
// A single fractional-coordinate path avoids paint_quad's device-pixel snapping
// and batches the entire halftone into one native primitive.
fn donut_path(step: usize) -> &'static DonutPath {
    use std::sync::OnceLock;
    static PATHS: [OnceLock<DonutPath>; donut::FRAME_COUNT] =
        [const { OnceLock::new() }; donut::FRAME_COUNT];
    let step = step % donut::FRAME_COUNT;
    PATHS[step].get_or_init(|| build_donut_path(step))
}

fn build_donut_path(step: usize) -> DonutPath {
    let mut builder = gpui::PathBuilder::fill().with_style(gpui::PathStyle::Fill(
        gpui::FillOptions::default()
            .with_fill_rule(gpui::FillRule::NonZero)
            .with_tolerance(0.01),
    ));
    for dot in donut::frame(step) {
        let p = |x: f32, y: f32| gpui::point(px(dot.x + x), px(dot.y + y));
        let r = dot.radius;
        let k = r * 0.552_284_8;
        builder.move_to(p(r, 0.0));
        builder.cubic_bezier_to(p(0.0, r), p(r, k), p(k, r));
        builder.cubic_bezier_to(p(-r, 0.0), p(-k, r), p(-r, k));
        builder.cubic_bezier_to(p(0.0, -r), p(-r, -k), p(-k, -r));
        builder.cubic_bezier_to(p(r, 0.0), p(k, -r), p(r, -k));
        builder.close();
    }
    let path = builder
        .build()
        .expect("finite bounded donut circles tessellate");
    DonutPath {
        bounds: path.bounds,
        vertices: path
            .vertices
            .into_iter()
            .map(|vertex| {
                debug_assert_eq!(vertex.st_position, gpui::point(0.0, 1.0));
                debug_assert_eq!(vertex.content_mask, Default::default());
                vertex.xy_position
            })
            .collect(),
    }
}

impl Render for Spinner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let spinner = cx.entity().downgrade();
        let step = self.step;
        div()
            .debug_selector(|| "panel-activity-spinner".into())
            .relative()
            .flex_none()
            .size(px(14.0))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if bounds.intersects(&window.content_mask().bounds) {
                            let reduce_motion = crate::config::get().appearance.reduce_motion;
                            let color = Theme::global().ACCENT;
                            let path =
                                donut_path(visible_step(step, reduce_motion)).at(bounds.origin);
                            window.paint_path(path, color);
                            let _ =
                                spinner.update(cx, |spinner, cx| spinner.arm(reduce_motion, cx));
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn reduced_motion_always_shows_canonical_pose() {
        for step in 0..donut::FRAME_COUNT {
            assert_eq!(visible_step(step, true), 0);
            assert_eq!(visible_step(step, false), step);
        }
    }

    #[test]
    fn compact_path_restores_fill_vertices_at_translated_origin() {
        let origin = gpui::point(px(123.25), px(456.5));
        for step in 0..donut::FRAME_COUNT {
            let cached = donut_path(step);
            let path = cached.at(origin);
            assert_eq!(path.bounds.origin, cached.bounds.origin + origin);
            assert_eq!(path.bounds.size, cached.bounds.size);
            assert_eq!(path.vertices.len(), cached.vertices.len());
            for (vertex, position) in path.vertices.iter().zip(&cached.vertices) {
                assert_eq!(vertex.xy_position, *position + origin);
                assert_eq!(vertex.st_position, gpui::point(0.0, 1.0));
                assert_eq!(vertex.content_mask, Default::default());
            }
        }
    }

    #[test]
    fn native_paths_are_cached_finite_and_bounded() {
        let start = std::time::Instant::now();
        let mut vertices = 0;
        for step in 0..donut::FRAME_COUNT {
            let path = donut_path(step);
            assert!(std::ptr::eq(path, donut_path(step + donut::FRAME_COUNT)));
            assert!(!path.vertices.is_empty());
            for vertex in &path.vertices {
                let x = f32::from(vertex.x);
                let y = f32::from(vertex.y);
                assert!(x.is_finite() && y.is_finite());
                assert!((0.0..=14.0).contains(&x) && (0.0..=14.0).contains(&y));
            }
            vertices += path.vertices.len();
        }
        // A finite shared cache, independent of spinner count and run duration.
        assert!(vertices * std::mem::size_of::<gpui::Point<gpui::Pixels>>() <= 3 * 1024 * 1024);
        eprintln!(
            "32 native paths: {:?}, {vertices} vertices, {} vertex bytes",
            start.elapsed(),
            vertices * std::mem::size_of::<gpui::Point<gpui::Pixels>>()
        );
    }

    #[test]
    #[ignore = "manual cold native mesh timing without machine-dependent assertions"]
    fn benchmark_cold_native_meshes() {
        let start = std::time::Instant::now();
        let mut slowest = Duration::ZERO;
        let mut bytes = 0;
        for step in 0..donut::FRAME_COUNT {
            let frame_start = std::time::Instant::now();
            let path = std::hint::black_box(build_donut_path(step));
            slowest = slowest.max(frame_start.elapsed());
            bytes += path.vertices.len() * std::mem::size_of::<gpui::Point<gpui::Pixels>>();
        }
        eprintln!(
            "cold32 native meshes: {:?}, slowest frame {slowest:?}, {bytes} compact vertex bytes",
            start.elapsed()
        );
    }

    #[test]
    #[ignore = "manual native paint-preparation timing without machine-dependent assertions"]
    fn benchmark_hot_native_paint_preparation() {
        let mut bytes = 0;
        let mut max_vertices = 0;
        for step in 0..donut::FRAME_COUNT {
            let vertices = donut_path(step).vertices.len();
            bytes += vertices * std::mem::size_of::<gpui::Point<gpui::Pixels>>();
            max_vertices = max_vertices.max(vertices);
        }
        let start = std::time::Instant::now();
        for step in 0..10_000 {
            let origin = gpui::point(px(100.25), px(200.5));
            let path = donut_path(step).at(origin);
            // paint_path performs this final device-scale allocation/map.
            std::hint::black_box(path.scale(2.0));
        }
        eprintln!(
            "10k native paint preparations: {:.3} us/frame, {bytes} cached vertex bytes, max {max_vertices} vertices/frame, one path primitive",
            start.elapsed().as_secs_f64() * 1_000_000.0 / 10_000.0,
        );
    }

    #[gpui::test]
    fn activity_clock_is_bounded_and_respects_reduced_motion(cx: &mut gpui::TestAppContext) {
        let spinner = cx.new(Spinner::new);
        spinner.update(cx, |spinner, cx| {
            spinner.arm(true, cx);
            assert!(spinner.tick.is_none());
            spinner.arm(false, cx);
            spinner.arm(false, cx);
            assert!(spinner.tick.is_some());
        });
        cx.run_until_parked();
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        spinner.read_with(cx, |spinner, _| {
            assert_eq!(spinner.step, 1, "only one timer may be armed");
            assert!(spinner.tick.is_none());
        });
        // No paint means no new timer, even after a long hidden interval.
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        spinner.read_with(cx, |spinner, _| assert_eq!(spinner.step, 1));
    }

    #[gpui::test]
    fn activity_clock_wraps_and_cancels_when_motion_is_reduced(cx: &mut gpui::TestAppContext) {
        let spinner = cx.new(Spinner::new);
        spinner.update(cx, |spinner, cx| {
            spinner.step = donut::FRAME_COUNT - 1;
            spinner.arm(false, cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(TICK - Duration::from_millis(1));
        cx.run_until_parked();
        spinner.read_with(cx, |spinner, _| {
            assert_eq!(spinner.step, donut::FRAME_COUNT - 1)
        });
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        spinner.update(cx, |spinner, cx| {
            assert_eq!(spinner.step, 0);
            spinner.arm(false, cx);
        });
        cx.run_until_parked();
        spinner.update(cx, |spinner, cx| {
            spinner.arm(true, cx);
            assert!(spinner.tick.is_none());
        });
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        spinner.read_with(cx, |spinner, _| assert_eq!(spinner.step, 0));
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
