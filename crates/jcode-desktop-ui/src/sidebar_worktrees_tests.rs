use super::*;

fn entry(path: &str, branch: &str) -> Worktree {
    Worktree {
        path: path.into(),
        branch: Some(format!("refs/heads/{branch}")),
        head: "abc123456789".into(),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    }
}

fn setup(
    cx: &mut gpui::TestAppContext,
) -> (
    Entity<Workspace>,
    &mut gpui::VisualTestContext,
    std::sync::mpsc::Receiver<Command>,
) {
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
    });
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.set_test_bridge(bridge);
        w.push_test_panel("main-session", cx);
        w.slots[0]
            .panel
            .update(cx, |panel, _| panel.working_dir = Some("/project".into()));
        let mut session =
            crate::workspace::tests::session_info("main-session", Some("Main conversation"));
        session.working_dir = Some("/project/src".into());
        w.sessions.push(session);
        w.worktrees.directory = Some("/project".into());
        w.worktrees.entries = vec![
            entry("/project", "main"),
            entry("/project-trees/search", "feature/search"),
        ];
        w
    });
    vcx.run_until_parked();
    (workspace, vcx, commands)
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx.debug_bounds(selector).expect(selector);
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

#[test]
fn worktree_ownership_uses_components_and_most_specific_checkout() {
    let entries = vec![entry("/repo", "main"), entry("/repo/nested", "nested")];
    assert_eq!(owner("/repo/src", &entries), Some("/repo"));
    assert_eq!(owner("/repo/nested/src", &entries), Some("/repo/nested"));
    assert_eq!(owner("/repo-other", &entries), None);
}

#[test]
fn worktree_labels_cover_branch_detached_and_bare_checkouts() {
    let mut checkout = entry("/project", "feature/long/name");
    assert_eq!(checkout_label(&checkout), "feature/long/name");
    checkout.branch = None;
    checkout.detached = true;
    assert_eq!(checkout_label(&checkout), "Detached · abc12345");
    checkout.locked = Some("keep this checkout".into());
    assert!(checkout_available(&checkout));
    checkout.prunable = Some("missing".into());
    assert!(!checkout_available(&checkout));
    checkout.bare = true;
    assert_eq!(checkout_label(&checkout), "Bare repository");
}

#[gpui::test]
fn worktree_groups_are_compact_and_collapse_without_navigation(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "sidebar-mode-worktrees");
    let header = vcx.debug_bounds("worktree-0").unwrap();
    let chat = vcx.debug_bounds("worktree-session-main-session").unwrap();
    assert_eq!(header.size.height, px(30.0));
    assert_eq!(chat.size.height, px(28.0));
    assert!(chat.left() > header.left());
    assert!(
        vcx.debug_bounds("worktree-session-main-session-selected")
            .is_some()
    );
    click(vcx, "worktree-toggle-0");
    assert!(vcx.debug_bounds("worktree-session-main-session").is_none());
    assert!(vcx.debug_bounds("worktree-1").is_some());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.slots.len(), 1);
        assert!(w.worktrees.collapsed.contains("/project"));
    });
    assert!(commands.try_recv().is_err());
    click(vcx, "worktree-toggle-0");
    assert!(vcx.debug_bounds("worktree-session-main-session").is_some());
}

#[gpui::test]
fn worktree_new_chat_is_local_and_visible_before_backend_reply(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "sidebar-mode-worktrees");
    click(vcx, "worktree-toggle-0");
    workspace.update(vcx, |w, _| {
        w.remotes.default_host = Some("elsewhere".into())
    });
    click(vcx, "worktree-new-chat-0");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { working_dir: Some(dir), .. }) if dir == "/project")
    );
    assert!(
        commands.try_recv().is_err(),
        "new-chat click must not bubble into checkout navigation"
    );
    let id = workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert!(!w.worktrees.collapsed.contains("/project"));
        assert_eq!(w.worktrees.entries.len(), 2);
        w.slots[w.active].panel.read(cx).session_id.clone()
    });
    assert!(
        vcx.debug_bounds(format!("worktree-session-{id}").leak())
            .is_some()
    );
    assert!(
        vcx.debug_bounds(format!("worktree-session-{id}-selected").leak())
            .is_some()
    );
}

#[gpui::test]
fn worktree_navigation_keeps_groups_and_collapse_state(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "sidebar-mode-worktrees");
    click(vcx, "worktree-toggle-0");
    click(vcx, "worktree-empty-1");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { working_dir: Some(dir), .. }) if dir == "/project-trees/search")
    );
    assert!(vcx.debug_bounds("worktree-0").is_some());
    assert!(vcx.debug_bounds("worktree-1").is_some());
    assert!(vcx.debug_bounds("worktree-session-main-session").is_none());
    workspace.read_with(vcx, |w, _| {
        assert!(w.worktrees.collapsed.contains("/project"));
        assert!(!w.worktrees.loading);
        assert!(w.worktrees.error.is_none());
    });
    click(vcx, "worktree-toggle-0");
    click(vcx, "worktree-session-main-session");
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2, "existing chat is reused");
        assert_eq!(w.slots[w.active].panel.read(cx).session_id, "main-session");
    });
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn worktree_groups_hide_archived_remote_and_sibling_sessions_and_sort_by_recency(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, _) = setup(cx);
    workspace.update(vcx, |w, cx| {
        for (id, directory, archived, updated) in [
            ("older", "/project", false, 100),
            ("newer", "/project/src", false, 200),
            ("archived", "/project", true, 300),
            ("ssh://server/remote", "/project", false, 300),
            ("sibling", "/project-other", false, 300),
        ] {
            let mut session = crate::workspace::tests::session_info(id, Some(id));
            session.working_dir = Some(directory.into());
            session.archived = archived;
            session.updated_at_ms = Some(updated);
            w.sessions.push(session);
        }
        cx.notify();
    });
    click(vcx, "sidebar-mode-worktrees");
    assert!(
        vcx.debug_bounds("worktree-session-newer").unwrap().top()
            < vcx.debug_bounds("worktree-session-older").unwrap().top()
    );
    for id in ["archived", "ssh://server/remote", "sibling"] {
        assert!(
            vcx.debug_bounds(format!("worktree-session-{id}").leak())
                .is_none()
        );
    }
}

#[gpui::test]
fn worktree_unavailable_checkout_disables_chat_and_creation(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.worktrees.entries[1].prunable = Some("missing".into());
        let mut session = crate::workspace::tests::session_info("missing-chat", Some("History"));
        session.working_dir = Some("/project-trees/search".into());
        w.sessions.push(session);
        cx.notify();
    });
    click(vcx, "sidebar-mode-worktrees");
    assert!(vcx.debug_bounds("worktree-new-chat-1").is_none());
    click(vcx, "worktree-1");
    click(vcx, "worktree-session-missing-chat");
    assert_eq!(workspace.read_with(vcx, |w, _| w.slots.len()), 1);
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn worktree_toggle_preserves_sessions_and_persists_with_legacy_default(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx);
    assert!(!workspace.read_with(vcx, |w, _| w.worktree_mode));
    click(vcx, "sidebar-mode-worktrees");
    assert!(vcx.debug_bounds("sidebar-worktrees").is_some());
    assert!(vcx.debug_bounds("sidebar-session-list").is_none());
    assert!(vcx.debug_bounds("worktree-session-main-session").is_some());
    let snapshot =
        vcx.update(|window, cx| workspace.read_with(cx, |w, cx| w.snapshot(window, cx).unwrap()));
    assert!(snapshot.worktree_mode);
    assert!(
        WorkspaceSnapshot::decode(&snapshot.encode().unwrap())
            .unwrap()
            .worktree_mode
    );
    let mut legacy = serde_json::to_value(&snapshot).unwrap();
    legacy.as_object_mut().unwrap().remove("worktree_mode");
    assert!(
        !WorkspaceSnapshot::decode(&serde_json::to_vec(&legacy).unwrap())
            .unwrap()
            .worktree_mode
    );
    click(vcx, "sidebar-mode-swarm");
    assert!(vcx.debug_bounds("sidebar-session-list").is_some());
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 1);
        assert_eq!(
            w.slots[0].panel.read(cx).working_dir.as_deref(),
            Some("/project")
        );
    });
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn worktree_row_reuses_existing_session_and_creates_local_draft_in_other_checkout(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "sidebar-mode-worktrees");
    click(vcx, "worktree-0");
    assert_eq!(workspace.read_with(vcx, |w, _| w.slots.len()), 1);
    // Local navigation must never forward local paths to the configured SSH host.
    workspace.update(vcx, |w, _| {
        w.remotes.default_host = Some("elsewhere".into())
    });
    click(vcx, "worktree-1");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { working_dir: Some(dir), .. }) if dir == "/project-trees/search")
    );
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert_eq!(
            w.slots[w.active].panel.read(cx).working_dir.as_deref(),
            Some("/project-trees/search")
        );
    });
}

#[gpui::test]
fn worktree_creation_form_validates_and_cancels_without_git_or_composer_submission(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx);
    click(vcx, "sidebar-mode-worktrees");
    click(vcx, "worktree-new");
    assert!(vcx.debug_bounds("worktree-create-form").is_some());
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("worktree-error").is_some());
    workspace.read_with(vcx, |w, _| assert!(!w.worktrees.creating));
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("worktree-create-form").is_none());
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn worktree_remote_sessions_cannot_operate_on_matching_local_paths(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.slots[0].panel.update(cx, |panel, _| {
            panel.session_id = "ssh://server/session".into()
        });
        cx.notify();
    });
    click(vcx, "sidebar-mode-worktrees");
    assert!(vcx.debug_bounds("worktree-new").is_none());
    assert!(vcx.debug_bounds("worktree-0").is_none());
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn worktree_create_enter_runs_real_git_and_opens_isolated_session(cx: &mut gpui::TestAppContext) {
    let temporary = tempfile::tempdir().unwrap();
    let repository = temporary.path().join("project");
    std::fs::create_dir(&repository).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(&repository)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    git(&["init", "-b", "main"]);
    // Fixture identity applies only inside this disposable test repository.
    git(&["config", "user.name", "Test Fixture"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(repository.join("tracked"), "committed").unwrap();
    git(&["add", "tracked"]);
    git(&["commit", "-m", "fixture"]);
    std::fs::write(repository.join("tracked"), "dirty original").unwrap();
    let directory = repository.to_str().unwrap().to_owned();
    let (workspace, vcx, commands) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.slots[0]
            .panel
            .update(cx, |panel, _| panel.working_dir = Some(directory.clone()));
        w.worktrees = State::default();
        cx.notify();
    });
    click(vcx, "sidebar-mode-worktrees");
    assert!(vcx.debug_bounds("worktree-0").is_some());
    click(vcx, "worktree-new");
    vcx.simulate_keystrokes("f e a t u r e - a enter");
    vcx.run_until_parked();
    let created = match commands.try_recv().unwrap() {
        Command::CreateSession {
            working_dir: Some(directory),
            ..
        } => directory,
        _ => panic!("Expected local CreateSession"),
    };
    assert_ne!(created, directory);
    assert_eq!(
        std::fs::read_to_string(Path::new(&created).join("tracked")).unwrap(),
        "committed"
    );
    assert_eq!(
        std::fs::read_to_string(repository.join("tracked")).unwrap(),
        "dirty original"
    );
    assert_eq!(git(&["branch", "--show-current"]).trim(), "main");
    assert_eq!(jcode_sdk::worktrees::list(&directory).unwrap().len(), 2);
    workspace.read_with(vcx, |w, cx| {
        assert!(!w.worktrees.creating);
        assert!(w.worktrees.error.is_none(), "{:?}", w.worktrees.error);
        assert!(w.worktrees.input.is_none());
        assert_eq!(
            w.slots[w.active].panel.read(cx).working_dir.as_deref(),
            Some(created.as_str())
        );
    });
}

#[gpui::test]
fn worktree_remote_review_never_uses_local_git(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.slots[0].panel.update(cx, |panel, _| {
            panel.session_id = "review://ssh://server/session".into()
        });
        cx.notify();
    });
    click(vcx, "sidebar-mode-worktrees");
    assert!(vcx.debug_bounds("worktree-new").is_none());
    assert!(vcx.debug_bounds("worktree-0").is_none());
    assert!(commands.try_recv().is_err());
}
