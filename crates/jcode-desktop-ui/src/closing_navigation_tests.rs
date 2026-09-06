//! Keyboard navigation must ignore dismissed panels while their surfaces fade.
use super::*;

/// Platform repeats have no intervening key-up. Focus changes during dismissal
/// must not swallow the next repeat or target a still-fading panel.
#[gpui::test]
fn held_close_dismisses_each_live_panel_once(cx: &mut gpui::TestAppContext) {
    cx.update(crate::bind_workspace_keys);
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for name in ["first", "second", "third", "fourth"] {
            workspace.push_test_panel(name, cx);
        }
        for slot in &mut workspace.slots {
            slot.close_progress = AnimatedValue::new(1.0, Duration::from_secs(60));
        }
        workspace.set_active(0, cx);
        workspace
    });
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| workspace.focus_active(window, cx));
    });
    vcx.run_until_parked();
    let keystroke = gpui::Keystroke::parse("super-q").unwrap();
    for repeat in 0..6 {
        vcx.simulate_event(gpui::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: repeat > 0,
            prefer_character_input: false,
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert_eq!(
                workspace.slots.iter().filter(|slot| slot.closing).count(),
                (repeat + 1).min(4),
                "one live panel per keydown, including repeats after focus changes"
            );
        });
    }
    vcx.simulate_event(gpui::KeyUpEvent { keystroke });
    vcx.run_until_parked();
    // Closing the final panel must leave a functional workspace, not quit it.
    vcx.simulate_keystrokes("super-n");
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.slots.iter().filter(|slot| !slot.closing).count(), 1);
        assert!(!workspace.slots[workspace.active].closing);
    });
}

fn check_navigation_across_closing_panels(cx: &mut gpui::TestAppContext, right: bool) {
    cx.update(crate::bind_workspace_keys);
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for name in ["left", "closing-one", "closing-two", "right"] {
            workspace.push_test_panel(name, cx);
        }
        // Keep dismissal in flight independently of test executor wall time.
        for slot in &mut workspace.slots {
            slot.close_progress = AnimatedValue::new(1.0, Duration::from_secs(60));
        }
        workspace.set_active(1, cx);
        workspace
    });
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| workspace.focus_active(window, cx));
    });
    vcx.run_until_parked();
    for chord in ["ctrl-shift-w", "super-q"] {
        vcx.simulate_keystrokes(chord);
        vcx.run_until_parked();
    }
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.active, 3);
        assert!(workspace.slots[1].closing);
        assert!(workspace.slots[2].closing);
    });
    if right {
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.set_active(0, cx);
                workspace.focus_active(window, cx);
            });
        });
        vcx.run_until_parked();
    }
    let (chord, expected) = if right {
        ("super-l", 3)
    } else {
        ("super-h", 0)
    };
    vcx.simulate_keystrokes(chord);
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let workspace = workspace.read(cx);
        assert_eq!(
            workspace.active, expected,
            "{chord} must skip both closing slots"
        );
        assert!(!workspace.slots[workspace.active].closing);
        assert_eq!(
            workspace.navigation_state(window, cx)["keyboard_panel"],
            expected
        );
    });
    // Repeating into the outer edge remains a no-op, not a wraparound.
    vcx.simulate_keystrokes(chord);
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| assert_eq!(workspace.active, expected));
}

#[gpui::test]
fn focus_left_skips_closing_panels(cx: &mut gpui::TestAppContext) {
    check_navigation_across_closing_panels(cx, false);
}

#[gpui::test]
fn focus_right_skips_closing_panels(cx: &mut gpui::TestAppContext) {
    check_navigation_across_closing_panels(cx, true);
}

#[gpui::test]
fn close_remembers_surviving_panel_when_returning_to_row(cx: &mut gpui::TestAppContext) {
    cx.update(crate::bind_workspace_keys);
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for name in ["left", "closing", "survivor"] {
            workspace.push_test_panel(name, cx);
        }
        for slot in &mut workspace.slots {
            slot.close_progress = AnimatedValue::new(1.0, Duration::from_secs(60));
        }
        workspace.set_active(1, cx);
        workspace
    });
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| workspace.focus_active(window, cx));
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("super-q");
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.active, 2);
        assert_eq!(
            workspace.row_focus[0],
            Some(workspace.slots[2].panel.entity_id())
        );
    });
    vcx.simulate_keystrokes("super-j super-k");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active, 2);
        assert_eq!(workspace.navigation_state(window, cx)["keyboard_panel"], 2);
    });
}

/// Check every Linux Super binding, including less frequently taught aliases,
/// against the rendered root and composer dispatch paths. No external action
/// (mail, terminal, quit, etc.) is executed by this registration audit.
#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn all_super_shortcuts_have_handlers(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        crate::bind_workspace_keys(cx);
        // Production activation registers Quit at app scope.
        cx.on_action(|_: &Quit, _| {});
    });
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.push_test_panel("left", cx);
        workspace.push_test_panel("right", cx);
        workspace
    });
    let cases: Vec<(&str, Box<dyn gpui::Action>)> = vec![
        ("super-h", Box::new(FocusLeft)),
        ("super-l", Box::new(FocusRight)),
        ("super-j", Box::new(FocusDown)),
        ("super-k", Box::new(FocusUp)),
        ("super-left", Box::new(FocusLeft)),
        ("super-right", Box::new(FocusRight)),
        ("super-down", Box::new(FocusDown)),
        ("super-up", Box::new(FocusUp)),
        ("super-home", Box::new(FocusFirst)),
        ("super-end", Box::new(FocusLast)),
        ("super-u", Box::new(FocusFirst)),
        ("super-p", Box::new(FocusLast)),
        ("super-shift-h", Box::new(MovePanelLeft)),
        ("super-shift-l", Box::new(MovePanelRight)),
        ("super-shift-k", Box::new(MovePanelUp)),
        ("super-shift-j", Box::new(MovePanelDown)),
        ("super-shift-home", Box::new(MovePanelToFirst)),
        ("super-shift-end", Box::new(MovePanelToLast)),
        ("super-n", Box::new(NewPanel)),
        ("super-space", Box::new(ForkPanel)),
        ("super-t", Box::new(NewTerminal)),
        ("super-shift-g", Box::new(OpenGmail)),
        ("super-shift-d", Box::new(OpenTodoist)),
        ("super-enter", Box::new(NewPanelInPinnedDirectory)),
        ("super-;", Box::new(NewPanelInPinnedDirectory)),
        ("super-'", Box::new(NewPanel)),
        ("super-q", Box::new(ClosePanel)),
        ("super-tab", Box::new(FocusPrevious)),
        ("super-shift-tab", Box::new(ToggleOverview)),
        ("super-o", Box::new(ToggleOverview)),
        ("super-/", Box::new(ToggleHints)),
        ("super-shift-s", Box::new(ToggleShowcase)),
        ("super-shift-t", Box::new(CycleTheme)),
        ("super-b", Box::new(ToggleSidebar)),
        ("super-shift-/", Box::new(NewHelpSession)),
        ("super-r", Box::new(CycleWidth)),
        ("super-f", Box::new(MaximizeWidth)),
        ("super-1", Box::new(WidthPreset1)),
        ("super-2", Box::new(WidthPreset2)),
        ("super-3", Box::new(WidthPreset3)),
        ("super-4", Box::new(WidthPreset4)),
        ("super-shift-q", Box::new(Quit)),
    ];
    assert_eq!(
        include_str!("lib.rs")
            .matches("KeyBinding::new(\"super-")
            .count(),
        cases.len(),
        "every new Super binding must be added to the dispatch-path audit"
    );
    for composer in [false, true] {
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                if composer {
                    workspace.focus_active(window, cx);
                } else {
                    window.focus(&workspace.focus_handle, cx);
                }
            });
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let available = window.available_actions(cx);
            for (chord, action) in &cases {
                assert!(
                    available
                        .iter()
                        .any(|candidate| candidate.as_any().type_id() == action.as_any().type_id()),
                    "{chord} has no handler (composer={composer})"
                );
                assert!(
                    window
                        .bindings_for_action(action.as_ref())
                        .iter()
                        .any(|binding| binding
                            .keystrokes()
                            .iter()
                            .map(|key| key.unparse())
                            .collect::<Vec<_>>()
                            .join(" ")
                            == *chord),
                    "{chord} does not map to its advertised action (composer={composer})"
                );
            }
        });
    }
}
