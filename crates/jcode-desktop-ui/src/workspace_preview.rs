//! Transient self-development panels. They are deliberately omitted from snapshots.
use super::*;
use crate::{
    preview_control::{self, Request},
    preview_state::PreviewState,
};

impl Workspace {
    pub(super) fn start_preview_control(&mut self, cx: &mut Context<Self>) {
        if !preview_control::enabled() {
            return;
        }
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            // The old root must drop before this generation binds the same PID.
            // Never downcast an old generation's Workspace with a new layout.
            let mut connection = None;
            for attempt in 0..20 {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                match preview_control::Server::start() {
                    Ok(value) => {
                        connection = Some(value);
                        break;
                    }
                    Err(error) if attempt == 19 => {
                        eprintln!("preview control unavailable: {error}")
                    }
                    Err(_) => (),
                }
            }
            let Some((server, requests)) = connection else {
                return;
            };
            if this
                .update(cx, |workspace, _| workspace.preview_control = Some(server))
                .is_err()
            {
                return;
            }
            while let Ok(pending) = requests.recv().await {
                if Instant::now() >= pending.deadline {
                    continue;
                }
                let result = this.update(cx, |workspace, cx| {
                    workspace.handle_preview_request(pending.request, cx)
                });
                let Ok(result) = result else {
                    break;
                };
                let _ = pending.reply.send(result);
            }
        }));
    }

    pub(super) fn handle_preview_request(
        &mut self,
        request: Request,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        use serde_json::json;
        if !preview_control::enabled() {
            return json!({"ok":false,"error":"self-development previews disabled"});
        }
        let (state, reset) = match request {
            Request::List {} => {
                return json!({"ok":true,"pid":std::process::id(),"states":PreviewState::ALL.iter().map(|s| json!({"id":s.id(),"title":s.title()})).collect::<Vec<_>>() });
            }
            Request::Onboarding {} => {
                self.restart_onboarding_simulator(cx);
                return json!({"ok":true,"step":"welcome"});
            }
            Request::Open { state } => (state, false),
            Request::Reset { state } => (state, true),
        };
        let Ok(state) = state.parse::<PreviewState>() else {
            return json!({"ok":false,"error":"unknown preview state"});
        };
        let index = if reset {
            let Some(index) = self.slots.iter().rposition(|slot| {
                !slot.closing && slot.panel.read(cx).preview_state == Some(state)
            }) else {
                return json!({"ok":false,"error":"no open preview for this state"});
            };
            self.slots[index]
                .panel
                .update(cx, |panel, cx| panel.reset_preview(cx));
            self.set_active(index, cx);
            self.focus_pending = true;
            self.retarget_camera();
            cx.notify();
            index
        } else {
            self.open_preview_unchecked(state, cx)
        };
        json!({"ok":true,"state":state.id(),"id":self.slots[index].panel.read(cx).session_id,"panel_count":self.slots.iter().filter(|slot| !slot.closing).count()})
    }

    /// Reusable UI entry point. Opening adds a column, never replaces a real session.
    pub fn open_preview(&mut self, state: PreviewState, cx: &mut Context<Self>) -> Option<usize> {
        preview_control::enabled().then(|| self.open_preview_unchecked(state, cx))
    }

    pub(super) fn open_preview_unchecked(
        &mut self,
        state: PreviewState,
        cx: &mut Context<Self>,
    ) -> usize {
        let panel = cx.new(|cx| Panel::new_preview(state, cx));
        let width_fraction = if self.row_indices(self.active_row).count() == 0 {
            1.0
        } else {
            DEFAULT_WIDTH
        };
        let active_on_row = self
            .slots
            .get(self.active)
            .is_some_and(|slot| slot.row == self.active_row);
        let index = insert_index(
            self.active,
            active_on_row,
            self.row_indices(self.active_row).last(),
            self.slots.len(),
        );
        self.slots.insert(
            index,
            Slot {
                panel,
                row: self.active_row,
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
        let count = self.row_indices(self.active_row).count();
        for other in self.row_indices(self.active_row).collect::<Vec<_>>() {
            let slot = &mut self.slots[other];
            let width = demoted_width(slot.width_fraction, count);
            if width != slot.width_fraction {
                slot.width_fraction = width;
                slot.animated_width.set(width, Instant::now());
                slot.restore_fraction = None;
            }
        }
        self.set_active(index, cx);
        self.focus_pending = true;
        self.retarget_camera();
        cx.notify();
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn previews_append_without_runtime_commands_and_are_not_persisted(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("real-session", cx);
            workspace
        });
        vcx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                let real = workspace.slots[0].panel.entity_id();
                for state in PreviewState::ALL {
                    workspace.open_preview_unchecked(*state, cx);
                    workspace.fork_panel(&ForkPanel, window, cx);
                }
                assert_eq!(workspace.slots.len(), PreviewState::ALL.len() + 1);
                assert_eq!(workspace.slots[0].panel.entity_id(), real);
                let snapshot = workspace.snapshot(window, cx).unwrap();
                assert_eq!(snapshot.slots.len(), 1);
                assert_eq!(snapshot.slots[0].panel.session_id, "real-session");
                workspace.close_panel(&ClosePanel, window, cx);
                let mut unsafe_snapshot = snapshot;
                unsafe_snapshot.slots[0].panel.session_id = "preview://old-generation/1".into();
                workspace.apply_snapshot(unsafe_snapshot, cx);
                assert!(
                    workspace.slots.is_empty(),
                    "old preview IDs must not be watched on restore"
                );
                assert!(
                    commands.try_recv().is_err(),
                    "preview operations must never reach runtime bridge"
                );
            })
        });
    }
}
