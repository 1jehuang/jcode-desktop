//! Offline update summaries and version-grouped history, following the TUI.
//! Rendering is pure. Only startup acknowledgement advances the seen commit.
use std::{fs, path::Path, sync::OnceLock};

const SUMMARY_LIMIT: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
    #[default]
    Latest,
    History,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    hash: String,
    version: Option<String>,
    date: String,
    subject: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub version: String,
    pub date: String,
    pub entries: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    FirstVisit,
    New(usize),
    OutsideHistory,
}

static SEEN: OnceLock<Seen> = OnceLock::new();

fn parse(raw: &str) -> Vec<Entry> {
    raw.split('\x1f')
        .filter_map(|record| {
            let mut fields = record.trim().splitn(4, '\x1e');
            let hash = fields.next()?.trim();
            let refs = fields.next()?;
            let date = fields.next()?.trim();
            let subject = fields.next()?.trim();
            if hash.is_empty() || subject.is_empty() {
                return None;
            }
            // Ignore branch names and non-release tags. Prefer the highest
            // version when multiple release tags happen to share a commit.
            let version = refs
                .split(',')
                .filter_map(|reference| {
                    let tag = reference.trim().strip_prefix("tag: ")?;
                    let label = tag.strip_prefix("desktop-").unwrap_or(tag);
                    let version = semver::Version::parse(label.strip_prefix('v')?).ok()?;
                    Some((version, label.to_owned()))
                })
                .max_by(|a, b| a.0.cmp(&b.0))
                .map(|(_, label)| label);
            Some(Entry {
                hash: hash.into(),
                version,
                date: date.into(),
                subject: subject.into(),
            })
        })
        .collect()
}

fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        parse(include_str!(concat!(
            env!("OUT_DIR"),
            "/update-history.txt"
        )))
    })
}

fn unseen(entries: &[Entry], previous: Option<&str>) -> Seen {
    match previous.map(str::trim).filter(|hash| !hash.is_empty()) {
        None => Seen::FirstVisit,
        Some(hash) => entries
            .iter()
            .position(|entry| entry.hash == hash)
            .map(Seen::New)
            .unwrap_or(Seen::OutsideHistory),
    }
}

fn acknowledge(entries: &[Entry], path: &Path) -> Seen {
    let previous = fs::read_to_string(path).ok();
    let seen = unseen(entries, previous.as_deref());
    if let Some(latest) = entries.first() {
        // Best effort, just like the build marker. Never make startup depend
        // on a writable home directory, and never write during rendering.
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, &latest.hash);
    }
    seen
}

pub(crate) fn capture_unseen(path: &Path, shown: bool) {
    SEEN.get_or_init(|| {
        if shown {
            acknowledge(entries(), path)
        } else {
            unseen(entries(), fs::read_to_string(path).ok().as_deref())
        }
    });
}

fn group(entries: &[Entry], current_version: &str) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for entry in entries {
        if groups.is_empty() || entry.version.is_some() {
            groups.push(Group {
                version: entry
                    .version
                    .clone()
                    .unwrap_or_else(|| format!("v{current_version} · Unreleased")),
                date: entry.date.clone(),
                entries: Vec::new(),
            });
        }
        groups
            .last_mut()
            .unwrap()
            .entries
            .push(entry.subject.clone());
    }
    groups
}

pub(crate) fn history() -> Vec<Group> {
    group(entries(), crate::build_info::VERSION)
}

pub(crate) struct Summary {
    pub heading: String,
    pub description: &'static str,
    pub entries: Vec<String>,
    pub remaining: usize,
}

fn summarize(entries: &[Entry], seen: Seen) -> Summary {
    let (heading, description, count) = match seen {
        Seen::New(0) => (
            "No new changes in this build".into(),
            "You’ve already seen these commits. Recent changes are listed below.",
            entries.len(),
        ),
        Seen::New(count) => (
            format!(
                "{} new {}",
                count,
                if count == 1 { "change" } else { "changes" }
            ),
            "Included in this build since your last visit.",
            count,
        ),
        Seen::FirstVisit => (
            "Latest updates".into(),
            "A quick look at what’s changed in Jcode Desktop.",
            entries.len(),
        ),
        Seen::OutsideHistory => (
            "Latest updates".into(),
            "Your previous build is outside the included history. Showing recent changes.",
            entries.len(),
        ),
    };
    Summary {
        heading,
        description,
        entries: entries
            .iter()
            .take(count.min(SUMMARY_LIMIT))
            .map(|entry| entry.subject.clone())
            .collect(),
        remaining: count.saturating_sub(SUMMARY_LIMIT),
    }
}

pub(crate) fn summary() -> Summary {
    summarize(entries(), SEEN.get().copied().unwrap_or(Seen::FirstVisit))
}

/// Editorial release notes are separate from commit history and unseen counts.
/// Missing editorial notes are represented by `None`, not another release.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReleaseOverview {
    pub title: String,
    pub sections: Vec<ReleaseSection>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReleaseSection {
    pub label: &'static str,
    pub entries: Vec<String>,
}

fn release_version(raw: &str) -> Option<semver::Version> {
    let mut version = semver::Version::parse(raw.trim().trim_start_matches('v')).ok()?;
    // Build metadata does not identify a different release. Other prerelease
    // channels must match exactly, never borrow notes from a future stable tag.
    version.build = semver::BuildMetadata::EMPTY;
    Some(version)
}

fn overview(raw: &str, current: &str) -> Option<ReleaseOverview> {
    let mut result = ReleaseOverview {
        title: format!("Jcode Desktop {}", current.trim().trim_start_matches('v')),
        sections: Vec::new(),
    };
    let version = release_version(current)?;
    let mut target = version.clone();
    let development = version.pre.as_str().split('.').next() == Some("dev");
    if development {
        target.pre = semver::Prerelease::EMPTY;
        result.title = format!("Jcode Desktop {target} · Development preview");
    }

    let mut matched = false;
    let mut label = Some("Highlights");
    let mut continuation = false;
    let mut awaiting_headline = true;
    let mut fenced = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some(heading) = line.strip_prefix("### Jcode Desktop ") {
            if matched {
                break;
            }
            matched = release_version(heading).as_ref() == Some(&target);
            continue;
        }
        if !matched {
            continue;
        }
        if line.starts_with("# ") || line.starts_with("## ") || line.starts_with("### ") {
            break;
        }
        // An optional introductory prose line is the editor's theme headline.
        // Read it only before sections/bullets so download footers cannot become
        // a release title. No heuristic summaries are generated from commits.
        if awaiting_headline && !trimmed.is_empty() {
            awaiting_headline = false;
            if !trimmed.starts_with(['#', '-', '*', '>']) {
                result.title.push_str(" · ");
                result.title.push_str(trimmed);
                continue;
            }
        }
        if let Some(heading) = line.strip_prefix("#### ") {
            label = match heading.trim().to_ascii_lowercase().as_str() {
                "themes" => Some("Themes"),
                "highlights" => Some("Highlights"),
                "improvements" => Some("Improvements"),
                "fixes" => Some("Fixes"),
                // Do not accidentally present installation or download steps
                // as release highlights when the document gains new sections.
                _ => None,
            };
            continuation = false;
            continue;
        }
        let Some(label) = label else { continue };
        if let Some(entry) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            let entry = entry.trim();
            continuation = !entry.is_empty();
            if !continuation {
                continue;
            }
            let index = result
                .sections
                .iter()
                .position(|section| section.label == label)
                .unwrap_or_else(|| {
                    result.sections.push(ReleaseSection {
                        label,
                        entries: Vec::new(),
                    });
                    result.sections.len() - 1
                });
            result.sections[index].entries.push(entry.to_owned());
        } else if continuation
            && (line.starts_with("  ") || line.starts_with('\t'))
            && !trimmed.is_empty()
        {
            if let Some(entry) = result
                .sections
                .iter_mut()
                .find(|section| section.label == label)
                .and_then(|section| section.entries.last_mut())
            {
                entry.push(' ');
                entry.push_str(trimmed);
            }
        } else {
            continuation = false;
        }
    }
    (!result.sections.is_empty()).then_some(result)
}

/// Pure, offline and Desktop-specific. Never substitutes CLI release notes or
/// the latest available release when this build has no matching editorial notes.
pub(crate) fn release_overview() -> Option<ReleaseOverview> {
    overview(fallback(), crate::build_info::VERSION)
}

pub(crate) fn fallback() -> &'static str {
    include_str!("../../../CHANGELOG.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_matches_only_the_requested_desktop_release() {
        let raw = "## What's new\n### Jcode Desktop 2.0.0\n- Future\n## Previous releases\n### Jcode Desktop 1.0.0\n- Current\nDownloads elsewhere.\n## Previous update\n- Unversioned";
        let notes = overview(raw, "v1.0.0+build.9").unwrap();
        assert_eq!(notes.title, "Jcode Desktop 1.0.0+build.9");
        assert_eq!(
            notes.sections,
            vec![ReleaseSection {
                label: "Highlights",
                entries: vec!["Current".into()],
            }]
        );
        for missing in ["0.86.0", "1.0.1", "1.0.0-beta.1", "invalid", ""] {
            assert_eq!(overview(raw, missing), None, "{missing}");
        }
        assert_eq!(overview("### Jcode 1.0.0\n- CLI only", "1.0.0"), None);
        assert_eq!(
            overview("### Jcode Desktop 1.0.0\nNo bullets", "1.0.0"),
            None
        );
    }

    #[test]
    fn editorial_headline_retains_version_and_ignores_footer_prose() {
        let raw = "### Jcode Desktop 1.0.0\n\nClearer navigation and richer feedback\n\n#### Highlights\n- Native panels\n\nDownloads elsewhere.";
        assert_eq!(
            overview(raw, "1.0.0").unwrap().title,
            "Jcode Desktop 1.0.0 · Clearer navigation and richer feedback"
        );
        assert_eq!(
            overview(raw, "1.0.0-dev.1").unwrap().title,
            "Jcode Desktop 1.0.0 · Development preview · Clearer navigation and richer feedback"
        );
        assert_eq!(
            overview("### Jcode Desktop 1.0.0\n- Notes\nDownloads", "1.0.0")
                .unwrap()
                .title,
            "Jcode Desktop 1.0.0"
        );
    }

    #[test]
    fn development_preview_matches_base_but_other_prereleases_are_exact() {
        let raw = "### Jcode Desktop 1.0.0\n- Stable\n### Jcode Desktop 1.0.0-beta.2\n- Beta";
        let preview = overview(raw, "1.0.0-dev.12+local").unwrap();
        assert_eq!(preview.title, "Jcode Desktop 1.0.0 · Development preview");
        assert_eq!(preview.sections[0].entries, ["Stable"]);
        let beta = overview(raw, "1.0.0-beta.2").unwrap();
        assert_eq!(beta.sections[0].entries, ["Beta"]);
        assert_eq!(overview(raw, "1.0.0-beta.1"), None);
        assert_eq!(overview(raw, "2.0.0-dev.1"), None);
    }

    #[test]
    fn overview_preserves_editorial_sections_and_wrapped_text() {
        let raw = "### Jcode Desktop 1.0.0\n#### Themes\n- Navigation\n  and clarity.\n\n#### Highlights\n* Native panels\n#### Improvements\n- Selection\n#### Fixes\n- Scrolling\n#### Downloads\n- Not a feature\n#### Fixes\n- Recovery\n```md\n### Jcode Desktop 9.0.0\n- Example only\n```\n## Previous releases\n- Never included";
        let notes = overview(raw, "1.0.0").unwrap();
        assert_eq!(
            notes.sections.iter().map(|s| s.label).collect::<Vec<_>>(),
            ["Themes", "Highlights", "Improvements", "Fixes"]
        );
        assert_eq!(notes.sections[0].entries, ["Navigation and clarity."]);
        assert_eq!(notes.sections[3].entries, ["Scrolling", "Recovery"]);
        assert!(
            !notes
                .sections
                .iter()
                .flat_map(|s| &s.entries)
                .any(|entry| entry.contains("feature")
                    || entry.contains("Example")
                    || entry.contains("Never"))
        );
    }

    #[test]
    fn bundled_editorial_notes_are_desktop_specific_and_version_bounded() {
        let current = overview(fallback(), "0.3.1").unwrap();
        assert_eq!(
            current.sections.iter().map(|s| s.label).collect::<Vec<_>>(),
            ["Themes", "Highlights", "Improvements", "Fixes"]
        );
        let text = current
            .sections
            .iter()
            .flat_map(|s| &s.entries)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Thinking Orbs"));
        assert!(text.contains("experimental Unix-only"));
        assert!(!text.contains("Nari credentials"));
        assert!(!text.contains("Downloads are available"));
        let previous = overview(fallback(), "0.2.1").unwrap();
        assert_eq!(previous.sections.len(), 1);
        assert_eq!(previous.sections[0].label, "Highlights");
        assert!(
            previous.sections[0]
                .entries
                .iter()
                .any(|entry| entry.contains("Nari credentials"))
        );
        assert!(
            !previous.sections[0]
                .entries
                .iter()
                .any(|entry| entry.contains("FPS indicator"))
        );
    }

    fn fixture() -> Vec<Entry> {
        parse(
            "new\x1eHEAD -> main\x1e2026-09-13 20:00 UTC\x1eNew: keep punctuation\x1f\nrelease\x1etag: desktop-v0.2.0, tag: unrelated\x1e2026-09-12 20:00 UTC\x1eRelease\x1f\nolder\x1e\x1e2026-09-11 20:00 UTC\x1eOlder change\x1f\nold-release\x1etag: desktop-v0.1.0\x1e2026-09-10 20:00 UTC\x1eFirst release\x1f",
        )
    }

    #[test]
    fn parses_release_tags_dates_and_subjects_without_colon_splitting() {
        let entries = fixture();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].version, None);
        assert_eq!(entries[0].subject, "New: keep punctuation");
        assert_eq!(entries[1].version.as_deref(), Some("v0.2.0"));
        assert_eq!(entries[1].date, "2026-09-12 20:00 UTC");
        assert!(parse("broken\x1f\x1e\x1e\x1eempty hash\x1f").is_empty());
        let multiple = parse(
            "h\x1etag: desktop-v1.0.0-beta.1, tag: desktop-v1.0.0, tag: v0.9.0\x1e\x1eSubject\x1f",
        );
        assert_eq!(multiple[0].version.as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn groups_newest_first_at_release_boundaries_like_tui() {
        let entries = fixture();
        let groups = group(&entries, "0.3.0");
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].version, "v0.3.0 · Unreleased");
        assert_eq!(groups[0].entries, ["New: keep punctuation"]);
        assert_eq!(groups[1].version, "v0.2.0");
        assert_eq!(groups[1].entries, ["Release", "Older change"]);
        assert_eq!(groups[1].date, entries[1].date);
        assert_eq!(group(&entries[1..], "0.2.0")[0].version, "v0.2.0");
        assert!(group(&[], "0.1.0").is_empty());
    }

    #[test]
    fn seen_hash_is_exclusive_and_missing_history_is_not_claimed_as_new() {
        let entries = fixture();
        assert_eq!(unseen(&entries, None), Seen::FirstVisit);
        assert_eq!(unseen(&entries, Some(" \n")), Seen::FirstVisit);
        assert_eq!(unseen(&entries, Some("new\n")), Seen::New(0));
        assert_eq!(unseen(&entries, Some("release")), Seen::New(1));
        assert_eq!(unseen(&entries, Some("missing")), Seen::OutsideHistory);
        assert_eq!(
            summarize(&entries, Seen::New(1)).entries,
            ["New: keep punctuation"]
        );
        assert_eq!(summarize(&entries, Seen::New(1)).heading, "1 new change");
        assert_eq!(
            summarize(&entries, Seen::New(0)).heading,
            "No new changes in this build"
        );
    }

    #[test]
    fn summary_is_bounded_and_reports_more_without_dropping_history() {
        let raw = (0..12)
            .map(|n| format!("{n}\x1e\x1eDate\x1eChange {n}\x1f"))
            .collect::<String>();
        let entries = parse(&raw);
        for seen in [Seen::FirstVisit, Seen::New(12), Seen::OutsideHistory] {
            let summary = summarize(&entries, seen);
            assert_eq!(summary.entries.len(), 5);
            assert_eq!(summary.remaining, 7);
        }
        assert_eq!(group(&entries, "1.0.0")[0].entries.len(), 12);
        assert!(summarize(&[], Seen::FirstVisit).entries.is_empty());
    }

    #[test]
    fn acknowledgment_persists_but_rendering_does_not_change_it() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("nested/seen");
        let entries = fixture();
        assert_eq!(acknowledge(&entries, &path), Seen::FirstVisit);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(acknowledge(&entries, &path), Seen::New(0));
        fs::write(&path, "release").unwrap();
        assert_eq!(acknowledge(&entries, &path), Seen::New(1));
        let _ = summarize(&entries, Seen::New(1));
        let _ = group(&entries, "0.3.0");
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(acknowledge(&[], &path), Seen::OutsideHistory);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(
            acknowledge(&entries, &path.join("unwritable")),
            Seen::FirstVisit
        );
    }
}
