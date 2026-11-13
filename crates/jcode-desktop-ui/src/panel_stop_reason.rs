//! Explicit abnormal endings. A transport outage is not evidence of a crashed agent.
use super::*;
use jcode_sdk::TurnStopReason;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StopNotice {
    pub(super) title: String,
    pub(super) detail: String,
    provider_stop_reason: Option<String>,
    pub(super) failure: bool,
    pub(super) provisional: bool,
    /// The complete raw text for Copy, when `detail` is a summary of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) raw: Option<String>,
    /// Set while automatic recovery owns this failure. A failed attempt that
    /// is being retried has not stopped the response.
    #[serde(default, skip_serializing_if = "AutoRetry::is_transient")]
    pub(super) retry: AutoRetry,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) enum AutoRetry {
    #[default]
    None,
    Pending,
    Recovered,
}

impl AutoRetry {
    /// A countdown does not survive a restart, so only the outcome persists.
    fn is_transient(&self) -> bool {
        *self != Self::Recovered
    }
}

impl StopNotice {
    pub(super) fn title(&self) -> &str {
        match self.retry {
            AutoRetry::None => &self.title,
            AutoRetry::Pending => "Request failed. Retrying automatically",
            AutoRetry::Recovered => "Request failed. Retried automatically",
        }
    }

    /// The red recovery box with Resume belongs to a failure nobody is handling.
    pub(super) fn shows_error(&self) -> bool {
        self.failure && self.retry == AutoRetry::None
    }

    /// Text shown under the title when this is not an actionable error.
    pub(super) fn plain_detail(&self) -> String {
        if self.failure {
            crate::friendly_error::summarize(&self.detail)
        } else {
            self.detail.clone()
        }
    }

    pub(super) fn from_event(
        reason: &TurnStopReason,
        message: &str,
        provider: Option<&str>,
    ) -> Self {
        let title = match reason {
            TurnStopReason::Interrupted => "Response interrupted",
            TurnStopReason::Failure => "Response stopped: error",
            TurnStopReason::Crash => "Response stopped: session crashed",
            TurnStopReason::ProviderGuardrail => "Response stopped: provider guardrail",
            TurnStopReason::LimitReached => "Response stopped: limit reached",
            _ => "Response stopped",
        };
        Self {
            title: title.into(),
            detail: message.to_owned(),
            provider_stop_reason: provider.map(str::to_owned),
            failure: *reason != TurnStopReason::Interrupted,
            provisional: false,
            raw: None,
            retry: AutoRetry::None,
        }
    }

    pub(super) fn cancel_requested() -> Self {
        Self {
            title: "Stop requested by you".into(),
            detail:
                "You interrupted this response. Waiting for the runtime to confirm cancellation."
                    .into(),
            provider_stop_reason: None,
            failure: false,
            provisional: true,
            raw: None,
            retry: AutoRetry::None,
        }
    }

    pub(super) fn from_status(status: &str) -> Option<Self> {
        let (reason, message) = match status {
            "cancelled" | "canceled" | "interrupted" => (
                TurnStopReason::Interrupted,
                "The runtime confirmed this response was interrupted.",
            ),
            "crashed" => (
                TurnStopReason::Crash,
                "The runtime reports that the session crashed. No further details were provided.",
            ),
            "error" | "failed" => (
                TurnStopReason::Failure,
                "The runtime reports that the response failed. No further details were provided.",
            ),
            _ => return None,
        };
        Some(Self::from_event(&reason, message, None))
    }
}

/// The cause in plain words, unless it only restates the title.
fn connection_lost_detail(reason: &str) -> String {
    let cause = crate::friendly_error::summarize(reason);
    if cause.starts_with("Lost connection") {
        "The response may still be running.".into()
    } else {
        format!("{cause} The response may still be running.")
    }
}

impl Panel {
    /// Update the failure of the current turn as automatic recovery progresses.
    pub(super) fn set_auto_retry(&mut self, retry: AutoRetry) {
        let notice = self
            .items
            .iter_mut()
            .rev()
            .take_while(|item| !matches!(item, Item::User(_)))
            .find_map(|item| match item {
                Item::Stopped(notice) if notice.failure && !notice.provisional => Some(notice),
                _ => None,
            });
        if let Some(notice) = notice
            && notice.retry != retry
            // Only a countdown that was shown can be recovered from or abandoned.
            && (retry == AutoRetry::Pending || notice.retry == AutoRetry::Pending)
        {
            notice.retry = retry;
            self.transcript_measurements.dirty = true;
        }
    }

    pub(super) fn record_stop(&mut self, notice: StopNotice) {
        if !notice.provisional {
            // Confirmed rejection/cancellation settles unacknowledged prompts.
            // A transport loss alone is ambiguous and must preserve them for
            // reconnect reconciliation rather than claiming delivery.
            self.pending_users.clear();
        }
        self.pause_queue_for_stop();
        self.sound_events.cancel();
        // Reconnect can replay processing and usage without delivering new
        // output. Do not append another accounting footer for that replay.
        let replayed_stop = self.streaming_text.trim().is_empty()
            && self.streaming_reasoning.trim().is_empty()
            && matches!(
                self.items
                    .iter()
                    .rev()
                    .find(|item| !matches!(item, Item::ResponseStats(_))),
                Some(Item::Stopped(_))
            );
        if replayed_stop {
            self.response_stats.finish();
        }
        self.finish_response();
        // Settle tool spinners without claiming the underlying command completed
        // successfully. Detached background tasks remain independent.
        for item in self
            .items
            .iter_mut()
            .rev()
            .take_while(|item| !matches!(item, Item::User(_)))
        {
            if let Item::Tool { done, error, .. } = item
                && !*done
            {
                *done = true;
                *error = Some("Response stopped before the tool result was received.".into());
            }
        }
        // Accounting can be appended by finish_response or replayed during
        // reconnect. It is not new response output, so look past it, but never
        // cross a user message or any actual assistant/tool output.
        let previous = self
            .items
            .iter_mut()
            .rev()
            .find(|item| !matches!(item, Item::ResponseStats(_)));
        if let Some(Item::Stopped(previous)) = previous {
            // A confirmed reason replaces our optimistic local cancellation or
            // unknown transport outcome. Replayed statuses never erase details.
            if previous.provisional || !notice.provisional {
                *previous = notice;
            }
        } else {
            self.items.push(Item::Stopped(notice));
        }
    }

    pub(crate) fn connection_lost(&mut self, reason: &str, cx: &mut Context<Self>) {
        if self.activity_active() || !self.pending_users.is_empty() {
            self.record_stop(StopNotice {
                title: "Connection lost. Reconnecting…".into(),
                detail: connection_lost_detail(reason),
                provider_stop_reason: None,
                failure: false,
                provisional: true,
                raw: Some(reason.to_owned()),
                retry: AutoRetry::None,
            });
        }
        self.status = format!("lost: {}", crate::friendly_error::summarize(reason));
        cx.notify();
    }

    pub(super) fn render_stop_notice(
        &self,
        index: usize,
        notice: &StopNotice,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let compact = crate::config::display().compact_notifications;
        div()
            .debug_selector(|| "response-stop-notice".into())
            .flex()
            .flex_col()
            .map(|el| {
                if compact {
                    el.gap_0().px_2().py_1()
                } else {
                    el.gap_1().px_3().py_2()
                }
            })
            .rounded_xl()
            .bg(Theme::global().HEADER_BG)
            .text_size(px(if compact { 11. } else { 12. }))
            .text_color(Theme::global().TEXT_DIM)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(Theme::global().TEXT)
                    .child(notice.title().to_owned()),
            )
            .when(notice.shows_error(), |el| {
                el.child(self.render_recovery_error(index, &notice.detail, window, cx))
            })
            .when(!notice.shows_error(), |el| {
                let copy = notice.raw.clone().unwrap_or_else(|| notice.detail.clone());
                el.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(text_selection::plain(
                            self.transcript_selection.clone(),
                            format!("{index}-stop-reason"),
                            notice.plain_detail(),
                            window,
                            cx,
                        )))
                        .child(super::recovery::copy_pill("stop-notice-copy", index, copy)),
                )
            })
            .children(
                notice
                    .provider_stop_reason
                    .as_ref()
                    .map(|reason| div().child(format!("Provider stop reason: {reason}"))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "panel_stop_reason_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "panel_reconnect_notice_tests.rs"]
mod reconnect_notice_tests;
