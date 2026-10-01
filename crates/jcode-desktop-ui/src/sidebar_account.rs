//! The account pill pinned to the bottom of the sidebar.
//!
//! It names the login the focused panel is spending, with a glance at its
//! tightest quota, and opens the full Accounts view on click.
use super::*;

impl Workspace {
    /// The account the sidebar footer should name: the focused panel's
    /// credential when known, otherwise the most relevant connected login.
    fn sidebar_account(&self, cx: &mut Context<Self>) -> Option<&accounts::Account> {
        let active = self.slots.get(self.active).and_then(|slot| {
            let panel = slot.panel.read(cx);
            panel
                .provider
                .as_deref()
                .map(|provider| accounts::credential_id(provider, panel.auth_method.as_deref()))
        });
        accounts::ordered(&self.accounts, active.as_deref(), &self.recent_accounts)
            .into_iter()
            .next()
    }

    pub(super) fn render_sidebar_account(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let account = self.sidebar_account(cx).cloned();
        let open = self.sidebar_view == SidebarView::Accounts;
        let connected = account.as_ref().is_some_and(|account| account.available());

        let (title, detail) = match &account {
            Some(account) if account.available() => {
                // The tightest limit is the one worth seeing at a glance.
                let detail = account
                    .limits
                    .iter()
                    .max_by(|a, b| a.usage_percent.total_cmp(&b.usage_percent))
                    .map(|limit| format!("{} · {:.0}% used", limit.name, limit.usage_percent))
                    .unwrap_or_else(|| account.status_label().to_string());
                (account.display_name.clone(), detail)
            }
            Some(account) => (account.display_name.clone(), account.status_label().into()),
            None => ("Sign in".to_string(), "Connect an account".to_string()),
        };
        let others = self
            .accounts
            .iter()
            .filter(|other| other.available())
            .count()
            .saturating_sub(usize::from(connected));

        let ink = if connected { theme.TEXT } else { theme.TEXT_DIM };
        let logo: gpui::AnyElement = match account.as_ref().and_then(|a| accounts::logo(&a.id)) {
            Some(bytes) => gpui::svg()
                .data(bytes)
                .size(px(14.0))
                .flex_none()
                .text_color(ink)
                .into_any_element(),
            None => div()
                .size(px(14.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .text_color(ink)
                .child(
                    account
                        .as_ref()
                        .map(|a| accounts::lettermark(&a.display_name))
                        .unwrap_or_else(|| "?".into()),
                )
                .into_any_element(),
        };

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
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if open { theme.HEADER_BG } else { theme.INLINE_CODE_BG })
                    .hover(|el| el.bg(theme.HEADER_BG))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.sidebar_view = if this.sidebar_view == SidebarView::Accounts {
                                SidebarView::Sessions
                            } else {
                                SidebarView::Accounts
                            };
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .size(px(24.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.PANEL_BG)
                            .child(logo),
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
                                    .child(title),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "sidebar-account-detail".into())
                                    .truncate()
                                    .text_size(px(9.0))
                                    .line_height(px(12.0))
                                    .text_color(
                                        if account.as_ref().is_some_and(|a| a.status == "expired") {
                                            theme.WARN
                                        } else {
                                            theme.TEXT_DIM
                                        },
                                    )
                                    .child(detail),
                            ),
                    )
                    .when(others > 0, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .px(px(6.0))
                                .rounded_full()
                                .bg(theme.PANEL_BG)
                                .text_size(px(9.0))
                                .text_color(theme.TEXT_DIM)
                                .child(format!("+{others}")),
                        )
                    }),
            )
            .into_any_element()
    }
}
