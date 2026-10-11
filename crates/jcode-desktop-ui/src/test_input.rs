//! Test input helpers.

/// Type `text` into the focused element as one IME commit: one key-down event
/// and one `replace_text_in_range` through the window's input handler, the
/// same path `simulate_input` takes for each character. Use it when a test
/// needs text in the composer but is not about per-keystroke behavior.
/// `simulate_input` re-dispatches and re-renders the window once per
/// character, which made long prompts the slowest part of the suite.
pub(crate) fn commit_text(vcx: &mut gpui::VisualTestContext, text: &str) {
    let Some(first) = text.chars().next() else {
        return;
    };
    let keystroke = gpui::Keystroke {
        modifiers: gpui::Modifiers::default(),
        key: first.to_lowercase().to_string(),
        key_char: Some(text.to_owned()),
    };
    vcx.update(|window, cx| {
        window.dispatch_keystroke(keystroke, cx);
    });
    vcx.run_until_parked();
}
