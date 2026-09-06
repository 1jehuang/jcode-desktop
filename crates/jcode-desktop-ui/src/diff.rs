//! Previews of requested tool edits, derived solely from their arguments.
//!
//! These are not working-tree diffs: no files are read, replacement occurrence
//! counts are unknown, and a `write` may overwrite an existing file. File headers
//! are kept as metadata, while `lines` contains hunk headers and diff body lines.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FileDiff {
    pub path: String,
    pub previous_path: Option<String>,
    pub kind: String,
    pub lines: Vec<String>,
}

impl FileDiff {
    pub(crate) fn added(&self) -> usize {
        // A body line such as "+++ heading" is an addition, not a file header.
        self.lines
            .iter()
            .filter(|line| line.starts_with('+'))
            .count()
    }

    pub(crate) fn removed(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.starts_with('-'))
            .count()
    }
}

/// Preview supported tools. Invalid or unrelated inputs produce no preview.
/// `replace_all` and deletion contents cannot be resolved without filesystem
/// access, so only the text actually supplied by the caller is displayed.
pub(crate) fn tool_diffs(name: &str, input: &str) -> Vec<FileDiff> {
    match serde_json::from_str::<Value>(input) {
        Ok(value) => value_diffs(name, &value, 0),
        Err(_) if matches!(name, "apply_patch" | "patch") => raw_patch_diffs(input),
        Err(_) => Vec::new(),
    }
}

/// Freeform tool arguments may be streaming. Buffer unterminated metadata
/// so a partially received filename is never presented as a real path.
/// JSON arguments, unlike raw input, have an explicit completeness boundary.
fn raw_patch_diffs(input: &str) -> Vec<FileDiff> {
    let input = input.trim_start();
    if !(input.starts_with("*** Begin Patch\n")
        || input.starts_with("*** Begin Patch\r\n")
        || input.starts_with("diff --git ")
        || input.starts_with("--- "))
    {
        return Vec::new();
    }
    parse_patch(input, !input.ends_with('\n'))
}

fn file_diff(path: &str, kind: &str) -> FileDiff {
    FileDiff {
        path: path.to_owned(),
        previous_path: None,
        kind: kind.to_owned(),
        lines: Vec::new(),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name)?.as_str()
}

fn value_diffs(name: &str, input: &Value, depth: usize) -> Vec<FileDiff> {
    if depth >= 16 {
        return Vec::new();
    }
    // Batched calls may carry JSON-encoded arguments instead of an object.
    if let Some(encoded) = input.as_str() {
        return match serde_json::from_str::<Value>(encoded) {
            Ok(decoded) => value_diffs(name, &decoded, depth + 1),
            Err(_) if matches!(name, "apply_patch" | "patch") => patch_diffs(encoded),
            Err(_) => Vec::new(),
        };
    }
    match name {
        "edit" | "multiedit" | "write" => {
            let Some(path) = field(input, "file_path").filter(|path| !path.is_empty()) else {
                return Vec::new();
            };
            let mut diff = file_diff(
                path,
                if name == "write" {
                    "Written"
                } else {
                    "Modified"
                },
            );
            match name {
                "write" => {
                    let Some(content) = field(input, "content") else {
                        return Vec::new();
                    };
                    append_lines(&mut diff.lines, '+', content);
                }
                "edit" => {
                    if !append_edit(&mut diff.lines, input) {
                        return Vec::new();
                    }
                }
                _ => {
                    let Some(edits) = input.get("edits").and_then(Value::as_array) else {
                        return Vec::new();
                    };
                    let mut valid = false;
                    for edit in edits {
                        // Keep operations separate rather than suggesting they
                        // are adjacent in the original file.
                        let mut lines = Vec::new();
                        if append_edit(&mut lines, edit) {
                            if valid {
                                diff.lines.push("@@ next requested edit @@".to_owned());
                            }
                            diff.lines.extend(lines);
                            valid = true;
                        }
                    }
                    if !valid {
                        return Vec::new();
                    }
                }
            }
            vec![diff]
        }
        "apply_patch" | "patch" => field(input, "patch_text")
            .map(patch_diffs)
            .unwrap_or_default(),
        "batch" => input
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .flat_map(|call| {
                        let Some(tool) = field(call, "tool") else {
                            return Vec::new();
                        };
                        let args = call
                            .get("input")
                            .or_else(|| call.get("arguments"))
                            .unwrap_or(call);
                        value_diffs(tool, args, depth + 1)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn append_lines(lines: &mut Vec<String>, prefix: char, text: &str) {
    lines.extend(text.lines().map(|line| format!("{prefix}{line}")));
    if !text.is_empty() && !text.ends_with('\n') {
        lines.push("\\ No newline at end of file".to_owned());
    }
}

fn append_edit(lines: &mut Vec<String>, input: &Value) -> bool {
    let (Some(old), Some(new)) = (field(input, "old_string"), field(input, "new_string")) else {
        return false;
    };
    // Snippets are not complete files, so their missing trailing newline does
    // not imply that the file itself lacks one.
    lines.extend(old.lines().map(|line| format!("-{line}")));
    lines.extend(new.lines().map(|line| format!("+{line}")));
    true
}

fn patch_diffs(patch: &str) -> Vec<FileDiff> {
    parse_patch(patch, false)
}

fn parse_patch(patch: &str, partial_last_line: bool) -> Vec<FileDiff> {
    let lines: Vec<&str> = patch.lines().collect();
    if lines.iter().any(|line| *line == "*** Begin Patch") {
        codex_diffs(&lines, partial_last_line)
    } else {
        unified_diffs(&lines, partial_last_line)
    }
}

fn finish(current: &mut Option<FileDiff>, result: &mut Vec<FileDiff>) {
    if let Some(diff) = current.take() {
        result.push(diff);
    }
}

fn codex_diffs(lines: &[&str], partial_last_line: bool) -> Vec<FileDiff> {
    let mut result = Vec::new();
    let mut current = None;
    let mut in_patch = false;
    for (index, &line) in lines.iter().enumerate() {
        if partial_last_line && index + 1 == lines.len() && line.starts_with("***") {
            break;
        }
        if line == "*** Begin Patch" {
            finish(&mut current, &mut result);
            in_patch = true;
        } else if line == "*** End Patch" {
            finish(&mut current, &mut result);
            in_patch = false;
        } else if in_patch {
            let header = [
                ("*** Update File: ", "Modified"),
                ("*** Add File: ", "Added"),
                ("*** Delete File: ", "Deleted"),
            ]
            .iter()
            .find_map(|(prefix, kind)| line.strip_prefix(prefix).map(|path| (path, *kind)));
            if let Some((path, kind)) = header {
                finish(&mut current, &mut result);
                current = (!path.trim().is_empty()).then(|| file_diff(path, kind));
            } else if let Some(diff) = &mut current {
                if let Some(path) = line.strip_prefix("*** Move to: ") {
                    if !path.trim().is_empty() && diff.kind == "Modified" {
                        diff.previous_path =
                            Some(std::mem::replace(&mut diff.path, path.to_owned()));
                        diff.kind = "Renamed".to_owned();
                    }
                } else if line.is_empty()
                    || line.starts_with([' ', '+', '-'])
                    || line.starts_with("@@")
                    || line == "\\ No newline at end of file"
                {
                    diff.lines.push(line.to_owned());
                }
            }
        }
    }
    finish(&mut current, &mut result);
    result
}

#[derive(Default)]
struct UnifiedFile {
    old: Option<String>,
    new: Option<String>,
    kind: Option<&'static str>,
    lines: Vec<String>,
    git: bool,
}

impl UnifiedFile {
    fn finish(self, result: &mut Vec<FileDiff>) {
        let old = self
            .old
            .filter(|path| !path.is_empty() && path != "/dev/null");
        let new = self
            .new
            .filter(|path| !path.is_empty() && path != "/dev/null");
        let Some(path) = new.as_ref().or(old.as_ref()) else {
            return;
        };
        let kind = if self.kind == Some("Added") || old.is_none() {
            "Added"
        } else if self.kind == Some("Deleted") || new.is_none() {
            "Deleted"
        } else if old != new {
            "Renamed"
        } else {
            "Modified"
        };
        result.push(FileDiff {
            path: path.clone(),
            previous_path: (kind == "Renamed").then_some(old).flatten(),
            kind: kind.to_owned(),
            lines: self.lines,
        });
    }
}

/// Hunk lengths distinguish file headers from removed `-- ...` or added
/// `++ ...` content. Looking for `---`/`+++` alone would split a valid file.
fn hunk_counts(line: &str) -> Option<(usize, usize)> {
    let body = line.strip_prefix("@@ -")?;
    let (old, body) = body.split_once(" +")?;
    let (new, _) = body.split_once(" @@")?;
    fn count(range: &str) -> Option<usize> {
        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
        start.parse::<usize>().ok()?;
        count.parse().ok()
    }
    Some((count(old)?, count(new)?))
}

fn unified_diffs(lines: &[&str], partial_last_line: bool) -> Vec<FileDiff> {
    let mut result = Vec::new();
    let mut current: Option<UnifiedFile> = None;
    let mut hunk: Option<(usize, usize)> = None;
    // Once hunk syntax becomes inconsistent, bare ---/+++ lines are
    // ambiguous. Only an explicit git boundary safely resumes parsing.
    let mut invalid_hunk = false;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let (Some(file), Some((old, new))) = (&mut current, &mut hunk) {
            let consumed = match line.as_bytes().first() {
                Some(b' ') if *old > 0 && *new > 0 => {
                    *old -= 1;
                    *new -= 1;
                    true
                }
                Some(b'-') if *old > 0 => {
                    *old -= 1;
                    true
                }
                Some(b'+') if *new > 0 => {
                    *new -= 1;
                    true
                }
                _ => line == "\\ No newline at end of file",
            };
            if consumed {
                file.lines.push(line.to_owned());
                index += 1;
                continue;
            }
            if *old > 0 || *new > 0 {
                invalid_hunk = true;
            }
        }
        hunk = None;
        if partial_last_line && index + 1 == lines.len() {
            break;
        }
        if invalid_hunk && !line.starts_with("diff --git ") {
            index += 1;
            continue;
        }
        if let Some(header) = line.strip_prefix("diff --git ") {
            invalid_hunk = false;
            if let Some(file) = current.take() {
                file.finish(&mut result);
            }
            let (old, new) = git_paths(header);
            current = Some(UnifiedFile {
                old,
                new,
                git: true,
                ..Default::default()
            });
        } else if let Some(old_header) = line.strip_prefix("--- ") {
            if partial_last_line && index + 2 == lines.len() {
                break;
            }
            if let Some(new_header) = lines
                .get(index + 1)
                .and_then(|line| line.strip_prefix("+++ "))
            {
                // A git metadata-only entry has not acquired its file headers
                // yet. Any other header pair starts a distinct file.
                if current
                    .as_ref()
                    .is_some_and(|file| !file.git || !file.lines.is_empty())
                {
                    current.take().unwrap().finish(&mut result);
                }
                let file = current.get_or_insert_with(UnifiedFile::default);
                let old = header_path(old_header);
                let new = header_path(new_header);
                let prefixed = file.git
                    || (old.starts_with("a/") && (new.starts_with("b/") || new == "/dev/null"))
                    || (old == "/dev/null" && new.starts_with("b/"));
                file.old = Some(if prefixed {
                    strip_side(&old, "a/")
                } else {
                    old
                });
                file.new = Some(if prefixed {
                    strip_side(&new, "b/")
                } else {
                    new
                });
                // Further pairs must not overwrite an empty first file.
                file.git = false;
                index += 1;
            } else if current.is_some() {
                invalid_hunk = true;
            }
        } else if let Some(file) = &mut current {
            if let Some(counts) = hunk_counts(line) {
                file.lines.push(line.to_owned());
                hunk = Some(counts);
            } else if line.starts_with("@@") {
                file.lines.push(line.to_owned());
                invalid_hunk = true;
            } else if line.starts_with("new file mode ") {
                file.kind = Some("Added");
            } else if line.starts_with("deleted file mode ") {
                file.kind = Some("Deleted");
            } else if let Some(path) = line.strip_prefix("rename from ") {
                file.old = Some(header_path(path));
            } else if let Some(path) = line.strip_prefix("rename to ") {
                file.new = Some(header_path(path));
            } else if line.starts_with(['+', '-', ' ']) {
                invalid_hunk = true;
            }
        }
        index += 1;
    }
    if let Some(file) = current {
        file.finish(&mut result);
    }
    result
}

fn strip_side(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_owned()
}

fn header_path(header: &str) -> String {
    if header.starts_with('"') {
        if let Some((path, _)) = quoted_path(header) {
            return path;
        }
    }
    // Unified timestamps are tab-delimited. Spaces belong to the filename.
    header.split('\t').next().unwrap_or_default().to_owned()
}

fn git_paths(header: &str) -> (Option<String>, Option<String>) {
    let paths = if header.starts_with('"') {
        quoted_path(header).and_then(|(old, rest)| {
            let rest = rest.trim_start();
            let new = if rest.starts_with('"') {
                quoted_path(rest)?.0
            } else {
                rest.to_owned()
            };
            Some((old, new))
        })
    } else if let Some((old, new)) = header.split_once(" \"b/") {
        quoted_path(&format!("\"b/{new}")).map(|(new, _)| (old.to_owned(), new))
    } else {
        // Git does not necessarily quote spaces in filenames.
        // Prefer an equal-path split so `a/dir b/file b/dir b/file`
        // does not invent a rename. Real renames have authoritative
        // `rename from/to` metadata or a later file-header pair.
        header
            .match_indices(" b/")
            .find_map(|(index, _)| {
                let old = &header[..index];
                let new = &header[index + 1..];
                (old.strip_prefix("a/") == new.strip_prefix("b/"))
                    .then(|| (old.to_owned(), new.to_owned()))
            })
            .or_else(|| {
                header
                    .split_once(" b/")
                    .map(|(old, new)| (old.to_owned(), format!("b/{new}")))
            })
    };
    paths.map_or((None, None), |(old, new)| {
        (Some(strip_side(&old, "a/")), Some(strip_side(&new, "b/")))
    })
}

/// Decode git's C-quoted paths, including octal UTF-8 bytes, without slicing
/// a Rust string at arbitrary byte boundaries.
fn quoted_path(input: &str) -> Option<(String, &str)> {
    let bytes = input.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut output = Vec::new();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                return Some((
                    String::from_utf8_lossy(&output).into_owned(),
                    &input[i + 1..],
                ));
            }
            b'\\' => {
                i += 1;
                let escaped = *bytes.get(i)?;
                if (b'0'..=b'7').contains(&escaped) {
                    let mut octal = u16::from(escaped - b'0');
                    for _ in 0..2 {
                        if let Some(&digit @ b'0'..=b'7') = bytes.get(i + 1) {
                            i += 1;
                            octal = octal * 8 + u16::from(digit - b'0');
                        } else {
                            break;
                        }
                    }
                    output.push(u8::try_from(octal).ok()?);
                } else {
                    output.push(match escaped {
                        b'a' => 7,
                        b'b' => 8,
                        b't' => b'\t',
                        b'n' => b'\n',
                        b'v' => 11,
                        b'f' => 12,
                        b'r' => b'\r',
                        b'"' => b'"',
                        b'\\' => b'\\',
                        _ => return None,
                    });
                }
            }
            byte => output.push(byte),
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, input: Value) -> Vec<FileDiff> {
        tool_diffs(name, &input.to_string())
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
            let diffs = tool_diffs("patch", &patch);
            assert_eq!(diffs.len(), 1, "{body}");
            assert_eq!(diffs[0].path, "real", "{body}");
        }
    }

    #[test]
    fn explicit_git_boundary_recovers_after_a_malformed_hunk() {
        let patch = "diff --git a/real b/real\n--- a/real\n+++ b/real\n@@ -0,0 +1,2 @@\n--- a/fake\n+++ b/fake\ndiff --git a/next b/next\n--- a/next\n+++ b/next\n@@ -1 +1 @@\n-a\n+b\n";
        let diffs = tool_diffs("patch", patch);
        assert_eq!(
            diffs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["real", "next"]
        );
        assert_eq!(diffs[1].lines, ["@@ -1 +1 @@", "-a", "+b"]);
    }

    #[test]
    fn raw_patch_prefixes_never_expose_partial_or_content_filenames() {
        let patches = [
            (
                "apply_patch",
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
                .map(|(index, _)| index)
                .chain(std::iter::once(patch.len()))
            {
                for diff in tool_diffs(name, &patch[..end]) {
                    assert!(
                        names.contains(&diff.path.as_str()),
                        "{name} prefix {end}: {diff:?}"
                    );
                    if let Some(previous) = diff.previous_path {
                        assert!(
                            names.contains(&previous.as_str()),
                            "{name} prefix {end}: {previous}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn raw_headers_wait_for_a_line_terminator_but_complete_json_does_not() {
        for input in [
            "*** Begin Patch\n*** Add File: par",
            "*** Begin Patch\n*** Update File: par",
            "*** Begin Patch\n*** Delete File: par",
            "diff --git a/partial b/par",
            "--- a/partial\n+++ b/par",
        ] {
            assert!(tool_diffs("apply_patch", input).is_empty(), "{input}");
        }
        let complete = tool(
            "patch",
            json!({"patch_text":"--- a/old\n+++ b/new\n@@ -1 +1 @@\n-old\n+new"}),
        );
        assert_eq!(complete[0].lines, ["@@ -1 +1 @@", "-old", "+new"]);
    }

    #[test]
    fn raw_unterminated_body_lines_are_preserved_without_becoming_headers() {
        let unified = tool_diffs("patch", "--- a/x\n+++ b/x\n@@ -1 +1 @@\n--- old\n+++ new");
        assert_eq!(unified[0].path, "x");
        assert_eq!(unified[0].lines, ["@@ -1 +1 @@", "--- old", "+++ new"]);
        let codex = tool_diffs("apply_patch", "*** Begin Patch\n*** Add File: x\n+++ new");
        assert_eq!(codex[0].path, "x");
        assert_eq!(codex[0].lines, ["+++ new"]);
    }

    #[test]
    fn incomplete_json_is_not_reinterpreted_as_freeform_patch_text() {
        let json = json!({"patch_text":"*** Begin Patch\n*** Add File: real🦀\n+x\n*** End Patch"})
            .to_string();
        for (end, _) in json.char_indices() {
            assert!(tool_diffs("apply_patch", &json[..end]).is_empty());
        }
        assert_eq!(tool_diffs("apply_patch", &json)[0].path, "real🦀");
        assert!(
            tool_diffs(
                "apply_patch",
                "{\"patch_text\": \"\n*** Begin Patch\n*** Add File: fake\n+x\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn metadata_only_paths_containing_git_separator_are_not_split_early() {
        let diffs = tool_diffs(
            "patch",
            "diff --git a/dir b/🦀 b/dir b/🦀\nold mode 100644\nnew mode 100755\n",
        );
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "dir b/🦀");
        assert_eq!(diffs[0].kind, "Modified");
        assert_eq!(diffs[0].previous_path, None);
    }

    #[test]
    fn codex_blank_context_and_empty_added_lines_are_preserved() {
        let diffs = tool_diffs(
            "apply_patch",
            "*** Begin Patch\n*** Update File: x\n@@\n\n-old\n+new\n*** Add File: y\n+\n+\n*** End Patch",
        );
        assert_eq!(diffs[0].lines, ["@@", "", "-old", "+new"]);
        assert_eq!(diffs[1].lines, ["+", "+"]);
        assert_eq!(diffs[1].added(), 2);
    }

    #[test]
    fn no_filename_is_invented_for_orphan_hunks_or_null_headers() {
        for patch in [
            "@@ -1 +1 @@\n-a\n+b",
            "--- /dev/null\n+++ /dev/null\n",
            "--- \n+++ \n",
            "+++ b/orphan\n+x",
        ] {
            assert!(tool_diffs("patch", patch).is_empty(), "{patch}");
        }
    }

    #[test]
    fn generated_multifile_hunks_keep_every_line_in_its_own_file() {
        let content = [
            "",
            "++ b/fake",
            "-- a/fake",
            "🦀 日本語",
            "*** Update File: fake",
            "@@ -1 +1 @@",
        ];
        let mut patch = String::new();
        let mut expected = Vec::new();
        for (index, old) in content.iter().enumerate() {
            for (next, new) in content.iter().enumerate() {
                let path = format!("file-{index}-{next}");
                let mut diff = file_diff(&path, "Modified");
                diff.lines = vec![
                    "@@ -1,3 +1,3 @@ 🦀".into(),
                    " first".into(),
                    format!("-{old}"),
                    format!("+{new}"),
                    " last".into(),
                ];
                patch.push_str(&format!(
                    "--- a/{path}\n+++ b/{path}\n{}\n",
                    diff.lines.join("\n")
                ));
                expected.push(diff);
            }
        }
        let actual = tool_diffs("patch", &patch);
        assert_eq!(actual, expected);
        assert!(
            actual
                .iter()
                .all(|diff| diff.added() == 1 && diff.removed() == 1)
        );
    }

    #[test]
    fn edits_show_requested_unicode_text_only() {
        let diffs = tool(
            "edit",
            json!({"file_path":"不存在/🦀.rs", "old_string":"你好\n🦀", "new_string":"こんにちは\n🦀!", "replace_all":true}),
        );
        assert_eq!(
            diffs,
            vec![FileDiff {
                path: "不存在/🦀.rs".into(),
                previous_path: None,
                kind: "Modified".into(),
                lines: vec![
                    "-你好".into(),
                    "-🦀".into(),
                    "+こんにちは".into(),
                    "+🦀!".into()
                ],
            }]
        );
        assert_eq!((diffs[0].added(), diffs[0].removed()), (2, 2));
    }

    #[test]
    fn write_does_not_claim_a_new_file() {
        let diffs = tool(
            "write",
            json!({"file_path":"existing", "content":"++ heading\n\n🦀"}),
        );
        assert_eq!(diffs[0].kind, "Written");
        assert_eq!(
            diffs[0].lines,
            ["+++ heading", "+", "+🦀", "\\ No newline at end of file"]
        );
        assert_eq!((diffs[0].added(), diffs[0].removed()), (3, 0));
        let empty = tool("write", json!({"file_path":"empty", "content":""}));
        assert_eq!(empty[0].kind, "Written");
        assert!(empty[0].lines.is_empty());
    }

    #[test]
    fn multiedit_preserves_operation_order_and_separation() {
        let diffs = tool(
            "multiedit",
            json!({"file_path":"x", "edits":[
                {"old_string":"a", "new_string":"b"}, {"unrelated":true},
                {"old_string":"b", "new_string":"c\nd"}
            ]}),
        );
        assert_eq!(diffs.len(), 1);
        assert_eq!(
            diffs[0].lines,
            ["-a", "+b", "@@ next requested edit @@", "-b", "+c", "+d"]
        );
        assert_eq!((diffs[0].added(), diffs[0].removed()), (3, 2));
    }

    #[test]
    fn codex_multiple_files_and_all_kinds() {
        let patch = "*** Begin Patch\n*** Update File: src/旧.rs\n*** Move to: src/新.rs\n@@ fn 日本語()\n context\n-old\n+new\n*** Add File: new.txt\n+hello\n+++ content\n*** Delete File: gone.txt\n*** Update File: last.txt\n@@\n-last\n+final\n*** End of File\n*** End Patch\n";
        let diffs = tool("apply_patch", json!({"patch_text":patch}));
        assert_eq!(diffs.len(), 4);
        assert_eq!(diffs[0].path, "src/新.rs");
        assert_eq!(diffs[0].previous_path.as_deref(), Some("src/旧.rs"));
        assert_eq!(diffs[0].kind, "Renamed");
        assert_eq!(
            diffs[0].lines,
            ["@@ fn 日本語()", " context", "-old", "+new"]
        );
        assert_eq!(diffs[1].kind, "Added");
        assert_eq!(diffs[1].lines, ["+hello", "+++ content"]);
        assert_eq!(diffs[1].added(), 2);
        assert_eq!(diffs[2].path, "gone.txt");
        assert_eq!(diffs[2].kind, "Deleted");
        assert_eq!(diffs[2].removed(), 0); // No file contents were supplied.
        assert_eq!(diffs[3].path, "last.txt");
        assert_eq!(diffs[3].kind, "Modified");
        assert_eq!(diffs[3].lines, ["@@", "-last", "+final"]);
    }

    #[test]
    fn codex_markers_inside_content_are_not_metadata() {
        let diffs = tool_diffs(
            "apply_patch",
            "*** Begin Patch\n*** Update File: x\n@@\n-*** Add File: fake\n+*** Delete File: fake\n *** End Patch\n*** End Patch\n+ignored\n",
        );
        assert_eq!(diffs.len(), 1);
        assert_eq!(
            diffs[0].lines,
            [
                "@@",
                "-*** Add File: fake",
                "+*** Delete File: fake",
                " *** End Patch"
            ]
        );
    }

    #[test]
    fn unified_multiple_files_preserve_context_and_hunks() {
        let patch = "--- a/one\n+++ b/one\n@@ -1,3 +1,3 @@ section\n first\n-old\n+new\n last\n@@ -8 +8 @@\n-before\n+after\n--- a/two\n+++ b/two\n@@ -1 +1 @@\n-two\n+second\n";
        let diffs = tool("patch", json!({"patch_text":patch}));
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].path, "one");
        assert_eq!(diffs[0].kind, "Modified");
        assert_eq!(
            diffs[0].lines,
            [
                "@@ -1,3 +1,3 @@ section",
                " first",
                "-old",
                "+new",
                " last",
                "@@ -8 +8 @@",
                "-before",
                "+after"
            ]
        );
        assert_eq!((diffs[0].added(), diffs[0].removed()), (2, 2));
        assert_eq!(diffs[1].path, "two");
        assert_eq!(diffs[1].lines, ["@@ -1 +1 @@", "-two", "+second"]);
    }

    #[test]
    fn header_like_content_cannot_steal_file_ownership() {
        let patch = "--- a/real\n+++ b/real\n@@ -1,2 +1,2 @@\n--- a/not-a-file\n+++ b/not-a-file\n--- old heading\n+++ new heading\n--- a/next\n+++ b/next\n@@ -0,0 +1 @@\n+++ another heading\n";
        let diffs = tool_diffs("patch", patch);
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].path, "real");
        assert_eq!(
            diffs[0].lines,
            [
                "@@ -1,2 +1,2 @@",
                "--- a/not-a-file",
                "+++ b/not-a-file",
                "--- old heading",
                "+++ new heading"
            ]
        );
        assert_eq!((diffs[0].added(), diffs[0].removed()), (2, 2));
        assert_eq!(diffs[1].path, "next");
        assert_eq!(diffs[1].added(), 1);
    }

    #[test]
    fn unified_added_deleted_and_renamed_files() {
        let diffs = tool_diffs(
            "patch",
            "--- /dev/null\n+++ b/added\n@@ -0,0 +1,2 @@\n+a\n+b\n--- a/deleted\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n--- old name\t2026-01-01\n+++ new name\t2026-01-02\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_eq!(diffs.len(), 3);
        assert_eq!(
            (&*diffs[0].path, &*diffs[0].kind, diffs[0].added()),
            ("added", "Added", 2)
        );
        assert_eq!(
            (&*diffs[1].path, &*diffs[1].kind, diffs[1].removed()),
            ("deleted", "Deleted", 2)
        );
        assert_eq!(diffs[2].path, "new name");
        assert_eq!(diffs[2].previous_path.as_deref(), Some("old name"));
        assert_eq!(diffs[2].kind, "Renamed");
    }

    #[test]
    fn git_metadata_only_entries_stay_separate() {
        let diffs = tool_diffs(
            "patch",
            "diff --git a/old name b/new name\nsimilarity index 100%\nrename from old name\nrename to new name\ndiff --git a/empty b/empty\nnew file mode 100644\nindex 0000000..e69de29\ndiff --git a/gone b/gone\ndeleted file mode 100644\nindex e69de29..0000000\ndiff --git a/mode b/mode\nold mode 100644\nnew mode 100755\n",
        );
        assert_eq!(
            diffs
                .iter()
                .map(|d| (d.path.as_str(), d.kind.as_str()))
                .collect::<Vec<_>>(),
            [
                ("new name", "Renamed"),
                ("empty", "Added"),
                ("gone", "Deleted"),
                ("mode", "Modified")
            ]
        );
        assert_eq!(diffs[0].previous_path.as_deref(), Some("old name"));
        assert!(diffs.iter().all(|d| d.lines.is_empty()));
    }

    #[test]
    fn git_headers_do_not_duplicate_files() {
        let diffs = tool_diffs(
            "patch",
            "diff --git a/x b/x\nindex 123..456 100644\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/y b/y\nnew file mode 100644\n--- /dev/null\n+++ b/y\n@@ -0,0 +1 @@\n+y\n",
        );
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].path, "x");
        assert_eq!(diffs[1].path, "y");
        assert_eq!(diffs[1].kind, "Added");
    }

    #[test]
    fn quoted_git_unicode_paths_and_escapes() {
        let diffs = tool_diffs(
            "patch",
            "diff --git \"a/\\346\\227\\247\\tname\" \"b/\\346\\226\\260\\tname\"\nrename from \"\\346\\227\\247\\tname\"\nrename to \"\\346\\226\\260\\tname\"\n--- \"a/\\346\\227\\247\\tname\"\n+++ \"b/\\346\\226\\260\\tname\"\n@@ -1 +1 @@\n-🦀\n+日本語\n",
        );
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "新\tname");
        assert_eq!(diffs[0].previous_path.as_deref(), Some("旧\tname"));
        assert_eq!((diffs[0].added(), diffs[0].removed()), (1, 1));
        assert_eq!(
            quoted_path("\"a/🦀\\\"\\\\.rs\" rest"),
            Some(("a/🦀\"\\.rs".into(), " rest"))
        );
    }

    #[test]
    fn git_paths_with_spaces_and_mixed_quoting() {
        for header in [
            "a/a space b/a space",
            "\"a/a space\" b/a space",
            "a/a space \"b/a space\"",
            "\"a/a space\" \"b/a space\"",
        ] {
            assert_eq!(
                git_paths(header),
                (Some("a space".into()), Some("a space".into()))
            );
        }
    }

    #[test]
    fn no_newline_markers_are_preserved_not_counted() {
        let diffs = tool_diffs(
            "patch",
            "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n--- a/y\n+++ b/y\n@@ -0,0 +1 @@\n+y\n",
        );
        assert_eq!(diffs.len(), 2);
        assert_eq!(
            diffs[0].lines,
            [
                "@@ -1 +1 @@",
                "-old",
                "\\ No newline at end of file",
                "+new",
                "\\ No newline at end of file"
            ]
        );
        assert_eq!((diffs[0].added(), diffs[0].removed()), (1, 1));
    }

    #[test]
    fn crlf_and_unprefixed_paths() {
        let diffs = tool_diffs(
            "patch",
            "--- a/local.txt\tdate\r\n+++ a/local.txt\tdate\r\n@@ -1 +1 @@\r\n-旧\r\n+新\r\n",
        );
        assert_eq!(diffs[0].path, "a/local.txt");
        assert_eq!(diffs[0].kind, "Modified");
        assert_eq!(diffs[0].lines, ["@@ -1 +1 @@", "-旧", "+新"]);
    }

    #[test]
    fn header_only_files_are_not_overwritten() {
        let diffs = tool_diffs(
            "patch",
            "--- a/first\n+++ b/first\n--- a/second\n+++ b/second\n@@ -0,0 +1 @@\n+x\n",
        );
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].path, "first");
        assert!(diffs[0].lines.is_empty());
        assert_eq!(diffs[1].path, "second");
    }

    #[test]
    fn nested_batch_accepts_inline_object_and_encoded_arguments() {
        let diffs = tool(
            "batch",
            json!({"tool_calls":[
                {"tool":"write", "file_path":"one", "content":"1"},
                {"tool":"batch", "input":{"tool_calls":[
                    {"tool":"edit", "arguments":{"file_path":"two", "old_string":"a", "new_string":"b"}},
                    {"tool":"write", "input":"{\"file_path\":\"three\",\"content\":\"3\"}"},
                    {"tool":"read", "file_path":"not-a-diff"}
                ]}},
                {"tool":"apply_patch", "patch_text":"*** Begin Patch\n*** Delete File: four\n*** End Patch"}
            ]}),
        );
        assert_eq!(
            diffs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["one", "two", "three", "four"]
        );
    }

    #[test]
    fn batch_recursion_is_bounded() {
        let mut input = json!({"tool_calls":[{"tool":"write", "file_path":"x", "content":"x"}]});
        for _ in 0..20 {
            input = json!({"tool_calls":[{"tool":"batch", "input":input}]});
        }
        assert!(tool("batch", input).is_empty());
    }

    #[test]
    fn invalid_and_unknown_inputs_are_ignored() {
        for name in [
            "read",
            "edit",
            "write",
            "multiedit",
            "batch",
            "patch",
            "apply_patch",
        ] {
            for input in ["", "not json", "null", "[]", "{}", "{\"file_path\":\"x\"}"] {
                assert!(tool_diffs(name, input).is_empty(), "{name}: {input}");
            }
        }
        assert!(
            tool(
                "edit",
                json!({"file_path":"x", "old_string":1, "new_string":"x"})
            )
            .is_empty()
        );
        assert!(tool("multiedit", json!({"file_path":"x", "edits":[]})).is_empty());
        assert!(tool("write", json!({"file_path":"", "content":"x"})).is_empty());
    }

    #[test]
    fn raw_and_json_encoded_patches_match() {
        let patch = "*** Begin Patch\n*** Add File: x\n+x\n*** End Patch";
        let expected = tool_diffs("patch", patch);
        assert_eq!(expected, tool("apply_patch", json!({"patch_text":patch})));
        assert_eq!(expected, tool("patch", json!(patch)));
    }

    #[test]
    fn hunk_ranges_default_to_one_and_reject_malformed_counts() {
        assert_eq!(hunk_counts("@@ -3 +4 @@ function"), Some((1, 1)));
        assert_eq!(hunk_counts("@@ -0,0 +1,5 @@"), Some((0, 5)));
        for invalid in [
            "@@",
            "@@ -1,x +1,2 @@",
            "@@ -x +1 @@",
            "@@ -1 +1",
            "@@@ -1 +1 @@@",
            "@@ -1,9999999999999999999999999 +1 @@",
        ] {
            assert_eq!(hunk_counts(invalid), None);
        }
    }
}
