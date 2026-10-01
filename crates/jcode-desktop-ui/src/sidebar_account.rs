//! The Jcode account pill pinned to the bottom of the sidebar.
//!
//! It names the signed-in Jcode account (not an AI provider login) with its
//! plan, and opens account management or sign-in on click.
use super::*;
use jcode_base::provider_catalog::load_env_value_from_env_or_config;
use jcode_base::subscription_catalog as subscription;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Whether the sidebar may show the account email. Off by default so a
/// streamed or shared screen never reveals it.
fn show_email_flag() -> &'static AtomicBool {
    static FLAG: OnceLock<AtomicBool> = OnceLock::new();
    FLAG.get_or_init(|| AtomicBool::new(crate::config::get().workspace.show_account_email))
}

pub(super) fn show_email() -> bool {
    show_email_flag().load(Ordering::Relaxed)
}

/// The locally saved Jcode account. Only non-secret metadata is kept here.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct JcodeAccount {
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<&'static str>,
}

impl JcodeAccount {
    fn load() -> Self {
        let value = |key| {
            load_env_value_from_env_or_config(key, subscription::JCODE_ENV_FILE)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Self {
            signed_in: subscription::has_credentials(),
            email: value(subscription::JCODE_ACCOUNT_EMAIL_ENV),
            plan: subscription::cached_tier().map(subscription::JcodeTier::display_name),
        }
    }

    /// Re-read the account file at most every few seconds, so sign-in and
    /// sign-out from the CLI show up without hitting the disk every frame.
    pub(super) fn current() -> Self {
        const STALE: Duration = Duration::from_secs(3);
        thread_local! {
            static CACHE: RefCell<Option<(Instant, JcodeAccount)>> = const { RefCell::new(None) };
        }
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            match &*cache {
                Some((at, account)) if at.elapsed() < STALE => account.clone(),
                _ => {
                    let account = Self::load();
                    *cache = Some((Instant::now(), account.clone()));
                    account
                }
            }
        })
    }

    fn title(&self, show_email: bool) -> String {
        match (&self.email, self.signed_in) {
            (Some(email), true) if show_email => email.clone(),
            (_, true) => "Jcode account".into(),
            _ => "Sign in to Jcode".into(),
        }
    }

    fn detail(&self) -> String {
        match (self.signed_in, self.plan) {
            (true, Some(plan)) => format!("{plan} plan"),
            (true, None) => "Signed in".into(),
            (false, _) => "Not signed in".into(),
        }
    }

    fn initial(&self, show_email: bool) -> String {
        self.email
            .as_deref()
            .filter(|_| self.signed_in && show_email)
            .and_then(|email| email.chars().next())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "J".into())
    }
}

impl Workspace {
    pub(super) fn render_sidebar_account(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let account = JcodeAccount::current();
        let signed_in = account.signed_in;
        let show_email = show_email();
        let ink = if signed_in { theme.TEXT } else { theme.TEXT_DIM };

        div()
            .flex_none()
            .px_2()
            .pt_1()
            .pb_2()
            .child(
                div()
                    .id("sidebar-account")
                    .debug_selector(|| "sidebar-account".into())
                    .h(px(40.0))
                    .pl(px(8.0))
                    .pr_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(theme.INLINE_CODE_BG)
                    .hover(|el| el.bg(theme.HEADER_BG))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if signed_in {
                                cx.open_url(subscription::JCODE_ACCOUNT_URL);
                                return;
                            }
                            // Email sign-in is paused, so reuse the Jcode
                            // provider login the Accounts page already runs.
                            let source = this
                                .slots
                                .get(this.active)
                                .filter(|slot| !slot.closing)
                                .map(|slot| slot.panel.entity_id());
                            match source {
                                Some(source) => this.open_accounts(
                                    &OpenAccounts {
                                        source,
                                        login_command: Some("/login jcode".into()),
                                    },
                                    window,
                                    cx,
                                ),
                                None => this.open_account_sign_in(window, cx),
                            }
                        }),
                    )
                    .child(
                        div()
                            .debug_selector(|| "sidebar-account-avatar".into())
                            .size(px(26.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(if signed_in {
                                theme.ACCENT_DIM
                            } else {
                                theme.PANEL_BG
                            })
                            .text_size(px(11.0))
                            .text_color(ink)
                            .child(account.initial(show_email)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .debug_selector(|| "sidebar-account-name".into())
                                    .truncate()
                                    .text_size(px(11.0))
                                    .line_height(px(14.0))
                                    .text_color(ink)
                                    .child(account.title(show_email)),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "sidebar-account-detail".into())
                                    .truncate()
                                    .text_size(px(9.0))
                                    .line_height(px(12.0))
                                    .text_color(theme.TEXT_DIM)
                                    .child(account.detail()),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Settings row: reveal the account email in the sidebar. Off by default.
    pub(super) fn render_account_email_setting(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let enabled = show_email();
        div()
            .id("settings-show-account-email")
            .debug_selector(|| "settings-show-account-email".into())
            .flex_none()
            .px_2()
            .py_2()
            .rounded_sm()
            .flex()
            .justify_between()
            .gap_2()
            .cursor_pointer()
            .bg(theme.PANEL_BG)
            .hover(|el| el.bg(theme.TOOL_BG))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let enabled = !show_email();
                    if let Err(error) = crate::config::persist_show_account_email(enabled) {
                        this.status = format!("Could not save account email setting: {error}");
                    }
                    show_email_flag().store(enabled, Ordering::Relaxed);
                    cx.notify();
                }),
            )
            .child("Show account email in sidebar")
            .child(
                div()
                    .text_color(if enabled { theme.ACCENT } else { theme.TEXT_DIM })
                    .child(if enabled { "On" } else { "Off" }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::JcodeAccount;

    #[test]
    fn signed_in_account_names_email_and_plan() {
        let account = JcodeAccount {
            signed_in: true,
            email: Some("ada@example.com".into()),
            plan: Some("Pro"),
        };
        assert_eq!(account.title(true), "ada@example.com");
        assert_eq!(account.detail(), "Pro plan");
        assert_eq!(account.initial(true), "A");
    }

    #[test]
    fn hidden_email_never_reaches_the_pill() {
        let account = JcodeAccount {
            signed_in: true,
            email: Some("ada@example.com".into()),
            plan: Some("Pro"),
        };
        assert_eq!(account.title(false), "Jcode account");
        assert_eq!(account.initial(false), "J");
        assert!(!crate::config::DesktopConfig::default()
            .workspace
            .show_account_email);
    }

    #[test]
    fn signed_out_account_invites_sign_in() {
        let account = JcodeAccount {
            signed_in: false,
            email: Some("stale@example.com".into()),
            plan: None,
        };
        assert_eq!(account.title(true), "Sign in to Jcode");
        assert_eq!(account.detail(), "Not signed in");
        assert_eq!(account.initial(true), "J");
    }
}
