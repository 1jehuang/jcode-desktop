//! Chunked reveal for live reasoning and response text.
//!
//! Providers deliver text a few tokens at a time. Painting every token, or
//! pacing a word-by-word reveal, keeps the transcript in constant motion: each
//! word nudges the line, each wrap pushes everything below it down. Instead
//! the received text is released in whole blocks at natural boundaries:
//!
//! * as soon as a line or paragraph completes,
//! * after a short hold, up to the last sentence (or word) of a long line,
//! * and immediately once the stream pauses.
//!
//! Releases are rate limited, so the transcript changes a few times per second
//! at most, and each released block fades in as a unit.

use std::time::{Duration, Instant};

use crate::markdown::StreamFade;

/// Minimum time between two releases. Bounds how often the transcript moves.
const MIN_INTERVAL: Duration = Duration::from_millis(140);
/// How long unreleased text without a line break may wait before the reveal
/// settles for a sentence or word boundary instead.
const HOLD: Duration = Duration::from_millis(260);
/// The stream has paused when nothing arrived for this long. Release it all.
const IDLE: Duration = Duration::from_millis(160);
/// How long a released block takes to fade in.
const FADE: Duration = Duration::from_millis(220);
/// Opacity deficit a block starts its fade at.
const FADE_FROM: f32 = 0.85;
/// Larger backlogs (history restores, reconnect recovery) appear immediately.
const SNAP_BACKLOG: usize = 6_000;

#[derive(Debug, Clone, Default)]
pub(super) struct StreamReveal {
    /// Released prefix length in bytes, always on a char boundary.
    shown: usize,
    /// Start of the newest released block, which fades in.
    fade_from: usize,
    /// Current fade of the newest block, 0 when fully opaque.
    fade: f32,
    target: usize,
    released_at: Option<Instant>,
    /// When the oldest unreleased byte arrived.
    pending_since: Option<Instant>,
    /// When the received text last grew.
    grew_at: Option<Instant>,
    /// Only a rendered, animating stream is chunked. Until then (tests,
    /// reduced motion, rows read outside a frame) the full text is shown.
    engaged: bool,
}

impl StreamReveal {
    /// Show `len` bytes immediately, for text that did not arrive as a live stream.
    pub(super) fn snap(&mut self, len: usize) {
        *self = Self {
            shown: len,
            fade_from: len,
            target: len,
            ..Self::default()
        };
    }

    /// Advance toward the received `text`. Returns true while still animating.
    pub(super) fn tick(&mut self, text: &str, now: Instant, instant: bool) -> bool {
        let len = text.len();
        if len < self.target || len == 0 || self.shown > len {
            // Cleared or replaced: the live row starts over.
            self.snap(0);
        }
        if len > self.target {
            self.grew_at = Some(now);
        }
        self.target = len;
        if instant || len - self.shown > SNAP_BACKLOG {
            self.snap(len);
            return false;
        }
        self.engaged = true;

        if self.shown < len {
            let pending_since = *self.pending_since.get_or_insert(now);
            let rested = self
                .released_at
                .is_none_or(|at| now.saturating_duration_since(at) >= MIN_INTERVAL);
            if rested {
                let paused = self
                    .grew_at
                    .is_none_or(|at| now.saturating_duration_since(at) >= IDLE);
                let held = now.saturating_duration_since(pending_since) >= HOLD;
                if let Some(cut) = release_point(text, self.shown, paused, held) {
                    self.fade_from = self.shown;
                    self.shown = cut;
                    self.released_at = Some(now);
                    self.pending_since = (cut < len).then_some(now);
                }
            }
        }

        let progress = self.released_at.map_or(1.0, |at| {
            now.saturating_duration_since(at).as_secs_f32() / FADE.as_secs_f32()
        });
        self.fade = if progress >= 1.0 || self.fade_from >= self.shown {
            0.0
        } else {
            // Ease out: most of the block is legible almost at once.
            FADE_FROM * (1.0 - progress).powi(2)
        };
        self.shown < len || self.fade > 0.0
    }

    /// The released prefix.
    pub(super) fn visible<'a>(&self, text: &'a str) -> &'a str {
        if !self.engaged {
            return text;
        }
        &text[..self.shown.min(text.len())]
    }

    /// Bytes at the end of the visible prefix of `text` still fading in.
    pub(super) fn fading(&self, text: &str) -> usize {
        if !self.engaged || self.fade <= 0.0 {
            return 0;
        }
        self.shown.min(text.len()).saturating_sub(self.fade_from)
    }

    /// The fade applied to the newest block of `text` when painting it.
    pub(super) fn fade(&self, text: &str) -> StreamFade {
        StreamFade {
            bytes: self.fading(text),
            amount: self.fade,
        }
    }

    #[cfg(test)]
    pub(super) fn shown_len(&self) -> usize {
        if !self.engaged {
            return self.target;
        }
        self.shown
    }
}

/// Where to cut the next release of `text` beyond `from`, if anywhere yet.
fn release_point(text: &str, from: usize, paused: bool, held: bool) -> Option<usize> {
    let pending = &text[from..];
    if pending.is_empty() {
        return None;
    }
    if paused {
        return Some(text.len());
    }
    // A completed line or paragraph is a block: release it right away.
    if let Some(newline) = pending.rfind('\n') {
        return Some(from + newline + 1);
    }
    if !held {
        return None;
    }
    // A long line is still arriving. Settle for its last full sentence, else
    // its last full word, so the line grows in phrases rather than letters.
    let sentence = pending
        .match_indices([' ', '\t'])
        .rev()
        .find(|(at, _)| {
            pending[..*at]
                .chars()
                .next_back()
                .is_some_and(|c| matches!(c, '.' | '!' | '?' | ':' | ';'))
        })
        .map(|(at, _)| at + 1);
    let word = pending.rfind([' ', '\t']).map(|at| at + 1);
    sentence.or(word).map(|cut| from + cut).filter(|&cut| cut > from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    #[test]
    fn completed_lines_are_released_as_whole_blocks() {
        let mut reveal = StreamReveal::default();
        let start = Instant::now();
        let text = "First paragraph is done.\n\nSecond is still arr";
        assert!(reveal.tick(text, start, false));
        assert_eq!(reveal.visible(text), "First paragraph is done.\n\n");
        assert!(reveal.fading(text) > 0, "the new block fades in");
        assert!(reveal.fade(text).amount > 0.5);
    }

    #[test]
    fn partial_lines_wait_then_release_at_a_sentence() {
        let mut reveal = StreamReveal::default();
        let mut now = Instant::now();
        let mut text = String::new();
        // A steady stream with no line break: tokens every frame.
        let words = "This sentence ends here. And this one keeps going on and on ".repeat(3);
        let mut first_release = None;
        for (frame, word) in words.split_inclusive(' ').enumerate() {
            text.push_str(word);
            reveal.tick(&text, now, false);
            if first_release.is_none() && reveal.shown_len() > 0 {
                first_release = Some((frame, reveal.visible(&text).to_owned()));
            }
            now += FRAME;
        }
        let (frame, shown) = first_release.expect("released while streaming");
        assert!(frame as u32 * 16 >= HOLD.as_millis() as u32, "held first");
        assert!(
            shown.ends_with(". ") || shown.ends_with(' '),
            "cut at a boundary: {shown:?}"
        );
    }

    #[test]
    fn releases_are_rate_limited() {
        let mut reveal = StreamReveal::default();
        let mut now = Instant::now();
        let mut text = String::new();
        let mut releases = 0;
        let mut previous = 0;
        // A new line every frame for a second.
        for n in 0..60 {
            text.push_str(&format!("line {n}\n"));
            reveal.tick(&text, now, false);
            if reveal.shown_len() != previous {
                releases += 1;
                previous = reveal.shown_len();
            }
            now += FRAME;
        }
        assert!(releases <= 8, "{releases} releases in a second");
        assert!(text.len() - reveal.shown_len() < 80, "kept up eagerly");
    }

    #[test]
    fn a_paused_stream_releases_everything_and_settles_opaque() {
        let mut reveal = StreamReveal::default();
        let start = Instant::now();
        let text = "no boundary at all yet";
        assert!(reveal.tick(text, start, false));
        assert_eq!(reveal.shown_len(), 0);
        let mut now = start;
        for _ in 0..60 {
            now += FRAME;
            reveal.tick(text, now, false);
        }
        assert_eq!(reveal.visible(text), text);
        assert_eq!(reveal.fading(text), 0);
        assert!(!reveal.tick(text, now + FRAME, false));
    }

    #[test]
    fn clears_restores_and_reduced_motion_snap() {
        let mut reveal = StreamReveal::default();
        let now = Instant::now();
        reveal.tick(&"x".repeat(100), now, false);
        assert!(!reveal.tick("", now, false));
        assert_eq!(reveal.shown_len(), 0);

        let big = "y".repeat(SNAP_BACKLOG + 1);
        assert!(!reveal.tick(&big, now, false));
        assert_eq!(reveal.shown_len(), SNAP_BACKLOG + 1);

        let mut reduced = StreamReveal::default();
        let text = "x".repeat(50);
        assert!(!reduced.tick(&text, now, true));
        assert_eq!((reduced.shown_len(), reduced.fading(&text)), (50, 0));
        assert_eq!(reduced.visible(&text), text, "unengaged passes text through");
    }

    #[test]
    fn release_points_respect_char_boundaries() {
        let text = "αβ γδ. εζ";
        let cut = release_point(text, 0, false, true).unwrap();
        assert!(text.is_char_boundary(cut));
        assert_eq!(&text[..cut], "αβ γδ. ");
        assert_eq!(release_point("αβγ", 0, false, true), None);
        assert_eq!(release_point("αβγ", 0, true, false), Some("αβγ".len()));
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

    fn frames(vcx: &mut gpui::VisualTestContext, count: usize) {
        for _ in 0..count {
            vcx.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            vcx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    #[gpui::test]
    fn streamed_blocks_appear_whole_and_settle(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("stream-reveal", cx);
            workspace
        });
        let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        let reasoning = "Weighing the options.\n\nStill weigh";
        let answer = "Here is a considered paragraph.\n\nAnd a second one still arriv";
        panel.update(vcx, |panel, cx| {
            panel.animate_stream_in_tests = true;
            panel.items.clear();
            panel.apply(
                &ApiEvent::ReasoningDelta {
                    session_id: panel.session_id.clone(),
                    text: reasoning.into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let partial = panel
            .read_with(vcx, |panel, _| live_text(panel, usize::MAX - 1))
            .expect("live reasoning row");
        assert_eq!(partial, "Weighing the options.\n\n", "whole block first");
        assert!(panel.read_with(vcx, |panel, _| {
            panel.reasoning_reveal.fading(&panel.streaming_reasoning) > 0
        }));

        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: panel.session_id.clone(),
                    text: answer.into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let partial = panel
            .read_with(vcx, |panel, _| live_text(panel, usize::MAX))
            .expect("live response row");
        assert!(partial.len() < answer.len());

        frames(vcx, 60);
        panel.read_with(vcx, |panel, _| {
            assert_eq!(live_text(panel, usize::MAX).as_deref(), Some(answer));
            assert_eq!(panel.text_reveal.fading(&panel.streaming_text), 0);
        });
    }
}
