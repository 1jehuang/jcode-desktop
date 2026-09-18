//! Offline GPUI coverage. No startup credential discovery or authentication requests.
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
    assert!(vcx.debug_bounds("account-sign-in-skip").is_some());
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
    click(vcx, "account-sign-in-skip");
    assert_draft_and_focus(&workspace, vcx);
    workspace.update_in(vcx, |w, window, cx| w.open_account_sign_in(window, cx));
    vcx.run_until_parked();
    click(vcx, "account-sign-in-primary");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn keyboard_tab_shift_tab_and_enter_follow_visible_choices(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    for (key, expected) in [
        ("tab", 0),
        ("tab", 1),
        ("tab", 0),
        ("shift-tab", 1),
        ("shift-tab", 0),
    ] {
        vcx.simulate_keystrokes(key);
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.account_sign_in.keyboard_choice, Some(expected))
        });
    }
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Waiting { .. }))
    });
    vcx.simulate_keystrokes("shift-tab");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.keyboard_choice, Some(2))
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    workspace.read_with(vcx, |w, _| {
        assert!(matches!(w.account_sign_in.stage, Stage::Welcome))
    });
    vcx.simulate_keystrokes("shift-tab");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_draft_and_focus(&workspace, vcx);
}

#[gpui::test]
fn settings_can_reopen_skipped_account_onboarding(cx: &mut gpui::TestAppContext) {
    let (workspace, vcx) = setup(cx);
    click(vcx, "account-sign-in-skip");
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
    assert!(vcx.debug_bounds("account-sign-in-primary").is_some());
    assert!(vcx.debug_bounds("account-sign-in-skip").is_none());
    assert!(vcx.debug_bounds("account-sign-in-back").is_none());
    workspace.read_with(vcx, |w, _| {
        assert_eq!(w.account_sign_in.primary_label(), "Continue to workspace")
    });
    for key in ["tab", "shift-tab"] {
        vcx.simulate_keystrokes(key);
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.account_sign_in.keyboard_choice, Some(0))
        });
    }
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
            "account-sign-in-skip",
            "account-sign-in-back",
        ] {
            if let Some(bounds) = vcx.debug_bounds(selector) {
                assert!(
                    bounds.left() >= card.left() && bounds.right() <= card.right(),
                    "{selector}: {bounds:?}"
                );
                assert!(
                    bounds.top() >= card.top() && bounds.bottom() <= card.bottom(),
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
    click(vcx, "account-sign-in-skip");
    assert!(
        cancelled.load(Ordering::SeqCst),
        "Skip must drop the pending future, not just hide onboarding"
    );
    assert_draft_and_focus(&workspace, vcx);
}
