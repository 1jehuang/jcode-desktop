//! Agent-managed documents are native workspace panels, not transcript messages.
use super::*;
use jcode_sdk::SidePanelSnapshot;
use sha2::{Digest, Sha256};

/// Replay/dismissal history must survive UI replacement, but must not retain a
/// second copy of every PDF. Hash the serialized page directly into the digest.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub(super) struct SidePanelRoutingState {
    focus_revision: u64,
    focused_page_id: Option<String>,
    pages: HashMap<String, [u8; 32]>,
}

impl SidePanelRoutingState {
    fn from_snapshot(snapshot: &SidePanelSnapshot) -> Self {
        struct HashWriter(Sha256);
        impl std::io::Write for HashWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut pages = HashMap::new();
        for page in &snapshot.pages {
            // Match the router's first-record-wins handling of duplicate IDs.
            pages.entry(page.id.clone()).or_insert_with(|| {
                let mut writer = HashWriter(Sha256::new());
                serde_json::to_writer(&mut writer, page).expect("side panel page serializes");
                writer.0.finalize().into()
            });
        }
        Self {
            focus_revision: snapshot.focus_revision,
            focused_page_id: snapshot.focused_page_id.clone(),
            pages,
        }
    }
}

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
        let routing = SidePanelRoutingState::from_snapshot(snapshot);
        let previous = self
            .side_panel_snapshots
            .insert(owner.to_owned(), routing.clone());
        let focus_changed = previous.as_ref().is_none_or(|old| {
            old.focus_revision != snapshot.focus_revision
                || (snapshot.focus_revision == 0
                    && (old.focused_page_id != snapshot.focused_page_id || old == &routing))
        });

        if self.single_panel {
            let owner = owner.to_owned();
            let snapshot = snapshot.clone();
            let workspace = cx.entity().downgrade();
            // Runtime events do not borrow a Window. Wait until the current
            // entity/window update has finished before finding its native root.
            cx.defer(move |cx| {
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                let origin = cx.windows().into_iter().find(|handle| {
                    handle
                        .update(cx, |root, _, _| {
                            root.downcast::<Workspace>()
                                .is_ok_and(|root| root.entity_id() == workspace.entity_id())
                        })
                        .unwrap_or(false)
                });
                let Some(origin) = origin else {
                    return;
                };
                workspace.update(cx, |this, cx| {
                    this.apply_side_panel_windows(
                        &owner,
                        &snapshot,
                        previous.as_ref(),
                        &routing,
                        focus_changed,
                        origin,
                        cx,
                    );
                });
            });
            return true;
        }

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
            // A user-dismissed document must not reopen just because an unrelated
            // panel changed. An explicit focus or a change to that document can reopen it.
            let unchanged = previous
                .as_ref()
                .is_some_and(|old| old.pages.get(&page.id) == routing.pages.get(&page.id));
            let explicitly_focused =
                focus_changed && snapshot.focused_page_id.as_deref() == Some(&page.id);
            let absent_or_closing = !self.slots.iter().any(|slot| {
                !slot.closing
                    && slot.panel.read(cx).side_document_owner() == Some(owner)
                    && slot.panel.read(cx).side_document_page_id() == Some(&page.id)
            });
            if unchanged && absent_or_closing && !explicitly_focused {
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
            .filter(|id| {
                focus_changed || (snapshot.focus_revision == 0 && newly_opened.contains(*id))
            })
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

    fn apply_side_panel_windows(
        &mut self,
        owner: &str,
        snapshot: &SidePanelSnapshot,
        previous: Option<&SidePanelRoutingState>,
        routing: &SidePanelRoutingState,
        focus_changed: bool,
        origin: gpui::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .slots
            .iter()
            .find(|slot| !slot.closing && slot.panel.read(cx).session_id == owner)
            .map(|slot| slot.panel.clone())
        else {
            return;
        };
        let mut documents = HashMap::new();
        for handle in cx.windows() {
            let Some(handle) = handle.downcast::<panel_window::PanelWindow>() else {
                continue;
            };
            let Ok(root) = handle.read(cx) else {
                continue;
            };
            let panel = root.panel.clone();
            if panel.read(cx).side_document_owner() != Some(owner) {
                continue;
            }
            let Some(id) = panel.read(cx).side_document_page_id().map(str::to_owned) else {
                continue;
            };
            if snapshot.pages.iter().any(|page| page.id == id) {
                documents.insert(id, (handle, panel));
            } else {
                let _ = handle.update(cx, |root, window, _| root.remove_document(window));
            }
        }
        let mut seen = HashSet::new();
        for page in &snapshot.pages {
            if !seen.insert(&page.id) {
                continue;
            }
            let unchanged =
                previous.is_some_and(|old| old.pages.get(&page.id) == routing.pages.get(&page.id));
            let explicitly_focused =
                focus_changed && snapshot.focused_page_id.as_deref() == Some(&page.id);
            let existing = documents.get(&page.id);
            if unchanged && existing.is_none() && !explicitly_focused {
                continue;
            }
            let activate = snapshot.focused_page_id.as_deref() == Some(&page.id)
                && (focus_changed || (snapshot.focus_revision == 0 && existing.is_none()));
            let panel = if let Some((handle, panel)) = existing {
                panel.update(cx, |panel, cx| panel.update_side_document(page, cx));
                let _ = handle.update(cx, |root, window, cx| {
                    window.set_window_title(&format!("{} · Jcode", root.panel.read(cx).title));
                });
                if !activate {
                    continue;
                }
                panel.clone()
            } else {
                cx.new(|cx| Panel::new_side_document(owner, page, self.bridge.clone(), cx))
            };
            if let Err(error) = panel_window::open_panel_window_at(
                panel,
                Some(source.clone()),
                origin,
                activate,
                cx,
            ) {
                eprintln!("could not open side document window: {error:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_sdk::{ApiEvent, SidePanelPage};

    fn snapshot(content: &str, focus: bool) -> SidePanelSnapshot {
        SidePanelSnapshot {
            focus_revision: 0,
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

    #[test]
    fn routing_state_fingerprints_payload_without_serializing_it_again() {
        let mut state = snapshot("fallback", true);
        state.focus_revision = 7;
        state.pages[0].pdf_data = Some("private PDF bytes".repeat(65_536));
        let routing = SidePanelRoutingState::from_snapshot(&state);
        let encoded = serde_json::to_vec(&routing).unwrap();
        assert!(
            encoded.len() < 512,
            "routing state must not retain PDF bytes"
        );
        let restored: SidePanelRoutingState = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(routing, restored);
        let expected: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&state.pages[0]).unwrap()).into();
        assert_eq!(routing.pages["notes"], expected);
        state.pages[0].pdf_data.as_mut().unwrap().push('x');
        assert_ne!(
            routing.pages,
            SidePanelRoutingState::from_snapshot(&state).pages
        );
    }

    #[gpui::test]
    fn fresh_workspace_restore_preserves_dismissal_and_replay_focus(cx: &mut gpui::TestAppContext) {
        let mut state = snapshot("dismissed document", true);
        state.focus_revision = 7;
        state.pages.push(SidePanelPage {
            id: "retained".into(),
            content: "still open".into(),
            ..Default::default()
        });
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("owner", cx);
            w.apply(event("owner", state.clone()), cx);
            w
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                assert_eq!(
                    w.slots[w.active].panel.read(cx).side_document_page_id(),
                    Some("notes")
                );
                w.close_panel(&ClosePanel, window, cx);
                w.set_active(0, cx);
            });
        });
        let saved = vcx.update(|window, cx| {
            let bytes = workspace
                .read(cx)
                .snapshot(window, cx)
                .unwrap()
                .encode()
                .unwrap();
            WorkspaceSnapshot::decode(&bytes).unwrap()
        });
        assert_eq!(
            saved.slots.len(),
            2,
            "snapshot omits locally closed document"
        );

        // A real UI reload creates a new Workspace, unlike restoring into the
        // original entity, whose routing history could mask this regression.
        let restored = vcx.update(|_, cx| {
            cx.new(|cx| {
                let mut w = Workspace::for_test(learning::Coach::new(), cx);
                assert!(w.side_panel_snapshots.is_empty());
                w.apply_snapshot(saved, cx);
                w
            })
        });
        restored.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(
                w.slots.len(),
                2,
                "replay must not resurrect a locally closed document"
            );
            assert_eq!(
                w.slots[w.active].panel.read(cx).session_id,
                "owner",
                "replay must preserve restored chat focus"
            );
            assert_eq!(
                w.slots[1].panel.read(cx).side_document_page_id(),
                Some("retained")
            );
            state.focus_revision += 1;
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(
                w.slots.len(),
                3,
                "fresh focus intent still reopens a dismissed document"
            );
            assert_eq!(
                w.slots[w.active].panel.read(cx).side_document_page_id(),
                Some("notes")
            );
        });
    }

    #[gpui::test]
    fn panel_focus_revisions_do_not_steal_focus_on_replay_or_reopen_dismissed_documents(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("owner", cx);
            w
        });
        workspace.update(vcx, |w, cx| {
            let mut state = snapshot("original", true);
            state.focus_revision = 1;
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(w.active, 1);
            w.set_active(0, cx);
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(
                w.active, 0,
                "identical reconnect state must not steal focus"
            );
            state.pages[0].content = "changed".into();
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(w.active, 0, "background update preserves focus");
            state.focus_revision += 1;
            state.pages[0].content = "changed and explicitly focused".into();
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(
                w.active, 1,
                "explicit update focus works even for the same focused id"
            );
            w.slots.remove(1); // A local dismissal does not mutate the agent's source record.
            w.active = 0;
            state.pages.push(SidePanelPage {
                id: "second".into(),
                content: "new document".into(),
                ..Default::default()
            });
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(
                w.slots.len(),
                2,
                "unrelated spawn must not resurrect a dismissed panel"
            );
            assert_eq!(
                w.slots[1].panel.read(cx).side_document_page_id(),
                Some("second")
            );
            state.focus_revision += 1;
            w.apply(event("owner", state), cx);
            assert_eq!(
                w.slots.len(),
                3,
                "explicit focus can reopen a dismissed document"
            );
            assert_eq!(
                w.slots[w.active].panel.read(cx).side_document_page_id(),
                Some("notes")
            );
        });
    }

    #[gpui::test]
    fn closing_or_updating_a_background_document_does_not_move_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("owner", cx);
            w
        });
        workspace.update(vcx, |w, cx| {
            let mut state = snapshot("first", true);
            state.focus_revision = 1;
            state.pages.push(SidePanelPage {
                id: "second".into(),
                content: "second".into(),
                ..Default::default()
            });
            w.apply(event("owner", state.clone()), cx);
            w.set_active(0, cx);
            state.pages.remove(0);
            state.focused_page_id = Some("second".into()); // Core chooses a fallback when the first record is deleted.
            w.apply(event("owner", state.clone()), cx);
            assert_eq!(
                w.active, 0,
                "closing a background document must not focus its fallback"
            );
            w.slots.remove(1);
            state.pages[0].content = "updated while locally dismissed".into();
            w.apply(event("owner", state), cx);
            assert_eq!(w.slots.len(), 2);
            assert_eq!(
                w.active, 0,
                "focus=false updates remain nonfocusing when reopening a document"
            );
        });
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

    fn document_windows(
        vcx: &mut gpui::VisualTestContext,
    ) -> Vec<(gpui::WindowHandle<panel_window::PanelWindow>, Entity<Panel>)> {
        vcx.update(|_, cx| {
            cx.windows()
                .into_iter()
                .filter_map(|handle| {
                    let handle = handle.downcast::<panel_window::PanelWindow>()?;
                    let panel = handle.read(cx).ok()?.panel.clone();
                    panel.read(cx).is_side_document().then_some((handle, panel))
                })
                .collect()
        })
    }

    #[gpui::test]
    fn single_panel_documents_spawn_update_focus_delete_without_changing_source(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.single_panel = true;
            w.set_test_bridge(bridge);
            w.push_test_panel("owner", cx);
            w
        });
        let source = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
        workspace.update_in(vcx, |w, window, cx| {
            source.update(cx, |panel, cx| {
                panel
                    .items
                    .push(crate::panel::Item::User("Conversation stays here".into()));
                panel.input.update(cx, |input, cx| {
                    input.set_content("Unsent source draft".into(), cx)
                });
            });
            w.focus_active(window, cx);
            window.activate_window();
        });
        vcx.run_until_parked();
        let origin = vcx.update(|window, _| window.window_handle());
        let before = source.read_with(vcx, |panel, cx| {
            (panel.items.clone(), panel.snapshot(cx).draft)
        });
        let bounds = vcx.debug_bounds("single-panel-root").unwrap();
        let mut state = snapshot("First", false);
        state.focus_revision = 1;
        workspace.update(vcx, |w, cx| {
            assert!(w.apply(event("owner", state.clone()), cx));
        });
        vcx.run_until_parked();
        let docs = document_windows(vcx);
        assert_eq!(docs.len(), 1);
        let (child, document) = docs[0].clone();
        vcx.update(|_, cx| {
            assert_eq!(
                cx.active_window(),
                Some(origin),
                "background spawn must not activate its window"
            )
        });
        state.pages[0].content = "Updated".into();
        state.pages[0].title = "Updated title".into();
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        assert_eq!(document_windows(vcx)[0].1, document);
        document.read_with(vcx, |panel, cx| {
            assert_eq!(
                panel.snapshot(cx).side_document.unwrap().page.content,
                "Updated"
            );
            assert_eq!(panel.title.as_ref(), "Updated title");
        });
        // Duplicate records still obey first-record-wins and cannot open another window.
        let mut duplicate = state.pages[0].clone();
        duplicate.content = "Must not replace first".into();
        state.pages.push(duplicate);
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        assert_eq!(document_windows(vcx).len(), 1);
        state.focus_revision += 1;
        state.focused_page_id = Some("notes".into());
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                cx.active_window(),
                Some(child.into()),
                "explicit focus activates child"
            )
        });
        vcx.update(|window, _| window.activate_window());
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                cx.active_window(),
                Some(origin),
                "replay must not steal focus"
            )
        });
        assert_eq!(document_windows(vcx).len(), 1);
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", SidePanelSnapshot::default()), cx);
        });
        vcx.run_until_parked();
        assert!(document_windows(vcx).is_empty());
        assert_eq!(vcx.debug_bounds("single-panel-root"), Some(bounds));
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.active, 0);
            assert_eq!(w.slots[0].panel, source);
            assert_eq!(
                (
                    source.read(cx).items.clone(),
                    source.read(cx).snapshot(cx).draft
                ),
                before
            );
        });
        vcx.update(|_, cx| assert_eq!(cx.active_window(), Some(origin)));
        assert!(
            commands.try_recv().is_err(),
            "documents never attach or create sessions"
        );
    }

    #[gpui::test]
    fn single_panel_document_dismissal_replay_and_changed_page_reopening(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.single_panel = true;
            w.push_test_panel("owner", cx);
            w
        });
        let mut state = snapshot("Original", true);
        state.focus_revision = 4;
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        let (child, _) = document_windows(vcx).pop().unwrap();
        let mut child_cx = gpui::VisualTestContext::from_window(child.into(), vcx);
        child_cx.run_until_parked();
        child_cx.dispatch_action(ClosePanel);
        vcx.run_until_parked();
        assert!(document_windows(vcx).is_empty());
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        assert!(
            document_windows(vcx).is_empty(),
            "replay must not reopen a dismissed page"
        );
        state.pages.push(SidePanelPage {
            id: "other".into(),
            content: "unrelated".into(),
            ..Default::default()
        });
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        let docs = document_windows(vcx);
        assert_eq!(docs.len(), 1);
        docs[0].1.read_with(vcx, |panel, _| {
            assert_eq!(panel.side_document_page_id(), Some("other"))
        });
        state.pages[0].content = "Changed while dismissed".into();
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            document_windows(vcx).len(),
            2,
            "changed dismissed page reopens"
        );
        let origin = vcx.update(|window, cx| {
            assert_eq!(
                cx.active_window(),
                Some(window.window_handle()),
                "reopen without new focus intent stays background"
            );
            window.window_handle()
        });
        let (notes, _) = document_windows(vcx)
            .into_iter()
            .find(|(_, panel)| {
                panel.read_with(vcx, |panel, _| {
                    panel.side_document_page_id() == Some("notes")
                })
            })
            .unwrap();
        notes
            .update(vcx, |root, window, _| root.remove_document(window))
            .unwrap();
        state.focus_revision += 1;
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", state.clone()), cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            document_windows(vcx).len(),
            2,
            "explicit focus reopens unchanged dismissed page"
        );
        vcx.update(|_, cx| assert_ne!(cx.active_window(), Some(origin)));
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.active, 0);
        });
    }

    #[gpui::test]
    fn single_panel_documents_find_replaced_host_root_and_ignore_late_events(
        cx: &mut gpui::TestAppContext,
    ) {
        struct HostRoot;
        impl Render for HostRoot {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }
        let (_, vcx) = cx.add_window_view(|_, _| HostRoot);
        // Native host handles retain their original root type even after activation.
        let workspace = vcx.update(|window, cx| {
            window.replace_root(cx, |_, cx| {
                let mut w = Workspace::for_test(learning::Coach::new(), cx);
                w.single_panel = true;
                w.push_test_panel("owner", cx);
                w
            })
        });
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", snapshot("Document", false)), cx);
        });
        vcx.run_until_parked();
        assert_eq!(document_windows(vcx).len(), 1);
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", SidePanelSnapshot::default()), cx);
        });
        vcx.run_until_parked();
        assert!(document_windows(vcx).is_empty());
        workspace.update(vcx, |w, cx| {
            w.apply(event("owner", snapshot("Late document", false)), cx);
            w.slots.clear();
        });
        vcx.run_until_parked();
        assert!(
            document_windows(vcx).is_empty(),
            "source closed before deferred routing must not create an orphan"
        );
    }
}
