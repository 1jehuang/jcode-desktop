use super::*;

#[gpui::test]
fn preview_login_never_allocates_oauth_or_submits_credentials(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::LoginDialogError, cx));
    let (bridge, commands) = crate::harness::spawn_recording();
    panel.update(vcx, |panel, cx| {
        panel.bridge = bridge;
        for method in [LoginMethod::OAuth, LoginMethod::ApiKey] {
            let provider = panel
                .login
                .as_ref()
                .unwrap()
                .providers
                .iter()
                .find(|p| p.method == method)
                .copied()
                .unwrap();
            panel.select_login_provider(provider, cx);
            panel.submit_login(cx);
            let state = panel.login.as_ref().unwrap();
            assert!(state.flow.is_none());
            assert!(state.task.is_none());
            assert!(!state.busy);
            assert!(!state.complete);
            assert!(state.error.as_ref().unwrap().contains("Offline preview"));
            assert!(state.input.read(cx).content_empty());
        }
        panel.run_login_task(
            || panic!("preview must not execute authentication work"),
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(commands.try_recv().is_err());
    assert!(vcx.debug_bounds("login-open-browser").is_none());
}

#[gpui::test]
fn preview_api_key_click_discards_secret_without_backend(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::LoginDialogError, cx));
    vcx.run_until_parked();
    let (bridge, commands) = crate::harness::spawn_recording();
    panel.update(vcx, |panel, _| panel.bridge = bridge);
    let button = vcx.debug_bounds("login-provider-openai-api").unwrap();
    vcx.simulate_click(button.center(), gpui::Modifiers::default());
    vcx.simulate_input("preview-only-fake-secret");
    panel.read_with(vcx, |panel, cx| {
        assert!(!panel.login.as_ref().unwrap().input.read(cx).content_empty());
    });
    let button = vcx.debug_bounds("login-submit").unwrap();
    vcx.simulate_click(button.center(), gpui::Modifiers::default());
    panel.read_with(vcx, |panel, cx| {
        let state = panel.login.as_ref().unwrap();
        assert!(state.input.read(cx).content_empty());
        assert!(state.task.is_none());
        assert!(state.flow.is_none());
        assert!(panel.items.is_empty());
        assert!(panel.input.read(cx).content.is_empty());
    });
    assert!(commands.try_recv().is_err());
}
