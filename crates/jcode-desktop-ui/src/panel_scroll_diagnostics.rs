//! Bounded flight recorder for transcript scroll jumps. No text or session IDs.
//! Samples only existing paints and never requests a repaint or polls the disk.
use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui::{EntityId, ListState};
use serde::Serialize;

const HISTORY: usize = 24;
const ACTIVE_WINDOW: Duration = Duration::from_millis(750);
const MAX_GAP: Duration = Duration::from_millis(500);
const COOLDOWN: Duration = Duration::from_secs(10);
const JUMP_PX: f32 = 24.0;

#[derive(Clone, Debug, Default, Serialize)]
struct Sample {
    timestamp_ms: u128,
    dt_ms: u128,
    input_px: f32,
    input_events: u32,
    precise: bool,
    applied_px: f32,
    // Positive movement means scrolling down through the document.
    observed_px: Option<f32>,
    row: usize,
    offset_in_row: f32,
    estimated_scroll_y: f32,
    estimated_max_y: f32,
    rows: usize,
    viewport: [i32; 4],
    following_tail: bool,
    dragging: bool,
    active: bool,
}

#[derive(Default)]
pub(super) struct Recorder {
    recent: VecDeque<Sample>,
    last_paint: Option<Instant>,
    last_activity: Option<Instant>,
    last_log: Option<Instant>,
    input_px: f32,
    input_events: u32,
    precise: bool,
    applied_px: f32,
    anchor: Option<(usize, f32)>,
}

impl Recorder {
    pub(super) fn input(&mut self, pixels: f32, precise: bool, now: Instant) {
        if !pixels.is_finite() || pixels == 0.0 {
            return;
        }
        self.input_px += pixels;
        self.input_events = self.input_events.saturating_add(1);
        self.precise = precise;
        self.last_activity = Some(now);
    }

    pub(super) fn applied(&mut self, pixels: f32, now: Instant) {
        if !pixels.is_finite() || pixels == 0.0 {
            return;
        }
        self.applied_px += pixels;
        self.last_activity = Some(now);
    }

    pub(super) fn paint(&mut self, list: &ListState, viewport: [i32; 4], entity: EntityId) {
        let now = Instant::now();
        let top = list.logical_scroll_top();
        let bounds = list.viewport_bounds();
        // Track a measured row, not the scrollbar estimate. Measuring previously
        // unseen rows can change the latter without moving anything on screen.
        let observed_px = self
            .anchor
            .and_then(|(ix, y)| {
                list.bounds_for_item(ix)
                    .map(|b| y - f32::from(b.top() - bounds.top()))
            })
            .or_else(|| {
                self.recent.back().and_then(|previous| {
                    (previous.row == top.item_ix)
                        .then(|| f32::from(top.offset_in_item) - previous.offset_in_row)
                })
            });
        let anchor_ix = (top.item_ix + 1).min(list.item_count().saturating_sub(1));
        self.anchor = list
            .bounds_for_item(anchor_ix)
            .map(|b| (anchor_ix, f32::from(b.top() - bounds.top())));
        let sample = Sample {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            observed_px,
            row: top.item_ix,
            offset_in_row: f32::from(top.offset_in_item),
            estimated_scroll_y: f32::from(list.scroll_px_offset_for_scrollbar().y),
            estimated_max_y: f32::from(list.max_offset_for_scrollbar().y),
            rows: list.item_count(),
            viewport,
            following_tail: list.is_following_tail(),
            dragging: list.is_scrollbar_dragging(),
            ..Sample::default()
        };
        if let Some(reason) = self.observe(sample, now) {
            // One bounded JSON record per occurrence, with the lead-up included.
            // The host redirects stderr to its persistent local diagnostic log.
            let record = serde_json::json!({
                "event": "scroll_anomaly",
                "panel": format!("{entity:?}"),
                "reason": reason,
                "samples": self.recent,
            });
            eprintln!("jcode desktop: {record}");
        }
    }

    fn observe(&mut self, mut sample: Sample, now: Instant) -> Option<&'static str> {
        let gap = self
            .last_paint
            .map(|last| now.saturating_duration_since(last));
        self.last_paint = Some(now);
        sample.dt_ms = gap.unwrap_or_default().as_millis();
        sample.active = self
            .last_activity
            .is_some_and(|last| now.saturating_duration_since(last) <= ACTIVE_WINDOW);
        sample.input_px = std::mem::take(&mut self.input_px);
        sample.input_events = std::mem::take(&mut self.input_events);
        sample.precise = self.precise;
        sample.applied_px = std::mem::take(&mut self.applied_px);
        let reason = self.recent.back().and_then(|previous| {
            if !sample.active || sample.dragging || previous.dragging || gap? > MAX_GAP {
                return None;
            }
            if let Some(observed) = sample.observed_px {
                if (observed - sample.applied_px).abs() > JUMP_PX {
                    return Some("painted_movement_mismatch");
                }
            } else if sample.row != previous.row
                && (sample.applied_px.abs() < 1.0
                    || (sample.row > previous.row) != (sample.applied_px > 0.0))
            {
                // A large jump can move the anchor out of the measured range.
                // Row ordering still proves reversal or uncommanded movement.
                return Some("logical_scroll_discontinuity");
            }
            if previous.active && sample.dt_ms >= 80 && sample.applied_px.abs() > 1.0 {
                return Some("scroll_frame_gap");
            }
            None
        });
        if gap.is_some_and(|gap| gap > MAX_GAP) {
            self.recent.clear();
        }
        if self.recent.len() == HISTORY {
            self.recent.pop_front();
        }
        self.recent.push_back(sample);
        if reason.is_some()
            && self
                .last_log
                .is_none_or(|last| now.saturating_duration_since(last) >= COOLDOWN)
        {
            self.last_log = Some(now);
            reason
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn real_panel_wheel_and_unexpected_painted_jump_are_recorded(cx: &mut gpui::TestAppContext) {
        use gpui::{point, px};
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("scroll-diagnostics", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items = (0..30)
                .map(|_| crate::panel::Item::Assistant("Long row\n\n".repeat(30)))
                .collect();
            cx.notify();
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| panel.scroll_transcript_direct(300.0, cx));
        vcx.run_until_parked();
        // Reset just the recorder so initial fixture navigation is not evidence.
        panel.update(vcx, |panel, cx| {
            panel.flicker_diagnostics.borrow_mut().scroll = Recorder::default();
            cx.notify();
        });
        vcx.run_until_parked();
        let position = vcx.debug_bounds("transcript").unwrap().center();
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(20.0))),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            let detector = panel.flicker_diagnostics.borrow();
            assert!(
                detector
                    .scroll
                    .recent
                    .iter()
                    .any(|sample| sample.input_events == 1 && sample.precise)
            );
            assert!(detector.scroll.last_log.is_none());
        });
        // Inject a real ListState displacement without a matching input step.
        panel.update(vcx, |panel, cx| {
            panel.cancel_transcript_momentum();
            panel.transcript_list.scroll_by(px(100.0));
            cx.notify();
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert!(
                panel.flicker_diagnostics.borrow().scroll.last_log.is_some(),
                "unexpected painted displacement must reach the mounted logger"
            );
        });
    }

    #[test]
    fn missing_anchor_still_detects_uncommanded_row_jump() {
        let now = Instant::now();
        let mut recorder = Recorder::default();
        recorder.observe(sample(0.0), now);
        recorder.input(5.0, true, now);
        let jumped = Sample {
            row: 50,
            observed_px: None,
            ..sample(0.0)
        };
        assert_eq!(
            recorder.observe(jumped, now),
            Some("logical_scroll_discontinuity")
        );
    }

    fn sample(movement: f32) -> Sample {
        Sample {
            observed_px: Some(movement),
            rows: 100,
            viewport: [0, 0, 600, 400],
            ..Sample::default()
        }
    }

    #[test]
    fn jump_records_leadup_and_coalesced_input() {
        let now = Instant::now();
        let mut recorder = Recorder::default();
        recorder.observe(sample(0.0), now);
        recorder.input(20.0, true, now);
        recorder.input(10.0, true, now);
        recorder.applied(8.0, now);
        assert_eq!(
            recorder.observe(sample(-50.0), now + Duration::from_millis(16)),
            Some("painted_movement_mismatch")
        );
        assert_eq!(recorder.recent.len(), 2);
        let last = recorder.recent.back().unwrap();
        assert_eq!(last.input_px, 30.0);
        assert_eq!(last.input_events, 2);
        assert_eq!(last.applied_px, 8.0);
        assert!(last.precise);
    }

    #[test]
    fn normal_motion_direction_changes_and_estimate_changes_are_quiet() {
        let now = Instant::now();
        let mut recorder = Recorder::default();
        recorder.observe(sample(0.0), now);
        for frame in 1..100 {
            let time = now + Duration::from_millis(frame * 16);
            let movement = if frame % 2 == 0 { 80.0 } else { -80.0 };
            recorder.input(movement, false, time);
            recorder.applied(movement, time);
            let mut sample = sample(movement);
            sample.estimated_scroll_y = frame as f32 * 1000.0;
            assert_eq!(recorder.observe(sample, time), None);
        }
        assert_eq!(recorder.recent.len(), HISTORY);
    }

    #[test]
    fn detects_stalls_but_not_idle_resumes_or_hidden_windows() {
        let now = Instant::now();
        let mut recorder = Recorder::default();
        recorder.observe(sample(0.0), now);
        recorder.input(10.0, true, now);
        recorder.applied(10.0, now);
        assert_eq!(
            recorder.observe(sample(10.0), now + Duration::from_millis(100)),
            None
        );
        recorder.applied(10.0, now + Duration::from_millis(200));
        assert_eq!(
            recorder.observe(sample(10.0), now + Duration::from_millis(200)),
            Some("scroll_frame_gap")
        );
        recorder.input(10.0, true, now + Duration::from_secs(2));
        assert_eq!(
            recorder.observe(sample(1000.0), now + Duration::from_secs(2)),
            None
        );
        assert_eq!(recorder.recent.len(), 1);
    }

    #[test]
    fn dragging_and_inactive_changes_are_not_scroll_anomalies() {
        let now = Instant::now();
        let mut recorder = Recorder::default();
        recorder.observe(sample(0.0), now);
        assert_eq!(recorder.observe(sample(500.0), now), None);
        recorder.input(20.0, true, now);
        let mut dragging = sample(500.0);
        dragging.dragging = true;
        assert_eq!(recorder.observe(dragging, now), None);
        assert_eq!(recorder.observe(sample(500.0), now), None);
    }

    #[test]
    fn repeated_jumps_are_rate_limited_and_history_stays_bounded() {
        let now = Instant::now();
        let mut recorder = Recorder::default();
        let mut reports = 0;
        for frame in 0..250 {
            let time = now + Duration::from_millis(frame * 100);
            recorder.input(1.0, true, time);
            reports += usize::from(recorder.observe(sample(100.0), time).is_some());
        }
        assert_eq!(reports, 3);
        assert_eq!(recorder.recent.len(), HISTORY);
    }
}
