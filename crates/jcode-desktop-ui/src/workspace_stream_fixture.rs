//! An offline streaming workload for performance runs. With
//! `JCODE_DESKTOP_SCREENSHOT_STREAM=1` in screenshot mode, the demo script is
//! streamed into the first panel's session through `Workspace::apply`, the
//! path live bridge updates take, so a profile of it costs what a real agent
//! turn costs: transcript reveal, tool rows, spinners and the sidebar.
use super::*;
use jcode_base::transcript_sample::SampleTurn;

pub(super) fn enabled() -> bool {
    harness::screenshot_mode()
        && matches!(
            std::env::var("JCODE_DESKTOP_SCREENSHOT_STREAM").as_deref(),
            Ok("1" | "steady")
        )
}

/// Streams the demo script into `session_id` forever, at about the cadence a
/// provider delivers deltas.
pub(super) fn spawn(session_id: String, cx: &mut Context<Workspace>) -> gpui::Task<()> {
    const CHUNK_DELAY: Duration = Duration::from_millis(30);
    // `JCODE_DESKTOP_SCREENSHOT_STREAM=steady` leaves out the pauses that a
    // real turn has, so every sampling window measures the same load.
    let steady = std::env::var("JCODE_DESKTOP_SCREENSHOT_STREAM").as_deref() == Ok("steady");
    let (tool_run, turn_gap) = if steady {
        (CHUNK_DELAY, CHUNK_DELAY)
    } else {
        (Duration::from_millis(600), Duration::from_millis(800))
    };
    cx.spawn(async move |this, cx| {
        let executor = cx.background_executor().clone();
        let send = |event: jcode_sdk::ApiEvent, cx: &mut gpui::AsyncApp| {
            let session_id = session_id.clone();
            this.update(cx, |workspace, cx| {
                if workspace.apply(Update::Event { session_id, event }, cx) {
                    cx.notify();
                }
            })
            .is_ok()
        };
        let mut call = 0usize;
        loop {
            for turn in crate::panel::demo_replay::builtin_script() {
                let session_id = session_id.clone();
                let alive = match turn {
                    SampleTurn::User(_) => send(
                        jcode_sdk::ApiEvent::SessionStatus {
                            session_id,
                            status: "processing".into(),
                            pending_soft_interrupts: None,
                        },
                        cx,
                    ),
                    SampleTurn::Reasoning(text) | SampleTurn::Assistant(text) => {
                        let reasoning = text.starts_with("Flicker");
                        let mut ok = true;
                        for piece in text
                            .split_inclusive(' ')
                            .collect::<Vec<_>>()
                            .chunks(3)
                            .map(|words| words.concat())
                        {
                            let session_id = session_id.clone();
                            ok &= send(
                                if reasoning {
                                    jcode_sdk::ApiEvent::ReasoningDelta {
                                        session_id,
                                        text: piece,
                                    }
                                } else {
                                    jcode_sdk::ApiEvent::TextDelta {
                                        session_id,
                                        text: piece,
                                        message_id: None,
                                    }
                                },
                                cx,
                            );
                            executor.timer(CHUNK_DELAY).await;
                        }
                        ok
                    }
                    SampleTurn::Tool {
                        name,
                        input,
                        output,
                    } => {
                        call += 1;
                        let call_id = format!("stream-{call}");
                        let mut ok = send(
                            jcode_sdk::ApiEvent::ToolStart {
                                session_id: session_id.clone(),
                                call_id: call_id.clone(),
                                name: name.clone(),
                            },
                            cx,
                        );
                        for piece in input.as_bytes().chunks(12) {
                            ok &= send(
                                jcode_sdk::ApiEvent::ToolInputDelta {
                                    session_id: session_id.clone(),
                                    call_id: call_id.clone(),
                                    delta: String::from_utf8_lossy(piece).into_owned(),
                                },
                                cx,
                            );
                            executor.timer(CHUNK_DELAY).await;
                        }
                        executor.timer(tool_run).await;
                        ok && send(
                            jcode_sdk::ApiEvent::ToolDone {
                                session_id,
                                call_id,
                                name,
                                output,
                                error: None,
                            },
                            cx,
                        )
                    }
                };
                if !alive {
                    return;
                }
            }
            if !send(
                jcode_sdk::ApiEvent::TurnDone {
                    session_id: session_id.clone(),
                    pending_soft_interrupts: None,
                },
                cx,
            ) {
                return;
            }
            executor.timer(turn_gap).await;
        }
    })
}
