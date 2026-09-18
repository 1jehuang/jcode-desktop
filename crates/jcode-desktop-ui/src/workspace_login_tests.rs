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
