//! Project file tree for the right-hand Files panel.
//!
//! Rendering must never touch the filesystem. Directories are listed once, off
//! the UI thread, when they first become visible, and the tree keeps a flat
//! list of visible rows so a virtualized list can draw only what is on screen.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Version-control internals and OS metadata are never useful in the tree.
/// Other dotfiles such as `.env` or `.github` stay visible.
pub const HIDDEN_NAMES: &[&str] = &[".git", ".hg", ".svn", ".jj", ".DS_Store"];
/// Upper bound on indexed paths so a home directory cannot exhaust memory.
pub const MAX_INDEXED: usize = 200_000;
/// Upper bound on search results, which are rendered in a virtualized list.
pub const MAX_MATCHES: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: Arc<str>,
    pub is_dir: bool,
}

/// List one directory: folders first, then case-insensitive name order.
/// Symlinks to directories are shown as directories.
pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if HIDDEN_NAMES.contains(&name.as_str()) {
            continue;
        }
        let is_dir = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => entry.path().is_dir(),
            Ok(kind) => kind.is_dir(),
            Err(_) => false,
        };
        entries.push(Entry {
            name: name.into(),
            is_dir,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Listing {
    Loading,
    Loaded(Arc<[Entry]>),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    File,
    Dir {
        expanded: bool,
    },
    /// Placeholder while a directory is being listed.
    Loading,
    /// A directory could not be listed (permissions, removed, ...).
    Error(String),
    /// An expanded directory with no visible entries.
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    pub name: Arc<str>,
    pub depth: usize,
    pub kind: RowKind,
}

/// Expanded state and cached listings for one root directory.
#[derive(Debug)]
pub struct FileTree {
    root: PathBuf,
    listings: HashMap<PathBuf, Listing>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    /// Visible directories with no listing yet, in display order.
    pending: Vec<PathBuf>,
}

impl FileTree {
    pub fn new(root: PathBuf) -> Self {
        let mut tree = Self {
            root,
            listings: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            pending: Vec::new(),
        };
        tree.rebuild();
        tree
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Visible rows in display order, excluding the root itself.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Directories the caller must list with [`list_dir`] and report back via
    /// [`FileTree::set_listing`]. Each directory is handed out only once.
    pub fn take_pending(&mut self) -> Vec<PathBuf> {
        let pending = std::mem::take(&mut self.pending);
        for dir in &pending {
            self.listings.insert(dir.clone(), Listing::Loading);
        }
        pending
    }

    /// Store a directory listing. Results for directories that are no longer
    /// part of this tree are ignored.
    pub fn set_listing(&mut self, dir: &Path, result: Result<Vec<Entry>, String>) {
        if !dir.starts_with(&self.root) {
            return;
        }
        let listing = match result {
            Ok(entries) => Listing::Loaded(entries.into()),
            Err(error) => Listing::Failed(error),
        };
        self.listings.insert(dir.to_path_buf(), listing);
        self.rebuild();
    }

    /// Expand or collapse a directory. Nested expansion state is remembered
    /// while an ancestor is collapsed, like editor file trees.
    pub fn toggle(&mut self, dir: &Path) {
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_path_buf());
        }
        self.rebuild();
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        self.rebuild();
    }

    /// Directories to list again for a manual refresh: the root and every
    /// loaded, visible directory. Current rows stay on screen until each new
    /// listing arrives, so refreshing never flashes a loading state.
    pub fn refresh_targets(&self) -> Vec<PathBuf> {
        std::iter::once(self.root.clone())
            .chain(
                self.rows
                    .iter()
                    .filter(|row| matches!(row.kind, RowKind::Dir { expanded: true }))
                    .map(|row| row.path.clone()),
            )
            .filter(|dir| matches!(self.listings.get(dir), Some(Listing::Loaded(_))))
            .collect()
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::with_capacity(self.rows.len());
        let mut pending = Vec::new();
        let root = self.root.clone();
        self.push_children(&root, 0, &mut rows, &mut pending);
        self.rows = rows;
        self.pending = pending;
    }

    fn push_children(
        &self,
        dir: &Path,
        depth: usize,
        rows: &mut Vec<Row>,
        pending: &mut Vec<PathBuf>,
    ) {
        let placeholder = |kind| Row {
            path: dir.to_path_buf(),
            name: "".into(),
            depth,
            kind,
        };
        match self.listings.get(dir) {
            None => {
                pending.push(dir.to_path_buf());
                rows.push(placeholder(RowKind::Loading));
            }
            Some(Listing::Loading) => rows.push(placeholder(RowKind::Loading)),
            Some(Listing::Failed(error)) => rows.push(placeholder(RowKind::Error(error.clone()))),
            Some(Listing::Loaded(entries)) if entries.is_empty() => {
                rows.push(placeholder(RowKind::Empty));
            }
            Some(Listing::Loaded(entries)) => {
                for entry in entries.iter() {
                    let path = dir.join(&*entry.name);
                    if entry.is_dir {
                        let expanded = self.expanded.contains(&path);
                        rows.push(Row {
                            path: path.clone(),
                            name: entry.name.clone(),
                            depth,
                            kind: RowKind::Dir { expanded },
                        });
                        if expanded {
                            self.push_children(&path, depth + 1, rows, pending);
                        }
                    } else {
                        rows.push(Row {
                            path,
                            name: entry.name.clone(),
                            depth,
                            kind: RowKind::File,
                        });
                    }
                }
            }
        }
    }
}

/// Every file below a root, relative to it, for name search.
#[derive(Clone, Debug, Default)]
pub struct FileIndex {
    paths: Vec<Arc<str>>,
    lowercase: Vec<Box<str>>,
    pub truncated: bool,
}

impl FileIndex {
    /// Walk `root`, honoring `.gitignore`, `.ignore`, and global git excludes
    /// so dependency and build folders do not drown out project files. Hidden
    /// files are included. Blocking: call off the UI thread.
    pub fn build(root: &Path) -> Self {
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .filter_entry(|entry| {
                !HIDDEN_NAMES.contains(&entry.file_name().to_string_lossy().as_ref())
            })
            .build();
        let mut index = Self::default();
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let Ok(relative) = entry.path().strip_prefix(root) else {
                continue;
            };
            if index.paths.len() >= MAX_INDEXED {
                index.truncated = true;
                break;
            }
            index.push(relative.to_string_lossy().as_ref());
        }
        index
    }

    fn push(&mut self, path: &str) {
        self.lowercase.push(path.to_lowercase().into());
        self.paths.push(path.into());
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Case-insensitive search. Every whitespace-separated term must occur in
    /// the relative path. Matches in the file name rank above matches only in
    /// a parent folder, then shorter paths first.
    pub fn search(&self, query: &str) -> Vec<Arc<str>> {
        let query = query.trim().to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<(u8, usize, usize)> = self
            .lowercase
            .iter()
            .enumerate()
            .filter(|(_, path)| terms.iter().all(|term| path.contains(term)))
            .map(|(index, path)| {
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                let rank = if name.starts_with(&query) {
                    0
                } else if name.contains(&query) {
                    1
                } else if terms.iter().all(|term| name.contains(term)) {
                    2
                } else {
                    3
                };
                (rank, path.len(), index)
            })
            .collect();
        let keep = MAX_MATCHES.min(matches.len());
        if keep < matches.len() {
            matches.select_nth_unstable(keep);
            matches.truncate(keep);
        }
        matches.sort_unstable();
        matches
            .into_iter()
            .map(|(_, _, index)| self.paths[index].clone())
            .collect()
    }
}

/// Coarse file category for choosing an icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Code,
    Config,
    Text,
    Image,
    Secret,
    Lock,
    Other,
}

pub fn file_kind(name: &str) -> FileKind {
    let lower = name.to_ascii_lowercase();
    if lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
    {
        return FileKind::Secret;
    }
    if lower.ends_with(".lock") || lower.ends_with("-lock.json") || lower.ends_with(".lockb") {
        return FileKind::Lock;
    }
    if matches!(
        lower.as_str(),
        "dockerfile"
            | "makefile"
            | "justfile"
            | ".gitignore"
            | ".gitattributes"
            | ".editorconfig"
            | ".npmrc"
            | ".nvmrc"
            | ".prettierrc"
            | ".eslintrc"
    ) {
        return FileKind::Config;
    }
    let extension = lower
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension);
    match extension {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" | "py" | "go"
        | "rb" | "php" | "java" | "kt" | "swift" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs"
        | "sh" | "bash" | "zsh" | "fish" | "lua" | "zig" | "dart" | "scala" | "ex" | "exs"
        | "html" | "css" | "scss" | "sass" | "less" | "sql" | "graphql" | "nix" => FileKind::Code,
        "json" | "jsonc" | "toml" | "yaml" | "yml" | "xml" | "ini" | "cfg" | "conf" | "env"
        | "plist" => FileKind::Config,
        "md" | "mdx" | "txt" | "rst" | "adoc" | "csv" | "log" | "pdf" => FileKind::Text,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "avif" | "heic" => {
            FileKind::Image
        }
        _ => FileKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn names(tree: &FileTree) -> Vec<(usize, String, RowKind)> {
        tree.rows()
            .iter()
            .map(|row| (row.depth, row.name.to_string(), row.kind.clone()))
            .collect()
    }

    /// Mimic the UI: list every pending directory synchronously.
    fn load(tree: &mut FileTree) {
        loop {
            let pending = tree.take_pending();
            if pending.is_empty() {
                break;
            }
            for dir in pending {
                tree.set_listing(&dir, list_dir(&dir).map_err(|error| error.to_string()));
            }
        }
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "src/main.rs",
            "src/ui/app.rs",
            "README.md",
            ".env",
            "b.txt",
            "A.txt",
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join(".DS_Store"), "x").unwrap();
        fs::create_dir_all(root.join("empty")).unwrap();
        dir
    }

    #[test]
    fn lists_folders_first_case_insensitively_and_hides_vcs_internals() {
        let dir = fixture();
        let names: Vec<_> = list_dir(dir.path())
            .unwrap()
            .into_iter()
            .map(|entry| entry.name.to_string())
            .collect();
        assert_eq!(
            names,
            ["empty", "src", ".env", "A.txt", "b.txt", "README.md"]
        );
    }

    #[test]
    fn rows_load_lazily_and_expand_without_rereading_cached_directories() {
        let dir = fixture();
        let mut tree = FileTree::new(dir.path().to_path_buf());
        assert_eq!(names(&tree), [(0, String::new(), RowKind::Loading)]);
        assert_eq!(tree.take_pending(), [dir.path().to_path_buf()]);
        assert!(
            tree.take_pending().is_empty(),
            "a directory is requested once"
        );
        load_dir(&mut tree, dir.path());

        let src = dir.path().join("src");
        tree.toggle(&src);
        assert_eq!(tree.take_pending(), [src.clone()]);
        assert!(names(&tree).contains(&(1, String::new(), RowKind::Loading)));
        load_dir(&mut tree, &src);
        let ui = src.join("ui");
        tree.toggle(&ui);
        load(&mut tree);
        assert_eq!(
            names(&tree),
            [
                (0, "empty".into(), RowKind::Dir { expanded: false }),
                (0, "src".into(), RowKind::Dir { expanded: true }),
                (1, "ui".into(), RowKind::Dir { expanded: true }),
                (2, "app.rs".into(), RowKind::File),
                (1, "main.rs".into(), RowKind::File),
                (0, ".env".into(), RowKind::File),
                (0, "A.txt".into(), RowKind::File),
                (0, "b.txt".into(), RowKind::File),
                (0, "README.md".into(), RowKind::File),
            ]
        );

        // Collapsing hides children but remembers nested expansion.
        tree.toggle(&src);
        assert_eq!(tree.rows().len(), 6);
        tree.toggle(&src);
        assert!(tree.take_pending().is_empty(), "cached listings are reused");
        assert_eq!(tree.rows().len(), 9);
        assert_eq!(
            tree.refresh_targets(),
            [dir.path().to_path_buf(), src.clone(), ui.clone()]
        );

        tree.collapse_all();
        assert_eq!(tree.rows().len(), 6);
    }

    fn load_dir(tree: &mut FileTree, dir: &Path) {
        tree.set_listing(dir, list_dir(dir).map_err(|error| error.to_string()));
    }

    #[test]
    fn empty_and_unreadable_directories_get_a_placeholder_row() {
        let dir = fixture();
        let mut tree = FileTree::new(dir.path().to_path_buf());
        load(&mut tree);
        let empty = dir.path().join("empty");
        tree.toggle(&empty);
        load(&mut tree);
        assert!(names(&tree).contains(&(1, String::new(), RowKind::Empty)));

        let missing = dir.path().join("missing");
        let mut tree = FileTree::new(missing.clone());
        load(&mut tree);
        assert!(matches!(tree.rows()[0].kind, RowKind::Error(_)));

        // Late results from another root are ignored.
        let mut tree = FileTree::new(dir.path().join("src"));
        tree.set_listing(&missing, Ok(Vec::new()));
        assert_eq!(tree.rows()[0].kind, RowKind::Loading);
    }

    #[test]
    fn index_respects_gitignore_and_ranks_file_name_matches_first() {
        let dir = fixture();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/main.rs"), "x").unwrap();
        fs::create_dir_all(root.join("main")).unwrap();
        fs::write(root.join("main/other.txt"), "x").unwrap();

        let index = FileIndex::build(root);
        assert!(!index.truncated);
        let sep = std::path::MAIN_SEPARATOR;
        let all = index.search("t");
        assert!(all.iter().all(|path| !path.starts_with("node_modules")));
        assert!(
            all.iter()
                .all(|path| !path.starts_with(&format!(".git{sep}")))
        );
        assert!(all.iter().any(|path| &**path == ".gitignore"));

        assert_eq!(
            index.search("MAIN"),
            [format!("src{sep}main.rs"), format!("main{sep}other.txt")]
                .map(|path| Arc::<str>::from(path))
        );
        assert_eq!(
            index.search("ui app"),
            [Arc::<str>::from(format!("src{sep}ui{sep}app.rs"))]
        );
        assert!(index.search("   ").is_empty());
        assert!(index.search("nothing-matches").is_empty());
    }

    #[test]
    fn file_kinds_cover_common_project_files() {
        assert_eq!(file_kind("main.rs"), FileKind::Code);
        assert_eq!(file_kind("nuxt.config.ts"), FileKind::Code);
        assert_eq!(file_kind("package.json"), FileKind::Config);
        assert_eq!(file_kind(".gitignore"), FileKind::Config);
        assert_eq!(file_kind(".env.example"), FileKind::Secret);
        assert_eq!(file_kind("bun.lock"), FileKind::Lock);
        assert_eq!(file_kind("package-lock.json"), FileKind::Lock);
        assert_eq!(file_kind("README.md"), FileKind::Text);
        assert_eq!(file_kind("logo.SVG"), FileKind::Image);
        assert_eq!(file_kind("data.bin"), FileKind::Other);
    }
}
