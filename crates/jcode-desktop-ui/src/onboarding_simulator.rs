//! A local-only first-run rehearsal. No provider, filesystem, or runtime calls.
use super::*;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
enum Step {
    #[default]
    Welcome,
    Account,
    Folder,
    Ready,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(super) struct Simulation {
    step: Step,
    connected: bool,
    connection_error: bool,
    folder_selected: bool,
}

impl Workspace {
    pub(crate) fn toggle_onboarding_simulator(
        &mut self,
        _: &ToggleOnboardingSimulator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if self.onboarding_simulator.is_some() {
            self.close_onboarding_simulator(window, cx);
        } else {
            self.onboarding_simulator = Some(Simulation::default());
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    fn close_onboarding_simulator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.onboarding_simulator = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn advance_onboarding_simulator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sim) = &mut self.onboarding_simulator else {
            return;
        };
        match sim.step {
            Step::Welcome => sim.step = Step::Account,
            Step::Account => sim.step = Step::Folder,
            Step::Folder => sim.step = Step::Ready,
            Step::Ready => {
                self.close_onboarding_simulator(window, cx);
                return;
            }
        }
        cx.notify();
    }

    fn back_onboarding_simulator(&mut self, cx: &mut Context<Self>) {
        if let Some(sim) = &mut self.onboarding_simulator {
            sim.step = match sim.step {
                Step::Welcome | Step::Account => Step::Welcome,
                Step::Folder => Step::Account,
                Step::Ready => Step::Folder,
            };
            cx.notify();
        }
    }

    pub(super) fn render_onboarding_simulator(&self, cx: &mut Context<Self>) -> gpui::Div {
        let sim = self
            .onboarding_simulator
            .as_ref()
            .expect("simulator is open");
        let (number, title, description, next) = match sim.step {
            Step::Welcome => (
                1,
                "Welcome to Jcode Desktop",
                "A space for your code, conversations, and ideas. Rehearse the first-run experience here.",
                "Get started",
            ),
            Step::Account => (
                2,
                "Connect your AI",
                "Try a successful connection or a sign-in error. These are demo states, not real accounts.",
                if sim.connected {
                    "Continue"
                } else {
                    "Skip for now"
                },
            ),
            Step::Folder => (
                3,
                "Choose a project",
                "Give your first session a place to work. This demo folder is never opened or created.",
                if sim.folder_selected {
                    "Continue"
                } else {
                    "Skip for now"
                },
            ),
            Step::Ready => (
                4,
                "You're ready",
                "In the real workspace, write a prompt to begin. Use the Learn tab to explore panel navigation and shortcuts.",
                "Return to workspace",
            ),
        };
        let mut details = div().flex().flex_col().gap_3().min_h(px(120.0));
        match sim.step {
            Step::Welcome => {
                details = details
                    .child("1. Connect an AI account")
                    .child("2. Choose your project folder")
                    .child("3. Start your first conversation");
            }
            Step::Account => {
                details =
                    details
                        .child(
                            div()
                                .p_3()
                                .rounded_lg()
                                .bg(Theme::global().HEADER_BG)
                                .child(if sim.connected {
                                    "Demo account connected"
                                } else {
                                    "Demo AI account · Not connected"
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    sim_button(
                                        "onboarding-connect",
                                        if sim.connection_error {
                                            "Retry demo connection"
                                        } else {
                                            "Connect demo account"
                                        },
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            if let Some(sim) = &mut this.onboarding_simulator {
                                                sim.connected = true;
                                                sim.connection_error = false;
                                            }
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    sim_button("onboarding-error", "Simulate sign-in error")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(sim) = &mut this.onboarding_simulator {
                                                sim.connected = false;
                                                sim.connection_error = true;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when(sim.connection_error, |el| {
                            el.child(
                        div().debug_selector(|| "onboarding-connection-error".into())
                            .child("Demo sign-in failed. Retry the connection or skip for now.")
                    )
                        });
            }
            Step::Folder => {
                details = details
                    .child(
                        div()
                            .p_3()
                            .rounded_lg()
                            .bg(Theme::global().HEADER_BG)
                            .child(if sim.folder_selected {
                                "Selected: ~/Projects/hello-jcode (demo)"
                            } else {
                                "No demo project selected"
                            }),
                    )
                    .child(
                        sim_button("onboarding-folder", "Use demo project").on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(sim) = &mut this.onboarding_simulator {
                                    sim.folder_selected = true;
                                }
                                cx.notify();
                            },
                        )),
                    );
            }
            Step::Ready => {
                details = details
                    .child(if sim.connected {
                        "AI account: Demo connected"
                    } else {
                        "AI account: Skipped"
                    })
                    .child(if sim.folder_selected {
                        "Project: ~/Projects/hello-jcode (demo)"
                    } else {
                        "Project: Skipped"
                    })
                    .child("Your real workspace and settings are unchanged.");
            }
        }
        div().size_full().flex().flex_col().justify_center().items_center().p_6()
            .bg(Theme::global().BG).text_color(Theme::global().TEXT)
            .font_family(Theme::global().FONT_UI).text_size(px(14.0))
            .track_focus(&self.focus_handle)
            .capture_action(cx.listener(Self::toggle_onboarding_simulator))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                cx.stop_propagation();
                if event.is_held { return; }
                match event.keystroke.key.as_str() {
                    "escape" => this.close_onboarding_simulator(window, cx),
                    "enter" => this.advance_onboarding_simulator(window, cx),
                    "left" => this.back_onboarding_simulator(cx),
                    _ => {}
                }
            }))
            .child(div().id("onboarding-simulator").debug_selector(|| "onboarding-simulator".into())
                .w_full().max_w(px(600.0)).max_h_full().overflow_y_scroll()
                .p_6().rounded_xl().border_1().border_color(Theme::global().PANEL_BORDER)
                .bg(Theme::global().PANEL_BG).flex().flex_col().gap_5()
                .child(div().flex().justify_between().items_center().gap_3()
                    .child(div().text_size(px(11.0)).text_color(Theme::global().TEXT_DIM).child("ONBOARDING SIMULATOR"))
                    .child(sim_button("onboarding-exit", "Exit · Alt+9")
                        .on_click(cx.listener(|this, _, window, cx| this.close_onboarding_simulator(window, cx)))))
                .child(div().text_size(px(12.0)).text_color(Theme::global().TEXT_DIM).child(format!("Step {number} of 4 · Preview only")))
                .child(div().text_size(px(28.0)).child(title))
                .child(description)
                .child(details)
                .child(div().text_size(px(12.0)).text_color(Theme::global().TEXT_DIM)
                    .child("No credentials, settings, or sessions are changed. Esc exits at any time."))
                .child(div().flex().flex_wrap().justify_between().gap_2()
                    .child(div().flex().gap_2()
                        .when(sim.step != Step::Welcome, |el| el.child(sim_button("onboarding-back", "Back")
                            .on_click(cx.listener(|this, _, _, cx| this.back_onboarding_simulator(cx)))))
                        .child(sim_button("onboarding-restart", "Restart")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.onboarding_simulator = Some(Simulation::default());
                                cx.notify();
                            }))))
                    .child(sim_button("onboarding-next", next)
                        .on_click(cx.listener(|this, _, window, cx| this.advance_onboarding_simulator(window, cx)))))
            )
    }
}

fn sim_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(Theme::global().PANEL_BORDER)
        .cursor_pointer()
        .hover(|el| el.bg(Theme::global().HEADER_BG))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(cx: &mut gpui::VisualTestContext, id: &'static str) {
        let bounds = cx
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("missing {id}"));
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn onboarding_simulator_shortcut_walkthrough_is_sandboxed(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w.push_test_panel("existing", cx);
            w
        });
        vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
        vcx.simulate_input("keep my draft");
        let before = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        vcx.simulate_keystrokes("alt-9");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("onboarding-simulator").is_some());
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.focus_active(window, cx);
                assert!(w.focus_handle.is_focused(window));
            })
        });
        // Real navigation/creation shortcuts cannot mutate the hidden workspace.
        vcx.simulate_keystrokes("super-n super-q super-j");
        click(vcx, "onboarding-next");
        click(vcx, "onboarding-error");
        assert!(vcx.debug_bounds("onboarding-connection-error").is_some());
        click(vcx, "onboarding-connect");
        assert!(vcx.debug_bounds("onboarding-connection-error").is_none());
        click(vcx, "onboarding-next");
        click(vcx, "onboarding-folder");
        click(vcx, "onboarding-next");
        workspace.read_with(vcx, |w, _| {
            let sim = w.onboarding_simulator.as_ref().unwrap();
            assert_eq!(sim.step, Step::Ready);
            assert!(sim.connected && sim.folder_selected);
        });
        click(vcx, "onboarding-back");
        click(vcx, "onboarding-restart");
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.onboarding_simulator, Some(Simulation::default()))
        });
        vcx.simulate_keystrokes("enter enter enter enter");
        vcx.run_until_parked();
        let after = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        assert_eq!(
            before, after,
            "simulation must preserve drafts and workspace state"
        );
        vcx.simulate_keystrokes("alt-9 alt-9");
        workspace.read_with(vcx, |w, _| assert!(w.onboarding_simulator.is_none()));
        vcx.simulate_keystrokes("alt-9 escape");
        workspace.read_with(vcx, |w, _| assert!(w.onboarding_simulator.is_none()));
        assert!(
            commands.try_recv().is_err(),
            "simulation must never call the runtime"
        );
    }

    #[gpui::test]
    fn onboarding_simulator_command_and_reload(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("existing", cx);
            w
        });
        vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
        vcx.simulate_input("/onboarding-sim");
        vcx.simulate_keystrokes("enter enter");
        vcx.run_until_parked();
        click(vcx, "onboarding-connect");
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                let snapshot = w.snapshot(window, cx).unwrap();
                let snapshot = WorkspaceSnapshot::decode(&snapshot.encode().unwrap()).unwrap();
                let state = snapshot.onboarding_simulator.clone();
                w.apply_snapshot(snapshot, cx);
                w.restore_focus(window, cx);
                assert_eq!(w.onboarding_simulator, state);
                assert!(w.focus_handle.is_focused(window));
            })
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("onboarding-simulator").is_some());
        vcx.simulate_keystrokes("escape");
        assert!(workspace.read_with(vcx, |w, _| w.onboarding_simulator.is_none()));
    }
}
