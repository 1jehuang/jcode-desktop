//! Exercise the real composer dispatcher, not the panel-local login helper.
use super::*;

fn setup<'a>(
    cx: &'a mut gpui::TestAppContext,
    session: &str,
) -> (
    Entity<Workspace>,
    &'a mut gpui::VisualTestContext,
    std::sync::mpsc::Receiver<Command>,
) {
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
    });
    let (bridge, commands) = harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.push_test_panel(session, cx);
        workspace
    });
    workspace.update_in(vcx, |workspace, window, cx| {
        workspace.focus_active(window, cx)
    });
    vcx.run_until_parked();
    (workspace, vcx, commands)
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx.debug_bounds(selector).expect(selector);
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

fn submit(vcx: &mut gpui::VisualTestContext, command: &str) {
    vcx.simulate_input(command);
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
}

#[gpui::test]
fn login_composer_opens_adjacent_panel_with_working_provider_back_and_close(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx, "login-source");
    let source = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
    submit(vcx, "/login");
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 2);
        assert_eq!(w.active, 1);
        assert_eq!(w.slots[0].panel, source);
        assert!(w.slots[1].panel.read(cx).is_accounts_panel());
        assert!(source.read(cx).input.read(cx).content.is_empty());
        assert!(source.read(cx).items.is_empty());
    });
    assert!(vcx.debug_bounds("accounts-panel").is_some());
    // The real provider catalog overflows a half-width panel. Click only
    // after scrolling the choice into its clipped viewport.
    for _ in 0..20 {
        let dialog = vcx.debug_bounds("login-dialog").unwrap();
        let provider = vcx.debug_bounds("login-provider-openai-api").unwrap();
        if provider.bottom() < dialog.bottom() - px(20.) {
            break;
        }
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: dialog.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-100.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
    }
    click(vcx, "login-provider-openai-api");
    assert!(vcx.debug_bounds("login-submit").is_some());
    click(vcx, "login-submit"); // Empty keys fail locally, never authenticate.
    assert!(vcx.debug_bounds("login-error").is_some());
    click(vcx, "login-back");
    assert!(vcx.debug_bounds("login-provider-openai-api").is_some());
    click(vcx, "login-close");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.slots[w.active].panel, source);
        assert_eq!(w.slots.iter().filter(|slot| !slot.closing).count(), 1);
    });
    vcx.update(|window, cx| assert!(source.read(cx).input_focus_handle(cx).is_focused(window)));
    assert!(
        matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "login-source")
    );
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn login_composer_provider_argument_reuses_accounts_and_escape_returns(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx, "login-source");
    submit(vcx, "/login");
    let accounts = workspace.read_with(vcx, |w, _| w.slots[w.active].panel.clone());
    workspace.update_in(vcx, |w, window, cx| {
        w.set_active(0, cx);
        w.focus_active(window, cx);
    });
    submit(vcx, "/login openai-api");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.slots.len(), 2);
        assert_eq!(w.slots[w.active].panel, accounts);
    });
    assert!(vcx.debug_bounds("login-paste").is_some());
    assert!(vcx.debug_bounds("login-submit").is_some());
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| assert_eq!(w.active, 0));
    assert!(
        matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "login-source")
    );
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn login_composer_remote_provider_keeps_native_recovery(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx, "ssh://test-host/session");
    submit(vcx, "/login openai-api");
    assert!(vcx.debug_bounds("accounts-panel").is_some());
    assert!(vcx.debug_bounds("login-error").is_some());
    assert!(vcx.debug_bounds("login-submit").is_none());
    assert!(vcx.debug_bounds("login-provider-openai").is_none());
    assert!(commands.try_recv().is_err());
    workspace.read_with(vcx, |w, cx| {
        assert!(w.slots[0].panel.read(cx).items.is_empty())
    });
}

fn accounts_child(vcx: &mut gpui::VisualTestContext) -> gpui::AnyWindowHandle {
    vcx.update(|_, cx| {
        cx.windows()
            .into_iter()
            .find(|handle| handle.downcast::<panel_window::PanelWindow>().is_some())
            .expect("Accounts opens a separate native window")
    })
}

#[gpui::test]
fn single_panel_footer_accounts_preserves_draft_conversation_and_bounds(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx, "single-login-source");
    let source = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
    workspace.update(vcx, |w, cx| {
        w.single_panel = true;
        source.update(cx, |panel, cx| {
            panel.items = vec![
                crate::panel::Item::User("Keep this conversation".into()),
                crate::panel::Item::Assistant("And its response".into()),
            ];
            panel.input.update(cx, |input, cx| {
                input.set_content("Unsent draft survives Accounts".into(), cx);
            });
        });
        cx.notify();
    });
    vcx.run_until_parked();
    let before = source.read_with(vcx, |panel, cx| {
        (panel.items.clone(), panel.input.read(cx).content.clone())
    });
    let layout = workspace.read_with(vcx, |w, _| {
        (w.active, w.active_row, w.slots[0].width_fraction)
    });
    let bounds = vcx.debug_bounds("single-panel-root").unwrap();
    let input_bounds = vcx.debug_bounds("prompt-input").unwrap();
    click(vcx, "panel-login");
    assert_eq!(vcx.debug_bounds("single-panel-root"), Some(bounds));
    assert_eq!(vcx.debug_bounds("prompt-input"), Some(input_bounds));
    assert!(vcx.debug_bounds("login-dialog").is_none());
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 1);
        assert_eq!(w.slots[0].panel, source);
        assert_eq!((w.active, w.active_row, w.slots[0].width_fraction), layout);
        let panel = source.read(cx);
        assert_eq!(
            (panel.items.clone(), panel.input.read(cx).content.clone()),
            before
        );
    });
    let child = accounts_child(vcx);
    let mut child_cx = gpui::VisualTestContext::from_window(child, vcx);
    child_cx.run_until_parked();
    assert!(child_cx.debug_bounds("accounts-panel").is_some());
    click(&mut child_cx, "login-close");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        assert_eq!(cx.windows().len(), 1);
        assert!(source.read(cx).input_focus_handle(cx).is_focused(window));
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "single-login-source")
    );
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn single_panel_login_provider_uses_child_and_escape_restores_composer(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx, "single-login-provider");
    workspace.update(vcx, |w, cx| {
        w.single_panel = true;
        cx.notify();
    });
    vcx.run_until_parked();
    submit(vcx, "/login openai-api");
    let source = workspace.read_with(vcx, |w, cx| {
        assert_eq!(w.slots.len(), 1);
        assert_eq!(w.active, 0);
        let source = w.slots[0].panel.clone();
        assert!(source.read(cx).items.is_empty());
        assert!(source.read(cx).input.read(cx).content.is_empty());
        source
    });
    let child = accounts_child(vcx);
    let mut child_cx = gpui::VisualTestContext::from_window(child, vcx);
    child_cx.run_until_parked();
    assert!(child_cx.debug_bounds("login-submit").is_some());
    assert!(child_cx.debug_bounds("login-paste").is_some());
    child_cx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        assert_eq!(cx.windows().len(), 1);
        assert!(source.read(cx).input_focus_handle(cx).is_focused(window));
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "single-login-provider")
    );
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn single_panel_accounts_choose_model_closes_accounts_and_opens_shared_picker(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx, commands) = setup(cx, "single-login-model");
    workspace.update(vcx, |w, cx| {
        w.single_panel = true;
        cx.notify();
    });
    vcx.run_until_parked();
    let source = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
    click(vcx, "panel-login");
    let child = accounts_child(vcx);
    child
        .downcast::<panel_window::PanelWindow>()
        .unwrap()
        .update(vcx, |root, _, cx| {
            root.panel
                .update(cx, |_, cx| cx.emit(crate::panel::AccountsPanelChooseModel));
        })
        .unwrap();
    vcx.run_until_parked();
    vcx.update(|_, cx| {
        assert!(!cx.windows().contains(&child));
        assert_eq!(cx.windows().len(), 1);
    });
    // No routes yet: the shared entry point offers account setup in place.
    assert!(vcx.debug_bounds("recovery-connect-account").is_some());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.slots.len(), 1);
        assert_eq!(w.slots[w.active].panel, source);
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "single-login-model")
    );
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn footer_and_account_model_actions_share_slash_menu_without_new_windows(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx, commands) = setup(cx, "unified-model-picker");
    let source = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
    source.update(vcx, |panel, cx| {
        panel.apply(&jcode_sdk::ApiEvent::RuntimeInfo {
            session_id: "unified-model-picker".into(),
            provider: Some("openai".into()),
            model: Some("test".into()),
            reasoning_effort: None,
            routes: vec![jcode_sdk::ModelRouteInfo {
                model: "test".into(), provider: "openai".into(),
                api_method: "openai-api-key".into(), available: true,
                detail: String::new(), usage: None,
            }],
        }, cx);
        panel.input.update(cx, |input, cx| input.set_content("keep this draft".into(), cx));
    });
    for single in [false, true] {
        workspace.update(vcx, |w, cx| {
            w.single_panel = single;
            source.update(cx, |_, cx| cx.notify());
            cx.notify();
        });
        vcx.run_until_parked();
        click(vcx, "panel-model");
        assert!(vcx.debug_bounds("slash-command-overlay").is_some());
        assert!(vcx.debug_bounds("recovery-model-picker").is_none());
        vcx.update(|_, cx| assert_eq!(cx.windows().len(), 1));
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        source.read_with(vcx, |panel, cx| assert_eq!(panel.input.read(cx).content.as_ref(), "keep this draft"));
        source.update(vcx, |panel, cx| panel.choose_account_model(cx));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("slash-command-overlay").is_some());
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(matches!(commands.try_recv(), Ok(Command::SetModel { model, .. }) if model == "openai-api:test"));
        source.read_with(vcx, |panel, cx| assert_eq!(panel.input.read(cx).content.as_ref(), "keep this draft"));
    }
    assert!(commands.try_recv().is_err());
}
