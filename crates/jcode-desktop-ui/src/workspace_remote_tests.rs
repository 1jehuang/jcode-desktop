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
        assert_eq!(w.sidebar_view, SidebarView::Machines)
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
        assert_eq!(w.slots.len(), 1);
    });
    // All generic and pinned new-panel shortcuts honor the remote preference,
    // without accidentally forwarding this computer's pinned directory.
    for key in ["super-n", "ctrl-alt-enter"] {
        vcx.simulate_keystrokes(key);
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
fn hiding_machines_returns_focus_to_the_existing_composer(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (workspace, vcx) = cx.add_window_view(|window, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.push_test_panel("existing", cx);
        w.open_machines(window, cx);
        w
    });
    vcx.run_until_parked();
    for hide_sidebar in [false, true] {
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| w.open_machines(window, cx));
        });
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            if hide_sidebar {
                w.show_sidebar = false;
            } else {
                w.sidebar_view = SidebarView::Sessions;
            }
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            assert_eq!(
                workspace.read(cx).navigation_state(window, cx)["keyboard_panel"],
                0
            );
        });
    }
}
