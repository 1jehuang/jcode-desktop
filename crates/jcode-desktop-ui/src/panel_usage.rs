//! Compact, truthful context and credential quota meters for the session footer.
use super::*;
use crate::accounts::{Account, UsageLimit};

#[derive(Default)]
pub(crate) struct StatusAccounts(pub Vec<Account>);
impl gpui::Global for StatusAccounts {}

struct MeterTooltip(String);
impl Render for MeterTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global();
        div()
            .px_3()
            .py_2()
            .max_w(px(380.))
            .rounded_md()
            .bg(theme.HEADER_BG)
            .border_1()
            .border_color(theme.PANEL_BORDER)
            .text_size(px(11.))
            .text_color(theme.TEXT_DIM)
            .child(self.0.clone())
    }
}

fn meter(
    id: String,
    label: String,
    percent: Option<f32>,
    detail: String,
) -> gpui::Stateful<gpui::Div> {
    let theme = Theme::global();
    let used = percent.filter(|p| p.is_finite()).map(|p| p.clamp(0., 100.));
    let selector = id.clone();
    let (name, value) = label.rsplit_once(' ').unwrap_or((&label, ""));
    let name = name.to_owned();
    let value = value.to_owned();
    div()
        .id(SharedString::from(id))
        .debug_selector(move || selector.clone())
        .min_w_0()
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(22.))
        .text_color(theme.TEXT_DIM)
        .tooltip(move |_, cx| cx.new(|_| MeterTooltip(detail.clone())).into())
        .child(div().min_w_0().truncate().child(name))
        .child(div().flex_none().child(value))
        .children(used.map(|used| {
            let color = if used >= 90. {
                theme.ERROR
            } else if used >= 70. {
                theme.WARN
            } else {
                theme.ACCENT
            };
            div()
                .flex_none()
                .w(px(24.))
                .h(px(4.))
                .rounded_full()
                .overflow_hidden()
                .bg(theme.INLINE_CODE_BG)
                .child(
                    div()
                        .h_full()
                        .w(relative(used / 100.))
                        .rounded_full()
                        .bg(color),
                )
        }))
}

/// Never substitute another credential's limits (e.g. ChatGPT for an API key).
fn active_limits<'a>(
    accounts: &'a [Account],
    provider: Option<&str>,
    auth: Option<&str>,
) -> Option<&'a [UsageLimit]> {
    let provider = provider?;
    // Runtime identity arrives asynchronously. Ambiguous auth is not OAuth.
    if auth.is_none() && matches!(provider, "openai" | "anthropic" | "gemini") {
        return None;
    }
    let id = crate::accounts::credential_id(provider, auth);
    accounts
        .iter()
        .find(|account| account.id == id)
        .and_then(Account::active_limits)
}

impl Panel {
    pub(super) fn render_usage_meters(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if self.model.is_none() && self.provider.is_none() {
            return None;
        }
        let mut row = div()
            .debug_selector(|| "panel-usage".into())
            .flex()
            .min_w_0()
            .items_center()
            .gap_2()
            .flex_nowrap()
            .overflow_hidden();
        let window = self.model.as_deref().and_then(context_window_for_model);
        let used = self.context_tokens;
        let percent = used
            .zip(window)
            .map(|(used, window)| (used as f64 / window as f64 * 100.) as f32);
        let label = match percent {
            Some(percent) => format!("Context {:.0}%", percent.min(100.)),
            None => "Context —".into(),
        };
        let detail = context_usage_label(self.model.as_deref(), self.context_tokens)
            .map(|label| format!("Context window: {label}. Model capacity is an estimate. Usage reflects the latest reported request."))
            .unwrap_or_else(|| "Context usage is not reported yet.".into());
        row = row.child(meter("panel-context-meter".into(), label, percent, detail));
        // Local account snapshots cannot describe credentials on a remote host.
        let limits = if crate::harness::remote_host(&self.session_id).is_none() {
            cx.try_global::<StatusAccounts>().and_then(|snapshot| {
                active_limits(
                    &snapshot.0,
                    self.provider.as_deref(),
                    self.auth_method.as_deref(),
                )
            })
        } else {
            None
        };
        if let Some(limits) = limits.filter(|limits| !limits.is_empty()) {
            for (index, limit) in limits.iter().enumerate() {
                let percent = limit
                    .usage_percent
                    .is_finite()
                    .then_some(limit.usage_percent);
                let label = percent
                    .map(|p| format!("{} {:.0}%", limit.name, p.clamp(0., 100.)))
                    .unwrap_or_else(|| format!("{} —", limit.name));
                let reset = limit
                    .reset_in
                    .as_deref()
                    .map(|reset| format!(" Resets in {reset}."))
                    .unwrap_or_default();
                let detail = format!(
                    "{}: {label} used.{reset}",
                    account_method_label(self.provider.as_deref(), self.auth_method.as_deref())
                );
                row = row.child(meter(
                    format!("panel-limit-{index}"),
                    label,
                    percent,
                    detail,
                ));
            }
        } else {
            row = row.child(meter("panel-limits-unavailable".into(), "Limits —".into(), None,
                "Usage limits are unavailable for the current connection method. Open Accounts for details.".into()));
        }
        Some(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn context_meter_uses_prompt_occupancy_not_billing_totals(cx: &mut gpui::TestAppContext) {
        let (_, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("usage-session", cx);
            workspace.test_panel(0).unwrap().update(cx, |panel, cx| {
                for (provider, input, read, write, expected) in [
                    ("openai", 100_000, Some(28_000), None, 100_000),
                    ("anthropic", 10_000, Some(80_000), Some(5_000), 95_000),
                    ("openai", 5_000, None, None, 5_000),
                ] {
                    panel.provider = Some(provider.into());
                    panel.apply(
                        &ApiEvent::TokenUsage {
                            session_id: "usage-session".into(),
                            input,
                            output: 8_000,
                            cache_read_input: read,
                            cache_creation_input: write,
                        },
                        cx,
                    );
                    assert_eq!(panel.context_tokens, Some(expected));
                }
            });
            workspace
        });
        vcx.run_until_parked();
    }

    #[test]
    fn active_limits_match_credentials_not_model_family() {
        let mut accounts = crate::accounts::parse(r#"{"providers":[{"id":"openai","status":"available"},{"id":"openai-api","status":"available"},{"id":"openrouter","status":"available"}]}"#).unwrap();
        accounts[0].limits.push(UsageLimit {
            name: "5 hour".into(),
            usage_percent: 25.,
            reset_in: Some("2h".into()),
        });
        assert_eq!(
            active_limits(&accounts, Some("openai"), Some("oauth"))
                .unwrap()
                .len(),
            1
        );
        assert!(
            active_limits(&accounts, Some("openai"), Some("api key"))
                .unwrap()
                .is_empty()
        );
        assert!(
            active_limits(&accounts, Some("openrouter"), Some("api key"))
                .unwrap()
                .is_empty()
        );
        assert!(active_limits(&accounts, Some("openai"), None).is_none());
        assert!(active_limits(&accounts, None, None).is_none());
    }

    #[gpui::test]
    fn status_rows_wrap_with_bounded_height_and_visible_controls(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("fixed-status-row", cx);
            workspace
        });
        let panel = workspace.read_with(vcx, |workspace, _| workspace.test_panel(0).unwrap());
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [1440., 640., 400., 320.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            for populated in [false, true] {
                for status in [
                    "idle",
                    "running_tools",
                    "disconnected with a long status message",
                ] {
                    panel.update(vcx, |panel, cx| {
                        panel.model = populated.then(|| "a-very-long-model-name".repeat(4));
                        panel.provider = populated.then(|| "openai".into());
                        panel.auth_method = populated.then(|| "oauth".into());
                        panel.working_dir = Some("/a/very/long/working/directory".repeat(4));
                        panel.reasoning_effort = Some("high".into());
                        panel.status = status.into();
                        cx.notify();
                    });
                    vcx.run_until_parked();
                    let row = vcx.debug_bounds("panel-meta").expect("status row");
                    // Three bounded groups may wrap, but long metadata/status
                    // text must never create unbounded footer height.
                    assert!(
                        row.size.height >= px(30.) && row.size.height <= px(90.),
                        "width={width}, status={status}: {row:?}"
                    );
                    for selector in [
                        "panel-identity",
                        "panel-build",
                        "panel-status",
                        "panel-usage",
                        "voice-toggle",
                        "voice-shortcut",
                    ] {
                        if let Some(child) = vcx.debug_bounds(selector) {
                            assert!(child.top() >= row.top(), "{selector}: {child:?}");
                            assert!(child.bottom() <= row.bottom(), "{selector}: {child:?}");
                        }
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn footer_meters_paint_and_follow_credential_switches(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut accounts = crate::accounts::parse(r#"{"providers":[{"id":"openai","status":"available"},{"id":"openai-api","status":"available"}]}"#).unwrap();
            accounts[0].limits = vec![
                UsageLimit { name: "5 hour".into(), usage_percent: 25., reset_in: Some("2h".into()) },
                UsageLimit { name: "Weekly".into(), usage_percent: 75., reset_in: Some("3d".into()) },
            ];
            cx.set_global(StatusAccounts(accounts));
        });
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("usage-session", cx);
            let panel = workspace.test_panel(0).unwrap();
            panel.update(cx, |panel, _| {
                panel.provider = Some("openai".into());
                panel.auth_method = Some("oauth".into());
                panel.model = Some("gpt-5.6-sol".into());
                panel.context_tokens = Some(100_000);
            });
            workspace
        });
        vcx.run_until_parked();
        for selector in ["panel-context-meter", "panel-limit-0", "panel-limit-1"] {
            let bounds = vcx.debug_bounds(selector).expect(selector);
            assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        }
        let handle = vcx.update(|window, _| window.window_handle());
        vcx.simulate_window_resize(handle, gpui::size(px(640.), px(480.)));
        vcx.run_until_parked();
        let context = vcx.debug_bounds("panel-context-meter").unwrap();
        assert!(vcx.debug_bounds("panel-model").unwrap().size.width > px(20.));
        for selector in ["panel-limit-0", "panel-limit-1"] {
            let limit = vcx.debug_bounds(selector).unwrap();
            assert_eq!(context.center().y, limit.center().y);
            assert!(limit.right() <= px(640.));
        }
        workspace.update(vcx, |workspace, cx| {
            workspace.test_panel(0).unwrap().update(cx, |panel, cx| {
                panel.auth_method = Some("api key".into());
                cx.notify();
            });
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("panel-limit-0").is_none());
        assert!(vcx.debug_bounds("panel-limits-unavailable").is_some());
    }
}
