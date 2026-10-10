//! Short, calm summaries of raw runtime errors.
//!
//! Raw errors from the SDK, bridge and providers name sockets, syscalls and
//! CLI commands. The transcript shows a one-line summary and a Copy button
//! keeps the complete original for bug reports.

/// Longest summary shown inline, in characters.
pub const MAX_SUMMARY_CHARS: usize = 140;

/// A short, human summary of `raw`. Never empty for non-empty input.
pub fn summarize(raw: &str) -> String {
    let message = strip_code_prefix(raw.trim());
    if let Some(known) = known_summary(message) {
        return known.to_string();
    }
    truncate(first_sentence(message), MAX_SUMMARY_CHARS)
}

/// Whether the summary drops information that Copy should preserve.
pub fn summary_hides_details(raw: &str) -> bool {
    summarize(raw).trim_end_matches('.') != raw.trim().trim_end_matches('.')
}

fn known_summary(message: &str) -> Option<&'static str> {
    let lower = message.to_ascii_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|term| lower.contains(term));
    let runtime = has(&["harness", "jcode-api", "jcode.sock", ".sock", "runtime", "bridge"]);
    if has(&["refuses connections", "connection refused", "refused the connection"]) && runtime {
        Some("Can't reach the Jcode runtime.")
    } else if has(&["no harness api socket", "harness is not running", "harness not running"]) {
        Some("The Jcode runtime isn't running.")
    } else if runtime && has(&["permission denied"]) {
        Some("Can't access the Jcode runtime. It may belong to another user.")
    } else if has(&[
        "socket closed",
        "connection closed",
        "connection reset",
        "broken pipe",
        "unexpected eof",
    ]) {
        Some("Lost connection to the Jcode runtime.")
    } else if has(&["timed out", "timeout"]) {
        Some("The request timed out.")
    } else {
        None
    }
}

/// Drops a leading machine code such as `connect_failed: `.
fn strip_code_prefix(message: &str) -> &str {
    match message.split_once(": ") {
        Some((code, rest))
            if !code.is_empty()
                && !rest.is_empty()
                && code
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') =>
        {
            rest
        }
        _ => message,
    }
}

fn first_sentence(message: &str) -> &str {
    let bytes = message.as_bytes();
    for (index, char) in message.char_indices() {
        if matches!(char, '.' | '!' | '?')
            && bytes.get(index + 1).is_none_or(|next| next.is_ascii_whitespace())
            && index + 1 >= 12
        {
            return &message[..=index];
        }
        if char == '\n' && index > 0 {
            return message[..index].trim_end();
        }
    }
    message
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 1).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALE: &str = "connect_failed: /run/user/1000/jcode-api.sock exists but refuses connections: a previous harness left a stale socket behind. Remove it and start the harness again.";

    #[test]
    fn stale_socket_becomes_one_short_line() {
        assert_eq!(summarize(STALE), "Can't reach the Jcode runtime.");
        assert!(summary_hides_details(STALE));
    }

    #[test]
    fn missing_socket_and_closed_connections_are_named_plainly() {
        assert_eq!(
            summarize("connect_failed: no harness API socket at /x.sock: the jcode harness is not running."),
            "The Jcode runtime isn't running."
        );
        assert_eq!(summarize("Socket closed"), "Lost connection to the Jcode runtime.");
        assert_eq!(
            summarize("connect_failed: harness refused the connection at /run/user/1000/jcode-api.sock. It may be restarting or stopped."),
            "Can't reach the Jcode runtime."
        );
        assert_eq!(
            summarize("connect_failed: harness not running (no socket at /x.sock)."),
            "The Jcode runtime isn't running."
        );
    }

    #[test]
    fn unknown_errors_keep_their_first_sentence() {
        assert_eq!(summarize("I/O failure: read only"), "I/O failure: read only");
        assert!(!summary_hides_details("I/O failure: read only"));
        assert_eq!(
            summarize("Disk is full. Free some space and try again."),
            "Disk is full."
        );
        assert_eq!(summarize("first line\nsecond line"), "first line");
    }

    #[test]
    fn long_errors_are_truncated_on_a_word() {
        let long = "word ".repeat(80);
        let summary = summarize(&long);
        assert!(summary.chars().count() <= MAX_SUMMARY_CHARS);
        assert!(summary.ends_with("word…"));
    }

    #[test]
    fn version_numbers_and_paths_do_not_end_a_sentence() {
        assert_eq!(
            summarize("Failed to load v1.2.3 config at ~/.jcode/config.toml"),
            "Failed to load v1.2.3 config at ~/.jcode/config.toml"
        );
    }
}
