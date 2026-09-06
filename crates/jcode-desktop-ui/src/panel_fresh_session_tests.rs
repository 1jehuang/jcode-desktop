use super::*;

#[gpui::test]
fn fresh_session_composer_is_centered_spacious_and_stable_while_typing(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.push_test_panel("session-a", cx);
        workspace
    });
    let panel = workspace
        .read_with(vcx, |workspace, _| workspace.test_panel(0))
        .unwrap();
    vcx.update(|window, cx| {
        let focus = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    });
    let handle = vcx.update(|window, _| window.window_handle());
    for (width, height) in [(800., 600.), (1440., 1000.), (640., 480.)] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(height)));
        vcx.run_until_parked();
        let fresh = vcx
            .debug_bounds("fresh-session")
            .expect("fresh session paints");
        let input = vcx.debug_bounds("prompt-input").expect("input paints");
        assert!(input.size.height >= px(112.));
        assert!(input.size.width <= px(760.));
        assert!(input.left() >= fresh.left() && input.right() <= fresh.right());
        assert!((f32::from(input.center().x - fresh.center().x)).abs() < 1.);
        assert!((f32::from(input.center().y - fresh.center().y)).abs() < 70.);
        assert!(input.bottom() < fresh.bottom() - px(40.));
    }
    let input = vcx.debug_bounds("prompt-input").unwrap();
    vcx.simulate_input("Plan a small project");
    vcx.run_until_parked();
    assert_eq!(vcx.debug_bounds("prompt-input"), Some(input));

    // A real keyboard submission moves the same focused editor to the bottom.
    vcx.simulate_keystrokes("enter");
    assert!(matches!(
        commands.recv_timeout(std::time::Duration::from_millis(100)),
        Ok(Command::Send { content, .. }) if content == "Plan a small project"
    ));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("fresh-session").is_none());
    let compact = vcx
        .debug_bounds("prompt-input")
        .expect("compact input paints");
    assert!(compact.size.height < input.size.height);
    assert!(compact.top() > input.bottom());
    vcx.simulate_input("Next message");
    panel.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "Next message");
    });
}

#[gpui::test]
fn fresh_session_slash_palette_stays_above_the_larger_composer(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.push_test_panel("session-a", cx);
        workspace
    });
    let panel = workspace
        .read_with(vcx, |workspace, _| workspace.test_panel(0))
        .unwrap();
    vcx.update(|window, cx| {
        let focus = panel.read(cx).input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
    });
    vcx.simulate_input("/help");
    vcx.run_until_parked();
    let input = vcx.debug_bounds("prompt-input").unwrap();
    let palette = vcx
        .debug_bounds("slash-command-overlay")
        .expect("palette paints");
    assert!(palette.bottom() <= input.top());
    assert!(vcx.debug_bounds("fresh-session").is_some());
}
