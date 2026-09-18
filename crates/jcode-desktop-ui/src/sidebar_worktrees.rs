//! Git checkout navigation. Swarms remain the default and are never reconfigured.
use super::*;
use jcode_sdk::worktrees::Worktree;

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
            .mx_3()
            .my_2()
            .p_1()
            .rounded_md()
            .bg(Theme::global().TOOL_BG)
            .children(
                [
                    (false, "Swarm", "sidebar-mode-swarm"),
                    (true, "Worktrees", "sidebar-mode-worktrees"),
                ]
                .into_iter()
                .map(|(mode, label, id)| {
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .flex_1()
                        .py_1()
                        .rounded_sm()
                        .text_center()
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .text_color(if mode == self.worktree_mode {
                            Theme::global().TEXT
                        } else {
                            Theme::global().TEXT_DIM
                        })
                        .when(mode == self.worktree_mode, |el| {
                            el.bg(Theme::global().PANEL_BG)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.worktree_mode = mode;
                            this.worktrees.input = None;
                            this.worktrees.input_directory = None;
                            this.focus_pending = true;
                            cx.notify();
                        }))
                        .child(label)
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
        if directory != self.worktrees.directory && !self.worktrees.creating {
            self.worktrees.input = None;
            self.worktrees.input_directory = None;
            if let Some(directory) = &directory {
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
        body = body.child(
            div()
                .text_size(px(11.0))
                .text_color(Theme::global().TEXT_DIM)
                .child("Separate branches and files. Sessions stay in their checkout."),
        );
        body = body.child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .id("worktree-refresh")
                        .debug_selector(|| "worktree-refresh".into())
                        .cursor_pointer()
                        .py_1()
                        .text_color(Theme::global().TEXT_DIM)
                        .child("Refresh")
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
                        .text_color(Theme::global().ACCENT)
                        .child(if self.worktrees.creating {
                            "Creating…"
                        } else {
                            "+ New worktree"
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
        for (index, entry) in self.worktrees.entries.iter().enumerate() {
            let path = entry.path.clone();
            let enabled = !entry.bare && entry.prunable.is_none();
            let label = entry
                .branch
                .as_deref()
                .map(|branch| branch.trim_start_matches("refs/heads/").to_owned())
                .unwrap_or_else(|| {
                    if entry.bare {
                        "Bare repository".into()
                    } else {
                        format!(
                            "Detached · {}",
                            entry.head.chars().take(8).collect::<String>()
                        )
                    }
                });
            let mut row = div()
                .id(gpui::SharedString::from(format!("worktree-{index}")))
                .debug_selector(move || format!("worktree-{index}"))
                .flex_none()
                .p_2()
                .rounded_md()
                .flex()
                .flex_col()
                .gap_1()
                .bg(if active_path.as_ref() == Some(&path) {
                    Theme::global().TOOL_BG
                } else {
                    Theme::global().BG
                })
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(div().flex_1().min_w_0().text_ellipsis().child(label))
                        .when(active_path.as_ref() == Some(&path), |el| {
                            el.child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(Theme::global().ACCENT)
                                    .child("Active"),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .overflow_hidden()
                        .child(compact_working_dir(&path)),
                );
            if enabled {
                row = row
                    .cursor_pointer()
                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_worktree(path.clone(), cx)),
                    );
            }
            if entry.locked.is_some() || entry.prunable.is_some() {
                row = row.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(if entry.prunable.is_some() {
                            "Unavailable · prune in Git to remove"
                        } else {
                            "Locked"
                        }),
                );
            }
            list = list.child(row);
            for session in self.sessions.iter().filter(|session| {
                !session.archived
                    && !session.session_id.contains("://")
                    && session
                        .working_dir
                        .as_deref()
                        .and_then(|dir| owner(dir, &self.worktrees.entries))
                        == Some(entry.path.as_str())
            }) {
                let session = session.clone();
                let label = session
                    .title
                    .clone()
                    .unwrap_or_else(|| session.session_id.clone());
                let selector = format!("worktree-session-{}", session.session_id);
                list = list.child(
                    div()
                        .id(gpui::SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .flex_none()
                        .ml_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_size(px(11.0))
                        .text_ellipsis()
                        .cursor_pointer()
                        .text_color(Theme::global().TEXT_DIM)
                        .hover(|el| el.bg(Theme::global().TOOL_BG))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let existing = this.slots.iter().position(|slot| {
                                !slot.closing
                                    && slot.panel.read(cx).session_id == session.session_id
                            });
                            let index =
                                existing.unwrap_or_else(|| this.open_session(session.clone(), cx));
                            this.set_active(index, cx);
                            this.overview = false;
                            this.overview_progress.set(0.0, Instant::now());
                            this.focus_pending = true;
                            cx.notify();
                        })),
                );
            }
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
