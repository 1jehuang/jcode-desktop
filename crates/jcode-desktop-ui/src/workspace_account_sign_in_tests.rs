//! GPUI coverage with offline controls and isolated loopback HTTP authentication.
//! Never discovers, reads, or writes real credentials and never opens a browser.
use super::*;

const DRAFT: &str = "Keep my unfinished prompt";

fn setup(cx: &mut gpui::TestAppContext) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        crate::bind_workspace_keys(cx);
        crate::input::bind_keys(cx);
    });
    let (workspace, vcx) = cx.add_window_view(|_, cx| {
        let mut w = Workspace::for_test(learning::Coach::new(), cx);
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

#[gpui::test]
fn welcome_native_primary_waits_offline_and_cancel_returns(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    assert!(vcx.debug_bounds("account-sign-in-card").is_some());
    assert!(vcx.debug_bounds("account-sign-in-brand").is_some());
    assert!(vcx.debug_bounds("account-sign-in-continue").is_some());
    assert!(vcx.debug_bounds("account-sign-in-back").is_none());
    workspace.read_with(vcx, |w, _| {
        assert!(w.account_sign_in.visible);
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
        assert_eq!(w.account_sign_in.primary_label(), "Sign in with email");
    });
    click(vcx, "account-sign-in-primary");
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Waiting { url } if url == "https://jcode.sh/account"));
        assert!(w.account_sign_in.task.is_none(), "offline primary must not spawn authentication");
        assert_eq!(w.account_sign_in.primary_label(), "Open browser again");
    });
    click(vcx, "account-sign-in-primary"); // Reopening is also inert in tests.
    click(vcx, "account-sign-in-back");
    workspace.read_with(vcx, |w, _| {
        assert!(w.account_sign_in.visible);
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
        assert!(w.account_sign_in.error.is_none());
        assert!(w.account_sign_in.keyboard_choice.is_none());
    });
    assert!(vcx.debug_bounds("account-sign-in-back").is_none());
}

#[gpui::test]
fn skip_and_escape_preserve_composer_draft_and_focus(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    click(vcx, "account-sign-in-continue");
    assert_draft_and_focus(&workspace, vcx);
    workspace.update_in(vcx, |w, window, cx| w.open_account_sign_in(window, cx));
    vcx.run_until_parked();
    click(vcx, "account-sign-in-primary");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

fn choice(workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext) -> Option<Choice> {
    workspace.read_with(vcx, |w, _| {
        let state = &w.account_sign_in;
        state.keyboard_choice.and_then(|index| state.choices().get(index).copied())
    })
}

#[gpui::test]
fn keyboard_tab_shift_tab_and_enter_follow_visible_choices(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    vcx.simulate_keystrokes("tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Primary));
    vcx.simulate_keystrokes("tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Subscribe));
    vcx.simulate_keystrokes("shift-tab shift-tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Continue));
    vcx.simulate_keystrokes("tab enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Waiting { .. }))
    });
    vcx.simulate_keystrokes("tab tab tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::StartOver));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome))
    });
    vcx.simulate_keystrokes("shift-tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Continue));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn enter_without_focus_continues(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn detected_logins_import_less_and_continue_recap(cx: &mut gpui::TestAppContext) {
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
    // Rows are read-only until the user asks to import less.
    click(vcx, "account-import-0");
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.selected_imports(), vec![0, 1])
    });
    click(vcx, "account-import-less");
    click(vcx, "account-import-0");
    workspace.read_with(vcx, |w, _| {
        assert!(w.account_sign_in.choosing);
        assert_eq!(w.account_sign_in.selected_imports(), vec![1]);
        assert!(w.account_sign_in.choices().contains(&Choice::Login(0)));
    });
    // Keyboard toggles the focused login row too.
    workspace.update(vcx, |w, cx| {
        let index = w.account_sign_in.choices().iter().position(|c| *c == Choice::Login(1));
        w.account_sign_in.keyboard_choice = index;
        cx.notify();
    });
    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| assert!(w.account_sign_in.selected_imports().is_empty()));
    // "Import all" restores the default of importing everything.
    click(vcx, "account-import-less");
    workspace.read_with(vcx, |w, _| {
        assert!(!w.account_sign_in.choosing);
        assert_eq!(w.account_sign_in.selected_imports(), vec![0, 1]);
    });
    click(vcx, "account-sign-in-continue");
    assert_draft_and_focus(&workspace, vcx);
    workspace.read_with(vcx, |w, _| {
        assert!(w.account_sign_in.candidates.is_empty());
        assert!(w.account_sign_in.import_task.is_none(), "tests never import real credentials");
    });
}

#[gpui::test]
fn theme_swatches_and_telemetry_choices_apply(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    let original = Theme::active_preset();
    let target = crate::theme::ThemePreset::ALL
        .into_iter()
        .position(|preset| preset != Theme::active_preset())
        .unwrap();
    click(vcx, Box::leak(format!("account-theme-{target}").into_boxed_str()));
    assert_eq!(Theme::active_preset(), crate::theme::ThemePreset::ALL[target]);
    for (selector, level) in [
        ("account-telemetry-off", Telemetry::Off),
        ("account-telemetry-everything", Telemetry::Everything),
    ] {
        click(vcx, selector);
        workspace.read_with(vcx, |w, _| assert_eq!(w.account_sign_in.telemetry, level));
    }
    workspace.read_with(vcx, |w, _| assert!(w.account_sign_in.visible));
    Theme::select(original);
}

#[gpui::test]
fn settings_can_reopen_skipped_account_onboarding(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    click(vcx, "account-sign-in-continue");
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
fn error_retry_and_complete_render_without_credentials(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    workspace.update(vcx, |w, cx| {
        w.account_sign_in_failed("Offline test failure".into(), cx)
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-sign-in-error").is_some());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.primary_label(), "Try again")
    });
    click(vcx, "account-sign-in-primary");
    assert!(vcx.debug_bounds("account-sign-in-error").is_none());
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Waiting { .. }))
    });
    workspace.update(vcx, |w, cx| {
        w.account_sign_in.stage = Stage::Complete {
            email: "offline@example.invalid".into(),
        };
        w.account_sign_in.connected = true;
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("account-sign-in-primary").is_none());
    assert!(vcx.debug_bounds("account-sign-in-continue").is_some());
    assert!(vcx.debug_bounds("account-sign-in-back").is_none());
    vcx.simulate_keystrokes("tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Theme));
    vcx.simulate_keystrokes("shift-tab");
    assert_eq!(choice(&workspace, vcx), Some(Choice::Continue));
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
        Stage::Starting,
        Stage::Waiting {
            url: "https://example.invalid/offline".into(),
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
        let proceed = vcx.debug_bounds("account-sign-in-continue").unwrap();
        assert!(proceed.left() >= px(0.) && proceed.right() <= px(360.), "{proceed:?}");
        assert!(proceed.bottom() <= px(800.) && proceed.top() >= card.bottom() - px(1.), "{proceed:?}");
        assert!(
            card.left() >= px(0.) && card.right() <= px(360.),
            "{card:?}"
        );
        assert!(
            card.top() >= px(0.) && card.bottom() <= px(800.),
            "{card:?}"
        );
        for selector in [
            "account-sign-in-primary",
            "account-sign-in-subscribe",
            "account-sign-in-back",
            "account-sign-in-copy",
        ] {
            if let Some(bounds) = vcx.debug_bounds(selector) {
                assert!(
                    bounds.left() >= card.left() && bounds.right() <= card.right(),
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
        Stage::Waiting {
            url: "https://example.invalid/flow?device_code=FAKE_DEVICE_SECRET".into(),
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
        w.account_sign_in.stage = Stage::Starting;
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
    click(vcx, "account-sign-in-continue");
    assert!(
        cancelled.load(Ordering::SeqCst),
        "Skip must drop the pending future, not just hide onboarding"
    );
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn copy_link_uses_only_public_flow_and_resets_on_restart(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    click(vcx, "account-sign-in-primary");
    click(vcx, "account-sign-in-copy");
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("https://jcode.sh/account")
        );
        assert!(workspace.read(cx).account_sign_in.link_copied);
    });
    click(vcx, "account-sign-in-back");
    click(vcx, "account-sign-in-primary");
    workspace.read_with(vcx, |w, _| assert!(!w.account_sign_in.link_copied));
    vcx.simulate_keystrokes("tab tab enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| assert!(w.account_sign_in.link_copied));
}

const HTTP_DEVICE: &str = r#"{"device_code":"isolated-device-secret","flow_id":"test-public-flow","verification_uri":"https://jcode.sh/account","verification_uri_complete":"https://jcode.sh/account?flow=test-public-flow","expires_in":600,"interval":3}"#;
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

fn poll_after(vcx: &mut gpui::VisualTestContext, seconds: u64) {
    vcx.executor().advance_clock(Duration::from_secs(seconds));
    vcx.run_until_parked();
}

#[gpui::test]
fn real_http_pending_retry_slowdown_and_free_account_approval(cx: &mut gpui::TestAppContext) {
    let (endpoint, requests) = account_http_server(vec![
        (200, HTTP_DEVICE),
        (428, r#"{"error":"authorization_pending"}"#),
        (503, "{}"),
        (429, "{}"),
        (200, HTTP_APPROVED),
    ]);
    let (workspace, vcx) = setup(cx);
    workspace.update(vcx, |w, _| w.account_sign_in.test_api_base = Some(endpoint));
    click(vcx, "account-sign-in-primary");
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Waiting { url } if url == "https://jcode.sh/account?flow=test-public-flow"));
        assert_eq!(w.account_sign_in.remaining, Some(Duration::from_secs(600)));
    });
    assert!(vcx.debug_bounds("account-sign-in-progress").is_some());
    click(vcx, "account-sign-in-copy");
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("https://jcode.sh/account?flow=test-public-flow")
        )
    });
    poll_after(vcx, 3);
    workspace.read_with(vcx, |w, _| assert!(w.account_sign_in.error.is_none()));
    poll_after(vcx, 3);
    workspace.read_with(vcx, |w, _| {
        assert!(
            w.account_sign_in
                .error
                .as_deref()
                .unwrap()
                .contains("Connection interrupted")
        )
    });
    poll_after(vcx, 5);
    workspace.read_with(vcx, |w, _| {
        assert!(
            w.account_sign_in
                .error
                .as_deref()
                .unwrap()
                .contains("service is busy")
        )
    });
    poll_after(vcx, 8);
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(&w.account_sign_in.stage, Stage::Complete { email } if email == "local@example.invalid"));
        assert!(w.account_sign_in.connected);
        assert!(w.account_sign_in.error.is_none());
    });
    let wire: Vec<_> = requests.try_iter().collect();
    assert_eq!(wire.len(), 5);
    assert!(wire[0].starts_with("POST /v1/auth/device "));
    assert!(wire[0].contains(r#""client_name":"jcode-cli""#));
    assert!(
        wire[1..]
            .iter()
            .all(|request| request.starts_with("POST /v1/auth/token ")
                && request.contains("isolated-device-secret"))
    );
    click(vcx, "account-sign-in-continue");
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn real_http_denial_expiry_and_invalid_start_are_recoverable(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    for (responses, expected) in [
        (
            vec![(200, HTTP_DEVICE), (403, r#"{"error":"access_denied"}"#)],
            "not approved",
        ),
        (
            vec![(200, HTTP_DEVICE), (400, r#"{"error":"expired_token"}"#)],
            "expired",
        ),
        (vec![(200, "{not-json}")], "invalid response"),
    ] {
        let (endpoint, requests) = account_http_server(responses);
        workspace.update(vcx, |w, _| w.account_sign_in.test_api_base = Some(endpoint));
        click(vcx, "account-sign-in-primary");
        poll_after(vcx, 3);
        workspace.read_with(vcx, |w, _| {
            assert!(matches!(w.account_sign_in.stage, Stage::Welcome));
            assert!(
                w.account_sign_in
                    .error
                    .as_deref()
                    .unwrap()
                    .contains(expected)
            );
            assert!(!w.account_sign_in.connected);
        });
        assert!(!requests.try_iter().collect::<Vec<_>>().is_empty());
    }
    click(vcx, "account-sign-in-continue");
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn real_http_skip_cancels_polling_and_preserves_draft(cx: &mut gpui::TestAppContext) {
    let (endpoint, requests) = account_http_server(vec![(200, HTTP_DEVICE)]);
    let (workspace, vcx) = setup(cx);
    workspace.update(vcx, |w, _| w.account_sign_in.test_api_base = Some(endpoint));
    click(vcx, "account-sign-in-primary");
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Waiting { .. }));
        assert!(w.account_sign_in.task.is_some());
    });
    click(vcx, "account-sign-in-continue");
    poll_after(vcx, 30);
    workspace.read_with(vcx, |w, _| {
        assert!(!w.account_sign_in.connected);
        assert!(w.account_sign_in.error.is_none());
    });
    assert_eq!(requests.try_iter().count(), 1);
    assert_draft_and_focus(&workspace, vcx);
}
