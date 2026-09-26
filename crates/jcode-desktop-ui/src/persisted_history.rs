//! Read-only access to a local session's persisted transcript.
//!
//! Attaching a runtime takes hundreds of milliseconds (the daemon restores the
//! agent before it can answer `get_history`). The stored record renders in a
//! few milliseconds, so panels paint it immediately and let the authoritative
//! reply replace it. The resume picker uses the same path for its preview.

pub(crate) type History = (
    Vec<jcode_sdk::HistoryMessage>,
    Vec<jcode_sdk::RenderedImage>,
);

/// Whether `id` names a session that may live in local storage. Remote,
/// pending, and synthetic panels never read the disk.
pub(crate) fn is_local_session_id(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '\\', ':']) && id != "." && id != ".."
}

pub(crate) fn load(id: &str) -> Result<History, String> {
    if !is_local_session_id(id) {
        return Err("Not a local session.".into());
    }
    let path = jcode_base::session::session_path(id)
        .map_err(|_| "Conversation storage unavailable.".to_owned())?;
    load_path(&path)
}

pub(crate) fn load_path(path: &std::path::Path) -> Result<History, String> {
    let session = jcode_base::session::Session::load_from_path(path)
        .map_err(|_| "Conversation preview unavailable. Resume to load this session.".to_owned())?;
    Ok(render(&session))
}

/// Same rendering the server uses for `History`, converted through the same
/// wire shape the harness API delivers to a resumed chat panel.
pub(crate) fn render(session: &jcode_base::session::Session) -> History {
    let (messages, images) = jcode_base::session::render_messages_and_images(session);
    let messages = messages
        .into_iter()
        .map(|message| jcode_sdk::HistoryMessage {
            response_stats: message
                .response_stats
                .and_then(|stats| serde_json::to_value(stats).ok())
                .and_then(|stats| serde_json::from_value(stats).ok()),
            role: message.role,
            content: message.content,
        })
        .collect();
    let images = images
        .into_iter()
        .filter_map(|image| {
            serde_json::to_value(image)
                .ok()
                .and_then(|image| serde_json::from_value(image).ok())
        })
        .collect();
    (messages, images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_ids_are_local() {
        assert!(is_local_session_id("session_fox_1_abc"));
        for id in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "remote://host/x",
            "startup://draft",
        ] {
            assert!(!is_local_session_id(id), "{id}");
        }
    }
}
