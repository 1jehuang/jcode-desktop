//! Keyword search over local session transcripts for the resume picker.
//!
//! Parsing every stored session would take seconds, so this scans the raw
//! snapshot and journal bytes instead. Matching is case-insensitive and every
//! query word must appear somewhere in the session, like the metadata search.
//! Bare JSON tokens such as `"text"` or `"content"` are ignored so structural
//! keys and enum values do not make every session match.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Session id to a short, human readable excerpt around the first hit.
pub(crate) type Hits = HashMap<String, String>;

const SNIPPET_RADIUS: usize = 70;

/// Lowercased words worth searching for. Single characters match nearly every
/// transcript, so a query made only of them does not trigger a content scan.
pub(crate) fn query_words(query: &str) -> Vec<String> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.iter().all(|word| word.chars().count() < 2) {
        return Vec::new();
    }
    words
}

pub(crate) fn sessions_dir() -> Option<PathBuf> {
    jcode_base::session::session_path("probe")
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// Scan every stored session under `dir`. Returns `None` when cancelled.
pub(crate) fn scan(dir: &Path, words: &[String], cancel: &AtomicBool) -> Option<Hits> {
    if words.is_empty() {
        return Some(Hits::new());
    }
    let mut snapshots: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let id = path
                .file_name()?
                .to_str()?
                .strip_suffix(".json")?
                .to_owned();
            crate::persisted_history::is_local_session_id(&id).then_some((id, path))
        })
        .collect();
    snapshots.sort();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk = snapshots.len().div_ceil(threads).max(1);
    let hits = std::thread::scope(|scope| {
        let workers: Vec<_> = snapshots
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut hits = Hits::new();
                    for (id, path) in chunk {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        if let Some(snippet) = search_session(path, words) {
                            hits.insert(id.clone(), snippet);
                        }
                    }
                    hits
                })
            })
            .collect();
        workers
            .into_iter()
            .filter_map(|worker| worker.join().ok())
            .fold(Hits::new(), |mut all, hits| {
                all.extend(hits);
                all
            })
    });
    (!cancel.load(Ordering::Relaxed)).then_some(hits)
}

fn search_session(snapshot: &Path, words: &[String]) -> Option<String> {
    let journal = jcode_base::session::session_journal_path_from_snapshot(snapshot);
    let files = [
        std::fs::read(snapshot).unwrap_or_default(),
        std::fs::read(journal).unwrap_or_default(),
    ];
    let mut snippet = None;
    for word in words {
        let hit = files
            .iter()
            .find_map(|bytes| find_word(bytes, word).map(|pos| (bytes, pos)))?;
        if snippet.is_none() {
            snippet = Some(excerpt(hit.0, hit.1, word.len()));
        }
    }
    snippet
}

/// First case-insensitive occurrence that is not a whole JSON string token.
fn find_word(haystack: &[u8], word: &str) -> Option<usize> {
    if !word.is_ascii() {
        // Rare: fall back to a lossy lowercase copy for non-ASCII queries.
        // Lowercasing can shift byte offsets, so only trust the position when
        // the lengths still line up.
        let text = String::from_utf8_lossy(haystack).to_lowercase();
        let pos = text.find(word)?;
        return Some(if text.len() == haystack.len() { pos } else { 0 });
    }
    let needle = word.as_bytes();
    let (&first, rest) = needle.split_first()?;
    let upper = first.to_ascii_uppercase();
    let mut offset = 0;
    while offset + needle.len() <= haystack.len() {
        let pos = offset + memchr::memchr2(first, upper, &haystack[offset..])?;
        let end = pos + needle.len();
        if end > haystack.len() {
            return None;
        }
        if haystack[pos + 1..end].eq_ignore_ascii_case(rest)
            && !is_json_structure(haystack, pos, end)
        {
            return Some(pos);
        }
        offset = pos + 1;
    }
    None
}

/// Whether `haystack[pos..end]` is a whole JSON key, or the whole value of a
/// structural field such as `"type"` or `"role"`, rather than conversation text.
fn is_json_structure(haystack: &[u8], pos: usize, end: usize) -> bool {
    if pos == 0 || haystack[pos - 1] != b'"' || haystack.get(end) != Some(&b'"') {
        return false;
    }
    if haystack.get(end + 1) == Some(&b':') {
        return true;
    }
    let before = &haystack[..pos - 1];
    [
        &b"\"type\":"[..],
        b"\"role\":",
        b"\"status\":",
        b"\"kind\":",
        b"\"provider_key\":",
        b"\"model\":",
    ]
    .iter()
    .any(|key| before.ends_with(key))
}

/// Decode the JSON-escaped neighbourhood of a hit into one readable line.
fn excerpt(bytes: &[u8], pos: usize, len: usize) -> String {
    let start = pos.saturating_sub(SNIPPET_RADIUS);
    let end = (pos + len + SNIPPET_RADIUS).min(bytes.len());
    let raw = String::from_utf8_lossy(&bytes[start..end]);
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('n' | 't' | 'r') => text.push(' '),
                Some('u') => {
                    let code: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&code, 16).ok().and_then(char::from_u32) {
                        text.push(ch);
                    }
                }
                Some(other) => text.push(other),
                None => {}
            },
            '\u{fffd}' => {}
            ch => text.push(ch),
        }
    }
    // Tool inputs are JSON inside JSON, so one decode pass can leave escapes.
    let text = text
        .replace("\\n", " ")
        .replace("\\t", " ")
        .replace("\\\"", "\"");
    let mut words: Vec<&str> = text.split_whitespace().collect();
    // Drop partial words cut by the byte window.
    if start > 0 && words.len() > 1 {
        words.remove(0);
    }
    if end < bytes.len() && words.len() > 1 {
        words.pop();
    }
    let joined = words.join(" ");
    format!(
        "{}{joined}{}",
        if start > 0 { "…" } else { "" },
        if end < bytes.len() { "…" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_words_in_snapshot_and_journal_but_not_json_tokens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session_a.json"),
            r#"{"messages":[{"role":"user","content":[{"type":"text","text":"Fix the Kubernetes\nrollout"}]}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("session_a.journal.jsonl"),
            r#"{"append":{"text":"deploy to staging"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("session_b.json"),
            r#"{"messages":[{"role":"user","content":[{"type":"text","text":"unrelated"}]}]}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("session_a.bak"), "kubernetes").unwrap();
        let cancel = AtomicBool::new(false);
        let scan = |query: &str| {
            let mut ids: Vec<_> = scan(dir.path(), &query_words(query), &cancel)
                .unwrap()
                .into_keys()
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(scan("KUBERNETES staging"), vec!["session_a"]);
        assert!(scan("text").is_empty(), "bare JSON tokens must not match");
        assert!(scan("kubernetes missing").is_empty());
        assert_eq!(scan("unrelated"), vec!["session_b"]);
        let hits = super::scan(dir.path(), &query_words("kubernetes"), &cancel).unwrap();
        assert!(
            hits["session_a"].contains("Fix the Kubernetes rollout"),
            "{hits:?}"
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(super::scan(dir.path(), &query_words("unrelated"), &cancel).is_none());
    }

    #[test]
    fn single_character_queries_do_not_scan() {
        assert!(query_words("a b").is_empty());
        assert_eq!(query_words("a Rust"), vec!["a", "rust"]);
    }
}
