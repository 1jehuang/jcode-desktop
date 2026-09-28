//! KV (prompt) cache miss notices reported by the daemon.
//!
//! A miss means a large part of the previously cached prompt had to be resent
//! and billed again. Harness-caused misses (the harness mutated the cached
//! prefix) are rendered in the warning tone because they indicate a bug;
//! switches, expiry and documented refreshes are informational.
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheMissNotice {
    pub(super) reason: String,
    pub(super) harness_caused: bool,
    pub(super) documented_cause: Option<String>,
    pub(super) missed_tokens: u64,
    pub(super) message: String,
}

impl CacheMissNotice {
    pub(super) fn from_event(event: &ApiEvent) -> Option<Self> {
        let ApiEvent::KvCacheMiss {
            reason,
            harness_caused,
            missed_tokens,
            documented_cause,
            message,
            ..
        } = event
        else {
            return None;
        };
        Some(Self {
            reason: reason.clone(),
            harness_caused: *harness_caused,
            documented_cause: documented_cause.clone(),
            missed_tokens: *missed_tokens,
            message: message.clone(),
        })
    }

    /// Unexplained harness mutation of the cached prefix.
    pub(super) fn alarming(&self) -> bool {
        self.harness_caused && self.documented_cause.is_none()
    }

    pub(super) fn text(&self) -> String {
        self.message.clone()
    }
}

impl Panel {
    pub(super) fn render_cache_miss_notice(
        &self,
        index: usize,
        notice: &CacheMissNotice,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let tone = if notice.alarming() {
            theme.WARN
        } else {
            theme.TEXT_DIM
        };
        div()
            .debug_selector(|| "kv-cache-miss-notice".into())
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .rounded_full()
            .bg(theme.HEADER_BG)
            .text_size(px(11.))
            .text_color(tone)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(if notice.alarming() {
                        "⚠ Cache bust"
                    } else {
                        "Cache miss"
                    }),
            )
            .child(div().min_w_0().flex_1().child(text_selection::plain(
                self.transcript_selection.clone(),
                format!("{index}-kv-cache-miss"),
                notice.text(),
                window,
                cx,
            )))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn miss(harness_caused: bool, documented: Option<&str>) -> ApiEvent {
        ApiEvent::KvCacheMiss {
            session_id: "cache".into(),
            reason: "prefix_changed".into(),
            harness_caused,
            missed_tokens: 46_000,
            expected_tokens: 50_000,
            read_tokens: 4_000,
            documented_cause: documented.map(str::to_string),
            message: "KV cache miss: ~46K tokens resent (an earlier message was modified).".into(),
        }
    }

    #[test]
    fn only_undocumented_harness_misses_are_alarming() {
        let alarm = CacheMissNotice::from_event(&miss(true, None)).unwrap();
        assert!(alarm.alarming());
        let documented = CacheMissNotice::from_event(&miss(true, Some("config reload"))).unwrap();
        assert!(!documented.alarming());
        let expired = CacheMissNotice::from_event(&miss(false, None)).unwrap();
        assert!(!expired.alarming());
    }

    #[gpui::test]
    fn kv_cache_miss_event_appends_visible_transcript_notice(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("cache", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            panel.history_loaded = true;
            panel.apply(
                &ApiEvent::TextDelta {
                    session_id: "cache".into(),
                    text: "partial answer".into(),
                    message_id: None,
                },
                cx,
            );
            panel.apply(&miss(true, None), cx);
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert!(
                matches!(panel.items.last(), Some(Item::CacheMiss(notice)) if notice.alarming())
            );
            assert!(
                panel
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::Assistant(text) if text == "partial answer")),
                "streamed text must settle before the notice"
            );
        });
        assert!(
            vcx.debug_bounds("kv-cache-miss-notice").is_some(),
            "notice must paint in the transcript"
        );
    }
}
