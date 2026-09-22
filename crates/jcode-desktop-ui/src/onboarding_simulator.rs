//! Entry points for the real, isolated production first-run Desktop flow.
//! The launcher owns profile isolation. This workspace is never replaced.
use super::*;
use std::process::Command;

#[derive(Default)]
pub(super) struct LaunchState {
    #[cfg(not(test))]
    pending: bool,
    pub(super) error: Option<String>,
    #[cfg(test)]
    requests: Vec<PathBuf>,
}

fn launcher_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/onboarding-desktop.py")
}

/// Keep command construction injectable and independent of the caller's cwd.
/// No account/session arguments or shell interpolation are passed to the script.
fn launcher_command(script: &Path) -> Command {
    let mut command = Command::new("python3");
    command.arg(script).stdin(std::process::Stdio::null());
    command
}

#[cfg(not(test))]
fn run_launcher(script: &Path) -> Result<(), String> {
    let output = launcher_command(script)
        .output()
        .map_err(|error| format!("Could not start fresh-profile Desktop: {error}"))?;
    if !output.status.success() {
        // Avoid surfacing arbitrary environment/account data from child output.
        return Err(format!(
            "Fresh-profile Desktop launcher failed ({}).",
            output.status
        ));
    }
    validate_response(&output.stdout)
}

fn validate_response(stdout: &[u8]) -> Result<(), String> {
    let result: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|_| "Fresh-profile Desktop launcher returned an invalid response.".to_owned())?;
    if result.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("Fresh-profile Desktop launcher could not open the new window.".into());
    }
    Ok(())
}

impl Workspace {
    pub(super) fn launch_onboarding(&mut self, cx: &mut Context<Self>) {
        let script = launcher_path();
        // Unit/visual tests record the exact production route, never launch a process.
        #[cfg(test)]
        {
            self.onboarding_launch.requests.push(script);
            let _ = cx;
            return;
        }
        #[cfg(not(test))]
        {
            if harness::screenshot_mode() || self.onboarding_launch.pending {
                return;
            }
            self.onboarding_launch.pending = true;
            self.onboarding_launch.error = None;
            // The script starts the separate window, prints JSON, then exits.
            // Both process creation and waiting happen off the UI thread.
            let launch = cx
                .background_executor()
                .spawn(async move { run_launcher(&script) });
            cx.spawn(async move |this, cx| {
                let result = launch.await;
                let _ = this.update(cx, |workspace, cx| {
                    workspace.onboarding_launch.pending = false;
                    workspace.onboarding_launch.error = result.err();
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(crate) fn toggle_onboarding_simulator(
        &mut self,
        _: &ToggleOnboardingSimulator,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.launch_onboarding(cx);
    }

    pub(super) fn render_onboarding_launch_error(&self, cx: &Context<Self>) -> gpui::Div {
        div()
            .debug_selector(|| "onboarding-launch-error".into())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                cx.stop_propagation();
                if event.keystroke.key == "escape" {
                    this.onboarding_launch.error = None;
                    this.focus_active(window, cx);
                    cx.notify();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap_3()
            .bg(Theme::global().BG)
            .text_color(Theme::global().TEXT)
            .child("Could not open first-run Desktop")
            .child(self.onboarding_launch.error.clone().unwrap_or_default())
            .child(
                div()
                    .id("onboarding-launch-dismiss")
                    .debug_selector(|| "onboarding-launch-dismiss".into())
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .child("Return to workspace")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.onboarding_launch.error = None;
                        this.focus_active(window, cx);
                        cx.notify();
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_uses_absolute_script_without_personal_arguments() {
        let script = launcher_path();
        assert!(script.is_absolute());
        assert!(script.ends_with("scripts/onboarding-desktop.py"));
        let command = launcher_command(&script);
        assert_eq!(command.get_program(), "python3");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![script.as_os_str()]
        );
    }

    #[test]
    fn launcher_response_errors_are_visible_without_leaking_child_output() {
        assert!(validate_response(br#"{"ok":true,"pid":42}"#).is_ok());
        assert!(validate_response(br#"{"ok":false,"error":"private token"}"#).is_err());
        assert!(validate_response(b"null").is_err());
        assert!(validate_response(b"{}").is_err());
        let error = validate_response(b"private token").unwrap_err();
        assert!(!error.contains("private token"));
    }

    #[gpui::test]
    fn onboarding_aliases_route_to_production_launcher(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w.push_test_panel("existing", cx);
            w
        });
        vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
        let mut before = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        for command in ["/onboarding-sim", "/onboarding-preview"] {
            vcx.simulate_input(command);
            vcx.simulate_keystrokes("enter");
            vcx.run_until_parked();
        }
        workspace.read_with(vcx, |w, _| {
            assert_eq!(
                w.onboarding_launch.requests,
                vec![launcher_path(), launcher_path()]
            );
        });
        let after = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        // Normal command submission adds local input history, but no session state changes.
        before.slots[0].panel.draft.history =
            vec!["/onboarding-sim".into(), "/onboarding-preview".into()];
        assert_eq!(before, after);
        assert!(commands.try_recv().is_err());
    }

    #[gpui::test]
    fn launcher_error_is_dismissible_without_changing_workspace(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("existing", cx);
            w
        });
        vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
        let before = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        workspace.update(vcx, |w, cx| {
            w.onboarding_launch.error = Some("Could not start fresh-profile Desktop".into());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("onboarding-launch-error").is_some());
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("onboarding-launch-error").is_none());
        let after = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        assert_eq!(before, after);
    }

    #[gpui::test]
    fn production_onboarding_routes_preserve_workspace(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w.push_test_panel("existing", cx);
            w
        });
        vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
        vcx.simulate_input("preserved draft");
        let before = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        vcx.simulate_keystrokes("alt-9");
        workspace.update(vcx, |w, cx| {
            let reply =
                w.handle_preview_request(crate::preview_control::Request::Onboarding {}, cx);
            assert_eq!(reply["flow"], "production");
            assert_eq!(reply["separate_window"], true);
            assert_eq!(
                w.onboarding_launch.requests,
                vec![launcher_path(), launcher_path()]
            );
        });
        let after = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        assert_eq!(before, after);
        assert!(commands.try_recv().is_err());

        // Legacy fake-simulator state is an ignored field, never restored as UI.
        let mut old: serde_json::Value = serde_json::from_slice(&before.encode().unwrap()).unwrap();
        old["onboarding_simulator"] = serde_json::json!({"step":"Account","connected":true});
        assert_eq!(
            WorkspaceSnapshot::decode(&serde_json::to_vec(&old).unwrap()).unwrap(),
            before
        );
    }
}
