//! Standalone windows reuse the chat and its utilities, never the spatial canvas.
//! New utility panels open in separate native windows. The back control only
//! handles legacy/restored layouts that already contain additional surfaces.
use super::*;

impl Workspace {
    pub(super) fn single_panel_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active == 0 {
            return;
        }
        let panel = self.slots[self.active].panel.clone();
        if panel.read(cx).is_accounts_panel() {
            let source = self.slots[0].panel.clone();
            self.close_accounts(&panel, &source, false, window, cx);
        } else {
            self.close_panel(&ClosePanel, window, cx);
        }
        self.set_active(0, cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(super) fn render_single_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // Layout preferences and hot-reload snapshots cannot turn this window
        // back into a workspace. No strip router is mounted, so touchpad motion
        // belongs entirely to the transcript and its controls.
        self.show_sidebar = false;
        self.show_minimap = false;
        self.overview = false;
        self.hints_overlay = false;
        self.active_row = 0;
        self.outgoing_row = None;
        self.camera_x.fill(0.0);
        self.camera_target.fill(0.0);
        self.camera_started.fill(None);
        self.last_canvas_width = Some(f32::from(window.viewport_size().width));
        for slot in &mut self.slots {
            slot.row = 0;
            slot.width_fraction = 1.0;
            slot.animated_width = AnimatedValue::new(1.0, Duration::ZERO);
            slot.order_offset = AnimatedValue::new(0.0, Duration::ZERO);
            if slot.closing {
                slot.close_progress = AnimatedValue::new(0.0, Duration::ZERO);
            }
        }
        self.remove_finished_closing_panels(Instant::now(), window, cx);
        if self.focus_pending {
            self.focus_pending = false;
            self.focus_active(window, cx);
        }
        let content = self.slots.get(self.active).map(|slot| {
            if slot.panel.read(cx).is_pending_session() {
                self.render_pending_session(self.active, cx)
            } else if slot.panel.read(cx).is_default_directory() {
                self.render_folder_picker(cx)
            } else {
                slot.panel
                    .clone()
                    .cached(gpui::StyleRefinement::default().size_full())
                    .into_any_element()
            }
        });
        let back = self.active != 0;
        let inset = content_top_inset(false, window.is_fullscreen());
        let root = div()
            .debug_selector(|| "single-panel-root".into())
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .pt(px(inset))
            .bg(Theme::global().PANEL_BG)
            .font_family(Theme::global().FONT_UI)
            .text_size(px(14.0 * crate::config::get().appearance.text_scale))
            .text_color(Theme::global().TEXT)
            .track_focus(&self.focus_handle)
            .capture_action(cx.listener(Self::toggle_onboarding_simulator))
            .capture_action(cx.listener(Self::toggle_voice))
            .capture_action(cx.listener(Self::toggle_panel_voice))
            .capture_action(cx.listener(Self::begin_voice_hold))
            .capture_action(cx.listener(Self::end_voice_hold))
            .capture_key_down(cx.listener(Self::copilot_key_down))
            .capture_key_up(cx.listener(Self::copilot_key_up))
            .capture_action(cx.listener(Self::rename_session))
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if this.rename_editor.is_some() && event.keystroke.key == "escape" {
                    this.close_rename_editor(cx);
                    cx.stop_propagation();
                }
            }))
            // Deliberately omit new/move/resize/overview/sidebar/workspace
            // actions. The host still handles Ctrl+R, and the panel owns chat,
            // model, login, clipboard, image and transcript shortcuts.
            .on_action(cx.listener(Self::close_panel))
            .on_action(cx.listener(Self::cycle_theme))
            .on_action(cx.listener(Self::open_accounts))
            .on_action(cx.listener(Self::open_model_window_or_picker))
            .on_action(cx.listener(Self::open_changelog))
            .on_action(cx.listener(Self::open_resume))
            .on_action(cx.listener(Self::open_change_review))
            .on_action(cx.listener(Self::close_change_review))
            .when(back, |root| {
                root.child(
                    div()
                        .id("single-panel-back")
                        .debug_selector(|| "single-panel-back".into())
                        .flex_none()
                        .px_3()
                        .py_2()
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .hover(|el| el.bg(Theme::global().TOOL_BG))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.single_panel_back(window, cx);
                        }))
                        .child("‹ Back to chat"),
                )
            })
            .child(
                div()
                    .debug_selector(|| "single-panel-surface".into())
                    .relative()
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .children(content),
            )
            .when(self.rename_editor.is_some(), |root| {
                root.child(self.render_rename_editor(cx))
            })
            .when_some(self.render_update_chip(cx), |root, chip| root.child(chip));
        if Theme::is_transitioning() {
            cx.refresh_windows();
            window.request_animation_frame();
        }
        self.dump_state(window, cx);
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standalone(cx: &mut Context<Workspace>) -> Workspace {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        workspace.single_panel = true;
        workspace.open_startup_draft(cx);
        workspace
    }

    #[gpui::test]
    fn single_panel_voice_hold_press_and_release_reach_focused_chat(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = standalone(cx);
            workspace.slots[0].panel = cx.new(|cx| Panel::new_preview(crate::preview_state::PreviewState::Empty, cx));
            workspace
        });
        vcx.update(|window, cx| workspace.update(cx, |workspace, cx| {
            workspace.focus_active(window, cx);
        }));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("single-panel-root").is_some());
        vcx.simulate_event(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("super-shift-xf86assistant").unwrap(),
            is_held: false,
            prefer_character_input: false,
        });
        workspace.update(vcx, |workspace, cx| {
            assert!(workspace.voice_key.is_down());
            workspace.slots[0].panel.update(cx, |panel, _| panel.set_voice_hold_checking_for_test());
        });
        vcx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("xf86touchpadoff").unwrap(),
        });
        workspace.read_with(vcx, |workspace, cx| {
            assert!(!workspace.voice_key.is_down());
            assert!(!workspace.slots[0].panel.read(cx).voice_active());
            assert_eq!(workspace.slots.len(), 1);
        });
    }

    #[gpui::test]
    fn single_panel_fills_window_without_workspace_chrome(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| standalone(cx));
        vcx.run_until_parked();
        let root = vcx.debug_bounds("single-panel-root").unwrap();
        let surface = vcx.debug_bounds("single-panel-surface").unwrap();
        assert_eq!(root.size.width, surface.size.width);
        assert_eq!(root.bottom(), surface.bottom());
        if !cfg!(target_os = "macos") {
            assert_eq!(root, surface);
        }
        for selector in [
            "workspace-body",
            "workspace-canvas",
            "sidebar",
            "empty-strip-hint",
            "single-panel-back",
        ] {
            assert!(
                vcx.debug_bounds(selector).is_none(),
                "{selector} must not be mounted"
            );
        }
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.slots.len(), 1);
            assert!(!w.show_sidebar);
            assert_eq!(w.slots[0].width_fraction, 1.0);
        });
    }

    #[gpui::test]
    fn single_panel_workspace_shortcuts_and_fallback_cannot_escape(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let w = standalone(cx);
            w.focus_active(window, cx);
            w
        });
        vcx.run_until_parked();
        vcx.simulate_input("keep my draft");
        for detached in [false, true] {
            if detached {
                vcx.update(|window, _| window.blur());
            }
            for chord in [
                "super-j",
                "super-k",
                "super-h",
                "super-l",
                "super-enter",
                "super-t",
                "super-o",
                "super-b",
                "super-1",
                "super-shift-j",
            ] {
                vcx.simulate_keystrokes(chord);
                vcx.run_until_parked();
                workspace.read_with(vcx, |w, cx| {
                    assert_eq!(w.slots.len(), 1, "{chord}");
                    assert_eq!(w.active_row, 0, "{chord}");
                    assert_eq!(w.slots[0].width_fraction, 1.0, "{chord}");
                    assert!(!w.overview && !w.show_sidebar, "{chord}");
                    assert_eq!(
                        w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                        "keep my draft"
                    );
                });
            }
        }
    }

    #[gpui::test]
    fn single_panel_reload_preserves_draft_and_presentation(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let w = standalone(cx);
            w.focus_active(window, cx);
            w
        });
        vcx.run_until_parked();
        vcx.simulate_input("survive the reload");
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                let snapshot = w.snapshot(window, cx).unwrap();
                w.apply_snapshot(snapshot, cx);
                w.restore_focus(window, cx);
                cx.notify();
            })
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("single-panel-root").is_some());
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(
                w.slots[0].panel.read(cx).input.read(cx).content.as_ref(),
                "survive the reload"
            );
        });
    }

    #[derive(Clone, Copy)]
    enum Utility {
        Accounts,
        Review,
        Changelog,
    }

    fn open_utility(
        utility: Utility,
        workspace: &Entity<Workspace>,
        source: &Entity<Panel>,
        vcx: &mut gpui::VisualTestContext,
    ) {
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| match utility {
                Utility::Accounts => w.open_accounts(
                    &OpenAccounts {
                        source: source.entity_id(),
                        login_command: None,
                    },
                    window,
                    cx,
                ),
                Utility::Review => w.open_change_review(
                    &change_review::OpenChangeReview {
                        source: source.entity_id(),
                        output: String::new(),
                        name: "write".into(),
                        input: serde_json::json!({"file_path":"example.rs","content":"hello\n"})
                            .to_string(),
                        selected: 0,
                        done: true,
                        failed: false,
                    },
                    window,
                    cx,
                ),
                Utility::Changelog => w.open_changelog(&OpenChangelog, window, cx),
            })
        });
        vcx.run_until_parked();
    }

    fn assert_separate_utility_window(utility: Utility, cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = standalone(cx);
            workspace.set_test_bridge(bridge);
            workspace.focus_active(window, cx);
            workspace
        });
        vcx.run_until_parked();
        vcx.simulate_input("do not lose my draft");
        let source_window = vcx.update(|window, _| window.window_handle());
        let source = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
        let before = source.read_with(vcx, |panel, cx| panel.snapshot(cx));
        let state = workspace.read_with(vcx, |w, _| {
            (w.active, w.active_row, w.previous, w.slots[0].width_fraction)
        });
        open_utility(utility, &workspace, &source, vcx);
        let child = vcx.update(|_, cx| {
            let windows = cx.windows();
            assert_eq!(windows.len(), 2, "utility must open a separate window");
            windows.into_iter().find(|handle| *handle != source_window).unwrap()
        });
        let hosted = vcx.update(|_, cx| {
            let root = child.downcast::<panel_window::PanelWindow>().unwrap();
            let panel = root.read(cx).unwrap().panel.clone();
            assert!(match utility {
                Utility::Accounts => panel.read(cx).is_accounts_panel(),
                Utility::Review => panel.read(cx).is_change_review(),
                Utility::Changelog => panel.read(cx).is_changelog(),
            });
            panel
        });
        open_utility(utility, &workspace, &source, vcx);
        vcx.update(|_, cx| {
            assert_eq!(cx.windows().len(), 2, "repeated open must reuse the child");
            assert!(cx.windows().contains(&child));
            let root = child.downcast::<panel_window::PanelWindow>().unwrap();
            assert_eq!(root.read(cx).unwrap().panel, hosted);
        });
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.slots[0].panel, source);
            assert_eq!(
                (w.active, w.active_row, w.previous, w.slots[0].width_fraction),
                state,
                "opening a utility must not alter source layout or selection"
            );
            let after = source.read(cx).snapshot(cx);
            assert_eq!(before.draft, after.draft);
            assert_eq!(before.scroll_y, after.scroll_y);
        });
        assert!(vcx.debug_bounds("single-panel-root").is_some());
        assert!(vcx.debug_bounds("single-panel-back").is_none());
        assert!(vcx.debug_bounds("panel-window-root").is_none());
        let mut child_cx = gpui::VisualTestContext::from_window(child, vcx);
        child_cx.run_until_parked();
        assert!(child_cx.debug_bounds("panel-window-root").is_some());
        assert!(child_cx.debug_bounds("workspace-canvas").is_none());
        assert!(child_cx.debug_bounds("single-panel-back").is_none());
        child_cx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            assert_eq!(cx.windows(), vec![source_window], "Escape closes only the child");
            assert!(source.read(cx).input_focus_handle(cx).is_focused(window));
        });
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.slots[w.active].panel, source);
            assert_eq!(source.read(cx).snapshot(cx).draft, before.draft);
        });
        // A stale child handle must not prevent reopening after close.
        open_utility(utility, &workspace, &source, vcx);
        let reopened = vcx.update(|_, cx| {
            let windows = cx.windows();
            assert_eq!(windows.len(), 2);
            windows.into_iter().find(|handle| *handle != source_window).unwrap()
        });
        let mut reopened_cx = gpui::VisualTestContext::from_window(reopened, vcx);
        reopened_cx.dispatch_action(ClosePanel);
        vcx.run_until_parked();
        vcx.update(|_, cx| assert_eq!(cx.windows(), vec![source_window]));
        assert!(commands.try_iter().all(|command| !matches!(command,
            Command::Watch { session_id } | Command::Unwatch { session_id }
                if session_id.starts_with("review://") || session_id == Panel::CHANGELOG_SESSION_ID
        )));
    }

    #[gpui::test]
    fn single_panel_accounts_reuses_child_and_preserves_source(cx: &mut gpui::TestAppContext) {
        assert_separate_utility_window(Utility::Accounts, cx);
    }

    #[gpui::test]
    fn single_panel_review_reuses_child_and_preserves_source(cx: &mut gpui::TestAppContext) {
        assert_separate_utility_window(Utility::Review, cx);
    }

    #[gpui::test]
    fn single_panel_changelog_reuses_child_and_preserves_source(cx: &mut gpui::TestAppContext) {
        assert_separate_utility_window(Utility::Changelog, cx);
    }
}
