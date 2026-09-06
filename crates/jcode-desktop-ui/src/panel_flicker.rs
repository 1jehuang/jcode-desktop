//! Passive, bounded geometry diagnostics. Never schedules or invalidates a frame.
//!
//! An ABAB pattern is evidence of alternating layout, not proof of its cause.
//! Logs contain numeric entity IDs and integer-pixel geometry only, never session
//! names, paths, draft text, image bytes, or transcript contents.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui::{AnyElement, Bounds, EntityId, ListState, Pixels, canvas, prelude::*};

const MAX_SAMPLE_GAP: Duration = Duration::from_millis(500);
const LOG_COOLDOWN: Duration = Duration::from_secs(10);

type Rect = [i32; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    viewport: Rect,
    input: Option<Rect>,
    pinned_prompt: bool,
}

#[derive(Default)]
pub(super) struct Detector {
    // Fixed-size storage even during an indefinitely oscillating layout.
    recent: [Option<Geometry>; 4],
    last_sample: Option<Instant>,
    last_log: Option<Instant>,
}

impl Detector {
    fn observe(&mut self, geometry: Geometry, now: Instant) -> Option<[Geometry; 2]> {
        if self
            .last_sample
            .is_some_and(|last| now.saturating_duration_since(last) > MAX_SAMPLE_GAP)
        {
            self.recent = [None; 4];
        }
        self.last_sample = Some(now);
        self.recent.rotate_left(1);
        self.recent[3] = Some(geometry);
        let [Some(a), Some(b), Some(c), Some(d)] = self.recent else {
            return None;
        };
        if a == b || a != c || b != d {
            return None;
        }
        if self
            .last_log
            .is_some_and(|last| now.saturating_duration_since(last) < LOG_COOLDOWN)
        {
            return None;
        }
        self.last_log = Some(now);
        Some([a, b])
    }
}

fn relative_rect(bounds: Bounds<Pixels>, panel: Bounds<Pixels>) -> Rect {
    // Ignore subpixel raster noise and workspace camera translation. Detect
    // changes within the panel, not the panel moving around the workspace.
    [
        f32::from(bounds.origin.x - panel.origin.x).round() as i32,
        f32::from(bounds.origin.y - panel.origin.y).round() as i32,
        f32::from(bounds.size.width).round() as i32,
        f32::from(bounds.size.height).round() as i32,
    ]
}

pub(super) fn observer(
    detector: Rc<RefCell<Detector>>,
    list: ListState,
    input: Rc<Cell<Option<Bounds<Pixels>>>>,
    pinned_prompt: bool,
    entity_id: EntityId,
) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |panel_bounds, _, window, _| {
            // Offscreen panels may still be laid out. Do not report invisible
            // geometry as a user-visible flicker occurrence.
            if !panel_bounds.intersects(&window.content_mask().bounds) {
                return;
            }
            let geometry = Geometry {
                viewport: relative_rect(list.viewport_bounds(), panel_bounds),
                input: input.get().map(|bounds| relative_rect(bounds, panel_bounds)),
                pinned_prompt,
            };
            if let Some([a, b]) = detector.borrow_mut().observe(geometry, Instant::now()) {
                let timestamp_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                eprintln!(
                    "jcode desktop: panel_geometry_oscillation timestamp_ms={timestamp_ms} panel={entity_id:?} pattern=ABAB a={a:?} b={b:?} cooldown_ms={}",
                    LOG_COOLDOWN.as_millis()
                );
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(height: i32, pinned_prompt: bool) -> Geometry {
        Geometry {
            viewport: [0, 0, 600, height],
            input: Some([0, height, 600, 112]),
            pinned_prompt,
        }
    }

    #[test]
    fn abab_logs_both_geometries_only_after_four_samples() {
        let mut detector = Detector::default();
        let now = Instant::now();
        let a = geometry(300, false);
        let b = geometry(180, true);
        for sample in [a, b, a] {
            assert_eq!(detector.observe(sample, now), None);
        }
        assert_eq!(detector.observe(b, now), Some([a, b]));
    }

    #[test]
    fn steady_and_monotonic_layouts_do_not_log() {
        let mut detector = Detector::default();
        let now = Instant::now();
        for _ in 0..100 {
            assert_eq!(detector.observe(geometry(300, false), now), None);
        }
        for height in 0..100 {
            assert_eq!(detector.observe(geometry(height, false), now), None);
        }
    }

    #[test]
    fn continuous_oscillation_is_rate_limited_per_panel() {
        let mut detector = Detector::default();
        let start = Instant::now();
        let a = geometry(300, false);
        let b = geometry(180, true);
        let mut logs = Vec::new();
        for frame in 0..250_u64 {
            let at = start + Duration::from_millis(frame * 100);
            if detector
                .observe(if frame % 2 == 0 { a } else { b }, at)
                .is_some()
            {
                logs.push(at);
            }
        }
        assert_eq!(logs.len(), 3);
        assert!(
            logs.windows(2)
                .all(|pair| pair[1] - pair[0] >= LOG_COOLDOWN)
        );
        assert_eq!(detector.recent.len(), 4);
    }

    #[test]
    fn idle_gap_breaks_a_pattern() {
        let mut detector = Detector::default();
        let start = Instant::now();
        let a = geometry(300, false);
        let b = geometry(180, true);
        for sample in [a, b, a] {
            assert_eq!(detector.observe(sample, start), None);
        }
        assert_eq!(
            detector.observe(b, start + MAX_SAMPLE_GAP + Duration::from_millis(1)),
            None
        );
    }

    #[test]
    fn pin_only_oscillation_is_detected_and_panels_are_independent() {
        let mut first = Detector::default();
        let mut second = Detector::default();
        let now = Instant::now();
        let a = geometry(300, false);
        let b = geometry(300, true);
        for detector in [&mut first, &mut second] {
            for sample in [a, b, a] {
                assert_eq!(detector.observe(sample, now), None);
            }
            assert_eq!(detector.observe(b, now), Some([a, b]));
        }
    }

    #[test]
    fn rects_ignore_panel_translation_and_round_subpixels() {
        let panel = Bounds::new(
            gpui::point(gpui::px(40.), gpui::px(80.)),
            gpui::size(gpui::px(600.), gpui::px(500.)),
        );
        let bounds = Bounds::new(
            gpui::point(gpui::px(40.2), gpui::px(100.2)),
            gpui::size(gpui::px(599.8), gpui::px(299.8)),
        );
        assert_eq!(relative_rect(bounds, panel), [0, 20, 600, 300]);
    }
}
