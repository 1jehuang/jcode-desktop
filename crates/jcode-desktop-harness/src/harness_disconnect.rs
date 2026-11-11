//! Match the TUI's fail-fast policy for corrupt remote protocol frames.
use super::{Update, UpdateSender};

pub(super) fn report_protocol_failure(
    error: &jcode_sdk::Error,
    session_id: &str,
    updates: &UpdateSender,
) -> bool {
    if error.kind != jcode_sdk::ErrorKind::Protocol {
        return false;
    }
    let reason = format!(
        "Session protocol error. Automatic reconnect stopped to avoid repeatedly loading malformed or oversized conversation data. Start a new session, or reduce oversized tool output or repair this session history before reopening the panel. {error}"
    );
    eprintln!("jcode desktop: session {session_id}: {reason}");
    let _ = updates.send(Update::SessionLost {
        session_id: session_id.to_owned(),
        reason: reason.clone(),
    });
    let _ = updates.send(Update::CommandFailed {
        session_id: session_id.to_owned(),
        reason,
    });
    true
}
