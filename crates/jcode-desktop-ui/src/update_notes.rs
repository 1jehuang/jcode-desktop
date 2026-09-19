//! Offline update summaries and version-grouped history, following the TUI.
//! Rendering is pure. Only startup acknowledgement advances the seen commit.
use std::{fs, path::Path, sync::OnceLock};

const SUMMARY_LIMIT: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
    #[default]
    Latest,
    History,
    Build,
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

pub(crate) fn development() -> bool {
    crate::changelog::development()
}

pub(crate) fn build_details() -> String {
    format!(
        "Version `{}`\n\nCommit `{}`\n\nUpdate base `{}`\n\nBuild `{}`\n\n{}",
        crate::build_info::VERSION,
        crate::build_info::revision(),
        env!("JCODE_DESKTOP_VERSION"),
        env!("JCODE_DESKTOP_BUILD_ID"),
        include_str!(concat!(env!("OUT_DIR"), "/changelog-debug.md"))
    )
}

pub(crate) fn fallback() -> &'static str {
    include_str!("../../../CHANGELOG.md")
}

#[cfg(test)]
mod tests {
    use super::*;

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
