//! Git checkout navigation. Swarms remain the default and are never reconfigured.
use super::*;
use jcode_sdk::worktrees::Worktree;

const FOLDER_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7Zm0 3h18" fill="none" stroke="black" stroke-width="1.6" stroke-linejoin="round"/></svg>"#;
const BRANCH_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="black" stroke-width="1.6"><circle cx="6" cy="5" r="2.5"/><circle cx="18" cy="5" r="2.5"/><circle cx="6" cy="19" r="2.5"/><path d="M6 7.5v9M18 7.5c0 6-12 2-12 8"/></g></svg>"#;

#[derive(Default)]
pub(super) struct State {
    directory: Option<String>,
    entries: Vec<Worktree>,
    loading: bool,
    creating: bool,
    error: Option<String>,
    input: Option<Entity<PromptInput>>,
    input_directory: Option<String>,
    scroll: ScrollHandle,
    listing: Option<gpui::Task<()>>,
    creation: Option<gpui::Task<()>>,
    collapsed: HashSet<String>,
}

struct WorktreeTooltip(String);

impl Render for WorktreeTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .max_w(px(420.0))
            .rounded_md()
            .bg(Theme::global().HEADER_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_size(px(11.0))
            .text_color(Theme::global().TEXT_DIM)
            .child(self.0.clone())
    }
}

fn checkout_label(entry: &Worktree) -> String {
    entry
        .branch
        .as_deref()
        .map(|branch| {
            branch
                .strip_prefix("refs/heads/")
                .unwrap_or(branch)
                .to_owned()
        })
        .unwrap_or_else(|| {
            if entry.bare {
                "Bare repository".into()
            } else {
                format!(
                    "Detached · {}",
                    entry.head.chars().take(8).collect::<String>()
                )
            }
        })
}

fn checkout_available(entry: &Worktree) -> bool {
    !entry.bare && entry.prunable.is_none()
}

// Longest component prefix prevents sibling names and nested worktrees from
// assigning a session to the wrong checkout. Remote paths are never local Git.
fn owner<'a>(directory: &str, entries: &'a [Worktree]) -> Option<&'a str> {
    entries
        .iter()
        .filter(|entry| !entry.bare)
        .filter(|entry| Path::new(directory).starts_with(&entry.path))
        .max_by_key(|entry| Path::new(&entry.path).components().count())
        .map(|entry| entry.path.as_str())
}

fn local_conversation(panel: &Panel) -> bool {
    panel.is_pending_session() || (panel.can_fork() && !panel.session_id.contains("://"))
}

impl Workspace {
    fn worktree_chat_catalog(&self, cx: &App) -> Vec<jcode_sdk::SessionInfo> {
        let mut catalog = self.sessions.clone();
        let mut seen = catalog
            .iter()
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        // A new chat has a panel immediately, before CreateSession completes.
        // Keep it navigable here without inserting a provisional ID into history.
        for slot in self.slots.iter().filter(|slot| !slot.closing) {
            let panel = slot.panel.read(cx);
            if !local_conversation(panel) || !seen.insert(panel.session_id.clone()) {
                continue;
            }
            catalog.push(jcode_sdk::SessionInfo {
                session_id: panel.session_id.clone(),
                working_dir: panel.working_dir.clone(),
                title: Some(panel.title.to_string()),
                status: panel.status.clone(),
                transcript_bytes: None,
                saved: false,
                updated_at_ms: None,
                last_active_at_ms: None,
                archived: false,
                archived_at_ms: None,
                save_label: None,
                parent_session_id: None,
                agent_label: None,
                swarm_status: None,
                edit_stats: None,
            });
        }
        sidebar_session_order(&catalog)
    }

    pub(super) fn init_worktree_fixture(&mut self) {
        if !harness::screenshot_mode()
            || std::env::var_os("JCODE_DESKTOP_SCREENSHOT_WORKTREES").is_none()
        {
            return;
        }
        self.worktree_mode = true;
        self.worktrees.directory = Some("/workspace/example".into());
        self.worktrees.entries = [
            ("/workspace/example", "main"),
            (
                "/workspace/example-worktrees/feature%2Fsearch",
                "feature/search",
            ),
            ("/workspace/example-worktrees/fix%2Fsidebar", "fix/sidebar"),
        ]
        .into_iter()
        .map(|(path, branch)| Worktree {
            path: path.into(),
            branch: Some(format!("refs/heads/{branch}")),
            head: "abc12345678".into(),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        })
        .collect();
        let mut session = self.sessions[0].clone();
        session.session_id = "worktree-fixture-search".into();
        session.title = Some("Build project search".into());
        session.working_dir = Some(self.worktrees.entries[1].path.clone());
        session.updated_at_ms = Some(unix_now_ms() - 120_000);
        self.sessions.push(session.clone());
        session.session_id = "worktree-fixture-index".into();
        session.title = Some("Index workspace files".into());
        session.updated_at_ms = Some(unix_now_ms() - 7_200_000);
        self.sessions.push(session);
    }

    fn worktree_directory(&self, cx: &App) -> Option<String> {
        if let Some(slot) = self
            .slots
            .get(self.active)
            .filter(|s| !s.closing && s.row == self.active_row)
        {
            let panel = slot.panel.read(cx);
            if !local_conversation(panel) {
                return None;
            }
            if let Some(directory) = &panel.working_dir {
                return Some(directory.clone());
            }
        }
        if self.remotes.default_host.is_some() {
            None
        } else {
            self.pinned_working_dir.clone()
        }
    }

    pub(super) fn render_workflow_switch(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .id("sidebar-workflow-switch")
            .debug_selector(|| "sidebar-workflow-switch".into())
            .flex_none()
            .flex()
            .gap_1()
            .mr_3()
            .children(
                [
                    (
                        false,
                        "Swarm",
                        "sidebar-mode-swarm",
                        include_bytes!("../../../assets/icons/swarm.svg").as_slice(),
                    ),
                    (
                        true,
                        "Worktrees",
                        "sidebar-mode-worktrees",
                        include_bytes!("../../../assets/icons/worktree.svg").as_slice(),
                    ),
                ]
                .into_iter()
                .map(|(mode, label, id, icon)| {
                    let ink = if mode == self.worktree_mode {
                        Theme::global().ACCENT
                    } else {
                        Theme::global().TEXT_DIM
                    };
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .flex_none()
                        .size(px(28.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .tooltip(move |_, cx| {
                            cx.new(|_| WorktreeTooltip(format!("{label} mode"))).into()
                        })
                        .when(mode == self.worktree_mode, |el| {
                            el.bg(Theme::global().ACCENT_DIM)
                        })
                        .hover(|el| el.bg(Theme::global().TOOL_BG))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.worktree_mode = mode;
                            this.worktrees.input = None;
                            this.worktrees.input_directory = None;
                            this.focus_pending = true;
                            cx.notify();
                        }))
                        .child(gpui::svg().data(icon).size(px(16.0)).text_color(ink))
                }),
            )
            .into_any_element()
    }

    fn refresh_worktrees(&mut self, directory: String, cx: &mut Context<Self>) {
        if self.worktrees.creating {
            return;
        }
        self.worktrees.directory = Some(directory.clone());
        self.worktrees.entries.clear();
        self.worktrees.loading = true;
        self.worktrees.error = None;
        let task = cx.background_executor().spawn(async move {
            let result =
                jcode_sdk::worktrees::list(&directory).map_err(|error| format!("{error:#}"));
            (directory, result)
        });
        self.worktrees.listing = Some(cx.spawn(async move |this, cx| {
            let (directory, result) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.worktrees.directory.as_ref() != Some(&directory) {
                    return;
                }
                this.worktrees.loading = false;
                match result {
                    Ok(entries) => this.worktrees.entries = entries,
                    Err(error) => this.worktrees.error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    fn open_worktree(&mut self, path: String, cx: &mut Context<Self>) {
        let existing = self.slots.iter().position(|slot| {
            let panel = slot.panel.read(cx);
            !slot.closing
                && local_conversation(panel)
                && panel
                    .working_dir
                    .as_deref()
                    .and_then(|dir| owner(dir, &self.worktrees.entries))
                    == Some(path.as_str())
        });
        if let Some(index) = existing {
            self.set_active(index, cx);
            self.overview = false;
            self.overview_progress.set(0.0, Instant::now());
            self.focus_pending = true;
            cx.notify();
        } else {
            self.open_local_draft(Some(path), cx);
        }
    }

    fn begin_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktrees.loading || self.worktrees.creating || self.worktrees.entries.is_empty() {
            return;
        }
        let Some(directory) = self.worktree_directory(cx) else {
            return;
        };
        let weak = cx.weak_entity();
        let cancel = weak.clone();
        let input = cx.new(|cx| {
            PromptInput::new(
                cx,
                "Branch name, e.g. feature/search",
                move |branch, _, _, app| {
                    let _ = weak.update(app, |this, cx| this.create_worktree(&branch, cx));
                },
            )
            .with_on_overlay_cancel(move |app| {
                let _ = cancel.update(app, |this, cx| {
                    if !this.worktrees.creating {
                        this.worktrees.input = None;
                        this.worktrees.input_directory = None;
                        this.focus_pending = true;
                        cx.notify();
                    }
                });
                true
            })
        });
        window.focus(&input.read(cx).focus_handle.clone(), cx);
        self.focus_pending = false;
        self.worktrees.input = Some(input);
        self.worktrees.input_directory = Some(directory);
        self.worktrees.error = None;
        cx.notify();
    }

    fn create_worktree(&mut self, branch: &str, cx: &mut Context<Self>) {
        if self.worktrees.creating {
            return;
        }
        let Some(directory) = self.worktrees.input_directory.clone() else {
            return;
        };
        let branch = branch.trim().to_owned();
        if branch.is_empty() {
            self.worktrees.error = Some("Enter a new branch name.".into());
            cx.notify();
            return;
        }
        self.worktrees.creating = true;
        self.worktrees.error = None;
        let origin = directory.clone();
        let task = cx.background_executor().spawn(async move {
            jcode_sdk::worktrees::create(&directory, &branch).map_err(|error| format!("{error:#}"))
        });
        self.worktrees.creation = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.worktrees.creating = false;
                match result {
                    Ok(entry) => {
                        this.worktrees.input = None;
                        this.worktrees.input_directory = None;
                        // The filesystem operation completes even if navigation changed,
                        // but must not steal focus from a different repository/view.
                        let still_here = this.worktree_mode
                            && this.sidebar_view == SidebarView::Sessions
                            && this.worktree_directory(cx).as_ref() == Some(&origin);
                        this.worktrees.directory = None;
                        if still_here {
                            this.open_local_draft(Some(entry.path), cx);
                        }
                    }
                    Err(error) => this.worktrees.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn render_worktrees(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let directory = self.worktree_directory(cx);
        // Switching to a known checkout keeps the list steady. Unknown paths
        // still query Git, since a subdirectory may be a separate nested repo.
        let known_checkout = self
            .worktrees
            .entries
            .iter()
            .any(|entry| !entry.bare && Some(&entry.path) == directory.as_ref());
        if directory != self.worktrees.directory && !self.worktrees.creating {
            self.worktrees.input = None;
            self.worktrees.input_directory = None;
            if known_checkout {
                self.worktrees.directory = directory.clone();
            } else if let Some(directory) = &directory {
                self.refresh_worktrees(directory.clone(), cx);
            } else {
                self.worktrees = State::default();
            }
        }
        let active_path = directory
            .as_deref()
            .and_then(|dir| owner(dir, &self.worktrees.entries))
            .map(str::to_owned);
        let mut body = div()
            .id("sidebar-worktrees")
            .debug_selector(|| "sidebar-worktrees".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .pb_2()
            .text_size(px(12.0));
        if directory.is_none() {
            return body.child(div().text_color(Theme::global().TEXT_DIM)
                .child("Open a local Git project to use worktrees. Remote worktrees are not supported yet.")).into_any_element();
        }
        let project_path = self
            .worktrees
            .entries
            .first()
            .map(|entry| entry.path.as_str())
            .or(directory.as_deref())
            .unwrap_or_default();
        let project_name = Path::new(project_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| project_path.to_owned());
        let project_detail = format!(
            "{} · Separate checkouts, each with its own chats",
            compact_working_dir(project_path)
        );
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("worktree-project")
                        .debug_selector(|| "worktree-project".into())
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .text_color(Theme::global().TEXT_DIM)
                        .tooltip(move |_, cx| {
                            cx.new(|_| WorktreeTooltip(project_detail.clone())).into()
                        })
                        .child(project_name),
                )
                .child(
                    div()
                        .id("worktree-refresh")
                        .debug_selector(|| "worktree-refresh".into())
                        .cursor_pointer()
                        .px_1()
                        .py_1()
                        .text_color(Theme::global().TEXT_DIM)
                        .tooltip(|_, cx| {
                            cx.new(|_| WorktreeTooltip("Refresh worktrees".into()))
                                .into()
                        })
                        .child("↻")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.worktrees.loading && !this.worktrees.creating {
                                if let Some(dir) = this.worktree_directory(cx) {
                                    this.refresh_worktrees(dir, cx);
                                }
                            }
                        })),
                )
                .child(
                    div()
                        .id("worktree-new")
                        .debug_selector(|| "worktree-new".into())
                        .cursor_pointer()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(Theme::global().TOOL_BG)
                        .text_size(px(11.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(if self.worktrees.creating {
                            "Creating…"
                        } else {
                            "+ Worktree"
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.begin_worktree(window, cx)),
                        ),
                ),
        );
        if let Some(input) = &self.worktrees.input {
            body = body.child(div().id("worktree-create-form").debug_selector(|| "worktree-create-form".into())
                .flex().flex_col().gap_2()
                .capture_action(cx.listener(|this, _: &crate::input::Submit, _, cx| {
                    cx.stop_propagation();
                    if let Some(input) = &this.worktrees.input {
                        let branch = input.read(cx).content.to_string();
                        this.create_worktree(&branch, cx);
                    }
                }))
                .child(input.clone())
                .child(div().text_size(px(10.0)).text_color(Theme::global().TEXT_DIM)
                    .child("New branch from HEAD. Uncommitted edits stay here. Enter to create · Esc to cancel.")));
        }
        if let Some(error) = &self.worktrees.error {
            body = body.child(
                div()
                    .id("worktree-error")
                    .debug_selector(|| "worktree-error".into())
                    .text_size(px(11.0))
                    .text_color(Theme::global().ERROR)
                    .child(error.clone()),
            );
        }
        if self.worktrees.loading {
            body = body.child(
                div()
                    .text_color(Theme::global().TEXT_DIM)
                    .child("Loading worktrees…"),
            );
        }
        let mut list = div()
            .id("worktree-list")
            .debug_selector(|| "worktree-list".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.worktrees.scroll)
            .flex()
            .flex_col()
            .gap_2();
        let active_id = self
            .slots
            .get(self.active)
            .filter(|slot| !slot.closing)
            .map(|slot| slot.panel.read(cx).session_id.clone());
        let ordered = self.worktree_chat_catalog(cx);
        for (index, entry) in self.worktrees.entries.iter().enumerate() {
            let path = entry.path.clone();
            let enabled = checkout_available(entry);
            let collapsed = self.worktrees.collapsed.contains(&path);
            let selected = active_path.as_ref() == Some(&path);
            let sessions = ordered
                .iter()
                .filter(|session| {
                    !session.archived
                        && (!session.session_id.contains("://")
                            || self.slots.iter().any(|slot| {
                                let panel = slot.panel.read(cx);
                                !slot.closing
                                    && panel.session_id == session.session_id
                                    && local_conversation(panel)
                            }))
                        && session
                            .working_dir
                            .as_deref()
                            .and_then(|dir| owner(dir, &self.worktrees.entries))
                            == Some(path.as_str())
                })
                .collect::<Vec<_>>();
            let label = checkout_label(entry);
            let detail = format!(
                "{label}\n{}{}",
                compact_working_dir(&path),
                if entry.prunable.is_some() {
                    "\nUnavailable. Prune in Git to remove."
                } else if entry.locked.is_some() {
                    "\nLocked against removal in Git."
                } else {
                    ""
                }
            );
            let toggle_path = path.clone();
            let new_path = path.clone();
            let mut header = div()
                .id(gpui::SharedString::from(format!("worktree-{index}")))
                .debug_selector(move || format!("worktree-{index}"))
                .group("worktree-header")
                .h(px(30.0))
                .flex_none()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .px_1()
                .rounded_md()
                .text_color(if selected {
                    Theme::global().TEXT
                } else {
                    Theme::global().TEXT_DIM
                })
                .when(!enabled, |el| el.opacity(0.55))
                .tooltip(move |_, cx| cx.new(|_| WorktreeTooltip(detail.clone())).into())
                .child(
                    div()
                        .id(gpui::SharedString::from(format!("worktree-toggle-{index}")))
                        .debug_selector(move || format!("worktree-toggle-{index}"))
                        .size(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|el| el.bg(Theme::global().TOOL_BG))
                        .child(if collapsed { "›" } else { "⌄" })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if !this.worktrees.collapsed.remove(&toggle_path) {
                                this.worktrees.collapsed.insert(toggle_path.clone());
                            }
                            cx.notify();
                        })),
                )
                .child(
                    gpui::svg()
                        .data(if index == 0 { FOLDER_ICON } else { BRANCH_ICON })
                        .size(px(14.0))
                        .flex_none()
                        .text_color(Theme::global().TEXT_DIM),
                )
                .child(div().flex_1().min_w_0().text_ellipsis().child(label))
                .when(index == 0 && !entry.bare, |el| {
                    el.child(
                        div()
                            .text_size(px(9.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child("Local"),
                    )
                })
                .when(collapsed && !sessions.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(px(10.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(sessions.len().to_string()),
                    )
                })
                .when(enabled, |el| {
                    el.child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "worktree-new-chat-{index}"
                            )))
                            .debug_selector(move || format!("worktree-new-chat-{index}"))
                            .size(px(22.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .opacity(if selected { 1.0 } else { 0.0 })
                            .group_hover("worktree-header", |el| el.opacity(1.0))
                            .hover(|el| el.bg(Theme::global().TOOL_BG))
                            .tooltip(|_, cx| {
                                cx.new(|_| WorktreeTooltip("New chat in this checkout".into()))
                                    .into()
                            })
                            .child("+")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.worktrees.collapsed.remove(&new_path);
                                this.open_local_draft(Some(new_path.clone()), cx);
                            })),
                    )
                });
            if enabled {
                header = header
                    .cursor_pointer()
                    .hover(|el| el.bg(Theme::global().TOOL_BG))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_worktree(path.clone(), cx)),
                    );
            }
            let mut group = div()
                .flex_none()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(header);
            if !collapsed {
                if entry.locked.is_some() || entry.prunable.is_some() {
                    group = group.child(
                        div()
                            .ml(px(42.0))
                            .text_size(px(10.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(if entry.prunable.is_some() {
                                "Unavailable · prune in Git"
                            } else {
                                "Locked"
                            }),
                    );
                }
                if sessions.is_empty() && enabled {
                    let path = entry.path.clone();
                    group = group.child(
                        div()
                            .id(gpui::SharedString::from(format!("worktree-empty-{index}")))
                            .debug_selector(move || format!("worktree-empty-{index}"))
                            .ml(px(24.0))
                            .px_2()
                            .h(px(28.0))
                            .relative()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .cursor_pointer()
                            .hover(|el| el.bg(Theme::global().TOOL_BG))
                            .child("Start a chat")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_local_draft(Some(path.clone()), cx)
                            })),
                    );
                }
                for session in sessions {
                    let session = session.clone();
                    let open = self
                        .slots
                        .iter()
                        .find(|slot| {
                            !slot.closing && slot.panel.read(cx).session_id == session.session_id
                        })
                        .map(|slot| slot.panel.read(cx));
                    let title = sidebar_session_title_with_open_title(
                        &session,
                        open.map(|panel| panel.title.as_ref()),
                    )
                    .1;
                    let activity = match open {
                        Some(panel) => panel.sidebar_mark(),
                        None => self.daemon_running_mark(&session.session_id),
                    };
                    let focused = active_id.as_deref() == Some(session.session_id.as_str());
                    let timestamp = sidebar_session_recency_ms(&session);
                    let age = if timestamp > 0 {
                        format_time_ago(timestamp as u64, unix_now_ms().max(0) as u64)
                            .trim_end_matches(" ago")
                            .to_owned()
                    } else {
                        String::new()
                    };
                    let selector = format!("worktree-session-{}", session.session_id);
                    let selection_selector = format!("{selector}-selected");
                    let tooltip = format!("{}\n{}", title, compact_working_dir(&entry.path));
                    group = group.child(
                        div()
                            .id(gpui::SharedString::from(selector.clone()))
                            .debug_selector(move || selector.clone())
                            .h(px(28.0))
                            .relative()
                            .flex_none()
                            .min_w_0()
                            .ml(px(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .when(focused, |el| {
                                el.bg(Theme::global().TOOL_BG)
                                    .text_color(Theme::global().TEXT)
                                    .child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .debug_selector(move || selection_selector.clone()),
                                    )
                            })
                            .tooltip(move |_, cx| {
                                cx.new(|_| WorktreeTooltip(tooltip.clone())).into()
                            })
                            .when(enabled, |el| {
                                el.cursor_pointer()
                                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let existing = this.slots.iter().position(|slot| {
                                            !slot.closing
                                                && slot.panel.read(cx).session_id
                                                    == session.session_id
                                        });
                                        let index = existing.unwrap_or_else(|| {
                                            this.open_session(session.clone(), cx)
                                        });
                                        this.set_active(index, cx);
                                        this.overview = false;
                                        this.overview_progress.set(0.0, Instant::now());
                                        this.focus_pending = true;
                                        cx.notify();
                                    }))
                            })
                            .child(
                                div()
                                    .size(px(14.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .map(|el| {
                                        if let Some(activity) = activity {
                                            el.child(activity)
                                        } else {
                                            el.child(div().size(px(5.0)).rounded_full().bg(
                                                if focused {
                                                    Theme::global().ACCENT
                                                } else {
                                                    Theme::global().TEXT_FAINT
                                                },
                                            ))
                                        }
                                    }),
                            )
                            .child(div().flex_1().min_w_0().text_ellipsis().child(title))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(Theme::global().TEXT_FAINT)
                                    .child(age),
                            ),
                    );
                }
            }
            list = list.child(group);
        }
        body.child(
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .flex()
                .flex_col()
                .child(list.pr(px(crate::scrollbar::GUTTER)))
                .child(crate::scrollbar::vertical_with_track(
                    &self.worktrees.scroll,
                    "worktree-scrollbar",
                )),
        )
        .into_any_element()
    }
}

#[cfg(test)]
#[path = "sidebar_worktrees_tests.rs"]
mod tests;
