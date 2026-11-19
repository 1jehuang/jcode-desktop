//! Manual `/compact` refusals are expected outcomes, not failures.
//!
//! The daemon refuses compaction when there is nothing worth summarizing, a
//! compaction is already running, or the cut would split a tool call. It
//! reports that as an `invalid_request` carrying the TUI's markdown status
//! block, which reads as a crash when shown verbatim. Desktop turns it into a
//! one-line warning that says why and what to do.

/// A warning for a refused compaction, or `None` when the error is a real
/// failure (transport, timeout, unknown session) that must stay an error.
pub(super) fn refusal_warning(error: &jcode_sdk::Error) -> Option<String> {
    if error.kind != jcode_sdk::ErrorKind::Harness(jcode_sdk::api::ErrorCode::InvalidRequest) {
        return None;
    }
    Some(describe_refusal(&error.message))
}

fn describe_refusal(message: &str) -> String {
    let reason = refusal_reason(message);
    let usage = token_usage(message);
    let lower = reason.to_lowercase();
    if lower.contains("context usage too low") || lower.contains("nothing to compact") {
        let percent = percent_in(&reason);
        return match (percent, usage) {
            (Some(percent), Some(usage)) => format!(
                "Nothing to compact yet. The context is only {percent} full ({usage} tokens)."
            ),
            (Some(percent), None) => {
                format!("Nothing to compact yet. The context is only {percent} full.")
            }
            _ => "Nothing to compact yet. The context is still small.".into(),
        };
    }
    if lower.contains("not enough messages") || lower.contains("no messages available") {
        return "Nothing to compact yet. The conversation only has recent turns, which are always kept.".into();
    }
    if lower.contains("already in progress") {
        return "Compaction is already running. The summary is applied when it is ready.".into();
    }
    if lower.contains("split tool call") {
        return "Cannot compact right now without splitting a tool call. Try again after the current tool finishes.".into();
    }
    if lower.contains("lock held") {
        return "Compaction is busy. Try again in a moment.".into();
    }
    if lower.contains("not available for this provider") {
        return "Manual compaction is not available for this provider.".into();
    }
    if reason.is_empty() {
        "Compaction was skipped.".into()
    } else {
        format!("Compaction skipped: {reason}")
    }
}

/// The daemon's reason without the status block and markdown decoration.
fn refusal_reason(message: &str) -> String {
    let tail = message
        .rsplit_once("Cannot compact:")
        .map_or(message, |(_, tail)| tail);
    tail.trim_start_matches(['*', ' '])
        .replace("**", "")
        .replace('⚠', "")
        .trim()
        .trim_end_matches('.')
        .to_string()
}

/// `~50k / 1000k` from the status block's "Token usage" line.
fn token_usage(message: &str) -> Option<String> {
    let line = message
        .lines()
        .find_map(|line| line.split_once("Token usage:").map(|(_, rest)| rest))?;
    let used = line.split_whitespace().next()?;
    let budget = line.split_once('/')?.1.split_whitespace().next()?;
    Some(format!("{used} of {budget}"))
}

fn percent_in(text: &str) -> Option<String> {
    let end = text.find('%')?;
    let start = text[..end]
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |index| index + 1);
    (start < end).then(|| text[start..=end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOW_USAGE: &str = "**Context Status:**\n• Messages: 43 (active), 43 (total history)\n• Token usage: ~50k (estimate ~38k) / 1000k (5.1%)\n• Has summary: no\n• Compacting: no\n\n⚠ **Cannot compact:** Context usage too low (5.1%) - nothing to compact";

    fn harness_error(code: jcode_sdk::api::ErrorCode, message: &str) -> jcode_sdk::Error {
        jcode_sdk::Error::new(jcode_sdk::ErrorKind::Harness(code), message)
    }

    #[test]
    fn low_usage_refusal_is_a_short_warning() {
        let warning = refusal_warning(&harness_error(
            jcode_sdk::api::ErrorCode::InvalidRequest,
            LOW_USAGE,
        ))
        .unwrap();
        assert_eq!(
            warning,
            "Nothing to compact yet. The context is only 5.1% full (~50k of 1000k tokens)."
        );
        assert!(!warning.contains("**"));
        assert!(!warning.contains("invalid_request"));
    }

    #[test]
    fn known_refusals_are_explained() {
        let cases = [
            (
                "⚠ **Cannot compact:** Compaction already in progress",
                "already running",
            ),
            (
                "⚠ **Cannot compact:** Not enough messages to compact (need more than 4, have 2)",
                "only has recent turns",
            ),
            (
                "⚠ **Cannot compact:** Cannot compact - would split tool call/result pairs",
                "splitting a tool call",
            ),
            ("⚠ Cannot access compaction manager (lock held)", "busy"),
        ];
        for (message, expected) in cases {
            assert!(describe_refusal(message).contains(expected), "{message}");
        }
    }

    #[test]
    fn unknown_refusals_keep_the_reason_without_markdown() {
        assert_eq!(
            describe_refusal("**Context Status:**\n\n⚠ **Cannot compact:** something new."),
            "Compaction skipped: something new"
        );
    }

    #[test]
    fn transport_failures_stay_errors() {
        let error = jcode_sdk::Error::new(jcode_sdk::ErrorKind::Disconnected, "closed");
        assert!(refusal_warning(&error).is_none());
        let internal = harness_error(jcode_sdk::api::ErrorCode::Internal, "boom");
        assert!(refusal_warning(&internal).is_none());
    }
}
