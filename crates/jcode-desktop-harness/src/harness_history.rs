//! Failed bootstrap reads must not leave a connected panel without history forever.
//! This budget belongs to one attachment and retries reads only, never prompts.
use std::time::{Duration, Instant};

const MAX_RETRIES: u32 = 4;
#[cfg(not(test))]
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const RETRY_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Default)]
pub(super) struct HistoryRecovery {
    next: Option<Instant>,
    retries: u32,
}

impl HistoryRecovery {
    pub(super) fn load(
        &mut self,
        client: &jcode_sdk::JcodeClient,
        real_id: &str,
        session_id: &str,
        updates: &super::UpdateSender,
    ) {
        match client.get_history_with_images(real_id) {
            Ok((messages, images)) => {
                self.loaded();
                let _ = updates.send(super::Update::History {
                    session_id: session_id.to_owned(),
                    messages,
                    images,
                });
            }
            Err(error) => {
                eprintln!("jcode desktop: session {session_id} history read failed: {error}");
                if self.failed(Instant::now()) {
                    let _ = updates.send(super::Update::CommandFailed {
                        session_id: session_id.to_owned(),
                        reason: format!(
                            "Could not load conversation history after repeated attempts. Close and reopen this panel to retry. Live updates remain connected. {error}"
                        ),
                    });
                }
            }
        }
    }

    pub(super) fn due(&mut self, now: Instant) -> bool {
        if self.next.is_some_and(|next| now >= next) {
            self.next = None;
            self.retries += 1;
            true
        } else {
            false
        }
    }

    /// Return true once, when the initial read plus all retries have failed.
    pub(super) fn failed(&mut self, now: Instant) -> bool {
        if self.retries >= MAX_RETRIES {
            self.next = None;
            true
        } else {
            self.next = Some(now + RETRY_INTERVAL);
            false
        }
    }

    pub(super) fn loaded(&mut self) {
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_history_reads_retry_at_a_bounded_rate_then_stop() {
        let mut recovery = HistoryRecovery::default();
        let mut now = Instant::now();
        assert!(!recovery.due(now));
        assert!(!recovery.failed(now));
        for attempt in 1..=MAX_RETRIES {
            assert!(!recovery.due(now));
            assert!(!recovery.due(now + RETRY_INTERVAL / 2));
            now += RETRY_INTERVAL;
            assert!(recovery.due(now));
            assert!(!recovery.due(now));
            assert_eq!(recovery.failed(now), attempt == MAX_RETRIES);
        }
        assert!(!recovery.due(now + Duration::from_secs(3600)));
    }

    #[test]
    fn loaded_cancels_the_watchdog_and_new_attachment_gets_a_fresh_budget() {
        let now = Instant::now();
        let mut recovery = HistoryRecovery::default();
        assert!(!recovery.failed(now));
        recovery.loaded();
        assert!(!recovery.due(now + Duration::from_secs(3600)));
        let mut reattached = HistoryRecovery::default();
        assert!(!reattached.failed(now));
        assert!(reattached.due(now + RETRY_INTERVAL));
    }
}
