//! Read-only tool snapshots live beside their originating conversation.
use super::*;

#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct OpenChangeReview {
    pub source: gpui::EntityId,
    pub name: String,
    pub input: String,
    pub output: String,
    pub selected: usize,
    pub done: bool,
    pub failed: bool,
}

#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct CloseChangeReview {
    pub panel: gpui::EntityId,
}

impl Workspace {
    pub(super) fn close_change_review(
        &mut self,
        request: &CloseChangeReview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.slots.iter().position(|slot| {
            !slot.closing
                && slot.panel.entity_id() == request.panel
                && slot.panel.read(cx).is_change_review()
        }) else {
            return;
        };
        let source_id = self.slots[index]
            .panel
            .read(cx)
            .session_id
            .trim_start_matches("review://")
            .to_owned();
        self.set_active(index, cx);
        self.close_panel(&ClosePanel, window, cx);
        if let Some(source) = self
            .slots
            .iter()
            .position(|slot| !slot.closing && slot.panel.read(cx).session_id == source_id)
        {
            self.set_active(source, cx);
            self.focus_active(window, cx);
        }
    }

    pub(super) fn open_change_review(
        &mut self,
        request: &OpenChangeReview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source_index) = self
            .slots
            .iter()
            .position(|slot| !slot.closing && slot.panel.entity_id() == request.source)
        else {
            return;
        };
        if request.selected >= crate::diff::tool_diffs(&request.name, &request.input).len() {
            return;
        }
        let source = self.slots[source_index].panel.read(cx);
        let session_id = format!("review://{}", source.session_id);
        let working_dir = source.working_dir.clone();
        let row = self.slots[source_index].row;
        // Reuse the source conversation's review column, including when it has
        // been moved, rather than opening a new column for every file click.
        let index = if let Some(index) = self
            .slots
            .iter()
            .position(|slot| !slot.closing && slot.panel.read(cx).session_id == session_id)
        {
            self.slots[index]
                .panel
                .update(cx, |panel, cx| panel.set_change_review(request, cx));
            index
        } else {
            let panel = cx.new(|cx| {
                Panel::new_change_review(
                    session_id.clone(),
                    working_dir,
                    request,
                    self.bridge.clone(),
                    cx,
                )
            });
            let width_fraction = DEFAULT_WIDTH;
            let index = source_index + 1;
            self.slots.insert(
                index,
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
            if self.active >= index {
                self.active += 1;
            }
            let row_count = self
                .slots
                .iter()
                .filter(|slot| slot.row == row && !slot.closing)
                .count();
            let source = &mut self.slots[source_index];
            let width = demoted_width(source.width_fraction, row_count);
            if width != source.width_fraction {
                source.width_fraction = width;
                source.animated_width.set(width, Instant::now());
                source.restore_fraction = None;
            }
            crate::sounds::play(crate::sounds::Cue::PanelOpen, cx);
            index
        };
        self.set_active(index, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn diff_counts_open_adjacent_preserve_chat_and_focus_review(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("source", cx);
            workspace.push_test_panel("neighbour", cx);
            workspace
        });
        let source = workspace.read_with(vcx, |workspace, _| workspace.slots[0].panel.clone());
        source.update(vcx, |panel, cx| {
            panel.items = vec![crate::panel::Item::Tool {
                call_id: "edit".into(),
                name: "write".into(),
                input: serde_json::json!({"file_path":"src/example.rs","content":"new content\n"})
                    .to_string(),
                output: String::new(),
                done: true,
                error: None,
            }];
            panel.input.update(cx, |input, cx| {
                input.set_content("draft remains here".into(), cx)
            });
            cx.notify();
        });
        vcx.run_until_parked();
        let before = source.read_with(vcx, |panel, cx| panel.snapshot(cx));
        let header = vcx
            .debug_bounds("edit-change-counts-0")
            .expect("change counts paint");
        vcx.simulate_click(header.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let review = workspace.read_with(vcx, |workspace, cx| {
            assert_eq!(workspace.slots.len(), 3);
            assert_eq!(workspace.slots[0].panel, source);
            assert_eq!(workspace.slots[2].panel.read(cx).session_id, "neighbour");
            assert_eq!(workspace.active, 1);
            assert_eq!(workspace.slots[1].row, workspace.slots[0].row);
            let review = workspace.slots[1].panel.clone();
            assert!(review.read(cx).is_change_review());
            assert!(!review.read(cx).can_fork());
            review
        });
        vcx.update(|window, cx| {
            assert!(review.read(cx).input_focus_handle(cx).is_focused(window));
            let snapshot = workspace.read(cx).snapshot(window, cx).unwrap();
            assert_eq!(
                snapshot.slots.len(),
                2,
                "transient reviews must not restore as server sessions"
            );
            assert!(
                snapshot
                    .slots
                    .iter()
                    .all(|slot| !slot.panel.session_id.starts_with("review://"))
            );
        });
        vcx.simulate_keystrokes("x");
        let after = source.read_with(vcx, |panel, cx| panel.snapshot(cx));
        assert_eq!(before.draft, after.draft);
        assert_eq!(before.scroll_y, after.scroll_y);
        assert_eq!(source.read_with(vcx, |panel, _| panel.items.len()), 1);
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert_eq!(workspace.slots[workspace.active].panel, source);
            assert!(
                workspace
                    .slots
                    .iter()
                    .filter(|slot| slot.panel == review)
                    .all(|slot| slot.closing)
            );
        });
        vcx.update(|window, cx| assert!(source.read(cx).input_focus_handle(cx).is_focused(window)));
        assert!(commands.try_iter().all(|command| !matches!(command,
            Command::Watch { session_id } | Command::Unwatch { session_id } if session_id.starts_with("review://")
        )));
    }

    #[gpui::test]
    fn review_reuses_entity_on_its_strip_and_closes_back_to_source(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("remote-source", cx);
            workspace
        });
        let source = workspace.read_with(vcx, |workspace, _| workspace.slots[0].panel.clone());
        source.update(vcx, |panel, _| {
            panel.working_dir = Some("/remote/project".into())
        });
        let request = OpenChangeReview {
            source: source.entity_id(),
            output: String::new(),
            name: "write".into(),
            input: serde_json::json!({"file_path":"/remote/project/empty.rs","content":""})
                .to_string(),
            selected: 0,
            done: true,
            failed: false,
        };
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_change_review(&request, window, cx)
            })
        });
        vcx.run_until_parked();
        let review = workspace.read_with(vcx, |workspace, cx| {
            let review = workspace.slots[1].panel.clone();
            assert_eq!(
                review.read(cx).working_dir.as_deref(),
                Some("/remote/project")
            );
            review
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.slots[1].row = 2;
                workspace.set_active(0, cx);
                workspace.open_change_review(&request, window, cx);
                assert_eq!(workspace.slots.len(), 2);
                assert_eq!(workspace.slots[1].panel, review);
                assert_eq!(workspace.active_row, 2);
                assert_eq!(workspace.row_focus[2], Some(review.entity_id()));
                let snapshot = workspace.snapshot(window, cx).unwrap();
                assert_eq!(snapshot.active, 0);
                assert_eq!(snapshot.active_row, 0);
                assert_eq!(snapshot.focus, FocusSnapshot::Panel(0));
                workspace.close_change_review(
                    &CloseChangeReview {
                        panel: review.entity_id(),
                    },
                    window,
                    cx,
                );
                assert_eq!(workspace.active_row, 0);
                assert_eq!(workspace.slots[workspace.active].panel, source);
            })
        });
    }

    #[gpui::test]
    fn review_ignores_invalid_or_closed_source_requests(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("source", cx);
            workspace
        });
        let source = workspace.read_with(vcx, |workspace, _| workspace.slots[0].panel.entity_id());
        let mut request = OpenChangeReview {
            source,
            output: String::new(),
            name: "write".into(),
            input: "{}".into(),
            selected: 0,
            done: false,
            failed: true,
        };
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_change_review(&request, window, cx);
                assert_eq!(workspace.slots.len(), 1);
                request.input =
                    serde_json::json!({"file_path":"file.rs","content":"line"}).to_string();
                request.selected = 99;
                workspace.open_change_review(&request, window, cx);
                assert_eq!(workspace.slots.len(), 1);
                request.selected = 0;
                workspace.slots[0].closing = true;
                workspace.open_change_review(&request, window, cx);
                assert_eq!(workspace.slots.len(), 1);
            })
        });
    }
}
