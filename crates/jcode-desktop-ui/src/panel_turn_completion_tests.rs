use super::*;

#[gpui::test]
fn reconnect_turn_completion_reconciles_idle_and_active_history(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("reconnect-turn", cx);
        workspace
    });
    let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    for active in [false, true] {
        panel.update(vcx, |panel, cx| {
            panel.history_loaded = true;
            panel.items = vec![Item::User("hello".into())];
            panel.streaming_text = "partial".into();
            panel.connection_phase = "streaming".into();
            panel.status = "lost: reconnecting".into();
            panel.load_history(vec![jcode_sdk::HistoryMessage {
                response_stats: None, role: "user".into(), content: "hello".into(),
            }, jcode_sdk::HistoryMessage {
                response_stats: None,
                role: "assistant".into(), content: "partial response".into(),
            }], vec![], cx);
            panel.apply(&ApiEvent::SessionStatus {
                session_id: panel.session_id.clone(),
                status: if active { "running" } else { "idle" }.into(),
            }, cx);
            assert_eq!(panel.activity_active(), active);
            assert_eq!(panel.sidebar_activity().is_some(), active);
            if active {
                assert_ne!(panel.status_line(), "Ready");
            } else {
                assert_eq!(panel.status_line(), "Ready");
            }
            if !active {
                assert!(panel.streaming_text.is_empty());
                assert!(matches!(panel.items.last(), Some(Item::Assistant(text)) if text == "partial response"));
            }
            // Transport events cannot replace the authoritative turn state.
            panel.apply(&ApiEvent::SessionStatus {
                session_id: panel.session_id.clone(), status: "attached".into(),
            }, cx);
            assert_eq!(panel.activity_active(), active);
            panel.apply(&ApiEvent::TurnDone { session_id: panel.session_id.clone() }, cx);
            assert_eq!(panel.status_line(), "Ready");
            assert!(!panel.activity_active());
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("transcript-activity").is_none());
        assert!(vcx.debug_bounds("panel-activity-spinner").is_none());
    }
}

#[gpui::test]
fn reconnect_history_does_not_replay_previous_answer_over_a_new_turn(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("reconnect-turn", cx);
        workspace
    });
    let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    panel.update(vcx, |panel, cx| {
        panel.history_loaded = true;
        panel.items = vec![
            Item::Assistant("old answer".into()),
            Item::User("next".into()),
        ];
        panel.streaming_text = "new partial".into();
        panel.load_history(
            vec![
                jcode_sdk::HistoryMessage {
                    response_stats: None,
                    role: "assistant".into(),
                    content: "old answer".into(),
                },
                jcode_sdk::HistoryMessage {
                    response_stats: None,
                    role: "user".into(),
                    content: "next".into(),
                },
            ],
            vec![],
            cx,
        );
        assert_eq!(panel.streaming_text, "new partial");
        assert_eq!(panel.items.len(), 2);
    });
}
