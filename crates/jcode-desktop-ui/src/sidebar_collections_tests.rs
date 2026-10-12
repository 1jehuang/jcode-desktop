use super::*;

fn store_in(dir: &tempfile::TempDir) -> State {
    State::with_path(dir.path().join("collections.json"))
}

fn panel_in(workspace: &mut Workspace, id: &str, dir: &str, cx: &mut Context<Workspace>) {
    workspace.push_test_panel(id, cx);
    let panel = workspace.slots.last().unwrap().panel.clone();
    panel.update(cx, |panel, _| panel.working_dir = Some(dir.into()));
}

fn rows(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) -> Vec<(String, usize)> {
    workspace.read_with(vcx, |w, _| {
        w.sidebar_session_layout
            .iter()
            .map(|item| (item.session_id.clone(), item.project))
            .collect()
    })
}

#[test]
fn keys_never_collide_with_project_paths() {
    let key = key("collection_1");
    assert_eq!(id_from_key(&key), Some("collection_1"));
    assert_eq!(id_from_key("/home/me/collection:x"), None);
    assert_eq!(id_from_key(""), None);
    assert_eq!(expand_home("  /tmp  "), "/tmp");
    assert_eq!(expand_home("~notauser"), "~notauser");
}

#[test]
fn changes_replay_identically_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collections.json");
    let mut memory = jcode_sdk::collections::Collections::default();
    let id = memory.new_id();
    let changes = [
        Change::Create {
            id: id.clone(),
            name: "Client X".into(),
            default_dir: None,
            instructions: Some("Be brief.".into()),
        },
        Change::Assign {
            session_id: "s1".into(),
            collection: Some(id.clone()),
        },
    ];
    for change in &changes {
        change.apply(&mut memory).unwrap();
        jcode_sdk::collections::update_at(&path, |store| change.apply(store)).unwrap();
    }
    let disk = jcode_sdk::collections::load_from(&path).unwrap();
    assert_eq!(disk.collections, memory.collections);
    assert_eq!(disk.members, memory.members);
}

#[gpui::test]
fn collections_lead_the_sidebar_and_take_members_out_of_projects(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.collections = store_in(&dir);
        panel_in(&mut workspace, "session_fox_alpha", "/nowhere/alpha", cx);
        panel_in(&mut workspace, "session_owl_beta", "/nowhere/beta", cx);
        panel_in(&mut workspace, "session_hare_alpha", "/nowhere/alpha", cx);
        workspace.active = 0;
        workspace
    });
    vcx.run_until_parked();
    assert_eq!(
        rows(&workspace, vcx),
        vec![
            ("session_fox_alpha".into(), 0),
            ("session_hare_alpha".into(), 0),
            ("session_owl_beta".into(), 1),
        ]
    );

    // A collection spanning two directories gathers its chats on top.
    workspace.update(vcx, |w, cx| {
        let id = w.collections.store.new_id();
        assert!(w.change_collections(
            Change::Create {
                id: id.clone(),
                name: "Q4 research".into(),
                default_dir: None,
                instructions: None,
            },
            cx,
        ));
        w.assign_session_collection("session_owl_beta", Some(id.clone()), cx);
        w.assign_session_collection("session_hare_alpha", Some(id), cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        rows(&workspace, vcx),
        vec![
            ("session_owl_beta".into(), 0),
            ("session_hare_alpha".into(), 0),
            ("session_fox_alpha".into(), 1),
        ]
    );
    let label = vcx.debug_bounds("sidebar-project-label-0");
    assert!(label.is_some(), "collection header renders");
    assert!(vcx.debug_bounds("sidebar-collection-edit-0").is_some());

    // Writes reach the shared store in order.
    let writes = workspace.update(vcx, |w, _| w.collection_writes());
    for write in writes {
        vcx.background_executor.run_until_parked();
        let _ = write;
    }
    vcx.run_until_parked();
    let disk = jcode_sdk::collections::load_from(&dir.path().join("collections.json")).unwrap();
    assert_eq!(disk.collections.len(), 1);
    assert_eq!(disk.members.len(), 2);

    // Removing a chat returns it to its directory group.
    workspace.update(vcx, |w, cx| {
        w.assign_session_collection("session_hare_alpha", None, cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        rows(&workspace, vcx),
        vec![
            ("session_owl_beta".into(), 0),
            ("session_fox_alpha".into(), 1),
            ("session_hare_alpha".into(), 1),
        ]
    );

    // Deleting the collection keeps every chat.
    workspace.update(vcx, |w, cx| {
        let id = w.collections.store.collections[0].id.clone();
        w.delete_collection(id, cx);
    });
    vcx.run_until_parked();
    let mut ids = rows(&workspace, vcx)
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "session_fox_alpha",
            "session_hare_alpha",
            "session_owl_beta"
        ]
    );
}

#[gpui::test]
fn row_menu_moves_a_chat_and_the_form_creates_and_validates(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.collections = store_in(&dir);
        panel_in(&mut workspace, "session_fox_one", "/nowhere/one", cx);
        panel_in(&mut workspace, "session_owl_two", "/nowhere/two", cx);
        workspace.active = 0;
        workspace
    });
    vcx.run_until_parked();

    // The quick action opens an empty form. Blank names are refused.
    let create = vcx.debug_bounds("sidebar-new-collection").unwrap();
    vcx.simulate_click(create.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("collection-form").is_some());
    let save = vcx.debug_bounds("collection-form-save").unwrap();
    vcx.simulate_click(save.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("collection-form-error").is_some());

    // A folder that does not exist is refused before anything is saved.
    workspace.update(vcx, |w, cx| {
        let form = w.collections.form.as_ref().unwrap();
        let (name, directory) = (form.name.clone(), form.directory.clone());
        name.update(cx, |input, cx| input.set_content("Client X".into(), cx));
        directory.update(cx, |input, cx| {
            input.set_content("/definitely/not/a/folder".into(), cx)
        });
    });
    let save = vcx.debug_bounds("collection-form-save").unwrap();
    vcx.simulate_click(save.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(w.collections.store.collections.is_empty());
        assert!(
            w.collections
                .error
                .as_deref()
                .unwrap()
                .contains("not a folder")
        );
    });

    let folder = dir.path().to_string_lossy().into_owned();
    workspace.update(vcx, |w, cx| {
        let form = w.collections.form.as_ref().unwrap();
        let (directory, instructions) = (form.directory.clone(), form.instructions.clone());
        directory.update(cx, |input, cx| input.set_content(folder.clone(), cx));
        instructions.update(cx, |input, cx| {
            input.set_content("Cite sources.".into(), cx)
        });
    });
    let save = vcx.debug_bounds("collection-form-save").unwrap();
    vcx.simulate_click(save.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("collection-form").is_none());
    workspace.read_with(vcx, |w, _| {
        let collection = &w.collections.store.collections[0];
        assert_eq!(collection.name, "Client X");
        assert_eq!(collection.default_dir.as_deref(), Some(folder.as_str()));
        assert_eq!(collection.instructions.as_deref(), Some("Cite sources."));
    });
    // An empty collection is still listed and offers editing.
    assert!(vcx.debug_bounds("sidebar-empty-collection-0").is_some());

    // Right-clicking a chat opens "Move to collection"; choosing moves it.
    let second = vcx.debug_bounds("sidebar-session-1").unwrap();
    vcx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Right,
        position: second.center(),
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Right,
        position: second.center(),
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
    });
    vcx.run_until_parked();
    let choice = vcx.debug_bounds("sidebar-collection-choice-0").unwrap();
    vcx.simulate_click(choice.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("sidebar-collection-menu").is_none());
    assert!(vcx.debug_bounds("sidebar-empty-collection-0").is_none());
    assert_eq!(rows(&workspace, vcx)[0].0, "session_owl_two");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(
            w.collections
                .store
                .instructions_for("session_owl_two")
                .unwrap()
                .rsplit("\n\n")
                .next(),
            Some("Cite sources.")
        );
        assert_eq!(w.slots.len(), 2, "moving never opens or closes panels");
    });

    // Editing a duplicate name is refused and keeps the original.
    workspace.update(vcx, |w, cx| {
        let id = w.collections.store.new_id();
        w.change_collections(
            Change::Create {
                id,
                name: "Other".into(),
                default_dir: None,
                instructions: None,
            },
            cx,
        );
        assert!(!w.change_collections(
            Change::Edit {
                id: w.collections.store.collections[1].id.clone(),
                name: "client x".into(),
                default_dir: None,
                instructions: None,
            },
            cx,
        ));
        assert_eq!(w.collections.store.collections[1].name, "Other");
    });
}

#[gpui::test]
fn new_chat_in_a_collection_joins_it_when_created(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.collections = store_in(&dir);
        workspace
    });
    vcx.run_until_parked();
    let draft = workspace.update(vcx, |w, cx| {
        let id = w.collections.store.new_id();
        w.change_collections(
            Change::Create {
                id: id.clone(),
                name: "Notes".into(),
                default_dir: Some(folder.clone()),
                instructions: None,
            },
            cx,
        );
        w.new_chat_in_collection(id, cx);
        let panel = w.slots[w.active].panel.read(cx);
        assert_eq!(panel.working_dir.as_deref(), Some(folder.as_str()));
        panel.session_id.clone()
    });
    assert!(Panel::is_pending_session_id(&draft));
    workspace.update(vcx, |w, cx| {
        let mut session = super::super::tests::session_info("session_fox_new", None);
        session.working_dir = Some(folder.clone());
        w.apply(
            Update::SessionCreated {
                session,
                request_id: Some(draft.clone()),
            },
            cx,
        );
        assert_eq!(
            w.collections
                .collection_of("session_fox_new")
                .map(|c| c.name.as_str()),
            Some("Notes")
        );
        assert!(w.pending_collection_drafts.is_empty());
    });
}
