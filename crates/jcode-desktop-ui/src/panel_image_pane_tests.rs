//! Image-pane contracts. Register this module alongside panel_image_transcript_tests.
use super::*;

const DRAFT: &str = "Keep my unfinished image question";

fn setup(cx: &mut gpui::TestAppContext) -> (Entity<Panel>, &mut gpui::VisualTestContext) {
    cx.update(|cx| crate::input::bind_keys(cx));
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("image-pane-session", cx);
        workspace
    });
    let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    panel.update(vcx, |panel, cx| {
        panel.items = vec![
            Item::User("Compare these images".into()),
            Item::Image(fixture("first.png")),
            Item::Assistant("Another image follows".into()),
            Item::Image(fixture("second.png")),
        ];
        panel
            .input
            .update(cx, |input, cx| input.set_content(DRAFT.into(), cx));
        cx.notify();
    });
    vcx.run_until_parked();
    (panel, vcx)
}

fn fixture(label: &str) -> TranscriptImage {
    let mut image = image_preview::fixture_image();
    image.label = Some(label.into());
    image
}

fn incoming(call: &str) -> jcode_sdk::RenderedImage {
    let image = fixture(call);
    jcode_sdk::RenderedImage {
        media_type: image.media_type,
        data: image.data,
        label: image.label,
        source: jcode_sdk::RenderedImageSource::ToolResult {
            tool_name: "read".into(),
        },
        anchor: Some(jcode_sdk::RenderedImageAnchor::ToolCall { id: call.into() }),
        history_message_index: None,
    }
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx.debug_bounds(selector).expect(selector);
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

fn assert_draft(panel: &Entity<Panel>, vcx: &mut gpui::VisualTestContext) {
    assert_eq!(
        panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
        DRAFT
    );
}

#[gpui::test]
fn image_pane_button_toggles_links_and_close_restores_inline(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        assert!(!panel.image_pane_open);
        assert_eq!(panel.image_pane_selected, None);
        assert_eq!(panel.session_image_indices(), vec![1, 3]);
        assert!(panel.render_image_pane(cx).is_none());
    });
    assert!(vcx.debug_bounds("transcript-image").is_some());
    assert!(vcx.debug_bounds("session-image-pane").is_none());

    click(vcx, "panel-images");
    assert!(panel.read_with(vcx, |panel, _| panel.image_pane_open));
    assert!(vcx.debug_bounds("session-image-pane").is_some());
    assert!(vcx.debug_bounds("session-image-main").is_some());
    assert!(vcx.debug_bounds("transcript-image").is_none());
    assert!(vcx.debug_bounds("transcript-image-link-1").is_some());
    assert!(vcx.debug_bounds("transcript-image-link-3").is_some());
    assert_draft(&panel, vcx);

    click(vcx, "panel-images");
    assert!(!panel.read_with(vcx, |panel, _| panel.image_pane_open));
    assert!(vcx.debug_bounds("session-image-pane").is_none());
    assert!(vcx.debug_bounds("transcript-image").is_some());

    click(vcx, "panel-images");
    click(vcx, "session-image-close");
    assert!(!panel.read_with(vcx, |panel, _| panel.image_pane_open));
    assert!(vcx.debug_bounds("session-image-pane").is_none());
    assert!(vcx.debug_bounds("transcript-image-link-1").is_none());
    assert!(vcx.debug_bounds("transcript-image").is_some());
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_thumbnails_select_item_indices_and_main_opens_lightbox(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    click(vcx, "panel-images");
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel.selected_session_image_index()),
        Some(3)
    );
    click(vcx, "session-image-thumb-1");
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel.image_pane_selected),
        Some(1)
    );
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel.selected_session_image_index()),
        Some(1)
    );
    click(vcx, "session-image-main");
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel
            .image_preview
            .as_ref()
            .and_then(|image| image.label.clone())),
        Some("first.png".into())
    );
    assert!(vcx.debug_bounds("image-preview-full").is_some());
    vcx.simulate_keystrokes("x");
    assert_draft(&panel, vcx);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
    assert!(vcx.debug_bounds("session-image-pane").is_some());
    click(vcx, "session-image-thumb-3");
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel.image_pane_selected),
        Some(3)
    );
    click(vcx, "session-image-main");
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel
            .image_preview
            .as_ref()
            .and_then(|image| image.label.clone())),
        Some("second.png".into())
    );
    click(vcx, "image-preview-close");
    click(vcx, "session-image-close");
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_follows_incoming_images_only_without_explicit_selection(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    click(vcx, "panel-images");
    panel.update(vcx, |panel, cx| {
        panel.insert_rendered_image(incoming("third.png"));
        assert_eq!(panel.image_pane_selected, None);
        assert_eq!(panel.session_image_indices(), vec![1, 3, 4]);
        assert_eq!(panel.selected_session_image_index(), Some(4));
        cx.notify();
    });
    vcx.run_until_parked();
    click(vcx, "session-image-main");
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel
            .image_preview
            .as_ref()
            .and_then(|image| image.label.clone())),
        Some("third.png".into())
    );
    click(vcx, "image-preview-close");
    click(vcx, "session-image-thumb-1");
    panel.update(vcx, |panel, cx| {
        panel.insert_rendered_image(incoming("fourth.png"));
        assert_eq!(panel.image_pane_selected, Some(1));
        assert_eq!(panel.selected_session_image_index(), Some(1));
        cx.notify();
    });
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_selection_tracks_image_when_earlier_tool_result_arrives(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.items[0] = Item::Tool {
            call_id: "early-read".into(),
            name: "read".into(),
            input: String::new(),
            output: "Image loaded".into(),
            done: true,
            error: None,
        };
        panel.set_image_pane_open(true, cx);
        panel.image_pane_selected = Some(3);
        panel.insert_rendered_image(incoming("early-read"));
        assert_eq!(panel.session_image_indices(), vec![1, 2, 4]);
        assert_eq!(panel.image_pane_selected, Some(4));
        assert_eq!(panel.selected_session_image_index(), Some(4));
        assert!(matches!(&panel.items[4], Item::Image(image) if image.label.as_deref() == Some("second.png")));
        // Replayed results do not shift the selection a second time.
        panel.insert_rendered_image(incoming("early-read"));
        assert_eq!(panel.image_pane_selected, Some(4));
        assert_eq!(panel.session_image_indices(), vec![1, 2, 4]);
    });
}

#[gpui::test]
fn image_pane_state_is_independent_between_session_panels(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("images-a", cx);
        workspace.push_test_panel("images-b", cx);
        workspace
    });
    let first = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    let second = workspace.update(vcx, |workspace, _| workspace.test_panel(1).unwrap());
    for panel in [&first, &second] {
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Image(fixture("one")), Item::Image(fixture("two"))];
            cx.notify();
        });
    }
    first.update(vcx, |panel, cx| {
        panel.set_image_pane_open(true, cx);
        panel.image_pane_selected = Some(0);
    });
    second.update(vcx, |panel, cx| {
        assert!(!panel.image_pane_open);
        assert_eq!(panel.image_pane_selected, None);
        assert!(panel.render_image_pane(cx).is_none());
        panel.set_image_pane_open(true, cx);
        assert_eq!(panel.selected_session_image_index(), Some(1));
    });
    first.update(vcx, |panel, cx| {
        assert_eq!(panel.image_pane_selected, Some(0));
        panel.set_image_pane_open(false, cx);
    });
    second.update(vcx, |panel, cx| {
        assert!(panel.image_pane_open);
        assert!(panel.render_image_pane(cx).is_some());
        assert_eq!(panel.image_pane_selected, None);
    });
}

#[gpui::test]
fn image_pane_snapshot_roundtrip_preserves_open_and_draft_but_follows_latest(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.set_image_pane_open(true, cx);
        panel.image_pane_selected = Some(1);
        let json = serde_json::to_value(panel.snapshot(cx)).unwrap();
        assert_eq!(json["image_pane_open"], true);
        // Item indices are not stable across history reconstruction.
        assert!(json.get("image_pane_selected").is_none());
        let snapshot: PanelSnapshot = serde_json::from_value(json).unwrap();
        assert!(snapshot.image_pane_open);
        panel.set_image_pane_open(false, cx);
        panel
            .input
            .update(cx, |input, cx| input.set_content("changed".into(), cx));
        panel.restore_snapshot(snapshot, cx);
        assert!(panel.image_pane_open);
        assert_eq!(panel.image_pane_selected, None);
        assert_eq!(panel.selected_session_image_index(), Some(3));
    });
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_legacy_snapshot_defaults_closed(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        let mut json = serde_json::to_value(panel.snapshot(cx)).unwrap();
        json.as_object_mut().unwrap().remove("image_pane_open");
        json.as_object_mut().unwrap().remove("image_pane_selected");
        let legacy: PanelSnapshot = serde_json::from_value(json).unwrap();
        assert!(!legacy.image_pane_open);
        panel.set_image_pane_open(true, cx);
        panel.image_pane_selected = Some(1);
        panel.restore_snapshot(legacy, cx);
        assert!(!panel.image_pane_open);
        assert_eq!(panel.image_pane_selected, None);
        assert!(panel.render_image_pane(cx).is_none());
    });
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_follow_latest_button_resumes_after_manual_selection(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = setup(cx);
    click(vcx, "panel-images");
    click(vcx, "session-image-thumb-1");
    click(vcx, "session-image-latest");
    panel.update(vcx, |panel, cx| {
        assert_eq!(panel.image_pane_selected, None);
        assert_eq!(panel.selected_session_image_index(), Some(3));
        panel.insert_rendered_image(incoming("resumed.png"));
        assert_eq!(panel.selected_session_image_index(), Some(4));
        cx.notify();
    });
    vcx.run_until_parked();
    click(vcx, "session-image-main");
    assert_eq!(
        panel.read_with(vcx, |panel, _| panel
            .image_preview
            .as_ref()
            .and_then(|image| image.label.clone())),
        Some("resumed.png".into())
    );
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_empty_session_accepts_first_image(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        panel.items.clear();
        assert!(panel.session_image_indices().is_empty());
        assert_eq!(panel.selected_session_image_index(), None);
        cx.notify();
    });
    vcx.run_until_parked();
    click(vcx, "panel-images");
    assert!(vcx.debug_bounds("session-image-pane").is_some());
    assert!(vcx.debug_bounds("session-image-main").is_none());
    panel.update(vcx, |panel, cx| {
        panel.insert_rendered_image(incoming("first-in-empty.png"));
        assert_eq!(panel.selected_session_image_index(), Some(0));
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("session-image-main").is_some());
    assert!(vcx.debug_bounds("session-image-thumb-0").is_some());
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_missing_preview_does_not_open_lightbox(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = setup(cx);
    panel.update(vcx, |panel, cx| {
        let mut image = fixture("unavailable.png");
        image.preview = None;
        panel.items = vec![Item::Image(image)];
        cx.notify();
    });
    vcx.run_until_parked();
    click(vcx, "panel-images");
    click(vcx, "session-image-thumb-0");
    click(vcx, "session-image-main");
    assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
    assert!(vcx.debug_bounds("image-preview").is_none());
    assert!(vcx.debug_bounds("session-image-pane").is_some());
    assert_draft(&panel, vcx);
}

#[gpui::test]
fn image_pane_narrow_and_wide_geometry_keeps_toggle_and_composer_visible(
    cx: &mut gpui::TestAppContext,
) {
    let (bridge, _commands) = crate::harness::spawn_recording();
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new("image-pane-responsive".into(), None, None, bridge, cx)
    });
    panel.update(vcx, |panel, cx| {
        panel.items = vec![Item::Image(fixture("responsive.png"))];
        panel
            .input
            .update(cx, |input, cx| input.set_content(DRAFT.into(), cx));
        cx.notify();
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for width in [480., 1000., 480.] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(800.)));
        panel.update(vcx, |panel, cx| panel.set_image_pane_open(false, cx));
        vcx.run_until_parked();
        click(vcx, "panel-images");
        // The layout observer schedules the responsive orientation update.
        panel.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        for selector in [
            "panel-images",
            "prompt-input",
            "session-image-pane",
            "session-image-main",
            "session-image-close",
        ] {
            let bounds = vcx.debug_bounds(selector).expect(selector);
            assert!(
                bounds.size.width > px(0.) && bounds.size.height > px(0.),
                "{selector} at width {width}: {bounds:?}"
            );
            assert!(
                bounds.left() >= px(0.) && bounds.right() <= px(width + 1.),
                "{selector} at width {width}: {bounds:?}"
            );
            assert!(
                bounds.top() >= px(0.) && bounds.bottom() <= px(801.),
                "{selector} at width {width}: {bounds:?}"
            );
        }
        let transcript = vcx.debug_bounds("transcript").unwrap();
        let gallery = vcx.debug_bounds("session-image-pane").unwrap();
        if width < 600. {
            assert!(
                gallery.top() >= transcript.bottom(),
                "narrow pane should stack below chat: {gallery:?} / {transcript:?}"
            );
        } else {
            assert!(
                gallery.left() >= transcript.right(),
                "wide pane should sit beside chat: {gallery:?} / {transcript:?}"
            );
        }
        assert_draft(&panel, vcx);
        click(vcx, "session-image-close");
        assert!(vcx.debug_bounds("transcript-image").is_some());
    }
}
