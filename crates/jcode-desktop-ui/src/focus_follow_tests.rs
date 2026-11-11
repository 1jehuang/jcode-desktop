//! The highlighted panel must always be the one that receives typing.
use super::*;

#[gpui::test]
fn clicking_transcript_background_unfocuses_until_typing(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_conversations(cx);
    let first = workspace.read_with(vcx, |w, _| w.test_panel(0).unwrap());
    let bounds = vcx.debug_bounds("panel-0").unwrap();
    let transcript = vcx.debug_bounds("transcript").unwrap();
    vcx.simulate_click(
        gpui::point(bounds.center().x, transcript.center().y),
        Default::default(),
    );
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        assert!(
            !first
                .read(cx)
                .input
                .read(cx)
                .focus_handle
                .is_focused(window)
        );
    });
    let reading_focus = vcx.update(|window, cx| window.focused(cx).unwrap());
    vcx.simulate_keystrokes("ctrl-c");
    vcx.run_until_parked();
    vcx.update(|window, _| assert!(reading_focus.is_focused(window)));
    vcx.simulate_keystrokes("h e l l o");
    vcx.run_until_parked();
    first.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "hello");
    });
    vcx.update(|window, cx| {
        assert!(
            first
                .read(cx)
                .input
                .read(cx)
                .focus_handle
                .is_focused(window)
        );
    });
}

#[gpui::test]
fn typing_from_workspace_targets_active_composer_and_preserves_first_key(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = two_panels(cx);
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            w.set_active(1, cx);
            window.focus(&w.focus_handle, cx);
        })
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("H i space !");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(
            w.test_panel(1)
                .unwrap()
                .read(cx)
                .input
                .read(cx)
                .content
                .as_ref(),
            "Hi !"
        );
        assert!(
            w.test_panel(0)
                .unwrap()
                .read(cx)
                .input
                .read(cx)
                .content
                .is_empty()
        );
    });
}

#[gpui::test]
fn typing_in_rename_field_does_not_focus_composer(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_panels(cx);
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            w.rename_session(&RenameSession, window, cx);
        })
    });
    vcx.run_until_parked();
    let rename_focus = vcx.update(|window, cx| window.focused(cx).unwrap());
    vcx.simulate_keystrokes("n e w");
    vcx.run_until_parked();
    vcx.update(|window, _| assert!(rename_focus.is_focused(window)));
    workspace.read_with(vcx, |w, cx| {
        assert!(w.rename_editor.is_some());
        assert!(
            w.test_panel(0)
                .unwrap()
                .read(cx)
                .input
                .read(cx)
                .content
                .is_empty()
        );
    });
}

#[gpui::test]
fn navigation_and_shortcuts_do_not_resume_composer(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_panels(cx);
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            window.focus(&w.focus_handle, cx);
        })
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("left ctrl-c escape");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        assert!(workspace.read(cx).focus_handle.is_focused(window));
    });
}

fn two_panels(cx: &mut gpui::TestAppContext) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.show_sidebar = false;
        for name in ["first", "second"] {
            workspace.push_test_panel(name, cx);
        }
        for slot in &mut workspace.slots {
            slot.width_fraction = 0.5;
            slot.animated_width =
                AnimatedValue::new(0.5, transition::policy(Transition::PanelWidth).duration);
        }
        workspace.active = 0;
        workspace
    });
    vcx.update(|window, cx| {
        workspace
            .read(cx)
            .test_panel(0)
            .unwrap()
            .read(cx)
            .input_focus_handle(cx)
            .focus(window, cx)
    });
    vcx.run_until_parked();
    (workspace, vcx)
}

/// Two side-by-side sessions with history, so each composer is docked over
/// its transcript the way it is in a real conversation.
fn two_conversations(
    cx: &mut gpui::TestAppContext,
) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    let (workspace, vcx) = two_panels(cx);
    for index in 0..2 {
        let panel = workspace.read_with(vcx, |w, _| w.test_panel(index).unwrap());
        panel.update(vcx, |panel, cx| panel.test_settled_conversation(40, cx));
    }
    vcx.run_until_parked();
    for _ in 0..120 {
        if vcx.debug_bounds("composer-dock").is_some() {
            break;
        }
        vcx.executor().advance_clock(Duration::from_millis(16));
        vcx.update(|window, cx| window.simulate_next_frame(cx));
        vcx.run_until_parked();
    }
    assert!(vcx.debug_bounds("composer-dock").is_some());
    (workspace, vcx)
}

#[gpui::test]
fn keyboard_focus_entering_another_panel_activates_it(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_panels(cx);
    let second = workspace.read_with(vcx, |w, _| w.test_panel(1).unwrap());
    // Many paths focus a composer directly: panel buttons, restored drafts,
    // voice, and pickers. None of them go through `set_active`.
    vcx.update(|window, cx| second.read(cx).input_focus_handle(cx).focus(window, cx));
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert_eq!(
            w.active, 1,
            "the panel that takes typing must be highlighted"
        );
    });
    vcx.simulate_input("hello");
    vcx.run_until_parked();
    second.read_with(vcx, |panel, cx| {
        assert_eq!(panel.input.read(cx).content.as_ref(), "hello");
    });
}

#[gpui::test]
fn pressing_in_another_composer_and_releasing_elsewhere_activates_it(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = two_panels(cx);
    let first = vcx.debug_bounds("panel-0").unwrap();
    let second = vcx.debug_bounds("panel-1").unwrap();
    let composer = gpui::point(second.center().x, second.bottom() - px(40.));
    vcx.simulate_mouse_down(composer, gpui::MouseButton::Left, Default::default());
    vcx.simulate_mouse_up(first.center(), gpui::MouseButton::Left, Default::default());
    vcx.run_until_parked();
    let focused = workspace.read_with(vcx, |w, _| w.test_panel(1).unwrap());
    let typing_in_second =
        vcx.update(|window, cx| focused.read(cx).input_focus_handle(cx).is_focused(window));
    workspace.read_with(vcx, |w, _| {
        assert_eq!(
            typing_in_second,
            w.active == 1,
            "highlight and typing target disagree"
        );
    });
}

#[gpui::test]
fn activating_another_panel_moves_typing_to_it(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_panels(cx);
    // Sidebar rows, tabs and commands activate a panel without touching focus.
    workspace.update(vcx, |w, cx| w.set_active(1, cx));
    vcx.run_until_parked();
    vcx.simulate_input("hello");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.active, 1);
        let text = |index| {
            w.test_panel(index)
                .unwrap()
                .read(cx)
                .input
                .read(cx)
                .content
                .clone()
        };
        assert_eq!(text(1).as_ref(), "hello");
        assert_eq!(
            text(0).as_ref(),
            "",
            "the outgoing session must not receive the prompt"
        );
    });
}

/// The recorded bug: with transcripts present the composer is docked and
/// blocks the mouse from the panel surface, so a click in the other session's
/// composer moved the caret there while the previous session stayed
/// highlighted.
#[gpui::test]
fn clicking_another_docked_composer_highlights_its_panel(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_conversations(cx);
    let composer = vcx.debug_bounds("prompt-input").unwrap();
    let panels = [
        vcx.debug_bounds("panel-0").unwrap(),
        vcx.debug_bounds("panel-1").unwrap(),
    ];
    for target in [1, 0, 1] {
        let point = gpui::point(panels[target].center().x, composer.center().y);
        vcx.simulate_click(point, gpui::Modifiers::default());
        vcx.run_until_parked();
        let panel = workspace.read_with(vcx, |w, _| w.test_panel(target).unwrap());
        let typing = vcx.update(|window, cx| {
            panel
                .read(cx)
                .input
                .read(cx)
                .focus_handle
                .is_focused(window)
        });
        assert!(typing, "the click must put the caret in composer {target}");
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.active, target, "the clicked session must be highlighted");
        });
    }
}

/// The dock's pills and veil are part of the session too.
#[gpui::test]
fn clicking_the_dock_beside_the_composer_activates_its_panel(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = two_conversations(cx);
    let composer = vcx.debug_bounds("prompt-input").unwrap();
    let second = vcx.debug_bounds("panel-1").unwrap();
    // Just above the input, inside the veil.
    let point = gpui::point(second.center().x, composer.top() - px(6.));
    vcx.simulate_click(point, gpui::Modifiers::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| assert_eq!(w.active, 1));
}
