//! Per-response accounting, separate from latest-request context occupancy.
use super::*;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseStats {
    pub duration_secs: Option<f64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub tool_calls: Option<u64>,
}

impl From<jcode_sdk::ResponseStats> for ResponseStats {
    fn from(stats: jcode_sdk::ResponseStats) -> Self {
        Self {
            duration_secs: stats.duration_secs,
            input_tokens: stats.input_tokens,
            output_tokens: stats.output_tokens,
            cache_read_tokens: stats.cache_read_tokens,
            cache_creation_tokens: stats.cache_creation_tokens,
            tool_calls: None,
        }
    }
}

pub(super) fn fixture() -> ResponseStats {
    ResponseStats {
        duration_secs: Some(84.),
        input_tokens: Some(24_600),
        output_tokens: Some(1_800),
        cache_read_tokens: Some(18_200),
        cache_creation_tokens: None,
        tool_calls: Some(5),
    }
}

#[derive(Default)]
pub(super) struct Tracker {
    started: Option<Instant>,
    active: bool,
    stats: ResponseStats,
    tools: HashSet<String>,
}

impl Tracker {
    pub(super) fn observe(&mut self, event: &ApiEvent, _provider: Option<&str>) {
        match event {
            ApiEvent::MessageAccepted { .. } => {
                self.started.get_or_insert_with(Instant::now);
            }
            ApiEvent::SessionStatus { status, .. }
                if matches!(
                    status.as_str(),
                    "processing" | "running" | "busy" | "generating" | "thinking" | "running_tools"
                ) =>
            {
                self.started.get_or_insert_with(Instant::now);
            }
            ApiEvent::TextDelta { text, .. } | ApiEvent::ReasoningDelta { text, .. } => {
                self.active |= !text.is_empty();
            }
            ApiEvent::ToolStart { call_id, .. } => {
                self.active = true;
                if self.tools.insert(call_id.clone()) {
                    *self.stats.tool_calls.get_or_insert(0) += 1;
                }
            }
            // The harness emits one usage event per completed model request.
            // Sum tool rounds, never session totals or latest context occupancy.
            ApiEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                cache_creation_input,
                ..
            } => {
                self.active = true;
                add(&mut self.stats.input_tokens, *input);
                add(&mut self.stats.output_tokens, *output);
                if let Some(read) = cache_read_input {
                    add(&mut self.stats.cache_read_tokens, *read);
                }
                if let Some(created) = cache_creation_input {
                    add(&mut self.stats.cache_creation_tokens, *created);
                }
            }
            _ => {}
        }
    }

    pub(super) fn finish(&mut self) -> Option<ResponseStats> {
        let mut finished = std::mem::take(self);
        if !finished.active {
            return None;
        }
        finished.stats.duration_secs = finished.started.map(|at| at.elapsed().as_secs_f64());
        (!finished.stats.labels().is_empty()).then_some(finished.stats)
    }
}

fn add(total: &mut Option<u64>, value: u64) {
    *total = Some(total.unwrap_or_default().saturating_add(value));
}

fn duration_label(seconds: f64) -> Option<String> {
    if !seconds.is_finite() || seconds < 0. {
        return None;
    }
    let seconds = seconds.round() as u64;
    Some(if seconds == 0 {
        "<1s".into()
    } else if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    })
}

fn tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.)
    } else {
        value.to_string()
    }
}

struct StatsTooltip(String);
impl Render for StatsTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .max_w(px(380.))
            .rounded_md()
            .bg(Theme::global().HEADER_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_size(px(11.))
            .text_color(Theme::global().TEXT_DIM)
            .child(self.0.clone())
    }
}

impl ResponseStats {
    pub(super) fn is_empty(&self) -> bool {
        self.labels().is_empty()
    }
    fn labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        if let Some(duration) = self.duration_secs.and_then(duration_label) {
            labels.push(format!("Worked {duration}"));
        }
        if let Some(input) = self.input_tokens {
            labels.push(format!("↑ {} in", tokens(input)));
        }
        if let Some(output) = self.output_tokens {
            labels.push(format!("↓ {} out", tokens(output)));
        }
        if let Some(count) = self.tool_calls.filter(|count| *count > 0) {
            labels.push(format!(
                "{count} {}",
                if count == 1 { "tool" } else { "tools" }
            ));
        }
        labels
    }

    fn detail(&self) -> String {
        let mut details = vec!["Provider-reported totals for this response across model requests. Input includes cache for OpenAI, but excludes cache reads/writes for Anthropic. Elapsed time includes tool work observed by Desktop.".to_string()];
        if let Some(input) = self.input_tokens {
            details.push(format!("Input: {input} tokens."));
        }
        if let Some(output) = self.output_tokens {
            details.push(format!("Output: {output} tokens."));
        }
        if let Some(read) = self.cache_read_tokens {
            details.push(format!("Cache read: {read} tokens."));
        }
        if let Some(created) = self.cache_creation_tokens {
            details.push(format!("Cache write: {created} tokens."));
        }
        if self.input_tokens.is_none() {
            details.push("Token usage was not reported.".into());
        }
        details.join(" ")
    }

    pub(super) fn render(&self, index: usize) -> gpui::Stateful<gpui::Div> {
        let detail = self.detail();
        div()
            .id(("response-stats", index))
            .debug_selector(|| "response-stats".into())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .gap_y_1()
            .px_1()
            .py_1()
            .font_family(Theme::global().FONT_MONO)
            .text_size(px(10.))
            .text_color(Theme::global().TEXT_DIM)
            .tooltip(move |_, cx| cx.new(|_| StatsTooltip(detail.clone())).into())
            .children(self.labels().into_iter().enumerate().map(|(index, label)| {
                div().flex_none().child(if index == 0 {
                    label
                } else {
                    format!("· {label}")
                })
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cache: Option<u64>) -> ApiEvent {
        ApiEvent::TokenUsage {
            session_id: "stats".into(),
            input,
            output,
            cache_read_input: cache,
            cache_creation_input: None,
        }
    }

    #[gpui::test]
    fn response_stats_appear_only_after_completion_and_paint_below_answer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("stats", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |w, _| w.test_panel(0).unwrap());
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            panel.apply(
                &ApiEvent::SessionStatus {
                    session_id: "stats".into(),
                    status: "processing".into(),
                },
                cx,
            );
            panel.response_stats.started = Some(Instant::now() - Duration::from_secs(84));
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: "stats".into(),
                    text: "Completed answer.".into(),
                },
                cx,
            );
            panel.apply(&usage(24_600, 1_800, Some(18_200)), cx);
            assert!(
                !panel
                    .items
                    .iter()
                    .any(|item| matches!(item, Item::ResponseStats(_)))
            );
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("response-stats").is_none());
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TurnDone {
                    session_id: "stats".into(),
                },
                cx,
            );
            panel.apply(
                &ApiEvent::TurnDone {
                    session_id: "stats".into(),
                },
                cx,
            );
            panel.apply(
                &ApiEvent::SessionStatus {
                    session_id: "stats".into(),
                    status: "idle".into(),
                },
                cx,
            );
            assert!(
                matches!(&panel.items[0], Item::Assistant(text) if text == "Completed answer.")
            );
            assert_eq!(
                panel.items.len(),
                2,
                "duplicate completion must not duplicate the footer"
            );
            let Item::ResponseStats(stats) = &panel.items[1] else {
                panic!("missing footer")
            };
            assert_eq!(
                stats.labels(),
                ["Worked 1m 24s", "↑ 24.6k in", "↓ 1.8k out"]
            );
        });
        vcx.run_until_parked();
        let answer = vcx.debug_bounds("assistant-response").unwrap();
        let footer = vcx.debug_bounds("response-stats").unwrap();
        assert!(footer.size.height > px(0.));
        assert!(
            footer.top() >= answer.bottom(),
            "footer must follow the response"
        );
    }

    #[test]
    fn response_stats_count_unique_tools_and_keep_missing_tokens_unknown() {
        let mut tracker = Tracker::default();
        for call_id in ["one", "one", "two"] {
            tracker.observe(
                &ApiEvent::ToolStart {
                    session_id: "stats".into(),
                    call_id: call_id.into(),
                    name: "read".into(),
                },
                None,
            );
        }
        let stats = tracker.finish().unwrap();
        assert_eq!(stats.labels(), ["2 tools"]);
        assert_eq!(stats.input_tokens, None);
        assert!(stats.detail().contains("not reported"));
        assert!(tracker.finish().is_none());
    }

    #[gpui::test]
    fn response_stats_history_and_reconnect_keep_one_authoritative_footer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("stats-history", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |w, _| w.test_panel(0).unwrap());
        let history = || {
            vec![
                jcode_sdk::HistoryMessage {
                    role: "user".into(),
                    content: "Question".into(),
                    response_stats: None,
                },
                jcode_sdk::HistoryMessage {
                    role: "assistant".into(),
                    content: "Answer".into(),
                    response_stats: Some(jcode_sdk::ResponseStats {
                        input_tokens: Some(12_000),
                        output_tokens: Some(500),
                        ..Default::default()
                    }),
                },
            ]
        };
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            panel.history_loaded = false;
            panel.load_history(history(), vec![], cx);
            assert_eq!(panel.items.len(), 3);
            let Item::ResponseStats(stats) = &panel.items[2] else {
                panic!("missing saved stats")
            };
            assert_eq!(stats.labels(), ["↑ 12.0k in", "↓ 500 out"]);
            for _ in 0..2 {
                panel.load_history(history(), vec![], cx);
                panel.apply(
                    &ApiEvent::SessionStatus {
                        session_id: "stats-history".into(),
                        status: "idle".into(),
                    },
                    cx,
                );
            }
            assert_eq!(
                panel.items.len(),
                3,
                "history refresh must not append duplicate stats"
            );
            panel.items.truncate(1);
            panel.streaming_text = "Ans".into();
            panel.response_stats.observe(&usage(100, 10, None), None);
            panel.response_stats.started = Some(Instant::now() - Duration::from_secs(84));
            panel.load_history(history(), vec![], cx);
            assert_eq!(
                panel.items.len(),
                1,
                "history stats are not a completion signal"
            );
            panel.apply(
                &ApiEvent::SessionStatus {
                    session_id: "stats-history".into(),
                    status: "idle".into(),
                },
                cx,
            );
            assert_eq!(panel.items.len(), 3);
            let Item::ResponseStats(stats) = &panel.items[2] else {
                panic!("missing recovered stats")
            };
            assert_eq!(
                stats.input_tokens,
                Some(12_000),
                "authoritative history replaces partial telemetry"
            );
            assert_eq!(stats.output_tokens, Some(500));
            assert_eq!(stats.labels()[0], "Worked 1m 24s");
        });
    }

    #[test]
    fn response_stats_sum_rounds_and_reset_without_double_counting_cache() {
        let mut tracker = Tracker::default();
        tracker.started = Some(Instant::now() - Duration::from_secs(84));
        tracker.observe(&usage(12_000, 500, Some(8_000)), Some("openai"));
        tracker.observe(&usage(15_000, 900, Some(10_000)), Some("openai"));
        let stats = tracker.finish().unwrap();
        assert_eq!(stats.input_tokens, Some(27_000));
        assert_eq!(stats.output_tokens, Some(1_400));
        assert_eq!(stats.cache_read_tokens, Some(18_000));
        assert_eq!(stats.labels()[0], "Worked 1m 24s");
        assert!(tracker.finish().is_none());
        tracker.observe(&usage(100, 20, None), Some("openai"));
        assert_eq!(tracker.finish().unwrap().input_tokens, Some(100));
    }

    #[test]
    fn response_stats_preserve_provider_counters_and_unknowns() {
        let mut tracker = Tracker::default();
        tracker.observe(&usage(2_000, 400, Some(8_000)), Some("anthropic"));
        assert_eq!(tracker.finish().unwrap().input_tokens, Some(2_000));
        tracker.observe(
            &ApiEvent::TextDelta {
                message_id: None,
                session_id: "stats".into(),
                text: "Hi".into(),
            },
            None,
        );
        assert!(tracker.finish().is_none());
    }

    #[test]
    fn response_stats_format_short_long_and_invalid_durations() {
        assert_eq!(duration_label(0.1).as_deref(), Some("<1s"));
        assert_eq!(duration_label(59.8).as_deref(), Some("1m 0s"));
        assert_eq!(duration_label(3724.).as_deref(), Some("1h 2m"));
        assert_eq!(duration_label(f64::NAN), None);
        assert_eq!(duration_label(-1.), None);
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(12_400), "12.4k");
        assert_eq!(tokens(1_500_000), "1.5m");
    }
}
