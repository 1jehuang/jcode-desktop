//! Structured, presentation-independent previews of proposed file changes.
//!
//! Tool edits contain snippets, not complete files. Their numbers are relative to
//! each snippet, whereas unified patch numbers are taken from the hunk headers.
//! No filesystem access is performed and a `write` is never assumed to be a create.

use std::collections::HashMap;
use std::ops::Range;

use serde_json::Value;

#[path = "diff_tool_result.rs"]
mod tool_result;

pub(crate) fn from_tool_result(name: &str, input: &str, output: &str) -> Option<DiffPreview> {
    tool_result::from_result(name, input, output)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffPreview {
    pub(crate) files: Vec<DiffFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffFile {
    pub(crate) path: String,
    pub(crate) previous_path: Option<String>,
    pub(crate) kind: String,
    pub(crate) hunks: Vec<DiffHunk>,
    pub(crate) note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffHunk {
    pub(crate) header: String,
    pub(crate) lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffLine {
    pub(crate) kind: LineKind,
    /// Line content without the diff prefix or line terminator.
    pub(crate) text: String,
    pub(crate) old_line: Option<usize>,
    pub(crate) new_line: Option<usize>,
    /// UTF-8 byte offsets into `text`, always on character boundaries.
    pub(crate) emphasis: Option<Range<usize>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineKind {
    Context,
    Added,
    Removed,
    Meta,
}

impl DiffPreview {
    pub(crate) fn counts(&self) -> (usize, usize) {
        self.files.iter().fold((0, 0), |(a, r), file| {
            let (added, removed) = file.counts();
            (a + added, r + removed)
        })
    }
}

impl DiffFile {
    pub(crate) fn counts(&self) -> (usize, usize) {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .fold((0, 0), |(added, removed), line| match line.kind {
                LineKind::Added => (added + 1, removed),
                LineKind::Removed => (added, removed + 1),
                _ => (added, removed),
            })
    }

    /// Copyable preview, including file identity, notes and hunk boundaries.
    /// Snippet previews are deliberately not presented as applicable patches.
    pub(crate) fn plain_text(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        if let Some(previous) = &self.previous_path {
            let _ = writeln!(out, "{}: {} -> {}", self.kind, previous, self.path);
        } else {
            let _ = writeln!(out, "{}: {}", self.kind, self.path);
        }
        if let Some(note) = &self.note {
            let _ = writeln!(out, "{note}");
        }
        for hunk in &self.hunks {
            let _ = writeln!(out, "{}", hunk.header);
            for line in &hunk.lines {
                let prefix = match line.kind {
                    LineKind::Context => " ",
                    LineKind::Added => "+",
                    LineKind::Removed => "-",
                    LineKind::Meta => "",
                };
                let _ = writeln!(out, "{prefix}{}", line.text);
            }
        }
        out
    }
}

fn file(path: &str, kind: &str) -> DiffFile {
    DiffFile {
        path: path.to_owned(),
        previous_path: None,
        kind: kind.to_owned(),
        hunks: Vec::new(),
        note: None,
    }
}

fn note(file: &mut DiffFile, text: &str) {
    match &mut file.note {
        Some(existing) if !existing.contains(text) => {
            existing.push(' ');
            existing.push_str(text);
        }
        None => file.note = Some(text.to_owned()),
        _ => {}
    }
}

fn line(kind: LineKind, text: &str, old: Option<usize>, new: Option<usize>) -> DiffLine {
    DiffLine {
        kind,
        text: text.to_owned(),
        old_line: old,
        new_line: new,
        emphasis: None,
    }
}

const SNIPPET_NOTE: &str = "Line numbers are snippet-relative, not file positions.";
const NO_NEWLINE: &str = "\\ No newline at end of file";
const NO_SNIPPET_NEWLINE: &str = "\\ No newline at end of snippet";

/// Recognize desktop file-editing tools, with or without `functions.` prefix.
/// Invalid JSON or missing required arguments does not produce an invented diff.
/// Raw patch arguments may be streaming, so unterminated metadata is buffered.
pub(crate) fn from_tool(name: &str, input: &str) -> Option<DiffPreview> {
    parse_tool(name, input, 0)
}

fn parse_tool(name: &str, input: &str, depth: usize) -> Option<DiffPreview> {
    if depth >= 16 {
        return None;
    }
    match serde_json::from_str::<Value>(input) {
        Ok(value) => tool_value(name, &value, depth),
        Err(_)
            if matches!(
                name.trim()
                    .strip_prefix("functions.")
                    .unwrap_or(name.trim()),
                "apply_patch" | "patch"
            ) =>
        {
            raw_patch(input)
        }
        Err(_) => None,
    }
}

fn tool_value(name: &str, value: &Value, depth: usize) -> Option<DiffPreview> {
    if depth >= 16 {
        return None;
    }
    let name = name
        .trim()
        .strip_prefix("functions.")
        .unwrap_or(name.trim());
    if matches!(name, "batch" | "multi_tool_use.parallel") {
        let calls = value
            .get("tool_calls")
            .or_else(|| value.get("tool_uses"))?
            .as_array()?;
        let mut files = Vec::new();
        for call in calls {
            let Some(name) = call
                .get("tool")
                .or_else(|| call.get("recipient_name"))
                .or_else(|| call.get("name"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let arguments = call
                .get("parameters")
                .or_else(|| call.get("arguments"))
                .or_else(|| call.get("input"))
                .unwrap_or(call);
            let preview = if let Some(input) = arguments.as_str() {
                parse_tool(name, input, depth + 1)
            } else {
                tool_value(name, arguments, depth + 1)
            };
            if let Some(preview) = preview {
                files.extend(preview.files);
            }
        }
        return (!files.is_empty()).then_some(DiffPreview { files });
    }
    if !matches!(
        name,
        "edit" | "write" | "multiedit" | "apply_patch" | "patch"
    ) {
        return None;
    }
    if matches!(name, "apply_patch" | "patch") {
        let patch = value.as_str().or_else(|| {
            ["patch_text", "patch", "input"]
                .iter()
                .find_map(|key| value.get(*key).and_then(Value::as_str))
        })?;
        return from_patch(patch);
    }
    let path = value
        .get("file_path")
        .or_else(|| value.get("path"))?
        .as_str()?;
    if path.trim().is_empty() {
        return None;
    }
    let mut result = file(path, if name == "write" { "Write" } else { "Edit" });
    match name {
        "write" => {
            let content = value.get("content")?.as_str()?;
            note(
                &mut result,
                "Proposed write content. Previous file content is unknown.",
            );
            let mut lines = Vec::new();
            for (i, source) in source_lines(content).iter().enumerate() {
                lines.push(line(LineKind::Added, source.text, None, Some(i + 1)));
                if !source.terminated {
                    lines.push(line(LineKind::Meta, NO_NEWLINE, None, None));
                }
            }
            result.hunks.push(DiffHunk {
                header: "@@ Write content (previous content unknown) @@".to_owned(),
                lines,
            });
            if content.is_empty() {
                note(&mut result, "Empty content.");
            }
        }
        "edit" if value.get("edits").is_none() => {
            note(&mut result, SNIPPET_NOTE);
            result.hunks = snippet_diff(
                value.get("old_string")?.as_str()?,
                value.get("new_string")?.as_str()?,
            );
            if value.get("replace_all").and_then(Value::as_bool) == Some(true) {
                note(
                    &mut result,
                    "Replace all matches. Counts show one supplied snippet only.",
                );
            }
        }
        "multiedit" | "edit" => {
            note(&mut result, SNIPPET_NOTE);
            let edits = value.get("edits")?.as_array()?;
            for (i, edit) in edits.iter().enumerate() {
                let mut hunks = snippet_diff(
                    edit.get("old_string")?.as_str()?,
                    edit.get("new_string")?.as_str()?,
                );
                for hunk in &mut hunks {
                    hunk.header = format!("Edit {}: {}", i + 1, hunk.header);
                }
                result.hunks.extend(hunks);
                if edit.get("replace_all").and_then(Value::as_bool) == Some(true) {
                    note(
                        &mut result,
                        "Replace all matches. Counts show supplied snippets only.",
                    );
                }
            }
            note(
                &mut result,
                "Edits are sequential snippets, not a combined file diff.",
            );
        }
        _ => return None,
    }
    if result.hunks.is_empty() {
        note(&mut result, "No changes in the supplied snippets.");
    }
    Some(DiffPreview {
        files: vec![result],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SourceLine<'a> {
    text: &'a str,
    terminated: bool,
}

fn source_lines(text: &str) -> Vec<SourceLine<'_>> {
    text.split_inclusive('\n')
        .map(|text| SourceLine {
            text: text.strip_suffix('\n').unwrap_or(text),
            terminated: text.ends_with('\n'),
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Op {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// A bounded LCS handles small gaps, with patience anchors for large edits.
/// Total dynamic-programming work per snippet is capped. Large anchorless gaps
/// use bounded lookahead instead of allocating a quadratic matrix.
fn diff_ops<'a>(old: &[SourceLine<'a>], new: &[SourceLine<'a>]) -> Vec<Op> {
    // Comparing long, nearly identical strings in every LCS cell would defeat
    // the work limit. Intern exact lines once, then compare constant-time IDs.
    let mut ids = HashMap::new();
    let mut intern = |source: &SourceLine<'a>| {
        let next = ids.len();
        *ids.entry(*source).or_insert(next)
    };
    let old_ids: Vec<usize> = old.iter().map(&mut intern).collect();
    let new_ids: Vec<usize> = new.iter().map(&mut intern).collect();
    diff_line_ids(&old_ids, &new_ids)
}

fn diff_line_ids(old: &[usize], new: &[usize]) -> Vec<Op> {
    const MAX_CELLS: usize = 300_000;
    let mut budget = MAX_CELLS;
    let mut ops = Vec::with_capacity(old.len().saturating_add(new.len()));
    let mut prefix = 0;
    while prefix < old.len().min(new.len()) && old[prefix] == new[prefix] {
        ops.push(Op::Equal(prefix, prefix));
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len().min(new.len()) - prefix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    if (old_end - prefix + 1)
        .checked_mul(new_end - prefix + 1)
        .is_some_and(|cells| cells <= budget)
    {
        diff_gap(
            old,
            new,
            prefix..old_end,
            prefix..new_end,
            &mut budget,
            &mut ops,
        );
        for offset in 0..suffix {
            ops.push(Op::Equal(old_end + offset, new_end + offset));
        }
        return ops;
    }
    let mut unique: HashMap<usize, (usize, usize, usize, usize)> = HashMap::new();
    for (i, &text) in old.iter().enumerate().take(old_end).skip(prefix) {
        let entry = unique.entry(text).or_insert((0, 0, i, 0));
        entry.0 += 1;
    }
    for (i, &text) in new.iter().enumerate().take(new_end).skip(prefix) {
        let entry = unique.entry(text).or_insert((0, 0, 0, i));
        entry.1 += 1;
        entry.3 = i;
    }
    let pairs: Vec<(usize, usize)> = old
        .iter()
        .enumerate()
        .take(old_end)
        .skip(prefix)
        .filter_map(|(i, text)| {
            let &(a, b, _, j) = unique.get(text)?;
            (a == 1 && b == 1).then_some((i, j))
        })
        .collect();
    // Longest increasing subsequence of the unique lines' new positions.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![None; pairs.len()];
    for (i, &(_, j)) in pairs.iter().enumerate() {
        let at = tails.partition_point(|&index| pairs[index].1 < j);
        previous[i] = at.checked_sub(1).map(|at| tails[at]);
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }
    let mut anchors = Vec::new();
    let mut cursor = tails.last().copied();
    while let Some(i) = cursor {
        anchors.push(pairs[i]);
        cursor = previous[i];
    }
    anchors.reverse();
    let (mut a, mut b) = (prefix, prefix);
    for (i, j) in anchors {
        diff_gap(old, new, a..i, b..j, &mut budget, &mut ops);
        ops.push(Op::Equal(i, j));
        a = i + 1;
        b = j + 1;
    }
    diff_gap(old, new, a..old_end, b..new_end, &mut budget, &mut ops);
    for offset in 0..suffix {
        ops.push(Op::Equal(old_end + offset, new_end + offset));
    }
    ops
}

fn diff_gap(
    old: &[usize],
    new: &[usize],
    mut a: Range<usize>,
    mut b: Range<usize>,
    budget: &mut usize,
    ops: &mut Vec<Op>,
) {
    while !a.is_empty() && !b.is_empty() && old[a.start] == new[b.start] {
        ops.push(Op::Equal(a.start, b.start));
        a.start += 1;
        b.start += 1;
    }
    let mut suffix = 0;
    while suffix < a.len().min(b.len()) && old[a.end - 1 - suffix] == new[b.end - 1 - suffix] {
        suffix += 1;
    }
    a.end -= suffix;
    b.end -= suffix;
    let cells = (a.len() + 1).checked_mul(b.len() + 1);
    if !a.is_empty() && !b.is_empty() && cells.is_some_and(|n| n <= *budget) {
        let cells = cells.unwrap();
        *budget -= cells;
        let width = b.len() + 1;
        let mut lcs = vec![0u32; cells];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                lcs[i * width + j] = if old[a.start + i] == new[b.start + j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && old[a.start + i] == new[b.start + j] {
                ops.push(Op::Equal(a.start + i, b.start + j));
                i += 1;
                j += 1;
            } else if i < a.len()
                && (j == b.len() || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
            {
                ops.push(Op::Delete(a.start + i));
                i += 1;
            } else {
                ops.push(Op::Insert(b.start + j));
                j += 1;
            }
        }
    } else {
        fallback_diff(old, new, a.clone(), b.clone(), ops);
    }
    for offset in 0..suffix {
        ops.push(Op::Equal(a.end + offset, b.end + offset));
    }
}

/// Preserve obvious common runs even when every line is repeated and patience
/// anchors are unavailable. The fixed lookahead makes the fallback linear.
fn fallback_diff(
    old: &[usize],
    new: &[usize],
    a: Range<usize>,
    b: Range<usize>,
    ops: &mut Vec<Op>,
) {
    const LOOKAHEAD: usize = 32;
    let (mut i, mut j) = (a.start, b.start);
    while i < a.end && j < b.end {
        if old[i] == new[j] {
            ops.push(Op::Equal(i, j));
            i += 1;
            j += 1;
            continue;
        }
        let removed = (i + 1..a.end.min(i.saturating_add(LOOKAHEAD)))
            .find(|&at| old[at] == new[j])
            .map(|at| at - i);
        let added = (j + 1..b.end.min(j.saturating_add(LOOKAHEAD)))
            .find(|&at| new[at] == old[i])
            .map(|at| at - j);
        match (removed, added) {
            (Some(count), other) if other.is_none_or(|n| count <= n) => {
                ops.extend((i..i + count).map(Op::Delete));
                i += count;
            }
            (_, Some(count)) => {
                ops.extend((j..j + count).map(Op::Insert));
                j += count;
            }
            _ => {
                ops.push(Op::Delete(i));
                ops.push(Op::Insert(j));
                i += 1;
                j += 1;
            }
        }
    }
    ops.extend((i..a.end).map(Op::Delete));
    ops.extend((j..b.end).map(Op::Insert));
}

fn snippet_diff(old_text: &str, new_text: &str) -> Vec<DiffHunk> {
    const CONTEXT: usize = 3;
    let old = source_lines(old_text);
    let new = source_lines(new_text);
    let ops = diff_ops(&old, &new);
    let mut spans: Vec<Range<usize>> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if !matches!(op, Op::Equal(..)) {
            let span = i.saturating_sub(CONTEXT)..(i + CONTEXT + 1).min(ops.len());
            if let Some(previous) = spans.last_mut().filter(|p| p.end >= span.start) {
                previous.end = span.end;
            } else {
                spans.push(span);
            }
        }
    }
    // Record cursors once. Avoid rescanning the prefix for each separated hunk.
    let mut positions = Vec::with_capacity(ops.len() + 1);
    let (mut a, mut b) = (0, 0);
    positions.push((a, b));
    for op in &ops {
        match op {
            Op::Equal(..) => {
                a += 1;
                b += 1;
            }
            Op::Delete(_) => a += 1,
            Op::Insert(_) => b += 1,
        }
        positions.push((a, b));
    }
    spans.into_iter().map(|span| {
        let (old_before, new_before) = positions[span.start];
        let (old_after, new_after) = positions[span.end];
        let old_count = old_after - old_before;
        let new_count = new_after - new_before;
        let old_start = old_before + usize::from(old_count != 0);
        let new_start = new_before + usize::from(new_count != 0);
        let mut lines = Vec::new();
        for op in &ops[span] {
            let (kind, source, old_line, new_line) = match *op {
                Op::Equal(a, b) => (LineKind::Context, old[a], Some(a + 1), Some(b + 1)),
                Op::Delete(a) => (LineKind::Removed, old[a], Some(a + 1), None),
                Op::Insert(b) => (LineKind::Added, new[b], None, Some(b + 1)),
            };
            lines.push(line(kind, source.text, old_line, new_line));
            if !source.terminated {
                lines.push(line(LineKind::Meta, NO_SNIPPET_NEWLINE, None, None));
            }
        }
        emphasize(&mut lines);
        DiffHunk {
            header: format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@ (snippet-relative lines)"),
            lines,
        }
    }).collect()
}

/// Pair adjacent removed/added blocks without repeatedly scanning entire hunks.
fn emphasize(lines: &mut [DiffLine]) {
    let mut start = 0;
    while start < lines.len() {
        if lines[start].kind == LineKind::Context {
            start += 1;
            continue;
        }
        let mut end = start;
        let (mut removed, mut added) = (Vec::new(), Vec::new());
        while end < lines.len() && lines[end].kind != LineKind::Context {
            match lines[end].kind {
                LineKind::Removed => removed.push(end),
                LineKind::Added => added.push(end),
                _ => {}
            }
            end += 1;
        }
        for (a, b) in replacement_pairs(lines, &removed, &added) {
            let (old, new) = changed_ranges(&lines[a].text, &lines[b].text);
            lines[a].emphasis = old;
            lines[b].emphasis = new;
        }
        start = end;
    }
}

/// Return absolute `(removed_index, added_index)` pairs for one changed block.
/// The supplied indices should follow source order. Pairing never crosses that
/// order, and equal-sized runs retain their stable positional alignment.
/// Unequal runs may skip surplus lines on the longer side, for example a newly
/// inserted comment before a replacement. Work is bounded by fixed lookahead
/// and capped prefix/suffix comparisons, rather than a cross-product matrix.
pub(crate) fn replacement_pairs(
    lines: &[DiffLine],
    removed: &[usize],
    added: &[usize],
) -> Vec<(usize, usize)> {
    const LOOKAHEAD: usize = 16;
    // Trim indentation once per line, not once per candidate comparison.
    let indexed = |indices: &[usize], kind| {
        indices
            .iter()
            .filter_map(|&index| {
                let line = lines.get(index).filter(|line| line.kind == kind)?;
                Some((index, line.text.trim_start()))
            })
            .collect::<Vec<_>>()
    };
    let removed = indexed(removed, LineKind::Removed);
    let added = indexed(added, LineKind::Added);
    if removed.len() == added.len() {
        return removed
            .iter()
            .zip(&added)
            .map(|(a, b)| (a.0, b.0))
            .collect();
    }
    let removed_is_shorter = removed.len() < added.len();
    let (shorter, longer) = if removed_is_shorter {
        (&removed, &added)
    } else {
        (&added, &removed)
    };
    let mut result = Vec::with_capacity(shorter.len());
    let mut cursor = 0;
    for (at, &(short_index, short_text)) in shorter.iter().enumerate() {
        // Leave at least one candidate for every remaining shorter-side line.
        let surplus = longer.len() - cursor - (shorter.len() - at);
        let last = cursor + surplus.min(LOOKAHEAD);
        let mut best = cursor;
        let mut best_score = replacement_similarity(short_text, longer[cursor].1);
        for candidate in cursor + 1..=last {
            let score = replacement_similarity(short_text, longer[candidate].1);
            // Nearest candidate wins ties, including wholly unrelated lines.
            if score > best_score {
                best = candidate;
                best_score = score;
            }
        }
        let long_index = longer[best].0;
        result.push(if removed_is_shorter {
            (short_index, long_index)
        } else {
            (long_index, short_index)
        });
        cursor = best + 1;
    }
    result
}

fn replacement_similarity(old: &str, new: &str) -> usize {
    const EDGE_CHARS: usize = 128;
    let mut prefix_bytes = 0;
    let mut prefix_chars = 0;
    for (a, b) in old.chars().zip(new.chars()).take(EDGE_CHARS) {
        if a != b {
            break;
        }
        prefix_bytes += a.len_utf8();
        prefix_chars += 1;
    }
    // Slice on matched character boundaries so prefix and suffix cannot count
    // the same character twice. Prefix gets extra weight over common closing
    // punctuation, which alone is weak evidence of an actual replacement.
    let suffix_chars = old[prefix_bytes..]
        .chars()
        .rev()
        .zip(new[prefix_bytes..].chars().rev())
        .take(EDGE_CHARS)
        .take_while(|(a, b)| a == b)
        .count();
    prefix_chars * 2 + suffix_chars
}

fn changed_ranges(old: &str, new: &str) -> (Option<Range<usize>>, Option<Range<usize>>) {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    (
        (prefix < old_end).then_some(prefix..old_end),
        (prefix < new_end).then_some(prefix..new_end),
    )
}

/// Parse Codex patches and ordinary unified diffs, preserving all files/hunks.
/// Incomplete or malformed hunks retain useful content with an explicit note.
/// Unlike raw input to `from_tool`, this argument is considered complete, so a
/// final metadata line does not need a trailing newline.
pub(crate) fn from_patch(patch: &str) -> Option<DiffPreview> {
    parse_patch(patch, false)
}

fn raw_patch(patch: &str) -> Option<DiffPreview> {
    let patch = patch.trim_start();
    // Incomplete JSON must not be reinterpreted as freeform patch content.
    if !(patch.starts_with("*** ") || patch.starts_with("diff --git ") || patch.starts_with("--- "))
    {
        return None;
    }
    parse_patch(patch, !patch.ends_with('\n'))
}

fn parse_patch(patch: &str, partial_last_line: bool) -> Option<DiffPreview> {
    if patch.lines().any(|l| {
        matches!(l.trim_end_matches('\r'), "*** Begin Patch")
            || l.starts_with("*** Update File: ")
            || l.starts_with("*** Add File: ")
            || l.starts_with("*** Delete File: ")
    }) {
        codex_patch(patch, partial_last_line)
    } else {
        unified_patch(patch, partial_last_line)
    }
}

fn finish_file(current: &mut Option<DiffFile>, files: &mut Vec<DiffFile>) {
    if let Some(mut file) = current.take() {
        for hunk in &mut file.hunks {
            emphasize(&mut hunk.lines);
        }
        files.push(file);
    }
}

fn codex_patch(patch: &str, partial_last_line: bool) -> Option<DiffPreview> {
    let mut files = Vec::new();
    let mut current = None;
    let (mut old, mut new) = (1usize, 1usize);
    let lines: Vec<_> = patch.lines().collect();
    for (index, raw) in lines.iter().enumerate() {
        let raw = raw.trim_end_matches('\r');
        if partial_last_line
            && index + 1 == lines.len()
            && (raw.starts_with("***") || raw.starts_with("@@"))
        {
            break;
        }
        let declaration = [
            ("*** Update File: ", "Edit"),
            ("*** Add File: ", "Add"),
            ("*** Delete File: ", "Delete"),
        ]
        .iter()
        .find_map(|(prefix, kind)| raw.strip_prefix(prefix).map(|p| (p, *kind)));
        if let Some((path, kind)) = declaration {
            finish_file(&mut current, &mut files);
            if path.trim().is_empty() {
                continue;
            }
            let mut next = file(path, kind);
            if kind == "Edit" {
                note(&mut next, SNIPPET_NOTE);
            } else if kind == "Delete" {
                note(
                    &mut next,
                    "File deletion. Previous content was not supplied.",
                );
            }
            current = Some(next);
            old = 1;
            new = 1;
            continue;
        }
        if raw == "*** End Patch" {
            finish_file(&mut current, &mut files);
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if let Some(path) = raw.strip_prefix("*** Move to: ") {
            if !path.trim().is_empty() {
                file.previous_path = Some(std::mem::replace(&mut file.path, path.to_owned()));
                file.kind = "Rename".to_owned();
            } else {
                note(file, "Malformed move destination.");
            }
        } else if raw.starts_with("@@") {
            old = 1;
            new = 1;
            file.hunks.push(DiffHunk {
                header: format!(
                    "{raw} (snippet-relative lines, hunk {})",
                    file.hunks.len() + 1
                ),
                lines: Vec::new(),
            });
        } else if raw == "*** End of File" || raw == NO_NEWLINE {
            if let Some(hunk) = file.hunks.last_mut() {
                hunk.lines.push(line(LineKind::Meta, raw, None, None));
            }
        } else {
            let parsed = match raw.as_bytes().first() {
                Some(b'+') => Some((LineKind::Added, &raw[1..])),
                Some(b'-') => Some((LineKind::Removed, &raw[1..])),
                Some(b' ') => Some((LineKind::Context, &raw[1..])),
                None if !file.hunks.is_empty() && file.kind != "Add" => {
                    Some((LineKind::Context, ""))
                }
                _ => None,
            };
            if let Some((kind, text)) = parsed {
                if file.hunks.is_empty() {
                    file.hunks.push(DiffHunk {
                        header: if file.kind == "Add" {
                            "@@ New file, line 1 @@".to_owned()
                        } else {
                            "@@ (snippet-relative lines, hunk 1) @@".to_owned()
                        },
                        lines: Vec::new(),
                    });
                }
                let a = matches!(kind, LineKind::Context | LineKind::Removed).then_some(old);
                let b = matches!(kind, LineKind::Context | LineKind::Added).then_some(new);
                old = old.saturating_add(usize::from(a.is_some()));
                new = new.saturating_add(usize::from(b.is_some()));
                file.hunks
                    .last_mut()
                    .unwrap()
                    .lines
                    .push(line(kind, text, a, b));
            } else if !raw.is_empty() {
                note(file, "Unrecognized patch content was omitted.");
            }
        }
    }
    finish_file(&mut current, &mut files);
    (!files.is_empty()).then_some(DiffPreview { files })
}

#[derive(Clone, Copy)]
struct HunkPosition {
    old: usize,
    new: usize,
    old_left: usize,
    new_left: usize,
}

fn parse_range(text: &str, sign: char) -> Option<(usize, usize)> {
    let text = text.strip_prefix(sign)?;
    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
    if !start.bytes().all(|b| b.is_ascii_digit()) || !count.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (start, count) = (start.parse::<usize>().ok()?, count.parse::<usize>().ok()?);
    if (start == 0 && count != 0) || start.checked_add(count).is_none() {
        return None;
    }
    Some((start, count))
}

fn hunk_position(header: &str) -> Option<HunkPosition> {
    let mut parts = header.strip_prefix("@@ ")?.split_whitespace();
    let (old, old_left) = parse_range(parts.next()?, '-')?;
    let (new, new_left) = parse_range(parts.next()?, '+')?;
    if parts.next()? != "@@" {
        return None;
    }
    Some(HunkPosition {
        old,
        new,
        old_left,
        new_left,
    })
}

fn incomplete_hunk(file: &mut DiffFile, position: &mut Option<HunkPosition>) {
    if position
        .take()
        .is_some_and(|p| p.old_left != 0 || p.new_left != 0)
    {
        note(file, "Incomplete or malformed unified hunk.");
    }
}

fn unified_patch(patch: &str, partial_last_line: bool) -> Option<DiffPreview> {
    let lines: Vec<&str> = patch.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;
    let mut position: Option<HunkPosition> = None;
    let mut saw_headers = false;
    let mut invalid_hunk = false;
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        // Inside a declared hunk, --- and +++ may be literal changed lines.
        if let (Some(file), Some(p)) = (current.as_mut(), position.as_mut()) {
            if raw == NO_NEWLINE {
                if let Some(hunk) = file.hunks.last_mut() {
                    hunk.lines.push(line(LineKind::Meta, raw, None, None));
                }
                i += 1;
                continue;
            }
            let kind = match raw.as_bytes().first() {
                Some(b' ') if p.old_left > 0 && p.new_left > 0 => Some(LineKind::Context),
                Some(b'-') if p.old_left > 0 => Some(LineKind::Removed),
                Some(b'+') if p.new_left > 0 => Some(LineKind::Added),
                None if p.old_left > 0 && p.new_left > 0 => Some(LineKind::Context),
                _ => None,
            };
            if let Some(kind) = kind {
                let a = matches!(kind, LineKind::Context | LineKind::Removed).then_some(p.old);
                let b = matches!(kind, LineKind::Context | LineKind::Added).then_some(p.new);
                if a.is_some() {
                    p.old += 1;
                    p.old_left -= 1;
                }
                if b.is_some() {
                    p.new += 1;
                    p.new_left -= 1;
                }
                file.hunks.last_mut().unwrap().lines.push(line(
                    kind,
                    raw.get(1..).unwrap_or(""),
                    a,
                    b,
                ));
                i += 1;
                continue;
            }
            invalid_hunk |= p.old_left != 0 || p.new_left != 0;
            incomplete_hunk(file, &mut position);
        }
        // Body text streams immediately. Metadata waits for its line ending.
        if partial_last_line && i + 1 == lines.len() {
            break;
        }
        if invalid_hunk && !raw.starts_with("diff --git ") {
            if let Some(file) = current.as_mut() {
                unpositioned_line(file, raw);
            }
            i += 1;
            continue;
        }
        if let Some(paths) = raw.strip_prefix("diff --git ") {
            invalid_hunk = false;
            if let Some(file) = current.as_mut() {
                incomplete_hunk(file, &mut position);
            }
            finish_file(&mut current, &mut files);
            if let Some((a, b)) = git_paths(paths) {
                let mut next = file(&b, "Edit");
                if a != b {
                    next.previous_path = Some(a);
                    next.kind = "Rename".to_owned();
                }
                current = Some(next);
            }
            saw_headers = false;
        } else if let Some(a) = raw
            .strip_prefix("--- ")
            .filter(|_| lines.get(i + 1).is_some_and(|l| l.starts_with("+++ ")))
        {
            if partial_last_line && i + 2 == lines.len() {
                break;
            }
            if saw_headers {
                finish_file(&mut current, &mut files);
            }
            let mut a = header_path(a);
            let mut b = header_path(&lines[i + 1][4..]);
            // Strip only a conventional paired Git prefix, not a real `a/`
            // directory appearing identically on both sides of a plain diff.
            if (a.starts_with("a/") && b.starts_with("b/"))
                || (a == "/dev/null" && b.starts_with("b/"))
                || (b == "/dev/null" && a.starts_with("a/"))
            {
                a = a.strip_prefix("a/").unwrap_or(&a).to_owned();
                b = b.strip_prefix("b/").unwrap_or(&b).to_owned();
            }
            if !a.is_empty() && !b.is_empty() && !(a == "/dev/null" && b == "/dev/null") {
                let path = if b == "/dev/null" { &a } else { &b };
                let next = current.get_or_insert_with(|| file(path, "Edit"));
                next.path = path.to_owned();
                if a == "/dev/null" {
                    next.kind = "Add".to_owned();
                    next.previous_path = None;
                } else if b == "/dev/null" {
                    next.kind = "Delete".to_owned();
                    next.previous_path = None;
                } else if a != b {
                    next.kind = "Rename".to_owned();
                    next.previous_path = Some(a);
                }
                saw_headers = true;
            }
            i += 1;
        } else if let Some(file) = current.as_mut() {
            if raw.starts_with("@@") {
                position = hunk_position(raw);
                if position.is_none() {
                    invalid_hunk = true;
                    note(
                        file,
                        "Malformed unified hunk header. Line positions are unavailable.",
                    );
                }
                file.hunks.push(DiffHunk {
                    header: raw.to_owned(),
                    lines: Vec::new(),
                });
            } else if let Some(path) = raw.strip_prefix("rename from ") {
                file.previous_path = Some(unquote_path(path));
                file.kind = "Rename".to_owned();
            } else if let Some(path) = raw.strip_prefix("rename to ") {
                file.path = unquote_path(path);
                file.kind = "Rename".to_owned();
            } else if raw.starts_with("new file mode ") {
                file.kind = "Add".to_owned();
            } else if raw.starts_with("deleted file mode ") {
                file.kind = "Delete".to_owned();
            } else if raw.starts_with("Binary files ") || raw == "GIT binary patch" {
                note(file, "Binary file change. Text preview unavailable.");
            } else if raw.starts_with("old mode ") || raw.starts_with("new mode ") {
                note(file, raw);
            } else if raw.starts_with(['+', '-', ' ']) && !file.hunks.is_empty() {
                // Keep malformed/truncated preview content, but never invent positions.
                note(file, "Incomplete or malformed unified hunk.");
                invalid_hunk = true;
                unpositioned_line(file, raw);
            } else if raw == NO_NEWLINE {
                if let Some(hunk) = file.hunks.last_mut() {
                    hunk.lines.push(line(LineKind::Meta, raw, None, None));
                }
            } else if !raw.is_empty() && !file.hunks.is_empty() {
                invalid_hunk = true;
                note(file, "Incomplete or malformed unified hunk.");
            }
        }
        i += 1;
    }
    if let Some(file) = current.as_mut() {
        incomplete_hunk(file, &mut position);
    }
    finish_file(&mut current, &mut files);
    (!files.is_empty()).then_some(DiffPreview { files })
}

fn unpositioned_line(file: &mut DiffFile, raw: &str) {
    let Some(hunk) = file.hunks.last_mut() else {
        return;
    };
    let kind = match raw.as_bytes().first() {
        Some(b'+') => LineKind::Added,
        Some(b'-') => LineKind::Removed,
        Some(b' ') => LineKind::Context,
        _ => return,
    };
    hunk.lines.push(line(kind, &raw[1..], None, None));
}

fn header_path(text: &str) -> String {
    // Unified timestamps are separated by a tab. Spaces belong to the path.
    if text.starts_with('"') {
        quoted_token(text)
            .map(|(p, _)| p)
            .unwrap_or_else(|| text.to_owned())
    } else {
        text.split('\t').next().unwrap_or(text).to_owned()
    }
}

fn unquote_path(text: &str) -> String {
    quoted_token(text)
        .map(|(path, _)| path)
        .unwrap_or_else(|| text.to_owned())
}

/// Git's quoted paths use C escapes and octal UTF-8 bytes.
fn quoted_token(text: &str) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((String::from_utf8_lossy(&out).into_owned(), i + 1)),
            b'\\' => {
                i += 1;
                let byte = *bytes.get(i)?;
                if (b'0'..=b'7').contains(&byte) {
                    let mut value = 0u16;
                    let mut digits = 0;
                    while digits < 3 && i < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
                        value = value * 8 + (bytes[i] - b'0') as u16;
                        i += 1;
                        digits += 1;
                    }
                    out.push(u8::try_from(value).ok()?);
                    continue;
                }
                out.push(match byte {
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'r' => b'\r',
                    b'b' => 8,
                    b'f' => 12,
                    b'v' => 11,
                    b'a' => 7,
                    other => other,
                });
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    None
}

fn git_paths(text: &str) -> Option<(String, String)> {
    // Equal unquoted paths have a unique midpoint even when the filename
    // itself contains ` b/`. Checking that split once avoids quadratic scans.
    if !text.starts_with('"') {
        let middle = text.len().saturating_sub(1) / 2;
        if let (Some(a), Some(b)) = (text.get(..middle), text.get(middle..)) {
            if let (Some(a), Some(b)) = (a.strip_prefix("a/"), b.strip_prefix(" b/")) {
                if !a.is_empty() && a == b {
                    return Some((a.to_owned(), b.to_owned()));
                }
            }
        }
    }
    let (a, rest) = if text.starts_with('"') {
        let (a, end) = quoted_token(text)?;
        (a, text.get(end..)?.trim_start())
    } else if let Some(at) = text.find(" \"b/").or_else(|| text.find(" b/")) {
        (text[..at].to_owned(), &text[at + 1..])
    } else {
        let (a, b) = text.split_once(' ')?;
        (a.to_owned(), b)
    };
    let b = unquote_path(rest);
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some((
        a.strip_prefix("a/").unwrap_or(&a).to_owned(),
        b.strip_prefix("b/").unwrap_or(&b).to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(old: &str, new: &str) -> DiffFile {
        from_tool(
            "edit",
            &json!({"file_path":"src/test.rs", "old_string":old, "new_string":new}).to_string(),
        )
        .unwrap()
        .files
        .remove(0)
    }

    fn all_lines(file: &DiffFile) -> Vec<&DiffLine> {
        file.hunks.iter().flat_map(|h| &h.lines).collect()
    }

    #[test]
    fn edit_keeps_common_lines_as_context() {
        let file = edit("start\nold\ncommon\nend\n", "start\nnew\ncommon\nend\n");
        assert_eq!(file.counts(), (1, 1));
        assert_eq!(file.hunks.len(), 1);
        assert!(file.note.as_deref().unwrap().contains("snippet-relative"));
        assert_eq!(
            file.hunks[0].header,
            "@@ -1,4 +1,4 @@ (snippet-relative lines)"
        );
        let lines = all_lines(&file);
        assert_eq!(
            lines.iter().filter(|l| l.kind == LineKind::Context).count(),
            3
        );
        assert_eq!((lines[1].old_line, lines[1].new_line), (Some(2), None));
        assert_eq!((lines[2].old_line, lines[2].new_line), (None, Some(2)));
    }

    #[test]
    fn edit_separates_distant_changes_and_clips_context() {
        let old = (1..=40).map(|n| format!("line {n}\n")).collect::<String>();
        let new = old
            .replace("line 5\n", "changed five\n")
            .replace("line 35\n", "changed thirty five\n");
        let file = edit(&old, &new);
        assert_eq!(file.counts(), (2, 2));
        assert_eq!(file.hunks.len(), 2);
        assert!(file.hunks[0].header.starts_with("@@ -2,7 +2,7 @@"));
        assert!(file.hunks[1].header.starts_with("@@ -32,7 +32,7 @@"));
        assert!(!file.plain_text().contains("line 20\n"));
    }

    #[test]
    fn edit_merges_overlapping_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let file = edit(old, &old.replace("b\n", "B\n").replace("g\n", "G\n"));
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.counts(), (2, 2));
        assert_eq!(all_lines(&file).len(), 10);
    }

    #[test]
    fn edit_insert_delete_and_empty_snippets() {
        let inserted = edit("", "\nhello\n");
        assert_eq!(inserted.counts(), (2, 0));
        assert_eq!(
            inserted.hunks[0].header,
            "@@ -0,0 +1,2 @@ (snippet-relative lines)"
        );
        assert_eq!(inserted.hunks[0].lines[0].text, "");
        let deleted = edit("hello\n\n", "");
        assert_eq!(deleted.counts(), (0, 2));
        assert!(deleted.hunks[0].header.contains("+0,0"));
        for text in ["", "a", "a\n", "\n\n"] {
            let same = edit(text, text);
            assert!(same.hunks.is_empty());
            assert_eq!(same.counts(), (0, 0));
            assert!(same.note.as_deref().unwrap().contains("No changes"));
        }
    }

    #[test]
    fn newline_only_edit_is_visible_and_markers_do_not_count() {
        let file = edit("same", "same\n");
        assert_eq!(file.counts(), (1, 1));
        let lines = all_lines(&file);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].kind, LineKind::Meta);
        assert_eq!(lines[1].text, NO_SNIPPET_NEWLINE);
        assert_eq!((lines[1].old_line, lines[1].new_line), (None, None));
        assert!(lines.iter().all(|l| l.emphasis.is_none()));
        let file = edit("old", "new");
        assert_eq!(
            all_lines(&file)
                .iter()
                .filter(|l| l.kind == LineKind::Meta)
                .count(),
            2
        );
    }

    #[test]
    fn multiedit_has_distinct_relative_hunks_and_replace_all_note() {
        let preview = from_tool(
            "functions.multiedit",
            &json!({"file_path":"a", "edits":[
                {"old_string":"old\n", "new_string":"new\n"},
                {"old_string":"x\n", "new_string":"y\nz\n", "replace_all":true}
            ]})
            .to_string(),
        )
        .unwrap();
        let file = &preview.files[0];
        assert_eq!(file.hunks.len(), 2);
        assert_eq!(preview.counts(), (3, 2));
        assert!(file.hunks[0].header.starts_with("Edit 1: "));
        assert!(file.hunks[1].header.starts_with("Edit 2: "));
        assert_eq!(file.hunks[1].lines[0].old_line, Some(1));
        assert!(file.note.as_deref().unwrap().contains("sequential"));
        assert!(file.note.as_deref().unwrap().contains("Replace all"));
    }

    #[test]
    fn edit_with_edits_array_matches_multiedit_preview() {
        let input = json!({"file_path":"a", "edits":[
            {"old_string":"old\n", "new_string":"new\n"},
            {"old_string":"x\n", "new_string":"y\n"}
        ]})
        .to_string();
        let merged = from_tool("edit", &input).unwrap();
        let legacy = from_tool("multiedit", &input).unwrap();
        assert_eq!(merged.counts(), legacy.counts());
        assert_eq!(merged.files[0].hunks.len(), 2);
        assert!(merged.files[0].hunks[1].header.starts_with("Edit 2: "));
    }

    #[test]
    fn edit_replace_all_does_not_claim_total_file_counts() {
        let preview = from_tool(
            "edit",
            r#"{"file_path":"a","old_string":"x","new_string":"y","replace_all":true}"#,
        )
        .unwrap();
        assert!(
            preview.files[0]
                .note
                .as_deref()
                .unwrap()
                .contains("one supplied snippet")
        );
    }

    #[test]
    fn write_is_not_mislabeled_as_create() {
        for content in ["", "\n", "hello\n\n", "hello"] {
            let preview = from_tool(
                "functions.write",
                &json!({"file_path":"existing.txt", "content":content}).to_string(),
            )
            .unwrap();
            let file = &preview.files[0];
            assert_eq!(file.kind, "Write");
            assert!(
                file.note
                    .as_deref()
                    .unwrap()
                    .contains("Previous file content is unknown")
            );
            assert_eq!(preview.counts(), (source_lines(content).len(), 0));
            assert!(all_lines(file).iter().all(|l| l.old_line.is_none()));
            assert!(!file.plain_text().contains("New file"));
        }
    }

    #[test]
    fn malformed_tool_inputs_are_rejected() {
        for (name, input) in [
            ("edit", "not json"),
            ("edit", "null"),
            ("edit", "{}"),
            (
                "edit",
                r#"{"file_path":"a","old_string":3,"new_string":"x"}"#,
            ),
            ("write", r#"{"file_path":" ","content":"x"}"#),
            ("multiedit", r#"{"file_path":"a","edits":[{}]}"#),
            ("patch", "{}"),
            ("functions.read", "{}"),
            (
                "other.edit",
                r#"{"file_path":"a","old_string":"x","new_string":"y"}"#,
            ),
        ] {
            assert!(from_tool(name, input).is_none(), "{name}: {input}");
        }
        assert!(from_patch("").is_none());
        assert!(from_patch("plain text\n@@ -1 +1 @@\n-a\n+b").is_none());
        assert!(from_patch("*** Begin Patch\n*** End Patch").is_none());
    }

    #[test]
    fn batch_collects_supported_files_and_skips_unrelated_calls() {
        let input = json!({"tool_calls":[
            {"tool":"functions.edit", "file_path":"a", "old_string":"x\n", "new_string":"y\n"},
            {"tool":"read", "file_path":"ignored"},
            {"tool":"write", "file_path":"b", "content":"hello\n"},
            {"tool":"edit", "file_path":"malformed"},
            {"tool":"patch", "patch_text":"*** Delete File: c"}
        ]});
        let preview = from_tool("functions.batch", &input.to_string()).unwrap();
        assert_eq!(
            preview
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(preview.counts(), (2, 1));
        assert!(from_tool("batch", r#"{"tool_calls":[{"tool":"read"}]}"#).is_none());
    }

    #[test]
    fn parallel_recurses_through_nested_batches_and_string_arguments() {
        let input = json!({"tool_uses":[
            {"recipient_name":"functions.batch", "parameters":{"tool_calls":[
                {"tool":"functions.edit", "arguments":{"file_path":"a", "old_string":"x", "new_string":"y"}},
                {"tool":"functions.write", "input":json!({"file_path":"b", "content":"written\n"}).to_string()}
            ]}},
            {"recipient_name":"functions.apply_patch", "parameters":{"patch_text":"*** Add File: c\n+new\n"}},
            {"name":"edit", "arguments":{"file_path":"d", "old_string":"x", "new_string":"z"}}
        ]});
        let preview = from_tool("multi_tool_use.parallel", &input.to_string()).unwrap();
        assert_eq!(preview.files.len(), 4);
        assert_eq!(preview.counts(), (4, 2));
    }

    #[test]
    fn recursive_batches_are_bounded_without_discarding_shallow_siblings() {
        let mut nested = json!({"tool":"write", "file_path":"deep", "content":"x"});
        for _ in 0..20 {
            nested = json!({"tool":"batch", "tool_calls":[nested]});
        }
        let input =
            json!({"tool_calls":[nested, {"tool":"write", "file_path":"shallow", "content":"y"}]});
        let preview = from_tool("batch", &input.to_string()).unwrap();
        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.files[0].path, "shallow");
    }

    #[test]
    fn patch_tools_accept_wrapped_and_raw_formats() {
        let patch = "*** Begin Patch\n*** Add File: x\n+hi\n*** End Patch";
        for name in [
            "apply_patch",
            "patch",
            "functions.apply_patch",
            "functions.patch",
        ] {
            for input in [
                patch.to_owned(),
                json!(patch).to_string(),
                json!({"patch_text":patch}).to_string(),
                json!({"patch":patch}).to_string(),
                json!({"input":patch}).to_string(),
            ] {
                assert_eq!(from_tool(name, &input).unwrap().counts(), (1, 0));
            }
        }
    }

    #[test]
    fn codex_multiple_files_add_delete_and_move() {
        let patch = "*** Begin Patch\n*** Add File: new file.txt\n+first\n+\n+third\n*** Delete File: gone.txt\n*** Update File: before.rs\n*** Move to: after.rs\n@@ fn first()\n context\n-old\n+new\n@@ fn second()\n-x\n+y\n*** End of File\n*** End Patch";
        let preview = from_patch(patch).unwrap();
        assert_eq!(preview.files.len(), 3);
        assert_eq!(preview.counts(), (5, 2));
        let add = &preview.files[0];
        assert_eq!((&*add.path, &*add.kind), ("new file.txt", "Add"));
        assert_eq!(add.hunks[0].lines[1].text, "");
        assert_eq!(add.hunks[0].lines[2].new_line, Some(3));
        let delete = &preview.files[1];
        assert_eq!(delete.kind, "Delete");
        assert_eq!(delete.counts(), (0, 0));
        assert!(delete.note.as_deref().unwrap().contains("not supplied"));
        let moved = &preview.files[2];
        assert_eq!((&*moved.path, &*moved.kind), ("after.rs", "Rename"));
        assert_eq!(moved.previous_path.as_deref(), Some("before.rs"));
        assert_eq!(moved.hunks.len(), 2);
        assert_eq!(moved.hunks[1].lines[0].old_line, Some(1));
        assert!(
            moved
                .hunks
                .iter()
                .all(|h| h.header.contains("snippet-relative"))
        );
        assert_eq!(moved.hunks[1].lines[2].kind, LineKind::Meta);
    }

    #[test]
    fn codex_no_header_update_still_has_context_and_relative_label() {
        let preview = from_patch("*** Update File: x\n before\n-old\n+new\n after\n").unwrap();
        let file = &preview.files[0];
        assert_eq!(file.counts(), (1, 1));
        assert!(file.hunks[0].header.contains("snippet-relative"));
        assert_eq!(file.hunks[0].lines[3].old_line, Some(3));
        assert_eq!(file.hunks[0].lines[3].new_line, Some(3));
    }

    #[test]
    fn codex_malformed_empty_path_and_move_are_safe() {
        let preview = from_patch("*** Begin Patch\n*** Add File: \n+ignored\n*** Update File: x\n*** Move to: \nunrecognized\n@@\n-old\n+new\n").unwrap();
        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.files[0].path, "x");
        assert_eq!(preview.files[0].counts(), (1, 1));
        assert!(
            preview.files[0]
                .note
                .as_deref()
                .unwrap()
                .contains("Malformed")
        );
    }

    #[test]
    fn unified_actual_line_numbers_and_multiple_hunks() {
        let preview = from_patch("diff --git a/src/a.rs b/src/a.rs\nindex aaa..bbb 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -40,3 +50,4 @@ fn demo()\n first\n-old\n+new\n+extra\n last\n@@ -100 +111 @@\n-a\n+b\n").unwrap();
        let file = &preview.files[0];
        assert_eq!(file.path, "src/a.rs");
        assert_eq!(file.counts(), (3, 2));
        assert_eq!(file.note, None);
        assert_eq!(file.hunks[0].header, "@@ -40,3 +50,4 @@ fn demo()");
        let first = &file.hunks[0].lines;
        assert_eq!((first[0].old_line, first[0].new_line), (Some(40), Some(50)));
        assert_eq!((first[4].old_line, first[4].new_line), (Some(42), Some(53)));
        assert_eq!(file.hunks[1].lines[0].old_line, Some(100));
        assert_eq!(file.hunks[1].lines[1].new_line, Some(111));
    }

    #[test]
    fn unified_create_delete_rename_without_git_headers() {
        let preview = from_patch("--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+new\n+\n--- a/deleted.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-old\n-\n--- a/old.txt\n+++ b/renamed.txt\n@@ -5 +8 @@\n-a\n+b\n").unwrap();
        assert_eq!(preview.files.len(), 3);
        assert_eq!(preview.counts(), (3, 3));
        assert_eq!(
            (&*preview.files[0].path, &*preview.files[0].kind),
            ("new.txt", "Add")
        );
        assert_eq!(
            (&*preview.files[1].path, &*preview.files[1].kind),
            ("deleted.txt", "Delete")
        );
        assert_eq!(preview.files[2].previous_path.as_deref(), Some("old.txt"));
        assert_eq!(preview.files[2].kind, "Rename");
        assert!(preview.files.iter().all(|f| f.note.is_none()));
    }

    #[test]
    fn unified_rename_only_empty_create_delete_and_modes() {
        let preview = from_patch("diff --git a/old name b/new name\nsimilarity index 100%\nrename from old name\nrename to new name\ndiff --git a/empty b/empty\nnew file mode 100644\nindex 0000000..e69de29\ndiff --git a/gone b/gone\ndeleted file mode 100644\ndiff --git a/executable b/executable\nold mode 100644\nnew mode 100755\n").unwrap();
        assert_eq!(preview.files.len(), 4);
        assert_eq!(preview.counts(), (0, 0));
        assert_eq!(preview.files[0].path, "new name");
        assert_eq!(preview.files[0].previous_path.as_deref(), Some("old name"));
        assert_eq!(preview.files[1].kind, "Add");
        assert_eq!(preview.files[2].kind, "Delete");
        assert!(preview.files[3].note.as_deref().unwrap().contains("100755"));
    }

    #[test]
    fn unified_blank_context_and_no_newline_markers() {
        let file = from_patch("--- a/x\n+++ b/x\n@@ -7,2 +9,2 @@\n \n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n").unwrap().files.remove(0);
        assert_eq!(file.counts(), (1, 1));
        assert_eq!(file.hunks[0].lines[0].text, "");
        assert_eq!(file.hunks[0].lines[0].kind, LineKind::Context);
        assert_eq!(file.hunks[0].lines[3].new_line, Some(10));
        assert_eq!(file.note, None);
        assert_eq!(file.hunks[0].lines[4].kind, LineKind::Meta);
    }

    #[test]
    fn unified_header_like_content_is_not_a_file_boundary() {
        let preview =
            from_patch("--- a/x\n+++ b/x\n@@ -1 +1 @@\n--- a/not-a-file\n+++ b/not-a-file\n")
                .unwrap();
        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.files[0].path, "x");
        assert_eq!(preview.counts(), (1, 1));
        assert_eq!(preview.files[0].hunks[0].lines[0].text, "-- a/not-a-file");
    }

    #[test]
    fn unified_quoted_paths_unicode_octal_and_timestamps() {
        let preview = from_patch("diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\n--- \"a/caf\\303\\251.txt\"\t2025-01-01\n+++ \"b/caf\\303\\251.txt\"\t2025-01-02\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert_eq!(preview.files[0].path, "café.txt");
        let preview = from_patch("--- a/file with spaces\told timestamp\n+++ b/file with spaces\tnew timestamp\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert_eq!(preview.files[0].path, "file with spaces");
        assert_eq!(
            quoted_token("\"a\\t\\\"b\" rest"),
            Some(("a\t\"b".to_owned(), 8))
        );
    }

    #[test]
    fn unified_binary_content_is_not_made_into_fake_lines() {
        let preview = from_patch("diff --git a/a.png b/a.png\nindex abc..def 100644\nBinary files a/a.png and b/a.png differ\ndiff --git a/b.png b/b.png\nGIT binary patch\nliteral 4\nAcme\n").unwrap();
        assert_eq!(preview.files.len(), 2);
        assert_eq!(preview.counts(), (0, 0));
        assert!(
            preview
                .files
                .iter()
                .all(|f| f.note.as_deref().unwrap().contains("Binary"))
        );
    }

    #[test]
    fn malformed_unified_header_has_no_invented_positions() {
        for header in [
            "@@ nonsense @@",
            "@@ -0 +1 @@",
            "@@ -1,2 +1,no @@",
            "@@ -18446744073709551615,2 +1 @@",
            "@@ -1 +1 missing",
        ] {
            let preview = from_patch(&format!("--- a/x\n+++ b/x\n{header}\n-old\n+new\n")).unwrap();
            let file = &preview.files[0];
            assert_eq!(file.counts(), (1, 1));
            assert!(file.note.as_deref().unwrap().contains("Malformed"));
            assert!(
                all_lines(file)
                    .iter()
                    .all(|l| l.old_line.is_none() && l.new_line.is_none())
            );
        }
    }

    #[test]
    fn truncated_unified_hunk_is_explicit_and_next_file_survives() {
        let preview = from_patch("--- a/x\n+++ b/x\n@@ -1,10 +1,10 @@\n-old\n+new\ndiff --git a/y b/y\n--- a/y\n+++ b/y\n@@ -10 +20 @@\n-a\n+b\n").unwrap();
        assert_eq!(preview.files.len(), 2);
        assert!(
            preview.files[0]
                .note
                .as_deref()
                .unwrap()
                .contains("Incomplete")
        );
        assert_eq!(preview.files[1].hunks[0].lines[0].old_line, Some(10));
        assert_eq!(preview.files[1].note, None);
    }

    #[test]
    fn intraline_ranges_are_unicode_safe_and_precise() {
        for (a, b, expected_a, expected_b) in [
            ("let café = '猫';", "let café = '犬';", "猫", "犬"),
            ("a🦀z", "a🐍z", "🦀", "🐍"),
            ("prefix old suffix", "prefix new suffix", "old", "new"),
            ("e\u{301}", "é", "e\u{301}", "é"),
        ] {
            let (ar, br) = changed_ranges(a, b);
            assert_eq!(&a[ar.unwrap()], expected_a);
            assert_eq!(&b[br.unwrap()], expected_b);
            let file = edit(a, b);
            for line in all_lines(&file) {
                if let Some(range) = &line.emphasis {
                    assert!(line.text.is_char_boundary(range.start));
                    assert!(line.text.is_char_boundary(range.end));
                    assert!(range.start < range.end);
                }
            }
        }
        assert_eq!(changed_ranges("abc", "abXc"), (None, Some(2..3)));
        assert_eq!(changed_ranges("same", "same"), (None, None));
        assert_eq!(changed_ranges("", "🦀"), (None, Some(0..4)));
    }

    #[test]
    fn intraline_pairing_never_crosses_context_or_hunks() {
        let file = from_patch(
            "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n context\n+new\n@@ -10 +10 @@\n-abc\n+aXc\n",
        )
        .unwrap()
        .files
        .remove(0);
        assert!(file.hunks[0].lines.iter().all(|l| l.emphasis.is_none()));
        assert_eq!(file.hunks[1].lines[0].emphasis, Some(1..2));
        assert_eq!(file.hunks[1].lines[1].emphasis, Some(1..2));
    }

    #[test]
    fn plain_text_keeps_identity_notes_hunks_markers_and_blank_lines() {
        let file = from_patch("*** Update File: old\n*** Move to: new\n@@ first\n-\n+hello\n\\ No newline at end of file\n@@ second\n-old\n+new\n*** End Patch").unwrap().files.remove(0);
        let text = file.plain_text();
        assert!(text.starts_with("Rename: old -> new\n"));
        assert!(text.contains("snippet-relative"));
        assert!(text.contains("\n-\n+hello\n\\ No newline at end of file\n"));
        assert!(text.contains("@@ second"));
    }

    #[test]
    fn crlf_patch_protocol_is_supported() {
        let preview = from_patch("--- a/x\r\n+++ b/x\r\n@@ -1 +1 @@\r\n-old\r\n+new\r\n").unwrap();
        assert_eq!(preview.counts(), (1, 1));
        assert_eq!(preview.files[0].hunks[0].lines[0].text, "old");
    }

    #[test]
    fn unprefixed_blank_context_is_preserved() {
        for patch in [
            "*** Update File: x\n@@\n\n-old\n+new\n",
            "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n\n-old\n+new\n",
        ] {
            let preview = from_patch(patch).unwrap();
            let lines = &preview.files[0].hunks[0].lines;
            assert_eq!(lines[0].kind, LineKind::Context);
            assert_eq!(lines[0].text, "");
            assert_eq!(lines[1].old_line, Some(2));
            assert_eq!(lines[2].new_line, Some(2));
            assert_eq!(preview.counts(), (1, 1));
        }
    }

    #[test]
    fn large_sparse_edits_keep_common_interior_and_small_previews() {
        let old = (0..30_000)
            .map(|n| format!("unique line {n}\n"))
            .collect::<String>();
        let new = old
            .replace("unique line 100\n", "changed 100\n")
            .replace("unique line 29000\n", "changed 29000\n");
        let file = edit(&old, &new);
        assert_eq!(file.counts(), (2, 2));
        assert_eq!(file.hunks.len(), 2);
        assert!(all_lines(&file).len() < 20);
    }

    #[test]
    fn large_anchorless_replacement_has_bounded_fallback() {
        let file = edit(&"old\n".repeat(20_000), &"new\n".repeat(20_000));
        assert_eq!(file.counts(), (20_000, 20_000));
        assert_eq!(file.hunks.len(), 1);
    }

    #[test]
    fn large_repeated_interior_is_context_not_a_whole_file_replacement() {
        let middle = "repeated\n".repeat(25_000);
        let old = format!("old start\n{middle}old end\n");
        let new = format!("new start\n{middle}new end\n");
        let file = edit(&old, &new);
        assert_eq!(file.counts(), (2, 2));
        assert_eq!(file.hunks.len(), 2);
        assert!(all_lines(&file).len() < 20);
    }

    #[test]
    fn bounded_fallback_preserves_input_order_on_repeated_shifted_lines() {
        let old = (0..1200).map(|i| i % 7).collect::<Vec<_>>();
        let mut new = old.clone();
        new.splice(100..103, [999, 998, 997, 996]);
        new.splice(800..809, [990]);
        let ops = diff_line_ids(&old, &new);
        let (mut a, mut b) = (0, 0);
        for op in ops {
            match op {
                Op::Equal(i, j) => {
                    assert_eq!((i, j), (a, b));
                    assert_eq!(old[i], new[j]);
                    a += 1;
                    b += 1;
                }
                Op::Delete(i) => {
                    assert_eq!(i, a);
                    a += 1;
                }
                Op::Insert(j) => {
                    assert_eq!(j, b);
                    b += 1;
                }
            }
        }
        assert_eq!((a, b), (old.len(), new.len()));
    }

    #[test]
    fn ordinary_unified_paths_named_a_or_b_are_not_git_prefixes() {
        for path in ["a/foo", "b/foo", "normal/path"] {
            let preview = from_patch(&format!(
                "--- {path}\n+++ {path}\n@@ -1 +1 @@\n-old\n+new\n"
            ))
            .unwrap();
            assert_eq!(preview.files[0].path, path);
            assert_eq!(preview.files[0].kind, "Edit");
            assert_eq!(preview.files[0].previous_path, None);
        }
    }

    fn sequences() -> Vec<String> {
        let mut sequences = vec![String::new()];
        for len in 1..=5 {
            for bits in 0..(1 << len) {
                let s = (0..len)
                    .map(|bit| if bits & (1 << bit) == 0 { "a\n" } else { "b\n" })
                    .collect::<String>();
                sequences.push(s.clone());
                sequences.push(s.trim_end_matches('\n').to_owned());
            }
        }
        sequences
    }

    #[test]
    fn exhaustive_small_diffs_reconstruct_both_inputs_and_are_minimal() {
        let seqs = sequences();
        for a in &seqs {
            for b in &seqs {
                let old = source_lines(a);
                let new = source_lines(b);
                let ops = diff_ops(&old, &new);
                let (mut ai, mut bi) = (0, 0);
                let mut equal_count = 0;
                for op in ops {
                    match op {
                        Op::Equal(i, j) => {
                            assert_eq!((i, j), (ai, bi));
                            assert_eq!(old[i], new[j]);
                            ai += 1;
                            bi += 1;
                            equal_count += 1;
                        }
                        Op::Delete(i) => {
                            assert_eq!(i, ai);
                            ai += 1;
                        }
                        Op::Insert(j) => {
                            assert_eq!(j, bi);
                            bi += 1;
                        }
                    }
                }
                assert_eq!((ai, bi), (old.len(), new.len()));
                // Independent brute-force common-subsequence oracle.
                let longest = (0usize..1 << old.len())
                    .filter_map(|mask| {
                        let subseq = old
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| mask & (1 << i) != 0)
                            .map(|(_, line)| line)
                            .collect::<Vec<_>>();
                        let mut j = 0;
                        for line in &new {
                            if subseq.get(j) == Some(&line) {
                                j += 1;
                            }
                        }
                        (j == subseq.len()).then_some(j)
                    })
                    .max()
                    .unwrap();
                assert_eq!(equal_count, longest, "{a:?} -> {b:?}");
            }
        }
    }

    #[test]
    fn arbitrary_malformed_text_never_panics() {
        let alphabet = [
            "@@",
            "--- ",
            "+++ ",
            "*** Add File: ",
            "*** Update File: x",
            "diff --git a/x b/x",
            "@@ -1 +1 @@",
            "+",
            "-",
            " ",
            "\\ No newline at end of file",
            "🐍",
            "",
            "\0",
        ];
        let mut state = 13u64;
        for _ in 0..1500 {
            let mut input = String::new();
            for _ in 0..20 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                input.push_str(alphabet[(state as usize) % alphabet.len()]);
                input.push('\n');
            }
            if let Some(preview) = from_patch(&input) {
                let _ = preview.counts();
                for file in &preview.files {
                    let _ = file.plain_text();
                    for line in all_lines(file) {
                        if let Some(range) = &line.emphasis {
                            assert!(line.text.get(range.clone()).is_some());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn malformed_hunk_counts_never_promote_body_lines_to_file_headers() {
        for body in [
            "@@ -0,0 +1,2 @@\n--- a/fake\n+++ b/fake\n",
            "@@ -1,2 +0,0 @@\n+++ b/not-a-header\n--- a/fake\n+++ b/fake\n",
            "@@ -1,x +1,2 @@\n--- a/fake\n+++ b/fake\n",
            "@@ -1,2 +1,2\n--- a/fake\n+++ b/fake\n",
            "@@ -1,2 +1,2 @@\n malformed context\n?invalid\n--- a/fake\n+++ b/fake\n",
            "@@ -1 +1 @@\n-old\n+new\n+surplus\n--- a/fake\n+++ b/fake\n",
            "@@ -1 +1 @@\n-old\n+new\n--- unpaired body\n?invalid\n--- a/fake\n+++ b/fake\n",
        ] {
            let patch = format!("--- a/real\n+++ b/real\n{body}");
            let preview = from_tool("patch", &patch).unwrap();
            assert_eq!(preview.files.len(), 1, "{body}");
            assert_eq!(preview.files[0].path, "real", "{body}");
            assert!(preview.files[0].note.is_some());
        }
    }

    #[test]
    fn explicit_git_boundary_recovers_after_malformed_hunk() {
        let patch = "diff --git a/real b/real\n--- a/real\n+++ b/real\n@@ -0,0 +1,2 @@\n--- a/fake\n+++ b/fake\ndiff --git a/next b/next\n--- a/next\n+++ b/next\n@@ -11 +22 @@\n-a\n+b\n";
        let preview = from_tool("patch", patch).unwrap();
        assert_eq!(
            preview
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["real", "next"]
        );
        assert_eq!(preview.files[1].hunks[0].lines[0].old_line, Some(11));
        assert_eq!(preview.files[1].hunks[0].lines[1].new_line, Some(22));
        assert_eq!(preview.files[1].note, None);
    }

    #[test]
    fn every_raw_patch_prefix_avoids_partial_or_content_filenames() {
        let patches = [
            (
                "functions.apply_patch",
                "*** Begin Patch\n*** Update File: 旧🦀.rs\n*** Move to: 新🦀.rs\n@@\n--- a/fake\n+++ b/fake\n*** Add File: added.txt\n+new\n*** Delete File: deleted.txt\n*** End Patch",
            ),
            (
                "patch",
                "diff --git a/旧🦀.rs b/新🦀.rs\nrename from 旧🦀.rs\nrename to 新🦀.rs\n--- a/旧🦀.rs\n+++ b/新🦀.rs\n@@ -1 +1 @@\n--- a/fake\n+++ b/fake\n--- /dev/null\n+++ b/added.txt\n@@ -0,0 +1 @@\n+new\n--- a/deleted.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n",
            ),
        ];
        let names = ["旧🦀.rs", "新🦀.rs", "added.txt", "deleted.txt"];
        for (name, patch) in patches {
            for end in patch
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(patch.len()))
            {
                if let Some(preview) = from_tool(name, &patch[..end]) {
                    for file in preview.files {
                        assert!(
                            names.contains(&file.path.as_str()),
                            "{name} prefix {end}: {file:?}"
                        );
                        if let Some(previous) = file.previous_path {
                            assert!(
                                names.contains(&previous.as_str()),
                                "{name} prefix {end}: {previous}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn raw_headers_wait_for_newline_but_complete_payloads_do_not() {
        for input in [
            "*** Begin Patch\n*** Add File: par",
            "*** Begin Patch\n*** Update File: par",
            "*** Begin Patch\n*** Delete File: par",
            "diff --git a/partial b/par",
            "--- a/partial\n+++ b/par",
        ] {
            assert!(from_tool("apply_patch", input).is_none(), "{input}");
        }
        let patch = "*** Delete File: complete-name";
        assert_eq!(from_patch(patch).unwrap().files[0].path, "complete-name");
        assert_eq!(
            from_tool("patch", &json!({"patch_text":patch}).to_string())
                .unwrap()
                .files[0]
                .path,
            "complete-name"
        );
        let patch = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n--- old\n+++ new";
        let preview = from_tool("patch", patch).unwrap();
        assert_eq!(preview.files[0].path, "x");
        assert_eq!(preview.counts(), (1, 1));
        assert_eq!(preview.files[0].hunks[0].lines[1].text, "++ new");
    }

    #[test]
    fn incomplete_json_never_becomes_freeform_patch_text() {
        let input =
            json!({"patch_text":"*** Begin Patch\n*** Add File: real🦀\n+x\n*** End Patch"})
                .to_string();
        for (end, _) in input.char_indices() {
            assert!(from_tool("apply_patch", &input[..end]).is_none());
        }
        assert_eq!(
            from_tool("apply_patch", &input).unwrap().files[0].path,
            "real🦀"
        );
        assert!(
            from_tool(
                "apply_patch",
                "{\"patch_text\": \"\n*** Begin Patch\n*** Add File: fake\n+x\n"
            )
            .is_none()
        );
    }

    #[test]
    fn metadata_only_git_paths_with_embedded_separator_are_not_split_early() {
        let preview = from_tool(
            "patch",
            "diff --git a/dir b/🦀 b/dir b/🦀\nold mode 100644\nnew mode 100755\n",
        )
        .unwrap();
        assert_eq!(preview.files.len(), 1);
        assert_eq!(preview.files[0].path, "dir b/🦀");
        assert_eq!(preview.files[0].kind, "Edit");
        assert_eq!(preview.files[0].previous_path, None);
        for header in [
            "a/a space b/a space",
            "\"a/a space\" b/a space",
            "a/a space \"b/a space\"",
            "\"a/a space\" \"b/a space\"",
        ] {
            assert_eq!(
                git_paths(header),
                Some(("a space".to_owned(), "a space".to_owned()))
            );
        }
    }

    #[test]
    fn null_to_null_headers_and_orphan_hunks_have_no_file() {
        for patch in [
            "@@ -1 +1 @@\n-a\n+b",
            "--- /dev/null\n+++ /dev/null\n",
            "--- \n+++ \n",
            "+++ b/orphan\n+x",
        ] {
            assert!(from_patch(patch).is_none(), "{patch}");
        }
    }

    #[test]
    fn generated_multifile_hunks_never_change_content_ownership() {
        let content = [
            "",
            "++ b/fake",
            "-- a/fake",
            "🦀 日本語",
            "*** Update File: fake",
            "@@ -1 +1 @@",
        ];
        let mut patch = String::new();
        for (i, old) in content.iter().enumerate() {
            for (j, new) in content.iter().enumerate() {
                patch.push_str(&format!("--- a/file-{i}-{j}\n+++ b/file-{i}-{j}\n@@ -20,3 +30,3 @@ 🦀\n first\n-{old}\n+{new}\n last\n"));
            }
        }
        let preview = from_tool("patch", &patch).unwrap();
        assert_eq!(preview.files.len(), 36);
        for (i, file) in preview.files.iter().enumerate() {
            assert_eq!(file.path, format!("file-{}-{}", i / 6, i % 6));
            assert_eq!(file.counts(), (1, 1));
            assert_eq!(file.hunks[0].lines[1].text, content[i / 6]);
            assert_eq!(file.hunks[0].lines[2].text, content[i % 6]);
            assert_eq!(file.hunks[0].lines[1].old_line, Some(21));
            assert_eq!(file.hunks[0].lines[2].new_line, Some(31));
        }
    }

    fn replacement_block(old: &[&str], new: &[&str]) -> (Vec<DiffLine>, Vec<usize>, Vec<usize>) {
        let lines = old
            .iter()
            .map(|text| line(LineKind::Removed, text, None, None))
            .chain(
                new.iter()
                    .map(|text| line(LineKind::Added, text, None, None)),
            )
            .collect();
        (
            lines,
            (0..old.len()).collect(),
            (old.len()..old.len() + new.len()).collect(),
        )
    }

    #[test]
    fn review_fixture_pairs_title_replacement_after_inserted_comment() {
        let preview =
            from_patch(include_str!("../../../assets/previews/change-review.diff")).unwrap();
        let lines = &preview.files[0].hunks[0].lines;
        let removed = lines
            .iter()
            .position(|line| line.text.trim() == "title.to_string()")
            .unwrap();
        let added = lines
            .iter()
            .position(|line| line.text.trim() == "title.chars().take(80).collect()")
            .unwrap();
        let comment = added - 1;
        assert_eq!(
            replacement_pairs(lines, &[removed], &[comment, added]),
            [(removed, added)]
        );
        assert!(lines[comment].emphasis.is_none());
        assert_eq!(
            &lines[removed].text[lines[removed].emphasis.clone().unwrap()],
            "to_string"
        );
        assert_eq!(
            &lines[added].text[lines[added].emphasis.clone().unwrap()],
            "chars().take(80).collect"
        );
    }

    #[test]
    fn replacement_pairs_ignore_indentation_when_skipping_new_comments() {
        let (mut lines, removed, added) = replacement_block(
            &["                                title.to_string()"],
            &[
                "                                // explain the title",
                "title.chars().collect()",
            ],
        );
        assert_eq!(replacement_pairs(&lines, &removed, &added), [(0, 2)]);
        emphasize(&mut lines);
        assert!(lines[1].emphasis.is_none());
        assert!(lines[0].emphasis.is_some());
        assert!(lines[2].emphasis.is_some());
    }

    #[test]
    fn replacement_pairs_skip_removed_comments_symmetrically() {
        let (mut lines, removed, added) = replacement_block(
            &["// obsolete description", "    title.to_string()"],
            &["\ttitle.chars().collect()"],
        );
        assert_eq!(replacement_pairs(&lines, &removed, &added), [(1, 2)]);
        emphasize(&mut lines);
        assert!(lines[0].emphasis.is_none());
        assert!(lines[1].emphasis.is_some());
        assert!(lines[2].emphasis.is_some());
    }

    #[test]
    fn replacement_pairs_and_emphasis_are_unicode_safe() {
        let (mut lines, removed, added) = replacement_block(
            &["\u{3000}日本語🦀 = 猫;"],
            &["\u{3000}// 日本語の説明", "\t日本語🦀 = 犬;"],
        );
        assert_eq!(replacement_pairs(&lines, &removed, &added), [(0, 2)]);
        emphasize(&mut lines);
        assert!(lines[1].emphasis.is_none());
        for line in &lines {
            if let Some(range) = &line.emphasis {
                assert!(line.text.get(range.clone()).is_some());
            }
        }
        // Different indentation is genuinely part of the intraline change,
        // even though it is deliberately excluded from replacement scoring.
        assert!(lines[0].text[lines[0].emphasis.clone().unwrap()].contains('猫'));
        assert!(lines[2].text[lines[2].emphasis.clone().unwrap()].contains('犬'));
        assert!(replacement_similarity("日本語🦀.古い()", "日本語🦀.新しい()") > 0);
    }

    #[test]
    fn equal_sized_replacements_keep_stable_positional_pairs() {
        let (lines, removed, added) =
            replacement_block(&["alpha()", "beta()"], &["beta()", "alpha()"]);
        assert_eq!(
            replacement_pairs(&lines, &removed, &added),
            [(0, 2), (1, 3)]
        );
        let (lines, removed, added) = replacement_block(&["alpha()"], &["wholly different"]);
        assert_eq!(replacement_pairs(&lines, &removed, &added), [(0, 1)]);
    }

    #[test]
    fn replacement_pairs_preserve_both_orders_and_nearest_ties() {
        let (lines, removed, added) = replacement_block(
            &["alpha()", "beta()"],
            &["// first", "alpha(1)", "// second", "beta(2)"],
        );
        assert_eq!(
            replacement_pairs(&lines, &removed, &added),
            [(0, 3), (1, 5)]
        );
        let (lines, removed, added) = replacement_block(&["x", "y"], &["a", "b", "c"]);
        assert_eq!(
            replacement_pairs(&lines, &removed, &added),
            [(0, 2), (1, 3)]
        );
        for old_len in 0..24 {
            for new_len in 0..24 {
                let old = (0..old_len)
                    .map(|i| ["alpha()", "beta()", "gamma()"][i % 3])
                    .collect::<Vec<_>>();
                let new = (0..new_len)
                    .map(|i| ["// comment", "gamma()", "beta()", "alpha()"][i % 4])
                    .collect::<Vec<_>>();
                let (lines, removed, added) = replacement_block(&old, &new);
                let pairs = replacement_pairs(&lines, &removed, &added);
                assert_eq!(pairs.len(), old_len.min(new_len));
                assert!(
                    pairs
                        .windows(2)
                        .all(|pair| pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1)
                );
                assert!(
                    pairs
                        .iter()
                        .all(|(a, b)| removed.contains(a) && added.contains(b))
                );
            }
        }
    }

    #[test]
    fn replacement_pair_lookahead_and_long_line_scoring_are_capped() {
        let mut additions = vec!["// unrelated comment"; 32];
        additions.push("target(1)");
        let (lines, removed, added) = replacement_block(&["target()"], &additions);
        // A candidate beyond the fixed window is intentionally not scanned.
        assert_eq!(replacement_pairs(&lines, &removed, &added), [(0, 1)]);
        let long = "🦀".repeat(100_000);
        assert_eq!(replacement_similarity(&long, &long), 128 * 3);
        let (lines, removed, added) = replacement_block(&vec!["old"; 20_000], &vec!["new"; 40_000]);
        let pairs = replacement_pairs(&lines, &removed, &added);
        assert_eq!(pairs.len(), 20_000);
        assert_eq!(pairs[0], (0, 20_000));
        assert_eq!(pairs[19_999], (19_999, 39_999));
    }

    #[test]
    fn replacement_pairs_ignore_nonmatching_kinds_and_invalid_indices() {
        let (lines, _, _) = replacement_block(&["old"], &["new", "// extra"]);
        assert_eq!(
            replacement_pairs(&lines, &[0, usize::MAX], &[1, 2]),
            [(0, 1)]
        );
        assert!(replacement_pairs(&lines, &[1], &[0]).is_empty());
        assert!(replacement_pairs(&lines, &[], &[1, 2]).is_empty());
    }
}
