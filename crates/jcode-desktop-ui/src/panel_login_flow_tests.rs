//! End-to-end native controller → SDK → isolated CLI transport regression tests.
//! Only synthetic credentials are used. No browser or real runtime is contacted.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn fixture(kind: &str) -> (tempfile::TempDir, AuthClient) {
    fixture_with_url(kind, "https://example.invalid/authorize")
}

fn fixture_with_url(kind: &str, url: &str) -> (tempfile::TempDir, AuthClient) {
    fixture_with_validation(kind, url, false)
}

fn fixture_with_validation(
    kind: &str,
    url: &str,
    validation_warning: bool,
) -> (tempfile::TempDir, AuthClient) {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("auth-fixture.py");
    std::fs::write(
        &binary,
        format!(
            r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
home = Path(os.environ['JCODE_HOME'])
args = sys.argv[1:]
provider = args[args.index('--provider') + 1]
if '--print-auth-url' in args:
    print(json.dumps({{'provider':provider, 'status':'pending', 'auth_url':'{url}',
        'input_kind':'{kind}', 'user_code':'TEST-CODE', 'expires_at_ms':9999999999999}}))
elif '--cancel' in args:
    print(json.dumps({{'provider':provider, 'status':'cancelled'}}))
else:
    if (home / 'fail-exchange').exists():
        print('Error: Token exchange failed: test-private-code', file=sys.stderr)
        sys.exit(1)
    # The real SDK must send the code over stdin, never argv.
    data = sys.stdin.read() if '--complete' not in args else 'device-approved'
    assert 'test-private-code' not in ' '.join(args)
    (home / 'submitted').write_text(data)
    print(json.dumps({{'provider':provider, 'status':'authenticated'}}))
    # Older installed CLIs append this human report even with --json. The
    # credentials are already saved, so Desktop must not ask to reuse the code.
    print('=== auth-test: ' + provider + ' ===')
    print('result: ' + ('FAIL' if {validation_warning} else 'PASS'))
    sys.exit(1 if {validation_warning} else 0)
"#,
            validation_warning = if validation_warning { "True" } else { "False" }
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let client = AuthClient::new(AuthOptions {
        binary,
        jcode_home: Some(root.path().into()),
        socket: Some(root.path().join("absent.sock")),
        timeout: std::time::Duration::from_secs(3),
    });
    (root, client)
}

#[gpui::test]
fn saved_oauth_with_failed_validation_offers_models_not_code_retry(cx: &mut gpui::TestAppContext) {
    let (root, client) =
        fixture_with_validation("auth_code", "https://example.invalid/authorize", true);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("login-warning-test".into(), None, None, bridge, cx));
    panel.update(vcx, |panel, cx| {
        panel.open_login_picker(cx);
        panel.login.as_mut().unwrap().client = client;
        panel.login_command("/login claude", cx);
    });
    vcx.run_until_parked();
    vcx.simulate_input("test-private-code");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(root.path().join("submitted").is_file());
    assert!(vcx.debug_bounds("login-complete").is_some());
    assert!(vcx.debug_bounds("login-choose-model").is_some());
    assert!(vcx.debug_bounds("login-submit").is_none());
    assert!(vcx.debug_bounds("login-retry").is_none());
    panel.read_with(vcx, |panel, cx| {
        let state = panel.login.as_ref().unwrap();
        assert!(state.complete && !state.busy);
        assert!(
            state
                .error
                .as_deref()
                .unwrap()
                .contains("Credentials were saved")
        );
        assert!(state.input.read(cx).content_empty());
        assert!(state.prompt.is_none());
    });
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::RefreshRuntime { .. })
    ));
}

#[gpui::test]
fn code_login_enter_completes_real_sdk_transport_and_preserves_chat(cx: &mut gpui::TestAppContext) {
    let (root, client) = fixture("auth_code");
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) =
        cx.add_window_view(|_, cx| Panel::new("login-test".into(), None, None, bridge, cx));
    panel.update(vcx, |panel, cx| {
        panel.input.update(cx, |input, cx| {
            input.set_content("untouched draft".into(), cx)
        });
        panel.open_login_picker(cx);
        panel.login.as_mut().unwrap().client = client;
        panel.login_command("/login claude", cx);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("login-logo-claude").is_some());
    assert!(vcx.debug_bounds("login-open-browser").is_some());
    vcx.simulate_input("test-private-code");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(root.path().join("submitted")).unwrap(),
        "test-private-code"
    );
    assert!(vcx.debug_bounds("login-complete").is_some());
    panel.read_with(vcx, |panel, cx| {
        let state = panel.login.as_ref().unwrap();
        assert!(state.complete);
        assert!(state.input.read(cx).content_empty());
        assert!(state.prompt.is_none());
        assert_eq!(panel.input.read(cx).content, "untouched draft");
        assert!(panel.items.is_empty());
        assert!(
            !serde_json::to_string(&panel.snapshot(cx))
                .unwrap()
                .contains("test-private-code")
        );
    });
    assert!(matches!(
        commands.try_recv(),
        Ok(Command::RefreshRuntime { .. })
    ));
    vcx.simulate_keystrokes("enter");
    assert!(commands.try_recv().is_err(), "completion must not resubmit");
}

#[gpui::test]
fn device_login_polls_without_a_finish_click(cx: &mut gpui::TestAppContext) {
    let (root, client) = fixture("complete");
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new_accounts("device-test", None, crate::harness::spawn_inert(), cx)
    });
    panel.update(vcx, |panel, cx| {
        panel.login.as_mut().unwrap().client = client;
        panel.login_command("/login copilot", cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(root.path().join("submitted")).unwrap(),
        "device-approved"
    );
    assert!(vcx.debug_bounds("login-complete").is_some());
}

#[gpui::test]
fn empty_key_enter_has_actionable_error_and_provider_logos(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new_accounts("key-test", None, crate::harness::spawn_inert(), cx)
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("login-method-browser").is_some());
    assert!(vcx.debug_bounds("login-method-key").is_some());
    panel.update(vcx, |panel, cx| {
        panel.login_command("/login openai-api", cx);
    });
    vcx.simulate_keystrokes("enter");
    panel.read_with(vcx, |panel, _| {
        assert_eq!(
            panel.login.as_ref().unwrap().error.as_deref(),
            Some("Paste an API key first.")
        );
    });
}

#[gpui::test]
fn polling_keeps_browser_and_copy_controls_visible(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new_accounts("device-test", None, crate::harness::spawn_inert(), cx)
    });
    panel.update(vcx, |panel, cx| {
        let state = panel.login.as_mut().unwrap();
        state.provider = state.client.resolve_provider("copilot");
        state.prompt = Some(AuthPrompt {
            auth_url: "https://example.invalid/device".into(),
            input_kind: AuthInputKind::DeviceCode,
            user_code: Some("TEST-CODE".into()),
            expires_at_ms: i64::MAX,
        });
        state.busy = true;
        cx.notify();
    });
    for selector in [
        "login-open-browser",
        "login-copy-link",
        "login-copy-code",
        "login-busy",
        "login-close",
    ] {
        assert!(
            vcx.debug_bounds(selector).is_some(),
            "{selector} must remain reachable"
        );
    }
    let copy = vcx.debug_bounds("login-copy-code").unwrap();
    vcx.simulate_click(copy.center(), gpui::Modifiers::default());
    vcx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("TEST-CODE")
        )
    });
}

#[gpui::test]
fn browser_callback_completes_native_login_without_paste(cx: &mut gpui::TestAppContext) {
    use std::io::{Read, Write};
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let url = format!(
        "https://example.invalid/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A{port}%2Fauth%2Fcallback&state=test-state"
    );
    let (root, client) = fixture_with_url("callback_url", &url);
    let browser = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut stream = loop {
            if let Ok(stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                break stream;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "callback listener never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        write!(stream, "GET /auth/callback?code=test-private-code&state=test-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(!response.contains("test-private-code"));
    });
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new_accounts("callback-test", None, crate::harness::spawn_inert(), cx)
    });
    panel.update(vcx, |panel, cx| {
        panel.login.as_mut().unwrap().client = client;
        panel.login_command("/login openai", cx);
    });
    vcx.run_until_parked();
    browser.join().unwrap();
    assert!(vcx.debug_bounds("login-complete").is_some());
    assert!(
        std::fs::read_to_string(root.path().join("submitted"))
            .unwrap()
            .contains("code=test-private-code")
    );
    panel.read_with(vcx, |panel, cx| {
        let state = panel.login.as_ref().unwrap();
        assert!(state.complete && !state.callback_waiting && !state.busy);
        assert!(state.input.read(cx).content_empty());
        assert!(panel.items.is_empty());
    });
}

#[gpui::test]
fn callback_failure_preserves_real_safe_error_and_offers_restart(cx: &mut gpui::TestAppContext) {
    use std::io::{Read, Write};
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let url = format!(
        "https://example.invalid/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A{port}%2Fauth%2Fcallback&state=test-state"
    );
    let (root, client) = fixture_with_url("callback_url", &url);
    std::fs::write(root.path().join("fail-exchange"), "fixture").unwrap();
    let browser = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut stream = loop {
            if let Ok(stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                break stream;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "listener never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        write!(stream, "GET /auth/callback?code=test-private-code&state=test-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
    });
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        Panel::new_accounts(
            "callback-failure-test",
            None,
            crate::harness::spawn_inert(),
            cx,
        )
    });
    panel.update(vcx, |panel, cx| {
        panel.login.as_mut().unwrap().client = client;
        panel.login_command("/login openai", cx);
    });
    vcx.run_until_parked();
    browser.join().unwrap();
    for selector in [
        "login-error",
        "login-retry",
        "login-progress",
        "login-current-step",
    ] {
        assert!(vcx.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(vcx.debug_bounds("login-complete").is_none());
    panel.read_with(vcx, |panel, cx| {
        let state = panel.login.as_ref().unwrap();
        assert!(!state.busy && !state.callback_waiting && !state.complete);
        let error = state.error.as_ref().unwrap();
        assert!(error.contains("token exchange"), "{error}");
        assert!(!error.contains("test-private-code"));
        assert!(!state.flow.as_ref().unwrap().has_callback_listener());
        assert!(
            !serde_json::to_string(&panel.snapshot(cx))
                .unwrap()
                .contains("test-private-code")
        );
    });
}
