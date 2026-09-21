//! A local, read-only update panel. Never register it as a runtime session.
use super::*;

impl Workspace {
    pub(crate) fn open_changelog(
        &mut self,
        _: &OpenChangelog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_id = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.entity_id());
        let existing = self
            .slots
            .iter()
            .position(|slot| !slot.closing && slot.panel.read(cx).is_changelog());
        let index = existing.unwrap_or_else(|| {
            let panel = cx.new(|cx| {
                Panel::new(
                    Panel::CHANGELOG_SESSION_ID.into(),
                    Some("Desktop changelog".into()),
                    None,
                    self.bridge.clone(),
                    cx,
                )
            });
            let index = 0;
            let width = DEFAULT_WIDTH;
            self.slots.insert(
                index,
                Slot {
                    panel,
                    row: self.active_row,
                    width_fraction: width,
                    animated_width: AnimatedValue::new(
                        width,
                        transition::policy(Transition::PanelOpen).duration,
                    ),
                    order_offset: AnimatedValue::new(
                        0.0,
                        transition::policy(Transition::PanelOrder).duration,
                    ),
                    order_distance_fraction: width,
                    close_progress: AnimatedValue::new(
                        1.0,
                        transition::policy(Transition::PanelClose).duration,
                    ),
                    closing: false,
                    restore_fraction: None,
                },
            );
            index
        });
        // Notes always lead their row, including when an existing notes panel
        // was moved. Keep the outgoing chat's identity across the reordering.
        if index != 0 {
            let slot = self.slots.remove(index);
            self.slots.insert(0, slot);
        }
        if let Some(index) = active_id.and_then(|id| {
            self.slots
                .iter()
                .position(|slot| slot.panel.entity_id() == id)
        }) {
            self.active = index;
        }
        let row = self.slots[0].row;
        if let Some(chat) = self
            .slots
            .iter_mut()
            .find(|slot| slot.row == row && !slot.closing && slot.panel.read(cx).supports_voice())
        {
            chat.width_fraction = DEFAULT_WIDTH;
            chat.animated_width.set(DEFAULT_WIDTH, Instant::now());
            chat.restore_fraction = None;
        }
        // Explicit updates should surface their notes even from overview mode.
        self.overview = false;
        self.overview_progress.set(0.0, Instant::now());
        self.hints_overlay = false;
        self.set_active(0, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn changelog_can_open_without_a_chat(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.open_changelog(&OpenChangelog, window, cx);
                w.open_changelog(&OpenChangelog, window, cx);
                assert_eq!(w.slots.len(), 1);
                assert_eq!(w.active, 0);
                assert!(w.slots[0].panel.read(cx).is_changelog());
                assert!(w.snapshot(window, cx).unwrap().slots.is_empty());
            })
        });
    }

    #[gpui::test]
    fn changelog_leads_row_and_halves_only_first_live_chat(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("other-row", cx);
            w.slots[0].width_fraction = 1.0;
            w.push_test_panel("closing-chat", cx);
            w.slots[1].row = 1;
            w.slots[1].closing = true;
            w.push_test_panel("accounts://first", cx);
            w.slots[2].row = 1;
            w.push_test_panel("first-chat", cx);
            w.slots[3].row = 1;
            w.slots[3].width_fraction = 1.0;
            w.slots[3].animated_width = AnimatedValue::new(1.0, Duration::ZERO);
            w.slots[3].restore_fraction = Some(0.75);
            w.push_test_panel("focused-chat", cx);
            w.slots[4].row = 1;
            w.slots[4].width_fraction = 0.75;
            w.active = 4;
            w.active_row = 1;
            w
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                let source = w.slots[4].panel.entity_id();
                w.open_changelog(&OpenChangelog, window, cx);
                assert_eq!(w.active, 0);
                assert!(w.slots[0].panel.read(cx).is_changelog());
                assert_eq!(w.slots[0].row, 1);
                assert_eq!(w.slots[0].width_fraction, 0.5);
                assert_eq!(w.slots[1].width_fraction, 1.0);
                assert_eq!(w.slots[4].panel.read(cx).session_id, "first-chat");
                assert_eq!(w.slots[4].width_fraction, 0.5);
                assert_eq!(w.slots[4].animated_width.sample(Instant::now()), 0.5);
                assert_eq!(w.slots[4].restore_fraction, None);
                assert_eq!(w.slots[5].width_fraction, 0.75);
                assert_eq!(w.previous, Some(source));

                // Reopening an existing panel brings it back to the left edge,
                // without duplicating it or losing the focused chat identity.
                let notes = w.slots.remove(0);
                w.slots.push(notes);
                w.active = 4;
                w.open_changelog(&OpenChangelog, window, cx);
                assert_eq!(w.slots.len(), 6);
                assert!(w.slots[0].panel.read(cx).is_changelog());
                assert_eq!(w.previous, Some(source));
            })
        });
    }

    #[gpui::test]
    fn changelog_opens_once_is_read_only_and_never_reaches_runtime(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("existing-session", cx);
            workspace.slots[0].width_fraction = 1.0;
            workspace
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.open_changelog(&OpenChangelog, window, cx);
                w.open_changelog(&OpenChangelog, window, cx);
                assert_eq!(w.slots.len(), 2);
                assert_eq!(w.active, 0);
                assert_eq!(w.slots[1].width_fraction, 0.5);
                assert!(w.slots[w.active].panel.read(cx).is_changelog());
                assert!(!w.slots[w.active].panel.read(cx).can_fork());
                assert!(
                    w.slots[w.active]
                        .panel
                        .read(cx)
                        .input_focus_handle(cx)
                        .is_focused(window)
                );
                let snapshot = w.snapshot(window, cx).unwrap();
                assert_eq!(snapshot.slots.len(), 1);
                assert_eq!(snapshot.slots[0].panel.session_id, "existing-session");
            })
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-changelog").is_some());
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(
                w.slots[w.active].panel.read(cx).session_id,
                "existing-session"
            );
        });
        assert!(
            commands.try_recv().is_err(),
            "local notes must never watch, send, or unwatch a runtime session"
        );
    }

    #[gpui::test]
    fn changelog_reload_preserves_underlying_focus_draft_and_singleton(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("first", cx);
            w.push_test_panel("draft-session", cx);
            w.active = 1;
            w
        });
        vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
        vcx.simulate_input("keep this draft");
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                w.open_changelog(&OpenChangelog, window, cx);
                let snapshot = w.snapshot(window, cx).unwrap();
                assert_eq!(snapshot.active, 1);
                assert_eq!(snapshot.slots.len(), 2);
                let original_draft = snapshot.slots[1].panel.draft.clone();
                w.apply_snapshot(snapshot, cx);
                w.restore_focus(window, cx);
                w.open_changelog(&OpenChangelog, window, cx);
                w.open_changelog(&OpenChangelog, window, cx);
                assert_eq!(w.slots.len(), 3);
                let after = w.snapshot(window, cx).unwrap();
                assert_eq!(after.active, 1);
                assert_eq!(
                    serde_json::to_value(&after.slots[1].panel.draft).unwrap(),
                    serde_json::to_value(&original_draft).unwrap()
                );
            })
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "draft-session");
        });
    }

    #[gpui::test]
    fn changelog_slash_command_opens_local_panel_without_sending(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w.push_test_panel("source", cx);
            w
        });
        vcx.update(|window, cx| workspace.update(cx, |w, cx| w.focus_active(window, cx)));
        vcx.simulate_input("/changelog");
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("desktop-changelog").is_some());
        assert!(commands.try_recv().is_err());
    }
}
