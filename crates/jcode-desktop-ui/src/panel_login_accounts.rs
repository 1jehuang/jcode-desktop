//! Account rows for the Accounts picker: one pill per login, with its name,
//! limits and recorded usage, grouped by auto-switch membership. Rows drag
//! between the Auto-switch and Manual groups and reorder within Auto-switch.
//! Only labels, masked emails and usage figures are shown, never credentials.
use super::connection::{ConnectionStatus, ConnectionStatuses, status_for};
use super::*;
use crate::accounts::{Account, BankedReset, ResetProvider, UsageLimit, UsageReport};
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
    /// Banked usage resets this login holds (OpenAI) or a Claude session reset.
    pub banked: Option<BankedReset>,
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
                    | ConnectionStatus::Testing
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
    testing: &dyn Fn(&str) -> bool,
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
        if status != ConnectionStatus::NotConnected && testing(provider.id) {
            status = ConnectionStatus::Testing;
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
            let banked = report
                .and_then(|r| r.banked_reset.clone())
                .filter(|reset| reset.available_count > 0 || reset.next_available_at.is_some());
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
                banked,
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
    let banked = |provider, label: &str, count, limit_reached, next: Option<&str>| {
        // "soon" means about 3h from now, so screenshots read realistically.
        let next = next.map(|next| match next {
            "soon" => {
                let at =
                    std::time::SystemTime::now() + std::time::Duration::from_secs(3 * 3600 + 120);
                httpdate_rfc3339(at)
            }
            other => other.to_owned(),
        });
        Some(BankedReset {
            provider,
            account_label: Some(label.into()),
            available_count: count,
            limit_reached,
            next_available_at: next,
        })
    };
    let report = |label: &str, limits: Vec<UsageLimit>, today: &str| {
        UsageReport {
        provider_name: format!("OpenAI (ChatGPT) ({label})"),
        account_label: Some(label.into()),
        limits,
        banked_reset: match label {
            "openai-otter" => banked(ResetProvider::OpenAi, label, 2, true, None),
            _ => banked(ResetProvider::OpenAi, label, 1, false, None),
        },
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
                    banked_reset: banked(
                        ResetProvider::Claude,
                        "claude-otter",
                        0,
                        false,
                        Some("soon"),
                    ),
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

/// UTC RFC3339 for a wall-clock time (offline fixtures only).
fn httpdate_rfc3339(at: std::time::SystemTime) -> String {
    let secs = at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
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
        .flex_1()
        .min_w(px(72.))
        .max_w(px(160.))
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

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim().to_owned()
}

fn chip(label: impl Into<SharedString>, color: gpui::Rgba) -> gpui::Div {
    chip_base(color).child(label.into())
}

fn icon_reset(color: gpui::Rgba) -> gpui::Div {
    div().text_size(px(11.)).text_color(color).child("↻")
}

/// Caption for a login's banked resets, e.g. "2 resets banked".
pub(super) fn banked_reset_label(reset: &BankedReset) -> String {
    match (reset.provider, reset.available_count) {
        (ResetProvider::OpenAi, 1) => "1 reset banked".into(),
        (ResetProvider::OpenAi, n) if n > 1 => format!("{n} resets banked"),
        (ResetProvider::Claude, n) if n > 0 => "Session reset ready".into(),
        _ => match reset.next_available_at.as_deref() {
            Some(at) => format!("Next reset in {}", jcode_base::usage::format_reset_time(at)),
            None => "No resets".into(),
        },
    }
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

impl super::LoginState {
    /// Rows for `providers`, with any in-flight live test shown as Testing.
    pub(super) fn account_rows(&self, providers: &[LoginProvider]) -> Vec<AccountRow> {
        let testing = |id: &str| self.live_test_all || self.live_testing.contains(id);
        build_rows(
            providers,
            self.statuses.as_ref(),
            self.status_loading,
            &testing,
            &self.accounts,
        )
    }
}

impl Panel {
    /// Banked resets on a login. Clicking only opens the workspace review,
    /// which prepares against the provider and needs an explicit confirm.
    fn render_banked_reset(
        &self,
        key: &str,
        reset: &BankedReset,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let id = format!("login-reset-{key}");
        let selector = id.clone();
        let label = banked_reset_label(reset);
        let offerable = reset.offerable();
        let color = if offerable {
            theme.ACCENT
        } else {
            theme.TEXT_DIM
        };
        let mut pill = chip_base(color)
            .id(SharedString::from(id))
            .debug_selector(move || selector.clone())
            .child(icon_reset(color))
            .child(label)
            // Informational pills must not fall through to the row's sign-in.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if offerable {
            let reset = reset.clone();
            pill = pill
                .cursor_pointer()
                .hover(move |el| el.bg(color.opacity(0.24)))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(AccountsPanelRedeemReset(reset.clone()));
                }));
        } else {
            pill = pill.on_click(|_, _, cx| cx.stop_propagation());
        }
        pill.into_any_element()
    }

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
        // The first auto-switch account is the default for new sessions.
        let first = state
            .accounts
            .pool
            .members
            .first()
            .filter(|first| visible.first() != Some(&first.as_str()))
            .cloned();
        if !offline {
            let pool = state.accounts.pool.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = pool.save() {
                        eprintln!("[accounts] could not save auto-switch order: {error}");
                    }
                    if let Some(first) = first
                        && let Err(error) =
                            jcode_base::auth::account_pool::apply_default_account(&first)
                    {
                        eprintln!("[accounts] could not make {first} the default: {error}");
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
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .gap_y_1()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .text_size(px(14.))
                    .text_color(theme.TEXT)
                    .whitespace_nowrap()
                    .child(title.clone()),
            )
            .child(div().flex_none().child(login_method_icon(provider.method)));
        if row.active {
            name_line = name_line.child(chip("In use", theme.ACCENT));
        }
        if position == Some(0) {
            name_line = name_line.child(
                chip("Default", theme.ACCENT).debug_selector(|| "login-default-account".into()),
            );
        }
        if let Some(plan) = &row.plan {
            name_line = name_line.child(chip(plan.clone(), theme.TEXT_DIM));
        }
        if let Some(reset) = &row.banked {
            name_line = name_line.child(self.render_banked_reset(&row.key, reset, cx));
        }
        let detail: Vec<String> = [row.subtitle(), row.usage.clone()]
            .into_iter()
            .flatten()
            .collect();
        let mut text = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
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
        let connected = pooled.is_some();
        let live_error = self
            .login
            .as_ref()
            .and_then(|state| state.live_errors.get(provider.id))
            .filter(|_| {
                status == ConnectionStatus::Failed || status == ConnectionStatus::Unverified
            });
        if let Some(error) = live_error {
            text = text.child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.ERROR)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(first_line(error)),
            );
        }
        // Meters sit on their own line inside the text column. Beside the
        // name they overlapped the plan and reset chips on narrow panels.
        if !row.limits.is_empty() {
            let mut meters = div().flex().items_center().gap_4().w_full().pt(px(2.));
            for limit in row.limits.iter().take(2) {
                meters = meters.child(limit_meter(limit));
            }
            text = text.child(meters);
        }
        let mut element = div()
            .id(SharedString::from(row_id.clone()))
            .debug_selector(move || provider_selector.clone())
            .flex()
            .items_center()
            .gap_3()
            .pl(px(10.))
            .pr(px(10.))
            .py_1p5()
            .min_h(px(if connected { 56. } else { 44. }))
            .rounded_full()
            .bg(if connected {
                theme.HEADER_BG
            } else {
                theme.HEADER_BG.opacity(0.55)
            })
            .cursor_pointer()
            .hover(|el| el.bg(theme.ACCENT_DIM));
        // Auto-switch order reads left to right, before the provider.
        if let Some(position) = position {
            element = element.child(
                div()
                    .flex_none()
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme.ACCENT.opacity(0.16))
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.ACCENT)
                    .child(format!("{}", position + 1)),
            );
        } else if connected {
            // Manual rows keep the same logo column as ordered rows.
            element = element.child(div().flex_none().size(px(20.)));
        }
        element = element
            .child(
                div()
                    .flex_none()
                    .size(px(if connected { 34. } else { 28. }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme.PANEL_BG)
                    .child(login_logo(&provider)),
            )
            .child(text);
        if connected || status != ConnectionStatus::NotConnected {
            let tip = status.detail();
            element = element.child(
                status_chip(status.label(), color)
                    .id(SharedString::from(format!("login-status-chip-{key}")))
                    .debug_selector(move || status_selector.clone())
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::usage::MeterTooltip(tip.into())).into()
                    }),
            );
        } else {
            element = element.child(
                chip_base(theme.ACCENT)
                    .debug_selector(move || status_selector.clone())
                    .child("Connect"),
            );
        }
        if connected {
            let test_id = format!("login-test-{key}");
            let selector = test_id.clone();
            let provider_id = provider.id.to_owned();
            let busy = status == ConnectionStatus::Testing;
            element = element.child(
                div()
                    .id(SharedString::from(test_id))
                    .debug_selector(move || selector.clone())
                    .flex_none()
                    .px_2p5()
                    .py(px(3.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.PANEL_BORDER)
                    .text_size(px(11.))
                    .text_color(theme.TEXT_DIM)
                    .whitespace_nowrap()
                    .when(!busy, |el| {
                        el.cursor_pointer()
                            .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
                    })
                    .when(busy, |el| el.opacity(0.5))
                    .child("Test")
                    .tooltip(|_, cx| {
                        cx.new(|_| {
                            super::usage::MeterTooltip(
                                "Send one small live request through this account to confirm it works. Uses a little quota or credit.".into(),
                            )
                        })
                        .into()
                    })
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.live_test_providers(Some(provider_id.clone()), cx)
                    })),
            );
        }
        if let Some(pooled) = pooled {
            let toggle_id = format!("login-move-{key}");
            let selector = toggle_id.clone();
            let move_key = key.clone();
            let visible = visible.clone();
            element = element.child(
                div()
                    .id(SharedString::from(toggle_id))
                    .debug_selector(move || selector.clone())
                    .flex_none()
                    .w(px(96.))
                    .flex()
                    .justify_center()
                    .py(px(3.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.PANEL_BORDER)
                    .text_size(px(11.))
                    .text_color(theme.TEXT_DIM)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
                    .child(if pooled { "Make manual" } else { "Auto-switch" })
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.place_account(&move_key, !pooled, None, &visible, cx)
                    })),
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
                    .flex_col()
                    .gap(px(2.))
                    .px_2()
                    .pb_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.TEXT)
                                    .child(title),
                            )
                            .child(chip(rows.len().to_string(), theme.TEXT_DIM)),
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
        let rows = state.account_rows(providers);
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
                "The first account is the default for new sessions. The rest take over in order when one runs out or fails. Drag to reorder.",
                &groups.pooled,
                true,
                &visible,
                cx,
            ));
            out.push(self.render_account_group(
                "login-group-manual",
                "Manual",
                "Never switched to automatically. Used only when you pick it.",
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
        let rows = build_rows(&providers, Some(&statuses), false, &|_| false, &data);
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
                let rows = state.account_rows(&providers);
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
        // The row pill moves an account back without dragging.
        let pill = vcx
            .debug_bounds("login-move-openai:openai-fox")
            .unwrap()
            .center();
        vcx.simulate_click(pill, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(pooled(vcx).last().unwrap(), "openai:openai-fox");
        assert!(panel.read_with(vcx, |panel, _| {
            panel.login.as_ref().unwrap().provider.is_none()
        }));
        let pill = vcx
            .debug_bounds("login-move-openai:openai-fox")
            .unwrap()
            .center();
        vcx.simulate_click(pill, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(!pooled(vcx).contains(&"openai:openai-fox".to_string()));
    }

    #[gpui::test]
    fn live_test_controls_mark_rows_testing_without_opening_sign_in(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new_accounts("live-test", None, crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("login-test-all").is_some());
        assert!(vcx.debug_bounds("login-default-account").is_some());
        let test = vcx
            .debug_bounds("login-test-openai:openai-otter")
            .expect("connected rows offer a Test pill")
            .center();
        // Clicking Test marks the provider testing and never opens sign-in.
        panel.update(vcx, |panel, cx| {
            panel.live_test_providers(Some("openai".into()), cx);
            let state = panel.login.as_ref().unwrap();
            let rows = state.account_rows(&state.providers);
            assert!(
                rows.iter()
                    .filter(|row| row.provider.id == "openai")
                    .all(|row| row.status == ConnectionStatus::Testing)
            );
        });
        vcx.run_until_parked();
        vcx.simulate_click(test, gpui::Modifiers::default());
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            let state = panel.login.as_ref().unwrap();
            assert!(state.provider.is_none());
            assert!(state.live_testing.is_empty());
        });
    }

    #[gpui::test]
    fn banked_resets_show_per_login_and_click_only_requests_review(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new_accounts("reset-test", None, crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        let requested = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        vcx.update(|_, cx| {
            let requested = requested.clone();
            cx.subscribe(&panel, move |_, event: &AccountsPanelRedeemReset, _| {
                requested.borrow_mut().push(event.0.clone());
            })
            .detach();
        });
        assert!(vcx.debug_bounds("login-reset-openai:openai-fox").is_some());
        assert!(
            vcx.debug_bounds("login-reset-claude:claude-otter")
                .is_some()
        );
        // Not offerable yet (limit not binding): showing it must not redeem.
        let fox = vcx
            .debug_bounds("login-reset-openai:openai-fox")
            .unwrap()
            .center();
        vcx.simulate_click(fox, gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(requested.borrow().is_empty());
        let otter = vcx
            .debug_bounds("login-reset-openai:openai-otter")
            .unwrap()
            .center();
        vcx.simulate_click(otter, gpui::Modifiers::default());
        vcx.run_until_parked();
        let requested = requested.borrow();
        assert_eq!(requested.len(), 1);
        assert_eq!(requested[0].account_label.as_deref(), Some("openai-otter"));
        // The pill does not also open the provider's sign-in flow.
        assert!(panel.read_with(vcx, |panel, _| {
            panel.login.as_ref().unwrap().provider.is_none()
        }));
    }

    #[test]
    fn banked_reset_labels() {
        let reset = |provider, count, next: Option<&str>| BankedReset {
            provider,
            account_label: None,
            available_count: count,
            limit_reached: false,
            next_available_at: next.map(str::to_owned),
        };
        assert_eq!(
            banked_reset_label(&reset(ResetProvider::OpenAi, 1, None)),
            "1 reset banked"
        );
        assert_eq!(
            banked_reset_label(&reset(ResetProvider::OpenAi, 3, None)),
            "3 resets banked"
        );
        assert_eq!(
            banked_reset_label(&reset(ResetProvider::Claude, 1, None)),
            "Session reset ready"
        );
        assert!(
            banked_reset_label(&reset(
                ResetProvider::Claude,
                0,
                Some("2099-01-01T00:00:00Z")
            ))
            .starts_with("Next reset in ")
        );
    }

    #[test]
    fn usage_figures_are_trimmed_to_cents() {
        assert_eq!(dollars("x, $2693.5357 known"), Some("$2693.54".into()));
        assert_eq!(dollars("no amount"), None);
        assert_eq!(short_limit_name("5-hour window"), "5h");
        assert_eq!(short_limit_name("7-day window"), "7d");
        assert_eq!(mask_email("a@b.c"), "a***@b.c");
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_790_468_836);
        assert_eq!(httpdate_rfc3339(at), "2026-09-27T00:27:16Z");
    }
}
