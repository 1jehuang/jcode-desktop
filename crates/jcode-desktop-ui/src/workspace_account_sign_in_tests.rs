//! GPUI coverage with offline controls and isolated loopback HTTP authentication.
//! Never discovers, reads, or writes real credentials and never opens a browser.
use super::*;

const DRAFT: &str = "Keep my unfinished prompt";

/// The email flow is paused in the app but kept covered here.
fn setup(cx: &mut gpui::TestAppContext) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    setup_with_email(cx, true)
}

fn setup_with_email(
    cx: &mut gpui::TestAppContext,
    email: bool,
) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
    });
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
        w.account_sign_in.email = email;
        w.push_test_panel("account-onboarding-test", cx);
        w
    });
    workspace.update_in(vcx, |w, window, cx| w.focus_active(window, cx));
    vcx.run_until_parked();
    vcx.simulate_input(DRAFT);
    workspace.update_in(vcx, |w, window, cx| w.open_account_sign_in(window, cx));
    vcx.run_until_parked();
    (workspace, vcx)
}

fn click(vcx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = vcx.debug_bounds(selector).expect(selector);
    vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
    vcx.run_until_parked();
}

fn assert_draft_and_focus(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) {
    vcx.update(|window, cx| {
        let w = workspace.read(cx);
        assert!(!w.account_sign_in.visible);
        assert!(w.account_sign_in.task.is_none());
        let panel = w.slots[w.active].panel.read(cx);
        assert_eq!(panel.input.read(cx).content, DRAFT);
        assert!(panel.input_focus_handle(cx).is_focused(window));
    });
    assert!(vcx.debug_bounds("account-sign-in").is_none());
}

#[test]
fn account_offer_is_optional_and_fixture_safe() {
    assert!(EMAIL_SIGN_IN, "email sign-in is offered");
    for handled in [false, true] {
        for connected in [false, true] {
            for fixture in [false, true] {
                assert_eq!(
                    should_offer(handled, connected, fixture),
                    !handled && !connected && !fixture
                );
            }
        }
    }
    // This function is deliberately a no-op under cfg(test), not a disk writer.
    assert!(crate::config::persist_account_sign_in_handled().is_ok());
}

fn type_into_field(vcx: &mut gpui::VisualTestContext, text: &str) {
    click(vcx, "account-sign-in-field");
    vcx.simulate_input(text);
    vcx.run_until_parked();
}

#[gpui::test]
fn email_field_sends_offline_code_and_start_over_returns(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    assert!(vcx.debug_bounds("account-sign-in-card").is_some());
    assert!(vcx.debug_bounds("account-sign-in-brand").is_some());
    assert!(vcx.debug_bounds("account-sign-in-finish").is_some());
    assert!(vcx.debug_bounds("account-sign-in-field").is_some());
    assert!(vcx.debug_bounds("account-sign-in-back").is_none());
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
        assert_eq!(w.account_sign_in.primary_label(), "Sign in with email");
    });
    // Empty and malformed emails stay on the field with a message.
    click(vcx, "account-sign-in-primary");
    assert!(vcx.debug_bounds("account-sign-in-error").is_some());
    type_into_field(vcx, "not-an-email");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
        assert_eq!(
            w.account_sign_in.error.as_deref(),
            Some("Enter a valid email address.")
        );
        assert_eq!(
            w.account_sign_in.input.as_ref().unwrap().read(cx).content,
            "not-an-email"
        );
    });
    workspace.update(vcx, |w, cx| {
        w.account_sign_in
            .input
            .as_ref()
            .unwrap()
            .update(cx, |i, cx| i.set_content(String::new(), cx))
    });
    type_into_field(vcx, "me@example.com");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, cx| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Code { email, login: None } if email == "me@example.com"));
        assert!(w.account_sign_in.task.is_none(), "offline fixture must not spawn authentication");
        assert_eq!(w.account_sign_in.primary_label(), "Verify");
        assert!(w.account_sign_in.input.as_ref().unwrap().read(cx).content.is_empty());
    });
    assert!(vcx.debug_bounds("account-sign-in-sent").is_some());
    click(vcx, "account-sign-in-gmail");
    workspace.read_with(vcx, |w, _| {
        let url = w.account_sign_in.opened_url.as_deref().unwrap();
        assert!(url.starts_with("https://mail.google.com/mail/?authuser=me%40example.com#search/"));
        assert!(url.contains("from%3Alogin%40solosystems.dev"));
        assert!(url.contains("in%3Aanywhere"), "includes Spam");
    });
    click(vcx, "account-sign-in-back");
    workspace.read_with(vcx, |w, cx| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
        assert!(w.account_sign_in.error.is_none());
        assert_eq!(
            w.account_sign_in.input.as_ref().unwrap().read(cx).content,
            "me@example.com"
        );
    });
    assert!(vcx.debug_bounds("account-sign-in-back").is_none());
}

#[gpui::test]
fn typing_six_digits_signs_in_offline(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    type_into_field(vcx, "me@example.com");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    type_into_field(vcx, "123");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Code { .. }));
        assert!(
            w.account_sign_in
                .error
                .as_deref()
                .unwrap()
                .contains("6-digit")
        );
    });
    vcx.simulate_input("456");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Complete { email } if email == "me@example.com"));
        assert!(w.account_sign_in.connected);
    });
    assert!(vcx.debug_bounds("account-sign-in-status").is_some());
    assert!(vcx.debug_bounds("account-sign-in-field").is_none());
}

#[gpui::test]
fn skip_and_escape_preserve_composer_draft_and_focus(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    click(vcx, "account-sign-in-finish");
    assert_draft_and_focus(&workspace, vcx);
    workspace.update_in(vcx, |w, window, cx| w.open_account_sign_in(window, cx));
    vcx.run_until_parked();
    type_into_field(vcx, "draft@example.com");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

fn choice(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) -> Option<Choice> {
    workspace.read_with(vcx, |w, _| {
        let state = &w.account_sign_in;
        state
            .keyboard_choice
            .and_then(|index| state.choices().get(index).copied())
    })
}

#[gpui::test]
fn keyboard_tab_shift_tab_and_enter_follow_visible_choices(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    // Left column top to bottom: the email field and its button first.
    vcx.simulate_keystrokes("tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Field));
    // Tab puts the caret in the field, so typing and Enter go to it.
    vcx.simulate_input("me@example.com");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Code { .. }))
    });
    vcx.simulate_keystrokes("tab tab tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::OpenGmail));
    vcx.simulate_keystrokes("tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::StartOver));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome))
    });
    window_focus(&workspace, vcx);
    vcx.simulate_keystrokes("shift-tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Finish));
    vcx.simulate_keystrokes("shift-tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::ThemeToggle));
    vcx.simulate_keystrokes("tab enter");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

fn window_focus(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) {
    workspace.update_in(vcx, |w, window, cx| window.focus(&w.focus_handle, cx));
}

#[gpui::test]
fn enter_without_focus_continues(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn detected_logins_import_by_default_and_skip_per_row(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.set_account_import_candidates(
            vec![
                ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
                ExternalAuthReviewCandidate::fixture("Gemini", "Gemini CLI"),
            ],
            cx,
        )
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-import-0").is_some());
    assert!(vcx.debug_bounds("account-import-1").is_some());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.selected_imports(), vec![0, 1])
    });
    // Each row has its own Import/Skip slider, and clicking it again restores it.
    click(vcx, "account-import-toggle-0");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.selected_imports(), vec![1]);
        assert!(w.account_sign_in.choices().contains(&Choice::Login(0)));
    });
    click(vcx, "account-import-toggle-0");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.selected_imports(), vec![0, 1])
    });
    // Keyboard toggles the focused row too.
    workspace.update(vcx, |w, cx| {
        let index = w
            .account_sign_in
            .choices()
            .iter()
            .position(|c| *c == Choice::Login(1));
        w.account_sign_in.keyboard_choice = index;
        cx.notify();
    });
    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.selected_imports(), vec![0])
    });
    click(vcx, "account-sign-in-finish");
    assert_draft_and_focus(&workspace, vcx);
    workspace.read_with(vcx, |w, _| {
        assert!(w.account_sign_in.candidates.is_empty());
        assert!(
            w.account_sign_in.import_task.is_none(),
            "tests never import real credentials"
        );
    });
}

#[gpui::test]
fn finish_onboarding_pill_sits_bottom_left_and_finishes(cx: &mut gpui::TestAppContext) {
    for email in [true, false] {
        let (workspace, vcx) = setup_with_email(cx, email);
        let finish = vcx
            .debug_bounds("account-sign-in-finish")
            .expect("finish pill");
        let card = vcx.debug_bounds("account-sign-in-card").unwrap();
        let page = vcx.debug_bounds("account-sign-in").unwrap();
        assert!(finish.right() <= card.right(), "left column: {finish:?}");
        assert!(finish.top() >= card.bottom() - px(1.), "below the choices");
        assert!(
            finish.bottom() >= page.bottom() - px(48.),
            "pinned to the bottom"
        );
        click(vcx, "account-sign-in-finish");
        assert_draft_and_focus(&workspace, vcx);
    }
}

#[gpui::test]
fn demo_composer_shows_model_from_logins_and_imports(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    let identity = |vcx: &mut gpui::VisualTestContext| {
        let panel = workspace.read_with(vcx, |w, _| {
            w.account_sign_in.demo.as_ref().unwrap().panel.clone()
        });
        panel.read_with(vcx, |p, _| (p.provider.clone(), p.model.clone()))
    };
    // Nothing connected or detected: no invented identity.
    workspace.update(vcx, |w, cx| {
        w.accounts = Vec::new();
        cx.notify();
    });
    vcx.run_until_parked();
    assert_eq!(identity(vcx), (None, None));
    // An existing Claude login fills in Claude's default model.
    workspace.update(vcx, |w, cx| {
        w.accounts = accounts::parse(
            r#"{"providers":[
                {"id":"claude","display_name":"Claude","status":"available","auth_kind":"OAuth"}
            ]}"#,
        )
        .unwrap();
        cx.notify();
    });
    vcx.run_until_parked();
    assert_eq!(
        identity(vcx),
        (
            Some("claude".into()),
            Some(jcode_provider_core::DEFAULT_CLAUDE_MODEL.into())
        )
    );
    assert!(vcx.debug_bounds("panel-model-name").is_some());
    // A selected OpenAI import outranks it, and skipping it falls back.
    workspace.update(vcx, |w, cx| {
        w.set_account_import_candidates(
            vec![ExternalAuthReviewCandidate::fixture(
                "OpenAI/Codex",
                "Codex",
            )],
            cx,
        )
    });
    vcx.run_until_parked();
    assert_eq!(
        identity(vcx),
        (
            Some("openai".into()),
            Some(jcode_provider_core::DEFAULT_OPENAI_MODEL.into())
        )
    );
    click(vcx, "account-import-toggle-0");
    assert_eq!(identity(vcx).0.as_deref(), Some("claude"));
}

#[gpui::test]
fn theme_swatches_apply_and_onboarding_has_no_telemetry_or_subscribe(
    cx: &mut gpui::TestAppContext,
) {
    let _theme = crate::theme::test_theme_lock();
    let (workspace, vcx) = setup(cx);
    assert!(
        vcx.debug_bounds("account-theme-grid").is_none(),
        "themes start folded"
    );
    click(vcx, "account-theme-toggle");
    let original = Theme::active_preset();
    let target = crate::theme::ThemePreset::ALL
        .into_iter()
        .position(|preset| preset != Theme::active_preset())
        .unwrap();
    click(
        vcx,
        Box::leak(format!("account-theme-{target}").into_boxed_str()),
    );
    assert_eq!(
        Theme::active_preset(),
        crate::theme::ThemePreset::ALL[target]
    );
    // Telemetry keeps its default, and there is no Subscribe upsell here.
    assert!(vcx.debug_bounds("account-telemetry-menu").is_none());
    assert!(vcx.debug_bounds("account-sign-in-subscribe").is_none());
    workspace.read_with(vcx, |w, _| assert!(w.account_sign_in.visible));
    Theme::select(original);
}

#[gpui::test]
fn theme_hover_previews_before_and_after_a_click(cx: &mut gpui::TestAppContext) {
    let _theme = crate::theme::test_theme_lock();
    let (workspace, vcx) = setup(cx);
    click(vcx, "account-theme-toggle");
    let original = Theme::active_preset();
    let all = crate::theme::ThemePreset::ALL;
    let others: Vec<usize> = (0..all.len()).filter(|&i| all[i] != original).collect();
    let hover = |vcx: &mut gpui::VisualTestContext, index: usize| {
        let selector: &'static str = Box::leak(format!("account-theme-{index}").into_boxed_str());
        let bounds = vcx.debug_bounds(selector).expect(selector);
        vcx.simulate_mouse_move(bounds.center(), None, gpui::Modifiers::default());
        vcx.run_until_parked();
    };
    hover(vcx, others[0]);
    assert_eq!(
        Theme::active_preset(),
        all[others[0]],
        "hover previews before any click"
    );
    let picked: &'static str = Box::leak(format!("account-theme-{}", others[1]).into_boxed_str());
    click(vcx, picked);
    assert_eq!(Theme::active_preset(), all[others[1]]);
    hover(vcx, others[0]);
    assert_eq!(
        Theme::active_preset(),
        all[others[0]],
        "hover still previews after a click"
    );
    let heading = vcx.debug_bounds("account-sign-in-brand").unwrap();
    vcx.simulate_mouse_move(heading.center(), None, gpui::Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        Theme::active_preset(),
        all[others[1]],
        "leaving the swatches returns to the clicked theme"
    );
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.theme_picked, Some(all[others[1]]))
    });
    Theme::select(original);
}

fn field_text(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) -> String {
    workspace.read_with(vcx, |w, cx| {
        w.account_sign_in
            .input
            .as_ref()
            .map(|input| input.read(cx).content.to_string())
            .unwrap_or_default()
    })
}

fn detected(email: &str, sources: Vec<&'static str>) -> DetectedEmail {
    DetectedEmail {
        email: email.into(),
        sources,
    }
}

#[gpui::test]
fn detected_emails_prefill_and_pick_the_jcode_email(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    assert!(
        vcx.debug_bounds("account-emails").is_none(),
        "none detected"
    );
    workspace.update(vcx, |w, cx| {
        w.set_account_emails(
            vec![
                detected("me@home.example", vec!["Codex", "Claude Code"]),
                detected("me@work.example", vec!["Gemini CLI"]),
            ],
            cx,
        )
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-email-1").is_some());
    assert_eq!(field_text(&workspace, vcx), "me@home.example");
    click(vcx, "account-email-1");
    assert_eq!(field_text(&workspace, vcx), "me@work.example");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.picked_email, Some(1))
    });
    // Keyboard: the addresses are the first stops, Enter picks one.
    vcx.simulate_keystrokes("tab enter");
    vcx.run_until_parked();
    assert_eq!(field_text(&workspace, vcx), "me@home.example");
    // Editing the address by hand clears the radio.
    type_into_field(vcx, "x");
    assert_eq!(field_text(&workspace, vcx), "me@home.examplex");
    workspace.read_with(vcx, |w, _| assert_eq!(w.account_sign_in.picked_email, None));
    // Sending the code hides the picker.
    click(vcx, "account-email-0");
    click(vcx, "account-sign-in-primary");
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Code { email, .. } if email == "me@home.example"))
    });
    assert!(vcx.debug_bounds("account-emails").is_none());
}

#[gpui::test]
fn detected_emails_never_overwrite_a_typed_address(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    type_into_field(vcx, "typed@x.example");
    workspace.update(vcx, |w, cx| {
        w.set_account_emails(vec![detected("me@home.example", vec!["Codex"])], cx)
    });
    vcx.run_until_parked();
    assert_eq!(field_text(&workspace, vcx), "typed@x.example");
    workspace.read_with(vcx, |w, _| assert_eq!(w.account_sign_in.picked_email, None));
}

#[gpui::test]
fn detected_emails_hidden_without_email_sign_in(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup_with_email(cx, false);
    workspace.update(vcx, |w, cx| {
        w.set_account_emails(vec![detected("me@home.example", vec!["Codex"])], cx)
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-emails").is_none());
    workspace.read_with(vcx, |w, _| {
        assert!(
            !w.account_sign_in
                .choices()
                .iter()
                .any(|c| matches!(c, Choice::Email(_)))
        )
    });
}

#[gpui::test]
fn email_sign_in_lives_in_the_left_column_and_the_demo_is_untouched(
    cx: &mut gpui::TestAppContext,
) {
    let (workspace, vcx) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.set_account_emails(vec![detected("me@home.example", vec!["Codex"])], cx)
    });
    vcx.run_until_parked();
    let card = vcx.debug_bounds("account-sign-in-card").unwrap();
    let right = vcx.debug_bounds("account-sign-in-right").unwrap();
    for selector in [
        "account-sign-in-email",
        "account-emails",
        "account-sign-in-field",
        "account-sign-in-primary",
        "account-sign-in-finish",
    ] {
        let bounds = vcx.debug_bounds(selector).expect(selector);
        assert!(bounds.right() <= card.right() + px(1.), "{selector}: {bounds:?}");
        assert!(bounds.right() <= right.left() + px(1.), "{selector}: {bounds:?}");
    }
    // The email comes before the provider logins, right under the title.
    let email = vcx.debug_bounds("account-sign-in-email").unwrap();
    let theme = vcx.debug_bounds("account-theme-toggle").unwrap();
    assert!(email.bottom() <= theme.top(), "{email:?} {theme:?}");
    // No sign-in tab or skip icon over the demo any more.
    assert!(vcx.debug_bounds("account-sign-in-continue").is_none());
    let demo = vcx.debug_bounds("account-sign-in-demo").unwrap();
    assert!(demo.size.height > right.size.height * 0.9, "{demo:?}");
    vcx.executor().advance_clock(Duration::from_secs(5));
    vcx.run_until_parked();
    let panel = workspace.read_with(vcx, |w, _| {
        w.account_sign_in.demo.as_ref().unwrap().panel.clone()
    });
    panel.read_with(vcx, |panel, _| {
        assert!(panel.demo);
        assert!(!panel.items.is_empty(), "the replay is actively streaming");
    });
    // The code step replaces the picker in the same spot.
    click(vcx, "account-sign-in-primary");
    assert!(vcx.debug_bounds("account-emails").is_none());
    let sent = vcx.debug_bounds("account-sign-in-sent").unwrap();
    assert!(sent.right() <= card.right() + px(1.), "{sent:?}");
    click(vcx, "account-sign-in-finish");
    assert_draft_and_focus(&workspace, vcx);
    workspace.read_with(vcx, |w, _| assert!(w.account_sign_in.demo.is_none()));
}

#[gpui::test]
fn typing_anywhere_goes_to_the_left_email_field(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    // Typing with nothing focused keeps the keystroke in the email field.
    window_focus(&workspace, vcx);
    vcx.simulate_keystrokes("m e");
    workspace.read_with(vcx, |w, cx| {
        assert_eq!(
            w.account_sign_in.input.as_ref().unwrap().read(cx).content,
            "me"
        );
    });
    vcx.simulate_input("@example.com");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Code { email, .. } if email == "me@example.com"));
    });
}

#[gpui::test]
fn settings_can_reopen_skipped_account_onboarding(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    click(vcx, "account-sign-in-finish");
    super::super::tests::click_sidebar_navigation(&workspace, vcx, "sidebar-settings-tab");
    assert!(vcx.debug_bounds("workspace-settings").is_some());
    click(vcx, "settings-account-sign-in");
    assert!(vcx.debug_bounds("account-sign-in-card").is_some());
    workspace.read_with(vcx, |w, _| {
        assert!(w.account_sign_in.visible);
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
    });
}

#[gpui::test]
fn error_and_complete_render_without_credentials(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.account_sign_in_failed("Offline test failure".into(), cx)
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-sign-in-error").is_some());
    // Typing clears the message.
    type_into_field(vcx, "a");
    assert!(vcx.debug_bounds("account-sign-in-error").is_none());
    workspace.update(vcx, |w, cx| {
        w.account_sign_in.stage = Stage::Complete {
            email: "offline@example.invalid".into(),
        };
        w.account_sign_in.connected = true;
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-sign-in-primary").is_none());
    assert!(vcx.debug_bounds("account-sign-in-status").is_some());
    assert!(vcx.debug_bounds("account-sign-in-back").is_none());
    workspace.update_in(vcx, |w, window, cx| window.focus(&w.focus_handle, cx));
    vcx.simulate_keystrokes("tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::ThemeToggle));
    vcx.simulate_keystrokes("tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Finish));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn compact_360px_card_and_controls_stay_within_window(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    let handle = vcx.update(|window, _| window.window_handle());
    vcx.simulate_window_resize(handle, gpui::size(px(360.), px(800.)));
    for stage in [
        Stage::Welcome,
        Stage::Sending,
        Stage::Code {
            email: "offline@example.invalid".into(),
            login: None,
        },
        Stage::Complete {
            email: "offline@example.invalid".into(),
        },
    ] {
        workspace.update(vcx, |w, cx| {
            w.account_sign_in.stage = stage;
            cx.notify();
        });
        vcx.run_until_parked();
        let card = vcx.debug_bounds("account-sign-in-card").unwrap();
        let finish = vcx.debug_bounds("account-sign-in-finish").unwrap();
        assert!(
            finish.left() >= px(0.) && finish.right() <= px(360.) + px(1.),
            "{finish:?}"
        );
        assert!(
            finish.bottom() <= px(800.) && finish.top() >= card.bottom() - px(1.),
            "{finish:?}"
        );
        assert!(
            card.left() >= px(0.) && card.right() <= px(360.),
            "{card:?}"
        );
        assert!(
            card.top() >= px(0.) && card.bottom() <= px(800.),
            "{card:?}"
        );
        // The email controls scroll with the left column and never overflow it.
        for selector in [
            "account-sign-in-panel",
            "account-sign-in-primary",
            "account-sign-in-back",
            "account-sign-in-field",
        ] {
            if let Some(bounds) = vcx.debug_bounds(selector) {
                assert!(
                    bounds.left() >= card.left() && bounds.right() <= card.right() + px(1.),
                    "{selector}: {bounds:?}"
                );
            }
        }
    }
}

#[gpui::test]
fn snapshots_exclude_all_transient_account_state(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    let snapshot = |vcx: &mut gpui::VisualTestContext| {
        vcx.update(|window, cx| {
            serde_json::to_value(workspace.read(cx).snapshot(window, cx).unwrap()).unwrap()
        })
    };
    workspace.update(vcx, |w, _| w.account_sign_in.visible = false);
    let baseline = snapshot(vcx);
    for stage in [
        Stage::Code {
            email: "FAKE_DEVICE_SECRET@example.invalid".into(),
            login: None,
        },
        Stage::Complete {
            email: "private-offline@example.invalid".into(),
        },
    ] {
        workspace.update(vcx, |w, cx| {
            w.account_sign_in.stage = stage;
            w.account_sign_in.visible = true;
            w.account_sign_in.error = Some("FAKE_ACCESS_TOKEN FAKE_REFRESH_TOKEN".into());
            w.account_sign_in.connected = true;
            w.account_sign_in.keyboard_choice = Some(1);
            cx.notify();
        });
        let value = snapshot(vcx);
        assert_eq!(
            value, baseline,
            "account state must not change persisted workspace"
        );
        let serialized = value.to_string();
        for forbidden in [
            "account_sign_in",
            "device_code",
            "FAKE_DEVICE_SECRET",
            "private-offline@example.invalid",
            "FAKE_ACCESS_TOKEN",
            "FAKE_REFRESH_TOKEN",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "snapshot leaked {forbidden}"
            );
        }
    }
}

#[gpui::test]
fn skip_cancels_pending_gpui_task_before_restoring_composer(cx: &mut gpui::TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};

    struct CancellationProbe(Arc<AtomicBool>);
    impl Drop for CancellationProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let (workspace, vcx) = setup(cx);
    let cancelled = Arc::new(AtomicBool::new(false));
    let probe = CancellationProbe(cancelled.clone());
    workspace.update(vcx, |w, cx| {
        w.account_sign_in.stage = Stage::Sending;
        w.account_sign_in.task = Some(cx.spawn(async move |_, _| {
            let _probe = probe;
            // Model an in-flight request without constructing a client or touching credentials.
            std::future::pending::<()>().await;
        }));
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(!cancelled.load(Ordering::SeqCst));
    workspace.read_with(vcx, |w, _| assert!(w.account_sign_in.task.is_some()));
    click(vcx, "account-sign-in-finish");
    assert!(
        cancelled.load(Ordering::SeqCst),
        "Skip must drop the pending future, not just hide onboarding"
    );
    assert_draft_and_focus(&workspace, vcx);
}

const HTTP_STARTED: &str = r#"{"login_token":"isolated-login-token-000000000000000000000000","expires_in":900,"code_length":6}"#;
const HTTP_APPROVED: &str = r#"{"api_key":"isolated-account-secret","account_id":"test_account","email":"local@example.invalid","tier":"none","status":"inactive"}"#;

/// The production reqwest/Tokio adapter talks to an actual loopback TCP server.
/// Browser opening and credential saving are disabled at compile time in this binary.
fn account_http_server(
    responses: Vec<(u16, &'static str)>,
) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let read = stream.read(&mut bytes).unwrap();
                assert_ne!(read, 0);
                request.extend_from_slice(&bytes[..read]);
                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            tx.send(String::from_utf8(request).unwrap()).unwrap();
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (endpoint, rx)
}

fn submit_email(
    workspace: &Entity<Workspace>,
    vcx: &mut gpui::VisualTestContext,
    endpoint: String,
) {
    workspace.update(vcx, |w, _| w.account_sign_in.test_api_base = Some(endpoint));
    type_into_field(vcx, "local@example.invalid");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
}

#[gpui::test]
fn real_http_email_code_wrong_then_right_signs_in(cx: &mut gpui::TestAppContext) {
    let (endpoint, requests) = account_http_server(vec![
        (200, HTTP_STARTED),
        (
            400,
            r#"{"error":{"code":"invalid_code","message":"no","attempts_remaining":4}}"#,
        ),
        (200, HTTP_APPROVED),
    ]);
    let (workspace, vcx) = setup(cx);
    submit_email(&workspace, vcx, endpoint);
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Code { email, login: Some(_) } if email == "local@example.invalid"));
    });
    type_into_field(vcx, "000000");
    workspace.read_with(vcx, |w, cx| {
        assert!(matches!(w.account_sign_in.stage, Stage::Code { .. }));
        assert!(
            w.account_sign_in
                .error
                .as_deref()
                .unwrap()
                .contains("4 tries left")
        );
        assert!(
            w.account_sign_in
                .input
                .as_ref()
                .unwrap()
                .read(cx)
                .content
                .is_empty()
        );
    });
    type_into_field(vcx, "123456");
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Complete { email } if email == "local@example.invalid"));
        assert!(w.account_sign_in.connected);
        assert!(w.account_sign_in.error.is_none());
    });
    let wire: Vec<_> = requests.try_iter().collect();
    assert_eq!(wire.len(), 3);
    assert!(wire[0].starts_with("POST /v1/auth/email/start "));
    assert!(wire[0].contains(r#""email":"local@example.invalid""#));
    assert!(wire[1..].iter().all(
        |r| r.starts_with("POST /v1/auth/email/verify ") && r.contains("isolated-login-token")
    ));
    click(vcx, "account-sign-in-finish");
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn real_http_send_failure_and_expired_code_are_recoverable(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    let (endpoint, _requests) = account_http_server(vec![(
        429,
        r#"{"error":{"code":"rate_limited","message":"no"}}"#,
    )]);
    submit_email(&workspace, vcx, endpoint);
    workspace.read_with(vcx, |w, cx| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
        assert!(
            w.account_sign_in
                .error
                .as_deref()
                .unwrap()
                .contains("Too many")
        );
        assert_eq!(
            w.account_sign_in.input.as_ref().unwrap().read(cx).content,
            "local@example.invalid"
        );
    });
    let (endpoint, _requests) = account_http_server(vec![
        (200, HTTP_STARTED),
        (400, r#"{"error":{"code":"expired_code","message":"no"}}"#),
    ]);
    workspace.update(vcx, |w, cx| {
        w.account_sign_in
            .input
            .as_ref()
            .unwrap()
            .update(cx, |i, cx| i.set_content(String::new(), cx))
    });
    submit_email(&workspace, vcx, endpoint);
    type_into_field(vcx, "123456");
    workspace.read_with(vcx, |w, cx| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
        assert!(
            w.account_sign_in
                .error
                .as_deref()
                .unwrap()
                .contains("expired")
        );
        assert_eq!(
            w.account_sign_in.input.as_ref().unwrap().read(cx).content,
            "local@example.invalid"
        );
        assert!(!w.account_sign_in.connected);
    });
    click(vcx, "account-sign-in-finish");
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn logins_split_into_in_jcode_and_importable(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.accounts = accounts::parse(
            r#"{"providers":[
                {"id":"claude","display_name":"Claude","status":"available","auth_kind":"OAuth"},
                {"id":"gemini","display_name":"Gemini","status":"not_configured","auth_kind":"OAuth"}
            ]}"#,
        )
        .unwrap();
        w.set_account_import_candidates(
            vec![
                ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
                ExternalAuthReviewCandidate::fixture("Gemini", "Gemini CLI"),
                ExternalAuthReviewCandidate::fixture("Claude, Gemini", "OpenCode"),
            ],
            cx,
        )
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-logins-in-jcode").is_some());
    // Claude is already in Jcode, so only sources adding something new remain.
    assert!(vcx.debug_bounds("account-import-0").is_none());
    assert!(vcx.debug_bounds("account-import-1").is_some());
    assert!(vcx.debug_bounds("account-import-2").is_some());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.selected_imports(), vec![1, 2]);
        assert!(!w.account_sign_in.choices().contains(&Choice::Login(0)));
    });

    // Once everything detected is already in Jcode, nothing is offered.
    workspace.update(vcx, |w, cx| {
        w.accounts = accounts::parse(
            r#"{"providers":[
                {"id":"claude","display_name":"Claude","status":"available","auth_kind":"OAuth"},
                {"id":"gemini","display_name":"Gemini","status":"available","auth_kind":"OAuth"}
            ]}"#,
        )
        .unwrap();
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-logins-import-empty").is_some());
    assert!(vcx.debug_bounds("account-import-less").is_none());
    workspace.read_with(vcx, |w, _| {
        assert!(w.account_sign_in.selected_imports().is_empty())
    });
}

#[gpui::test]
fn paused_email_shows_only_finish_and_typing_forwards_to_a_panel(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup_with_email(cx, false);
    assert!(vcx.debug_bounds("account-sign-in-field").is_none());
    assert!(vcx.debug_bounds("account-sign-in-primary").is_none());
    assert!(vcx.debug_bounds("account-sign-in-email").is_none());
    assert!(vcx.debug_bounds("account-sign-in-panel").is_none());
    assert!(vcx.debug_bounds("panel-build").is_none());
    workspace.read_with(vcx, |w, _| {
        assert!(!w.account_sign_in.choices().contains(&Choice::Field));
        assert!(!w.account_sign_in.field_open());
    });

    vcx.simulate_input(" fix the sidebar");
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let w = workspace.read(cx);
        assert!(!w.account_sign_in.visible);
        let panel = w.slots[w.active].panel.read(cx);
        assert_eq!(
            panel.input.read(cx).content,
            format!("{DRAFT} fix the sidebar")
        );
        assert!(panel.input_focus_handle(cx).is_focused(window));
    });
    assert!(vcx.debug_bounds("account-sign-in").is_none());
}

#[gpui::test]
fn paused_email_enter_and_escape_enter_workspace(cx: &mut gpui::TestAppContext) {
    for key in ["enter", "escape"] {
        let (workspace, vcx) = setup_with_email(cx, false);
        vcx.simulate_keystrokes(key);
        vcx.run_until_parked();
        assert_draft_and_focus(&workspace, vcx);
    }
}
