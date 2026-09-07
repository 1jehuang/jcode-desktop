//! The visible local default for Super+Enter, backed by the existing pinned preference.
use super::*;

impl Workspace {
    pub(super) fn open_default_directory_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_folder(&OpenFolder, window, cx);
        self.folder_picker_sets_default = true;
        self.folder_search = Some(self.create_folder_search(cx));
        self.folder_picker_dir = self
            .pinned_working_dir
            .clone()
            .or_else(default_working_dir)
            .map(PathBuf::from);
        cx.notify();
    }

    pub(super) fn set_searched_default_directory(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim();
        let path = if query == "~" {
            default_working_dir().map(PathBuf::from)
        } else if let Some(rest) = query.strip_prefix("~/") {
            default_working_dir().map(|home| PathBuf::from(home).join(rest))
        } else {
            let path = PathBuf::from(query);
            if path.is_absolute() {
                Some(path)
            } else {
                self.folder_picker_dir.as_ref().map(|base| base.join(path))
            }
        };
        if let Some(path) = path {
            self.set_default_directory(path, cx);
        } else {
            self.folder_picker_error = Some("Enter an existing directory path.".into());
            cx.notify();
        }
    }

    pub(super) fn set_default_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // Never silently substitute a fuzzy match or a file's parent for a preference.
        let result = (|| -> Result<String, String> {
            if !path.is_absolute() || !path.is_dir() {
                return Err(format!("Not an existing directory: {}", path.display()));
            }
            std::fs::read_dir(&path)
                .map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
            let directory = path
                .to_str()
                .ok_or_else(|| "Directory path is not valid UTF-8.".to_string())?
                .to_owned();
            crate::config::persist_pinned_working_dir(&directory)
                .map_err(|error| format!("Could not save default directory: {error}"))?;
            Ok(directory)
        })();
        match result {
            Ok(directory) => {
                self.pinned_working_dir = Some(directory);
                self.close_folder_picker(cx);
            }
            Err(error) => {
                self.folder_picker_error = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn default_directory_description(&self, browsing: &Path) -> String {
        let current = self
            .pinned_working_dir
            .clone()
            .or_else(default_working_dir)
            .unwrap_or_else(|| "Not set".into());
        let scope = match &self.remotes.default_host {
            Some(host) => format!(
                "{} currently uses {host}:~. This preference applies to This computer.",
                spawn_shortcut()
            ),
            None => format!("{} opens in {current}", spawn_shortcut()),
        };
        format!(
            "{scope}\nChoose a frequent folder or enter a path. Browsing: {}",
            browsing.display()
        )
    }

    pub(super) fn render_default_directory_button(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let path = self
            .pinned_working_dir
            .clone()
            .or_else(default_working_dir)
            .unwrap_or_else(|| "Not set".into());
        let display = match &self.remotes.default_host {
            Some(host) => format!("{host}:~ (remote home)"),
            None => compact_path(&path),
        };
        div()
            .id("default-directory-button")
            .debug_selector(|| "default-directory-button".into())
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(Theme::global().PANEL_BORDER)
            .cursor_pointer()
            .hover(|el| el.bg(Theme::global().TOOL_BG))
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.open_default_directory_picker(window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(Theme::global().TEXT_DIM)
                            .child("Default directory"),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(spawn_shortcut()),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "default-directory-path".into())
                    .text_size(px(11.0))
                    .text_color(Theme::global().ACCENT)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(display),
            )
            .into_any_element()
    }
}

pub(super) fn spawn_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Enter"
    } else {
        "Super+Enter"
    }
}

pub(super) fn compact_path(path: &str) -> String {
    if let Some(home) = default_working_dir() {
        if path == home {
            return "~".into();
        }
        if let Ok(relative) = Path::new(path).strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    path.into()
}

/// Rank local session history by use count, then recency, with stable path ties.
/// The current directory and its children remain available even with no history.
pub(super) fn ranked_directories(
    sessions: &[jcode_sdk::SessionInfo],
    base: &Path,
    query: &str,
) -> Vec<(PathBuf, String)> {
    let mut usage = HashMap::<PathBuf, (usize, usize)>::new();
    for (recency, session) in sessions.iter().rev().enumerate() {
        if session.session_id.starts_with("ssh://") {
            continue;
        }
        let Some(path) = session.working_dir.as_deref().map(PathBuf::from) else {
            continue;
        };
        if path.is_absolute() && path.is_dir() {
            let entry = usage.entry(path).or_insert((0, recency));
            entry.0 += 1;
        }
    }
    for path in
        std::iter::once(base.to_path_buf()).chain(directory_entries(base).unwrap_or_default())
    {
        if path.is_absolute() && path.is_dir() {
            usage.entry(path).or_insert((0, usize::MAX));
        }
    }
    let mut entries: Vec<_> = usage
        .into_iter()
        .filter(|(path, _)| path.to_string_lossy().to_lowercase().contains(query))
        .collect();
    entries.sort_by(|(a, (ac, ar)), (b, (bc, br))| bc.cmp(ac).then(ar.cmp(br)).then(a.cmp(b)));
    entries
        .into_iter()
        .map(|(path, (count, _))| {
            (
                path,
                match count {
                    0 => "Set as default".into(),
                    1 => "1 session · Set as default".into(),
                    _ => format!("{count} sessions · Set as default"),
                },
            )
        })
        .collect()
}
