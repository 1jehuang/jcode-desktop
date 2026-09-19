//! Agent-managed documents are native workspace panels, not transcript messages.
use super::*;
use jcode_sdk::SidePanelSnapshot;

impl Workspace {
    pub(super) fn apply_side_panel(
        &mut self,
        owner: &str,
        snapshot: &SidePanelSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(source) = self
            .slots
            .iter()
            .find(|slot| !slot.closing && slot.panel.read(cx).session_id == owner)
        else {
            // Late events must not create orphan panels after their session closes.
            return false;
        };
        let row = source.row;
        let source_id = source.panel.entity_id();
        let active_id = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.entity_id());
        let previous = self
            .side_panel_snapshots
            .insert(owner.to_owned(), snapshot.clone());
        let focus_changed = previous
            .as_ref()
            .is_none_or(|old| old.focused_page_id != snapshot.focused_page_id || old == snapshot);

        // Deleting a tool page removes only that owner's document, never the chat.
        self.slots.retain(|slot| {
            let panel = slot.panel.read(cx);
            panel.side_document_owner() != Some(owner)
                || snapshot
                    .pages
                    .iter()
                    .any(|page| panel.side_document_page_id() == Some(page.id.as_str()))
        });
        let mut newly_opened = HashSet::new();
        let mut seen = HashSet::new();
        for page in &snapshot.pages {
            if !seen.insert(&page.id) {
                continue;
            }
            if let Some(slot) = self.slots.iter_mut().find(|slot| {
                let panel = slot.panel.read(cx);
                panel.side_document_owner() == Some(owner)
                    && panel.side_document_page_id() == Some(page.id.as_str())
            }) {
                slot.panel
                    .update(cx, |panel, cx| panel.update_side_document(page, cx));
                if slot.closing {
                    slot.closing = false;
                    slot.close_progress.set(1.0, Instant::now());
                    newly_opened.insert(page.id.clone());
                }
                continue;
            }
            let source_index = self
                .slots
                .iter()
                .position(|slot| slot.panel.entity_id() == source_id)
                .expect("source retained");
            let insert_at = self
                .slots
                .iter()
                .enumerate()
                .skip(source_index + 1)
                .take_while(|(_, slot)| slot.panel.read(cx).side_document_owner() == Some(owner))
                .last()
                .map_or(source_index + 1, |(index, _)| index + 1);
            let width_fraction = DEFAULT_WIDTH;
            let panel = cx.new(|cx| Panel::new_side_document(owner, page, self.bridge.clone(), cx));
            self.slots.insert(
                insert_at,
                Slot {
                    panel,
                    row,
                    width_fraction,
                    animated_width: AnimatedValue::new(
                        width_fraction,
                        transition::policy(Transition::PanelOpen).duration,
                    ),
                    order_offset: AnimatedValue::new(
                        0.0,
                        transition::policy(Transition::PanelOrder).duration,
                    ),
                    order_distance_fraction: width_fraction,
                    close_progress: AnimatedValue::new(
                        1.0,
                        transition::policy(Transition::PanelClose).duration,
                    ),
                    closing: false,
                    restore_fraction: None,
                },
            );
            newly_opened.insert(page.id.clone());
            let count = self
                .slots
                .iter()
                .filter(|slot| slot.row == row && !slot.closing)
                .count();
            for slot in self.slots.iter_mut().filter(|slot| slot.row == row) {
                let width = demoted_width(slot.width_fraction, count);
                if width != slot.width_fraction {
                    slot.width_fraction = width;
                    slot.animated_width.set(width, Instant::now());
                    slot.restore_fraction = None;
                }
            }
        }
        // Inserting/removing slots must not change the user's active chat by index.
        self.active = active_id
            .and_then(|id| {
                self.slots
                    .iter()
                    .position(|slot| slot.panel.entity_id() == id)
            })
            .unwrap_or_else(|| {
                self.slots
                    .iter()
                    .position(|slot| slot.panel.entity_id() == source_id)
                    .unwrap()
            });
        for remembered in &mut self.row_focus {
            if remembered
                .is_some_and(|id| !self.slots.iter().any(|slot| slot.panel.entity_id() == id))
            {
                *remembered = None;
            }
        }
        if self
            .previous
            .is_some_and(|id| !self.slots.iter().any(|slot| slot.panel.entity_id() == id))
        {
            self.previous = None;
        }
        let focus_target = snapshot
            .focused_page_id
            .as_ref()
            .filter(|id| focus_changed || newly_opened.contains(*id))
            .and_then(|id| {
                self.slots.iter().position(|slot| {
                    let panel = slot.panel.read(cx);
                    panel.side_document_owner() == Some(owner)
                        && panel.side_document_page_id() == Some(id.as_str())
                })
            });
        if let Some(index) = focus_target {
            self.set_active(index, cx);
            self.focus_pending = true;
        } else if active_id
            != self
                .slots
                .get(self.active)
                .map(|slot| slot.panel.entity_id())
        {
            self.set_active(self.active, cx);
            self.focus_pending = true;
        }
        self.camera_dirty[row] = true;
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_sdk::{ApiEvent, SidePanelPage};

    fn snapshot(content: &str, focus: bool) -> SidePanelSnapshot {
        SidePanelSnapshot {
            focused_page_id: focus.then(|| "notes".into()),
            pages: vec![SidePanelPage {
                id: "notes".into(),
                title: "Project notes".into(),
                content: content.into(),
                ..Default::default()
            }],
        }
    }

    fn event(owner: &str, snapshot: SidePanelSnapshot) -> Update {
        Update::Event {
            session_id: owner.into(),
            event: ApiEvent::SidePanelState {
                session_id: owner.into(),
                snapshot,
            },
        }
    }

    #[gpui::test]
    fn side_panel_spawns_adjacent_updates_in_place_and_deletes(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w.push_test_panel("owner", cx);
            w.slots[0].width_fraction = 1.0;
            w.push_test_panel("other", cx);
            w.slots[1].row = 1;
            w.active = 1;
            w.active_row = 1;
            w
        });
        let doc = workspace.update(vcx, |w, cx| {
            assert!(w.apply(event("owner", snapshot("# First preview", true)), cx));
            assert_eq!(w.slots.len(), 3);
            assert_eq!(w.active, 1);
            assert_eq!(w.active_row, 0);
            assert_eq!(w.slots[0].width_fraction, DEFAULT_WIDTH);
            let panel = w.slots[1].panel.clone();
            assert_eq!(panel.read(cx).side_document_owner(), Some("owner"));
            assert_eq!(panel.read(cx).side_document_page_id(), Some("notes"));
            assert_eq!(w.slots[2].panel.read(cx).session_id, "other");
            panel
        });
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            w.set_active(0, cx);
            w.apply(
                event(
                    "owner",
                    snapshot("# Updated preview\n\n**Live content**", true),
                ),
                cx,
            );
            assert_eq!(w.slots.len(), 3);
            assert_eq!(w.slots[1].panel.entity_id(), doc.entity_id());
            assert_eq!(w.active, 0, "content updates do not steal focus from chat");
            assert_eq!(
                doc.read(cx)
                    .snapshot(cx)
                    .side_document
                    .as_ref()
                    .unwrap()
                    .page
                    .content,
                "# Updated preview\n\n**Live content**"
            );
            // Repeating unchanged state is the native protocol's focus operation.
            w.apply(
                event(
                    "owner",
                    snapshot("# Updated preview\n\n**Live content**", true),
                ),
                cx,
            );
            assert_eq!(w.active, 1);
            w.apply(event("owner", SidePanelSnapshot::default()), cx);
            assert_eq!(w.slots.len(), 2);
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "owner");
        });
        assert!(
            !commands.try_iter().any(|command| matches!(
                command,
                Command::Watch { .. } | Command::Unwatch { .. } | Command::CreateSession { .. }
            )),
            "document lifecycle must never create or attach an agent session"
        );
    }

    #[gpui::test]
    fn side_panel_nonfocused_pages_and_identical_ids_are_session_scoped(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("one", cx);
            w.push_test_panel("two", cx);
            w.active = 1;
            w
        });
        workspace.update(vcx, |w, cx| {
            w.apply(event("one", snapshot("one", false)), cx);
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "two");
            w.apply(event("two", snapshot("two", false)), cx);
            assert_eq!(w.slots.len(), 4);
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "two");
            let docs: Vec<_> = w
                .slots
                .iter()
                .filter(|slot| slot.panel.read(cx).is_side_document())
                .map(|slot| slot.panel.read(cx).session_id.clone())
                .collect();
            assert_eq!(docs.len(), 2);
            assert_ne!(docs[0], docs[1]);
            w.apply(event("one", SidePanelSnapshot::default()), cx);
            assert_eq!(w.slots.len(), 3);
            assert!(
                w.slots
                    .iter()
                    .any(|slot| slot.panel.read(cx).side_document_owner() == Some("two"))
            );
            assert!(!w.apply(event("not-open", snapshot("orphan", true)), cx));
            assert_eq!(w.slots.len(), 3);
        });
    }

    #[gpui::test]
    fn side_panel_restores_without_attaching_document_as_session(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w.push_test_panel("owner", cx);
            w.apply(event("owner", snapshot("# Persisted", true)), cx);
            w
        });
        let saved = vcx.update(|window, cx| workspace.read(cx).snapshot(window, cx).unwrap());
        workspace.update(vcx, |w, cx| w.apply_snapshot(saved, cx));
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 2);
            let doc = w.slots[1].panel.read(cx);
            assert!(doc.is_side_document());
            assert_eq!(doc.side_document_owner(), Some("owner"));
            assert_eq!(
                doc.snapshot(cx)
                    .side_document
                    .as_ref()
                    .unwrap()
                    .page
                    .content,
                "# Persisted"
            );
        });
        let watched: Vec<_> = commands
            .try_iter()
            .filter_map(|command| match command {
                Command::Watch { session_id } => Some(session_id),
                _ => None,
            })
            .collect();
        assert_eq!(watched, vec!["owner"]);
    }
}
