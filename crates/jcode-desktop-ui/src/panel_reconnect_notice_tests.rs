use super::*;

fn active_panel(cx: &mut Context<Panel>) -> Panel {
    let mut panel = Panel::new(
        "reconnect-test".into(),
        None,
        None,
        crate::harness::spawn_inert(),
        cx,
    );
    panel.history_loaded = true;
    panel.status = "processing".into();
    panel.items.push(Item::User("Continue".into()));
    panel.streaming_text = "Partial answer".into();
    panel
}

fn replay_processing_and_usage(panel: &mut Panel, cx: &mut Context<Panel>) {
    panel.apply(
        &ApiEvent::SessionStatus {
            session_id: "reconnect-test".into(),
            status: "processing".into(),
            pending_soft_interrupts: None,
        },
        cx,
    );
    panel.apply(
        &ApiEvent::TokenUsage {
            session_id: "reconnect-test".into(),
            input: 100,
            output: 20,
            cache_read_input: None,
            cache_creation_input: None,
        },
        cx,
    );
}

fn notices(panel: &Panel) -> Vec<&StopNotice> {
    panel
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Stopped(notice) => Some(notice),
            _ => None,
        })
        .collect()
}

fn footer_count(panel: &Panel) -> usize {
    panel
        .items
        .iter()
        .filter(|item| matches!(item, Item::ResponseStats(_)))
        .count()
}

#[gpui::test]
fn reconnect_notice_tests_replayed_processing_and_stats_coalesce(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        replay_processing_and_usage(panel, cx);
        panel.connection_lost("Socket closed", cx);
        assert_eq!(footer_count(panel), 1);
        // History/accounting replay can put a stats row after the notice too.
        panel
            .items
            .push(Item::ResponseStats(response_stats::fixture()));
        let footers = footer_count(panel);
        for _ in 0..4 {
            replay_processing_and_usage(panel, cx);
            panel.connection_lost("Socket closed again", cx);
            assert_eq!(notices(panel).len(), 1);
            assert_eq!(footer_count(panel), footers);
        }
        assert_eq!(
            notices(panel)[0].raw.as_deref(),
            Some("Socket closed again")
        );
    });
}

#[gpui::test]
fn reconnect_notice_tests_confirmed_replaces_across_stats_and_survives_provisional(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        panel.connection_lost("Socket closed", cx);
        panel
            .items
            .push(Item::ResponseStats(response_stats::fixture()));
        replay_processing_and_usage(panel, cx);
        let confirmed = StopNotice::from_event(
            &TurnStopReason::Failure,
            "Specific runtime failure",
            Some("provider_detail"),
        );
        panel.record_stop(confirmed.clone());
        assert_eq!(notices(panel), vec![&confirmed]);
        let footers = footer_count(panel);
        replay_processing_and_usage(panel, cx);
        panel.connection_lost("Another disconnect", cx);
        assert_eq!(notices(panel), vec![&confirmed]);
        assert_eq!(footer_count(panel), footers);
    });
}

#[gpui::test]
fn reconnect_notice_tests_real_output_and_new_user_are_boundaries(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| active_panel(cx));
    panel.update(vcx, |panel, cx| {
        panel.connection_lost("Initial disconnect", cx);
        for boundary in [
            Item::Assistant("New answer".into()),
            Item::Reasoning("New reasoning".into()),
            Item::Tool {
                call_id: "new-tool".into(),
                name: "bash".into(),
                input: "{}".into(),
                output: "New result".into(),
                done: true,
                error: None,
            },
            Item::User("New turn".into()),
        ] {
            let count = notices(panel).len();
            let footers = footer_count(panel);
            panel.items.push(boundary);
            replay_processing_and_usage(panel, cx);
            panel.connection_lost("New disconnect", cx);
            assert_eq!(notices(panel).len(), count + 1);
            assert_eq!(footer_count(panel), footers + 1);
        }
        // Buffered output is flushed by finish_response and must also break coalescing.
        for reasoning in [false, true] {
            let count = notices(panel).len();
            let footers = footer_count(panel);
            if reasoning {
                panel.streaming_reasoning = "Fresh streamed reasoning".into();
            } else {
                panel.streaming_text = "Fresh streamed answer".into();
            }
            replay_processing_and_usage(panel, cx);
            panel.connection_lost("Disconnect after new output", cx);
            assert_eq!(notices(panel).len(), count + 1);
            assert_eq!(footer_count(panel), footers + 1);
        }
    });
}
