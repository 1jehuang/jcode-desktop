//! Worktree creation inside the unified project sidebar. There is no separate
//! worktree mode: every checkout of a project, and the threads (including
//! swarm agents) running in it, share one hierarchy of
//! project, then branch or worktree, then threads.
use super::*;

pub(super) const BRANCH_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><g fill="none" stroke="black" stroke-width="1.6"><circle cx="6" cy="5" r="2.5"/><circle cx="18" cy="5" r="2.5"/><circle cx="6" cy="19" r="2.5"/><path d="M6 7.5v9M18 7.5c0 6-12 2-12 8"/></g></svg>"#;
pub(super) const FOLDER_ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7Zm0 3h18" fill="none" stroke="black" stroke-width="1.6" stroke-linejoin="round"/></svg>"#;

#[derive(Default)]
pub(super) struct State {
    pub(super) creating: bool,
    pub(super) error: Option<String>,
    pub(super) input: Option<Entity<PromptInput>>,
    /// Checkout the new worktree branches from.
    pub(super) input_directory: Option<String>,
    creation: Option<gpui::Task<()>>,
}

/// A compact pill button used by sidebar project and checkout headers. It acts
/// on mouse-down and stops propagation so the header never toggles as well.
pub(super) fn header_action(
    id: gpui::SharedString,
    tooltip: &'static str,
    content: gpui::AnyElement,
    visible: bool,
    group: &'static str,
) -> gpui::Stateful<gpui::Div> {
    let selector = id.to_string();
    div()
        .id(id)
        .debug_selector(move || selector.clone())
        .flex_none()
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(13.0))
        .text_color(Theme::global().TEXT_DIM)
        .opacity(if visible { 1.0 } else { 0.0 })
        .group_hover(group, |el| el.opacity(1.0))
        .hover(|el| el.bg(Theme::global().PANEL_BG).text_color(Theme::global().TEXT))
        .tooltip(move |_, cx| cx.new(|_| remotes::HeaderTooltip(tooltip.into())).into())
        .child(content)
}

pub(super) fn icon(data: &'static [u8], size: f32) -> gpui::AnyElement {
    gpui::svg()
        .data(data)
        .size(px(size))
        .flex_none()
        .text_color(Theme::global().TEXT_DIM)
        .into_any_element()
}

impl Workspace {
    pub(super) fn init_worktree_fixture(&mut self, cx: &mut Context<Self>) {
        if !harness::screenshot_mode()
            || std::env::var_os("JCODE_DESKTOP_SCREENSHOT_WORKTREES").is_none()
        {
            return;
        }
        // Real `.git` markers so the sidebar exercises its filesystem probe.
        let root = std::env::temp_dir().join(format!("jcode-worktree-fixture-{}", std::process::id()));
        let main = root.join("example");
        let _ = std::fs::create_dir_all(main.join(".git/worktrees/search"));
        let _ = std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n");
        let search = root.join("example-search");
        let _ = std::fs::create_dir_all(&search);
        let gitdir = main.join(".git/worktrees/search");
        let _ = std::fs::write(search.join(".git"), format!("gitdir: {}\n", gitdir.display()));
        let _ = std::fs::write(gitdir.join("commondir"), "../..\n");
        let _ = std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/feature/search\n");
        let main = main.to_string_lossy().into_owned();
        let search = search.to_string_lossy().into_owned();
        if let Some(first) = self.sessions.first_mut() {
            first.working_dir = Some(main.clone());
        }
        for slot in &self.slots {
            slot.panel.update(cx, |panel, _| panel.working_dir = Some(main.clone()));
        }
        let mut session = self.sessions[0].clone();
        session.session_id = "worktree-fixture-search".into();
        session.title = Some("Build project search".into());
        session.working_dir = Some(search.clone());
        session.updated_at_ms = Some(unix_now_ms() - 120_000);
        self.sessions.push(session.clone());
        session.session_id = "worktree-fixture-index".into();
        session.title = Some("Index workspace files".into());
        session.updated_at_ms = Some(unix_now_ms() - 7_200_000);
        self.sessions.push(session);
    }

    pub(super) fn begin_worktree(
        &mut self,
        directory: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.worktrees.creating {
            return;
        }
        let weak = cx.weak_entity();
        let cancel = weak.clone();
        let input = cx.new(|cx| {
            PromptInput::new(
                cx,
                "New branch name, e.g. feature/search",
                move |branch, _, _, app| {
                    let _ = weak.update(app, |this, cx| this.create_worktree(&branch, cx));
                },
            )
            .with_on_overlay_cancel(move |app| {
                let _ = cancel.update(app, |this, cx| this.cancel_worktree(cx));
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

    fn cancel_worktree(&mut self, cx: &mut Context<Self>) {
        if !self.worktrees.creating {
            self.worktrees.input = None;
            self.worktrees.input_directory = None;
            self.worktrees.error = None;
            self.focus_pending = true;
            cx.notify();
        }
    }

    pub(super) fn create_worktree(&mut self, branch: &str, cx: &mut Context<Self>) {
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
                        // The new thread is what makes the worktree appear
                        // under its project, so always open one.
                        this.focus_pending = true;
                        this.open_local_draft(Some(entry.path), cx);
                    }
                    Err(error) => this.worktrees.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Inline form shown above the thread list while naming a new worktree.
    pub(super) fn render_worktree_form(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let input = self.worktrees.input.clone()?;
        let origin = self
            .worktrees
            .input_directory
            .as_deref()
            .map(|dir| {
                Path::new(dir)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dir.to_owned())
            })
            .unwrap_or_default();
        Some(
            div()
                .id("worktree-create-form")
                .debug_selector(|| "worktree-create-form".into())
                .flex_none()
                .mx_2()
                .mb_2()
                .p_2()
                .rounded_xl()
                .bg(Theme::global().PANEL_BG)
                .flex()
                .flex_col()
                .gap_2()
                .capture_action(cx.listener(|this, _: &crate::input::Submit, _, cx| {
                    cx.stop_propagation();
                    if let Some(input) = &this.worktrees.input {
                        let branch = input.read(cx).content.to_string();
                        this.create_worktree(&branch, cx);
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(icon(BRANCH_ICON, 12.0))
                        .child(if self.worktrees.creating {
                            format!("Creating worktree in {origin}…")
                        } else {
                            format!("New worktree in {origin}")
                        }),
                )
                .child(input)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child("Branches from HEAD. Uncommitted edits stay put. Enter to create, Esc to cancel."),
                )
                .children(self.worktrees.error.clone().map(|error| {
                    div()
                        .id("worktree-error")
                        .debug_selector(|| "worktree-error".into())
                        .text_size(px(11.0))
                        .text_color(Theme::global().ERROR)
                        .child(error)
                }))
                .into_any_element(),
        )
    }

    /// Always-visible entry points at the top of the thread list.
    pub(super) fn render_sidebar_quick_actions(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let pill = |id: &'static str, label: &'static str, glyph: gpui::AnyElement| {
            let tip = if id == "sidebar-open-project" { "Open a project folder" } else { "New thread in the default directory" };
            div()
                .id(id)
                .debug_selector(move || id.into())
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .h(px(26.0))
                .px_2()
                .flex()
                .items_center()
                .justify_center()
                .gap_1()
                .rounded_full()
                .cursor_pointer()
                .bg(Theme::global().TOOL_BG)
                .text_size(px(11.0))
                .text_color(Theme::global().TEXT_DIM)
                .hover(|el| el.bg(Theme::global().PANEL_BG).text_color(Theme::global().TEXT))
                .tooltip(move |_, cx| cx.new(|_| remotes::HeaderTooltip(tip.into())).into())
                .child(glyph)
                .child(div().min_w_0().truncate().child(label))
        };
        div()
            .flex_none()
            .w_full()
            .min_w_0()
            .pl_2()
            .pr(px(crate::scrollbar::GUTTER + 4.0))
            .pt_2()
            .flex()
            .gap_2()
            .child(
                pill("sidebar-new-thread", "Thread", div().child("+").into_any_element())
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.missed("new_panel", cx);
                            this.open_new_session(cx);
                        }),
                    ),
            )
            .child(
                pill("sidebar-open-project", "Project", icon(FOLDER_ICON, 12.0)).on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.open_folder(&OpenFolder, window, cx);
                    }),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "sidebar_worktrees_tests.rs"]
mod tests;
