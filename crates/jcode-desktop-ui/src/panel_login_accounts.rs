//! Account rows for the Accounts picker: one pill per login, with its name,
//! limits and recorded usage, grouped by auto-switch membership. Rows drag
//! between the Auto-switch and Manual groups and reorder within Auto-switch.
//! Only labels, masked emails and usage figures are shown, never credentials.
use super::connection::{ConnectionStatus, ConnectionStatuses, status_for};
use super::*;
use crate::accounts::{Account, UsageLimit, UsageReport};
use jcode_base::auth::account_pool::{AccountPool, OAuthLogin, account_key};
use std::rc::Rc;

/// Providers that store several named OAuth logins.
const MULTI_ACCOUNT: [&str; 2] = ["openai", "claude"];
/// Asking for an unknown label makes the CLI allocate the next free one.
const NEW_ACCOUNT_LABEL: &str = "new";

#[derive(Default)]
pub(super) struct AccountsData {
    pub accounts: Option<Vec<Account>>,
    pub logins: Vec<OAuthLogin>,
    pub pool: AccountPool,
}

#[derive(Clone, Debug)]
pub(super) struct AccountRow {
    pub key: String,
    pub provider: LoginProvider,
    pub label: Option<String>,
    pub email: Option<String>,
    pub active: bool,
    pub status: ConnectionStatus,
    pub limits: Vec<UsageLimit>,
    pub usage: Option<String>,
    pub plan: Option<String>,
    /// The auth report says a credential is stored, whatever its health.
    pub configured: bool,
    /// First row of its provider keeps the stable `login-provider-{id}` selector.
    pub first_of_provider: bool,
}

impl AccountRow {
    fn connected(&self) -> bool {
        self.label.is_some()
            || self.configured
            || matches!(
                self.status,
                ConnectionStatus::Connected
                    | ConnectionStatus::Unverified
                    | ConnectionStatus::Expired
                    | ConnectionStatus::Failed
            )
    }

    /// Subscriptions rotate by default. Metered API keys never do silently.
    fn default_pooled(&self) -> bool {
        self.provider.method != LoginMethod::ApiKey
    }

    fn subtitle(&self) -> Option<String> {
        let parts: Vec<&str> = [self.label.as_deref(), self.email.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

#[derive(Clone)]
pub(super) struct DraggedAccount {
    key: String,
    title: String,
}

struct AccountDragPreview(String);

impl Render for AccountDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global();
        div()
            .px_3()
            .py_1p5()
            .rounded_full()
            .bg(theme.ACCENT_DIM)
            .text_size(px(13.))
            .text_color(theme.TEXT)
            .child(self.0.clone())
    }
}

pub(super) struct Groups {
    pub pooled: Vec<AccountRow>,
    pub manual: Vec<AccountRow>,
    pub available: Vec<AccountRow>,
    /// Connected multi-account providers that can take another login.
    pub addable: Vec<LoginProvider>,
}

fn report_for<'a>(
    account: &'a Account,
    label: Option<&str>,
    logins: usize,
) -> Option<&'a UsageReport> {
    let reports = &account.usage_reports;
    if let Some(label) = label {
        let exact = reports.iter().find(|report| {
            report.account_label.as_deref() == Some(label) || report.provider_name.contains(label)
        });
        if exact.is_some() {
            return exact;
        }
        return (logins == 1 && reports.len() == 1).then(|| &reports[0]);
    }
    (reports.len() == 1).then(|| &reports[0])
}

/// First `$1.23` amount in a usage line, trimmed to cents.
fn dollars(text: &str) -> Option<String> {
    let start = text.find('$')?;
    let amount: String = text[start + 1..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f64 = amount.parse().ok()?;
    Some(format!("${value:.2}"))
}

fn usage_summary(report: &UsageReport) -> (Option<String>, Option<String>) {
    let value = |key: &str| {
        report
            .extra_info
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let plan = value("Plan").map(str::to_owned);
    if let Some(spend) = value("Local spend (this machine)") {
        return (Some(spend.to_owned()), plan);
    }
    let period = |key: &str| {
        value(key).map(|text| {
            let amount = if text.starts_with("No recorded usage") {
                "$0.00".to_owned()
            } else {
                dollars(text).unwrap_or_else(|| "n/a".into())
            };
            format!("{} {amount}", key.to_lowercase())
        })
    };
    let parts: Vec<String> = [period("Today"), period("Lifetime")]
        .into_iter()
        .flatten()
        .collect();
    let usage = (!parts.is_empty()).then(|| format!("{} (est.)", parts.join(" · ")));
    (usage, plan)
}

pub(super) fn build_rows(
    providers: &[LoginProvider],
    statuses: Option<&ConnectionStatuses>,
    loading: bool,
    data: &AccountsData,
) -> Vec<AccountRow> {
    let mut rows = Vec::new();
    for provider in providers {
        let mut status = status_for(statuses, provider.id, loading);
        let account = data
            .accounts
            .as_ref()
            .and_then(|accounts| accounts.iter().find(|a| a.id == provider.id));
        // Auth status saw a credential the health report did not check.
        if status == ConnectionStatus::NotConnected
            && account.is_some_and(|a| a.status == "available")
        {
            status = ConnectionStatus::Unverified;
        }
        let logins: Vec<&OAuthLogin> = if MULTI_ACCOUNT.contains(&provider.id) {
            data.logins
                .iter()
                .filter(|login| login.provider == provider.id)
                .collect()
        } else {
            Vec::new()
        };
        let row = |label: Option<&str>, email: Option<String>, active: bool, first: bool| {
            let report = account.and_then(|a| report_for(a, label, logins.len()));
            let limits = match (report, account) {
                (Some(report), _) => report.limits.clone(),
                (None, Some(account)) if label.is_none() => account.limits.clone(),
                _ => Vec::new(),
            };
            let (usage, plan) = report.map(usage_summary).unwrap_or_default();
            AccountRow {
                key: account_key(provider.id, label),
                provider: *provider,
                label: label.map(str::to_owned),
                email,
                active,
                status,
                limits,
                usage,
                plan,
                configured: account.is_some_and(|a| a.status != "not_configured"),
                first_of_provider: first,
            }
        };
        if logins.is_empty() {
            rows.push(row(None, None, false, true));
        } else {
            for (index, login) in logins.iter().enumerate() {
                rows.push(row(
                    Some(&login.label),
                    login.email.as_deref().map(mask_email),
                    login.active && logins.len() > 1,
                    index == 0,
                ));
            }
        }
    }
    rows
}

pub(super) fn group_rows(rows: Vec<AccountRow>, pool: &AccountPool) -> Groups {
    let (connected, available): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(AccountRow::connected);
    let keys: Vec<(String, bool)> = connected
        .iter()
        .map(|row| (row.key.clone(), row.default_pooled()))
        .collect();
    let (member_keys, manual_keys) = pool.partition(&keys);
    let take = |keys: &[&str]| -> Vec<AccountRow> {
        keys.iter()
            .filter_map(|key| connected.iter().find(|row| row.key == *key).cloned())
            .collect()
    };
    let mut addable = Vec::new();
    for row in &connected {
        if MULTI_ACCOUNT.contains(&row.provider.id)
            && !addable
                .iter()
                .any(|p: &LoginProvider| p.id == row.provider.id)
        {
            addable.push(row.provider);
        }
    }
    Groups {
        pooled: take(&member_keys),
        manual: take(&manual_keys),
        available,
        addable,
    }
}

fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return email.to_owned();
    };
    let mut chars = local.chars();
    match (chars.next(), chars.last()) {
        (Some(first), Some(last)) => format!("{first}***{last}@{domain}"),
        (Some(first), None) => format!("{first}***@{domain}"),
        _ => email.to_owned(),
    }
}

/// Offline data for previews, screenshots and tests. Never touches credentials.
pub(super) fn offline_data() -> AccountsData {
    let limit = |name: &str, used: f32, reset: &str| UsageLimit {
        name: name.into(),
        usage_percent: used,
        reset_in: Some(reset.into()),
    };
    let report = |label: &str, limits: Vec<UsageLimit>, today: &str| {
        UsageReport {
        provider_name: format!("OpenAI (ChatGPT) ({label})"),
        account_label: Some(label.into()),
        limits,
        banked_reset: None,
        extra_info: vec![
            ("Plan".into(), "plus".into()),
            ("Today".into(), format!("{today} API-equivalent estimate, not a bill")),
            ("Lifetime".into(), "2400000 input / 160000 output tokens, $84.1000 API-equivalent estimate, not a bill".into()),
        ],
    }
    };
    let account = |id: &str, name: &str, reports: Vec<UsageReport>| Account {
        id: id.into(),
        display_name: name.into(),
        status: "available".into(),
        auth_kind: "OAuth".into(),
        method: "Offline fixture".into(),
        limits: reports.iter().flat_map(|r| r.limits.clone()).collect(),
        usage_reports: reports,
    };
    let login = |provider, label: &str, email: &str, active| OAuthLogin {
        provider,
        label: label.into(),
        email: Some(email.into()),
        active,
    };
    AccountsData {
        accounts: Some(vec![
            account(
                "openai",
                "OpenAI",
                vec![
                    report(
                        "openai-otter",
                        vec![
                            limit("5-hour window", 92., "41m"),
                            limit("7-day window", 66., "3d 20h"),
                        ],
                        "120000 input tokens, $4.2000",
                    ),
                    report(
                        "openai-fox",
                        vec![
                            limit("5-hour window", 8., "4h 2m"),
                            limit("7-day window", 21., "5d"),
                        ],
                        "No recorded usage",
                    ),
                ],
            ),
            account(
                "claude",
                "Anthropic/Claude",
                vec![UsageReport {
                    provider_name: "Anthropic (Claude) (claude-otter)".into(),
                    account_label: None,
                    limits: vec![
                        limit("5-hour window", 35., "2h"),
                        limit("7-day window", 48., "4d"),
                    ],
                    banked_reset: None,
                    extra_info: vec![("Plan".into(), "max".into())],
                }],
            ),
            Account {
                id: "openai-api".into(),
                display_name: "OpenAI API".into(),
                status: "available".into(),
                auth_kind: "API key".into(),
                method: "Offline fixture".into(),
                limits: Vec::new(),
                usage_reports: vec![UsageReport {
                    provider_name: "OpenAI API key".into(),
                    account_label: None,
                    limits: Vec::new(),
                    banked_reset: None,
                    extra_info: vec![(
                        "Local spend (this machine)".into(),
                        "$3.10 today · $41.20 this month".into(),
                    )],
                }],
            },
        ]),
        logins: vec![
            login("openai", "openai-otter", "jeremy@personal.dev", true),
            login("openai", "openai-fox", "jeremy@work.dev", false),
            login("claude", "claude-otter", "jeremy@personal.dev", true),
        ],
        pool: AccountPool::default(),
    }
}

fn short_limit_name(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.starts_with("5-hour") || lower.starts_with("5 hour") {
        "5h".into()
    } else if lower.starts_with("7-day") || lower.starts_with("7 day") || lower.contains("week") {
        "7d".into()
    } else {
        name.split_whitespace().next().unwrap_or(name).to_owned()
    }
}

fn limit_meter(limit: &UsageLimit) -> gpui::AnyElement {
    let theme = Theme::global();
    let used = limit.usage_percent.clamp(0., 100.);
    let color = if used >= 90. {
        theme.ERROR
    } else if used >= 70. {
        theme.WARN
    } else {
        theme.ACCENT
    };
    let caption = match &limit.reset_in {
        Some(reset) => format!("{} {used:.0}% · {reset}", short_limit_name(&limit.name)),
        None => format!("{} {used:.0}%", short_limit_name(&limit.name)),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .w(px(118.))
        .flex_none()
        .child(
            div()
                .text_size(px(10.))
                .text_color(theme.TEXT_DIM)
                .whitespace_nowrap()
                .overflow_hidden()
                .child(caption),
        )
        .child(
            div()
                .w_full()
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
                ),
        )
        .into_any_element()
}

fn chip(label: impl Into<SharedString>, color: gpui::Rgba) -> gpui::Div {
    chip_base(color).child(label.into())
}

fn status_chip(label: &'static str, color: gpui::Rgba) -> gpui::Div {
    chip_base(color)
        .child(div().size(px(6.)).rounded_full().bg(color))
        .child(label)
}

fn chip_base(color: gpui::Rgba) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2p5()
        .py(px(3.))
        .rounded_full()
        .bg(color.opacity(0.12))
        .text_size(px(11.))
        .text_color(color)
        .whitespace_nowrap()
}

impl Panel {
    fn place_account(
        &mut self,
        key: &str,
        pooled: bool,
        before: Option<&str>,
        visible: &[String],
        cx: &mut Context<Self>,
    ) {
        let offline =
            self.preview_state.is_some() || cfg!(test) || crate::harness::screenshot_mode();
        let Some(state) = self.login.as_mut() else {
            return;
        };
        let remaining: Vec<&str> = visible
            .iter()
            .map(String::as_str)
            .filter(|member| *member != key)
            .collect();
        let index = before
            .and_then(|target| remaining.iter().position(|member| *member == target))
            .unwrap_or(remaining.len());
        let visible: Vec<&str> = visible.iter().map(String::as_str).collect();
        state.accounts.pool.place(key, pooled, index, &visible);
        if !offline {
            let pool = state.accounts.pool.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = pool.save() {
                        eprintln!("[accounts] could not save auto-switch order: {error}");
                    }
                })
                .detach();
        }
        cx.notify();
    }

    fn render_account_row(
        &self,
        row: &AccountRow,
        pooled: Option<bool>,
        visible: &Rc<Vec<String>>,
        position: Option<usize>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let provider = row.provider;
        let status = row.status;
        let color = status.color();
        let key = row.key.clone();
        let row_id = format!("login-account-{key}");
        let provider_selector = if row.first_of_provider {
            format!("login-provider-{}", provider.id)
        } else {
            format!(
                "login-provider-{}-{}",
                provider.id,
                row.label.as_deref().unwrap_or("")
            )
        };
        let status_selector = if row.first_of_provider {
            format!("login-status-{}", provider.id)
        } else {
            format!("login-status-{key}")
        };
        let title = provider.display_name.to_owned();
        let mut name_line = div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(theme.TEXT)
                    .whitespace_nowrap()
                    .child(title.clone()),
            )
            .child(login_method_icon(provider.method));
        if row.active {
            name_line = name_line.child(chip("In use", theme.ACCENT));
        }
        if let Some(plan) = &row.plan {
            name_line = name_line.child(chip(plan.clone(), theme.TEXT_DIM));
        }
        let detail: Vec<String> = [row.subtitle(), row.usage.clone()]
            .into_iter()
            .flatten()
            .collect();
        let mut text = div()
            .flex_1()
            .min_w(px(140.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(name_line);
        if !detail.is_empty() {
            text = text.child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.TEXT_DIM)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(detail.join(" · ")),
            );
        }
        let mut meters = div().flex().items_center().gap_3().flex_none();
        for limit in row.limits.iter().take(2) {
            meters = meters.child(limit_meter(limit));
        }
        let mut element = div()
            .id(SharedString::from(row_id.clone()))
            .debug_selector(move || provider_selector.clone())
            .flex()
            .items_center()
            .gap_3()
            .pl_2()
            .pr_2()
            .py_1p5()
            .min_h(px(52.))
            .rounded_full()
            .bg(theme.HEADER_BG)
            .cursor_pointer()
            .hover(|el| el.bg(theme.ACCENT_DIM))
            .child(login_logo(&provider))
            .child(text)
            .child(meters)
            .child(
                status_chip(status.label(), color).debug_selector(move || status_selector.clone()),
            );
        if let Some(position) = position {
            element = element.child(
                div()
                    .flex_none()
                    .w(px(22.))
                    .text_size(px(11.))
                    .text_color(theme.TEXT_DIM)
                    .child(format!("{}", position + 1)),
            );
        }
        let label = row.label.clone();
        element = element.on_click(cx.listener(move |this, _, _, cx| {
            this.select_login_provider_for(provider, label.clone(), cx)
        }));
        if let Some(pooled) = pooled {
            let before = key.clone();
            let visible = visible.clone();
            element = element
                .on_drag(
                    DraggedAccount {
                        key: key.clone(),
                        title: match &row.label {
                            Some(label) => format!("{title} · {label}"),
                            None => title,
                        },
                    },
                    |dragged, _, _, cx| cx.new(|_| AccountDragPreview(dragged.title.clone())),
                )
                .drag_over::<DraggedAccount>(|style, _, _, _| style.bg(Theme::global().ACCENT_DIM))
                .on_drop(cx.listener(move |this, dragged: &DraggedAccount, _, cx| {
                    this.place_account(&dragged.key, pooled, Some(&before), &visible, cx)
                }));
        }
        element.into_any_element()
    }

    fn render_account_group(
        &self,
        id: &'static str,
        title: &'static str,
        hint: &'static str,
        rows: &[AccountRow],
        pooled: bool,
        visible: &Rc<Vec<String>>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let mut group = div()
            .id(id)
            .debug_selector(move || id.into())
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_xl()
            .drag_over::<DraggedAccount>(|style, _, _, _| {
                style.bg(Theme::global().ACCENT_DIM.opacity(0.35))
            })
            .on_drop({
                let visible = visible.clone();
                cx.listener(move |this, dragged: &DraggedAccount, _, cx| {
                    this.place_account(&dragged.key, pooled, None, &visible, cx)
                })
            })
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .px_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.TEXT)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.TEXT_DIM)
                            .child(hint),
                    ),
            );
        if rows.is_empty() {
            group = group.child(
                div()
                    .px_3()
                    .py_3()
                    .rounded_full()
                    .bg(theme.HEADER_BG.opacity(0.5))
                    .text_size(px(12.))
                    .text_color(theme.TEXT_DIM)
                    .child("Drag an account here"),
            );
        }
        for (index, row) in rows.iter().enumerate() {
            group = group.child(self.render_account_row(
                row,
                Some(pooled),
                visible,
                pooled.then_some(index),
                cx,
            ));
        }
        group.into_any_element()
    }

    pub(super) fn render_account_catalog(
        &self,
        providers: &[LoginProvider],
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let Some(state) = self.login.as_ref() else {
            return Vec::new();
        };
        let theme = Theme::global();
        let rows = build_rows(
            providers,
            state.statuses.as_ref(),
            state.status_loading,
            &state.accounts,
        );
        let groups = group_rows(rows, &state.accounts.pool);
        let visible = Rc::new(
            groups
                .pooled
                .iter()
                .map(|row| row.key.clone())
                .collect::<Vec<_>>(),
        );
        let mut out = Vec::new();
        if !groups.pooled.is_empty() || !groups.manual.is_empty() {
            out.push(self.render_account_group(
                "login-group-auto",
                "Auto-switch",
                "When an account runs out, Jcode moves to the next one from the same provider, in this order",
                &groups.pooled,
                true,
                &visible,
                cx,
            ));
            out.push(self.render_account_group(
                "login-group-manual",
                "Manual",
                "Used only when you pick them",
                &groups.manual,
                false,
                &visible,
                cx,
            ));
        }
        if !groups.available.is_empty() || !groups.addable.is_empty() {
            let mut section = div()
                .debug_selector(|| "login-group-available".into())
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .child(
                    div()
                        .px_2()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Connect an account"),
                );
            if !groups.addable.is_empty() {
                let mut adds = div().flex().flex_wrap().gap_2().px_1();
                for provider in groups.addable {
                    let id = format!("login-add-{}", provider.id);
                    adds = adds.child(
                        div()
                            .id(SharedString::from(id.clone()))
                            .debug_selector(move || id.clone())
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1p5()
                            .rounded_full()
                            .bg(theme.ACCENT.opacity(0.16))
                            .text_color(theme.ACCENT)
                            .text_size(px(12.))
                            .cursor_pointer()
                            .hover(|el| el.bg(theme.ACCENT.opacity(0.26)))
                            .child(format!("+ Another {} account", provider.display_name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_login_provider_for(
                                    provider,
                                    Some(NEW_ACCOUNT_LABEL.into()),
                                    cx,
                                )
                            })),
                    );
                }
                section = section.child(adds);
            }
            for row in &groups.available {
                section = section.child(self.render_account_row(row, None, &visible, None, cx));
            }
            out.push(section.into_any_element());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &'static str, method: LoginMethod) -> LoginProvider {
        LoginProvider {
            id,
            display_name: id,
            detail: "",
            method,
        }
    }

    #[test]
    fn logins_become_rows_and_defaults_split_subscriptions_from_keys() {
        let providers = [
            provider("openai", LoginMethod::OAuth),
            provider("openai-api", LoginMethod::ApiKey),
            provider("claude", LoginMethod::OAuth),
            provider("cursor", LoginMethod::ApiKey),
        ];
        let statuses = ConnectionStatuses::from([
            ("openai".into(), ConnectionStatus::Connected),
            ("openai-api".into(), ConnectionStatus::Unverified),
            ("claude".into(), ConnectionStatus::Expired),
        ]);
        let data = offline_data();
        let rows = build_rows(&providers, Some(&statuses), false, &data);
        let keys: Vec<_> = rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "openai:openai-otter",
                "openai:openai-fox",
                "openai-api",
                "claude:claude-otter",
                "cursor"
            ]
        );
        let otter = &rows[0];
        assert!(otter.active);
        assert_eq!(otter.email.as_deref(), Some("j***y@personal.dev"));
        assert_eq!(otter.limits.len(), 2);
        assert_eq!(otter.limits[0].usage_percent, 92.);
        assert_eq!(
            otter.usage.as_deref(),
            Some("today $4.20 · lifetime $84.10 (est.)")
        );
        assert_eq!(otter.plan.as_deref(), Some("plus"));
        assert_eq!(
            rows[1].usage.as_deref(),
            Some("today $0.00 · lifetime $84.10 (est.)")
        );
        assert_eq!(
            rows[2].usage.as_deref(),
            Some("$3.10 today · $41.20 this month")
        );
        // One Claude login: its single report is attributed to it.
        assert_eq!(rows[3].limits.len(), 2);

        let groups = group_rows(rows, &AccountPool::default());
        let pooled: Vec<_> = groups.pooled.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            pooled,
            [
                "openai:openai-otter",
                "openai:openai-fox",
                "claude:claude-otter"
            ]
        );
        assert_eq!(groups.manual[0].key, "openai-api");
        assert_eq!(groups.available[0].key, "cursor");
        let addable: Vec<_> = groups.addable.iter().map(|p| p.id).collect();
        assert_eq!(addable, ["openai", "claude"]);
    }

    #[gpui::test]
    fn dragging_rows_moves_accounts_between_groups_and_reorders(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new_accounts("pool-test", None, crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("login-group-auto").is_some());
        assert!(vcx.debug_bounds("login-group-manual").is_some());
        assert!(vcx.debug_bounds("login-add-openai").is_some());
        let pooled = |vcx: &mut gpui::VisualTestContext| {
            panel.read_with(vcx, |panel, _| {
                let state = panel.login.as_ref().unwrap();
                let providers = super::super::catalog::filtered_providers(
                    &state.providers,
                    "",
                    state.statuses.as_ref(),
                    state.status_loading,
                    state.usage.as_ref(),
                );
                let rows = build_rows(
                    &providers,
                    state.statuses.as_ref(),
                    state.status_loading,
                    &state.accounts,
                );
                group_rows(rows, &state.accounts.pool)
                    .pooled
                    .into_iter()
                    .map(|row| row.key)
                    .collect::<Vec<_>>()
            })
        };
        let before = pooled(vcx);
        assert!(before.contains(&"openai:openai-otter".to_string()));
        assert!(!before.contains(&"openai-api".to_string()));

        let drag = |vcx: &mut gpui::VisualTestContext, from: &'static str, to: &'static str| {
            let from = vcx.debug_bounds(from).unwrap().center();
            let to = vcx.debug_bounds(to).unwrap().center();
            let left = gpui::MouseButton::Left;
            vcx.simulate_mouse_down(from, left, gpui::Modifiers::default());
            vcx.simulate_mouse_move(
                from + point(px(0.), px(12.)),
                left,
                gpui::Modifiers::default(),
            );
            vcx.simulate_mouse_move(to, left, gpui::Modifiers::default());
            vcx.simulate_mouse_up(to, left, gpui::Modifiers::default());
            vcx.run_until_parked();
        };
        // API key (manual) dropped onto the first auto-switch row goes first.
        drag(vcx, "login-provider-openai-api", "login-provider-openai");
        let after = pooled(vcx);
        assert_eq!(after[0], "openai-api");
        assert_eq!(after[1..], before[..]);
        // Reorder: the second OpenAI login moves ahead of the first.
        drag(
            vcx,
            "login-provider-openai-openai-fox",
            "login-provider-openai-api",
        );
        assert_eq!(pooled(vcx)[0], "openai:openai-fox");
        // Out of rotation: drop on the Manual group.
        drag(
            vcx,
            "login-provider-openai-openai-fox",
            "login-group-manual",
        );
        assert!(!pooled(vcx).contains(&"openai:openai-fox".to_string()));
    }

    #[test]
    fn usage_figures_are_trimmed_to_cents() {
        assert_eq!(dollars("x, $2693.5357 known"), Some("$2693.54".into()));
        assert_eq!(dollars("no amount"), None);
        assert_eq!(short_limit_name("5-hour window"), "5h");
        assert_eq!(short_limit_name("7-day window"), "7d");
        assert_eq!(mask_email("a@b.c"), "a***@b.c");
    }
}
