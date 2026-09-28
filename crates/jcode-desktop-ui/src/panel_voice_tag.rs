//! Speech-to-text output is marked so the model knows the words were
//! transcribed (and may contain recognition errors), not typed.

use std::borrow::Cow;

const OPEN: &str = "<transcription>";
const CLOSE: &str = "</transcription>";

/// Wrap a transcript for the model. The tags stay in the editable draft so
/// the user can see and remove them before sending.
pub(crate) fn wrap(text: &str) -> String {
    format!("{OPEN}\n{}\n{CLOSE}", text.trim())
}

/// Remove transcription tags for display. Returns the visible text and
/// whether any complete tagged segment was found. Unbalanced text is kept
/// verbatim so an unrelated literal `<transcription>` is never hidden.
pub(crate) fn strip(text: &str) -> (Cow<'_, str>, bool) {
    if !text.contains(OPEN) || !text.contains(CLOSE) {
        return (Cow::Borrowed(text), false);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut found = false;
    while let Some(start) = rest.find(OPEN) {
        let after_open = &rest[start + OPEN.len()..];
        let Some(end) = after_open.find(CLOSE) else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(after_open[..end].trim_matches('\n'));
        rest = &after_open[end + CLOSE.len()..];
        found = true;
    }
    if !found {
        return (Cow::Borrowed(text), false);
    }
    out.push_str(rest);
    (Cow::Owned(out.trim().to_string()), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_strip_round_trip() {
        let wrapped = wrap("  fix the flaky test ");
        assert_eq!(
            wrapped,
            "<transcription>\nfix the flaky test\n</transcription>"
        );
        assert_eq!(strip(&wrapped), (Cow::Borrowed("fix the flaky test"), true));
    }

    #[test]
    fn strip_keeps_typed_text_and_multiple_segments() {
        let text = format!("Typed first\n{}\nand {}", wrap("one"), wrap("two"));
        let (visible, found) = strip(&text);
        assert!(found);
        assert_eq!(visible, "Typed first\none\nand two");
    }

    #[test]
    fn strip_ignores_plain_and_unbalanced_text() {
        assert_eq!(strip("hello"), (Cow::Borrowed("hello"), false));
        let open = "literal <transcription> only";
        assert_eq!(strip(open), (Cow::Borrowed(open), false));
    }
}
