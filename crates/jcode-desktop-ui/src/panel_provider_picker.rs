//! Select an existing credential route in place, without opening account management.
use super::*;
use std::rc::Rc;

impl Panel {
    pub(crate) fn toggle_provider_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.login.as_ref().is_some_and(|state| state.selection_only) {
            self.close_login_picker(cx);
            self.focus_input(window, cx);
            return;
        }
        self.close_model_picker(cx);
        self.input.update(cx, |input, cx| input.close_effort_menu(cx));
        self.open_login_picker(cx);
        if let Some(state) = self.login.as_mut() {
            state.selection_only = true;
            state.focus_pending = true;
        }
        cx.notify();
    }

    pub(super) fn select_existing_provider(
        &mut self,
        provider: LoginProvider,
        _label: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spec = self.input.read(cx).provider_route_spec(provider.id, self.model.as_deref());
        let Some(spec) = spec else {
            if let Some(state) = self.login.as_mut() {
                state.error = Some(format!("{} has no available model for this session.", provider.display_name));
            }
            cx.notify();
            return;
        };
        self.close_login_picker(cx);
        if self.preview_state.is_none() {
            self.bridge.send(Command::SetModel { session_id: self.session_id.clone(), model: spec.clone() });
        }
        self.items.push(Item::Assistant(format!("Switching provider to {}…", provider.display_name)));
        self.focus_input(window, cx);
        cx.notify();
    }

    pub(crate) fn render_provider_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let state = self.login.as_mut().filter(|state| state.selection_only)?;
        if state.focus_pending {
            state.focus_pending = false;
            state.search.focus_handle(cx).focus(window, cx);
        }
        let theme = Theme::global();
        let state = self.login.as_ref()?;
        let query = state.search.read(cx).content.to_lowercase();
        let rows: Vec<_> = state.account_rows(&state.providers).into_iter()
            .filter(|row| row.connected())
            .filter(|row| format!("{} {} {} {}", row.provider.display_name,
                row.provider.id, row.label.as_deref().unwrap_or(""),
                row.email.as_deref().unwrap_or("")).to_lowercase().contains(query.trim()))
            .collect();
        let visible = Rc::new(Vec::new());
        let remote = self.login_is_remote();
        let mut body = div().flex().flex_col().gap_2();
        if remote {
            body = body.child("Select a remote provider through the model picker. Local sign-ins are not available on this machine.");
        } else if rows.is_empty() {
            body = body.child(if state.status_loading || state.accounts.accounts.is_none() {
                "Loading providers…"
            } else if query.trim().is_empty() {
                "No connected providers. Add a sign-in in Accounts first."
            } else {
                "No matching providers."
            });
        } else {
            for row in &rows {
                body = body.child(self.render_account_row(row, None, &visible, None, cx));
            }
        }
        if let Some(error) = &state.error {
            body = body.child(div().text_color(theme.ERROR).child(error.clone()));
        }
        Some(div()
            .id("composer-provider-picker")
            .debug_selector(|| "composer-provider-picker".into())
            .w_full().min_w_0().mb_2().p_3().rounded_xl()
            .bg(theme.PANEL_BG).border_1().border_color(theme.PANEL_BORDER)
            .flex().flex_col().gap_2().text_size(px(12.)).text_color(theme.TEXT_DIM)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().text_color(theme.TEXT).font_weight(FontWeight::SEMIBOLD)
                    .child("Select provider"))
                .child(login_button("provider-picker-close", "Close").on_click(cx.listener(|this, _, window, cx| {
                    this.close_login_picker(cx);
                    this.focus_input(window, cx);
                }))))
            .child(state.search.clone())
            .child(div().id("provider-picker-rows").max_h(px(240.)).overflow_y_scroll()
                .track_scroll(&state.scroll).child(body))
            .into_any_element())
    }
}
