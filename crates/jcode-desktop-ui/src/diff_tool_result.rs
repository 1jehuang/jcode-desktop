//! Prefer execution-time file positions over argument-only snippet offsets.
use super::DiffPreview;

pub(super) fn from_result(name: &str, input: &str, output: &str) -> Option<DiffPreview> {
    let name = name
        .trim()
        .strip_prefix("functions.")
        .unwrap_or(name.trim());
    let mut preview = super::from_tool(name, input)?;
    // Tool output is persisted in history and delivered by ToolDone. Never read
    // today's working tree to invent positions for an older edit.
    let mut applied = Vec::new();
    let mut lines = output.lines();
    while let Some(line) = lines.next() {
        if line != "File diff:" || lines.next() != Some("```diff") {
            continue;
        }
        let mut patch = String::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if line == "```" {
                closed = true;
                break;
            }
            patch.push_str(line);
            patch.push('\n');
        }
        if closed && let Some(parsed) = super::raw_patch(&patch) {
            applied.extend(parsed.files.into_iter().filter(|file| file.note.is_none()));
        }
    }
    for file in &mut preview.files {
        if let Some(index) = applied.iter().position(|actual| actual.path == file.path) {
            *file = applied.remove(index);
        }
    }
    // Older edit results include a numbered post-edit context window. A unique
    // match gives a real file location without trusting the lossy compact diff
    // (whose historical start counter was wrong for mid-line replacements).
    if name.trim_start_matches("functions.") == "edit" && preview.files.len() == 1 {
        anchor_legacy_edit(&mut preview.files[0], input, output);
    }
    Some(preview)
}

fn anchor_legacy_edit(file: &mut super::DiffFile, input: &str, output: &str) {
    if !file
        .hunks
        .iter()
        .any(|h| h.header.contains("snippet-relative"))
    {
        return;
    }
    let Ok(args) = serde_json::from_str::<serde_json::Value>(input) else {
        return;
    };
    if args.get("replace_all").and_then(|v| v.as_bool()) == Some(true) {
        return;
    }
    let Some(new) = args
        .get("new_string")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let Some((_, context)) = output.split_once("\nContext after edit (lines ") else {
        return;
    };
    let Some((range, context)) = context
        .split_once("): \n")
        .or_else(|| context.split_once("):\n"))
    else {
        return;
    };
    let Some((start, end)) = range.split_once('-') else {
        return;
    };
    let (Ok(context_start), Ok(context_end)) = (start.parse::<usize>(), end.parse::<usize>())
    else {
        return;
    };
    let mut first = None;
    let mut text = String::new();
    let mut expected = 0;
    for line in context.lines() {
        let Some((number, content)) = line.split_once("│ ") else {
            break;
        };
        let Ok(number) = number.trim().parse::<usize>() else {
            break;
        };
        if first.is_none() {
            first = Some(number);
            expected = number;
        }
        if number == 0 || number != expected {
            return;
        }
        let Some(next) = expected.checked_add(1) else {
            return;
        };
        expected = next;
        text.push_str(content);
        text.push('\n');
    }
    let Some(first) = first else { return };
    if first != context_start || expected.checked_sub(1) != Some(context_end) {
        return;
    }
    let Some(offset) = text.find(new) else {
        return;
    };
    let next_offset = offset + new.chars().next().unwrap().len_utf8();
    if text[next_offset..].contains(new) {
        return;
    }
    let Some(start) = first.checked_add(text[..offset].bytes().filter(|b| *b == b'\n').count())
    else {
        return;
    };
    if file
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .flat_map(|l| [l.old_line, l.new_line])
        .flatten()
        .any(|n| n.checked_add(start - 1).is_none())
    {
        return;
    }
    for hunk in &mut file.hunks {
        for line in &mut hunk.lines {
            line.old_line = line.old_line.map(|n| n + start - 1);
            line.new_line = line.new_line.map(|n| n + start - 1);
        }
        let old = hunk
            .lines
            .iter()
            .filter_map(|l| l.old_line)
            .collect::<Vec<_>>();
        let new = hunk
            .lines
            .iter()
            .filter_map(|l| l.new_line)
            .collect::<Vec<_>>();
        hunk.header = format!(
            "@@ -{},{} +{},{} @@",
            old.first().copied().unwrap_or(start.saturating_sub(1)),
            old.len(),
            new.first().copied().unwrap_or(start.saturating_sub(1)),
            new.len()
        );
    }
    file.note = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_model::LineKind;

    fn args() -> String {
        serde_json::json!({"file_path":"src/demo.rs", "old_string":"old();\n", "new_string":"new();\n"}).to_string()
    }

    #[test]
    fn execution_diff_supplies_actual_positions_and_full_context() {
        let output = "Edited src/demo.rs\n\nFile diff:\n```diff\n--- src/demo.rs\n+++ src/demo.rs\n@@ -41,2 +41,2 @@\n context\n-old();\n+new();\n```\n";
        let preview = from_result("edit", &args(), output).unwrap();
        let file = &preview.files[0];
        assert!(file.note.is_none());
        assert_eq!(file.hunks[0].lines[1].old_line, Some(42));
        assert_eq!(file.hunks[0].lines[2].new_line, Some(42));
        assert_eq!(file.hunks[0].lines[0].kind, LineKind::Context);
    }

    #[test]
    fn incomplete_or_unrelated_diff_cannot_assign_positions() {
        for output in [
            "File diff:\n```diff\n--- src/demo.rs\n+++ src/demo.rs\n@@ -41 +41 @@\n-old();\n+new();\n",
            "File diff:\n```diff\n--- other.rs\n+++ other.rs\n@@ -41 +41 @@\n-old();\n+new();\n```\n",
        ] {
            let preview = from_result("edit", &args(), output).unwrap();
            assert!(
                preview.files[0].hunks[0]
                    .header
                    .contains("snippet-relative")
            );
        }
    }

    #[test]
    fn old_edit_context_anchors_real_positions_not_buggy_summary() {
        let output = "Edited src/demo.rs: replaced 1 occurrence(s)\n43- old();\n43+ new();\n\nContext after edit (lines 41-43):\n  41│ before();\n  42│ new();\n  43│ after();";
        let preview = from_result("edit", &args(), output).unwrap();
        let file = &preview.files[0];
        assert_eq!(file.hunks[0].lines[0].old_line, Some(42));
        assert_eq!(file.hunks[0].lines[1].new_line, Some(42));
        assert!(!file.hunks[0].header.contains("snippet-relative"));
    }

    #[test]
    fn ambiguous_legacy_context_does_not_guess() {
        let output = "Context after edit (lines 41-42):\n  41│ new();\n  42│ new();";
        let preview = from_result("edit", &args(), &format!("\n{output}")).unwrap();
        assert!(
            preview.files[0].hunks[0]
                .header
                .contains("snippet-relative")
        );
    }
    #[test]
    fn header_only_execution_diff_means_no_net_changes() {
        let preview = from_result(
            "edit",
            &args(),
            "Done\n\nFile diff:\n```diff\n--- a/src/demo.rs\n+++ b/src/demo.rs\n```\n",
        )
        .unwrap();
        assert!(preview.files[0].hunks.is_empty());
    }

    #[test]
    fn unmarked_fences_and_empty_unknown_results_do_not_override_arguments() {
        for output in [
            "```diff\n--- a/src/demo.rs\n+++ b/src/demo.rs\n@@ -42 +42 @@\n-old();\n+new();\n```\n",
            "File diff:\n```diff\n```\n",
            "File diff:\n```diff\n--- a/src/demo.rs\n+++ b/src/demo.rs\n@@ -42,4 +42,4 @@\n-old();\n+new();\n```\n",
        ] {
            let preview = from_result("edit", &args(), output).unwrap();
            assert!(
                preview.files[0].hunks[0]
                    .header
                    .contains("snippet-relative")
            );
        }
    }

    #[test]
    fn mixed_noop_changed_and_unavailable_files_keep_distinct_states() {
        let input = serde_json::json!({"patch_text": "*** Begin Patch\n*** Update File: noop.rs\n@@\n-old\n+new\n*** Update File: changed.rs\n@@\n-old\n+new\n*** Update File: unavailable.rs\n@@\n-old\n+new\n*** End Patch"}).to_string();
        let output = "File diff:\n```diff\n--- a/noop.rs\n+++ b/noop.rs\n--- a/changed.rs\n+++ b/changed.rs\n@@ -99 +99 @@\n-old\n+new\n```\n";
        let preview = from_result("apply_patch", &input, output).unwrap();
        assert!(preview.files[0].hunks.is_empty());
        assert_eq!(preview.files[1].hunks[0].lines[0].old_line, Some(99));
        assert!(
            preview.files[2].hunks[0]
                .header
                .contains("snippet-relative")
        );
    }
    #[test]
    fn truncated_legacy_context_does_not_assign_positions() {
        let preview = from_result(
            "edit",
            &args(),
            "Done\nContext after edit (lines 41-45):\n  41│ new();\n... (truncated)",
        )
        .unwrap();
        assert!(
            preview.files[0].hunks[0]
                .header
                .contains("snippet-relative")
        );
    }
}
