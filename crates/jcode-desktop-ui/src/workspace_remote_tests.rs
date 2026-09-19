//! Native controls and actual workspace commands, without a live runtime.
use super::*;

fn session(id: &str) -> jcode_sdk::SessionInfo {
    jcode_sdk::SessionInfo {
        session_id: id.into(),
        title: Some("Remote work".into()),
        working_dir: Some("/remote/project".into()),
        status: "idle".into(),
        transcript_bytes: None,
        saved: false,
        updated_at_ms: None,
        last_active_at_ms: None,
        archived: false,
        archived_at_ms: None,
        parent_session_id: None,
        agent_label: None,
        swarm_status: None,
        edit_stats: None,
    }
}

fn click(cx: &mut gpui::VisualTestContext, selector: &'static str) {
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing {selector}"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn settings_machine_entry_opens_the_connectable_picker(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.push_test_panel("existing", cx);
        w.sidebar_view = SidebarView::Settings;
        w
    });
    click(vcx, "settings-machines");
    assert!(vcx.debug_bounds("machines-picker").is_some());
    assert!(vcx.debug_bounds("machine-host-input").is_some());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.sidebar_view, SidebarView::Settings)
    });
}

#[gpui::test]
fn machines_picker_connect_default_shortcuts_and_local_override(cx: &mut gpui::TestAppContext) {
    cx.update(crate::bind_workspace_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.bridge = bridge;
        workspace.remotes.hosts = vec!["desktop".into()];
        workspace.push_test_panel("existing", cx);
        workspace
    });
    click(vcx, "machines-picker-button");
    assert!(vcx.debug_bounds("machines-picker").is_some());
    click(vcx, "machine-connect-1");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {host, working_dir: None, request_id: None}) if host == "desktop")
    );
    workspace.read_with(vcx, |w, _| assert_eq!(w.remotes.default_host, None));
    click(vcx, "machine-default-1");
    assert!(
        commands.try_recv().is_err(),
        "changing default must not open or move sessions"
    );
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.remotes.default_host.as_deref(), Some("desktop"));
        assert_eq!(w.slots.len(), 2);
    });
    // All generic and pinned new-panel shortcuts honor the remote preference,
    // without accidentally forwarding this computer's pinned directory.
    for key in ["super-n", "ctrl-alt-enter", "pointer"] {
        if key == "pointer" {
            click(vcx, "tab-new-session");
        } else {
            vcx.simulate_keystrokes(key);
        }
        assert!(
            matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {host, working_dir: None, ..}) if host == "desktop")
        );
    }
    click(vcx, "machine-connect-0");
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::CreateSession { .. })
    ));
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.remotes.default_host.as_deref(), Some("desktop"))
    });
    click(vcx, "machine-default-0");
    vcx.simulate_keystrokes("super-n");
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::CreateSession { .. })
    ));
}

#[gpui::test]
fn remote_new_panel_exposes_progress_failure_retry_and_explicit_local_choice(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(crate::bind_workspace_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.remotes.default_host = Some("desktop".into());
        w.remotes.hosts = vec!["desktop".into()];
        w.pinned_working_dir = Some("/local-only".into());
        for index in 0..29 {
            w.push_test_panel(&format!("existing-{index}"), cx);
        }
        w.restore_focus(window, cx);
        w
    });
    vcx.run_until_parked();

    for key in ["super-n", "ctrl-alt-enter", "pointer"] {
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.set_active(0, cx);
                w.focus_active(window, cx);
                cx.notify();
            });
        });
        vcx.run_until_parked();
        if key == "pointer" {
            click(vcx, "tab-new-session");
        } else {
            vcx.simulate_keystrokes(key);
        }
        vcx.run_until_parked();
        assert!(
            matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {
            host, working_dir: None, request_id: None,
        }) if host == "desktop")
        );
        assert!(vcx.debug_bounds("machine-connection-status").is_some());
        assert!(vcx.debug_bounds("machine-connect-0").is_some());
        vcx.update(|window, cx| {
            assert!(
                workspace
                    .read(cx)
                    .remotes
                    .input
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .focus_handle
                    .is_focused(window)
            );
        });
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(
                w.slots.len(),
                30,
                "repeated attempts reuse connection controls"
            );
            assert!(w.slots[w.active].panel.read(cx).is_machines());
            assert_eq!(w.remotes.status.as_deref(), Some("Connecting to desktop…"));
            assert!(!w.remotes.failed, "retry clears the previous error state");
        });
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::RemoteStatus {
                    host: "desktop".into(),
                    message: "Your session has expired. Please reauthenticate.".into(),
                    request_id: None,
                    failed: true,
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert!(w.remotes.failed);
            assert!(w.remotes.status.as_deref().unwrap().contains("expired"));
            assert!(w.slots[w.active].panel.read(cx).is_machines());
        });
        assert!(vcx.debug_bounds("machine-connection-status").is_some());
        assert!(
            commands.try_recv().is_err(),
            "failure must never fall back locally"
        );
    }

    click(vcx, "machine-connect-0");
    assert!(matches!(commands.try_recv(), Ok(Command::CreateSession {
        request_id: Some(id), ..
    }) if Panel::is_pending_session_id(&id)));
    vcx.simulate_input("local draft survives cloud failure");
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.remotes.default_host.as_deref(), Some("desktop"));
        let panel = w.slots[w.active].panel.read(cx);
        assert!(Panel::is_pending_session_id(&panel.session_id));
        assert_eq!(
            panel.input.read(cx).content.as_ref(),
            "local draft survives cloud failure"
        );
    });
}

#[gpui::test]
fn local_connect_keeps_new_panel_selected_after_mouse_release(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.remotes.default_host = Some("desktop".into());
        w.push_test_panel("existing", cx);
        w.open_machines(window, cx);
        w
    });
    vcx.run_until_parked();
    let position = vcx.debug_bounds("machine-connect-0").unwrap().center();
    vcx.simulate_mouse_down(position, gpui::MouseButton::Left, Default::default());
    // Native input can redraw and attach the session while the button is held.
    // A same-frame simulate_click misses the new ancestor mouse-up listener.
    vcx.run_until_parked();
    let draft = workspace.read_with(vcx, |w, cx| {
        w.slots[w.active].panel.read(cx).session_id.clone()
    });
    assert!(Panel::is_pending_session_id(&draft));
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::CreateSession { .. })
    ));
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionCreated {
                session: session("session_local"),
                request_id: Some(draft.clone()),
            },
            cx,
        );
        cx.notify();
    });
    vcx.run_until_parked();
    let release = vcx.debug_bounds("machine-connect-0").unwrap().center();
    vcx.simulate_mouse_up(release, gpui::MouseButton::Left, Default::default());
    vcx.run_until_parked();
    vcx.simulate_input("local panel remains selected");
    workspace.read_with(vcx, |w, cx| {
        let panel = w.slots[w.active].panel.read(cx);
        assert_eq!(panel.session_id, "session_local");
        assert_eq!(
            panel.input.read(cx).content.as_ref(),
            "local panel remains selected"
        );
        assert_eq!(w.remotes.default_host.as_deref(), Some("desktop"));
    });
    vcx.update(|window, cx| {
        let w = workspace.read(cx);
        assert!(
            w.slots[w.active]
                .panel
                .read(cx)
                .input
                .read(cx)
                .focus_handle
                .is_focused(window)
        );
    });
}

#[gpui::test]
fn cloud_new_panel_shows_wake_progress_before_any_session_exists(cx: &mut gpui::TestAppContext) {
    cx.update(crate::bind_workspace_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.remotes.default_host = Some("jcode-cloud-alpha".into());
        w.push_test_panel("existing", cx);
        w.restore_focus(window, cx);
        w
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-alt-enter");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("machine-connection-status").is_some());
    assert!(vcx.debug_bounds("cloud-alpha-status").is_some());
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert!(w.slots[w.active].panel.read(cx).is_machines());
        assert_eq!(w.remotes.default_host.as_deref(), Some("jcode-cloud-alpha"));
        assert!(
            w.remotes
                .status
                .as_deref()
                .unwrap()
                .starts_with("Waking jcode-cloud-alpha")
        );
    });
    // Test builds refuse the wake, so no remote creation or implicit local
    // fallback may be issued. The explicit recovery controls remain mounted.
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn explicit_folder_and_help_stay_local_with_a_remote_default(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.remotes.default_host = Some("desktop".into());
        w.push_test_panel("existing", cx);
        w
    });
    workspace.update(vcx, |w, cx| {
        w.folder_picker_dir = Some(PathBuf::from("/local/project"));
        w.choose_browsed_folder(cx);
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { working_dir: Some(dir), .. }) if dir == "/local/project")
    );
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| w.new_help_session(&NewHelpSession, window, cx));
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { request_id: Some(id), .. }) if id.starts_with("startup://draft/help/"))
    );
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.remotes.default_host.as_deref(), Some("desktop"))
    });
}

#[gpui::test]
fn machines_input_validates_and_connects_without_changing_default(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.bridge = bridge;
        workspace.push_test_panel("existing", cx);
        workspace
    });
    click(vcx, "machines-picker-button");
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| w.focus_active(window, cx));
    });
    click(vcx, "machine-host-input");
    vcx.simulate_input("user@desktop");
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {host, ..}) if host == "user@desktop")
    );
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.remotes.hosts, ["user@desktop"]);
        assert!(w.remotes.default_host.is_none());
    });
    vcx.simulate_input("button@desktop");
    click(vcx, "machine-connect-input");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateRemoteSession { host, .. }) if host == "button@desktop")
    );
    workspace.update(vcx, |w, cx| {
        w.connect_machine(Some("-oProxyCommand=bad".into()), cx)
    });
    vcx.run_until_parked();
    assert!(commands.try_recv().is_err());
    assert!(vcx.debug_bounds("machine-error").is_some());
    // Escape belongs to the host editor. The preceding button click may move
    // focus, so explicitly return to the editor before exercising its handler.
    click(vcx, "machine-host-input");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("machines-picker").is_none());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.sidebar_view, SidebarView::Sessions)
    });
}

#[gpui::test]
fn remote_startup_is_independent_of_local_runtime_and_promotes_draft(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.connected = false;
        w.remotes.default_host = Some("desktop".into());
        w.open_startup_draft(cx);
        w.start_default_startup(cx);
        w.restore_focus(window, cx);
        w
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {working_dir: None, request_id: Some(id), ..}) if id == Panel::STARTUP_SESSION_ID)
    );
    vcx.run_until_parked();
    vcx.simulate_input("keep my remote draft");
    workspace.update(vcx, |w, cx| {
        w.set_default_machine(None, cx);
        w.apply(Update::Connected, cx);
    });
    assert!(
        commands.try_recv().is_err(),
        "local readiness must not start the same draft twice after a default change"
    );
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionCreated {
                session: session("ssh://desktop/remote-1"),
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        assert_eq!(w.slots.len(), 1);
        assert_eq!(
            w.slots[0].panel.read(cx).session_id,
            "ssh://desktop/remote-1"
        );
    });
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send {session_id, content, ..}) if session_id == "ssh://desktop/remote-1" && content == "keep my remote draft")
    );
}

#[gpui::test]
fn remote_failure_survives_local_status_and_startup_retry_is_explicit(
    cx: &mut gpui::TestAppContext,
) {
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.remotes.default_host = Some("desktop".into());
        w.open_startup_draft(cx);
        w.open_machines(window, cx);
        w
    });
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::RemoteStatus {
                host: "desktop".into(),
                message: "SSH permission denied".into(),
                failed: true,
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        w.apply(Update::Status("local starting".into()), cx);
        cx.notify();
        assert!(
            w.remotes
                .status
                .as_deref()
                .unwrap()
                .contains("permission denied")
        );
    });
    click(vcx, "machine-retry-startup");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {request_id: Some(id), ..}) if id == Panel::STARTUP_SESSION_ID)
    );
    assert!(
        vcx.debug_bounds("machine-retry-startup").is_none(),
        "retry disabled while connection is in flight"
    );
}

#[gpui::test]
fn remote_identity_survives_snapshot_and_local_files_are_not_shown(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.active = w.open_session(session("ssh://user%40desktop/remote-1"), cx);
        w.sidebar_view = SidebarView::Files;
        w
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Watch {session_id}) if session_id == "ssh://user%40desktop/remote-1")
    );
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("panel-remote-host").is_some());
    workspace.read_with(vcx, |w, cx| {
        assert!(
            w.slots[w.active]
                .panel
                .read(cx)
                .session_id
                .starts_with("ssh://")
        )
    });
    assert!(vcx.debug_bounds("remote-file-browser-notice").is_some());
    assert!(
        vcx.debug_bounds("sidebar-file-list").is_none(),
        "unexpected local file bounds: {:?}",
        vcx.debug_bounds("sidebar-file-list")
    );
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            let bytes = w.snapshot(window, cx).unwrap().encode().unwrap();
            w.apply_snapshot(WorkspaceSnapshot::decode(&bytes).unwrap(), cx);
            assert_eq!(
                w.slots[0].panel.read(cx).session_id,
                "ssh://user%40desktop/remote-1"
            );
        });
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Watch {session_id}) if session_id == "ssh://user%40desktop/remote-1")
    );
}

#[test]
fn remote_history_never_becomes_a_local_pinned_folder() {
    let mut remote = session("ssh://desktop/remote-1");
    remote.working_dir = Some("/not/a/local/directory".into());
    assert_eq!(most_used_working_dir(&[remote.clone()]), None);
    assert_eq!(
        sidebar_session_directory(&remote).as_deref(),
        Some("desktop · /not/a/local/directory")
    );
}

#[gpui::test]
fn machines_panel_preserves_sidebar_reuses_tabs_and_restores_input(cx: &mut gpui::TestAppContext) {
    cx.update(crate::bind_workspace_keys);
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.push_test_panel("existing", cx);
        w.sidebar_view = SidebarView::Files;
        w
    });
    click(vcx, "machines-picker-button");
    vcx.simulate_input("user@reload-host");
    let original = workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.sidebar_view, SidebarView::Files);
        assert_eq!(w.slots.len(), 2);
        assert!(!w.slots[w.active].panel.read(cx).can_fork());
        w.slots[0].panel.entity_id()
    });
    click(vcx, "machines-picker-button");
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert_eq!(w.slots[0].panel.entity_id(), original);
        assert_eq!(
            w.slots[w.active]
                .panel
                .read(cx)
                .input
                .read(cx)
                .snapshot()
                .content,
            "user@reload-host"
        );
    });
    // Sidebar visibility and selected view are independent of the workspace panel.
    workspace.update(vcx, |w, cx| {
        w.show_sidebar = false;
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            assert_eq!(w.navigation_state(window, cx)["keyboard_panel"], 1);
            let bytes = w.snapshot(window, cx).unwrap().encode().unwrap();
            w.apply_snapshot(WorkspaceSnapshot::decode(&bytes).unwrap(), cx);
            w.restore_focus(window, cx);
            assert_eq!(w.navigation_state(window, cx)["keyboard_panel"], 1);
            assert_eq!(w.sidebar_view, SidebarView::Files);
            assert_eq!(
                w.slots[1].panel.read(cx).input.read(cx).snapshot().content,
                "user@reload-host"
            );
        });
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Watch {session_id}) if session_id == "existing")
    );
    assert!(
        commands.try_recv().is_err(),
        "Machines must never be watched"
    );
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateRemoteSession {host, ..}) if host == "user@reload-host")
    );
    vcx.simulate_keystrokes("super-h");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        workspace.read_with(cx, |w, cx| {
            assert_eq!(w.active, 0);
            assert_eq!(w.navigation_state(window, cx)["keyboard_panel"], 0);
        })
    });
    vcx.simulate_keystrokes("super-l");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        workspace.read_with(cx, |w, cx| {
            assert_eq!(w.active, 1);
            assert_eq!(w.navigation_state(window, cx)["keyboard_panel"], 1);
        })
    });
    vcx.simulate_keystrokes("super-q");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        assert!(w.machines_panel_index(cx).is_none());
        assert_eq!(w.slots[w.active].panel.read(cx).session_id, "existing");
    });
    assert!(
        commands.try_recv().is_err(),
        "Machines must never be unwatched"
    );
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            w.open_machines(window, cx);
            w.close_panel(&ClosePanel, window, cx);
            w.open_machines(window, cx);
            assert_eq!(w.slots.len(), 2);
            assert_eq!(
                w.slots
                    .iter()
                    .filter(|slot| slot.panel.read(cx).is_machines())
                    .count(),
                1
            );
        });
    });
    vcx.run_until_parked();
    click(vcx, "machines-panel-close");
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 1);
        assert!(w.machines_panel_index(cx).is_none());
    });
    assert!(commands.try_recv().is_err());
}

#[test]
fn legacy_machines_sidebar_restores_sessions() {
    assert_eq!(
        serde_json::from_str::<SidebarView>("\"Machines\"").unwrap(),
        SidebarView::Sessions
    );
}

#[gpui::test]
fn startup_enter_queues_and_inline_local_recovery_preserves_it_without_remote_fallback(
    cx: &mut gpui::TestAppContext,
) {
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.bridge = bridge;
        w.remotes.default_host = Some("offline-remote".into());
        w.open_startup_draft(cx);
        w.start_default_startup(cx);
        w.restore_focus(window, cx);
        w
    });
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::CreateRemoteSession { .. })
    ));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pending-session-status").is_some());
    vcx.simulate_input("send after connection");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("prompt-queue").is_some());
    assert!(
        commands.try_recv().is_err(),
        "never send a placeholder session ID"
    );
    let (panel_id, input_id) = workspace.read_with(vcx, |w, cx| {
        let panel = &w.slots[0].panel;
        assert!(panel.read(cx).input.read(cx).content.is_empty());
        (panel.entity_id(), panel.read(cx).input.entity_id())
    });
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::RemoteStatus {
                host: "offline-remote".into(),
                message: "credentials expired".into(),
                failed: true,
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pending-session-retry").is_some());
    assert!(
        vcx.debug_bounds("machines-picker").is_none(),
        "recovery is beside the prompt"
    );
    assert!(
        commands.try_recv().is_err(),
        "failure must not silently fall back locally"
    );
    click(vcx, "pending-session-retry");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateRemoteSession { host, request_id: Some(id), .. })
        if host == "offline-remote" && id == Panel::STARTUP_SESSION_ID)
    );
    assert!(vcx.debug_bounds("pending-session-retry").is_none());
    // Even while retrying, an explicit target choice is available. Preserve
    // unsent editor text as well as the already submitted waiting prompt.
    vcx.simulate_input("next draft");
    click(vcx, "pending-session-local");
    let request_id = match commands.try_recv().unwrap() {
        Command::CreateSession {
            request_id: Some(id),
            ..
        } => id,
        _ => panic!("explicit local recovery must create a correlated local session"),
    };
    assert_ne!(request_id, Panel::STARTUP_SESSION_ID);
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 1);
        assert_eq!(w.slots[0].panel.entity_id(), panel_id);
        let panel = w.slots[0].panel.read(cx);
        assert_eq!(panel.input.entity_id(), input_id);
        assert_eq!(panel.input.read(cx).content.as_ref(), "next draft");
        assert_eq!(w.remotes.default_host.as_deref(), Some("offline-remote"));
        assert_eq!(
            serde_json::to_value(panel.snapshot(cx).prompt_queue).unwrap()["prompts"][0]["content"],
            "send after connection"
        );
    });
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionCreated {
                session: session("ssh://offline-remote/late"),
                request_id: Some(Panel::STARTUP_SESSION_ID.into()),
            },
            cx,
        );
        // Late cloud status must not overwrite the local draft's status.
        w.update_startup_status("late remote failure", true, cx);
        assert_eq!(
            w.slots[0].panel.read(cx).status,
            "Connecting to this computer…"
        );
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::Unwatch { session_id }) if session_id == "ssh://offline-remote/late")
    );
    workspace.update(vcx, |w, cx| {
        w.apply(
            Update::SessionCreated {
                session: session("ready-local"),
                request_id: Some(request_id),
            },
            cx,
        );
        w.apply(
            Update::History {
                session_id: "ready-local".into(),
                messages: vec![],
                images: vec![],
            },
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, .. })
        if session_id == "ready-local" && content == "send after connection")
    );
    assert!(commands.try_recv().is_err());
    assert!(vcx.debug_bounds("pending-session-status").is_none());
    vcx.simulate_keystrokes("enter");
    assert!(
        matches!(commands.try_recv(), Ok(Command::Send { session_id, content, .. })
        if session_id == "ready-local" && content == "next draft")
    );
}

#[gpui::test]
fn cloud_startup_failure_is_visible_in_the_composer_without_opening_machines(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.remotes.default_host = Some("jcode-cloud-alpha".into());
        w.open_startup_draft(cx);
        w.restore_focus(window, cx);
        w
    });
    vcx.run_until_parked();
    vcx.simulate_input("preserve my draft");
    workspace.update(vcx, |w, cx| {
        // This is the same path used by the cloud lifecycle monitor.
        w.update_startup_status("Cloud wake failed: expired credentials", true, cx);
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pending-session-status").is_some());
    assert!(vcx.debug_bounds("pending-session-retry").is_some());
    assert!(vcx.debug_bounds("pending-session-local").is_some());
    assert!(vcx.debug_bounds("machines-picker").is_none());
    workspace.read_with(vcx, |w, cx| {
        let panel = w.slots[0].panel.read(cx);
        assert!(panel.status.contains("expired credentials"));
        assert_eq!(panel.input.read(cx).content.as_ref(), "preserve my draft");
    });
}
