//! In-app Jcode account menu, opened from the sidebar account pill.
//!
//! Plan, monthly usage, the spending limit, billing, and sign-out are handled
//! here through the Jcode SDK instead of sending people to the website. Only
//! the Stripe billing portal (payment methods, invoices, cancellation) still
//! opens in the browser, because card entry must happen on Stripe.
use super::*;
use jcode_base::subscription_api::{self as api, AccountApiError, BillingStatus, SubscriptionMe};
use jcode_base::subscription_catalog as subscription;

/// Monthly spending limits offered as one-tap choices, in dollars.
const LIMIT_CHOICES: [u64; 5] = [25, 50, 100, 250, 500];

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Snapshot {
    pub email: String,
    pub plan: Option<&'static str>,
    pub status: String,
    pub used_usd: f64,
    pub billing: Option<BillingStatus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Loading,
    Limit(u64),
    Portal,
    SigningOut,
}

#[derive(Default)]
pub(super) struct State {
    pub open: bool,
    snapshot: Option<Snapshot>,
    busy: Option<Busy>,
    error: Option<String>,
    notice: Option<String>,
    confirm_sign_out: bool,
    task: Option<gpui::Task<()>>,
}

impl State {
    pub(super) fn startup() -> Self {
        let fixture = fixture();
        Self {
            open: fixture.is_some(),
            snapshot: fixture,
            ..Self::default()
        }
    }
}

/// Offline screenshot fixture: `JCODE_DESKTOP_SCREENSHOT_ACCOUNT_MENU=1`.
pub(super) fn fixture_enabled() -> bool {
    harness::screenshot_mode()
        && std::env::var("JCODE_DESKTOP_SCREENSHOT_ACCOUNT_MENU").as_deref() == Ok("1")
}

fn fixture() -> Option<Snapshot> {
    fixture_enabled().then(|| Snapshot {
        email: "ada@example.com".into(),
        plan: Some("Pro"),
        status: "active".into(),
        used_usd: 12.34,
        billing: Some(BillingStatus {
            monthly_hard_cap_cents: 5_000,
            period: api::BillingPeriod {
                resets_at: Some("2026-11-01T00:00:00.000Z".into()),
                billable_microusd: 12_340_000,
                remaining_cap_microusd: 37_660_000,
            },
            promotional_credit: api::BillingCredit {
                original_microusd: 5_000_000,
                remaining_microusd: 2_500_000,
            },
            ..BillingStatus::default()
        }),
    })
}

/// Run one async SDK call to completion off the UI thread.
fn run<T>(operation: impl std::future::Future<Output = T>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(operation))
        .map_err(|_| "Could not start a network request. Please try again.".to_string())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("jcode-desktop/{}", crate::build_info::VERSION))
        .build()
        .map_err(|error| error.to_string())
}

fn credentials() -> Result<(String, String), String> {
    let key = subscription::configured_api_key()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .ok_or_else(|| "You are signed out. Sign in to manage your account.".to_string())?;
    Ok((api::configured_api_base(), key))
}

fn describe(error: AccountApiError) -> String {
    match error {
        AccountApiError::Unauthorized => {
            "This sign-in is no longer valid. Sign out and sign in again.".into()
        }
        AccountApiError::Offline(_) => "Could not reach Jcode. Check your connection.".into(),
        AccountApiError::Http {
            code: Some(code), ..
        } if code == "no_billing_account" => {
            "No billing is set up yet. Choose a plan first.".into()
        }
        AccountApiError::Http {
            code: Some(code), ..
        } if code == "invalid_hard_cap" => "That limit is not allowed.".into(),
        other => other.to_string(),
    }
}

fn load_snapshot() -> Result<Snapshot, String> {
    let (base, key) = credentials()?;
    let client = client()?;
    run(async {
        let me: SubscriptionMe = api::fetch_subscription_me_with(&client, &base, &key)
            .await
            .map_err(describe)?;
        // Billing is optional: older servers and unactivated accounts may not
        // report it, and the rest of the menu is still useful without it.
        let billing = api::fetch_billing_with(&client, &base, &key).await.ok();
        Ok(Snapshot {
            plan: me.parsed_tier().map(subscription::JcodeTier::display_name),
            email: me.email,
            status: me.status,
            used_usd: me.usage.used_usd,
            billing,
        })
    })?
}

/// "Nov 1" from an RFC 3339 timestamp. Falls back to nothing on bad input.
fn short_date(timestamp: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month: usize = timestamp.get(5..7)?.parse().ok()?;
    let day: u32 = timestamp.get(8..10)?.parse().ok()?;
    Some(format!("{} {day}", MONTHS.get(month.checked_sub(1)?)?))
}

fn dollars(amount: f64) -> String {
    if amount >= 100.0 || (amount.fract() == 0.0 && amount >= 1.0) {
        format!("${amount:.0}")
    } else {
        format!("${amount:.2}")
    }
}

impl Snapshot {
    /// Spend this period and the limit it counts against, when known.
    fn spend(&self) -> (f64, Option<f64>) {
        match &self.billing {
            Some(billing) if billing.monthly_hard_cap_cents > 0 => (
                billing.billable_usd().max(self.used_usd),
                Some(billing.monthly_hard_cap_usd()),
            ),
            _ => (self.used_usd, None),
        }
    }

    fn usage_line(&self) -> String {
        match self.spend() {
            (used, Some(limit)) => format!("{} of {} limit", dollars(used), dollars(limit)),
            (used, None) => format!("{} used", dollars(used)),
        }
    }

    fn fraction(&self) -> Option<f32> {
        match self.spend() {
            (used, Some(limit)) if limit > 0.0 => Some((used / limit).clamp(0.0, 1.0) as f32),
            _ => None,
        }
    }

    fn reset_line(&self) -> Option<String> {
        let billing = self.billing.as_ref()?;
        let date = short_date(billing.period.resets_at.as_deref()?)?;
        let credit = billing.promotional_credit_usd();
        Some(if credit > 0.0 {
            format!("Resets {date} · {} credit left", dollars(credit))
        } else {
            format!("Resets {date}")
        })
    }

    fn limit_cents(&self) -> Option<u64> {
        self.billing.as_ref().map(|b| b.monthly_hard_cap_cents)
    }
}

impl Workspace {
    pub(super) fn toggle_account_menu(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.account_menu;
        menu.open = !menu.open;
        menu.confirm_sign_out = false;
        menu.notice = None;
        if menu.open {
            self.refresh_account_menu(cx);
        }
        cx.notify();
    }

    pub(super) fn close_account_menu(&mut self, cx: &mut Context<Self>) {
        if fixture_enabled() {
            return;
        }
        self.account_menu.open = false;
        self.account_menu.confirm_sign_out = false;
        cx.notify();
    }

    fn refresh_account_menu(&mut self, cx: &mut Context<Self>) {
        if fixture_enabled() || cfg!(test) {
            return;
        }
        self.account_menu.busy = Some(Busy::Loading);
        self.account_menu.error = None;
        let load = cx.background_executor().spawn(async { load_snapshot() });
        self.account_menu.task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| {
                let menu = &mut this.account_menu;
                menu.busy = None;
                match result {
                    Ok(snapshot) => menu.snapshot = Some(snapshot),
                    Err(error) => menu.error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    fn set_account_limit(&mut self, dollars_limit: u64, cx: &mut Context<Self>) {
        if self.account_menu.busy.is_some() {
            return;
        }
        if fixture_enabled() {
            if let Some(billing) = self
                .account_menu
                .snapshot
                .as_mut()
                .and_then(|s| s.billing.as_mut())
            {
                billing.monthly_hard_cap_cents = dollars_limit * 100;
            }
            cx.notify();
            return;
        }
        self.account_menu.busy = Some(Busy::Limit(dollars_limit));
        self.account_menu.error = None;
        self.account_menu.notice = None;
        let update = cx.background_executor().spawn(async move {
            let (base, key) = credentials()?;
            let client = client()?;
            run(api::set_monthly_hard_cap_with(
                &client,
                &base,
                &key,
                dollars_limit * 100,
            ))?
            .map_err(describe)
        });
        self.account_menu.task = Some(cx.spawn(async move |this, cx| {
            let result = update.await;
            let _ = this.update(cx, |this, cx| {
                let menu = &mut this.account_menu;
                menu.busy = None;
                match result {
                    Ok(billing) => {
                        if let Some(snapshot) = menu.snapshot.as_mut() {
                            snapshot.billing = Some(billing);
                        }
                        menu.notice = Some(format!("Monthly limit set to ${dollars_limit}"));
                    }
                    Err(error) => menu.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn open_billing_portal(&mut self, cx: &mut Context<Self>) {
        if self.account_menu.busy.is_some() || fixture_enabled() {
            return;
        }
        self.account_menu.busy = Some(Busy::Portal);
        self.account_menu.error = None;
        let portal = cx.background_executor().spawn(async {
            let (base, key) = credentials()?;
            let client = client()?;
            run(api::billing_portal_url_with(&client, &base, &key))?.map_err(describe)
        });
        self.account_menu.task = Some(cx.spawn(async move |this, cx| {
            let result = portal.await;
            let _ = this.update(cx, |this, cx| {
                this.account_menu.busy = None;
                match result {
                    Ok(url) => {
                        cx.open_url(&url);
                        this.account_menu.notice = Some("Billing opened in your browser".into());
                    }
                    Err(error) => this.account_menu.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn sign_out_account(&mut self, cx: &mut Context<Self>) {
        if self.account_menu.busy.is_some() || fixture_enabled() {
            return;
        }
        if !self.account_menu.confirm_sign_out {
            self.account_menu.confirm_sign_out = true;
            cx.notify();
            return;
        }
        self.account_menu.busy = Some(Busy::SigningOut);
        self.account_menu.error = None;
        let sign_out = cx.background_executor().spawn(async {
            let client = client()?;
            run(api::sign_out_current_account(&client))?.map_err(|error| error.to_string())
        });
        self.account_menu.task = Some(cx.spawn(async move |this, cx| {
            let result = sign_out.await;
            let _ = this.update(cx, |this, cx| {
                let menu = &mut this.account_menu;
                menu.busy = None;
                menu.confirm_sign_out = false;
                match result {
                    Ok(outcome) => {
                        menu.open = false;
                        menu.snapshot = None;
                        this.status = match outcome {
                            api::SignOutOutcome::LocalOnly(_) => {
                                "Signed out on this computer. The key could not be revoked online."
                                    .into()
                            }
                            _ => "Signed out of Jcode".into(),
                        };
                        sidebar_account::JcodeAccount::invalidate();
                        accounts::request_refresh();
                    }
                    Err(error) => menu.error = Some(format!("Could not sign out: {error}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn render_account_menu(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let menu = &self.account_menu;
        let show_email = sidebar_account::show_email();
        let account = sidebar_account::JcodeAccount::current();
        let snapshot = menu.snapshot.as_ref();
        let title = snapshot
            .map(|s| s.email.clone())
            .or(account.email.clone())
            .filter(|_| show_email)
            .unwrap_or_else(|| "Jcode account".into());
        let plan = snapshot
            .and_then(|s| s.plan)
            .or(account.plan)
            .map(|plan| format!("{plan} plan"))
            .unwrap_or_else(|| "Usage-based".into());
        let status = snapshot
            .map(|s| s.status.as_str())
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("active"))
            .map(|s| format!(" · {s}"))
            .unwrap_or_default();

        let mut body = div()
            .id("account-menu")
            .debug_selector(|| "account-menu".into())
            .w_full()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(theme.PANEL_BORDER)
            .bg(theme.PANEL_BG)
            .flex()
            .flex_col()
            .gap_3()
            .text_size(px(11.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(|| "account-menu-title".into())
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(theme.TEXT)
                            .child(title),
                    )
                    .child(
                        div()
                            .debug_selector(|| "account-menu-plan".into())
                            .text_color(theme.TEXT_DIM)
                            .child(format!("{plan}{status}")),
                    ),
            );

        // Usage this period.
        body = body.child(match snapshot {
            Some(snapshot) => {
                let mut usage = div()
                    .debug_selector(|| "account-menu-usage".into())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_color(theme.TEXT_DIM).child("This month"))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(theme.TEXT)
                            .child(snapshot.usage_line()),
                    );
                if let Some(fraction) = snapshot.fraction() {
                    let fill = if fraction >= 0.9 {
                        theme.ERROR
                    } else {
                        theme.ACCENT
                    };
                    usage = usage.child(
                        div()
                            .h(px(4.0))
                            .w_full()
                            .rounded_full()
                            .bg(theme.INLINE_CODE_BG)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(fill)
                                    .w(gpui::relative(fraction.max(0.02))),
                            ),
                    );
                }
                if let Some(reset) = snapshot.reset_line() {
                    usage = usage.child(div().text_color(theme.TEXT_DIM).child(reset));
                }
                usage
            }
            None => div()
                .text_color(theme.TEXT_DIM)
                .child(if menu.busy == Some(Busy::Loading) {
                    "Loading account…"
                } else {
                    "Account details unavailable"
                }),
        });

        // Monthly spending limit.
        if let Some(current) = snapshot.and_then(Snapshot::limit_cents) {
            let mut chips = div().flex().flex_wrap().gap_1();
            for amount in LIMIT_CHOICES {
                let selected = current == amount * 100;
                let pending = menu.busy == Some(Busy::Limit(amount));
                chips = chips.child(
                    div()
                        .id(("account-limit", amount as usize))
                        .debug_selector(move || format!("account-limit-{amount}"))
                        .px_2()
                        .py(px(3.0))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if selected {
                            theme.ACCENT
                        } else {
                            theme.INLINE_CODE_BG
                        })
                        .text_color(if selected { theme.BG } else { theme.TEXT_DIM })
                        .when(!selected, |el| {
                            el.hover(|el| el.bg(theme.HEADER_BG).text_color(theme.TEXT))
                        })
                        .when(pending, |el| el.opacity(0.6))
                        .child(format!("${amount}"))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_account_limit(amount, cx)),
                        ),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_color(theme.TEXT_DIM).child("Monthly limit"))
                    .child(chips),
            );
        }

        if let Some(message) = menu.error.clone().or(menu.notice.clone()) {
            let is_error = menu.error.is_some();
            body = body.child(
                div()
                    .debug_selector(|| "account-menu-message".into())
                    .text_color(if is_error {
                        theme.ERROR
                    } else {
                        theme.TEXT_DIM
                    })
                    .child(message),
            );
        }

        let action = |id: &'static str, label: String, danger: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(px(28.0))
                .px_3()
                .flex()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .bg(if danger {
                    theme.ERROR.opacity(0.12)
                } else {
                    theme.INLINE_CODE_BG
                })
                .text_color(if danger { theme.ERROR } else { theme.TEXT })
                .hover(move |el| {
                    el.bg(if danger {
                        theme.ERROR.opacity(0.2)
                    } else {
                        theme.HEADER_BG
                    })
                })
                .child(label)
        };
        let busy = menu.busy;
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    action(
                        "account-menu-billing",
                        if busy == Some(Busy::Portal) {
                            "Opening billing…".into()
                        } else {
                            "Payment & invoices ↗".into()
                        },
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.open_billing_portal(cx))),
                )
                .child(
                    action(
                        "account-menu-refresh",
                        if busy == Some(Busy::Loading) {
                            "Refreshing…".into()
                        } else {
                            "Refresh".into()
                        },
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_account_menu(cx))),
                )
                .child(
                    action(
                        "account-menu-sign-out",
                        match (busy, menu.confirm_sign_out) {
                            (Some(Busy::SigningOut), _) => "Signing out…".into(),
                            (_, true) => "Confirm sign out".into(),
                            _ => "Sign out".into(),
                        },
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.sign_out_account(cx))),
                ),
        );

        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(cap_cents: u64, billable_microusd: u64) -> Snapshot {
        Snapshot {
            email: "ada@example.com".into(),
            plan: Some("Pro"),
            status: "active".into(),
            used_usd: 0.0,
            billing: Some(BillingStatus {
                monthly_hard_cap_cents: cap_cents,
                period: api::BillingPeriod {
                    resets_at: Some("2026-11-01T00:00:00.000Z".into()),
                    billable_microusd,
                    remaining_cap_microusd: 0,
                },
                ..BillingStatus::default()
            }),
        }
    }

    #[test]
    fn usage_reads_against_the_monthly_limit() {
        let s = snapshot(5_000, 12_340_000);
        assert_eq!(s.usage_line(), "$12.34 of $50 limit");
        assert!((s.fraction().unwrap() - 0.2468).abs() < 1e-4);
        assert_eq!(s.reset_line().as_deref(), Some("Resets Nov 1"));
        assert_eq!(s.limit_cents(), Some(5_000));
    }

    #[test]
    fn usage_without_billing_shows_plain_spend() {
        let s = Snapshot {
            used_usd: 3.5,
            ..Snapshot::default()
        };
        assert_eq!(s.usage_line(), "$3.50 used");
        assert_eq!(s.fraction(), None);
        assert_eq!(s.limit_cents(), None);
    }

    #[test]
    fn short_dates_reject_garbage() {
        assert_eq!(short_date("2026-01-09T00:00:00Z").as_deref(), Some("Jan 9"));
        assert_eq!(short_date("bad"), None);
        assert_eq!(short_date("2026-13-01"), None);
    }
}
