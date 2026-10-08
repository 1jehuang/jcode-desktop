//! Smooth tail following for streamed text.
//!
//! Snapping to the end on every frame is correct for structural changes, but a
//! live row that wraps onto another line grows by a whole line height at once.
//! Pinned to the bottom, the transcript would lurch up by that line in one
//! frame while the text itself flows in smoothly. Instead, while text streams,
//! the scroll position is held for the layout pass that measures the growth,
//! then eased toward the new end so each new line glides into view.
//!
//! A pure spring toward the end works for slow streams but not fast ones. It
//! always lags the tail, so when lines arrive faster than it settles the gap
//! keeps growing until the old "too far, snap" escape hatch fires, and every
//! line restarts an accelerate/decelerate cycle. Fast output then reads as a
//! stutter of glides and jumps. The follower is therefore a velocity tracker:
//!
//! * it estimates how fast the content is growing (px/s, low-pass filtered),
//! * aims to scroll at that rate plus a term that closes the remaining gap,
//! * and eases its actual velocity toward that aim.
//!
//! Steady streams then scroll at a near-constant speed regardless of how
//! bursty the line wraps are, instead of a sawtooth. The lag is bounded so the
//! newest line never falls far below the fold, but bounding moves the view by
//! the overflow rather than snapping all the way to the end.
//!
//! Positions are tracked as floats and painted on whole device pixels. Glyphs
//! are rasterised on the vertical pixel grid while quads are not, so a
//! fractional scroll would make text and its backgrounds shimmer against each
//! other and step unevenly between frames.

use super::*;

/// Time constant for closing the remaining gap on top of the growth rate.
const CATCH_UP: f32 = 0.12;
/// How quickly the scroll velocity adopts a new aim. Small enough to hide the
/// line-sized steps of wrapping text, large enough to start promptly.
const VELOCITY_EASE: f32 = 0.06;
/// Smoothing of the content growth estimate.
const GROWTH_EASE: f32 = 0.3;
/// The least the live tail may trail the true end while streaming. The bound
/// scales up with the viewport so one tall card glides instead of jumping.
const MAX_LAG_PX: f32 = 240.0;
/// Smallest snap threshold. Gaps beyond three viewports snap (restores).
const MAX_GLIDE_PX: f32 = 160.0;
/// Ignore pathological frame gaps so a stale timestamp does not leap.
const MAX_STEP: Duration = Duration::from_millis(100);
/// How long after a turn stops producing output its tail still glides.
const LIVE_GRACE: Duration = Duration::from_millis(800);
/// Frame interval assumed for the first step of a glide.
const FIRST_STEP: Duration = Duration::from_millis(16);

/// Persistent state of the tail follower between frames.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TailGlide {
    /// Time of the previous follow frame. `None` when not gliding.
    at: Option<Instant>,
    /// Unrounded scroll position, px from the top.
    position: f32,
    /// Scroll velocity, px/s.
    velocity: f32,
    /// Filtered content growth rate, px/s.
    growth: f32,
    /// Maximum scroll offset seen on the previous frame.
    last_max: Option<f32>,
    /// The previous frame held still at the end, possibly for a long time.
    held: bool,
    /// Last frame the turn was producing output. Growth right after a turn
    /// ends (response stats, the final tool result) still glides.
    live_at: Option<Instant>,
}

impl TailGlide {
    pub(super) fn active(&self) -> bool {
        self.at.is_some()
    }

    pub(super) fn reset(&mut self) {
        *self = Self {
            live_at: self.live_at,
            ..Self::default()
        };
    }

    /// Advance one frame toward `target` from `current` after `dt` seconds.
    /// Returns the new unrounded position. Never passes the target.
    fn step(&mut self, current: f32, target: f32, dt: f32, max_lag: f32) -> f32 {
        if dt <= 0.0 {
            return current;
        }
        let grown = self.last_max.map_or(0.0, |last| (target - last).max(0.0));
        self.last_max = Some(target);
        let growth_now = grown / dt;
        self.growth += (growth_now - self.growth) * (1.0 - (-dt / GROWTH_EASE).exp());

        let gap = (target - current).max(0.0);
        // Never aim faster than would cover the gap in a few frames, so a
        // stale growth estimate cannot carry the view into the end and stop
        // abruptly after the stream pauses.
        let feed = self.growth.min(gap / 0.05);
        let aim = feed + gap / CATCH_UP;
        self.velocity += (aim - self.velocity) * (1.0 - (-dt / VELOCITY_EASE).exp());
        self.velocity = self.velocity.max(0.0);
        let mut next = (current + self.velocity * dt).min(target);
        if target - next > max_lag {
            next = target - max_lag;
        }
        if target - next <= 0.25 {
            next = target;
        }
        next
    }
}

impl Panel {
    /// A turn is producing output: text, reasoning, tool calls or results.
    /// The transcript may grow at any moment, so the tail is held by absolute
    /// position and growth glides in rather than being pinned by layout.
    fn tail_live(&self) -> bool {
        !self.streaming_text.is_empty()
            || !self.streaming_reasoning.is_empty()
            || self.activity_active()
    }

    /// Whether the follow position is eased rather than pinned this frame.
    pub(super) fn tail_gliding(&self) -> bool {
        self.stick_to_bottom && (self.tail_glide.active() || self.tail_live())
    }

    /// Keep a following transcript at its live tail. `snap` forces the
    /// immediate behaviour, for reduced motion.
    ///
    /// Every kind of growth at the tail glides: streamed text wrapping onto a
    /// new line, a tool card appearing, tool input or output filling in, the
    /// activity row, response stats. Only jumps larger than a few screens
    /// (history restores, reconnect recovery) snap. `_structural` is kept for
    /// callers. Row insertions are no longer a reason to jump.
    pub(super) fn follow_transcript_tail(
        &mut self,
        snap: bool,
        _structural: bool,
        window: &mut Window,
    ) {
        let bounds = self.transcript_list.viewport_bounds();
        let viewport = f32::from(bounds.size.height);
        let max = f32::from(self.transcript_list.max_offset_for_scrollbar().y).max(0.0);
        let listed = -f32::from(self.transcript_list.scroll_px_offset_for_scrollbar().y);
        let now = Instant::now();
        if self.tail_live() {
            self.tail_glide.live_at = Some(now);
        }
        let live = self
            .tail_glide
            .live_at
            .is_some_and(|at| now.saturating_duration_since(at) < LIVE_GRACE);
        // Resume from the unrounded position unless something else moved the list.
        let current =
            if self.tail_glide.active() && (self.tail_glide.position - listed).abs() <= 1.0 {
                self.tail_glide.position
            } else {
                listed
            };
        let gap = max - current;
        let limit = (viewport * 3.0).max(MAX_GLIDE_PX);
        if snap
            || !self.stick_to_bottom
            || viewport <= 0.0
            // Shrinking (a live row settling, the activity row leaving) is
            // held at the new end below, the same as layout would clamp it.
            || gap > limit
        {
            self.tail_glide.reset();
            self.transcript_list.scroll_to_end();
            return;
        }
        if gap <= 0.5 {
            if live {
                // Hold this position so growth measured in the coming layout
                // pass is eased in next frame instead of jumping.
                self.transcript_list
                    .set_offset_from_scrollbar(point(px(0.), px(-max)));
                let glide = &mut self.tail_glide;
                glide.held = true;
                glide.position = max;
                glide.last_max = Some(max);
                // Keep the growth estimate and timestamp warm between lines so
                // the next line starts at the stream's pace, not from rest.
                glide.velocity = glide.velocity.min(glide.growth);
                if glide.at.is_none() {
                    glide.growth = 0.0;
                }
                glide.at = Some(now);
                // Streamed text grows every frame. Other growth (tool cards,
                // tool output) is noticed after paint by
                // `tail_growth_observer`, so a long tool run does not redraw
                // the panel every frame while nothing changes.
                if !self.streaming_text.is_empty() || !self.streaming_reasoning.is_empty() {
                    window.request_animation_frame();
                }
            } else {
                // Idle: let layout pin the end without scheduling frames.
                self.tail_glide.reset();
                self.transcript_list.scroll_to_end();
            }
            return;
        }
        // After holding still, the previous timestamp only says when the
        // tail was last checked, not a frame interval.
        let dt = self
            .tail_glide
            .at
            .filter(|_| !self.tail_glide.held)
            .map(|last| now.saturating_duration_since(last).min(MAX_STEP))
            .unwrap_or(FIRST_STEP)
            .as_secs_f32();
        self.tail_glide.held = false;
        if self.tail_glide.at.is_none() {
            self.tail_glide.last_max = Some(current);
        }
        self.tail_glide.at = Some(now);
        let max_lag = (viewport * 0.6).max(MAX_LAG_PX);
        let next = self.tail_glide.step(current, max, dt, max_lag);
        self.tail_glide.position = next;
        let scale = window.scale_factor().max(1.0);
        let painted = if next >= max {
            max
        } else {
            ((next * scale).round() / scale).min(max)
        };
        self.transcript_list
            .set_offset_from_scrollbar(point(px(0.), px(-painted)));
        window.request_animation_frame();
    }
}

impl Panel {
    /// Paints after the transcript list. Layout may have grown the content
    /// (a tool card, tool output, an image) without anything scheduling the
    /// next frame, which would leave the new rows below the fold until some
    /// unrelated redraw, then jump. Wake the follower as soon as it happens.
    pub(super) fn tail_growth_observer(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let panel = cx.entity().downgrade();
        let list = self.transcript_list.clone();
        let following = self.stick_to_bottom;
        let held = self.tail_glide.position;
        // Mid-glide the follower already schedules its own frames. Only a
        // tail resting at the end needs waking when layout grows it.
        let resting = self.tail_glide.active() && self.tail_glide.held;
        gpui::canvas(
            |_, _, _| (),
            move |_, _, _, cx| {
                if !following || !resting {
                    return;
                }
                let max = f32::from(list.max_offset_for_scrollbar().y);
                if max <= held + 0.5 {
                    return;
                }
                cx.defer(move |cx| {
                    let _ = panel.update(cx, |_, cx| cx.notify());
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate a stream adding `line` px at random-ish intervals averaging
    /// `per_second` lines and return the per-frame scroll steps.
    fn simulate(per_second: f32, frames: usize) -> (Vec<f32>, f32) {
        let mut glide = TailGlide::default();
        let dt = 1.0 / 60.0;
        let line = 22.0;
        let mut target = 0.0f32;
        let mut position = 0.0f32;
        let mut debt = 0.0f32;
        let mut seed = 0x2545_f491u32;
        let mut steps = Vec::new();
        let mut max_lag = 0.0f32;
        glide.last_max = Some(0.0);
        for _ in 0..frames {
            // Bursty arrivals: a deterministic pseudo-random fraction of the
            // average per frame, so some frames add several lines.
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            debt += per_second * dt * 2.0 * (seed % 1000) as f32 / 1000.0;
            while debt >= 1.0 {
                target += line;
                debt -= 1.0;
            }
            let next = glide.step(position, target, dt, MAX_LAG_PX);
            assert!(next <= target && next >= position, "monotonic, bounded");
            steps.push(next - position);
            position = next;
            max_lag = max_lag.max(target - position);
        }
        (steps, max_lag)
    }

    fn variation(steps: &[f32]) -> f32 {
        let mean = steps.iter().sum::<f32>() / steps.len() as f32;
        let var = steps.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / steps.len() as f32;
        var.sqrt() / mean.max(0.01)
    }

    #[test]
    fn fast_streams_scroll_at_a_steady_pace_without_jumps() {
        for per_second in [20.0, 50.0, 80.0, 150.0] {
            let (steps, max_lag) = simulate(per_second, 600);
            let steady = &steps[120..];
            let mean = steady.iter().sum::<f32>() / steady.len() as f32;
            let expected = per_second * 22.0 / 60.0;
            assert!(
                (mean - expected).abs() < expected * 0.25,
                "{per_second}/s keeps pace: {mean} vs {expected}"
            );
            assert!(max_lag <= MAX_LAG_PX + 0.5, "{per_second}/s lag {max_lag}");
            // A single frame never moves more than a few lines' worth beyond
            // the average, so there are no catch-up snaps.
            let peak = steady.iter().cloned().fold(0.0, f32::max);
            assert!(
                peak < expected * 3.0 + 10.0,
                "{per_second}/s peak step {peak} vs mean {expected}"
            );
            assert!(
                variation(steady) < 0.9,
                "{per_second}/s smooth: {}",
                variation(steady)
            );
        }
    }

    #[test]
    fn a_single_line_eases_in_and_settles() {
        let mut glide = TailGlide {
            last_max: Some(0.0),
            ..Default::default()
        };
        let mut position = 0.0;
        let mut steps = Vec::new();
        for _ in 0..60 {
            let next = glide.step(position, 22.0, 1.0 / 60.0, MAX_LAG_PX);
            assert!(next <= 22.0 && next >= position);
            steps.push(next - position);
            position = next;
        }
        assert!(steps[0] < 6.0, "gentle start: {}", steps[0]);
        assert_eq!(position, 22.0, "settles exactly on the tail");
    }

    #[test]
    fn stops_promptly_when_the_stream_pauses() {
        let mut glide = TailGlide::default();
        glide.last_max = Some(0.0);
        let mut target = 0.0;
        let mut position = 0.0;
        for _ in 0..120 {
            target += 15.0;
            position = glide.step(position, target, 1.0 / 60.0, MAX_LAG_PX);
        }
        let mut frames = 0;
        while position < target {
            position = glide.step(position, target, 1.0 / 60.0, MAX_LAG_PX);
            frames += 1;
            assert!(frames < 60, "caught up within a second");
        }
    }

    fn gap(panel: &Panel) -> f32 {
        f32::from(
            panel.transcript_list.max_offset_for_scrollbar().y
                + panel.transcript_list.scroll_px_offset_for_scrollbar().y,
        )
    }

    fn frame(vcx: &mut gpui::VisualTestContext) {
        vcx.update(|window, cx| window.simulate_next_frame(cx));
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(8));
    }

    #[gpui::test]
    fn wrapped_streaming_lines_glide_into_view(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("tail-glide", cx);
            workspace
        });
        let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.animate_stream_in_tests = true;
            *panel.items = (0..60)
                .map(|n| Item::Assistant(format!("History {n}")))
                .collect();
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: panel.session_id.clone(),
                    text: "Opening line.".into(),
                },
                cx,
            );
        });
        for _ in 0..60 {
            frame(vcx);
        }
        panel.read_with(vcx, |panel, _| {
            assert!(panel.stick_to_bottom);
            assert!(
                gap(panel).abs() <= 0.5,
                "settled at the tail: {}",
                gap(panel)
            );
        });

        // New paragraphs add whole lines to the live row.
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: panel.session_id.clone(),
                    text: "\n\nSecond paragraph.\n\nThird paragraph.".into(),
                },
                cx,
            );
        });
        let mut max_gap = 0.0f32;
        let mut previous = None;
        for _ in 0..60 {
            frame(vcx);
            let now = panel.read_with(vcx, |panel, _| gap(panel));
            max_gap = max_gap.max(now);
            if let Some(previous) = previous {
                assert!(
                    now <= previous + 40.0,
                    "glide reversed: {previous} -> {now}"
                );
            }
            previous = Some(now);
        }
        assert!(max_gap > 1.0, "growth was eased, not snapped: {max_gap}");
        panel.read_with(vcx, |panel, _| {
            assert!(panel.stick_to_bottom);
            assert!(
                gap(panel).abs() <= 0.5,
                "caught up to the tail: {}",
                gap(panel)
            );
        });
    }

    #[gpui::test]
    fn fast_streams_follow_without_snapping(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("tail-glide-fast", cx);
            workspace
        });
        let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.animate_stream_in_tests = true;
            *panel.items = (0..60)
                .map(|n| Item::Assistant(format!("History {n}")))
                .collect();
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: panel.session_id.clone(),
                    text: "Opening line.".into(),
                },
                cx,
            );
        });
        for _ in 0..30 {
            frame(vcx);
        }
        let offset =
            |panel: &Panel| -f32::from(panel.transcript_list.scroll_px_offset_for_scrollbar().y);
        // A very fast model: several paragraphs every frame.
        let mut previous = panel.read_with(vcx, |panel, _| offset(panel));
        let mut steps = Vec::new();
        for n in 0..90 {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::TextDelta {
                        message_id: None,
                        session_id: panel.session_id.clone(),
                        text: format!("\n\nParagraph {n} streams in quickly."),
                    },
                    cx,
                );
            });
            frame(vcx);
            let now = panel.read_with(vcx, |panel, _| {
                assert!(panel.stick_to_bottom, "still following");
                assert!(gap(panel) <= MAX_LAG_PX + 1.0, "bounded lag {}", gap(panel));
                offset(panel)
            });
            assert!(
                now + 0.5 >= previous,
                "never scrolls backwards: {previous} -> {now}"
            );
            steps.push(now - previous);
            previous = now;
        }
        let moving: Vec<f32> = steps[20..].to_vec();
        let mean = moving.iter().sum::<f32>() / moving.len() as f32;
        let peak = moving.iter().cloned().fold(0.0, f32::max);
        assert!(mean > 1.0, "scrolled with the stream: {steps:?}");
        assert!(
            peak <= mean * 4.0 + 12.0,
            "no catch-up snaps: peak {peak} mean {mean} {steps:?}"
        );
    }

    #[gpui::test]
    fn tool_cards_and_text_after_them_glide_instead_of_jumping(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("tail-glide-tools", cx);
            workspace
        });
        let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.animate_stream_in_tests = true;
            *panel.items = (0..60)
                .map(|n| Item::Assistant(format!("History {n}")))
                .collect();
            panel.status = "running".into();
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: panel.session_id.clone(),
                    text: "Let me look.".into(),
                },
                cx,
            );
        });
        for _ in 0..30 {
            frame(vcx);
        }
        let offset =
            |panel: &Panel| -f32::from(panel.transcript_list.scroll_px_offset_for_scrollbar().y);
        let mut previous = panel.read_with(vcx, |panel, _| offset(panel));
        let mut glided = false;
        // Fast agent loop: tool calls, their input, results, and more text,
        // a new structural row every few frames.
        for n in 0..12 {
            let call = format!("call-{n}");
            for step in 0..6 {
                panel.update(vcx, |panel, cx| {
                    let session_id = panel.session_id.clone();
                    let event = match step {
                        0 => ApiEvent::ToolStart {
                            session_id,
                            call_id: call.clone(),
                            name: "bash".into(),
                        },
                        1 => ApiEvent::ToolInputDelta {
                            session_id,
                            call_id: call.clone(),
                            delta: format!(r#"{{"intent":"step {n}","command":"ls"}}"#),
                        },
                        2 => ApiEvent::ToolDone {
                            session_id,
                            call_id: call.clone(),
                            name: "bash".into(),
                            output: "one\ntwo\nthree\nfour".into(),
                            error: None,
                        },
                        _ => ApiEvent::TextDelta {
                            message_id: None,
                            session_id,
                            text: format!("\n\nResult {n}.{step} looks fine, continuing."),
                        },
                    };
                    panel.apply(&event, cx);
                });
                frame(vcx);
                let (now, gap_now) = panel.read_with(vcx, |panel, _| {
                    assert!(panel.stick_to_bottom, "still following");
                    (offset(panel), gap(panel))
                });
                assert!(
                    now + 0.5 >= previous,
                    "never scrolls backwards: {previous} -> {now}"
                );
                glided |= gap_now > 1.0;
                previous = now;
            }
        }
        assert!(glided, "tail growth was eased, not snapped");
        for _ in 0..90 {
            frame(vcx);
        }
        panel.read_with(vcx, |panel, _| {
            assert!(gap(panel).abs() <= 0.5, "caught up: {}", gap(panel));
        });
    }
}
