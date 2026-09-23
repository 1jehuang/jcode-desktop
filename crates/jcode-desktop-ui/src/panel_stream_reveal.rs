//! Paced reveal for live reasoning and response text.
//!
//! Providers deliver text in bursts: several words, a pause, then a paragraph.
//! Painting each burst verbatim makes the transcript jump. Instead the visible
//! prefix chases the received text at a rate proportional to the backlog, so
//! bursts become a steady flow that never falls far behind. A trailing window
//! of freshly revealed bytes fades in, and it finishes fading shortly after the
//! stream pauses so the text settles to full opacity.

use std::time::{Duration, Instant};

/// Time constant of the reveal. The visible prefix covers most of any backlog
/// in about this long, which keeps latency imperceptible while smoothing bursts.
const CATCH_UP: f64 = 0.14;
/// Minimum reveal speed so short trailing fragments do not crawl.
const MIN_RATE: f64 = 90.0;
/// Larger backlogs (history restores, reconnect recovery) appear immediately.
const SNAP_BACKLOG: usize = 6_000;
/// Longest fading tail, in bytes. Keeps fast streams legible.
const MAX_FADE: f64 = 48.0;
/// How quickly the fading tail settles once the reveal stops advancing.
const SETTLE: f64 = 0.22;
/// Ignore pathological frame gaps (suspend, debugger) so the reveal does not leap.
const MAX_STEP: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Default)]
pub(super) struct StreamReveal {
    /// Visible prefix length in bytes, kept fractional for smooth low rates.
    shown: f64,
    /// Bytes before this point are fully opaque. Trails `shown`.
    settled: f64,
    target: usize,
    last_tick: Option<Instant>,
    /// Only a rendered, animating stream is paced. Until then (tests, reduced
    /// motion, rows read outside a frame) the full received text is shown.
    engaged: bool,
}

impl StreamReveal {
    /// Show `len` bytes immediately, for text that did not arrive as a live stream.
    pub(super) fn snap(&mut self, len: usize) {
        self.shown = len as f64;
        self.settled = len as f64;
        self.target = len;
        self.last_tick = None;
        self.engaged = false;
    }

    /// Advance toward `len` received bytes. Returns true while still animating.
    pub(super) fn tick(&mut self, len: usize, now: Instant, instant: bool) -> bool {
        if len < self.target || len == 0 {
            // Cleared or replaced: the live row starts over.
            self.snap(0);
        }
        self.target = len;
        if instant || len - (self.shown as usize).min(len) > SNAP_BACKLOG {
            self.snap(len);
            return false;
        }
        self.engaged = true;
        let dt = self
            .last_tick
            .map(|last| now.saturating_duration_since(last).min(MAX_STEP))
            .unwrap_or(Duration::from_millis(16))
            .as_secs_f64();
        self.last_tick = Some(now);

        let target = len as f64;
        let backlog = target - self.shown;
        if backlog > 0.0 {
            let rate = (backlog / CATCH_UP).max(MIN_RATE);
            self.shown = (self.shown + rate * dt).min(target);
        }
        let gap = self.shown - self.settled;
        if gap > 0.0 {
            let rate = (gap / SETTLE).max(MIN_RATE * 0.5);
            self.settled = (self.settled + rate * dt).min(self.shown);
            self.settled = self.settled.max(self.shown - MAX_FADE);
        }
        let animating = self.shown < target || self.settled < self.shown;
        if !animating {
            self.last_tick = None;
        }
        animating
    }

    /// The revealed prefix, on a char boundary.
    pub(super) fn visible<'a>(&self, text: &'a str) -> &'a str {
        if !self.engaged {
            return text;
        }
        &text[..text.floor_char_boundary(self.shown as usize)]
    }

    /// Bytes at the end of the visible prefix that are still fading in.
    pub(super) fn fading(&self) -> usize {
        if !self.engaged {
            return 0;
        }
        (self.shown - self.settled).max(0.0).round() as usize
    }

    pub(super) fn shown_len(&self) -> usize {
        if !self.engaged {
            return self.target;
        }
        self.shown as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(reveal: &mut StreamReveal, len: usize, start: Instant, frames: u32) -> Instant {
        let mut now = start;
        for _ in 0..frames {
            now += Duration::from_millis(16);
            reveal.tick(len, now, false);
        }
        now
    }

    #[test]
    fn bursts_are_revealed_gradually_then_settle_opaque() {
        let mut reveal = StreamReveal::default();
        let start = Instant::now();
        assert!(reveal.tick(400, start, false));
        let after_one = start + Duration::from_millis(16);
        reveal.tick(400, after_one, false);
        let shown = reveal.shown_len();
        assert!(shown > 0 && shown < 400, "first frames reveal part of a burst: {shown}");
        assert!(reveal.fading() > 0);

        let now = run(&mut reveal, 400, after_one, 90);
        assert_eq!(reveal.shown_len(), 400);
        assert_eq!(reveal.fading(), 0);
        assert!(!reveal.tick(400, now + Duration::from_millis(16), false));
    }

    #[test]
    fn reveal_keeps_up_with_a_steady_stream() {
        let mut reveal = StreamReveal::default();
        let mut now = Instant::now();
        let mut len = 0;
        // 300 bytes per second in 30-byte bursts every 100 ms.
        for frame in 0..300 {
            if frame % 6 == 0 {
                len += 30;
            }
            now += Duration::from_millis(16);
            reveal.tick(len, now, false);
        }
        assert!(len - reveal.shown_len() <= 60, "lag {}", len - reveal.shown_len());
        assert!(reveal.fading() as f64 <= MAX_FADE);
    }

    #[test]
    fn clears_restores_and_reduced_motion_snap() {
        let mut reveal = StreamReveal::default();
        let now = Instant::now();
        reveal.tick(100, now, false);
        assert!(!reveal.tick(0, now, false));
        assert_eq!(reveal.shown_len(), 0);

        assert!(!reveal.tick(SNAP_BACKLOG + 1, now, false));
        assert_eq!(reveal.shown_len(), SNAP_BACKLOG + 1);

        let mut reduced = StreamReveal::default();
        assert!(!reduced.tick(50, now, true));
        assert_eq!((reduced.shown_len(), reduced.fading()), (50, 0));
    }

    #[test]
    fn visible_prefix_respects_char_boundaries() {
        let mut reveal = StreamReveal { shown: 2.0, engaged: true, ..Default::default() };
        assert_eq!(reveal.visible("αβγ"), "α");
        reveal.shown = 3.0;
        assert_eq!(reveal.visible("αβγ"), "α");
        reveal.snap(3);
        assert_eq!(reveal.visible("αβγ"), "αβγ", "unengaged reveal passes text through");
    }
}

#[cfg(test)]
mod panel_tests {
    use super::super::*;

    fn live_text(panel: &Panel, sentinel: usize) -> Option<String> {
        panel
            .transcript_render_rows()
            .into_iter()
            .find(|row| row.index == sentinel)
            .and_then(|row| match row.source {
                TranscriptRowSource::Owned(item) => match *item {
                    Item::Assistant(text) | Item::Reasoning(text) => Some(text),
                    _ => None,
                },
                _ => None,
            })
    }

    #[gpui::test]
    fn streamed_bursts_flow_in_and_settle(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("stream-reveal", cx);
            workspace
        });
        let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        let reasoning = "Weighing the options before answering. ".repeat(6);
        let answer = "Here is a considered, paragraph-sized burst of response text. ".repeat(6);
        panel.update(vcx, |panel, cx| {
            panel.animate_stream_in_tests = true;
            panel.items.clear();
            panel.apply(
                &ApiEvent::ReasoningDelta {
                    session_id: panel.session_id.clone(),
                    text: reasoning.clone(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let partial = panel.read_with(vcx, |panel, _| live_text(panel, usize::MAX - 1));
        let partial = partial.expect("live reasoning row");
        assert!(!partial.is_empty() && partial.len() < reasoning.len(), "{}", partial.len());
        assert!(panel.read_with(vcx, |panel, _| panel.reasoning_reveal.fading() > 0));

        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: panel.session_id.clone(),
                    text: answer.clone(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let partial = panel
            .read_with(vcx, |panel, _| live_text(panel, usize::MAX))
            .expect("live response row");
        assert!(partial.len() < answer.len());

        for _ in 0..120 {
            vcx.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            vcx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        panel.read_with(vcx, |panel, _| {
            assert_eq!(live_text(panel, usize::MAX).as_deref(), Some(answer.as_str()));
            assert_eq!(panel.text_reveal.fading(), 0);
        });
    }
}
