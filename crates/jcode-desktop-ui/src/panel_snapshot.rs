//! A hot-reload cache, not authoritative runtime history or a crash checkpoint.
use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSnapshot {
    items: Vec<Item>,
    streaming_text: String,
    streaming_reasoning: String,
    history_loaded: bool,
    status: String,
    connection_phase: String,
    #[serde(default)]
    expanded_tools: HashSet<String>,
}

impl PartialEq for TranscriptImage {
    fn eq(&self, other: &Self) -> bool {
        self.media_type == other.media_type
            && self.data == other.data
            && self.label == other.label
            && self.source == other.source
            && self.anchor == other.anchor
    }
}

impl Panel {
    pub fn snapshot_for_reload(&self, cx: &App) -> PanelSnapshot {
        let mut snapshot = self.snapshot(cx);
        snapshot.transcript = Some(self.transcript_snapshot());
        snapshot
    }

    pub(super) fn transcript_snapshot(&self) -> TranscriptSnapshot {
        TranscriptSnapshot {
            items: self.items.clone(),
            streaming_text: self.streaming_text.clone(),
            streaming_reasoning: self.streaming_reasoning.clone(),
            history_loaded: self.history_loaded,
            status: self.status.clone(),
            connection_phase: self.connection_phase.clone(),
            expanded_tools: self.expanded_tools.clone(),
        }
    }

    pub(super) fn restore_transcript(&mut self, snapshot: TranscriptSnapshot) {
        eprintln!(
            "jcode desktop: restored transcript items={} text_bytes={} reasoning_bytes={}",
            snapshot.items.len(),
            snapshot.streaming_text.len(),
            snapshot.streaming_reasoning.len()
        );
        self.restored_transcript = !snapshot.history_loaded
            && (!snapshot.items.is_empty()
                || !snapshot.streaming_text.is_empty()
                || !snapshot.streaming_reasoning.is_empty());
        self.expanded_tools = snapshot.expanded_tools;
        self.items = snapshot.items;
        for item in &mut self.items {
            if let Item::Image(image) = item {
                // GPU/image-cache handles never cross the plugin boundary.
                image.preview = TranscriptImage::new(
                    image.media_type.clone(),
                    image.data.clone(),
                    image.label.clone(),
                )
                .preview;
            }
        }
        self.streaming_text = snapshot.streaming_text;
        self.streaming_reasoning = snapshot.streaming_reasoning;
        // Restored text was already read. Do not replay its reveal.
        self.text_reveal.snap(self.streaming_text.len());
        self.reasoning_reveal.snap(self.streaming_reasoning.len());
        self.history_loaded = snapshot.history_loaded;
        self.status = snapshot.status;
        self.connection_phase = snapshot.connection_phase;
        self.transcript_measurements.dirty = true;
    }

    // A reload can precede the first history reply. Preserve the local suffix
    // rather than feeding the cache to the normal prepend-history path.
    pub(super) fn hydrate_restored_prefix(
        &mut self,
        messages: &[jcode_sdk::HistoryMessage],
    ) -> bool {
        if messages.is_empty() {
            return false;
        }
        let local_users: Vec<_> = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::User(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let history_users: Vec<_> = messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .collect();
        if !history_users.ends_with(&local_users) {
            return false;
        }
        let prefix_users = history_users.len() - local_users.len();
        let end = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "user")
            .nth(prefix_users)
            .map(|(index, _)| index)
            .unwrap_or_else(|| {
                messages
                    .iter()
                    .rposition(|m| m.role == "user")
                    .map_or(0, |index| index + 1)
            });
        let mut prefix = Vec::new();
        for message in &messages[..end] {
            match message.role.as_str() {
                "user" => prefix.push(Item::User(message.content.clone())),
                "assistant" if !message.content.trim().is_empty() => {
                    prefix.push(Item::Assistant(message.content.clone()))
                }
                _ => {}
            }
        }
        let offset = prefix.len();
        prefix.append(&mut self.items);
        self.items = prefix;
        for index in &mut self.pending_users {
            *index += offset;
        }
        self.accepted_users = std::mem::take(&mut self.accepted_users)
            .into_iter()
            .map(|(index, at)| (index + offset, at))
            .collect();
        self.history_loaded = true;
        self.restored_transcript = false;
        true
    }

    pub(super) fn defer_reconnect_history(&mut self, messages: &[jcode_sdk::HistoryMessage]) {
        self.reconnect_response = None;
        let local_users: Vec<_> = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::User(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let history_users: Vec<_> = messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .collect();
        // Content alone cannot identify repeated prompts. Compare the whole
        // sequence and never let a shorter persisted view roll a live turn back.
        if local_users.is_empty() || !history_users.starts_with(&local_users) {
            return;
        }
        if history_users.len() > local_users.len() {
            let next_user = messages
                .iter()
                .enumerate()
                .filter(|(_, m)| m.role == "user")
                .nth(local_users.len())
                .map(|(index, _)| index)
                .unwrap();
            let latest_user = messages.iter().rposition(|m| m.role == "user").unwrap();
            // A newer user boundary proves the preceding turn is historical.
            // Preserve its rich tools/images, then hydrate intervening settled
            // turns without copying the newest assistant over queued deltas.
            if let Some(response) = messages[..next_user]
                .iter()
                .rev()
                .take_while(|m| m.role != "user")
                .find(|m| m.role == "assistant" && !m.content.trim().is_empty())
            {
                self.recover_response(&response.content);
            }
            self.flush_reasoning();
            self.flush_streaming();
            for message in &messages[next_user..=latest_user] {
                match message.role.as_str() {
                    "user" => self.items.push(Item::User(message.content.clone())),
                    "assistant" if !message.content.trim().is_empty() => {
                        self.items.push(Item::Assistant(message.content.clone()))
                    }
                    _ => {}
                }
            }
        }
        self.reconnect_response = messages
            .iter()
            .rev()
            .take_while(|message| message.role != "user")
            .find(|message| message.role == "assistant" && !message.content.trim().is_empty())
            .cloned();
    }

    pub(super) fn reconcile_reconnect_response(&mut self) {
        if let Some(response) = self.reconnect_response.take() {
            // Do not flush a different live suffix and append an older stored
            // segment after it. Only equal/extended text can reconcile a stream.
            if self.streaming_text.is_empty() || response.content.starts_with(&self.streaming_text)
            {
                self.recover_response(&response.content);
                if let Some(stats) = response.response_stats {
                    self.finish_response();
                    let mut restored: response_stats::ResponseStats = stats.into();
                    if let Some(Item::ResponseStats(existing)) = self.items.last_mut() {
                        restored.duration_secs = restored.duration_secs.or(existing.duration_secs);
                        restored.tool_calls = existing.tool_calls;
                        *existing = restored;
                    } else if !restored.is_empty() {
                        self.items.push(Item::ResponseStats(restored));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "panel_snapshot_tests.rs"]
mod tests;
