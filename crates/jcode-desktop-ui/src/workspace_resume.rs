//! A transient, keyboard-first session browser. It never becomes a runtime slot.
use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Active,
    Saved,
}

pub(super) struct State {
    pub(super) search: Entity<PromptInput>,
    query: String,
    selected: Option<String>,
    filter: Filter,
    scroll: ScrollHandle,
}

pub(crate) fn is_resume_command(content: &str) -> bool {
    matches!(content.trim(), "/resume" | "/sessions" | "/session")
}

fn filtered_sessions(
    mut sessions: Vec<jcode_sdk::SessionInfo>,
    query: &str,
    filter: Filter,
) -> Vec<jcode_sdk::SessionInfo> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    sessions.retain(|session| {
        let text = format!(
            "{} {} {} {} {}",
            session.session_id,
            session.title.as_deref().unwrap_or_default(),
            session.working_dir.as_deref().unwrap_or_default(),
            session.status,
            session.agent_label.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        !session.archived
            && match filter {
                Filter::All => true,
                Filter::Active => matches!(
                    session.status.as_str(),
                    "running" | "working" | "busy" | "streaming"
                ),
                Filter::Saved => session.saved,
            }
            && words.iter().all(|word| text.contains(word))
    });
    sessions.sort_by(|a, b| {
        b.saved
            .cmp(&a.saved)
            .then_with(|| {
                b.last_active_at_ms
                    .or(b.updated_at_ms)
                    .cmp(&a.last_active_at_ms.or(a.updated_at_ms))
            })
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    let mut seen = HashSet::new();
    sessions.retain(|session| seen.insert(session.session_id.clone()));
    sessions
}

impl Workspace {
    fn resume_sessions(&self, cx: &App) -> Vec<jcode_sdk::SessionInfo> {
        let mut sessions = self.sessions.clone();
        // Keep local drafts and already-open chats reachable when switching the
        // standalone window's primary conversation. Never discard their editors.
        for slot in &self.slots {
            let panel = slot.panel.read(cx);
            if !slot.closing
                && panel.supports_voice()
                && !sessions.iter().any(|s| s.session_id == panel.session_id)
            {
                sessions.push(jcode_sdk::SessionInfo {
                    session_id: panel.session_id.clone(),
                    title: Some(panel.title.to_string()),
                    working_dir: panel.working_dir.clone(),
                    status: "open".into(),
                    transcript_bytes: None,
                    saved: false,
                    updated_at_ms: None,
                    last_active_at_ms: None,
                    archived: false,
                    archived_at_ms: None,
                    parent_session_id: None,
                    agent_label: None,
                    swarm_status: None,
                    edit_stats: None,
                });
            }
        }
        sessions
    }

    fn resume_matches(&self, cx: &App) -> Vec<jcode_sdk::SessionInfo> {
        let Some(state) = &self.resume else {
            return Vec::new();
        };
        filtered_sessions(self.resume_sessions(cx), &state.query, state.filter)
    }

    pub(super) fn open_resume(
        &mut self,
        _: &OpenResume,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = &self.resume {
            window.focus(&state.search.focus_handle(cx), cx);
            return;
        }
        let submit = cx.weak_entity();
        let change = submit.clone();
        let search = cx.new(|cx| {
            PromptInput::new(
                cx,
                "Search sessions by title, folder, or ID…",
                move |_, _, window, app| {
                    let _ = submit.update(app, |this, cx| this.resume_selected(window, cx));
                },
            )
            .with_on_change(move |query, app| {
                let _ = change.update(app, |this, cx| {
                    if let Some(state) = &mut this.resume {
                        state.query = query.to_owned();
                        state.selected = None;
                        state.scroll.scroll_to_item(0);
                    }
                    cx.notify();
                });
            })
        });
        window.focus(&search.focus_handle(cx), cx);
        self.focus_pending = false;
        self.resume = Some(State {
            search,
            query: String::new(),
            selected: None,
            filter: Filter::All,
            scroll: ScrollHandle::new(),
        });
        cx.notify();
    }

    fn close_resume(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resume = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn resume_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.resume_matches(cx);
        let selected = self.resume.as_ref().and_then(|s| s.selected.as_deref());
        let Some(session) = matches
            .iter()
            .find(|s| Some(s.session_id.as_str()) == selected)
            .or(matches.first())
            .cloned()
        else {
            return;
        };
        self.resume = None;
        self.activate_session(session, window, cx);
        if self.single_panel {
            // Slot zero is the standalone chat anchor. Utilities must return to
            // the resumed chat, not resurrect the previous chat as a back page.
            self.slots.swap(0, self.active);
            self.active = 0;
            self.active_row = 0;
            self.show_sidebar = false;
            self.overview = false;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn resume_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => self.close_resume(window, cx),
            "enter" => self.resume_selected(window, cx),
            "1" | "2" | "3" if event.keystroke.modifiers.control => {
                self.set_resume_filter(
                    match key {
                        "2" => Filter::Active,
                        "3" => Filter::Saved,
                        _ => Filter::All,
                    },
                    cx,
                );
            }
            "up" | "down" | "pageup" | "pagedown" => self.move_resume_selection(key, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn move_resume_selection(&mut self, key: &str, cx: &mut Context<Self>) {
        let matches = self.resume_matches(cx);
        if let Some(state) = &mut self.resume {
            let current = matches
                .iter()
                .position(|s| Some(&s.session_id) == state.selected.as_ref())
                .unwrap_or(0);
            let next = match key {
                "up" => current.saturating_sub(1),
                "pageup" => current.saturating_sub(8),
                "pagedown" => (current + 8).min(matches.len().saturating_sub(1)),
                _ => (current + 1).min(matches.len().saturating_sub(1)),
            };
            state.selected = matches.get(next).map(|s| s.session_id.clone());
            state.scroll.scroll_to_item(next);
            cx.notify();
        }
    }

    fn set_resume_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.resume {
            state.filter = filter;
            state.selected = None;
            state.scroll.scroll_to_item(0);
        }
        cx.notify();
    }

    pub(super) fn render_resume(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let matches = self.resume_matches(cx);
        let state = self.resume.as_mut().expect("resume picker is open");
        if !matches
            .iter()
            .any(|s| Some(&s.session_id) == state.selected.as_ref())
        {
            state.selected = matches.first().map(|s| s.session_id.clone());
        }
        let selected = state.selected.clone();
        let search = state.search.clone();
        let scroll = state.scroll.clone();
        let filter = state.filter;
        let theme = Theme::global();
        let mut rows = div()
            .id("resume-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll);
        for (index, session) in matches.iter().enumerate() {
            let id = session.session_id.clone();
            let is_selected = selected.as_ref() == Some(&id);
            let (icon, title) = sidebar_session_title(session);
            let metadata = format!(
                "{}{} · {}",
                if session.saved { "★ " } else { "" },
                session.status,
                sidebar_session_meta(session).unwrap_or_else(|| "Session history".into())
            );
            rows = rows.child(
                div()
                    .id(("resume-row", index))
                    .debug_selector(move || format!("resume-row-{index}"))
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .rounded_md()
                    .when(is_selected, |el| el.bg(theme.TOOL_BG))
                    .hover(|el| el.bg(theme.TOOL_BG))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(state) = &mut this.resume {
                            state.selected = Some(id.clone());
                        }
                        this.resume_selected(window, cx);
                    }))
                    .child(div().text_ellipsis().child(format!("{icon} {title}")))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(theme.TEXT_DIM)
                            .text_ellipsis()
                            .child(
                                session
                                    .working_dir
                                    .as_deref()
                                    .map(compact_working_dir)
                                    .unwrap_or_else(|| "No working directory".into()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.TEXT_FAINT)
                            .child(metadata),
                    ),
            );
        }
        if matches.is_empty() {
            rows = rows.child(div().p_4().text_color(theme.TEXT_DIM).child(
                if self.sessions.is_empty() {
                    "No sessions available yet."
                } else {
                    "No matching sessions. Try another search or filter."
                },
            ));
        }
        let mut preview = div()
            .id("resume-preview")
            .min_h_0()
            .overflow_y_scroll()
            .debug_selector(|| "resume-preview".into())
            .flex_1()
            .min_w_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme.TOOL_BG)
            .rounded_lg();
        if let Some(session) = matches
            .iter()
            .find(|s| Some(&s.session_id) == selected.as_ref())
        {
            let (_, title) = sidebar_session_title(session);
            preview =
                preview
                    .child(div().text_size(px(18.0)).child(title))
                    .child(
                        div().text_color(theme.TEXT_DIM).text_ellipsis().child(
                            session
                                .working_dir
                                .as_deref()
                                .map(compact_working_dir)
                                .unwrap_or_else(|| "No working directory".into()),
                        ),
                    )
                    .child(format!(
                        "Status: {}{}",
                        session.status,
                        if session.saved { " · Saved" } else { "" }
                    ))
                    .children(sidebar_session_meta(session))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.TEXT_DIM)
                            .text_ellipsis()
                            .child(session.session_id.clone()),
                    )
                    .child(div().text_size(px(12.0)).text_color(theme.TEXT_DIM).child(
                        "Press Enter to resume this conversation. Your current draft is kept.",
                    ));
        } else {
            preview = preview.child("Select a session to see its details.");
        }
        div()
            .debug_selector(|| "resume-panel".into())
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme.PANEL_BG)
            .text_color(theme.TEXT)
            .font_family(theme.FONT_UI)
            .text_size(px(14.0))
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(Self::resume_key_down))
            .capture_action(cx.listener(|this, _: &crate::input::Clear, window, cx| {
                this.close_resume(window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &crate::input::Submit, window, cx| {
                this.resume_selected(window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &crate::input::HistoryPrev, _, cx| {
                this.move_resume_selection("up", cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &crate::input::HistoryNext, _, cx| {
                this.move_resume_selection("down", cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_size(px(22.0)).child("Resume session"))
                    .child(
                        div()
                            .id("resume-close")
                            .debug_selector(|| "resume-close".into())
                            .cursor_pointer()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .hover(|el| el.bg(theme.TOOL_BG))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_resume(window, cx)),
                            )
                            .child("Back · Esc"),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "resume-search".into())
                    .child(search),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .children(
                        [
                            (Filter::All, "All", "resume-all"),
                            (Filter::Active, "Active", "resume-active"),
                            (Filter::Saved, "Saved", "resume-saved"),
                        ]
                        .into_iter()
                        .map(|(value, label, id)| {
                            div()
                                .id(id)
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .when(value == filter, |el| el.bg(theme.TOOL_BG))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_resume_filter(value, cx)
                                }))
                                .child(label)
                        }),
                    )
                    .child(
                        div()
                            .text_color(theme.TEXT_DIM)
                            .child(format!("{} sessions", matches.len())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(relative(0.55))
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(rows),
                    )
                    .child(preview),
            )
            .child(div().text_size(px(11.0)).text_color(theme.TEXT_DIM).child(
                "↑ ↓ Navigate   Page Up / Down   Enter Resume   Esc Back   Ctrl+1/2/3 Filters",
            ))
    }

    pub(super) fn init_resume_fixture(&mut self) {
        if !harness::screenshot_mode()
            || std::env::var_os("JCODE_DESKTOP_SCREENSHOT_RESUME_FIXTURE").is_none()
        {
            return;
        }
        let Some(base) = self.sessions.first().cloned() else {
            return;
        };
        for (index, name) in ["alpha", "beta"].into_iter().enumerate() {
            let mut session = base.clone();
            session.session_id = format!("resume-fixture-{name}");
            session.title = Some(format!("Resume acceptance {name}"));
            session.working_dir = Some(format!("/workspace/resume-{name}"));
            session.saved = index == 0;
            session.status = if index == 1 { "running" } else { "idle" }.into();
            session.transcript_bytes = Some(16_800);
            session.updated_at_ms = Some(100 - index as i64);
            self.sessions.push(session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::session_info;

    #[test]
    fn resume_search_is_case_insensitive_multiword_and_handles_unicode() {
        let mut alpha = session_info("alpha-123", Some("Résumé planner"));
        alpha.working_dir = Some("/workspace/Client".into());
        let beta = session_info("beta", Some("Other task"));
        let sessions = vec![alpha.clone(), beta];
        for query in ["  RÉSUMÉ   client ", "alpha-123", "PLANNER"] {
            assert_eq!(
                filtered_sessions(sessions.clone(), query, Filter::All),
                vec![alpha.clone()]
            );
        }
        assert!(filtered_sessions(sessions, "not-found", Filter::All).is_empty());
    }

    #[test]
    fn resume_filters_sort_saved_then_recent_exclude_archived_and_deduplicate() {
        let mut saved = session_info("saved", Some("Saved"));
        saved.saved = true;
        saved.updated_at_ms = Some(1);
        let mut running = session_info("running", None);
        running.status = "running".into();
        running.updated_at_ms = Some(20);
        let mut duplicate = running.clone();
        duplicate.updated_at_ms = Some(2);
        let mut archived = session_info("archived", None);
        archived.archived = true;
        let sessions = vec![duplicate, saved.clone(), archived, running.clone()];
        assert_eq!(
            filtered_sessions(sessions.clone(), "", Filter::All),
            vec![saved.clone(), running.clone()]
        );
        assert_eq!(
            filtered_sessions(sessions.clone(), "", Filter::Active),
            vec![running]
        );
        assert_eq!(filtered_sessions(sessions, "", Filter::Saved), vec![saved]);
        assert!(is_resume_command(" /session \n"));
        assert!(is_resume_command("/resume"));
        assert!(is_resume_command("/sessions"));
        assert!(!is_resume_command("/resumeall"));
        assert!(!is_resume_command("/resume unrelated"));
    }

    #[gpui::test]
    fn resume_cancel_preserves_draft_and_never_watches_a_picker(cx: &mut gpui::TestAppContext) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.single_panel = true;
            workspace.bridge = bridge;
            workspace.open_startup_draft(cx);
            workspace
        });
        workspace.update_in(vcx, |w, window, cx| {
            let input = w.slots[0].panel.read(cx).input.clone();
            input.update(cx, |input, cx| {
                let mut draft = input.snapshot();
                draft.content = "keep this composer draft".into();
                input.restore(draft, cx);
            });
            w.open_resume(&OpenResume, window, cx);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("resume-panel").is_some());
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert!(w.resume.is_none());
            assert_eq!(w.slots.len(), 1);
            assert_eq!(
                w.slots[0].panel.read(cx).input.read(cx).snapshot().content,
                "keep this composer draft"
            );
        });
        assert!(
            commands.try_recv().is_err(),
            "picker must not Watch/Unwatch a runtime session"
        );
    }

    #[gpui::test]
    fn resume_single_panel_reuses_entities_preserves_drafts_and_primary_anchor(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.single_panel = true;
            w.open_startup_draft(cx);
            w.sessions = vec![session_info("target", Some("Resumed target"))];
            w
        });
        let original = workspace.read_with(vcx, |w, _| w.slots[0].panel.clone());
        workspace.update_in(vcx, |w, window, cx| {
            w.open_resume(&OpenResume, window, cx);
            w.resume.as_mut().unwrap().selected = Some("target".into());
            w.resume_selected(window, cx);
            assert!(w.single_panel);
            assert_eq!(w.active, 0);
            assert_eq!(w.slots[0].panel.read(cx).session_id, "target");
            assert_eq!(w.slots.len(), 2);
            assert!(
                w.resume_sessions(cx)
                    .iter()
                    .any(|s| s.session_id == original.read(cx).session_id)
            );
            let target = w.slots[0].panel.clone();
            w.open_resume(&OpenResume, window, cx);
            w.resume.as_mut().unwrap().selected = Some("target".into());
            w.resume_selected(window, cx);
            assert_eq!(w.slots[0].panel, target);
            assert_eq!(w.slots.len(), 2);
            w.open_resume(&OpenResume, window, cx);
            w.resume.as_mut().unwrap().selected = Some(original.read(cx).session_id.clone());
            w.resume_selected(window, cx);
            assert_eq!(w.slots[0].panel, original);
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("single-panel-root").is_some());
        assert!(vcx.debug_bounds("single-panel-back").is_none());
        assert!(vcx.debug_bounds("sidebar").is_none());
        assert!(vcx.debug_bounds("workspace-canvas").is_none());
    }

    #[gpui::test]
    fn resume_keyboard_navigation_search_empty_and_catalog_refresh_are_safe(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.single_panel = true;
            w.open_startup_draft(cx);
            w.sessions = vec![
                session_info("a", Some("Alpha")),
                session_info("b", Some("Beta")),
            ];
            w
        });
        workspace.update_in(vcx, |w, window, cx| w.open_resume(&OpenResume, window, cx));
        vcx.run_until_parked();
        vcx.simulate_keystrokes("down");
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.resume.as_ref().unwrap().selected.as_deref(), Some("b"))
        });
        vcx.simulate_keystrokes("up");
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.resume.as_ref().unwrap().selected.as_deref(), Some("a"))
        });
        vcx.simulate_input("no such session");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("pagedown enter");
        workspace.read_with(vcx, |w, _| assert!(w.resume.is_some()));
        vcx.simulate_keystrokes("ctrl-a");
        vcx.simulate_input("Beta");
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            w.sessions.clear();
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        workspace.read_with(vcx, |w, _| {
            assert!(
                w.resume.is_some(),
                "stale selection must not resume removed session"
            )
        });
    }
    #[gpui::test]
    fn resume_aliases_open_locally_from_startup_in_both_modes(cx: &mut gpui::TestAppContext) {
        for single_panel in [false, true] {
            let (workspace, vcx) = cx.add_window_view(|_, cx| {
                let mut w = Workspace::for_test(learning::Coach::new(), cx);
                w.single_panel = single_panel;
                w.open_startup_draft(cx);
                w
            });
            for command in ["/resume", "/sessions", "/session"] {
                workspace.update_in(vcx, |w, window, cx| w.focus_active(window, cx));
                vcx.run_until_parked();
                vcx.simulate_input(command);
                vcx.simulate_keystrokes("enter");
                vcx.run_until_parked();
                workspace.read_with(vcx, |w, _| {
                    assert!(w.resume.is_some(), "{command} should open locally");
                    assert_eq!(w.single_panel, single_panel);
                });
                vcx.simulate_keystrokes("escape");
                vcx.run_until_parked();
            }
        }
    }
}
