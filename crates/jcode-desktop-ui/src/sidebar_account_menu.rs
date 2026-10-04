//! In-app Jcode account menu, opened from the sidebar account pill.
//!
//! Plan, monthly usage, the spending limit, billing, and sign-out are handled
//! here through the Jcode SDK instead of sending people to the website.
//!
//! Upgrading is built to need as little as possible from the person:
//! - Subscribers switch plans in place. The card already on the subscription
//!   is used, so nothing is entered.
//! - New subscribers pick a plan here and finish in Stripe Checkout, which
//!   offers Link and saved cards. Card details are never typed into the app.
//!   The menu watches the account and updates by itself once payment lands.
//! - An expired sign-in never signs anyone out. It offers "Sign in again",
//!   which replaces the key in place.
use super::*;
use jcode_base::subscription_api::{self as api, AccountApiError, BillingStatus, SubscriptionMe};
use jcode_base::subscription_catalog as subscription;

/// Monthly spending limits offered as one-tap choices, in dollars.
const LIMIT_CHOICES: [u64; 5] = [25, 50, 100, 250, 500];
/// Monthly plans offered as one-tap choices, in dollars. Each $10 buys $20 of
/// included usage.
const PLAN_CHOICES: [u64; 5] = [10, 20, 50, 100, 200];
/// How long to watch for a Checkout payment before giving up quietly.
const CHECKOUT_WATCH: Duration = Duration::from_secs(15 * 60);
const CHECKOUT_POLL: Duration = Duration::from_secs(4);

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
    Plan(u64),
    Checkout(u64),
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
    /// A plan switch waiting for a second tap to confirm.
    confirm_plan: Option<u64>,
    /// The plan bought in a Checkout that is still open in the browser.
    awaiting_checkout: Option<u64>,
    /// The stored key was rejected. Offer sign-in, never sign out.
    needs_sign_in: bool,
    task: Option<gpui::Task<()>>,
    watch_task: Option<gpui::Task<()>>,
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
    let subscribed =
        std::env::var("JCODE_DESKTOP_SCREENSHOT_ACCOUNT_MENU_PLAN").as_deref() != Ok("none");
    fixture_enabled().then(|| Snapshot {
        email: "ada@example.com".into(),
        plan: subscribed.then_some("Pro"),
        status: "active".into(),
        used_usd: 12.34,
        billing: Some(BillingStatus {
            monthly_hard_cap_cents: 5_000,
            plan_usd: subscribed.then_some(20),
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

/// Marker for a rejected key. The menu turns it into a "Sign in again" action.
const SIGN_IN_EXPIRED: &str = "Your Jcode sign-in has expired.";

fn describe(error: AccountApiError) -> String {
    match error {
        AccountApiError::Unauthorized => SIGN_IN_EXPIRED.into(),
        AccountApiError::Offline(_) => "Could not reach Jcode. Check your connection.".into(),
        AccountApiError::Http {
            code: Some(code), ..
        } if code == "no_billing_account" => {
            "No billing is set up yet. Choose a plan first.".into()
        }
        AccountApiError::Http {
            code: Some(code), ..
        } if code == "invalid_hard_cap" => "That limit is not allowed.".into(),
        AccountApiError::Http {
            code: Some(code), ..
        } if code == "already_activated" => {
            "You already have a plan. Pick a new one to switch.".into()
        }
        AccountApiError::Http {
            code: Some(code), ..
        } if code == "not_subscribed" => "Choose a plan to subscribe first.".into(),
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

fn checkout_notice(plan_usd: u64) -> String {
    format!("Finish the ${plan_usd}/month plan in your browser. This updates by itself.")
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

    /// The subscribed monthly plan in dollars, if any.
    fn plan_usd(&self) -> Option<u64> {
        self.billing.as_ref().and_then(|b| b.plan_usd)
    }

    /// Accounts on an older subscription. They keep their plan but cannot
    /// open a new Checkout or switch through the dollar plans.
    fn legacy(&self) -> bool {
        self.billing
            .as_ref()
            .is_some_and(|b| b.activation.state == "legacy")
    }

    /// Has a Stripe customer to manage, either a dollar plan or a legacy one.
    fn has_billing(&self) -> bool {
        self.plan_usd().is_some() || self.legacy()
    }

    fn plan_label(&self) -> String {
        match (self.plan_usd(), self.plan) {
            (Some(usd), _) => format!("${usd}/month plan"),
            (None, Some(tier)) => format!("{tier} plan"),
            (None, None) => "No plan yet".into(),
        }
    }
}

impl State {
    fn fail(&mut self, error: String) {
        self.needs_sign_in = error == SIGN_IN_EXPIRED;
        self.error = Some(error);
    }
}

impl Workspace {
    pub(super) fn toggle_account_menu(&mut self, cx: &mut Context<Self>) {
        let menu = &mut self.account_menu;
        menu.open = !menu.open;
        menu.confirm_sign_out = false;
        menu.confirm_plan = None;
        if menu.awaiting_checkout.is_none() {
            menu.notice = None;
        }
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
        self.account_menu.confirm_plan = None;
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
                    Ok(snapshot) => {
                        menu.needs_sign_in = false;
                        menu.snapshot = Some(snapshot);
                    }
                    Err(error) => menu.fail(error),
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
                    Err(error) => menu.fail(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Tap a plan. Subscribers confirm and switch in place; everyone else goes
    /// to Stripe Checkout and the menu watches for the payment.
    fn choose_plan(&mut self, plan_usd: u64, cx: &mut Context<Self>) {
        if self.account_menu.busy.is_some() {
            return;
        }
        let current = self
            .account_menu
            .snapshot
            .as_ref()
            .and_then(Snapshot::plan_usd);
        if current == Some(plan_usd) {
            return;
        }
        if current.is_some() {
            if self.account_menu.confirm_plan != Some(plan_usd) {
                self.account_menu.confirm_plan = Some(plan_usd);
                self.account_menu.notice = None;
                self.account_menu.error = None;
                cx.notify();
                return;
            }
            self.switch_plan(plan_usd, cx);
        } else {
            self.start_checkout(plan_usd, cx);
        }
    }

    fn switch_plan(&mut self, plan_usd: u64, cx: &mut Context<Self>) {
        self.account_menu.confirm_plan = None;
        if fixture_enabled() {
            if let Some(billing) = self
                .account_menu
                .snapshot
                .as_mut()
                .and_then(|s| s.billing.as_mut())
            {
                billing.plan_usd = Some(plan_usd);
                billing.monthly_hard_cap_cents = plan_usd * 1_000;
            }
            self.account_menu.notice = Some(format!("Switched to ${plan_usd}/month"));
            cx.notify();
            return;
        }
        self.account_menu.busy = Some(Busy::Plan(plan_usd));
        self.account_menu.error = None;
        self.account_menu.notice = None;
        let change = cx.background_executor().spawn(async move {
            let (base, key) = credentials()?;
            let client = client()?;
            run(api::change_plan_with(&client, &base, &key, plan_usd))?.map_err(describe)
        });
        self.account_menu.task = Some(cx.spawn(async move |this, cx| {
            let result = change.await;
            let _ = this.update(cx, |this, cx| {
                this.account_menu.busy = None;
                match result {
                    Ok(change) => {
                        let when = if change.effective == "next_invoice" {
                            " from your next bill"
                        } else {
                            ""
                        };
                        this.account_menu.notice =
                            Some(format!("Switched to ${}/month{when}", change.plan_usd));
                        this.refresh_account_menu(cx);
                    }
                    Err(error) => this.account_menu.fail(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn start_checkout(&mut self, plan_usd: u64, cx: &mut Context<Self>) {
        if fixture_enabled() {
            self.account_menu.awaiting_checkout = Some(plan_usd);
            self.account_menu.notice = Some(checkout_notice(plan_usd));
            cx.notify();
            return;
        }
        self.account_menu.busy = Some(Busy::Checkout(plan_usd));
        self.account_menu.error = None;
        self.account_menu.notice = None;
        let checkout = cx.background_executor().spawn(async move {
            let (base, key) = credentials()?;
            let client = client()?;
            run(api::start_subscription_checkout_with(
                &client, &base, &key, plan_usd,
            ))?
            .map_err(describe)
        });
        self.account_menu.task = Some(cx.spawn(async move |this, cx| {
            let result = checkout.await;
            let _ = this.update(cx, |this, cx| {
                this.account_menu.busy = None;
                match result {
                    Ok(url) => {
                        cx.open_url(&url);
                        this.account_menu.awaiting_checkout = Some(plan_usd);
                        this.account_menu.notice = Some(checkout_notice(plan_usd));
                        this.watch_checkout(cx);
                    }
                    Err(error) => this.account_menu.fail(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Poll the account until the Checkout payment activates a plan, then
    /// update the menu and the sidebar pill without any action.
    fn watch_checkout(&mut self, cx: &mut Context<Self>) {
        let started = Instant::now();
        self.account_menu.watch_task = Some(cx.spawn(async move |this, cx| {
            while started.elapsed() < CHECKOUT_WATCH {
                cx.background_executor().timer(CHECKOUT_POLL).await;
                let load = cx.background_executor().spawn(async { load_snapshot() });
                let Ok(snapshot) = load.await else { continue };
                let activated = snapshot.plan_usd().is_some();
                let keep_going = this
                    .update(cx, |this, cx| {
                        let menu = &mut this.account_menu;
                        if menu.awaiting_checkout.is_none() {
                            return false;
                        }
                        if activated {
                            let plan = snapshot.plan_usd().unwrap_or_default();
                            menu.snapshot = Some(snapshot);
                            menu.awaiting_checkout = None;
                            menu.error = None;
                            menu.notice = Some(format!("You're on the ${plan}/month plan"));
                            sidebar_account::JcodeAccount::invalidate();
                            accounts::request_refresh();
                            cx.notify();
                            return false;
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.account_menu.awaiting_checkout.take().is_some() {
                    this.account_menu.notice =
                        Some("Checkout not finished yet. Tap a plan to try again.".into());
                    cx.notify();
                }
            });
        }));
    }

    fn sign_in_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account_menu.open = false;
        let source = self
            .slots
            .get(self.active)
            .filter(|slot| !slot.closing)
            .map(|slot| slot.panel.entity_id());
        match source {
            Some(source) => self.open_accounts(
                &OpenAccounts {
                    source,
                    login_command: Some("/login jcode".into()),
                },
                window,
                cx,
            ),
            None => self.open_account_sign_in(window, cx),
        }
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
                    Err(error) => this.account_menu.fail(error),
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
        let plan = match snapshot {
            Some(snapshot) => snapshot.plan_label(),
            None => account
                .plan
                .map(|plan| format!("{plan} plan"))
                .unwrap_or_else(|| "Jcode account".into()),
        };
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

        // Plan: switch in place, or upgrade through Checkout.
        if let Some(snapshot) = snapshot.filter(|s| !s.legacy()) {
            let current = snapshot.plan_usd();
            let mut chips = div().flex().flex_wrap().gap_1();
            for amount in PLAN_CHOICES {
                let selected = current == Some(amount);
                let confirming = menu.confirm_plan == Some(amount);
                let pending = matches!(
                    menu.busy,
                    Some(Busy::Plan(p) | Busy::Checkout(p)) if p == amount
                ) || menu.awaiting_checkout == Some(amount);
                chips = chips.child(
                    div()
                        .id(("account-plan", amount as usize))
                        .debug_selector(move || format!("account-plan-{amount}"))
                        .px_2()
                        .py(px(3.0))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if selected {
                            theme.ACCENT
                        } else if confirming || pending {
                            theme.ACCENT_DIM
                        } else {
                            theme.INLINE_CODE_BG
                        })
                        .text_color(if selected {
                            theme.BG
                        } else if confirming || pending {
                            theme.TEXT
                        } else {
                            theme.TEXT_DIM
                        })
                        .when(!selected, |el| {
                            el.hover(|el| el.bg(theme.HEADER_BG).text_color(theme.TEXT))
                        })
                        .child(format!("${amount}"))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_plan(amount, cx))),
                );
            }
            let heading = if current.is_some() {
                "Plan · per month"
            } else {
                "Upgrade · per month"
            };
            let hint = match (menu.confirm_plan, current) {
                (Some(next), Some(_)) => Some(format!(
                    "Tap ${next} again to switch. Your card on file is used from the next bill."
                )),
                (_, None) if menu.awaiting_checkout.is_none() => {
                    Some("Each $10 includes $20 of usage. Pay with Link or a saved card.".into())
                }
                _ => None,
            };
            let mut section = div()
                .debug_selector(|| "account-menu-plans".into())
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_color(theme.TEXT_DIM).child(heading))
                .child(chips);
            if let Some(hint) = hint {
                section = section.child(
                    div()
                        .debug_selector(|| "account-menu-plan-hint".into())
                        .text_color(theme.TEXT_DIM)
                        .child(hint),
                );
            }
            body = body.child(section);
        }

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
        let subscribed = snapshot.is_some_and(Snapshot::has_billing);
        let mut actions = div().flex().flex_col().gap_1();
        if menu.needs_sign_in {
            actions = actions.child(
                action("account-menu-sign-in-again", "Sign in again".into(), false)
                    .on_click(cx.listener(|this, _, window, cx| this.sign_in_again(window, cx))),
            );
        }
        body = body.child(
            actions
                .when(subscribed, |el| {
                    el.child(
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
                })
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
    fn plan_label_prefers_the_subscribed_dollar_plan() {
        let mut s = snapshot(5_000, 0);
        assert_eq!(s.plan_label(), "Pro plan");
        s.billing.as_mut().unwrap().plan_usd = Some(50);
        assert_eq!(s.plan_usd(), Some(50));
        assert_eq!(s.plan_label(), "$50/month plan");
        s.plan = None;
        s.billing = None;
        assert_eq!(s.plan_label(), "No plan yet");
    }

    #[test]
    fn legacy_accounts_keep_billing_but_hide_new_checkout() {
        let mut s = snapshot(10_000, 0);
        assert!(!s.legacy() && !s.has_billing());
        s.billing.as_mut().unwrap().activation.state = "legacy".into();
        assert!(s.legacy() && s.has_billing());
    }

    #[test]
    fn rejected_keys_offer_sign_in_instead_of_signing_out() {
        let mut state = State::default();
        state.fail(describe(AccountApiError::Unauthorized));
        assert!(state.needs_sign_in);
        state.fail(describe(AccountApiError::Offline("x".into())));
        assert!(!state.needs_sign_in);
    }

    #[test]
    fn short_dates_reject_garbage() {
        assert_eq!(short_date("2026-01-09T00:00:00Z").as_deref(), Some("Jan 9"));
        assert_eq!(short_date("bad"), None);
        assert_eq!(short_date("2026-13-01"), None);
    }
}
