use super::*;

#[gpui::test]
fn long_drafts_scroll_and_reveal_keyboard_caret_in_short_windows(cx: &mut gpui::TestAppContext) {
    cx.update(bind_keys);
    let (input, vcx) = cx.add_window_view(|window, cx| {
        let input = PromptInput::new(cx, "test", |_, _, _, _| {});
        window.focus(&input.focus_handle, cx);
        input
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for (width, height) in [(240., 240.), (320., 300.), (600., 600.)] {
        vcx.simulate_window_resize(handle, size(px(width), px(height)));
        input.update(vcx, |input, cx| {
            input.set_content(
                "a long prompt with many wrapped lines ".repeat(100).into(),
                cx,
            )
        });
        vcx.run_until_parked();
        let viewport = vcx.debug_bounds("prompt-editor").unwrap();
        assert!(viewport.size.height <= px((height * 0.25).min(160.)));
        assert!(viewport.right() <= px(width));
        for key in ["ctrl-home", "ctrl-end"] {
            vcx.simulate_keystrokes(key);
            vcx.run_until_parked();
            input.read_with(vcx, |input, _| {
                let bounds = input.last_bounds.unwrap();
                let line = input.last_layout.as_ref().unwrap();
                // Use the actual shaped line height, including theme scaling.
                let caret = line
                    .position_for_index(
                        input.cursor_offset(),
                        bounds.size.height / input.visual_line_count as f32,
                    )
                    .unwrap();
                let y = bounds.top() + caret.y;
                assert!(
                    y >= viewport.top() - px(1.) && y < viewport.bottom(),
                    "{key} caret {y:?} outside {viewport:?}"
                );
            });
        }
        let before = input.read_with(vcx, |input, _| input.editor_scroll.offset().y);
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(40.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Started,
        });
        vcx.run_until_parked();
        let after = input.read_with(vcx, |input, _| input.editor_scroll.offset().y);
        assert!(
            after > before,
            "wheel scroll should not snap back to the caret"
        );
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        input.read_with(vcx, |input, _| {
            assert!(input.content.is_empty());
            assert_eq!(input.visual_line_count, 1);
            assert_eq!(input.editor_scroll.offset().y, px(0.));
        });
    }
}
