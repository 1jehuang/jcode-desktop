use super::*;

fn image(call: &str, data: &str, boundary: Option<usize>) -> jcode_sdk::RenderedImage {
    jcode_sdk::RenderedImage {
        media_type: "image/png".into(),
        data: data.into(),
        label: Some("chart.png".into()),
        source: jcode_sdk::RenderedImageSource::ToolResult {
            tool_name: "read".into(),
        },
        anchor: Some(jcode_sdk::RenderedImageAnchor::ToolCall { id: call.into() }),
        history_message_index: boundary,
    }
}

fn tool(call: &str) -> Item {
    Item::Tool {
        call_id: call.into(),
        name: "read".into(),
        input: String::new(),
        output: "Image loaded".into(),
        done: true,
        error: None,
    }
}

fn message(role: &str, content: &str) -> jcode_sdk::HistoryMessage {
    jcode_sdk::HistoryMessage {
        response_stats: None,
        role: role.into(),
        content: content.into(),
    }
}

#[gpui::test]
fn history_images_stay_between_read_and_response_across_turns(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        w.push_test_panel("image-history", cx);
        w
    });
    let panel = workspace.update(vcx, |w, _| w.test_panel(0).unwrap());
    panel.update(vcx, |panel, cx| {
        panel.items.clear();
        panel.load_history(vec![message("user", "Read chart"), message("assistant", "Reading now"),
            message("tool", "read result"), message("assistant", "Chart response"),
            message("user", "Read again"), message("tool", "read result"),
            message("assistant", "Second response")],
            vec![image("first", "same", Some(3)), image("second", "same", Some(6))], cx);
        assert!(matches!(panel.items.as_slice(), [Item::User(_), Item::Assistant(before),
            Item::Image(first), Item::Assistant(after), Item::User(_), Item::Image(second), Item::Assistant(last)]
            if before == "Reading now" && after == "Chart response" && last == "Second response"
                && first.anchor != second.anchor));
        for item in &panel.items {
            if let Item::Image(image) = item {
                assert_eq!(image.model_input_caption(), Some("Image provided to model"));
            }
        }
    });
}

#[gpui::test]
fn live_images_keep_batch_order_and_repeated_reads_but_deduplicate_replay(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        w.push_test_panel("images-live", cx);
        w
    });
    let panel = workspace.update(vcx, |w, _| w.test_panel(0).unwrap());
    panel.update(vcx, |panel, _| {
        panel.items = vec![tool("first"), Item::Assistant("After first read".into()), tool("second"), Item::User("pending".into())];
        panel.pending_users = [3].into();
        panel.accepted_users.insert(3, Instant::now());
        for _ in 0..2 {
            panel.insert_rendered_image(image("first", "one", None));
            panel.insert_rendered_image(image("first", "two", None));
            panel.insert_rendered_image(image("second", "one", None));
        }
        assert!(matches!(panel.items.as_slice(), [Item::Tool {..}, Item::Image(first), Item::Image(second),
            Item::Assistant(_), Item::Tool {..}, Item::Image(repeated), Item::User(_)]
            if first.data == "one" && second.data == "two" && repeated.data == "one"));
        assert_eq!(panel.pending_users.front(), Some(&6));
        assert!(panel.accepted_users.contains_key(&6));
    });
}

#[gpui::test]
fn history_image_boundaries_include_start_end_and_hidden_rows(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        w.push_test_panel("images-boundaries", cx);
        w
    });
    let panel = workspace.update(vcx, |w, _| w.test_panel(0).unwrap());
    panel.update(vcx, |panel, cx| {
        panel.items.clear();
        panel.load_history(vec![message("assistant", "First"), message("tool", "hidden"), message("assistant", "Last")],
            vec![image("start", "start", Some(0)), image("middle", "middle", Some(2)), image("end", "end", Some(3))], cx);
        assert!(matches!(panel.items.as_slice(), [Item::Image(a), Item::Assistant(_), Item::Image(b),
            Item::Assistant(_), Item::Image(c)] if a.data == "start" && b.data == "middle" && c.data == "end"));
    });
}

#[test]
fn only_tool_image_payloads_claim_model_input() {
    let pasted = TranscriptImage::new("image/png".into(), "bytes".into(), None);
    assert_eq!(pasted.model_input_caption(), None);
    let from_tool = TranscriptImage::from_rendered(image("read", "bytes", None));
    assert_eq!(
        from_tool.model_input_caption(),
        Some("Image provided to model")
    );
    assert_eq!(from_tool.label.as_deref(), Some("chart.png"));
}
