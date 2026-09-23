//! Native utility windows for standalone mode. The originating workspace is never rearranged.
use super::*;
use gpui::{AnyWindowHandle, WeakEntity, WindowOptions};

pub(crate) struct PanelWindow {
    pub(crate) panel: Entity<Panel>,
    source: Option<WeakEntity<Panel>>,
    source_session: Option<String>,
    source_window: AnyWindowHandle,
    finished: bool,
    window_background: Option<gpui::WindowBackgroundAppearance>,
}

/// Open a utility panel in its own native window, or activate its existing window.
/// Return the hosted entity so callers can apply commands even when reusing a window.
pub(crate) fn open_panel_window(
    panel: Entity<Panel>,
    source: Option<Entity<Panel>>,
    source_window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<Entity<Panel>> {
    open_panel_window_at(panel, source, source_window.window_handle(), true, cx)
}

/// Event-driven documents may open without a borrowed window or focus intent.
pub(crate) fn open_panel_window_at(
    panel: Entity<Panel>,
    source: Option<Entity<Panel>>,
    origin: AnyWindowHandle,
    activate: bool,
    cx: &mut App,
) -> anyhow::Result<Entity<Panel>> {
    let session_id = panel.read(cx).session_id.clone();
    let source_session = source
        .as_ref()
        .map(|source| source.read(cx).session_id.clone());
    for handle in cx.windows() {
        let Some(handle) = handle.downcast::<PanelWindow>() else {
            continue;
        };
        if handle
            .read(cx)
            .is_ok_and(|root| !root.finished && root.panel.read(cx).session_id == session_id)
        {
            return handle.update(cx, |root, window, cx| {
                root.source = source.as_ref().map(Entity::downgrade);
                root.source_session = source_session;
                root.source_window = origin;
                if activate {
                    root.panel
                        .update(cx, |panel, cx| panel.focus_input(window, cx));
                    window.activate_window();
                }
                root.panel.clone()
            });
        }
    }
    let hosted = panel.clone();
    cx.open_window(
        WindowOptions {
            app_id: Some(crate::APP_ID.into()),
            focus: activate,
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title(&format!("{} · Jcode", panel.read(cx).title));
            if activate {
                window.activate_window();
            }
            cx.new(|cx| {
                cx.subscribe_in(
                    &panel,
                    window,
                    |this: &mut PanelWindow,
                     _,
                     _: &crate::panel::AccountsPanelClosed,
                     window,
                     cx| {
                        this.close(false, window, cx);
                    },
                )
                .detach();
                cx.subscribe_in(
                    &panel,
                    window,
                    |this: &mut PanelWindow,
                     _,
                     _: &crate::panel::AccountsPanelChooseModel,
                     window,
                     cx| {
                        this.close(true, window, cx);
                    },
                )
                .detach();
                let weak = cx.entity().downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    let _ = weak.update(cx, |this, cx| this.finish(false, cx));
                    true
                });
                panel.update(cx, |panel, cx| panel.focus_input(window, cx));
                PanelWindow {
                    panel,
                    source: source.as_ref().map(Entity::downgrade),
                    source_session,
                    source_window: origin,
                    finished: false,
                    window_background: None,
                }
            })
        },
    )?;
    Ok(hosted)
}

impl PanelWindow {
    /// An agent deletion must not activate the source of a background document.
    pub(super) fn remove_document(&mut self, window: &mut Window) {
        self.finished = true;
        window.remove_window();
    }

    fn finish(&mut self, choose_model: bool, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.finished, true) {
            return;
        }
        let source = self.source.clone();
        let source_session = self.source_session.clone();
        let origin = self.source_window;
        let accounts = self.panel.read(cx).is_accounts_panel();
        // Release the child window update before focusing its originating window.
        cx.defer(move |cx| {
            let _ = origin.update(cx, |root, window, cx| {
                let workspace = root.downcast::<Workspace>().ok();
                let source = workspace
                    .as_ref()
                    .and_then(|workspace| {
                        workspace
                            .read(cx)
                            .slots
                            .iter()
                            .find(|slot| {
                                !slot.closing
                                    && source_session.as_deref()
                                        == Some(slot.panel.read(cx).session_id.as_str())
                            })
                            .map(|slot| slot.panel.clone())
                    })
                    .or_else(|| source.and_then(|source| source.upgrade()));
                let Some(source) = source else {
                    return;
                };
                if accounts && source.read(cx).can_refresh_account_runtime() {
                    if let Some(workspace) = workspace {
                        workspace.read(cx).bridge.send(Command::RefreshRuntime {
                            session_id: source.read(cx).session_id.clone(),
                        });
                    }
                }
                if choose_model {
                    window.dispatch_action(
                        Box::new(OpenModelPicker {
                            source: source.entity_id(),
                        }),
                        cx,
                    );
                }
                source.update(cx, |panel, cx| panel.focus_input(window, cx));
                window.activate_window();
            });
        });
    }

    fn close(&mut self, choose_model: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.finish(choose_model, cx);
        window.remove_window();
    }
}

impl Render for PanelWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Theme::sync_window_background(window, &mut self.window_background);
        let theme = Theme::global();
        div()
            .debug_selector(|| "panel-window-root".into())
            .size_full()
            .relative()
            .bg(theme.PANEL_BG)
            .text_color(theme.TEXT)
            .font_family(theme.FONT_UI)
            .text_size(px(14.0 * crate::config::get().appearance.text_scale))
            .on_action(cx.listener(
                |this, action: &change_review::CloseChangeReview, window, cx| {
                    if action.panel == this.panel.entity_id() {
                        this.close(false, window, cx);
                        cx.stop_propagation();
                    }
                },
            ))
            .on_action(cx.listener(|this, _: &ClosePanel, window, cx| {
                this.close(false, window, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close(false, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.panel.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn accounts_return_resolves_reloaded_source_by_session(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.single_panel = true;
            w.push_test_panel("reload-source", cx);
            w
        });
        let old_source = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
        workspace.update_in(vcx, |w, window, cx| {
            w.open_accounts(
                &OpenAccounts {
                    source: old_source.entity_id(),
                    login_command: None,
                },
                window,
                cx,
            );
        });
        vcx.run_until_parked();
        let (accounts_window, accounts) = vcx.update(|_, cx| {
            cx.windows()
                .into_iter()
                .find_map(|handle| {
                    let handle = handle.downcast::<PanelWindow>()?;
                    Some((handle, handle.read(cx).ok()?.panel.clone()))
                })
                .unwrap()
        });
        let (bridge, commands) = harness::spawn_recording();
        // Real hot reload replaces the root and all chat entities. Keep the old
        // source alive deliberately, proving the stable ID wins over its weak ref.
        let replacement = vcx.update(|window, cx| {
            let root = window.replace_root(cx, |_, cx| {
                let mut w = Workspace::for_test(learning::Coach::new(), cx);
                w.single_panel = true;
                w.set_test_bridge(bridge);
                w.push_test_panel("reload-source", cx);
                w
            });
            root.update(cx, |w, cx| w.focus_active(window, cx));
            root
        });
        vcx.run_until_parked();
        let new_source = replacement.read_with(vcx, |w, _| w.slots[0].panel.clone());
        assert_ne!(new_source, old_source);
        accounts.update(vcx, |_, cx| cx.emit(crate::panel::AccountsPanelClosed));
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            assert!(!cx.windows().contains(&accounts_window.into()));
            assert_eq!(cx.windows().len(), 1);
            assert!(
                new_source
                    .read(cx)
                    .input_focus_handle(cx)
                    .is_focused(window)
            );
            assert!(
                !old_source
                    .read(cx)
                    .input_focus_handle(cx)
                    .is_focused(window)
            );
        });
        assert!(
            matches!(commands.try_recv(), Ok(Command::RefreshRuntime { session_id }) if session_id == "reload-source")
        );
        assert!(commands.try_recv().is_err());
        replacement.read_with(vcx, |w, _| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.slots[0].panel, new_source);
        });
    }
}
