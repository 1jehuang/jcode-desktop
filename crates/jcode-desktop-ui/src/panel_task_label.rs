//! A bounded, grapheme-safe type-in for the pinned task label.
//!
//! The label owns its clock, so unrelated transcript renders cannot restart it.
//! Only painted labels arm a tick. Hidden cards stop after at most one tick.
use std::time::Duration;

use gpui::{Context, Render, Task, Window, canvas, div, prelude::*};
use unicode_segmentation::UnicodeSegmentation;

const TICK: Duration = Duration::from_millis(24);

#[derive(Default)]
struct Reveal {
    text: String,
    ends: Vec<usize>,
    elapsed: Duration,
    duration: Duration,
}

impl Reveal {
    fn set_text(&mut self, text: String, reduce_motion: bool) -> bool {
        if self.text == text {
            return false;
        }
        self.ends = text
            .grapheme_indices(true)
            .map(|(start, grapheme)| start + grapheme.len())
            .collect();
        self.duration = Duration::from_millis((self.ends.len() as u64 * 12).clamp(180, 420));
        self.elapsed = if reduce_motion {
            self.duration
        } else {
            Duration::ZERO
        };
        self.text = text;
        true
    }

    fn active(&self) -> bool {
        !self.text.is_empty() && self.elapsed < self.duration
    }

    fn finish(&mut self) {
        self.elapsed = self.duration;
    }

    fn advance(&mut self) {
        self.elapsed = (self.elapsed + TICK).min(self.duration);
    }

    fn visible(&self) -> &str {
        if !self.active() {
            return &self.text;
        }
        // Show one complete grapheme immediately, never a blank flashing card.
        let count = 1
            + ((self.ends.len().saturating_sub(1) as f64) * self.elapsed.as_secs_f64()
                / self.duration.as_secs_f64()) as usize;
        &self.text[..self.ends[count - 1]]
    }
}

pub(super) struct TypeInLabel {
    reveal: Reveal,
    tick: Option<Task<()>>,
}

impl TypeInLabel {
    pub(super) fn new(_: &mut Context<Self>) -> Self {
        Self {
            reveal: Reveal::default(),
            tick: None,
        }
    }

    pub(super) fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        let reduce_motion = cx.reduce_motion() || crate::config::get().appearance.reduce_motion;
        if self.reveal.set_text(text, reduce_motion) {
            // Dropping the previous task prevents rapid updates from advancing
            // the replacement label with an older label's pending timer.
            self.tick = None;
            cx.notify();
        }
    }

    fn arm(&mut self, cx: &mut Context<Self>) {
        if cx.reduce_motion() || crate::config::get().appearance.reduce_motion {
            self.tick = None;
            self.reveal.finish();
            return;
        }
        if self.tick.is_some() || !self.reveal.active() {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TICK).await;
            let _ = this.update(cx, |label, cx| {
                label.tick = None;
                label.reveal.advance();
                cx.notify();
            });
        }));
    }
}

impl Render for TypeInLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if cx.reduce_motion() || crate::config::get().appearance.reduce_motion {
            self.tick = None;
            self.reveal.finish();
        }
        let label = cx.entity().downgrade();
        div()
            .debug_selector(|| "pinned-todo-animated-label".into())
            .relative()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.reveal.visible().to_owned())
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if bounds.intersects(&window.content_mask().bounds) {
                            let _ = label.update(cx, |label, cx| label.arm(cx));
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_label_reveals_and_finishes_with_a_bounded_clock() {
        for text in ["A", "Implement task label", &"long title ".repeat(100)] {
            let mut reveal = Reveal::default();
            assert!(reveal.set_text(text.into(), false));
            assert!(!reveal.visible().is_empty());
            assert!(reveal.duration <= Duration::from_millis(420));
            let mut previous = 0;
            for _ in 0..18 {
                assert!(reveal.visible().len() >= previous);
                previous = reveal.visible().len();
                reveal.advance();
            }
            assert_eq!(reveal.visible(), text);
            assert!(!reveal.active());
        }
    }

    #[test]
    fn task_label_unchanged_text_does_not_restart_and_replacement_does() {
        let mut reveal = Reveal::default();
        reveal.set_text("First task".into(), false);
        reveal.advance();
        let elapsed = reveal.elapsed;
        assert!(!reveal.set_text("First task".into(), false));
        assert_eq!(reveal.elapsed, elapsed);
        assert!(reveal.set_text("Next task".into(), false));
        assert_eq!(reveal.visible(), "N");
        assert_eq!(reveal.elapsed, Duration::ZERO);
        reveal.finish();
        assert!(!reveal.set_text("Next task".into(), false));
        assert!(!reveal.active());
    }

    #[test]
    fn task_label_preserves_combining_marks_and_emoji_clusters() {
        let text = "👩🏽‍💻 e\u{301}lan 🇺🇸";
        let mut reveal = Reveal::default();
        reveal.set_text(text.into(), false);
        assert_eq!(reveal.visible(), "👩🏽‍💻");
        let boundaries: Vec<_> = text
            .grapheme_indices(true)
            .map(|(start, grapheme)| start + grapheme.len())
            .collect();
        for _ in 0..18 {
            assert!(boundaries.contains(&reveal.visible().len()));
            reveal.advance();
        }
        assert_eq!(reveal.visible(), text);
    }

    #[test]
    fn task_label_reduced_motion_empty_and_completion_labels_are_immediate_or_safe() {
        let mut reveal = Reveal::default();
        reveal.set_text("All tasks complete".into(), true);
        assert_eq!(reveal.visible(), "All tasks complete");
        assert!(!reveal.active());
        reveal.set_text("No active task".into(), false);
        reveal.finish();
        assert_eq!(reveal.visible(), "No active task");
        assert!(!reveal.active());
        reveal.set_text(String::new(), false);
        assert_eq!(reveal.visible(), "");
        assert!(!reveal.active());
    }

    #[gpui::test]
    fn task_label_timer_is_paint_armed_and_cancelled_on_replacement(cx: &mut gpui::TestAppContext) {
        let label = cx.new(TypeInLabel::new);
        label.update(cx, |label, cx| {
            label.set_text("Original title".into(), cx);
            assert!(
                label.tick.is_none(),
                "setting text alone must not run a hidden clock"
            );
            label.arm(cx);
            assert!(label.tick.is_some());
            label.set_text("Replacement title".into(), cx);
            assert!(label.tick.is_none());
            assert_eq!(label.reveal.visible(), "R");
            label.reveal.finish();
            label.arm(cx);
            assert!(
                label.tick.is_none(),
                "finished labels must not keep repainting"
            );
        });
    }

    #[gpui::test]
    fn task_label_clock_stops_without_paint_and_respects_motion_toggle(
        cx: &mut gpui::TestAppContext,
    ) {
        let label = cx.new(TypeInLabel::new);
        label.update(cx, |label, cx| {
            label.set_text("A visible task".into(), cx);
            label.arm(cx);
            label.arm(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        label.read_with(cx, |label, _| {
            assert_eq!(label.reveal.elapsed, TICK, "only one timer should run");
            assert!(label.tick.is_none());
        });
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        label.read_with(cx, |label, _| assert_eq!(label.reveal.elapsed, TICK));
        label.update(cx, |label, cx| label.arm(cx));
        cx.update(|cx| cx.set_reduce_motion(true));
        label.update(cx, |label, cx| {
            label.arm(cx);
            assert!(label.tick.is_none());
            assert_eq!(label.reveal.visible(), "A visible task");
            label.set_text("New reduced motion task".into(), cx);
            assert_eq!(label.reveal.visible(), "New reduced motion task");
            assert!(!label.reveal.active());
        });
        cx.update(|cx| cx.set_reduce_motion(false));
        label.update(cx, |label, cx| {
            label.arm(cx);
            assert!(
                label.tick.is_none(),
                "motion toggle must not replay a completed label"
            );
        });
    }

    #[gpui::test]
    fn task_label_real_pinned_card_paint_runs_animation_to_completion(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::super::Item;
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("label-paint", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items = vec![
                Item::User("Animate the current task".into()),
                Item::Tool {
                    call_id: "label-todo".into(), name: "todo".into(), input: "{}".into(),
                    output: r#"[{"id":"one","content":"Keep the task animation moving","status":"in_progress","priority":"high"}]"#.into(),
                    done: true, error: None,
                },
            ];
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pinned-todo-animated-label").is_some());
        let label = panel.read_with(vcx, |panel, _| panel.pinned_task_label.clone());
        label.read_with(vcx, |label, _| {
            assert!(
                label.tick.is_some(),
                "actual card paint must arm the first timer"
            );
        });
        for _ in 0..20 {
            vcx.executor().advance_clock(TICK);
            vcx.run_until_parked();
        }
        label.read_with(vcx, |label, _| {
            assert_eq!(label.reveal.visible(), "Keep the task animation moving");
            assert!(!label.reveal.active());
            assert!(label.tick.is_none());
        });
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Tool {
                call_id: "completed-label-todo".into(),
                name: "todo".into(),
                input: "{}".into(),
                output: r#"[{"content":"Keep the task animation moving","status":"completed","group":"Todo group header"}]"#.into(),
                done: true,
                error: None,
            }];
            cx.notify();
        });
        for _ in 0..20 {
            vcx.run_until_parked();
            vcx.executor().advance_clock(TICK);
        }
        vcx.run_until_parked();
        label.read_with(vcx, |label, _| {
            assert_eq!(label.reveal.visible(), "Todo group header");
            assert!(!label.reveal.active());
        });
    }
}
