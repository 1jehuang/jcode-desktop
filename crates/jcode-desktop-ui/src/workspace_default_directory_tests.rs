//! Default-directory picker regressions through the real GPUI view and keymap.
//!
//! All filesystem fixtures live in TempDirs. Workspace::for_test and a recording
//! bridge avoid starting a runtime, and config::persist_pinned_working_dir is a
//! no-op under cfg(test), so these tests never write the user's configuration.
//! Disk persistence itself is covered by config's temporary-file round-trip test.

use super::*;
use std::sync::mpsc::{Receiver, TryRecvError};

fn setup<'a>(
    cx: &'a mut gpui::TestAppContext,
    pinned: &Path,
) -> (
    Entity<Workspace>,
    &'a mut gpui::VisualTestContext,
    Receiver<Command>,
) {
    cx.update(crate::bind_workspace_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.pinned_working_dir = Some(pinned.to_string_lossy().into_owned());
        workspace.push_test_panel("existing-session", cx);
        workspace.push_test_panel("neighbor-session", cx);
        workspace
    });
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| workspace.focus_active(window, cx));
    });
    vcx.run_until_parked();
    assert_no_command(&commands);
    (workspace, vcx, commands)
}

fn assert_no_command(commands: &Receiver<Command>) {
    assert!(
        matches!(commands.try_recv(), Err(TryRecvError::Empty)),
        "changing a preference must not send a runtime command"
    );
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx.debug_bounds(selector).expect(selector);
    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

fn open_picker(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) {
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_default_directory_picker(window, cx);
        });
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("default-directory-panel").is_some());
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert!(workspace.folder_picker_sets_default);
    });
}

fn select_directory(
    workspace: &Entity<Workspace>,
    vcx: &mut gpui::VisualTestContext,
    directory: &Path,
) {
    workspace.update(vcx, |workspace, cx| {
        workspace.browse_to(directory.to_path_buf(), cx);
    });
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.folder_picker_dir.as_deref(), Some(directory));
        assert!(workspace.folder_picker_error.is_none());
    });
}

fn assert_created_in(commands: &Receiver<Command>, directory: &Path) {
    match commands
        .try_recv()
        .expect("one new session should be created")
    {
        Command::CreateSession {
            working_dir,
            request_id,
        } => {
            assert_eq!(working_dir.as_deref(), directory.to_str());
            assert!(
                request_id
                    .as_deref()
                    .is_some_and(Panel::is_pending_session_id)
            );
        }
        _ => panic!("expected a local CreateSession command"),
    }
    assert_no_command(commands);
}

#[gpui::test]
fn default_directory_panel_can_focus_move_resize_and_close_like_chat(
    cx: &mut gpui::TestAppContext,
) {
    let prior = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    vcx.simulate_keystrokes("u n f i n i s h e d");
    vcx.run_until_parked();
    let picker_id = workspace.read_with(vcx, |w, cx| {
        let slot = &w.slots[w.active];
        assert!(slot.panel.read(cx).is_default_directory());
        assert!(!slot.panel.read(cx).can_fork());
        slot.panel.entity_id()
    });
    vcx.simulate_keystrokes(&platform_chord("super-left"));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let w = workspace.read(cx);
        assert_ne!(w.slots[w.active].panel.entity_id(), picker_id);
        assert!(
            w.slots[w.active]
                .panel
                .read(cx)
                .input_focus_handle(cx)
                .is_focused(window)
        );
    });
    // The picker stays mounted and does not block another chat's input.
    assert!(vcx.debug_bounds("default-directory-panel").is_some());
    vcx.simulate_keystrokes("c h a t");
    vcx.simulate_keystrokes(&platform_chord("super-right"));
    vcx.run_until_parked();
    click(vcx, "default-directory-button");
    vcx.update(|window, cx| {
        let w = workspace.read(cx);
        assert_eq!(w.slots.len(), 3, "reopening must not duplicate the picker");
        assert_eq!(w.slots[w.active].panel.entity_id(), picker_id);
        assert_eq!(
            w.folder_search.as_ref().unwrap().read(cx).content,
            "unfinished"
        );
        assert!(
            w.slots[w.active]
                .panel
                .read(cx)
                .input_focus_handle(cx)
                .is_focused(window)
        );
    });
    vcx.update(|window, cx| {
        window.dispatch_action(Box::new(WidthPreset3), cx);
        window.dispatch_action(Box::new(MovePanelDown), cx);
    });
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.slots[w.active].panel.entity_id(), picker_id);
        assert_eq!(w.slots[w.active].row, 1);
        assert_eq!(w.slots[w.active].width_fraction, 0.75);
    });
    vcx.simulate_keystrokes(&platform_chord("super-q"));
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert!(w.default_directory_panel_index(cx).is_none());
        assert!(w.folder_search.is_none());
        assert_eq!(w.pinned_working_dir.as_deref(), prior.path().to_str());
    });
    assert!(vcx.debug_bounds("default-directory-panel").is_none());
    assert_no_command(&commands);
}

#[gpui::test]
fn default_directory_panel_snapshot_restores_search_and_focus_without_watching_settings(
    cx: &mut gpui::TestAppContext,
) {
    let prior = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    vcx.simulate_keystrokes("d r a f t");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            let snapshot = w.snapshot(window, cx).unwrap();
            let snapshot = WorkspaceSnapshot::decode(&snapshot.encode().unwrap()).unwrap();
            w.apply_snapshot(snapshot, cx);
            w.restore_focus(window, cx);
            cx.notify();
        });
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let w = workspace.read(cx);
        assert_eq!(w.slots.len(), 3);
        assert!(w.slots[w.active].panel.read(cx).is_default_directory());
        let search = w.folder_search.as_ref().unwrap();
        assert_eq!(search.read(cx).content, "draft");
        assert!(search.read(cx).focus_handle.is_focused(window));
        assert_eq!(
            w.slots[w.active].panel.read(cx).input.entity_id(),
            search.entity_id()
        );
    });
    for expected in ["existing-session", "neighbor-session"] {
        match commands.try_recv().unwrap() {
            Command::Watch { session_id } => assert_eq!(session_id, expected),
            _ => panic!("only chat sessions should be watched after reload"),
        }
    }
    assert_no_command(&commands);
    assert!(vcx.debug_bounds("default-directory-panel").is_some());
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("default-directory-panel").is_none());
    assert_no_command(&commands);
}

#[gpui::test]
fn default_directory_header_click_opens_focused_picker(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    // The control must remain usable in both supported workspace layouts.
    for layout in [
        crate::config::LayoutMode::FolderTabs,
        crate::config::LayoutMode::Normal,
    ] {
        workspace.update(vcx, |workspace, cx| {
            workspace.layout_mode = layout;
            cx.notify();
        });
        vcx.run_until_parked();
        click(vcx, "default-directory-button");
        assert!(vcx.debug_bounds("default-directory-panel").is_some());
        assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
        vcx.update(|window, cx| {
            let workspace = workspace.read(cx);
            assert!(workspace.folder_picker_sets_default);
            assert!(workspace.folder_picker_error.is_none());
            assert!(
                workspace
                    .folder_search
                    .as_ref()
                    .expect("default picker reuses the search input")
                    .read(cx)
                    .focus_handle
                    .is_focused(window)
            );
            assert_eq!(
                workspace.pinned_working_dir.as_deref(),
                prior.path().to_str()
            );
            assert_eq!(workspace.slots.len(), 3);
        });
        assert_no_command(&commands);
        click(vcx, "folder-picker-cancel");
    }
}

#[gpui::test]
fn choosing_default_directory_changes_preference_without_spawning_and_next_enter_uses_it(
    cx: &mut gpui::TestAppContext,
) {
    let prior = tempfile::tempdir().unwrap();
    let selected = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    let before = workspace.read_with(vcx, |workspace, _| {
        workspace
            .slots
            .iter()
            .map(|slot| slot.panel.entity_id())
            .collect::<Vec<_>>()
    });
    open_picker(&workspace, vcx);
    select_directory(&workspace, vcx, selected.path());
    click(vcx, "folder-picker-open");
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(
            workspace.pinned_working_dir.as_deref(),
            selected.path().to_str()
        );
        assert!(workspace.folder_picker_dir.is_none());
        assert!(workspace.folder_search.is_none());
        assert!(workspace.folder_picker_error.is_none());
        assert_eq!(
            workspace
                .slots
                .iter()
                .map(|slot| slot.panel.entity_id())
                .collect::<Vec<_>>(),
            before,
            "saving the default must neither replace existing panels nor add a draft"
        );
    });
    assert_no_command(&commands);

    // Do not repair focus in the test. Closing the search must leave the real
    // shortcut operational without waiting for a runtime reply.
    vcx.simulate_keystrokes(&platform_chord("super-enter"));
    vcx.run_until_parked();
    assert_created_in(&commands, selected.path());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.slots.len(), before.len() + 1);
    });
}

#[gpui::test]
fn default_directory_search_enter_sets_preference_without_spawning(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let selected = prior.path().join("selected");
    std::fs::create_dir(&selected).unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    select_directory(&workspace, vcx, prior.path());
    vcx.simulate_keystrokes("s e l e c t e d enter");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(workspace.pinned_working_dir.as_deref(), selected.to_str());
        assert_eq!(workspace.slots.len(), 2);
    });
    assert_no_command(&commands);
}

#[gpui::test]
fn invalid_default_directory_preserves_prior_and_displays_error(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let file = prior.path().join("not-a-directory.txt");
    std::fs::write(&file, "fixture").unwrap();
    let missing = prior.path().join("removed-after-browsing");
    std::fs::create_dir(&missing).unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    select_directory(&workspace, vcx, &missing);
    std::fs::remove_dir(&missing).unwrap();

    // Validate at confirmation time too: a browsed directory can disappear or
    // become a regular file while the picker is open.
    for invalid in [&missing, &file] {
        workspace.update(vcx, |workspace, cx| {
            workspace.folder_picker_dir = Some(invalid.clone());
            workspace.folder_picker_error = None;
            cx.notify();
        });
        vcx.run_until_parked();
        click(vcx, "folder-picker-open");
        workspace.read_with(vcx, |workspace, _| {
            assert_eq!(
                workspace.pinned_working_dir.as_deref(),
                prior.path().to_str()
            );
            assert_eq!(
                workspace.folder_picker_dir.as_deref(),
                Some(invalid.as_path())
            );
            assert!(workspace.folder_picker_sets_default);
            assert!(workspace.folder_search.is_some());
            assert!(
                workspace
                    .folder_picker_error
                    .as_ref()
                    .is_some_and(|error| !error.is_empty())
            );
            assert_eq!(workspace.slots.len(), 3);
        });
        assert!(vcx.debug_bounds("default-directory-panel").is_some());
        assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
        assert!(vcx.debug_bounds("folder-picker-error").is_some());
        assert_no_command(&commands);
    }

    // An error must not poison the picker or prevent a subsequent valid save.
    select_directory(&workspace, vcx, prior.path());
    assert!(vcx.debug_bounds("folder-picker-error").is_none());
    click(vcx, "folder-picker-open");
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    assert_no_command(&commands);
}

#[gpui::test]
fn cancelling_default_directory_preserves_prior_and_restores_shortcut(
    cx: &mut gpui::TestAppContext,
) {
    let prior = tempfile::tempdir().unwrap();
    let selected = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    select_directory(&workspace, vcx, selected.path());
    click(vcx, "folder-picker-cancel");
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(
            workspace.pinned_working_dir.as_deref(),
            prior.path().to_str()
        );
        assert!(workspace.folder_picker_dir.is_none());
        assert!(workspace.folder_search.is_none());
        assert!(workspace.folder_picker_error.is_none());
        assert_eq!(workspace.slots.len(), 2);
    });
    assert_no_command(&commands);
    vcx.simulate_keystrokes(&platform_chord("super-enter"));
    vcx.run_until_parked();
    assert_created_in(&commands, prior.path());
}

#[gpui::test]
fn ordinary_open_folder_resets_default_mode_and_still_spawns(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let selected = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    // Invoke the ordinary action with default mode still active. This catches
    // a sticky mode bit even if cancellation happens to reset it correctly.
    vcx.update(|window, cx| window.dispatch_action(Box::new(OpenFolder), cx));
    vcx.run_until_parked();
    workspace.read_with(vcx, |workspace, _| {
        assert!(!workspace.folder_picker_sets_default);
        assert!(workspace.folder_search.is_some());
    });
    select_directory(&workspace, vcx, selected.path());
    click(vcx, "folder-picker-open");
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(
            workspace.pinned_working_dir.as_deref(),
            prior.path().to_str()
        );
        assert_eq!(workspace.slots.len(), 3);
    });
    assert_created_in(&commands, selected.path());
}

#[gpui::test]
fn escape_cancels_default_directory_without_saving(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let selected = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    select_directory(&workspace, vcx, selected.path());
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(
            workspace.pinned_working_dir.as_deref(),
            prior.path().to_str()
        );
        assert_eq!(workspace.slots.len(), 2);
        assert!(!workspace.folder_picker_sets_default);
    });
    assert_no_command(&commands);
}

#[gpui::test]
fn typed_invalid_default_path_does_not_fall_back_to_parent_or_fuzzy_match(
    cx: &mut gpui::TestAppContext,
) {
    let prior = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    std::fs::create_dir(base.path().join("selected-project")).unwrap();
    std::fs::write(base.path().join("file.txt"), "fixture").unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    for keys in ["s e l e c t e d enter", "f i l e . t x t enter"] {
        open_picker(&workspace, vcx);
        select_directory(&workspace, vcx, base.path());
        vcx.simulate_keystrokes(keys);
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert_eq!(
                workspace.pinned_working_dir.as_deref(),
                prior.path().to_str()
            );
            assert!(workspace.folder_picker_error.is_some());
            assert_eq!(workspace.slots.len(), 3);
        });
        assert!(vcx.debug_bounds("folder-picker-error").is_some());
        assert_no_command(&commands);
        click(vcx, "folder-picker-cancel");
    }
}

#[gpui::test]
fn tilde_default_directory_paths_expand_to_home_without_spawning(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let home = default_working_dir().expect("test process has a home directory");
    let (workspace, vcx, commands) = setup(cx, prior.path());
    for query in ["~", "~/"] {
        open_picker(&workspace, vcx);
        workspace.update(vcx, |workspace, cx| {
            workspace.set_searched_default_directory(query, cx);
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert_eq!(
                workspace.pinned_working_dir.as_deref().map(Path::new),
                Some(Path::new(&home))
            );
            assert_eq!(workspace.slots.len(), 2);
        });
        assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
        assert_no_command(&commands);
    }
}

#[gpui::test]
fn invalid_typed_default_path_confirm_click_keeps_escape_working(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    vcx.simulate_keystrokes("m i s s i n g");
    vcx.run_until_parked();
    click(vcx, "folder-picker-open");
    assert!(vcx.debug_bounds("folder-picker-error").is_some());
    assert!(vcx.debug_bounds("default-directory-panel").is_some());
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(
            workspace.pinned_working_dir.as_deref(),
            prior.path().to_str()
        );
        assert_eq!(workspace.slots.len(), 3);
    });
    assert_no_command(&commands);

    // A full pointer click used to move focus from the search to the workspace
    // behind the modal, leaving its Escape callback unreachable after an error.
    // Do not restore focus or call close_folder_picker directly in this test.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    assert!(vcx.debug_bounds("folder-picker-error").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(
            workspace.pinned_working_dir.as_deref(),
            prior.path().to_str()
        );
        assert_eq!(workspace.slots.len(), 2);
        assert!(workspace.folder_search.is_none());
        assert!(!workspace.folder_picker_sets_default);
    });
    assert_no_command(&commands);
}

fn directory_session(id: &str, directory: &Path) -> jcode_sdk::SessionInfo {
    jcode_sdk::SessionInfo {
        session_id: id.into(),
        working_dir: Some(directory.to_string_lossy().into_owned()),
        title: None,
        status: "idle".into(),
        transcript_bytes: None,
        saved: false,
        updated_at_ms: None,
        last_active_at_ms: None,
        archived: false,
        archived_at_ms: None,
    }
}

#[test]
fn default_directories_rank_usage_and_recency_without_remote_or_invalid_paths() {
    let base = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let frequent = base.path().join("frequent");
    let recent = base.path().join("recent");
    let older = base.path().join("older");
    let unused = base.path().join("unused");
    let file = base.path().join("file");
    for directory in [&frequent, &recent, &older, &unused] {
        std::fs::create_dir(directory).unwrap();
    }
    std::fs::write(&file, "fixture").unwrap();
    let sessions = vec![
        directory_session("frequent-one", &frequent),
        directory_session("frequent-two", &frequent),
        directory_session("older", &older),
        directory_session("recent", &recent),
        directory_session("ssh://host/remote", external.path()),
        directory_session("file", &file),
        directory_session("missing", &base.path().join("missing")),
        directory_session("relative", Path::new("relative")),
    ];
    let entries = default_directory::ranked_directories(&sessions, base.path(), "");
    let paths = entries.iter().map(|(path, _)| path).collect::<Vec<_>>();
    assert_eq!(&paths[..3], &[&frequent, &recent, &older]);
    assert_eq!(entries[0].1, "2 sessions · Set as default");
    assert_eq!(entries[1].1, "1 session · Set as default");
    assert_eq!(
        entries.len(),
        5,
        "only valid local history, base, and child directories"
    );
    assert!(paths.contains(&&unused));
    assert!(paths.iter().any(|path| path.as_path() == base.path()));
    assert!(!paths.iter().any(|path| path.as_path() == external.path()));
    let filtered = default_directory::ranked_directories(&sessions, base.path(), "frequent");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].0, frequent);
}

#[gpui::test]
fn ranked_directory_click_sets_default_without_spawning(cx: &mut gpui::TestAppContext) {
    let prior = tempfile::tempdir().unwrap();
    let selected = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    workspace.update(vcx, |workspace, _| {
        workspace.sessions = vec![directory_session("recent", selected.path())];
    });
    open_picker(&workspace, vcx);
    click(vcx, "folder-picker-entry-0");
    assert!(vcx.debug_bounds("folder-picker-overlay").is_none());
    workspace.read_with(vcx, |workspace, _| {
        assert_eq!(
            workspace.pinned_working_dir.as_deref(),
            selected.path().to_str()
        );
        assert_eq!(workspace.slots.len(), 2);
    });
    assert_no_command(&commands);
}

#[gpui::test]
fn snapshot_round_trip_keeps_default_picker_mode_and_legacy_defaults_to_open_folder(
    cx: &mut gpui::TestAppContext,
) {
    let prior = tempfile::tempdir().unwrap();
    let (workspace, vcx, commands) = setup(cx, prior.path());
    open_picker(&workspace, vcx);
    vcx.simulate_keystrokes("p r o j");
    let snapshot = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
    assert!(snapshot.folder_picker_sets_default);
    assert_eq!(snapshot.folder_search.as_ref().unwrap().content, "proj");
    let encoded = snapshot.encode().unwrap();
    let decoded = WorkspaceSnapshot::decode(&encoded).unwrap();
    assert_eq!(decoded, snapshot);
    assert!(decoded.folder_picker_sets_default);
    let mut legacy: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("folder_picker_sets_default");
    let legacy = WorkspaceSnapshot::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(!legacy.folder_picker_sets_default);
    assert_eq!(legacy.folder_picker_dir.as_deref(), Some(prior.path()));
    assert_no_command(&commands);
}
