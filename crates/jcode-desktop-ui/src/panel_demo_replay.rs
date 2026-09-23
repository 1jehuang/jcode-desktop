//! Scripted chat replay for onboarding. A real `Panel` renders every step
//! through its ordinary `ApiEvent` reducer, so the streaming reveal, tool
//! spinners, and turn completion look exactly as they will in use.
use super::*;
use jcode_base::transcript_sample::SampleTurn;

/// Replays loop forever, so pause briefly between laps and turns.
const TURN_GAP: Duration = Duration::from_millis(1100);
const LAP_GAP: Duration = Duration::from_millis(2600);
const CHUNK_DELAY: Duration = Duration::from_millis(36);
const TOOL_RUN: Duration = Duration::from_millis(700);
/// Bytes per streamed delta. The panel's reveal smooths the rest.
const CHUNK: usize = 18;

/// A self-contained session shown when no other harness history exists.
pub(crate) fn builtin_script() -> Vec<SampleTurn> {
    let tool = |name: &str, input: serde_json::Value, output: &str| SampleTurn::Tool {
        name: name.into(),
        input: input.to_string(),
        output: output.into(),
    };
    vec![
        SampleTurn::User("The sidebar flickers when a session finishes. Can you find out why?".into()),
        SampleTurn::Reasoning("Flicker on completion usually means the list re-sorts or remounts. I'll look at how the sidebar keys its rows first.".into()),
        tool(
            "agentgrep",
            serde_json::json!({"query": "fn render_session_row", "path": "src"}),
            "src/sidebar.rs:212: fn render_session_row(&self, session: &Session)",
        ),
        tool(
            "read",
            serde_json::json!({"file_path": "src/sidebar.rs", "start_line": 200}),
            "let key = format!(\"{}-{}\", session.id, session.status);",
        ),
        SampleTurn::Assistant("Found it. Each row's key includes the session **status**, so when a session goes from `running` to `idle` the row is torn down and rebuilt:\n\n```rust\nlet key = format!(\"{}-{}\", session.id, session.status);\n```\n\nKeying on the id alone keeps the row mounted. I'll make that change and run the tests.".into()),
        tool(
            "edit",
            serde_json::json!({"file_path": "src/sidebar.rs", "old_string": "format!(\"{}-{}\", session.id, session.status)", "new_string": "session.id.clone()"}),
            "Edited src/sidebar.rs (+1 -1)",
        ),
        tool(
            "bash",
            serde_json::json!({"command": "cargo test sidebar", "intent": "Run sidebar tests"}),
            "running 14 tests\n..............\ntest result: ok. 14 passed; 0 failed",
        ),
        SampleTurn::Assistant("Fixed. Rows now stay mounted through status changes, and all 14 sidebar tests pass.\n\n- `src/sidebar.rs`: key rows by session id".into()),
        SampleTurn::User("Nice. Does anything else key on status?".into()),
        tool(
            "agentgrep",
            serde_json::json!({"query": "session.status)", "path": "src"}),
            "src/tabs.rs:88: let id = (session.id, session.status);",
        ),
        SampleTurn::Assistant("One more spot. The tab strip builds its ids the same way in `src/tabs.rs`, so tabs would flash too. Same one-line fix applies there.".into()),
    ]
}

impl Panel {
    /// An inert, sound-free panel for replaying `script` in a loop.
    pub(crate) fn new_demo(title: &str, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::new(
            "demo://onboarding".into(),
            Some(title.to_owned()),
            None,
            crate::harness::spawn_inert(),
            cx,
        );
        panel.demo = true;
        panel.history_loaded = true;
        panel.items.clear();
        panel.streaming_text.clear();
        panel.streaming_reasoning.clear();
        panel
    }

    fn demo_event(&mut self, event: ApiEvent, cx: &mut Context<Self>) {
        self.apply(&event, cx);
        cx.notify();
    }
}

/// Split on char boundaries, preferring to break after spaces.
fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.len() >= CHUNK && (ch == ' ' || ch == '\n' || current.len() >= CHUNK * 2) {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Drive `panel` through `script` until the returned task is dropped.
pub(crate) fn run(
    panel: Entity<Panel>,
    script: Vec<SampleTurn>,
    cx: &mut App,
) -> gpui::Task<()> {
    // Weak, so closing onboarding drops the panel and ends the loop.
    let panel = panel.downgrade();
    cx.spawn(async move |cx| {
        if script.is_empty() {
            return;
        }
        let executor = cx.background_executor().clone();
        let sleep = |duration| executor.timer(duration);
        loop {
            let session = panel
                .update(cx, |panel, cx| {
                    panel.items.clear();
                    panel.transcript_measurements.dirty = true;
                    cx.notify();
                    panel.session_id.clone()
                })
                .ok();
            let Some(session_id) = session else { return };
            let mut in_turn = false;
            let mut call = 0usize;
            for turn in &script {
                let alive = match turn {
                    SampleTurn::User(text) => {
                        // The first prompt appears at once so the panel is never blank.
                        if in_turn {
                            let _ = panel.update(cx, |panel, cx| {
                                panel.demo_event(ApiEvent::TurnDone { session_id: session_id.clone() }, cx)
                            });
                            sleep(TURN_GAP).await;
                        }
                        in_turn = true;
                        panel
                            .update(cx, |panel, cx| {
                                panel.items.push(Item::User(text.clone()));
                                panel.transcript_measurements.dirty = true;
                                panel.demo_event(
                                    ApiEvent::SessionStatus {
                                        session_id: session_id.clone(),
                                        status: "processing".into(),
                                    },
                                    cx,
                                );
                            })
                            .is_ok()
                    }
                    SampleTurn::Reasoning(text) | SampleTurn::Assistant(text) => {
                        let reasoning = matches!(turn, SampleTurn::Reasoning(_));
                        let mut ok = true;
                        for piece in chunks(text) {
                            ok &= panel
                                .update(cx, |panel, cx| {
                                    let session_id = session_id.clone();
                                    panel.demo_event(
                                        if reasoning {
                                            ApiEvent::ReasoningDelta { session_id, text: piece }
                                        } else {
                                            ApiEvent::TextDelta { session_id, text: piece, message_id: None }
                                        },
                                        cx,
                                    )
                                })
                                .is_ok();
                            if !ok {
                                break;
                            }
                            sleep(CHUNK_DELAY).await;
                        }
                        if reasoning && ok {
                            ok = panel
                                .update(cx, |panel, cx| {
                                    panel.demo_event(
                                        ApiEvent::ReasoningDone {
                                            session_id: session_id.clone(),
                                            duration_secs: None,
                                        },
                                        cx,
                                    )
                                })
                                .is_ok();
                        }
                        ok
                    }
                    SampleTurn::Tool { name, input, output } => {
                        call += 1;
                        let call_id = format!("demo-{call}");
                        let started = panel
                            .update(cx, |panel, cx| {
                                panel.demo_event(
                                    ApiEvent::ToolStart {
                                        session_id: session_id.clone(),
                                        call_id: call_id.clone(),
                                        name: name.clone(),
                                    },
                                    cx,
                                );
                                panel.demo_event(
                                    ApiEvent::ToolInputDelta {
                                        session_id: session_id.clone(),
                                        call_id: call_id.clone(),
                                        delta: input.clone(),
                                    },
                                    cx,
                                );
                            })
                            .is_ok();
                        sleep(TOOL_RUN).await;
                        started
                            && panel
                                .update(cx, |panel, cx| {
                                    panel.demo_event(
                                        ApiEvent::ToolDone {
                                            session_id: session_id.clone(),
                                            call_id,
                                            name: name.clone(),
                                            output: output.clone(),
                                            error: None,
                                        },
                                        cx,
                                    )
                                })
                                .is_ok()
                    }
                };
                if !alive {
                    return;
                }
            }
            let _ = panel.update(cx, |panel, cx| {
                panel.demo_event(ApiEvent::TurnDone { session_id: session_id.clone() }, cx)
            });
            sleep(LAP_GAP).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_preserve_text_and_char_boundaries() {
        let text = "Déjà vu: the sidebar re-keys rows on status, so each finish remounts them.";
        let pieces = chunks(text);
        assert!(pieces.len() > 2);
        assert_eq!(pieces.concat(), text);
    }

    #[gpui::test]
    fn replay_streams_real_items_into_the_panel(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_demo("Demo", cx));
        let script = vec![
            SampleTurn::User("Fix it".into()),
            SampleTurn::Tool { name: "bash".into(), input: r#"{"command":"ls"}"#.into(), output: "ok".into() },
            SampleTurn::Assistant("All done.".into()),
        ];
        let task = vcx.update(|_, cx| run(panel.clone(), script, cx));
        // One lap is about 0.8s, then LAP_GAP. Sample inside the gap, before
        // the next lap clears the transcript.
        vcx.executor().advance_clock(Duration::from_secs(2));
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert!(panel.demo);
            assert!(matches!(panel.items.first(), Some(Item::User(text)) if text == "Fix it"));
            assert!(panel.items.iter().any(|item| matches!(item, Item::Tool { done: true, output, .. } if output == "ok")));
            assert!(panel.items.iter().any(|item| matches!(item, Item::Assistant(text) if text == "All done.")));
        });
        drop(task);
    }
}
