//! Dependency-free build metadata, also tested directly with `rustc --test`.
use std::{path::Path, process::Command};

/// Read the root application's base, not the UI library's independent version.
/// Like the CLI build helper, this supports a literal package version without
/// bringing a TOML dependency into the build script.
pub fn root_package_version(root: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.split('#').next()?.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "version" {
                let value = value.trim();
                let version = value.strip_prefix('"')?.strip_suffix('"')?;
                return parse_base(version).map(|_| version.to_owned());
            }
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
pub struct VersionMetadata {
    /// Machine-readable package/update identity, never the auto-numbered version.
    pub version: String,
    /// Presentation semver. Git identity is supplied separately.
    pub display_version: String,
    pub git_hash: String,
    pub git_dirty: bool,
    pub development: bool,
}

pub fn resolve(root: &Path, base: &str, explicit: Option<&str>) -> VersionMetadata {
    let git_hash = git(root, &["rev-parse", "--short", "HEAD"])
        .filter(|hash| !hash.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let git_dirty = git(root, &["status", "--porcelain", "--untracked-files=normal"])
        .is_some_and(|status| !status.is_empty());
    let version = explicit.unwrap_or(base).to_owned();
    let display_version = explicit.map(str::to_owned).unwrap_or_else(|| {
        let (major, minor, patch) =
            parse_base(base).expect("Cargo package version must have a numeric semver core");
        let offset = commit_offset(root, &format!("{major}.{minor}.{patch}")).unwrap_or(0);
        format!("{major}.{minor}.{patch}-dev.{offset}")
    });
    VersionMetadata {
        version,
        display_version,
        git_hash,
        git_dirty,
        development: explicit.is_none(),
    }
}

fn parse_base(base: &str) -> Option<(u32, u32, u32)> {
    let core = base.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let result = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(result)
}

fn commit_offset(root: &Path, base: &str) -> Option<u32> {
    // Keep commit distance in the prerelease component, never the release
    // patch. Only reachable stable tags at or below the intended package base
    // can anchor it. This handles a newly bumped minor with no tag of its own
    // and ignores tags on other branches or for newer release lines.
    let base = parse_base(base)?;
    let tags = git(root, &["tag", "--merged", "HEAD"])?;
    for prefix in ["desktop-v", "v"] {
        let anchor = tags
            .lines()
            .filter_map(|tag| {
                let version = tag.strip_prefix(prefix)?;
                let parsed = parse_base(version)?;
                let canonical = format!("{}.{}.{}", parsed.0, parsed.1, parsed.2);
                (version == canonical && parsed <= base).then_some((parsed, tag))
            })
            .max_by_key(|(version, _)| *version);
        if let Some((_, tag)) = anchor {
            return git(
                root,
                &["rev-list", "--count", &format!("refs/tags/{tag}..HEAD")],
            )?
            .parse()
            .ok();
        }
    }
    git(root, &["rev-list", "--count", "HEAD"])?.parse().ok()
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn directory() -> Self {
            let parent = std::env::var_os("JCODE_SCRATCH_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let path = parent.join(format!(
                "desktop-version-tests-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn new() -> Self {
            let fixture = Self::directory();
            fixture.run(&["init", "-q", "--initial-branch=main"]);
            // Fixture commits use the caller's configured Git identity.
            fixture.run(&["config", "commit.gpgsign", "false"]);
            fixture.run(&["config", "tag.gpgsign", "false"]);
            fixture.commit();
            fixture
        }
        fn run(&self, args: &[&str]) -> String {
            git(&self.0, args)
                .unwrap_or_else(|| panic!("git {args:?} failed in {}", self.0.display()))
        }
        fn commit(&self) {
            let message = format!(
                "Version fixture {}",
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            );
            self.run(&["commit", "--allow-empty", "-qm", &message]);
        }
        fn metadata(&self) -> VersionMetadata {
            resolve(&self.0, "0.1.0", None)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn untagged_history_increases_and_is_repeatable() {
        let repo = Fixture::new();
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.1");
        assert_eq!(repo.metadata(), repo.metadata());
        repo.commit();
        let metadata = repo.metadata();
        assert_eq!(metadata.display_version, "0.1.0-dev.2");
        assert_eq!(metadata.version, "0.1.0");
        assert!(metadata.development);
        assert!(!metadata.git_dirty);
        assert_eq!(
            metadata.git_hash,
            repo.run(&["rev-parse", "--short", "HEAD"])
        );
    }

    #[test]
    fn desktop_base_tag_and_nonzero_patch() {
        let repo = Fixture::new();
        repo.run(&["tag", "desktop-v1.2.7"]);
        assert_eq!(
            resolve(&repo.0, "1.2.7", None).display_version,
            "1.2.7-dev.0"
        );
        repo.commit();
        repo.commit();
        assert_eq!(
            resolve(&repo.0, "1.2.7", None).display_version,
            "1.2.7-dev.2"
        );
    }

    #[test]
    fn cli_style_annotated_tag_supported_and_desktop_tag_preferred() {
        let repo = Fixture::new();
        repo.run(&["tag", "-am", "Base", "v0.1.0"]);
        repo.commit();
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.1");
        repo.run(&["tag", "desktop-v0.1.0"]);
        repo.commit();
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.1");
        repo.run(&["pack-refs", "--all"]);
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.1");
    }

    #[test]
    fn beta_tags_do_not_reset_numbering() {
        let repo = Fixture::new();
        repo.run(&["tag", "desktop-v0.1.0-beta.1"]);
        repo.commit();
        repo.run(&["tag", "desktop-v0.1.0-beta.30"]);
        repo.commit();
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.3");
    }

    #[test]
    fn next_minor_uses_latest_reachable_stable_release() {
        let repo = Fixture::new();
        repo.run(&["tag", "desktop-v0.2.0"]);
        repo.commit();
        repo.run(&["tag", "-am", "Release", "desktop-v0.2.1"]);
        repo.commit();
        repo.run(&["tag", "desktop-v0.2.2-beta.1"]);
        repo.commit();
        let metadata = resolve(&repo.0, "0.3.0", None);
        assert_eq!(metadata.version, "0.3.0");
        assert_eq!(metadata.display_version, "0.3.0-dev.2");
        repo.run(&["pack-refs", "--all"]);
        assert_eq!(resolve(&repo.0, "0.3.0", None), metadata);
    }

    #[test]
    fn unrelated_and_future_tags_do_not_anchor_development_count() {
        let repo = Fixture::new();
        repo.run(&["tag", "desktop-v0.2.0"]);
        repo.run(&["checkout", "-qb", "future"]);
        repo.commit();
        repo.run(&["tag", "desktop-v0.3.0"]);
        repo.run(&["checkout", "-q", "main"]);
        repo.commit();
        repo.run(&["tag", "desktop-v9.0.0"]);
        repo.run(&["tag", "desktop-v0.2.1+metadata"]);
        repo.run(&["tag", "desktop-v00.2.1"]);
        assert_eq!(
            resolve(&repo.0, "0.3.0", None).display_version,
            "0.3.0-dev.1"
        );
    }

    #[test]
    fn archive_without_git_retains_intended_release_base() {
        let repo = Fixture::directory();
        let metadata = resolve(&repo.0, "0.3.0", None);
        assert_eq!(metadata.display_version, "0.3.0-dev.0");
        assert_eq!(metadata.git_hash, "unknown");
        assert!(metadata.development);
    }

    #[test]
    fn dirty_status_does_not_change_number() {
        let repo = Fixture::new();
        fs::write(repo.0.join("tracked"), "one").unwrap();
        assert!(repo.metadata().git_dirty); // untracked
        repo.run(&["add", "tracked"]);
        assert!(repo.metadata().git_dirty); // staged
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.1");
        repo.commit();
        assert!(!repo.metadata().git_dirty);
        fs::write(repo.0.join("tracked"), "two").unwrap();
        assert!(repo.metadata().git_dirty); // unstaged
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.2");
    }

    #[test]
    fn packaging_override_is_exact_even_when_dirty() {
        let repo = Fixture::new();
        fs::write(repo.0.join("untracked"), "dirty").unwrap();
        for version in ["0.1.0-beta.30", "1.2.3", "1.2.3-rc.1+build.5"] {
            let metadata = resolve(&repo.0, "0.1.0", Some(version));
            assert_eq!(metadata.version, version);
            assert_eq!(metadata.display_version, version);
            assert!(!metadata.development);
            assert!(metadata.git_dirty);
        }
    }

    #[test]
    fn empty_repository_falls_back_without_panicking() {
        let repo = Fixture::directory();
        repo.run(&["init", "-q"]);
        let metadata = repo.metadata();
        assert_eq!(metadata.display_version, "0.1.0-dev.0");
        assert_eq!(metadata.git_hash, "unknown");
        assert!(!metadata.git_dirty);
    }

    #[test]
    fn linked_worktree_and_detached_head() {
        let repo = Fixture::new();
        repo.run(&["tag", "desktop-v0.1.0"]);
        repo.commit();
        let worktree = Fixture::directory();
        repo.run(&[
            "worktree",
            "add",
            "--detach",
            worktree.0.to_str().unwrap(),
            "HEAD",
        ]);
        assert_eq!(worktree.metadata(), repo.metadata());
    }

    #[test]
    fn merges_count_all_reachable_commits_like_cli() {
        let repo = Fixture::new();
        repo.run(&["tag", "desktop-v0.1.0"]);
        repo.run(&["checkout", "-qb", "feature"]);
        repo.commit();
        repo.run(&["checkout", "-q", "main"]);
        repo.commit();
        repo.run(&["merge", "--no-ff", "-qm", "Merge fixture", "feature"]);
        assert_eq!(repo.metadata().display_version, "0.1.0-dev.3");
    }

    #[test]
    fn numeric_core_preserves_large_patch() {
        assert_eq!(parse_base("1.2.3-beta.1+abc"), Some((1, 2, 3)));
        assert_eq!(parse_base("1.2.3.4"), None);
        assert_eq!(parse_base("bad"), None);
        let repo = Fixture::new();
        assert_eq!(
            resolve(&repo.0, "1.2.4294967295", None).display_version,
            "1.2.4294967295-dev.1"
        );
    }

    #[test]
    fn root_package_base_ignores_other_sections_and_comments() {
        let repo = Fixture::directory();
        assert_eq!(root_package_version(&repo.0), None);
        fs::write(repo.0.join("Cargo.toml"), "[workspace.package]\nversion = \"9.9.9\"\n[package] # application\nversion = \"1.2.3\" # base\n[dependencies]\nversion = \"8.8.8\"\n").unwrap();
        assert_eq!(root_package_version(&repo.0).as_deref(), Some("1.2.3"));
        fs::write(
            repo.0.join("Cargo.toml"),
            "[package]\nversion.workspace = true\n",
        )
        .unwrap();
        assert_eq!(root_package_version(&repo.0), None);
    }
}
