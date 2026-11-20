//! Right-hand Files panel: a lazily loaded tree of the active session's
//! working directory with a name filter.
//!
//! Rendering never touches the filesystem. Directory listings and the search
//! index are produced on the background executor and cached in
//! [`crate::file_tree`]; the list is virtualized so only visible rows are built.
use super::*;
use crate::file_tree::{self, FileIndex, FileKind, FileTree, RowKind};
use gpui::{UniformListScrollHandle, svg, uniform_list};
use std::sync::Arc;

pub(super) const FILE_EXPLORER_WIDTH: f32 = 280.0;
const MIN_WIDTH: f32 = 200.0;
const MAX_WIDTH: f32 = 720.0;
const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 12.0;

#[derive(Default)]
enum IndexState {
    #[default]
    Missing,
    Building(PathBuf),
    Ready(PathBuf, Arc<FileIndex>),
}

pub(super) struct State {
    pub open: bool,
    /// Dragged width. Survives closing the panel.
    pub width: f32,
    pub resizing: bool,
    tree: Option<FileTree>,
    scroll: UniformListScrollHandle,
    search: Option<Entity<PromptInput>>,
    query: String,
    index: IndexState,
    matches: Arc<[Arc<str>]>,
    selected: Option<PathBuf>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            width: FILE_EXPLORER_WIDTH,
            resizing: false,
            tree: None,
            scroll: UniformListScrollHandle::default(),
            search: None,
            query: String::new(),
            index: IndexState::Missing,
            matches: Arc::from([]),
            selected: None,
        }
    }
}

/// Keep the panel usable and leave the session canvas most of the window.
pub(super) fn clamp_width(width: f32, viewport_width: f32) -> f32 {
    width.clamp(
        MIN_WIDTH,
        (viewport_width * 0.5).clamp(MIN_WIDTH, MAX_WIDTH),
    )
}

fn file_icon(kind: FileKind) -> &'static [u8] {
    match kind {
        FileKind::Code => include_bytes!("../../../assets/icons/files/file-code.svg"),
        FileKind::Config => include_bytes!("../../../assets/icons/files/file-config.svg"),
        FileKind::Text => include_bytes!("../../../assets/icons/files/file-text.svg"),
        FileKind::Image => include_bytes!("../../../assets/icons/files/file-image.svg"),
        FileKind::Secret => include_bytes!("../../../assets/icons/files/file-secret.svg"),
        FileKind::Lock => include_bytes!("../../../assets/icons/files/file-lock.svg"),
        FileKind::Other => include_bytes!("../../../assets/icons/files/file.svg"),
    }
}

fn icon(data: &'static [u8], size: f32, color: gpui::Rgba) -> gpui::Svg {
    svg()
        .data(data)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

impl Workspace {
    pub(super) fn toggle_file_explorer(
        &mut self,
        _: &ToggleFileExplorer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_file_explorer_open(!self.file_explorer.open, cx);
    }

    #[cfg(test)]
    pub(super) fn file_explorer_tree(&self) -> Option<&FileTree> {
        self.file_explorer.tree.as_ref()
    }

    pub(super) fn set_file_explorer_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.file_explorer.open = open;
        if !open {
            // Free the cached tree and index; reopening lists afresh.
            let search = self.file_explorer.search.take();
            if let Some(search) = &search {
                search.update(cx, |input, cx| input.set_content(String::new(), cx));
            }
            let width = self.file_explorer.width;
            self.file_explorer = State {
                search,
                width,
                ..State::default()
            };
            self.focus_pending = true;
        }
        cx.notify();
    }

    /// Point the tree at the active session's directory and request listings
    /// for directories that became visible. Cheap when nothing changed.
    fn sync_file_explorer(&mut self, cx: &mut Context<Self>) {
        let root = self.file_browser_root(cx);
        let state = &mut self.file_explorer;
        if state.tree.as_ref().is_none_or(|tree| tree.root() != root) {
            state.tree = Some(FileTree::new(root));
            state.index = IndexState::Missing;
            state.matches = Arc::from([]);
            state.selected = None;
            if !state.query.trim().is_empty() {
                let query = state.query.clone();
                self.set_file_explorer_query(query, cx);
            }
        }
        let pending = self
            .file_explorer
            .tree
            .as_mut()
            .map(FileTree::take_pending)
            .unwrap_or_default();
        if !pending.is_empty() {
            self.list_file_explorer_dirs(pending, cx);
        }
    }

    fn list_file_explorer_dirs(&mut self, dirs: Vec<PathBuf>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let listings = cx
                .background_executor()
                .spawn(async move {
                    dirs.into_iter()
                        .map(|dir| {
                            let result =
                                file_tree::list_dir(&dir).map_err(|error| error.to_string());
                            (dir, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(tree) = this.file_explorer.tree.as_mut() {
                    for (dir, result) in listings {
                        tree.set_listing(&dir, result);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_file_explorer(&mut self, cx: &mut Context<Self>) {
        if let Some(tree) = self.file_explorer.tree.as_ref() {
            let targets = tree.refresh_targets();
            self.list_file_explorer_dirs(targets, cx);
        }
        // New and deleted files must reach search too.
        self.file_explorer.index = IndexState::Missing;
        let query = self.file_explorer.query.clone();
        self.set_file_explorer_query(query, cx);
    }

    pub(super) fn set_file_explorer_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.file_explorer.query = query;
        self.file_explorer
            .scroll
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        let query = self.file_explorer.query.trim().to_owned();
        let Some(root) = self
            .file_explorer
            .tree
            .as_ref()
            .map(|tree| tree.root().to_path_buf())
        else {
            return;
        };
        if query.is_empty() {
            self.file_explorer.matches = Arc::from([]);
            cx.notify();
            return;
        }
        match &self.file_explorer.index {
            IndexState::Ready(indexed, index) if *indexed == root => {
                self.file_explorer.matches = index.search(&query).into();
            }
            IndexState::Building(indexing) if *indexing == root => {}
            _ => {
                self.file_explorer.index = IndexState::Building(root.clone());
                cx.spawn(async move |this, cx| {
                    let index = {
                        let root = root.clone();
                        cx.background_executor()
                            .spawn(async move { Arc::new(FileIndex::build(&root)) })
                            .await
                    };
                    let _ = this.update(cx, |this, cx| {
                        let state = &mut this.file_explorer;
                        if !matches!(&state.index, IndexState::Building(indexing) if *indexing == root)
                        {
                            return;
                        }
                        state.matches = index.search(state.query.trim()).into();
                        state.index = IndexState::Ready(root, index);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn file_explorer_search(&mut self, cx: &mut Context<Self>) -> Entity<PromptInput> {
        if let Some(search) = &self.file_explorer.search {
            return search.clone();
        }
        let weak = cx.weak_entity();
        let change = weak.clone();
        let cancel = weak.clone();
        let search = cx.new(|cx| {
            PromptInput::new(cx, "Find file", move |_, _, window, app| {
                // Enter opens the best match.
                let _ = weak.update(app, |this, cx| {
                    let first = this.file_explorer.matches.first().cloned();
                    if let (Some(first), Some(tree)) = (first, this.file_explorer.tree.as_ref()) {
                        let path = tree.root().join(&*first);
                        this.open_code_file(path, window, cx);
                    }
                });
            })
            .without_command_completion()
            .without_chrome()
            .with_on_change(move |text, app| {
                let text = text.to_owned();
                let _ = change.update(app, |this, cx| this.set_file_explorer_query(text, cx));
            })
            .with_on_overlay_cancel(move |app| {
                cancel
                    .update(app, |this, cx| {
                        let had_query = !this.file_explorer.query.is_empty();
                        if let Some(search) = this.file_explorer.search.clone() {
                            search.update(cx, |input, cx| input.set_content(String::new(), cx));
                        }
                        this.set_file_explorer_query(String::new(), cx);
                        had_query
                    })
                    .unwrap_or(false)
            })
        });
        self.file_explorer.search = Some(search.clone());
        search
    }

    fn file_explorer_header_button(
        id: &'static str,
        data: &'static [u8],
        tooltip: &'static str,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .size(px(24.0))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .text_color(Theme::global().TEXT_DIM)
            .hover(|el| {
                el.bg(Theme::global().TOOL_BG)
                    .text_color(Theme::global().TEXT)
            })
            .tooltip(move |_, cx| cx.new(|_| live_tabs::TabTooltip(tooltip.into())).into())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                on_click(this, window, cx);
            }))
            .child(
                svg()
                    .data(data)
                    .size(px(14.0))
                    .text_color(Theme::global().TEXT_DIM),
            )
    }

    pub(super) fn render_file_explorer(
        &mut self,
        top_inset: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let _render_scope = crate::render_stats::scope("FileExplorer");
        self.sync_file_explorer(cx);
        let search = self.file_explorer_search(cx);
        let theme = Theme::global();
        let root_name = self
            .file_explorer
            .tree
            .as_ref()
            .map(|tree| {
                tree.root()
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| tree.root().display().to_string())
            })
            .unwrap_or_default();
        let root_path = self
            .file_explorer
            .tree
            .as_ref()
            .map(|tree| tree.root().display().to_string())
            .unwrap_or_default();
        let searching = !self.file_explorer.query.trim().is_empty();

        let header = div()
            .debug_selector(|| "file-explorer-header".into())
            .flex_none()
            .h(px(36.0))
            .px_3()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("file-explorer-root")
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.TEXT)
                    .tooltip(move |_, cx| {
                        let path = root_path.clone();
                        cx.new(|_| live_tabs::TabTooltip(path.into())).into()
                    })
                    .child(root_name),
            )
            .child(Self::file_explorer_header_button(
                "file-explorer-collapse",
                include_bytes!("../../../assets/icons/files/collapse.svg"),
                "Collapse folders",
                |this, _, cx| {
                    if let Some(tree) = this.file_explorer.tree.as_mut() {
                        tree.collapse_all();
                    }
                    cx.notify();
                },
                cx,
            ))
            .child(Self::file_explorer_header_button(
                "file-explorer-refresh",
                include_bytes!("../../../assets/icons/files/refresh.svg"),
                "Refresh",
                |this, _, cx| this.refresh_file_explorer(cx),
                cx,
            ))
            .child(Self::file_explorer_header_button(
                "file-explorer-close",
                include_bytes!("../../../assets/icons/files/files-panel.svg"),
                if cfg!(target_os = "macos") {
                    "Hide files (⌘⇧E)"
                } else {
                    "Hide files (Super+Shift+E)"
                },
                |this, _, cx| this.set_file_explorer_open(false, cx),
                cx,
            ));

        let search_focused = search.read(cx).focus_handle.is_focused(window);
        let search_box = div()
            .debug_selector(|| "file-explorer-search".into())
            .flex_none()
            .mx_3()
            .mb_2()
            .h(px(32.0))
            .pl_3()
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded_lg()
            .bg(theme.INPUT_BG)
            .border_1()
            .border_color(if search_focused {
                theme.PANEL_BORDER_FOCUS
            } else {
                theme.INPUT_BORDER
            })
            .child(icon(
                include_bytes!("../../../assets/icons/files/filter.svg"),
                14.0,
                theme.TEXT_DIM,
            ))
            // The composer pads its text for chat; pull it back beside the icon.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .ml(px(-8.0))
                    .text_size(px(13.0))
                    .child(search),
            );

        let body = if searching {
            self.render_file_explorer_matches(cx)
        } else {
            self.render_file_explorer_tree(cx)
        };

        let width = clamp_width(
            self.file_explorer.width,
            f32::from(window.viewport_size().width),
        );
        div()
            .id("file-explorer")
            .debug_selector(|| "file-explorer".into())
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .min_h_0()
            .pt(px(top_inset))
            .flex()
            .flex_col()
            .bg(theme.HEADER_BG)
            .border_l_1()
            .border_color(theme.PANEL_BORDER)
            .child(header)
            .child(search_box)
            .child(body)
            .child(self.render_file_explorer_resize_handle(cx))
            .into_any_element()
    }

    /// Drag the left edge to resize; double-click restores the default width.
    fn render_file_explorer_resize_handle(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .id("file-explorer-resize-handle")
            .debug_selector(|| "file-explorer-resize-handle".into())
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(-4.0))
            .w(px(8.0))
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        this.file_explorer.width = FILE_EXPLORER_WIDTH;
                    } else {
                        this.file_explorer.resizing = true;
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Captures the pointer for the whole window while dragging, so moving
    /// over sessions or terminals cannot steal the drag.
    pub(super) fn render_file_explorer_resize_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let stop = |this: &mut Self, cx: &mut Context<Self>| {
            this.file_explorer.resizing = false;
            cx.notify();
        };
        div()
            .id("file-explorer-resize-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    if event.dragging() {
                        let viewport = f32::from(window.viewport_size().width);
                        this.file_explorer.width =
                            clamp_width(viewport - f32::from(event.position.x), viewport);
                    } else {
                        this.file_explorer.resizing = false;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    stop(this, cx);
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| stop(this, cx)),
            )
            .into_any_element()
    }

    fn render_file_explorer_tree(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let count = self
            .file_explorer
            .tree
            .as_ref()
            .map_or(0, |tree| tree.rows().len());
        uniform_list(
            "file-explorer-rows",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                let Some(tree) = this.file_explorer.tree.as_ref() else {
                    return Vec::new();
                };
                let selected = this.file_explorer.selected.clone();
                tree.rows()[range.start.min(tree.rows().len())..range.end.min(tree.rows().len())]
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(offset, row)| {
                        let index = range.start + offset;
                        let is_selected = selected.as_deref() == Some(row.path.as_path());
                        render_tree_row(index, row, is_selected, cx)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .debug_selector(|| "file-explorer-rows".into())
        .track_scroll(&self.file_explorer.scroll)
        .flex_1()
        .min_h_0()
        .px_2()
        .pb_2()
        .into_any_element()
    }

    fn render_file_explorer_matches(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let status = match &self.file_explorer.index {
            IndexState::Building(_) => Some("Indexing files…".to_owned()),
            IndexState::Ready(_, _) if self.file_explorer.matches.is_empty() => {
                Some("No matching files".to_owned())
            }
            IndexState::Ready(_, index) if index.truncated => Some(format!(
                "Showing matches from the first {} files",
                file_tree::MAX_INDEXED
            )),
            _ => None,
        };
        let matches = self.file_explorer.matches.clone();
        let list = uniform_list(
            "file-explorer-matches",
            matches.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(root) = this
                    .file_explorer
                    .tree
                    .as_ref()
                    .map(|tree| tree.root().to_path_buf())
                else {
                    return Vec::new();
                };
                let matches = this.file_explorer.matches.clone();
                matches[range.start.min(matches.len())..range.end.min(matches.len())]
                    .iter()
                    .enumerate()
                    .map(|(offset, relative)| {
                        render_match_row(range.start + offset, &root, relative, cx)
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .debug_selector(|| "file-explorer-matches".into())
        .track_scroll(&self.file_explorer.scroll)
        .flex_1()
        .min_h_0()
        .px_2()
        .pb_2();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when_some(status, |el, status| {
                el.child(
                    div()
                        .debug_selector(|| "file-explorer-search-status".into())
                        .px_4()
                        .py_1()
                        .text_size(px(11.0))
                        .text_color(theme.TEXT_DIM)
                        .child(status),
                )
            })
            .child(list)
            .into_any_element()
    }

    fn activate_file_explorer_row(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_explorer.selected = Some(path.clone());
        if is_dir {
            if let Some(tree) = self.file_explorer.tree.as_mut() {
                tree.toggle(&path);
            }
            cx.notify();
        } else {
            self.open_code_file(path, window, cx);
        }
    }
}

fn row_shell(id: gpui::ElementId, depth: usize, selected: bool) -> gpui::Stateful<gpui::Div> {
    let theme = Theme::global();
    div()
        .id(id)
        .h(px(ROW_HEIGHT))
        .w_full()
        .pl(px(8.0 + depth as f32 * INDENT))
        .pr_2()
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded_full()
        .text_size(px(12.5))
        .text_color(theme.TEXT)
        .when(selected, |el| el.bg(theme.ACCENT_DIM))
}

fn label(text: impl Into<gpui::SharedString>) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(text.into())
}

fn render_tree_row(
    index: usize,
    row: file_tree::Row,
    selected: bool,
    cx: &mut Context<Workspace>,
) -> gpui::AnyElement {
    let theme = Theme::global();
    let note = |text: gpui::SharedString| {
        row_shell(("file-explorer-note", index).into(), row.depth, false)
            .pl(px(8.0 + row.depth as f32 * INDENT + 20.0))
            .text_color(theme.TEXT_DIM)
            .text_size(px(11.5))
            .child(label(text))
            .into_any_element()
    };
    match &row.kind {
        RowKind::Loading => return note("Loading…".into()),
        RowKind::Empty => return note("Empty folder".into()),
        RowKind::Error(error) => return note(format!("Cannot read folder: {error}").into()),
        RowKind::File | RowKind::Dir { .. } => {}
    }
    let (is_dir, expanded) = match row.kind {
        RowKind::Dir { expanded } => (true, expanded),
        _ => (false, false),
    };
    let chevron: gpui::AnyElement = if is_dir {
        icon(
            if expanded {
                include_bytes!("../../../assets/icons/files/chevron-down.svg")
            } else {
                include_bytes!("../../../assets/icons/files/chevron-right.svg")
            },
            12.0,
            theme.TEXT_DIM,
        )
        .into_any_element()
    } else {
        div().w(px(12.0)).flex_none().into_any_element()
    };
    let kind_icon = if is_dir {
        icon(
            if expanded {
                include_bytes!("../../../assets/icons/files/folder-open.svg")
            } else {
                include_bytes!("../../../assets/icons/files/folder.svg")
            },
            15.0,
            theme.TEXT_DIM,
        )
    } else {
        icon(
            file_icon(file_tree::file_kind(&row.name)),
            15.0,
            theme.TEXT_DIM,
        )
    };
    let path = row.path.clone();
    let name = row.name.to_string();
    row_shell(
        gpui::SharedString::from(format!("file-explorer-row:{}", row.path.display())).into(),
        row.depth,
        selected,
    )
    .debug_selector(move || format!("file-explorer-row-{name}"))
    .cursor_pointer()
    .hover(|el| el.bg(theme.TOOL_BG))
    .on_click(cx.listener(move |this, _, window, cx| {
        this.activate_file_explorer_row(path.clone(), is_dir, window, cx);
    }))
    .child(chevron)
    .child(kind_icon)
    .child(label(row.name.to_string()))
    .into_any_element()
}

fn render_match_row(
    index: usize,
    root: &Path,
    relative: &Arc<str>,
    cx: &mut Context<Workspace>,
) -> gpui::AnyElement {
    let theme = Theme::global();
    let relative_path = Path::new(&**relative);
    let name = relative_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative.to_string());
    let parent = relative_path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    let path = root.join(relative_path);
    row_shell(("file-explorer-match", index).into(), 0, false)
        .debug_selector(move || format!("file-explorer-match-{index}"))
        .cursor_pointer()
        .hover(|el| el.bg(theme.TOOL_BG))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.activate_file_explorer_row(path.clone(), false, window, cx);
        }))
        .child(div().w(px(0.0)).flex_none())
        .child(icon(
            file_icon(file_tree::file_kind(&name)),
            15.0,
            theme.TEXT_DIM,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap_2()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(div().flex_none().child(name))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(11.0))
                        .text_color(theme.TEXT_DIM)
                        .child(parent),
                ),
        )
        .into_any_element()
}
