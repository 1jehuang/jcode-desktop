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
