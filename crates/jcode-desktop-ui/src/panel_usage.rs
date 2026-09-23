//! Compact, truthful context and credential quota meters for the session footer.
use super::*;
use crate::accounts::{Account, UsageLimit};

#[derive(Default)]
pub(crate) struct StatusAccounts(pub Vec<Account>);
impl gpui::Global for StatusAccounts {}

pub(crate) struct MeterTooltip(pub(crate) String);
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

fn usage_color(used: f32, theme: &Theme) -> gpui::Rgba {
    if used >= 90. {
        theme.ERROR
    } else if used >= 70. {
        theme.WARN
    } else {
        theme.ACCENT
    }
}

/// Filled annulus segment from 12 o'clock, clockwise, `fraction` of a turn.
fn ring_path(
    center: gpui::Point<gpui::Pixels>,
    outer: f32,
    inner: f32,
    fraction: f32,
) -> Option<gpui::Path<gpui::Pixels>> {
    let fraction = fraction.clamp(0., 1.);
    if fraction <= 0. {
        return None;
    }
    let at = |r: f32, t: f32| {
        let angle = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        center + gpui::point(px(r * angle.cos()), px(r * angle.sin()))
    };
    let mut builder = gpui::PathBuilder::fill();
    if fraction >= 0.999 {
        // Two half arcs per circle: a single full-turn arc is degenerate.
        builder.move_to(at(outer, 0.));
        builder.arc_to(gpui::point(px(outer), px(outer)), px(0.), false, true, at(outer, 0.5));
        builder.arc_to(gpui::point(px(outer), px(outer)), px(0.), false, true, at(outer, 0.));
        builder.close();
        builder.move_to(at(inner, 0.));
        builder.arc_to(gpui::point(px(inner), px(inner)), px(0.), false, false, at(inner, 0.5));
        builder.arc_to(gpui::point(px(inner), px(inner)), px(0.), false, false, at(inner, 0.));
        builder.close();
    } else {
        let large = fraction > 0.5;
        builder.move_to(at(outer, 0.));
        builder.arc_to(gpui::point(px(outer), px(outer)), px(0.), large, true, at(outer, fraction));
        builder.line_to(at(inner, fraction));
        builder.arc_to(gpui::point(px(inner), px(inner)), px(0.), large, false, at(inner, 0.));
        builder.close();
    }
    builder.build().ok()
}

/// Circular context gauge: a faint full track with a colored used arc.
fn context_ring(used: Option<f32>) -> gpui::AnyElement {
    const SIZE: f32 = 13.;
    let theme = Theme::global();
    let used = used.filter(|p| p.is_finite()).map(|p| p.clamp(0., 100.));
    div()
        .debug_selector(|| "panel-context-ring".into())
        .flex_none()
        .size(px(SIZE))
        .child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let center = bounds.center();
                    let outer = SIZE / 2.;
                    let inner = outer - 2.6;
                    if let Some(track) = ring_path(center, outer, inner, 1.) {
                        window.paint_path(track, theme.INLINE_CODE_BG);
                    }
                    if let Some(used) = used
                        && let Some(arc) = ring_path(center, outer, inner, used / 100.)
                    {
                        window.paint_path(arc, usage_color(used, &theme));
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// USD with cents above a dollar and more precision for small sessions.
fn format_cost(usd: f64) -> String {
    if usd >= 1. {
        format!("${usd:.2}")
    } else if usd >= 0.01 {
        format!("${usd:.3}")
    } else if usd > 0. {
        "<$0.01".into()
    } else {
        "$0.00".into()
    }
}

/// Activity-ledger source key for a per-token (API key) route, or `None` for
/// subscription routes that show quota limits instead.
fn metered_source_key(provider: Option<&str>, auth: Option<&str>) -> Option<String> {
    let provider = provider?.trim().to_ascii_lowercase();
    let api_key = auth == Some("api key");
    Some(match provider.as_str() {
        "anthropic" | "claude" if api_key => "claude:api-key".into(),
        "anthropic-api" | "claude-api" => "claude:api-key".into(),
        "openai" if api_key => "openai:api-key".into(),
        "openai-api" => "openai:api-key".into(),
        "gemini" if api_key => "gemini".into(),
        "gemini-api" => "gemini".into(),
        "openrouter" | "bedrock" | "azure" | "azure-openai" => provider,
        other => {
            let id = other.strip_prefix("openai-compatible:").unwrap_or(other);
            if !api_key && !other.starts_with("openai-compatible:") {
                return None;
            }
            if jcode_base::model_pricing::models_dev_provider_id(id).is_some() {
                id.to_string()
            } else {
                format!("openai-compatible:{id}")
            }
        }
    })
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
    /// Estimated session spend on a per-token route, summed from completed
    /// response stats (live and restored). `None` on subscription routes or
    /// when the model cannot be priced.
    pub(super) fn session_api_cost(&self) -> Option<(f64, usize)> {
        let source = metered_source_key(self.provider.as_deref(), self.auth_method.as_deref())?;
        let model = self.model.as_deref()?;
        // Probe once so unpriced routes fall back to limits, not "$0.00".
        jcode_base::provider::pricing::metered_usage_cost_usd(&source, model, 0, 0, 0, 0)?;
        let mut total = 0.;
        let mut turns = 0;
        for item in &self.items {
            let Item::ResponseStats(stats) = item else {
                continue;
            };
            let cost = jcode_base::provider::pricing::metered_usage_cost_usd(
                &source,
                model,
                stats.input_tokens.unwrap_or(0),
                stats.output_tokens.unwrap_or(0),
                stats.cache_read_tokens.unwrap_or(0),
                stats.cache_creation_tokens.unwrap_or(0),
            )?;
            total += cost;
            turns += 1;
        }
        Some((total, turns))
    }

    pub(super) fn render_usage_meters(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if self.model.is_none() && self.provider.is_none() {
            return None;
        }
        let theme = Theme::global();
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
            Some(percent) => format!("{:.0}%", percent.min(100.)),
            None => "—".into(),
        };
        let detail = context_usage_label(self.model.as_deref(), self.context_tokens)
            .map(|label| format!("Context window: {label}. Model capacity is an estimate. Usage reflects the latest reported request."))
            .unwrap_or_else(|| "Context usage is not reported yet.".into());
        row = row.child(
            div()
                .id("panel-context-meter")
                .debug_selector(|| "panel-context-meter".into())
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .h(px(22.))
                .text_color(theme.TEXT_DIM)
                .tooltip(move |_, cx| cx.new(|_| MeterTooltip(detail.clone())).into())
                .child(context_ring(percent))
                .child(label),
        );
        if let Some((cost, turns)) = self.session_api_cost() {
            let detail = format!(
                "{}: estimated API spend for this session, priced from {turns} reported response{} at list rates. Not a bill.",
                account_method_label(self.provider.as_deref(), self.auth_method.as_deref()),
                if turns == 1 { "" } else { "s" },
            );
            row = row.child(
                div()
                    .id("panel-api-cost")
                    .debug_selector(|| "panel-api-cost".into())
                    .flex_none()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .text_color(theme.TEXT_DIM)
                    .tooltip(move |_, cx| cx.new(|_| MeterTooltip(detail.clone())).into())
                    .child(format_cost(cost)),
            );
            return Some(row);
        }
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
                        row.size.height >= px(22.) && row.size.height <= px(90.),
                        "width={width}, status={status}: {row:?}"
                    );
                    for selector in [
                        "panel-build",
                        "panel-status",
                        "panel-usage",
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
                // No catalog (static or cached) prices this id.
                panel.model = Some("unpriced-desktop-test-model".into());
                cx.notify();
            });
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("panel-limit-0").is_none());
        // Unpriced API routes stay honest instead of claiming $0.
        assert!(vcx.debug_bounds("panel-limits-unavailable").is_some());
        assert!(vcx.debug_bounds("panel-api-cost").is_none());
        // Priced API routes show session spend instead of quota limits.
        workspace.update(vcx, |workspace, cx| {
            workspace.test_panel(0).unwrap().update(cx, |panel, cx| {
                panel.provider = Some("anthropic".into());
                panel.model = Some("claude-sonnet-4-5".into());
                panel.items = vec![Item::ResponseStats(response_stats::ResponseStats {
                    input_tokens: Some(1_000_000),
                    output_tokens: Some(100_000),
                    ..Default::default()
                })];
                assert_eq!(panel.session_api_cost(), Some((4.5, 1)));
                cx.notify();
            });
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("panel-api-cost").is_some());
        assert!(vcx.debug_bounds("panel-limits-unavailable").is_none());
        assert!(vcx.debug_bounds("panel-context-ring").is_some());
    }

    #[test]
    fn metered_source_keys_only_for_per_token_routes() {
        assert_eq!(metered_source_key(Some("anthropic"), Some("api key")).as_deref(), Some("claude:api-key"));
        assert_eq!(metered_source_key(Some("openai"), Some("api key")).as_deref(), Some("openai:api-key"));
        assert_eq!(metered_source_key(Some("openrouter"), None).as_deref(), Some("openrouter"));
        assert_eq!(metered_source_key(Some("anthropic"), Some("oauth")), None);
        assert_eq!(metered_source_key(Some("openai"), Some("oauth")), None);
        assert_eq!(metered_source_key(Some("copilot"), None), None);
        assert_eq!(metered_source_key(None, Some("api key")), None);
    }

    #[test]
    fn cost_labels_and_ring_paths_are_bounded() {
        assert_eq!(format_cost(0.), "$0.00");
        assert_eq!(format_cost(0.004), "<$0.01");
        assert_eq!(format_cost(0.123), "$0.123");
        assert_eq!(format_cost(12.345), "$12.35");
        let center = gpui::point(px(10.), px(10.));
        assert!(ring_path(center, 6., 3., 0.).is_none());
        for fraction in [0.01, 0.25, 0.5, 0.75, 0.999, 1.0, 2.0] {
            assert!(ring_path(center, 6., 3., fraction).is_some(), "{fraction}");
        }
    }
}
