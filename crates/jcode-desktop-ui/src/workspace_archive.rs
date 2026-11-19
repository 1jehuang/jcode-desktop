//! Sidebar archiving. Archiving is the SDK's reversible hide: the transcript
//! stays on disk and an Undo pill restores the session right away.
use super::*;

/// How long the "Session archived" notice with its Undo pill stays up.
const ARCHIVE_NOTICE: Duration = Duration::from_secs(6);

impl Workspace {
    pub(super) fn set_session_archived(
        &mut self,
        session_id: &str,
        archived: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collections.menu = None;
        if archived {
            // An archived session has no place on screen: close its panel and
            // drop the row at once. The bridge refreshes the catalog after.
            self.close_sidebar_sessions(vec![session_id.to_string()], window, cx);
            self.sessions.retain(|session| session.session_id != session_id);
            self.sidebar_closed_sessions.remove(session_id);
            self.archive_notice = Some((session_id.to_string(), Instant::now()));
        } else {
            self.archive_notice = None;
            // Restored sessions come back as past rows of this window.
            self.sidebar_closed_sessions.insert(session_id.to_string());
            self.archived_sessions
                .retain(|session| session.session_id != session_id);
        }
        self.bridge.send(Command::SetSessionArchived {
            session_id: session_id.to_string(),
            archived,
        });
        cx.notify();
    }

    /// Re-read local archived sessions while their section is open.
    pub(super) fn reload_archived_sessions(&mut self) {
        if !self.archived_open || cfg!(test) {
            return;
        }
        self.archived_sessions = harness::jcode_home()
            .map(|home| harness::load_archived_sessions(&home))
            .unwrap_or_default();
    }

    /// Collapsible "Archived" section at the bottom of the session list.
    pub(super) fn render_archived_section(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let open = self.archived_open;
        let mut section = div()
            .id("sidebar-archived")
            .flex_none()
            .w_full()
            .min_w_0()
            .pl_2()
            .pr(px(crate::scrollbar::GUTTER + 4.0))
            .py_1()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .id("sidebar-archived-toggle")
                    .debug_selector(|| "sidebar-archived-toggle".into())
                    .h(px(24.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_full()
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(theme.TEXT_DIM)
                    .hover(|el| el.bg(theme.TOOL_BG).text_color(theme.TEXT))
                    .child(if open { "▾" } else { "▸" })
                    .child("Archived")
                    .when(open, |el| {
                        el.child(
                            div()
                                .text_color(theme.TEXT_FAINT)
                                .child(self.archived_sessions.len().to_string()),
                        )
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.archived_open = !this.archived_open;
                            this.reload_archived_sessions();
                            cx.notify();
                        }),
                    ),
            );
        if !open {
            return section.into_any_element();
        }
        if self.archived_sessions.is_empty() {
            section = section.child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(theme.TEXT_FAINT)
                    .child("No archived sessions"),
            );
        }
        let list = div()
            .id("sidebar-archived-list")
            .max_h(px(240.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(self.archived_sessions.iter().enumerate().map(|(index, session)| {
                let id = session.session_id.clone();
                let title = session
                    .title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| session.session_id.clone());
                let folder = session
                    .working_dir
                    .as_deref()
                    .and_then(|dir| Path::new(dir).file_name())
                    .map(|name| name.to_string_lossy().into_owned());
                div()
                    .id(("sidebar-archived-row", index))
                    .debug_selector(move || format!("sidebar-archived-row-{index}"))
                    .h(px(26.0))
                    .pl_3()
                    .pr_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .text_size(px(12.0))
                    .text_color(theme.TEXT_DIM)
                    .hover(|el| el.bg(theme.TOOL_BG))
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .children(folder.map(|folder| {
                        div()
                            .flex_none()
                            .max_w(px(70.0))
                            .truncate()
                            .text_size(px(10.0))
                            .text_color(theme.TEXT_FAINT)
                            .child(folder)
                    }))
                    .child(
                        div()
                            .id(("sidebar-archived-restore", index))
                            .debug_selector(move || format!("sidebar-archived-restore-{index}"))
                            .flex_none()
                            .px_2()
                            .py(px(1.0))
                            .rounded_full()
                            .cursor_pointer()
                            .bg(theme.TOOL_BG)
                            .text_size(px(10.0))
                            .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
                            .child("Restore")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    this.set_session_archived(&id, false, window, cx);
                                }),
                            ),
                    )
            }));
        section.child(list).into_any_element()
    }

    /// "Session archived" pill with an Undo action, near the bottom.
    pub(super) fn render_archive_notice(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (session_id, at) = self.archive_notice.clone()?;
        if at.elapsed() > ARCHIVE_NOTICE {
            self.archive_notice = None;
            return None;
        }
        let remaining = ARCHIVE_NOTICE - at.elapsed();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(remaining).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        let theme = Theme::global();
        Some(
            div()
                .debug_selector(|| "archive-notice".into())
                .absolute()
                .bottom(px(16.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .occlude()
                        .pl_4()
                        .pr_1()
                        .py_1()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded_full()
                        .bg(theme.PANEL_BG)
                        .border_1()
                        .border_color(theme.PANEL_BORDER)
                        .shadow_md()
                        .text_size(px(12.0))
                        .child("Session archived")
                        .child(
                            div()
                                .id("archive-undo")
                                .debug_selector(|| "archive-undo".into())
                                .px_3()
                                .py(px(2.0))
                                .rounded_full()
                                .cursor_pointer()
                                .bg(theme.TOOL_BG)
                                .hover(|el| el.bg(theme.ACCENT_DIM))
                                .child("Undo")
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        this.set_session_archived(&session_id, false, window, cx);
                                    }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn archived_section_lists_sessions_and_restores_one(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-archived-row-0").is_none());
        let toggle = vcx.debug_bounds("sidebar-archived-toggle").unwrap();
        vcx.simulate_click(toggle.center(), gpui::Modifiers::none());
        workspace.update(vcx, |w, cx| {
            let mut archived =
                crate::workspace::tests::session_info("session_fox_gone", Some("gone"));
            archived.archived = true;
            w.archived_sessions = vec![archived];
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-archived-row-0").is_some());
        while commands.try_recv().is_ok() {}

        let restore = vcx.debug_bounds("sidebar-archived-restore-0").unwrap();
        vcx.simulate_click(restore.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        let sent = std::iter::from_fn(|| commands.try_recv().ok()).find_map(|c| match c {
            Command::SetSessionArchived {
                session_id,
                archived,
            } => Some((session_id, archived)),
            _ => None,
        });
        assert_eq!(sent, Some(("session_fox_gone".into(), false)));
        assert!(vcx.debug_bounds("sidebar-archived-row-0").is_none());
    }

    #[gpui::test]
    fn archive_from_the_row_menu_hides_the_session_and_undo_restores_it(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w
        });
        workspace.update(vcx, |w, cx| {
            w.sidebar_closed_sessions
                .extend(["session_owl_keep".into(), "session_fox_old".into()]);
            let mut keep = crate::workspace::tests::session_info("session_owl_keep", Some("keep"));
            keep.updated_at_ms = Some(2_000);
            let mut old = crate::workspace::tests::session_info("session_fox_old", Some("old"));
            old.updated_at_ms = Some(1_000);
            w.apply(
                Update::Sessions {
                    sessions: vec![keep, old],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        while commands.try_recv().is_ok() {}

        let row = vcx.debug_bounds("sidebar-session-1").unwrap();
        vcx.simulate_mouse_move(row.center(), None, gpui::Modifiers::none());
        vcx.run_until_parked();
        let more = vcx.debug_bounds("sidebar-collection-button-1").unwrap();
        vcx.simulate_click(more.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        let archive = vcx
            .debug_bounds("sidebar-session-archive")
            .expect("the row menu offers Archive");
        vcx.simulate_click(archive.center(), gpui::Modifiers::none());
        vcx.run_until_parked();

        let sent = std::iter::from_fn(|| commands.try_recv().ok()).find_map(|c| match c {
            Command::SetSessionArchived {
                session_id,
                archived,
            } => Some((session_id, archived)),
            _ => None,
        });
        assert_eq!(sent, Some(("session_fox_old".into(), true)));
        workspace.read_with(vcx, |w, _| {
            assert!(!w.sessions.iter().any(|s| s.session_id == "session_fox_old"));
            assert!(w.sessions.iter().any(|s| s.session_id == "session_owl_keep"));
        });

        let undo = vcx
            .debug_bounds("archive-undo")
            .expect("archiving shows an Undo pill");
        vcx.simulate_click(undo.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        let sent = std::iter::from_fn(|| commands.try_recv().ok()).find_map(|c| match c {
            Command::SetSessionArchived {
                session_id,
                archived,
            } => Some((session_id, archived)),
            _ => None,
        });
        assert_eq!(sent, Some(("session_fox_old".into(), false)));
        assert!(vcx.debug_bounds("archive-notice").is_none());
    }
}
