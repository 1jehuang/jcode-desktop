//! Real native editor tests with deliberately withheld backend creation replies.
use super::*;
use base64::Engine as _;

fn created(id: &str) -> jcode_sdk::SessionInfo {
    jcode_sdk::SessionInfo {
        session_id: id.into(),
        title: None,
        working_dir: Some("/project".into()),
        status: "idle".into(),
        transcript_bytes: None,
        saved: false,
        updated_at_ms: None,
        last_active_at_ms: None,
        archived: false,
        archived_at_ms: None,
    }
}

fn request(commands: &std::sync::mpsc::Receiver<Command>) -> (String, Option<String>) {
    match commands
        .try_recv()
        .expect("native panel must request creation")
    {
        Command::CreateSession {
            request_id: Some(id),
            working_dir,
        } => (id, working_dir),
        _ => panic!("expected correlated create, never Watch on a placeholder"),
    }
}

fn setup(
    cx: &mut gpui::TestAppContext,
) -> (
    Entity<Workspace>,
    &mut gpui::VisualTestContext,
    std::sync::mpsc::Receiver<Command>,
) {
    cx.update(crate::bind_workspace_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.remotes.default_host = None;
        w.push_test_panel("existing", cx);
        w.restore_focus(window, cx);
        w
    });
    vcx.run_until_parked();
    (workspace, vcx, commands)
}

fn add_image(input: &Entity<PromptInput>, cx: &mut gpui::VisualTestContext) {
    input.update(cx, |input, cx| {
        let mut snapshot = input.snapshot();
        snapshot.attachments.push(crate::input::AttachmentSnapshot {
            media_type: "image/png".into(),
            encoded: base64::engine::general_purpose::STANDARD
                .encode(include_bytes!("../../../assets/previews/image-preview.png")),
            label: "pending image".into(),
        });
        input.restore(snapshot, cx);
    });
}

#[gpui::test]
fn local_draft_is_immediately_editable_and_attaches_without_replacing_editor(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx);
    let start = Instant::now();
    vcx.simulate_keystrokes("super-n");
    vcx.run_until_parked();
    let (id, _) = request(&commands);
    let panel = workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert_eq!(w.active, 1);
        assert!(!w.slots[1].animated_width.is_animating());
        assert_eq!(w.slots[1].panel.read(cx).session_id, id);
        assert!(!w.slots[1].panel.read(cx).can_fork());
        w.slots[1].panel.clone()
    });
    eprintln!(
        "optimistic native key-to-mounted-editor: {:?} (backend reply withheld)",
        start.elapsed()
    );
    let input = panel.read_with(vcx, |p, _| p.input.clone());
    add_image(&input, vcx);
    vcx.simulate_input("first");
    vcx.simulate_input(" second");
    vcx.simulate_keystrokes("shift-left");
    let before = input.read_with(vcx, |i, _| i.snapshot());
    eprintln!(
        "optimistic native panel request-to-editable: {:?} (backend reply withheld)",
        start.elapsed()
    );
    assert_eq!(before.content, "first second");
    assert_eq!(before.attachments.len(), 1);
    assert_ne!(before.selection_start, before.selection_end);
    vcx.simulate_keystrokes("enter");
    assert!(
        commands.try_recv().is_err(),
        "submission stays disabled while initializing"
    );
    // More than the measured 1.8s backend delay passes with the editor mounted.
    vcx.executor().advance_clock(Duration::from_secs(2));
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionCreated {
                session: created("ready"),
                request_id: Some(id),
            },
            cx,
        );
        assert_eq!(w.slots[1].panel.entity_id(), panel.entity_id());
        assert_eq!(panel.read(cx).input.entity_id(), input.entity_id());
        assert_eq!(input.read(cx).snapshot(), before);
    });
    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(
        input.read_with(vcx, |i, _| i.content.to_string()),
        "first secon"
    );
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, images }) if session_id == "ready" && content == "first secon" && images.len() == 1)
    );
}

#[gpui::test]
fn concurrent_drafts_attach_out_of_order_without_stealing_focus(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    vcx.simulate_keystrokes("super-n");
    vcx.run_until_parked();
    let (first, _) = request(&commands);
    vcx.simulate_input("first draft");
    vcx.simulate_keystrokes("super-n");
    vcx.run_until_parked();
    let (second, _) = request(&commands);
    assert_ne!(first, second);
    vcx.simulate_input("second draft");
    workspace.update(vcx, |w, cx| {
        let focused = w.slots[w.active].panel.entity_id();
        for (id, ready) in [(second, "second-ready"), (first, "first-ready")] {
            w.apply(
                Update::SessionCreated {
                    session: created(ready),
                    request_id: Some(id),
                },
                cx,
            );
            assert_eq!(w.slots[w.active].panel.entity_id(), focused);
            assert!(!w.focus_pending);
        }
        assert_eq!(w.slots.len(), 3);
        assert_eq!(w.slots[1].panel.read(cx).session_id, "first-ready");
        assert_eq!(
            w.slots[1].panel.read(cx).input.read(cx).content.as_ref(),
            "first draft"
        );
        assert_eq!(w.slots[2].panel.read(cx).session_id, "second-ready");
    });
    vcx.simulate_input(" stays focused");
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, .. }) if session_id == "second-ready" && content == "second draft stays focused")
    );
}

#[gpui::test]
fn closed_pending_draft_never_resurrects_even_during_close_animation(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx);
    for during_animation in [true, false] {
        vcx.simulate_keystrokes("super-n");
        vcx.run_until_parked();
        let (id, _) = request(&commands);
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.close_panel(&ClosePanel, window, cx);
                if !during_animation {
                    w.remove_finished_closing_panels(
                        Instant::now() + Duration::from_secs(2),
                        window,
                        cx,
                    );
                }
                let active = w.slots[w.active].panel.entity_id();
                w.apply(
                    Update::SessionCreated {
                        session: created("abandoned"),
                        request_id: Some(id),
                    },
                    cx,
                );
                assert_eq!(w.slots[w.active].panel.entity_id(), active);
                assert!(
                    w.slots
                        .iter()
                        .all(|s| s.panel.read(cx).session_id != "abandoned")
                );
            })
        });
        assert!(
            matches!(commands.try_recv(), Ok(Command::Unwatch { session_id }) if session_id == "abandoned")
        );
        assert!(commands.try_recv().is_err());
        vcx.run_until_parked();
    }
}

#[gpui::test]
fn pending_failure_is_visible_and_keeps_draft_unsent(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    vcx.simulate_keystrokes("super-n");
    vcx.run_until_parked();
    let (id, _) = request(&commands);
    vcx.simulate_input("do not lose me");
    workspace.update(vcx, |w, cx| {
        w.apply(Update::CommandFailed { session_id: id, reason: "agent init failed".into() }, cx);
        let p = w.slots[w.active].panel.read(cx);
        assert!(p.status.contains("failed"));
        assert!(p.items.iter().any(|item| matches!(item, crate::panel::Item::Error(reason) if reason == "agent init failed")));
    });
    vcx.simulate_input(" still editable");
    vcx.simulate_keystrokes("enter");
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(
            w.slots[w.active]
                .panel
                .read(cx)
                .input
                .read(cx)
                .content
                .as_ref(),
            "do not lose me still editable"
        )
    });
    assert!(commands.try_recv().is_err());
    vcx.simulate_keystrokes("escape");
    assert!(
        commands.try_recv().is_err(),
        "pending drafts must never send Cancel to a placeholder"
    );
}

#[gpui::test]
fn reload_restarts_only_live_pending_drafts_with_fresh_ids_and_original_local_paths(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.open_local_draft(Some("/pinned/local".into()), cx)
    });
    vcx.run_until_parked();
    let (old, _) = request(&commands);
    let input = workspace.read_with(vcx, |w, cx| w.slots[w.active].panel.read(cx).input.clone());
    add_image(&input, vcx);
    vcx.simulate_input("restored draft");
    vcx.simulate_keystrokes("shift-left");
    let before = workspace.read_with(vcx, |w, cx| {
        w.slots[w.active].panel.read(cx).input.read(cx).snapshot()
    });
    workspace.update(vcx, |w, cx| {
        w.open_local_draft(Some("/closed".into()), cx);
        w.slots[w.active].closing = true;
        w.active = 1;
        w.remotes.default_host = Some("different-machine".into());
    });
    let (closed, _) = request(&commands);
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            let encoded = w.snapshot(window, cx).unwrap().encode().unwrap();
            let snapshot = WorkspaceSnapshot::decode(&encoded).unwrap();
            assert_eq!(snapshot.slots.len(), 2);
            assert_eq!(snapshot.active, 1);
            w.apply_snapshot(snapshot, cx);
            w.restore_focus(window, cx);
        })
    });
    // Existing real sessions reconnect normally. Pending placeholders never Watch.
    assert!(
        matches!(commands.try_recv(), Ok(Command::Watch { session_id }) if session_id == "existing")
    );
    let (new, directory) = request(&commands);
    assert_eq!(directory.as_deref(), Some("/pinned/local"));
    assert_ne!(old, new);
    assert!(commands.try_recv().is_err());
    workspace.update(vcx, |w, cx| {
        assert_eq!(w.slots[1].panel.read(cx).input.read(cx).snapshot(), before);
        for stale in [old, closed] {
            w.apply(
                Update::SessionCreated {
                    session: created("stale"),
                    request_id: Some(stale),
                },
                cx,
            );
        }
        assert_eq!(w.slots.len(), 2);
        assert_eq!(w.slots[1].panel.read(cx).session_id, new);
        w.apply(
            Update::SessionCreated {
                session: created("restored-ready"),
                request_id: Some(new),
            },
            cx,
        );
        assert_eq!(w.slots[1].panel.read(cx).input.read(cx).snapshot(), before);
    });
    assert_eq!(
        commands
            .try_iter()
            .filter(|c| matches!(c, Command::Unwatch { .. }))
            .count(),
        2
    );
    vcx.run_until_parked();
    vcx.simulate_keystrokes("end");
    vcx.simulate_input(" after reload");
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, .. }) if session_id == "restored-ready" && content == "restored draft after reload")
    );
}

#[gpui::test]
fn help_prompt_is_correlated_and_closed_help_never_submits(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| w.new_help_session(&NewHelpSession, window, cx))
    });
    let (help, _) = request(&commands);
    workspace.update(vcx, |w, cx| w.open_local_draft(None, cx));
    let (other, _) = request(&commands);
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionCreated {
                session: created("other"),
                request_id: Some(other),
            },
            cx,
        );
        assert!(
            commands.try_recv().is_err(),
            "another creation must not consume the help prompt"
        );
        w.apply(
            Update::SessionCreated {
                session: created("help"),
                request_id: Some(help),
            },
            cx,
        );
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, .. }) if session_id == "help" && content == HELP_SESSION_PROMPT)
    );
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            w.new_help_session(&NewHelpSession, window, cx);
            w.close_panel(&ClosePanel, window, cx);
        })
    });
    let (closed, _) = request(&commands);
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionCreated {
                session: created("closed-help"),
                request_id: Some(closed),
            },
            cx,
        );
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Unwatch { session_id }) if session_id == "closed-help")
    );
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn pending_editor_is_fully_visible_on_first_frame_from_full_width_and_overflow(
    cx: &mut gpui::TestAppContext,
) {
    for width in [1440.0, 800.0] {
        for layout in [
            crate::config::LayoutMode::Normal,
            crate::config::LayoutMode::FolderTabs,
        ] {
            let (workspace, vcx, commands) = setup(cx);
            vcx.simulate_resize(gpui::size(px(width), px(1000.0)));
            workspace.update(vcx, |w, cx| {
                w.layout_mode = layout;
                w.slots[0].width_fraction = 1.0;
                w.slots[0].animated_width =
                    AnimatedValue::new(1.0, transition::policy(Transition::PanelWidth).duration);
                w.retarget_camera();
                cx.notify();
            });
            vcx.run_until_parked();
            for selector in ["panel-1", "panel-2", "panel-3"] {
                // Do not advance the clock or deliver any backend reply. The
                // initial painted frame must already expose the whole editor.
                vcx.simulate_keystrokes("super-n");
                vcx.run_until_parked();
                request(&commands);
                let canvas = vcx.debug_bounds("workspace-canvas").unwrap();
                let panel = vcx.debug_bounds(selector).unwrap();
                let input = vcx.debug_bounds("prompt-input").unwrap();
                assert!(
                    panel.left() >= canvas.left() - px(1.0),
                    "{width} {layout:?} {selector}: {panel:?} outside {canvas:?}"
                );
                assert!(
                    panel.right() <= canvas.right() + px(1.0),
                    "{width} {layout:?} {selector}: {panel:?} outside {canvas:?}"
                );
                assert!(
                    input.left() >= panel.left() && input.right() <= panel.right(),
                    "focused composer must be inside visible panel: {input:?}, {panel:?}"
                );
                assert!(input.size.width >= px(200.0));
                workspace.read_with(vcx, |w, _| {
                    assert!(w.camera_started[w.active_row].is_none());
                    assert!(w.slots.iter().all(|s| !s.animated_width.is_animating()));
                });
                vcx.simulate_input("Visible while starting");
                workspace.read_with(vcx, |w, cx| {
                    assert_eq!(
                        w.slots[w.active]
                            .panel
                            .read(cx)
                            .input
                            .read(cx)
                            .content
                            .as_ref(),
                        "Visible while starting"
                    );
                });
                assert!(commands.try_recv().is_err());
            }
        }
    }
}
