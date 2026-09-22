use super::*;

#[gpui::test]
fn transcript_drag_highlights_and_copies_text(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        let mut panel = Panel::new(
            "selection-test".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        );
        panel.items = vec![Item::Assistant("Select this transcript text".into())];
        panel
    });
    vcx.run_until_parked();
    let bounds = vcx.debug_bounds("selectable-text-0-0").unwrap();
    let start = point(bounds.left() + px(1.), bounds.center().y);
    let end = point(bounds.right() + px(20.), bounds.center().y);
    vcx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: start,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.run_until_parked();
    vcx.simulate_event(gpui::MouseMoveEvent {
        position: end,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::default(),
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, cx| {
        assert!(
            panel
                .transcript_selection
                .read(cx)
                .highlight("0-0", 26)
                .is_some(),
            "drag must paint a text highlight"
        );
    });
    vcx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: end,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
    });
    vcx.simulate_keystrokes("ctrl-c");
    let copied = vcx
        .update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("Select this transcript text"));
}

#[gpui::test]
fn transcript_selection_stops_after_release_outside_panel(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        let mut panel = Panel::new(
            "selection-release".into(),
            None,
            None,
            crate::harness::spawn_inert(),
            cx,
        );
        panel.items = vec![Item::Assistant("Select this transcript text".into())];
        panel
    });
    vcx.run_until_parked();
    let bounds = vcx.debug_bounds("selectable-text-0-0").unwrap();
    for release_delivered in [true, false] {
        let start = point(bounds.left() + px(1.), bounds.center().y);
        let end = point(bounds.right() + px(20.), bounds.center().y);
        vcx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: start,
            modifiers: gpui::Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.simulate_event(gpui::MouseMoveEvent {
            position: end,
            pressed_button: Some(gpui::MouseButton::Left),
            modifiers: gpui::Modifiers::default(),
        });
        if release_delivered {
            vcx.simulate_event(gpui::MouseUpEvent {
                button: gpui::MouseButton::Left,
                position: point(px(-10.), px(-10.)),
                modifiers: gpui::Modifiers::default(),
                click_count: 1,
            });
        }
        // No button held: neither a delivered nor a missed mouse-up may leave
        // a sticky selection that changes when merely hovering the transcript.
        vcx.simulate_event(gpui::MouseMoveEvent {
            position: start,
            pressed_button: None,
            modifiers: gpui::Modifiers::default(),
        });
        vcx.run_until_parked();
        panel.update(vcx, |panel, cx| {
            panel
                .transcript_selection
                .update(cx, |selection, cx| selection.copy(cx))
        });
        let copied = vcx
            .update(|_, cx| cx.read_from_clipboard())
            .and_then(|item| item.text());
        assert_eq!(
            copied.as_deref(),
            Some("Select this transcript text"),
            "release delivered: {release_delivered}"
        );
    }
}
