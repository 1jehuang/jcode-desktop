//! Desktop self-development publishing: one button that commits, pushes,
//! releases and publishes Jcode Desktop from its own checkout, then tracks the
//! pipeline in a dedicated session panel.
//!
//! The release itself stays owned by the repository's documented tooling
//! (`scripts/release-desktop.py` and its GitHub workflows). This module only
//! detects eligible checkouts and describes the staged run to the agent, so the
//! tracker panel can show live stage progress from the session's todo list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Draft request IDs for publish trackers. The kind travels with the draft
/// through hot reloads and retargeting, exactly like help drafts.
pub(crate) const DRAFT_PREFIX: &str = "startup://draft/publish/";

/// Display title used for the tracker session.
pub(crate) const SESSION_TITLE: &str = "Publish Jcode Desktop";

/// Ordered pipeline stages. The agent is told to use these exact todo labels,
/// so the tracker can map live todo updates onto fixed stage rows.
pub(crate) const STAGES: [(&str, &str); 6] = [
    ("Commit", "Logical commits for current work"),
    ("Push", "Push and land on origin/main"),
    ("Release", "Version bump, changelog and preflight"),
    ("Tag", "Immutable tag and native build dispatch"),
    ("Build", "macOS, Linux, Windows and FreeBSD builds"),
    (
        "Publish",
        "Public downloads, website, acceptance, announcement",
    ),
];

/// The Jcode Desktop checkout containing `path`, if any. Identity comes from
/// the shared runtime's detector, so the button appears exactly when the agent
/// runs in Desktop self-development mode.
pub(crate) fn checkout_root(path: &str) -> Option<PathBuf> {
    thread_local! {
        static CACHE: RefCell<HashMap<String, Option<PathBuf>>> = RefCell::new(HashMap::new());
    }
    if path.trim().is_empty() {
        return None;
    }
    // Rendering asks every frame. Canonicalizing and parsing Cargo.toml is
    // cheap once but not per frame, and a checkout's identity is stable.
    CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(path) {
            return hit.clone();
        }
        let root = jcode_selfdev_types::desktop_repo_root(Path::new(path));
        let mut cache = cache.borrow_mut();
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(path.to_owned(), root.clone());
        root
    })
}

pub(crate) fn is_publish_draft(id: &str) -> bool {
    id.starts_with(DRAFT_PREFIX)
}

/// Staged instructions for the tracker session. The first todo write must use
/// the exact stage labels so the tracker header can follow progress.
pub(crate) fn session_prompt(root: &Path) -> String {
    let stages = STAGES
        .iter()
        .map(|(name, detail)| format!("- \"{name}: {detail}\""))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"Commit, push, release, and publish Jcode Desktop from the checkout at {root}.

This session is the Desktop publish tracker. Its todo list is rendered as the live progress panel, so before doing anything else write a todo list with exactly these six items, in this order, all pending:
{stages}
Keep each item's text unchanged. Mark an item in_progress when its stage starts and completed only after its observable gate passes. If a stage fails or is blocked, leave it in_progress, add a short blocked_by reason, stop, and report instead of working around the gate.

Stages:
1. Commit: inspect git state in {root}, make logical commits for the current uncommitted work, preserve unrelated changes, and validate appropriately. Follow AGENTS.md, including the Git identity guard. Never override the configured identity.
2. Push: push without force. Releases are cut from origin/main, so if the work is on another branch, land it on main using the repository's established convention (fast-forward or merge, never a force push or history rewrite). Stop if that cannot be done safely.
3. Release: fetch origin, determine the next version from existing desktop-v* tags and the user-visible changes, then follow docs/release-orchestration.md. Bump only the four package manifests and their own Cargo.lock entries, update CHANGELOG.md, confirm the runtime pins, run the release contract tests, commit, and push to main. Run `python3 scripts/release-desktop.py <version>` without --apply first and read its plan.
4. Tag: run `python3 scripts/release-desktop.py <version> --apply` in the background with progress notifications. It owns tagging and build dispatch. Never move an existing tag.
5. Build: monitor the orchestrator's JSON gate lines until the macOS and cross-platform build workflows succeed. Report run URLs as they appear.
6. Publish: keep monitoring until the public-download publisher, macOS installation acceptance, Discord announcement, and website manifest verification all pass. A green build alone is not a published release.

If the orchestrator is interrupted, re-run the same command to resume. Never bypass gates, retry failed workflows automatically, or upload assets manually. Finish with a short summary of commits, version, tag, and every workflow run URL."#,
        root = root.display(),
    )
}

/// Tracker state derived from the latest todo snapshot, keyed by stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageState {
    Pending,
    Running,
    Blocked,
    Done,
}

/// Map todo items onto the fixed stages. Items are matched by their leading
/// stage label, so extra sub-items the agent adds never break the tracker.
pub(crate) fn stage_states<'a>(
    todos: impl IntoIterator<Item = (&'a str, &'a str, bool)>,
) -> [StageState; STAGES.len()] {
    let mut states = [StageState::Pending; STAGES.len()];
    for (content, status, blocked) in todos {
        let label = content.trim().split(':').next().unwrap_or_default().trim();
        let Some(index) = STAGES
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case(label))
        else {
            continue;
        };
        states[index] = match status {
            "completed" => StageState::Done,
            _ if blocked => StageState::Blocked,
            "in_progress" => StageState::Running,
            "cancelled" => StageState::Blocked,
            _ => StageState::Pending,
        };
    }
    states
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_names_every_stage_and_the_release_orchestrator() {
        let prompt = session_prompt(Path::new("/src/jcode-desktop"));
        for (name, detail) in STAGES {
            assert!(prompt.contains(&format!("\"{name}: {detail}\"")), "{name}");
        }
        assert!(prompt.contains("/src/jcode-desktop"));
        assert!(prompt.contains("scripts/release-desktop.py <version> --apply"));
        assert!(prompt.contains("Never move an existing tag"));
        assert!(prompt.contains("without force"));
    }

    #[test]
    fn stage_states_follow_todo_labels_and_ignore_extras() {
        let states = stage_states([
            (
                "Commit: Logical commits for current work",
                "completed",
                false,
            ),
            ("push: anything", "in_progress", false),
            ("Release: bump", "in_progress", true),
            ("Unrelated sub-step", "completed", false),
            ("Publish: downloads", "pending", false),
        ]);
        assert_eq!(
            states,
            [
                StageState::Done,
                StageState::Running,
                StageState::Blocked,
                StageState::Pending,
                StageState::Pending,
                StageState::Pending,
            ]
        );
    }

    #[test]
    fn detects_only_desktop_checkouts() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("crates/jcode-desktop-ui/src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = 'jcode-desktop'\nversion = '0.1.0'\n",
        )
        .unwrap();
        let nested = root.path().join("crates/jcode-desktop-ui/src");
        assert_eq!(
            checkout_root(nested.to_str().unwrap()),
            Some(root.path().canonicalize().unwrap())
        );
        let other = tempfile::tempdir().unwrap();
        assert_eq!(checkout_root(other.path().to_str().unwrap()), None);
        assert_eq!(checkout_root(""), None);
    }

    #[test]
    fn publish_drafts_are_pending_sessions() {
        let id = format!("{DRAFT_PREFIX}1-0");
        assert!(is_publish_draft(&id));
        assert!(crate::panel::Panel::is_pending_session_id(&id));
        assert!(!is_publish_draft("startup://draft/help/1-0"));
    }
}
