//! Composer chrome: folder tabs attached to the top edge of the prompt input.
//!
//! The model tab (pretty name plus reasoning effort) and the credential method
//! tab sit on the left, the voice tab on the right. Tabs share the input's
//! background and border, and overlap its top border by one pixel so each tab
//! reads as part of the input rather than a floating chip.
use super::*;

/// Tab height, excluding the one pixel that overlaps the input border.
pub(super) const TAB_HEIGHT: f32 = 24.;

impl Panel {
    /// Prompt input with its attached folder tabs. Every composer location
    /// (fresh session, startup layout, docked) uses this so the tabs never
    /// drift apart from the input they label.
    pub(super) fn render_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::global();
        let focused = self.input.read(cx).focus_handle.is_focused(window);
        let border = if focused {
            theme.PANEL_BORDER_FOCUS
        } else {
            theme.INPUT_BORDER
        };
        let account_label =
            account_method_label(self.provider.as_deref(), self.auth_method.as_deref());
        let model_label = model_tab_label(self.model.as_deref(), self.reasoning_effort.as_deref());
        let tabs = div()
            .debug_selector(|| "composer-tabs".into())
            .w_full()
            .min_w_0()
            .flex()
            .items_end()
            .gap_1()
            .px_2()
            // Overlap the input's top border so tab fills erase it below each tab.
            .mb(px(-1.))
            .child(
                div()
                    .debug_selector(|| "panel-identity".into())
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_end()
                    .gap_1()
                    .overflow_hidden()
                    .child(
                        composer_tab("panel-model", border)
                            .flex_shrink_1()
                            .min_w(px(56.))
                            .child(model_icon())
                            .child(div().min_w_0().truncate().child(model_label))
                            .child(div().flex_none().text_color(theme.TEXT_FAINT).child("⌄"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_model_picker(window, cx);
                                cx.stop_propagation();
                            })),
                    )
                    .child(
                        composer_tab("panel-login", border)
                            .flex_shrink(2.)
                            .min_w(px(48.))
                            .text_color(theme.TEXT_FAINT)
                            .child(div().min_w_0().truncate().child(account_label))
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(
                                    Box::new(crate::workspace::OpenAccounts {
                                        source: cx.entity_id(),
                                        login_command: None,
                                    }),
                                    cx,
                                );
                                cx.stop_propagation();
                            })),
                    ),
            )
            .child(self.render_voice_tab(border, cx));
        div()
            .debug_selector(|| "composer".into())
            .w_full()
            .min_w_0()
            .flex()
            // Reverse order paints the tabs after the input, so their fills
            // cover the input border where they join it.
            .flex_col_reverse()
            .child(self.render_voice_input_slot(cx))
            .child(tabs)
            .into_any_element()
    }
}

/// A folder tab: rounded top corners, open bottom, input-colored fill.
pub(super) fn composer_tab(id: &'static str, border: gpui::Rgba) -> gpui::Stateful<gpui::Div> {
    let theme = Theme::global();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .h(px(TAB_HEIGHT + 1.))
        .pb(px(1.))
        .px_2p5()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded_t_md()
        .border_t_1()
        .border_l_1()
        .border_r_1()
        .border_color(border)
        .bg(theme.INPUT_BG)
        .text_size(px(10.5))
        .font_family(theme.FONT_MONO)
        .text_color(theme.TEXT_DIM)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|el| el.text_color(theme.TEXT))
}

fn model_icon() -> impl IntoElement {
    div()
        .flex_none()
        .size(px(6.))
        .rounded_full()
        .bg(Theme::global().ACCENT)
}

/// `claude-opus-4-8` + `high` reads `Opus 4.8 · high`. The provider already
/// appears in the method tab, so the redundant `Claude` family prefix is
/// dropped, matching the TUI's compact model label.
pub(super) fn model_tab_label(model: Option<&str>, effort: Option<&str>) -> String {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return "Choose model".into();
    };
    let pretty = jcode_provider_core::model_names::pretty_model_display_name(model);
    let pretty = match pretty.strip_prefix("Claude ") {
        Some(rest) if !rest.trim().is_empty() => rest.to_string(),
        _ => pretty,
    };
    match effort.map(str::trim).filter(|effort| !effort.is_empty()) {
        Some(effort) => format!("{pretty} · {effort}"),
        None => pretty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_tab_uses_pretty_names_and_effort() {
        assert_eq!(model_tab_label(None, Some("high")), "Choose model");
        assert_eq!(model_tab_label(Some(" "), None), "Choose model");
        assert_eq!(model_tab_label(Some("claude-opus-4-8"), None), "Opus 4.8");
        assert_eq!(model_tab_label(Some("gpt-5.5"), Some("high")), "GPT-5.5 · high");
        assert_eq!(
            model_tab_label(Some("gpt-5.1-codex-max"), Some("")),
            "GPT-5.1 Codex Max"
        );
    }

    #[gpui::test]
    fn tabs_attach_to_the_input_top_edge(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [240., 480., 1440.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            panel.update(vcx, |panel, cx| {
                panel.model = Some("claude-opus-4-8".into());
                panel.provider = Some("anthropic".into());
                panel.auth_method = Some("oauth".into());
                panel.items = vec![Item::User("Hello".into())];
                cx.notify();
            });
            vcx.run_until_parked();
            let input = vcx.debug_bounds("prompt-input").unwrap();
            let model = vcx.debug_bounds("panel-model").unwrap();
            let login = vcx.debug_bounds("panel-login").unwrap();
            let voice = vcx.debug_bounds("voice-toggle").unwrap();
            for (name, tab) in [("model", model), ("login", login), ("voice", voice)] {
                assert!(
                    (f32::from(tab.bottom() - input.top()) - 1.).abs() < 0.5,
                    "{name} tab overlaps the input border at {width}: {tab:?} {input:?}"
                );
                assert!(tab.left() >= input.left() && tab.right() <= input.right());
            }
            assert!(model.right() <= login.left(), "method follows model");
            assert!(login.right() <= voice.left(), "voice sits on the right");
            assert!(
                (f32::from(input.right() - voice.right()) - 8.).abs() < 1.,
                "voice tab hugs the right edge"
            );
        }
    }
}
