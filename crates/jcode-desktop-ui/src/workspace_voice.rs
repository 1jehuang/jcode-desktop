//! Copilot is a physical key, not a repeatable editing command.
use super::*;

#[derive(Default)]
pub(super) struct CopilotLatch {
    down: bool,
}

fn is_copilot(stroke: &gpui::Keystroke) -> bool {
    let modifiers = stroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.function {
        return false;
    }
    match stroke.key.as_str() {
        "f23" => modifiers.platform && modifiers.shift,
        "xf86assistant" => {
            (modifiers.platform && modifiers.shift) || (!modifiers.platform && !modifiers.shift)
        }
        _ => false,
    }
}

impl CopilotLatch {
    /// Return Some for a consumed Copilot event, true only on the first down.
    /// X11 in our pinned GPUI reports repeats with is_held=false, so the latch
    /// is necessary even though Wayland supplies a reliable is_held flag.
    fn press(&mut self, event: &gpui::KeyDownEvent) -> Option<bool> {
        if !is_copilot(&event.keystroke) {
            return None;
        }
        let first = !self.down && !event.is_held;
        self.down = true;
        Some(first)
    }

    fn release(&mut self, key: &str) -> bool {
        // XKB can resolve the release to level 1 if the hardware releases its
        // synthetic modifiers first. Never bind TouchpadOff as a press.
        if self.down && matches!(key, "f23" | "xf86assistant" | "xf86touchpadoff") {
            self.down = false;
            true
        } else {
            false
        }
    }
}

impl Workspace {
    /// Alternate full-window surfaces must still let an explicit key stop an
    /// existing recording. toggle_voice refuses a new recording under a modal.
    pub(super) fn voice_modal_root(
        &self,
        content: impl IntoElement,
        cx: &Context<Self>,
    ) -> gpui::Div {
        div()
            .size_full()
            .capture_action(cx.listener(Self::toggle_voice))
            .capture_action(cx.listener(Self::toggle_panel_voice))
            .capture_key_down(cx.listener(Self::copilot_key_down))
            .capture_key_up(cx.listener(Self::copilot_key_up))
            .child(content)
    }

    pub(super) fn voice_target(&self, cx: &App) -> Option<usize> {
        let eligible = |index: usize| {
            self.slots
                .get(index)
                .is_some_and(|slot| !slot.closing && slot.panel.read(cx).supports_voice())
        };
        // Stopping/canceling always belongs to the original recording owner,
        // even after navigating to another conversation or utility surface.
        self.slots
            .iter()
            .position(|slot| slot.panel.read(cx).voice_active())
            .or_else(|| eligible(self.active).then_some(self.active))
            .or_else(|| {
                self.last_voice_chat.and_then(|id| {
                    self.slots
                        .iter()
                        .position(|slot| slot.panel.entity_id() == id)
                        .filter(|&index| eligible(index))
                })
            })
            .or_else(|| (0..self.slots.len()).find(|&index| eligible(index)))
    }

    pub(super) fn toggle_voice(
        &mut self,
        _: &ToggleVoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(index) = self.voice_target(cx) else {
            return;
        };
        let panel = self.slots[index].panel.clone();
        if !panel.read(cx).voice_active()
            && (self.show_beta_notice
                || self.account_sign_in.visible
                || self.onboarding_simulator.is_some())
        {
            return;
        }
        self.set_active(index, cx);
        if self.overview {
            self.overview = false;
            self.overview_progress.set(0.0, Instant::now());
        }
        self.focus_active(window, cx);
        panel.update(cx, |panel, cx| panel.toggle_voice(cx));
        cx.notify();
    }

    pub(super) fn toggle_panel_voice(
        &mut self,
        _: &crate::panel::voice::ToggleVoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_voice(&ToggleVoice, window, cx);
    }

    pub(super) fn copilot_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(first) = self.voice_key.press(event) {
            cx.stop_propagation();
            if first {
                self.toggle_voice(&ToggleVoice, window, cx);
            }
        }
    }

    pub(super) fn copilot_key_up(
        &mut self,
        event: &gpui::KeyUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.voice_key.release(&event.keystroke.key) {
            cx.stop_propagation();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disable_test_microphones(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
        for slot in &mut workspace.slots {
            slot.panel =
                cx.new(|cx| Panel::new_preview(crate::preview_state::PreviewState::Empty, cx));
        }
    }

    fn down(chord: &str, is_held: bool) -> gpui::KeyDownEvent {
        gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse(chord).unwrap(),
            is_held,
            prefer_character_input: false,
        }
    }

    #[test]
    fn linux_copilot_normalized_chords_are_exact() {
        for chord in [
            "super-shift-f23",
            "super-shift-xf86assistant",
            "xf86assistant",
        ] {
            assert!(
                is_copilot(&gpui::Keystroke::parse(chord).unwrap()),
                "{chord}"
            );
        }
        for chord in [
            "f23",
            "super-f23",
            "shift-f23",
            "xf86touchpadoff",
            "super-shift-xf86touchpadoff",
            "ctrl-super-shift-f23",
            "alt-super-shift-f23",
            "super-xf86assistant",
            "a",
        ] {
            assert!(
                !is_copilot(&gpui::Keystroke::parse(chord).unwrap()),
                "{chord}"
            );
        }
    }

    #[test]
    fn one_toggle_per_press_on_wayland_and_x11() {
        for chord in [
            "super-shift-f23",
            "super-shift-xf86assistant",
            "xf86assistant",
        ] {
            let mut latch = CopilotLatch::default();
            for _ in 0..3 {
                assert_eq!(latch.press(&down(chord, false)), Some(true));
                for held in [true, false, true, false] {
                    assert_eq!(latch.press(&down(chord, held)), Some(false));
                }
                assert!(!latch.release("shift"));
                assert!(latch.release("xf86touchpadoff"));
            }
        }
    }

    #[test]
    fn an_orphan_repeat_never_starts_recording() {
        let mut latch = CopilotLatch::default();
        assert_eq!(latch.press(&down("super-shift-f23", true)), Some(false));
        assert_eq!(latch.press(&down("super-shift-f23", false)), Some(false));
        assert!(latch.release("f23"));
        assert_eq!(latch.press(&down("super-shift-f23", false)), Some(true));
        assert_eq!(latch.press(&down("a", false)), None);
    }

    #[gpui::test]
    fn native_copilot_events_cancel_owner_once_until_release(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("recording-owner", cx);
            workspace.push_test_panel("other-chat", cx);
            disable_test_microphones(&mut workspace, cx);
            workspace
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.slots[0]
                    .panel
                    .update(cx, |panel, _| panel.set_voice_checking_for_test());
                workspace.set_active(1, cx);
                workspace.focus_active(window, cx);
                assert_eq!(workspace.voice_target(cx), Some(0));
            });
        });
        vcx.run_until_parked();
        vcx.simulate_event(down("super-shift-f23", false));
        workspace.read_with(vcx, |workspace, cx| {
            assert_eq!(workspace.active, 0);
            assert!(!workspace.slots[0].panel.read(cx).voice_active());
            assert!(!workspace.slots[1].panel.read(cx).voice_active());
        });
        for held in [true, false, true, false] {
            workspace.update(vcx, |workspace, cx| {
                workspace.slots[0]
                    .panel
                    .update(cx, |panel, _| panel.set_voice_checking_for_test());
            });
            vcx.simulate_event(down("super-shift-f23", held));
            workspace.read_with(vcx, |workspace, cx| {
                assert!(
                    workspace.slots[0].panel.read(cx).voice_active(),
                    "repeat must not cancel"
                );
            });
        }
        vcx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("f23").unwrap(),
        });
        vcx.simulate_event(down("super-shift-f23", false));
        workspace.read_with(vcx, |workspace, cx| {
            assert!(!workspace.slots[0].panel.read(cx).voice_active());
        });
    }

    #[gpui::test]
    fn host_named_action_reaches_voice_owner_without_composer_focus(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("recording-owner", cx);
            disable_test_microphones(&mut workspace, cx);
            workspace
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.slots[0]
                    .panel
                    .update(cx, |panel, _| panel.set_voice_checking_for_test());
            });
            window.blur();
            let action = cx.build_action("workspace::ToggleVoice", None).unwrap();
            // Match the stable host's fallback when no rendered focus target
            // has this action. The root workspace is a real tab stop.
            assert!(!window.is_action_available(action.as_ref(), cx));
            window.focus_next(cx);
            assert!(window.is_action_available(action.as_ref(), cx));
            window.dispatch_action(action, cx);
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, cx| {
            assert!(!workspace.slots[0].panel.read(cx).voice_active());
        });
    }

    #[gpui::test]
    fn account_modal_allows_stopping_but_never_starting_voice(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("recording-owner", cx);
            disable_test_microphones(&mut workspace, cx);
            workspace.account_sign_in.visible = true;
            workspace
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.slots[0]
                    .panel
                    .update(cx, |panel, _| panel.set_voice_checking_for_test());
                window.focus(&workspace.focus_handle, cx);
            })
        });
        vcx.run_until_parked();
        for _ in 0..2 {
            vcx.update(|window, cx| {
                let action = cx.build_action("workspace::ToggleVoice", None).unwrap();
                assert!(window.is_action_available(action.as_ref(), cx));
                window.dispatch_action(action, cx);
            });
            vcx.run_until_parked();
            workspace.read_with(vcx, |workspace, cx| {
                assert!(workspace.account_sign_in.visible);
                assert!(!workspace.slots[0].panel.read(cx).voice_active());
            });
        }
    }

    #[gpui::test]
    fn keyboard_and_button_stop_existing_owner_not_focused_chat(cx: &mut gpui::TestAppContext) {
        cx.update(crate::panel::voice::bind_keys);
        for button in [false, true] {
            let (workspace, vcx) = cx.add_window_view(|_, cx| {
                let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
                workspace.push_test_panel("owner", cx);
                workspace.push_test_panel("other", cx);
                disable_test_microphones(&mut workspace, cx);
                // Only B's row is mounted when pressing its button. Each input
                // path gets a fresh window, without a preceding row animation.
                workspace.slots[1].row = 1;
                workspace.set_active(1, cx);
                workspace
            });
            vcx.update(|window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.slots[0]
                        .panel
                        .update(cx, |panel, _| panel.set_voice_checking_for_test());
                    workspace.set_active(1, cx);
                    workspace.focus_active(window, cx);
                })
            });
            vcx.run_until_parked();
            if button {
                let bounds = vcx
                    .debug_bounds("voice-toggle")
                    .expect("focused chat button");
                vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
            } else {
                vcx.simulate_keystrokes("ctrl-shift-v");
            }
            vcx.run_until_parked();
            workspace.read_with(vcx, |workspace, cx| {
                assert_eq!(workspace.active, 0, "button={button}");
                assert!(
                    !workspace.slots[0].panel.read(cx).voice_active(),
                    "button={button}"
                );
                assert!(
                    !workspace.slots[1].panel.read(cx).voice_active(),
                    "button={button}"
                );
            });
        }
    }

    #[gpui::test]
    fn utility_navigation_remembers_last_chat_without_starting_audio(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("first-chat", cx);
            workspace.push_test_panel("second-chat", cx);
            workspace.push_test_panel("terminal://test", cx);
            workspace.push_test_panel("settings://machines", cx);
            workspace
        });
        workspace.update(vcx, |workspace, cx| {
            workspace.set_active(1, cx);
            assert_eq!(workspace.voice_target(cx), Some(1));
            workspace.set_active(2, cx);
            workspace.set_active(3, cx);
            assert_eq!(workspace.voice_target(cx), Some(1));
            workspace.slots[1].closing = true;
            assert_eq!(workspace.voice_target(cx), Some(0));
            workspace.slots[0].closing = true;
            assert_eq!(workspace.voice_target(cx), None);
        });
    }
}
