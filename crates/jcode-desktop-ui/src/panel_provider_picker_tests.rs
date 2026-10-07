use super::*;

#[gpui::test]
fn method_pill_selects_existing_provider_inline(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| crate::input::bind_keys(cx));
    let (bridge, commands) = crate::harness::spawn_recording();
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut workspace =
            crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
        workspace.set_test_bridge(bridge);
        workspace.push_test_panel("session-a", cx);
        workspace
    });
    let panel = workspace.update(vcx, |workspace, _| workspace.test_panel(0).unwrap());
    panel.update(vcx, |panel, cx| {
        let route = |model: &str, method: &str| jcode_sdk::ModelRouteInfo {
            model: model.into(),
            provider: String::new(),
            api_method: method.into(),
            available: true,
            detail: String::new(),
            usage: None,
        };
        panel.apply(
            &ApiEvent::RuntimeInfo {
                session_id: "session-a".into(),
                provider: Some("openai".into()),
                model: Some("gpt-5.6-sol".into()),
                reasoning_effort: None,
                auth_method: Some("oauth".into()),
                routes: vec![
                    route("gpt-5.6-sol", "openai-oauth"),
                    route("claude-fable-5", "claude-oauth"),
                ],
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let pill = vcx.debug_bounds("panel-login").unwrap();
    vcx.simulate_click(pill.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    let picker = vcx
        .debug_bounds("composer-provider-picker")
        .expect("inline picker");
    let input = vcx.debug_bounds("prompt-input").unwrap();
    assert!(picker.bottom() <= input.top());
    assert!(vcx.debug_bounds("accounts-panel").is_none());
    assert!(
        vcx.debug_bounds("login-close").is_none(),
        "no Accounts sign-in UI"
    );
    let row = vcx
        .debug_bounds("login-provider-claude")
        .expect("connected provider row is shown");
    vcx.simulate_click(row.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(matches!(
        commands.recv_timeout(std::time::Duration::from_millis(100)),
        Ok(Command::SetModel { session_id, model })
            if session_id == "session-a" && model == "claude-oauth:claude-fable-5"
    ));
    assert!(vcx.debug_bounds("composer-provider-picker").is_none());
    assert!(vcx.debug_bounds("login-busy").is_none());
}
