use super::*;

/// A main checkout at `root/project` on `main` and a linked worktree at
/// `root/project-search` on `feature/search`, built from real `.git` markers.
struct Fixture {
    _temp: tempfile::TempDir,
    main: String,
    search: String,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let main = root.join("project");
    let gitdir = main.join(".git/worktrees/search");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let search = root.join("project-search");
    std::fs::create_dir_all(search.join("crates/ui")).unwrap();
    std::fs::write(
        search.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/feature/search\n").unwrap();
    Fixture {
        _temp: temp,
        main: main.to_string_lossy().into_owned(),
        search: search.to_string_lossy().into_owned(),
    }
}

fn setup(
    cx: &mut gpui::TestAppContext,
    sessions: Vec<jcode_sdk::SessionInfo>,
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
        w.sessions = sessions;
        w
    });
    vcx.run_until_parked();
    (workspace, vcx, commands)
}

fn session(id: &str, dir: &str) -> jcode_sdk::SessionInfo {
    let mut info = crate::workspace::tests::session_info(id, Some(id));
    info.working_dir = Some(dir.into());
    info
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &str) {
    let bounds = vcx
        .debug_bounds(selector.to_owned().leak())
        .unwrap_or_else(|| panic!("missing {selector}"));
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

fn text(vcx: &mut gpui::VisualTestContext, selector: &str) -> bool {
    vcx.debug_bounds(selector.to_owned().leak()).is_some()
}

#[test]
fn checkout_probe_reads_branch_worktree_and_detached_head_without_git() {
    let f = fixture();
    let main = sidebar_projects::probe_checkout(Path::new(&f.main).join("src").as_path()).unwrap();
    assert_eq!(main.path, f.main);
    assert_eq!(main.branch, "main");
    assert!(!main.linked);
    let search =
        sidebar_projects::probe_checkout(Path::new(&f.search).join("crates/ui").as_path()).unwrap();
    assert_eq!(search.path, f.search);
    assert_eq!(search.branch, "feature/search");
    assert!(search.linked);
    std::fs::write(Path::new(&f.main).join(".git/HEAD"), "0123456789abcdef\n").unwrap();
    assert_eq!(
        sidebar_projects::probe_checkout(Path::new(&f.main))
            .unwrap()
            .branch,
        "detached 01234567"
    );
    assert!(sidebar_projects::probe_checkout(Path::new("/definitely/not/a/repo")).is_none());
}

#[gpui::test]
fn one_sidebar_without_modes_groups_threads_by_branch_under_their_project(
    cx: &mut gpui::TestAppContext,
) {
    let f = fixture();
    let mut newer = session("search-chat", &format!("{}/crates/ui", f.search));
    newer.updated_at_ms = Some(300);
    let mut main_chat = session("main-chat", &f.main);
    main_chat.updated_at_ms = Some(100);
    let (workspace, vcx, commands) = setup(cx, vec![newer, main_chat]);
    assert!(!text(vcx, "sidebar-mode-swarm"));
    assert!(!text(vcx, "sidebar-mode-worktrees"));
    assert!(text(vcx, "sidebar-project-0"));
    assert!(
        !text(vcx, "sidebar-project-1"),
        "a worktree belongs to its repository's project"
    );
    // The main checkout leads even though the worktree thread is more recent.
    let ids = workspace.read_with(vcx, |w, _| {
        w.sidebar_session_layout
            .iter()
            .map(|row| (row.session_id.clone(), row.checkout_header))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        ids,
        vec![("main-chat".into(), true), ("search-chat".into(), true)]
    );
    let main_row = vcx.debug_bounds("sidebar-checkout-0").unwrap();
    let search_row = vcx.debug_bounds("sidebar-checkout-1").unwrap();
    assert!(main_row.bottom() <= vcx.debug_bounds("sidebar-session-0").unwrap().top());
    assert!(vcx.debug_bounds("sidebar-session-0").unwrap().bottom() <= search_row.top());
    assert!(
        vcx.debug_bounds("sidebar-session-1").unwrap().left()
            > vcx.debug_bounds("sidebar-project-0").unwrap().left(),
        "threads indent beneath their branch"
    );
    assert!(
        !text(vcx, "sidebar-project-branch-0"),
        "branch rows replace the header pill"
    );
    assert!(
        commands.try_recv().is_err(),
        "rendering never creates sessions"
    );
}

#[gpui::test]
fn single_checkout_projects_show_their_branch_on_the_header(cx: &mut gpui::TestAppContext) {
    let f = fixture();
    let (_, vcx, _) = setup(cx, vec![session("main-chat", &format!("{}/src", f.main))]);
    assert!(text(vcx, "sidebar-project-branch-0"));
    assert!(!text(vcx, "sidebar-checkout-0"));
}

#[gpui::test]
fn project_and_branch_plus_open_local_threads_in_that_checkout(cx: &mut gpui::TestAppContext) {
    let f = fixture();
    let (workspace, vcx, commands) = setup(
        cx,
        vec![
            session("main-chat", &f.main),
            session("search-chat", &f.search),
        ],
    );
    // Local paths must never be forwarded to a configured SSH default.
    workspace.update(vcx, |w, _| {
        w.remotes.default_host = Some("elsewhere".into())
    });
    click(vcx, "sidebar-project-new-0");
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { working_dir: Some(dir), .. }) if dir == f.main)
    );
    assert!(
        commands.try_recv().is_err(),
        "the click must not also toggle or navigate"
    );
    let search_header = workspace.read_with(vcx, |w, _| {
        w.sidebar_session_layout
            .iter()
            .position(|row| row.session_id == "search-chat")
            .unwrap()
    });
    click(vcx, &format!("sidebar-checkout-new-{search_header}"));
    assert!(
        matches!(commands.try_recv(), Ok(Command::CreateSession { working_dir: Some(dir), .. }) if dir == f.search)
    );
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert_eq!(
            w.slots[w.active].panel.read(cx).working_dir.as_deref(),
            Some(f.search.as_str())
        );
    });
}

#[gpui::test]
fn quick_actions_open_a_thread_and_the_project_picker(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx, Vec::new());
    click(vcx, "sidebar-open-project");
    workspace.read_with(vcx, |w, _| assert!(w.folder_search.is_some()));
    assert!(
        commands.try_recv().is_err(),
        "choosing a folder comes before any session"
    );
    workspace.update(vcx, |w, cx| w.close_folder_picker(cx));
    click(vcx, "sidebar-new-thread");
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::CreateSession { .. })
    ));
}

#[gpui::test]
fn swarm_agents_nest_in_their_checkout_and_worktree_agents_get_their_branch(
    cx: &mut gpui::TestAppContext,
) {
    let f = fixture();
    let lead = session("lead", &f.main);
    let mut helper = session("helper", &f.main);
    helper.parent_session_id = Some("lead".into());
    helper.swarm_status = Some("working".into());
    let mut isolated = session("isolated", &f.search);
    isolated.parent_session_id = Some("lead".into());
    let (workspace, vcx, _) = setup(cx, vec![lead, helper, isolated]);
    let ids = workspace.read_with(vcx, |w, _| {
        w.sidebar_session_layout
            .iter()
            .map(|row| row.session_id.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(ids, vec!["lead".to_string(), "isolated".to_string()]);
    assert!(
        text(vcx, "sidebar-agents-0"),
        "the coordinator counts its same-checkout agent"
    );
    assert!(!text(vcx, "sidebar-agents-1"));
    assert!(
        text(vcx, "sidebar-checkout-1"),
        "the worktree agent sits under its own branch"
    );
}

#[gpui::test]
fn remote_projects_offer_no_local_thread_or_worktree_actions(cx: &mut gpui::TestAppContext) {
    let f = fixture();
    let (_, vcx, _) = setup(cx, vec![session("ssh://server/session", &f.main)]);
    assert!(text(vcx, "sidebar-project-0"));
    assert!(!text(vcx, "sidebar-project-new-0"));
    assert!(!text(vcx, "sidebar-project-worktree-0"));
    assert!(!text(vcx, "sidebar-project-branch-0"));
}

#[gpui::test]
fn worktree_form_validates_and_cancels_without_git_or_composer_submission(
    cx: &mut gpui::TestAppContext,
) {
    let f = fixture();
    let (workspace, vcx, commands) = setup(cx, vec![session("main-chat", &f.main)]);
    click(vcx, "sidebar-project-worktree-0");
    assert!(text(vcx, "worktree-create-form"));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(text(vcx, "worktree-error"));
    workspace.read_with(vcx, |w, _| assert!(!w.worktrees.creating));
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!text(vcx, "worktree-create-form"));
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn worktree_create_runs_real_git_and_the_new_branch_joins_its_project(
    cx: &mut gpui::TestAppContext,
) {
    let temporary = tempfile::tempdir().unwrap();
    let repository = std::fs::canonicalize(temporary.path())
        .unwrap()
        .join("project");
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
    let (workspace, vcx, commands) = setup(cx, vec![session("main-chat", &directory)]);
    click(vcx, "sidebar-project-worktree-0");
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
    workspace.read_with(vcx, |w, cx| {
        assert!(!w.worktrees.creating);
        assert!(w.worktrees.error.is_none(), "{:?}", w.worktrees.error);
        assert!(w.worktrees.input.is_none());
        assert_eq!(
            w.slots[w.active].panel.read(cx).working_dir.as_deref(),
            Some(created.as_str())
        );
    });
    // The new thread appears under the same project on its own branch row.
    workspace.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(!text(vcx, "sidebar-project-1"));
    let branches = workspace.read_with(vcx, |w, _| {
        w.sidebar_session_layout
            .iter()
            .filter(|row| row.checkout_header)
            .count()
    });
    assert_eq!(branches, 2);
}
