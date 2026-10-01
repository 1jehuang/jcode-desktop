//! Smooth tail following for streamed text.
//!
//! Snapping to the end on every frame is correct for structural changes, but a
//! live row that wraps onto another line grows by a whole line height at once.
//! Pinned to the bottom, the transcript would lurch up by that line in one
//! frame while the text itself flows in smoothly. Instead, while text streams,
//! the scroll position is held for the layout pass that measures the growth,
//! then eased toward the new end so each new line glides into view.
//!
//! The glide is a critically damped spring that carries velocity across
//! frames. A plain exponential moves fastest on its very first frame, which
//! reads as a small lurch on every wrapped line. The spring starts gently,
//! never overshoots, and when another line arrives mid-glide it keeps its
//! momentum instead of restarting, so continuous streaming scrolls steadily.

use super::*;

/// Approximate time for the spring to cover a new line. Short enough that a
/// line settles before the next one usually arrives, long enough to read as
/// continuous motion rather than a step.
const GLIDE: f32 = 0.13;
/// Larger gaps (tool cards, pasted blocks, restores) snap as before.
const MAX_GLIDE_PX: f32 = 160.0;
/// Ignore pathological frame gaps so a stale timestamp does not leap.
const MAX_STEP: Duration = Duration::from_millis(100);

impl Panel {
    /// Whether the follow position is eased rather than pinned this frame.
    pub(super) fn tail_gliding(&self) -> bool {
        self.stick_to_bottom
            && (self.tail_glide_at.is_some()
                || !self.streaming_text.is_empty()
                || !self.streaming_reasoning.is_empty())
    }

    /// Keep a following transcript at its live tail. `snap` forces the
    /// immediate behaviour, for reduced motion and structural changes.
    pub(super) fn follow_transcript_tail(&mut self, snap: bool, window: &mut Window) {
        let bounds = self.transcript_list.viewport_bounds();
        let max = f32::from(self.transcript_list.max_offset_for_scrollbar().y).max(0.0);
        let current = -f32::from(self.transcript_list.scroll_px_offset_for_scrollbar().y);
        let gap = max - current;
        if snap
            || !self.tail_gliding()
            || bounds.size.height <= px(0.)
            || !(-0.5..=MAX_GLIDE_PX).contains(&gap)
        {
            self.tail_glide_at = None;
            self.tail_glide_velocity = 0.0;
            self.transcript_list.scroll_to_end();
            return;
        }
        let streaming = !self.streaming_text.is_empty() || !self.streaming_reasoning.is_empty();
        if gap <= 0.5 {
            if streaming {
                // Hold this position so growth measured in the coming layout
                // pass is eased in next frame instead of jumping.
                self.transcript_list
                    .set_offset_from_scrollbar(point(px(0.), px(-max)));
                self.tail_glide_at = None;
                self.tail_glide_velocity = 0.0;
                window.request_animation_frame();
            } else {
                self.tail_glide_at = None;
                self.tail_glide_velocity = 0.0;
                self.transcript_list.scroll_to_end();
            }
            return;
        }
        let now = Instant::now();
        let dt = self
            .tail_glide_at
            .map(|last| now.saturating_duration_since(last).min(MAX_STEP))
            .unwrap_or(Duration::from_millis(16))
            .as_secs_f32();
        self.tail_glide_at = Some(now);
        let (next, velocity) = spring_step(current, max, self.tail_glide_velocity, dt);
        let (next, velocity) = if max - next <= 0.25 {
            (max, 0.0)
        } else {
            (next, velocity)
        };
        self.tail_glide_velocity = velocity;
        self.transcript_list
            .set_offset_from_scrollbar(point(px(0.), px(-next)));
        window.request_animation_frame();
    }
}

/// One step of a critically damped spring from `current` toward `target`.
/// Returns the new position and velocity. Never passes the target.
fn spring_step(current: f32, target: f32, velocity: f32, dt: f32) -> (f32, f32) {
    let omega = 2.0 / GLIDE;
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = current - target;
    let temp = (velocity + omega * change) * dt;
    let mut velocity = (velocity - omega * temp) * decay;
    let mut next = target + (change + temp) * decay;
    if (target - current > 0.0) == (next > target) {
        next = target;
        velocity = 0.0;
    }
    (next, velocity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_eases_in_and_out_without_overshoot() {
        let mut position = 0.0;
        let mut velocity = 0.0;
        let mut steps = Vec::new();
        for _ in 0..40 {
            let (next, v) = spring_step(position, 22.0, velocity, 1.0 / 60.0);
            assert!(next <= 22.0 && next >= position);
            steps.push(next - position);
            position = next;
            velocity = v;
        }
        let peak = steps
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!(peak > 0, "first frame is not the fastest: {steps:?}");
        assert!(steps[0] < 3.0, "gentle start: {}", steps[0]);
        assert!(22.0 - position < 0.5, "settles: {position}");
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
}
