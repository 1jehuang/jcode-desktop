//! Sidebar sessions are grouped by project: the Git repository a session was
//! spawned in, or its working directory outside Git. Grouping is presentation
//! only. It never closes, moves, or reorders panels.
use super::*;

/// Key for sessions without any working directory.
pub(super) const OTHER: &str = "";

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Project {
    pub key: String,
    pub label: String,
    /// Full location shown in the header tooltip.
    pub location: Option<String>,
}

/// One Git checkout inside a project: the main working tree or a linked
/// worktree, with the branch it currently has checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Checkout {
    pub path: String,
    pub branch: String,
    pub linked: bool,
}

/// Branches move underneath us (checkout, rebase), so HEAD is re-read after
/// this interval rather than on every hover repaint.
const CHECKOUT_TTL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct State {
    /// Explicit disclosure choices. Unset projects use their default.
    pub(super) collapsed: HashMap<String, bool>,
    focused: Option<String>,
    /// Working directory to repository root. Filesystem probes run once per
    /// directory, never on every hover repaint.
    roots: HashMap<String, Option<PathBuf>>,
    checkouts: HashMap<String, (Option<Checkout>, Instant)>,
}

impl State {
    /// History-only projects default to collapsed so open work stays on top.
    pub(super) fn collapsed(&self, key: &str, default: bool) -> bool {
        self.collapsed.get(key).copied().unwrap_or(default)
    }

    fn toggle(&mut self, key: &str, default: bool) {
        let next = !self.collapsed(key, default);
        self.collapsed.insert(key.to_owned(), next);
    }

    /// Focusing a session reveals its project once, and then respects manual
    /// collapse until focus moves to another session.
    pub(super) fn sync_focus(&mut self, session_id: Option<&str>, project: Option<&str>) {
        if self.focused.as_deref() != session_id {
            self.focused = session_id.map(str::to_owned);
            if let Some(project) = project {
                self.collapsed.insert(project.to_owned(), false);
            }
        }
    }

    /// The local checkout a session runs in. Remote sessions never probe local Git.
    pub(super) fn checkout(&mut self, session: &jcode_sdk::SessionInfo) -> Option<Checkout> {
        if harness::remote_host(&session.session_id).is_some() {
            return None;
        }
        let directory = session
            .working_dir
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())?;
        if let Some((cached, read)) = self.checkouts.get(directory)
            && read.elapsed() < CHECKOUT_TTL
        {
            return cached.clone();
        }
        let value = probe_checkout(Path::new(directory));
        self.checkouts
            .insert(directory.to_owned(), (value.clone(), Instant::now()));
        value
    }

    fn root(&mut self, directory: &str) -> Option<PathBuf> {
        self.roots
            .entry(directory.to_owned())
            .or_insert_with(|| git_root(Path::new(directory)))
            .clone()
    }

    /// The session's project and, when it differs, its directory inside it.
    pub(super) fn project(
        &mut self,
        session: &jcode_sdk::SessionInfo,
    ) -> (Project, Option<String>) {
        let directory = session
            .working_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty());
        if let Some(host) = harness::remote_host(&session.session_id) {
            let label = match directory {
                Some(dir) => format!("{host} · {}", path_label(dir)),
                None => host.clone(),
            };
            return (
                Project {
                    key: format!("ssh://{host}{}", directory.unwrap_or_default()),
                    label,
                    location: sidebar_session_directory(session),
                },
                None,
            );
        }
        let Some(directory) = directory else {
            return (
                Project {
                    key: OTHER.into(),
                    label: "No directory".into(),
                    location: None,
                },
                None,
            );
        };
        let root = self
            .root(directory)
            .unwrap_or_else(|| PathBuf::from(directory));
        let root_text = root.to_string_lossy().into_owned();
        let subdirectory = match Path::new(directory).strip_prefix(&root) {
            Ok(relative) if relative.as_os_str().is_empty() => None,
            Ok(relative) => Some(relative.to_string_lossy().into_owned()),
            // A linked worktree lives outside the main checkout.
            Err(_) => sidebar_session_directory(session),
        };
        (
            Project {
                label: path_label(&root_text),
                location: Some(compact_working_dir(&root_text)),
                key: root_text,
            },
            subdirectory,
        )
    }
}

fn path_label(path: &str) -> String {
    let compact = compact_working_dir(path);
    if compact.starts_with("~ ") {
        return "~".into();
    }
    Path::new(path.trim_end_matches('/'))
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or(compact)
}

/// Nearest enclosing repository. Linked worktrees resolve to the main
/// checkout so every branch of one repository shares a project.
pub(super) fn git_root(directory: &Path) -> Option<PathBuf> {
    for ancestor in directory.ancestors() {
        let marker = ancestor.join(".git");
        if marker.is_dir() {
            return Some(ancestor.to_path_buf());
        }
        if marker.is_file() {
            return Some(linked_worktree_main(ancestor, &marker).unwrap_or(ancestor.to_path_buf()));
        }
    }
    None
}

/// Nearest enclosing checkout and its branch, read straight from `.git` so the
/// sidebar never spawns Git while rendering.
pub(super) fn probe_checkout(directory: &Path) -> Option<Checkout> {
    for ancestor in directory.ancestors() {
        let marker = ancestor.join(".git");
        let (gitdir, linked) = if marker.is_dir() {
            (marker, false)
        } else if marker.is_file() {
            let contents = std::fs::read_to_string(&marker).ok()?;
            let gitdir = ancestor.join(contents.trim().strip_prefix("gitdir:")?.trim());
            // Submodules have a gitdir file but no commondir.
            let linked = gitdir.join("commondir").is_file();
            (gitdir, linked)
        } else {
            continue;
        };
        let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
        let head = head.trim();
        let branch = match head.strip_prefix("ref:") {
            Some(reference) => {
                let reference = reference.trim();
                reference
                    .strip_prefix("refs/heads/")
                    .unwrap_or(reference)
                    .to_owned()
            }
            None => format!("detached {}", head.chars().take(8).collect::<String>()),
        };
        return Some(Checkout {
            path: ancestor.to_string_lossy().into_owned(),
            branch,
            linked,
        });
    }
    None
}

fn linked_worktree_main(worktree: &Path, marker: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(marker).ok()?;
    let gitdir = contents.trim().strip_prefix("gitdir:")?.trim();
    let gitdir = worktree.join(gitdir);
    // Submodules have no commondir and remain their own project.
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = std::fs::canonicalize(gitdir.join(common.trim())).ok()?;
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf))?
}

impl Workspace {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_project_header(
        &self,
        index: usize,
        project: &Project,
        count: usize,
        collapsed: bool,
        branch: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let tooltip = format!(
            "{} {} · {} session{}",
            if collapsed { "Expand" } else { "Collapse" },
            project.location.as_deref().unwrap_or(&project.label),
            count,
            if count == 1 { "" } else { "s" },
        );
        let key = project.key.clone();
        // Only local directories can host a new thread or worktree from here.
        let local = Path::new(&project.key)
            .is_absolute()
            .then(|| project.key.clone());
        let git = local
            .as_deref()
            .is_some_and(|dir| Path::new(dir).join(".git").exists());
        let group: &'static str = "sidebar-project-header";
        div()
            .id(("sidebar-project", index))
            .debug_selector(move || format!("sidebar-project-{index}"))
            .group(group)
            .mx_2()
            .mb_1()
            .pl_2()
            .pr_1()
            .h(px(24.0))
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .cursor_pointer()
            .hover(|el| el.bg(theme.TOOL_BG))
            .tooltip(move |_, cx| {
                cx.new(|_| remotes::HeaderTooltip(tooltip.clone().into()))
                    .into()
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.sidebar_projects.toggle(&key, collapsed);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(12.0))
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(theme.TEXT_DIM)
                    .child(if collapsed { "›" } else { "⌄" }),
            )
            .child(
                div()
                    .flex_shrink(1.0)
                    .min_w_0()
                    .truncate()
                    .debug_selector(move || format!("sidebar-project-label-{index}"))
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.TEXT_DIM)
                    .child(project.label.clone()),
            )
            .children(branch.map(|branch| {
                div()
                    .debug_selector(move || format!("sidebar-project-branch-{index}"))
                    .flex_shrink(1.0)
                    .min_w(px(24.0))
                    .max_w(px(120.0))
                    .h(px(16.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .rounded_full()
                    .bg(theme.TOOL_BG)
                    .text_size(px(9.0))
                    .text_color(theme.TEXT_DIM)
                    .child(sidebar_worktrees::icon(sidebar_worktrees::BRANCH_ICON, 9.0))
                    .child(div().min_w_0().truncate().child(branch))
            }))
            .child(div().flex_1())
            .when(git, |el| {
                let directory = local.clone().unwrap_or_default();
                el.child(
                    sidebar_worktrees::header_action(
                        format!("sidebar-project-worktree-{index}").into(),
                        "New worktree: start a branch in its own checkout",
                        sidebar_worktrees::icon(sidebar_worktrees::BRANCH_ICON, 11.0),
                        false,
                        group,
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.begin_worktree(directory.clone(), window, cx);
                        }),
                    ),
                )
            })
            .when_some(local, |el, directory| {
                let key = project.key.clone();
                el.child(
                    sidebar_worktrees::header_action(
                        format!("sidebar-project-new-{index}").into(),
                        "New thread in this project",
                        div().child("+").into_any_element(),
                        false,
                        group,
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.sidebar_projects.collapsed.insert(key.clone(), false);
                            this.focus_pending = true;
                            this.open_local_draft(Some(directory.clone()), cx);
                        }),
                    ),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w(px(16.0))
                    .text_right()
                    .text_size(px(10.0))
                    .text_color(theme.TEXT_DIM)
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    /// Branch row separating checkouts of one project.
    pub(super) fn render_checkout_header(
        &self,
        index: usize,
        checkout: Option<&Checkout>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let group: &'static str = "sidebar-checkout-header";
        let (label, tooltip) = match checkout {
            Some(checkout) => (
                checkout.branch.clone(),
                format!(
                    "{} · {}",
                    if checkout.linked {
                        "Worktree"
                    } else {
                        "Main checkout"
                    },
                    compact_working_dir(&checkout.path)
                ),
            ),
            None => ("Outside Git".into(), "Threads outside any checkout".into()),
        };
        let path = checkout.map(|checkout| checkout.path.clone());
        div()
            .id(("sidebar-checkout", index))
            .debug_selector(move || format!("sidebar-checkout-{index}"))
            .group(group)
            .ml(px(18.0))
            .mr_2()
            .mb(px(2.0))
            .pl_1()
            .pr_1()
            .h(px(22.0))
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .text_size(px(11.0))
            .text_color(theme.TEXT_DIM)
            .tooltip(move |_, cx| {
                cx.new(|_| remotes::HeaderTooltip(tooltip.clone().into()))
                    .into()
            })
            .child(sidebar_worktrees::icon(
                if checkout.is_some_and(|c| c.linked) {
                    sidebar_worktrees::BRANCH_ICON
                } else {
                    sidebar_worktrees::FOLDER_ICON
                },
                11.0,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .debug_selector(move || format!("sidebar-checkout-label-{index}"))
                    .child(label),
            )
            .when(checkout.is_some_and(|c| c.linked), |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(6.0))
                        .rounded_full()
                        .bg(theme.TOOL_BG)
                        .text_size(px(9.0))
                        .child("worktree"),
                )
            })
            .when_some(path, |el, path| {
                el.child(
                    sidebar_worktrees::header_action(
                        format!("sidebar-checkout-new-{index}").into(),
                        "New thread on this branch",
                        div().child("+").into_any_element(),
                        false,
                        group,
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.focus_pending = true;
                            this.open_local_draft(Some(path.clone()), cx);
                        }),
                    ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_group_subdirectories_and_linked_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(temp.path()).unwrap().join("repo");
        std::fs::create_dir_all(repo.join(".git/worktrees/feature")).unwrap();
        std::fs::create_dir_all(repo.join("crates/ui")).unwrap();
        let feature = repo.parent().unwrap().join("feature");
        std::fs::create_dir_all(&feature).unwrap();
        std::fs::write(
            feature.join(".git"),
            format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/feature").display()
            ),
        )
        .unwrap();
        std::fs::write(repo.join(".git/worktrees/feature/commondir"), "../..\n").unwrap();
        let plain = repo.parent().unwrap().join("notes");
        std::fs::create_dir_all(&plain).unwrap();

        assert_eq!(git_root(&repo), Some(repo.clone()));
        assert_eq!(git_root(&repo.join("crates/ui")), Some(repo.clone()));
        assert_eq!(git_root(&feature), Some(repo.clone()));

        let mut state = State::default();
        let mut session = |id: &str, dir: &Path| {
            let mut info = super::super::tests::session_info(id, None);
            info.working_dir = Some(dir.to_string_lossy().into_owned());
            state.project(&info)
        };
        let (root, none) = session("a", &repo);
        let (nested, sub) = session("b", &repo.join("crates/ui"));
        let (linked, _) = session("c", &feature);
        let (notes, _) = session("d", &plain);
        assert_eq!(root.label, "repo");
        assert_eq!(none, None);
        assert_eq!(nested.key, root.key);
        assert_eq!(sub.as_deref(), Some("crates/ui"));
        assert_eq!(linked.key, root.key);
        assert_eq!(notes.label, "notes");
        assert_ne!(notes.key, root.key);
        let (other, _) = State::default().project(&super::super::tests::session_info("e", None));
        assert_eq!(other.key, OTHER);
    }

    #[test]
    fn focus_reveals_its_project_once_and_manual_collapse_survives_renders() {
        let mut state = State::default();
        assert!(state.collapsed("/a", true));
        assert!(!state.collapsed("/a", false));
        state.toggle("/a", false);
        state.sync_focus(Some("s1"), Some("/a"));
        assert!(!state.collapsed("/a", true));
        state.toggle("/a", false);
        state.sync_focus(Some("s1"), Some("/a"));
        assert!(
            state.collapsed("/a", false),
            "same focus must not undo manual collapse"
        );
        state.sync_focus(Some("s2"), Some("/a"));
        assert!(!state.collapsed("/a", true));
    }

    fn panel_in(workspace: &mut Workspace, id: &str, dir: &str, cx: &mut Context<Workspace>) {
        workspace.push_test_panel(id, cx);
        let panel = workspace.slots.last().unwrap().panel.clone();
        panel.update(cx, |panel, _| panel.working_dir = Some(dir.into()));
    }

    #[gpui::test]
    fn sessions_group_under_project_headers_with_open_panels_first(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            panel_in(
                &mut workspace,
                "session_fox_alpha_one",
                "/nowhere/alpha",
                cx,
            );
            panel_in(&mut workspace, "session_owl_beta_one", "/nowhere/beta", cx);
            panel_in(
                &mut workspace,
                "session_hare_alpha_two",
                "/nowhere/alpha",
                cx,
            );
            let mut history =
                super::super::tests::session_info("session_cat_alpha_old", Some("old"));
            history.working_dir = Some("/nowhere/alpha".into());
            let mut gamma = super::super::tests::session_info("session_elk_gamma", Some("gamma"));
            gamma.working_dir = Some("/nowhere/gamma".into());
            workspace.sessions = vec![history, gamma];
            workspace.active = 0;
            workspace
        });
        vcx.run_until_parked();
        let order = |workspace: &Entity<Workspace>, vcx: &mut gpui::VisualTestContext| {
            workspace.read_with(vcx, |w, _| {
                w.sidebar_session_layout
                    .iter()
                    .map(|item| (item.session_id.clone(), item.project))
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            order(&workspace, vcx),
            vec![
                ("session_fox_alpha_one".into(), 0),
                ("session_hare_alpha_two".into(), 0),
                ("session_cat_alpha_old".into(), 0),
                ("session_owl_beta_one".into(), 1),
                ("session_elk_gamma".into(), 2),
            ]
        );
        let alpha = vcx.debug_bounds("sidebar-project-0").unwrap();
        let beta = vcx.debug_bounds("sidebar-project-1").unwrap();
        assert!(alpha.bottom() <= vcx.debug_bounds("sidebar-session-0").unwrap().top());
        assert!(vcx.debug_bounds("sidebar-session-2").unwrap().bottom() <= beta.top());
        assert!(
            vcx.debug_bounds("sidebar-session-divider").is_some(),
            "history is divided from open panels inside a project"
        );

        // Collapsing hides members but keeps the header, and never navigates.
        vcx.simulate_click(beta.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-project-1").is_some());
        assert!(vcx.debug_bounds("sidebar-session-title-3").is_none());
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(
                w.slots[w.active].panel.read(cx).session_id,
                "session_fox_alpha_one"
            );
        });
        // Focusing a session inside a collapsed project reveals it.
        workspace.update(vcx, |w, cx| {
            w.active = 1;
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-session-title-3").is_some());
    }

    #[gpui::test]
    fn live_slots_without_catalog_entries_appear_immediately_in_map_order(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            // Creation order deliberately differs from the map's row-major order.
            workspace.active_row = 1;
            workspace.push_test_panel("session_fox_lower", cx);
            workspace.active_row = 0;
            workspace.push_test_panel("session_owl_upper_left", cx);
            workspace.push_test_panel("session_hare_upper_right", cx);
            workspace.active = 1;
            workspace
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert!(
                workspace.sessions.is_empty(),
                "rendering must not populate the catalog"
            );
            assert_eq!(
                workspace
                    .sidebar_session_layout
                    .iter()
                    .map(|item| item.session_id.as_str())
                    .collect::<Vec<_>>(),
                vec![
                    "session_owl_upper_left",
                    "session_hare_upper_right",
                    "session_fox_lower"
                ],
            );
        });
        assert!(
            vcx.debug_bounds("sidebar-project-0").is_none(),
            "a lone directory-less group needs no header"
        );
        for (selector, expected) in [
            ("sidebar-session-0", "session_owl_upper_left"),
            ("sidebar-session-1", "session_hare_upper_right"),
            ("sidebar-session-2", "session_fox_lower"),
        ] {
            let bounds = vcx.debug_bounds(selector).unwrap();
            vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            workspace.read_with(vcx, |workspace, cx| {
                assert_eq!(
                    workspace.slots.len(),
                    3,
                    "clicking a live row must not open a duplicate"
                );
                assert_eq!(
                    workspace.slots[workspace.active].panel.read(cx).session_id,
                    expected
                );
            });
        }
    }

    #[gpui::test]
    fn inactive_live_rows_in_a_shared_project_stay_compact(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for name in ["session_fox_1700000000000_a", "session_owl_1700000000001_b"] {
                panel_in(&mut workspace, name, "/nowhere/live-work", cx);
            }
            workspace.active = 0;
            workspace
        });
        vcx.run_until_parked();
        for active in [0, 1, 0] {
            workspace.update(vcx, |workspace, cx| {
                workspace.active = active;
                cx.notify();
            });
            vcx.run_until_parked();
            workspace.read_with(vcx, |workspace, _| {
                for (index, item) in workspace.sidebar_session_layout.iter().enumerate() {
                    assert_eq!(item.details, index == active);
                }
            });
            for index in 0..2 {
                let bounds = vcx
                    .debug_bounds(["sidebar-session-0", "sidebar-session-1"][index])
                    .unwrap();
                if index == active {
                    assert!(bounds.size.height > px(24.0));
                } else {
                    assert_eq!(bounds.size.height, px(24.0));
                }
            }
        }
    }
}
