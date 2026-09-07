//! A small activity clock, armed only when its spinner is actually painted.
//!
//! Never request a display-rate animation for a long-running agent. Eight steps
//! per second are enough for this discrete spinner. A clipped/hidden spinner
//! stops rearming, and reduced motion keeps the same static activity marker.

use std::time::Duration;

use gpui::{Context, Render, Task, Window, canvas, div, prelude::*, px};

use crate::theme::Theme;

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
        if self.tick.is_some() || reduce_motion {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TICK).await;
            let _ = this.update(cx, |spinner, cx| {
                spinner.tick = None;
                spinner.step = (spinner.step + 1) % 8;
                cx.notify();
            });
        }));
    }
}

fn dot_opacity(dot: usize, step: usize) -> f32 {
    1.0 - ((step + 8 - dot) % 8) as f32 * 0.10
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
            .children((0..8).map(|dot| {
                let angle = dot as f32 * std::f32::consts::TAU / 8.0;
                div()
                    .absolute()
                    .left(px(6.0 + angle.sin() * 5.0))
                    .top(px(6.0 - angle.cos() * 5.0))
                    .size(px(2.0))
                    .rounded_full()
                    .bg(Theme::global().ACCENT.opacity(dot_opacity(dot, step)))
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if bounds.intersects(&window.content_mask().bounds) {
                            let _ = spinner.update(cx, |spinner, cx| {
                                spinner.arm(crate::config::get().appearance.reduce_motion, cx)
                            });
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

    #[test]
    fn spinner_highlight_rotates_and_wraps() {
        for step in 0..16 {
            for dot in 0..8 {
                let opacity = dot_opacity(dot, step);
                assert!((0.29..=1.0).contains(&opacity));
                assert_eq!(opacity == 1.0, dot == step % 8);
            }
        }
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
                let activity = vcx.debug_bounds("transcript-activity").expect("activity paints");
                let content_rows = panel.read_with(vcx, |panel, _| {
                    assert_eq!(panel.transcript_row_count, panel.transcript_render_rows().len() + 1);
                    panel.transcript_render_rows().len()
                });
                if content_rows > 0 {
                    let last = vcx.debug_bounds(format!("transcript-row-{}", content_rows - 1).leak())
                        .expect("last content row paints");
                    assert!(activity.top() >= last.bottom(), "spinner needs its own tail line");
                }
                assert!(vcx.debug_bounds("panel-status-spinner").is_none());
            }
            panel.update(vcx, |panel, cx| match terminal {
                "done" => panel.apply(&ApiEvent::TurnDone { session_id: "tail-test".into() }, cx),
                "error" => panel.apply(&ApiEvent::Error {
                    code: jcode_sdk::api::ErrorCode::Internal,
                    message: "Provider failed".into(),
                }, cx),
                "failed" => panel.message_failed("Send failed".into(), cx),
                _ => panel.apply(&ApiEvent::SessionStatus {
                    session_id: "tail-test".into(), status: terminal.into(),
                }, cx),
            });
            vcx.run_until_parked();
            assert!(vcx.debug_bounds("transcript-activity").is_none(), "{terminal}");
            assert!(vcx.debug_bounds("panel-activity-spinner").is_none(), "{terminal}");
            panel.read_with(vcx, |panel, _| {
                assert!(!panel.activity_active(), "{terminal}");
                assert!(panel.streaming_reasoning.is_empty());
                assert!(panel.streaming_text.is_empty());
                assert_eq!(panel.transcript_row_count, panel.transcript_render_rows().len());
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

        for (status, expected) in [
            ("running", "Working"),
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
            assert!(vcx.debug_bounds("panel-activity-label").is_none());
            let spinner = vcx.debug_bounds("transcript-activity").unwrap();
            let transcript = vcx.debug_bounds("transcript").unwrap();
            assert!(spinner.top() >= transcript.top());
            assert!(spinner.bottom() <= transcript.bottom());
            assert!(vcx.debug_bounds("panel-status-spinner").is_none());
        }
        panel.update(vcx, |panel, cx| {
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
