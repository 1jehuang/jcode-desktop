//! Behavioral coverage beyond the 42-binding registration audit.
//!
//! Every action enters through GPUI's public keystroke dispatcher, from both
//! root and composer focus. Recording bridges never start a runtime/provider.
//! Gmail/Todoist cover only reuse of an existing fixture panel: their real
//! constructors start service workers and are deliberately not called here.
//! Run with `cargo test -p jcode-desktop-ui --lib super_action_behavior_tests
//! -- --test-threads=1`, since theme selection is process-global.
#![cfg(not(target_os = "macos"))]

use super::*;
use std::sync::mpsc::Receiver;

fn setup(
    cx: &mut gpui::TestAppContext,
) -> (
    Entity<Workspace>,
    &mut gpui::VisualTestContext,
    Receiver<Command>,
) {
    cx.update(crate::bind_workspace_keys);
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        // Keep these tests independent of the user's selected layout and motion.
        workspace.layout_mode = crate::config::LayoutMode::Normal;
        workspace.hints_progress = AnimatedValue::new(0.0, Duration::ZERO);
        workspace.push_test_panel("source-session", cx);
        workspace.push_test_panel("neighbor-session", cx);
        workspace
    });
    vcx.run_until_parked();
    assert!(commands.try_recv().is_err());
    (workspace, vcx, commands)
}

fn focus(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext, composer: bool) {
    vcx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.set_active(0, cx);
            if composer {
                workspace.focus_active(window, cx);
            } else {
                window.focus(&workspace.focus_handle, cx);
            }
        });
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let workspace = workspace.read(cx);
        let handle = if composer {
            workspace.slots[0].panel.read(cx).input_focus_handle(cx)
        } else {
            workspace.focus_handle.clone()
        };
        assert!(handle.is_focused(window), "composer={composer}");
    });
}

/// This GPUI version names its public primitive `dispatch_keystroke` (singular).
/// Do not dispatch an Action directly: parsing, keymap matching, and bubbling
/// through the rendered focus tree are part of the behavior being verified.
fn dispatch_keystrokes(vcx: &mut gpui::VisualTestContext, keys: &str) {
    for key in keys.split_whitespace() {
        vcx.update(|window, cx| {
            window.dispatch_keystroke(gpui::Keystroke::parse(key).unwrap(), cx);
        });
        vcx.run_until_parked();
    }
}

fn assert_painted(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx
        .debug_bounds(selector)
        .expect("surface should be painted");
    assert!(
        bounds.size.width > px(0.) && bounds.size.height > px(0.),
        "{selector}"
    );
}

#[gpui::test]
fn super_slash_toggles_visible_hints_from_root_and_composer(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    for composer in [false, true] {
        focus(&workspace, vcx, composer);
        assert!(vcx.debug_bounds("coach-card").is_none());
        dispatch_keystrokes(vcx, "super-/");
        workspace.read_with(vcx, |w, _| assert!(w.hints_overlay));
        assert_painted(vcx, "coach-card");
        dispatch_keystrokes(vcx, "super-/");
        workspace.read_with(vcx, |w, _| assert!(!w.hints_overlay));
        assert!(vcx.debug_bounds("coach-card").is_none());
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn super_shift_s_toggles_showcase_cue_from_root_and_composer(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    for composer in [false, true] {
        focus(&workspace, vcx, composer);
        dispatch_keystrokes(vcx, "super-shift-s");
        workspace.read_with(vcx, |w, _| {
            assert!(!w.showcase_mode);
            assert!(w.showcase_cue.is_none());
        });
        assert!(vcx.debug_bounds("showcase-shortcut").is_none());
        dispatch_keystrokes(vcx, "super-shift-s");
        workspace.read_with(vcx, |w, _| {
            assert!(w.showcase_mode);
            let cue = w.showcase_cue.as_ref().expect("enabled feedback");
            assert_eq!(cue.shortcut, "Super + Shift + S");
            assert_eq!(cue.action, "Showcase mode on");
        });
        assert_painted(vcx, "showcase-shortcut");
        assert_painted(vcx, "showcase-action");
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn super_shift_t_cycles_all_theme_presets_from_root_and_composer(cx: &mut gpui::TestAppContext) {
    // Theme selection is process-global. Restore it even if an assertion fails.
    // config::persist_theme is a no-op in unit tests, so no user config is written.
    struct RestoreTheme(ThemePreset);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            Theme::select(self.0);
        }
    }
    let _restore = RestoreTheme(Theme::active_preset());
    let (workspace, vcx, commands) = setup(cx);
    for composer in [false, true] {
        focus(&workspace, vcx, composer);
        let initial = Theme::active_preset();
        let mut expected = initial;
        for _ in ThemePreset::ALL {
            expected = expected.next();
            dispatch_keystrokes(vcx, "super-shift-t");
            assert_eq!(Theme::active_preset(), expected, "composer={composer}");
            assert_painted(vcx, "workspace-canvas");
        }
        assert_eq!(Theme::active_preset(), initial, "theme cycle wraps");
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn super_b_toggles_sidebar_without_mutating_panels(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    for composer in [false, true] {
        focus(&workspace, vcx, composer);
        let ids = workspace.read_with(vcx, |w, _| {
            w.slots
                .iter()
                .map(|s| s.panel.entity_id())
                .collect::<Vec<_>>()
        });
        assert_painted(vcx, "sidebar");
        let before = vcx.debug_bounds("workspace-canvas").unwrap();
        dispatch_keystrokes(vcx, "super-b");
        workspace.read_with(vcx, |w, _| assert!(!w.show_sidebar));
        assert!(vcx.debug_bounds("sidebar").is_none());
        assert!(vcx.debug_bounds("workspace-canvas").unwrap().size.width > before.size.width);
        dispatch_keystrokes(vcx, "super-b");
        assert_painted(vcx, "sidebar");
        workspace.read_with(vcx, |w, _| {
            assert!(w.show_sidebar);
            assert_eq!(w.active, 0);
            assert_eq!(
                w.slots
                    .iter()
                    .map(|s| s.panel.entity_id())
                    .collect::<Vec<_>>(),
                ids
            );
        });
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn super_space_emits_one_fork_for_the_focused_session(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    for composer in [false, true] {
        focus(&workspace, vcx, composer);
        dispatch_keystrokes(vcx, "super-space");
        assert!(
            matches!(commands.try_recv(), Ok(Command::Fork { session_id }) if session_id == "source-session")
        );
        assert!(commands.try_recv().is_err(), "one command per keypress");
        workspace.read_with(vcx, |w, _| assert_eq!(w.slots.len(), 2));
    }
    // Pending drafts must not send a placeholder ID to a real fork operation.
    workspace.update(vcx, |w, cx| {
        w.slots[0].panel.update(cx, |p, _| {
            p.session_id = "startup://draft/not-created".into()
        })
    });
    for composer in [false, true] {
        focus(&workspace, vcx, composer);
        dispatch_keystrokes(vcx, "super-space");
        assert!(commands.try_recv().is_err(), "pending sessions cannot fork");
    }
}

#[gpui::test]
fn super_t_inserts_and_focuses_an_inert_terminal(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    for (iteration, composer) in [false, true].into_iter().enumerate() {
        focus(&workspace, vcx, composer);
        dispatch_keystrokes(vcx, "super-t");
        vcx.update(|window, cx| {
            let w = workspace.read(cx);
            assert_eq!(w.slots.len(), 3 + iteration);
            assert_eq!(w.active, 1, "terminal is inserted immediately right");
            let panel = w.slots[1].panel.read(cx);
            assert_eq!(panel.session_id, "terminal");
            assert!(panel.test_terminal_contents(cx).is_some());
            assert_eq!(
                panel.snapshot(cx).terminal_resource_id,
                None,
                "inert host never creates a PTY"
            );
            assert!(panel.input_focus_handle(cx).is_focused(window));
        });
        assert_painted(vcx, "plain-terminal");
        dispatch_keystrokes(vcx, "super-space");
        assert!(commands.try_recv().is_err(), "terminals cannot fork");
    }
}

fn check_existing_service_panel(cx: &mut gpui::TestAppContext, chord: &str, session_id: &str) {
    let (workspace, vcx, commands) = setup(cx);
    // Deliberately use a bare panel, not new_gmail/new_todoist. This exercises
    // the existing-panel action branch without creating a service client.
    workspace.update(vcx, |w, cx| w.push_test_panel(session_id, cx));
    let service = workspace.read_with(vcx, |w, _| w.slots[2].panel.clone());
    for composer in [false, true] {
        focus(&workspace, vcx, composer);
        for _ in 0..2 {
            dispatch_keystrokes(vcx, chord);
            vcx.update(|window, cx| {
                let w = workspace.read(cx);
                assert_eq!(w.slots.len(), 3, "reuse never duplicates a service panel");
                assert_eq!(w.active, 2, "{chord}, composer={composer}");
                assert_eq!(w.slots[2].panel.entity_id(), service.entity_id());
                assert!(service.read(cx).input_focus_handle(cx).is_focused(window));
                assert_eq!(w.navigation_state(window, cx)["keyboard_panel"], 2);
            });
        }
    }
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn super_shift_g_focuses_existing_gmail_without_service_requests(cx: &mut gpui::TestAppContext) {
    check_existing_service_panel(cx, "super-shift-g", "gmail://inbox");
}

#[gpui::test]
fn super_shift_d_focuses_existing_todoist_without_service_requests(cx: &mut gpui::TestAppContext) {
    check_existing_service_panel(cx, "super-shift-d", "todoist://tasks");
}

#[gpui::test]
fn super_shift_slash_creates_correlated_help_and_records_its_prompt(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx);
    for (iteration, composer) in [false, true].into_iter().enumerate() {
        focus(&workspace, vcx, composer);
        dispatch_keystrokes(vcx, "super-/");
        assert_painted(vcx, "coach-card");
        dispatch_keystrokes(vcx, "super-shift-/");
        let request_id = match commands.try_recv().expect("help creation request") {
            Command::CreateSession {
                request_id: Some(id),
                working_dir,
            } => {
                assert_eq!(working_dir, default_working_dir());
                assert!(id.starts_with("startup://draft/help/"));
                id
            }
            _ => panic!("help must emit one local correlated creation"),
        };
        assert!(
            commands.try_recv().is_err(),
            "no prompt sent before creation reply"
        );
        assert!(vcx.debug_bounds("coach-card").is_none());
        vcx.update(|window, cx| {
            let w = workspace.read(cx);
            assert!(!w.hints_overlay);
            assert_eq!(w.slots.len(), 3 + iteration);
            assert_eq!(w.active, 1);
            let panel = w.slots[1].panel.read(cx);
            assert_eq!(panel.session_id, request_id);
            assert!(panel.input_focus_handle(cx).is_focused(window));
        });
        let session_id = format!("created-help-{iteration}");
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::SessionCreated {
                    session: jcode_sdk::SessionInfo {
                        session_id: session_id.clone(),
                        title: Some("Help".into()),
                        working_dir: default_working_dir(),
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
                    },
                    request_id: Some(request_id),
                },
                cx,
            )
        });
        assert!(
            matches!(commands.try_recv(), Ok(Command::Send { session_id: id, content, images })
            if id == session_id && content == HELP_SESSION_PROMPT && images.is_empty())
        );
        assert!(
            commands.try_recv().is_err(),
            "help prompt sent exactly once, to recording bridge only"
        );
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots[1].panel.read(cx).session_id, session_id)
        });
    }
}
