use super::*;
use std::sync::mpsc::Receiver;

fn setup(
    cx: &mut gpui::TestAppContext,
) -> (
    Entity<Workspace>,
    &mut gpui::VisualTestContext,
    Receiver<Command>,
) {
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
    });
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.set_test_bridge(bridge);
        w.push_test_panel("session_fox_original", cx);
        w.push_test_panel("session_owl_neighbor", cx);
        w.sessions.push(crate::workspace::tests::session_info(
            "session_fox_original",
            Some("Original"),
        ));
        w
    });
    vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
    vcx.run_until_parked();
    (workspace, vcx, commands)
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx.debug_bounds(selector).expect(selector);
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

#[test]
fn title_validation_trims_preserves_unicode_and_rejects_blank_or_multiline() {
    assert_eq!(
        validated_title("  Release 🦊 日本語  "),
        Ok("Release 🦊 日本語".into())
    );
    assert!(validated_title(" \n\t ").is_err());
    assert!(validated_title("one\ntwo").is_err());
    assert_eq!(validated_title("/help"), Ok("/help".into()));
}

#[gpui::test]
fn f2_enter_renames_selected_session_without_consuming_composer(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    vcx.simulate_keystrokes("d r a f t");
    vcx.simulate_keystrokes("f2");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("rename-session-dialog").is_some());
    workspace.read_with(vcx, |w, cx| {
        let editor = w.rename_editor.as_ref().unwrap();
        let snapshot = editor.input.read(cx).snapshot();
        assert_eq!(snapshot.selection_start, 0);
        assert_eq!(snapshot.selection_end, snapshot.content.len());
    });
    vcx.simulate_keystrokes("R e l e a s e enter");
    vcx.run_until_parked();
    match commands.try_recv().unwrap() {
        Command::SessionOperation {
            session_id,
            operation: harness::SessionOperation::Rename(Some(title)),
        } => {
            assert_eq!(session_id, "session_fox_original");
            assert_eq!(title, "Release");
        }
        _ => panic!("expected rename only"),
    }
    workspace.read_with(vcx, |w, cx| {
        assert!(w.rename_editor.is_none());
        let panel = w.slots[0].panel.read(cx);
        assert_eq!(panel.input.read(cx).content.as_ref(), "draft");
    });
    vcx.update(|window, cx| {
        workspace.read_with(cx, |w, cx| {
            assert!(
                w.slots[0]
                    .panel
                    .read(cx)
                    .input
                    .read(cx)
                    .focus_handle
                    .is_focused(window)
            );
        })
    });
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn button_cancel_escape_and_blank_save_never_rename(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "rename-session-button");
    click(vcx, "rename-session-cancel");
    assert!(workspace.read_with(vcx, |w, _| w.rename_editor.is_none()));
    click(vcx, "rename-session-button");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(workspace.read_with(vcx, |w, _| w.rename_editor.is_none()));
    click(vcx, "rename-session-button");
    vcx.simulate_keystrokes("backspace");
    click(vcx, "rename-session-save");
    assert!(workspace.read_with(vcx, |w, _| {
        w.rename_editor.as_ref().unwrap().error.is_some()
    }));
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn save_keeps_captured_identity_and_server_event_updates_titles(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "rename-session-button");
    workspace.update(vcx, |w, cx| {
        w.active = 1;
        w.rename_editor
            .as_ref()
            .unwrap()
            .input
            .update(cx, |input, cx| {
                input.set_content("  New title 🦊  ".into(), cx)
            });
    });
    click(vcx, "rename-session-save");
    match commands.try_recv().unwrap() {
        Command::SessionOperation {
            session_id,
            operation: harness::SessionOperation::Rename(Some(title)),
        } => {
            assert_eq!(session_id, "session_fox_original");
            assert_eq!(title, "New title 🦊");
        }
        _ => panic!("expected rename"),
    }
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::Event {
                session_id: "session_fox_original".into(),
                event: jcode_sdk::ApiEvent::SessionRenamed {
                    session_id: "session_fox_original".into(),
                    title: Some("New title 🦊".into()),
                    display_title: "New title 🦊".into(),
                },
            },
            cx,
        );
        assert_eq!(w.slots[0].panel.read(cx).title.as_ref(), "New title 🦊");
        assert_eq!(sidebar_session_title(&w.sessions[0]).1, "New title 🦊");
        let saved = w.slots[0].panel.read(cx).snapshot(cx);
        assert_eq!(saved.title, "New title 🦊");
        assert_eq!(
            w.slots[1].panel.read(cx).title.as_ref(),
            "session_owl_neighbor"
        );
    });
}

#[gpui::test]
fn non_session_panels_and_pending_drafts_cannot_be_renamed(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    for id in [
        "startup://draft/1",
        "desktop://changelog",
        "gmail://inbox",
        "todoist://tasks",
        "settings://machines",
    ] {
        workspace.update(vcx, |w, cx| {
            w.slots[0]
                .panel
                .update(cx, |panel, _| panel.session_id = id.into())
        });
        vcx.simulate_keystrokes("f2");
        vcx.run_until_parked();
        assert!(workspace.read_with(vcx, |w, _| w.rename_editor.is_none()));
        assert!(vcx.debug_bounds("rename-session-button").is_none());
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn enter_validates_blank_and_preserves_literal_slash_titles(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    vcx.simulate_keystrokes("f2 backspace enter");
    vcx.run_until_parked();
    assert!(workspace.read_with(vcx, |w, _| {
        w.rename_editor.as_ref().unwrap().error.is_some()
    }));
    assert!(commands.try_recv().is_err());
    vcx.simulate_keystrokes("/ m o d e l space p l a n enter");
    vcx.run_until_parked();
    match commands.try_recv().unwrap() {
        Command::SessionOperation {
            operation: harness::SessionOperation::Rename(Some(title)),
            ..
        } => assert_eq!(title, "/model plan"),
        _ => panic!("expected literal title rename"),
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn closed_target_does_not_rename_its_neighbor(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "rename-session-button");
    workspace.update(vcx, |w, cx| {
        w.slots[0].closing = true;
        w.save_session_title("Never rename neighbor", cx);
        assert!(w.rename_editor.as_ref().unwrap().error.is_some());
    });
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn busy_session_retains_title_for_retry_after_turn(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "rename-session-button");
    workspace.update(vcx, |w, cx| {
        w.slots[0]
            .panel
            .update(cx, |panel, _| panel.status = "running".into());
        w.rename_editor
            .as_ref()
            .unwrap()
            .input
            .update(cx, |input, cx| input.set_content("My title".into(), cx));
    });
    click(vcx, "rename-session-save");
    workspace.read_with(vcx, |w, cx| {
        let editor = w.rename_editor.as_ref().unwrap();
        assert!(editor.error.is_some());
        assert_eq!(editor.input.read(cx).content.as_ref(), "My title");
    });
    assert!(commands.try_recv().is_err());
    workspace.update(vcx, |w, cx| {
        w.slots[0]
            .panel
            .update(cx, |panel, _| panel.status = "idle".into())
    });
    click(vcx, "rename-session-save");
    assert!(
        matches!(commands.try_recv(), Ok(Command::SessionOperation { operation: harness::SessionOperation::Rename(Some(title)), .. }) if title == "My title")
    );
}

#[gpui::test]
fn rename_button_stays_clickable_without_squeezing_compact_tabs(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    let handle = vcx.update(|window, _| window.window_handle());
    for width in [1440.0, 640.0, 420.0, 360.0] {
        vcx.simulate_window_resize(handle, gpui::size(px(width), px(700.0)));
        vcx.run_until_parked();
        let button = vcx.debug_bounds("rename-session-button");
        let fps = vcx.debug_bounds("fps-counter").unwrap();
        let tabs = vcx.debug_bounds("live-session-tabs").unwrap();
        assert!(tabs.size.width > px(0.0), "width={width}");
        if let Some(button) = button {
            let tab = vcx.debug_bounds("live-session-tab-0").unwrap();
            assert!(button.left() >= tab.left() && button.right() <= tab.right());
            assert!(button.left() >= fps.right());
            click(vcx, "rename-session-button");
        } else {
            // Tiny exposed tabs keep their label and use the keyboard action.
            vcx.simulate_keystrokes("f2");
            vcx.run_until_parked();
        }
        assert!(workspace.read_with(vcx, |w, _| w.rename_editor.is_some()));
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn save_with_label_names_the_session_in_tabs_and_resume(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.slots[0].panel.update(cx, |panel, cx| {
            assert!(panel.handle_slash_command_for_test("/save  yc mcp ", cx));
        });
    });
    match commands.try_recv().unwrap() {
        Command::SessionOperation {
            session_id,
            operation: harness::SessionOperation::SetSaved(true, Some(label)),
        } => {
            assert_eq!(session_id, "session_fox_original");
            assert_eq!(label, "yc mcp");
        }
        _ => panic!("expected save"),
    }
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionSaved {
                session_id: "session_fox_original".into(),
                saved: true,
                label: Some("yc mcp".into()),
            },
            cx,
        );
        assert_eq!(w.slots[0].panel.read(cx).title.as_ref(), "yc mcp");
        assert!(w.sessions[0].saved);
        assert_eq!(w.sessions[0].save_label.as_deref(), Some("yc mcp"));
        assert_eq!(sidebar_session_title(&w.sessions[0]).1, "yc mcp");
        assert_eq!(
            w.slots[1].panel.read(cx).title.as_ref(),
            "session_owl_neighbor"
        );
    });
    assert!(matches!(commands.try_recv(), Ok(Command::RefreshSessions)));

    workspace.update(vcx, |w, cx| {
        w.slots[0].panel.update(cx, |panel, cx| {
            assert!(panel.handle_slash_command_for_test("/unsave", cx));
        });
    });
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::SessionOperation {
            operation: harness::SessionOperation::SetSaved(false, None),
            ..
        })
    ));
}
