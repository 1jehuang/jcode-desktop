use super::*;

fn setup(cx: &mut gpui::TestAppContext) -> (Entity<Panel>, &mut gpui::VisualTestContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("reload-test", cx);
        workspace
    });
    let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    (panel, vcx)
}

fn history(rows: &[(&str, &str)]) -> Vec<jcode_sdk::HistoryMessage> {
    rows.iter()
        .map(|(role, content)| jcode_sdk::HistoryMessage {
            role: (*role).into(),
            content: (*content).into(),
            response_stats: None,
        })
        .collect()
}

fn idle(panel: &mut Panel, cx: &mut Context<Panel>) {
    panel.apply(
        &ApiEvent::SessionStatus {
            session_id: "reload-test".into(),
            status: "idle".into(),
        },
        cx,
    );
}

#[gpui::test]
fn reload_snapshot_roundtrips_rich_transcript_without_checkpoint_payload(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.history_loaded = true;
        panel.items = vec![
            Item::User("question".into()),
            Item::Image(image_preview::fixture_image()),
            Item::Reasoning("earlier reasoning".into()),
            Item::Tool {
                call_id: "call".into(),
                name: "bash".into(),
                input: "pwd".into(),
                output: "".into(),
                done: false,
                error: None,
            },
            Item::Stopped(stop_reason::StopNotice::cancel_requested()),
        ];
        panel.streaming_text = "partial answer".into();
        panel.streaming_reasoning = "partial reasoning".into();
        panel.status = "streaming".into();
        panel.connection_phase = "streaming".into();
        assert!(
            panel.snapshot(cx).transcript.is_none(),
            "periodic checkpoints stay lightweight"
        );
        let expected = panel.items.clone();
        let encoded = serde_json::to_vec(&panel.snapshot_for_reload(cx)).unwrap();
        let saved: PanelSnapshot = serde_json::from_slice(&encoded).unwrap();
        panel.items.clear();
        panel.streaming_text.clear();
        panel.streaming_reasoning.clear();
        panel.restore_snapshot(saved, cx);
        assert_eq!(panel.items, expected);
        assert_eq!(panel.streaming_text, "partial answer");
        assert_eq!(panel.streaming_reasoning, "partial reasoning");
        assert!(panel.history_loaded);
        assert!(matches!(&panel.items[1], Item::Image(image) if image.preview.is_some()));
        assert_eq!(panel.status, "streaming");
        let legacy = serde_json::to_value(panel.snapshot(cx)).unwrap();
        assert!(legacy.get("transcript").is_none());
        assert!(
            serde_json::from_value::<PanelSnapshot>(legacy)
                .unwrap()
                .transcript
                .is_none()
        );
    });
}

#[gpui::test]
fn restored_stream_survives_empty_and_behind_history_then_idle_clears_responding(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.history_loaded = true;
        panel.items = vec![
            Item::User("old".into()),
            Item::Assistant("old answer".into()),
            Item::User("current".into()),
        ];
        panel.streaming_text = "live".into();
        panel.status = "streaming".into();
        panel.connection_phase = "streaming".into();
        let saved =
            serde_json::from_slice(&serde_json::to_vec(&panel.snapshot_for_reload(cx)).unwrap())
                .unwrap();
        panel.restore_snapshot(saved, cx);
        for messages in [
            vec![],
            history(&[("user", "old"), ("assistant", "old answer")]),
        ] {
            panel.load_history(messages, vec![], cx);
            assert_eq!(panel.items.len(), 3);
            assert_eq!(panel.streaming_text, "live");
            assert_eq!(panel.status_line(), "Responding");
        }
        idle(panel, cx);
        assert_eq!(panel.status_line(), "Ready");
        assert!(panel.streaming_text.is_empty());
        assert_eq!(panel.items.last(), Some(&Item::Assistant("live".into())));
    });
}

#[gpui::test]
fn reconnect_history_waits_for_idle_and_never_duplicates_buffered_delta(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.history_loaded = true;
        panel.items = vec![Item::User("question".into())];
        panel.streaming_text = "part".into();
        panel.load_history(
            history(&[("user", "question"), ("assistant", "partial")]),
            vec![],
            cx,
        );
        assert_eq!(panel.streaming_text, "part");
        panel.apply(
            &ApiEvent::TextDelta {
                session_id: "reload-test".into(),
                message_id: Some("answer".into()),
                text: "ial".into(),
            },
            cx,
        );
        idle(panel, cx);
        assert_eq!(
            panel.items,
            vec![
                Item::User("question".into()),
                Item::Assistant("partial".into())
            ]
        );
        panel.load_history(
            history(&[("user", "question"), ("assistant", "partial")]),
            vec![],
            cx,
        );
        idle(panel, cx);
        assert_eq!(panel.items.len(), 2);
    });
}

#[gpui::test]
fn reconnect_recovery_respects_user_boundary_and_hydrates_newer_turns(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.history_loaded = true;
        panel.items = vec![
            Item::User("old".into()),
            Item::Assistant("same prefix".into()),
            Item::User("new".into()),
        ];
        panel.load_history(
            history(&[
                ("user", "old"),
                ("assistant", "same prefix"),
                ("user", "new"),
                ("assistant", "same prefix extended"),
            ]),
            vec![],
            cx,
        );
        idle(panel, cx);
        assert_eq!(panel.items[1], Item::Assistant("same prefix".into()));
        assert_eq!(
            panel.items[3],
            Item::Assistant("same prefix extended".into())
        );
        panel.load_history(
            history(&[
                ("user", "old"),
                ("assistant", "same prefix"),
                ("user", "new"),
                ("assistant", "same prefix extended"),
                ("user", "while suspended"),
                ("assistant", "latest answer"),
            ]),
            vec![],
            cx,
        );
        assert_eq!(
            panel.items.last(),
            Some(&Item::User("while suspended".into()))
        );
        idle(panel, cx);
        assert_eq!(panel.items.len(), 6);
        assert_eq!(
            panel.items.last(),
            Some(&Item::Assistant("latest answer".into()))
        );
    });
}

#[gpui::test]
fn reload_before_first_history_preserves_echo_and_stream_without_duplicates(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.history_loaded = false;
        panel.items = vec![Item::User("new".into())];
        panel.streaming_text = "part".into();
        panel.expanded_tools.insert("tool-1".into());
        let saved =
            serde_json::from_slice(&serde_json::to_vec(&panel.snapshot_for_reload(cx)).unwrap())
                .unwrap();
        panel.restore_snapshot(saved, cx);
        assert!(panel.expanded_tools.contains("tool-1"));
        panel.load_history(vec![], vec![], cx);
        assert_eq!(panel.items.len(), 1);
        assert!(
            panel.history_loaded,
            "empty history completes attachment readiness"
        );
        panel.load_history(
            history(&[
                ("user", "old"),
                ("assistant", "old answer"),
                ("user", "new"),
                ("assistant", "partial"),
            ]),
            vec![],
            cx,
        );
        assert!(panel.history_loaded);
        assert_eq!(
            panel.items,
            vec![
                Item::User("old".into()),
                Item::Assistant("old answer".into()),
                Item::User("new".into())
            ]
        );
        assert_eq!(panel.streaming_text, "part");
        idle(panel, cx);
        assert_eq!(panel.items.len(), 4);
        assert_eq!(panel.items.last(), Some(&Item::Assistant("partial".into())));
    });
}
