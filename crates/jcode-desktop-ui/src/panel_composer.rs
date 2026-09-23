//! Composer chrome: a plain location line and compact pills above the input.
//!
//! The model pill, a separate reasoning effort pill, and the credential
//! method pill sit on the left, followed by the location (repo or directory)
//! as plain text that truncates first. The voice pill (microphone plus its
//! keybinding) sits on the right.
//! Pills are detached from the input and shaped like the transcript's user
//! prompt cards, so nothing reads as a folder tab.
use super::*;

/// Pill height.
pub(super) const TAB_HEIGHT: f32 = 22.;
/// Gap between the pill row and the input.
const PILL_GAP: f32 = 4.;

impl Panel {
    /// Prompt input with its location line and pills. Every composer location
    /// (fresh session, startup layout, docked) uses this so the pills never
    /// drift apart from the input they label.
    pub(super) fn render_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let _ = window;
        let theme = Theme::global();
        let account_label =
            account_method_label(self.provider.as_deref(), self.auth_method.as_deref());
        let model_label = pretty_model_label(self.model.as_deref());
        let effort = self
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|effort| !effort.is_empty() && self.model.is_some())
            .map(str::to_string);
        let location = self
            .working_dir
            .as_deref()
            .filter(|dir| !dir.is_empty())
            .map(location_label);
        let pills = div()
            .debug_selector(|| "composer-tabs".into())
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .mb(px(PILL_GAP))
            .child(
                div()
                    .debug_selector(|| "panel-identity".into())
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .overflow_hidden()
                    .child(
                        composer_pill("panel-model")
                            .flex_shrink_1()
                            .min_w(px(56.))
                            .child(
                                div()
                                    .debug_selector(|| "panel-model-name".into())
                                    .min_w_0()
                                    .truncate()
                                    .child(model_label),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_model_picker(window, cx);
                                cx.stop_propagation();
                            })),
                    )
                    .children(effort.map(|effort| {
                        composer_pill("panel-model-effort")
                            .flex_shrink(3.)
                            .min_w(px(24.))
                            .text_color(theme.TEXT_FAINT)
                            .child(div().min_w_0().truncate().child(effort))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_effort_picker(window, cx);
                                cx.stop_propagation();
                            }))
                    }))
                    .child(
                        composer_pill("panel-login")
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
                    )
                    .children(location.map(|(name, detail)| {
                        div()
                            .debug_selector(|| "composer-location".into())
                            .flex_shrink(4.)
                            .min_w_0()
                            .pl_1()
                            .flex()
                            .items_baseline()
                            .gap_1p5()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.5))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme.TEXT_DIM)
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(name),
                            )
                            .children(detail.map(|detail| {
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .font_family(theme.FONT_MONO)
                                    .text_color(theme.TEXT_FAINT)
                                    .child(detail)
                            }))
                    })),
            )
            .children(self.render_publish_button(cx))
            .child(self.render_voice_tab(cx));
        div()
            .debug_selector(|| "composer".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(pills)
            .child(self.render_voice_input_slot(cx))
            .into_any_element()
    }
}

/// A compact rounded pill, filled like the transcript's user prompt cards.
pub(super) fn composer_pill(id: &'static str) -> gpui::Stateful<gpui::Div> {
    let theme = Theme::global();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .h(px(TAB_HEIGHT))
        .px_2p5()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded_full()
        .bg(theme.prompt_background(usize::MAX))
        .text_size(px(10.5))
        .font_family(theme.FONT_MONO)
        .text_color(theme.TEXT_DIM)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|el| el.text_color(theme.TEXT).bg(theme.USER_BG))
}

/// `claude-opus-4-8` reads `Opus 4.8`. The provider already appears in the
/// method pill, so the redundant `Claude` family prefix is dropped, matching
/// the TUI's compact model label.
pub(super) fn pretty_model_label(model: Option<&str>) -> String {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return "Choose model".into();
    };
    let pretty = jcode_provider_core::model_names::pretty_model_display_name(model);
    match pretty.strip_prefix("Claude ") {
        Some(rest) if !rest.trim().is_empty() => rest.to_string(),
        _ => pretty,
    }
}

/// Model label plus effort, used where a single string is needed.
#[cfg(test)]
pub(super) fn model_tab_label(model: Option<&str>, effort: Option<&str>) -> String {
    let pretty = pretty_model_label(model);
    match effort.map(str::trim).filter(|effort| !effort.is_empty()) {
        Some(effort) if model.is_some_and(|m| !m.trim().is_empty()) => format!("{pretty} {effort}"),
        _ => pretty,
    }
}

/// Repo name (the last path component) plus its home-relative parent path.
/// `/home/me/src/jcode` reads `jcode` then `~/src`.
pub(super) fn location_label(path: &str) -> (String, Option<String>) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return ("/".into(), None);
    }
    let home = std::env::var("HOME").ok().filter(|home| !home.is_empty());
    if home.as_deref() == Some(trimmed) {
        return ("~".into(), None);
    }
    let (parent, name) = match trimmed.rsplit_once('/') {
        Some((parent, name)) => (parent, name),
        None => return (trimmed.into(), None),
    };
    let parent = if parent.is_empty() { "/" } else { parent };
    let parent = match home.as_deref() {
        Some(home) if parent == home => "~".to_string(),
        Some(home) => match parent.strip_prefix(&format!("{home}/")) {
            Some(rest) => format!("~/{rest}"),
            None => parent.to_string(),
        },
        None => parent.to_string(),
    };
    (name.to_string(), Some(parent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_tab_uses_pretty_names_and_effort() {
        assert_eq!(model_tab_label(None, Some("high")), "Choose model");
        assert_eq!(model_tab_label(Some(" "), None), "Choose model");
        assert_eq!(model_tab_label(Some("claude-opus-4-8"), None), "Opus 4.8");
        assert_eq!(
            model_tab_label(Some("gpt-5.5"), Some("high")),
            "GPT-5.5 high"
        );
        assert_eq!(model_tab_label(None, Some("high")), "Choose model");
        assert_eq!(
            model_tab_label(Some("gpt-5.1-codex-max"), Some("")),
            "GPT-5.1 Codex Max"
        );
    }

    #[test]
    fn location_label_splits_repo_name_from_parent() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            location_label(&format!("{home}/jcode")),
            ("jcode".into(), Some("~".into()))
        );
        assert_eq!(
            location_label(&format!("{home}/src/jcode/")),
            ("jcode".into(), Some("~/src".into()))
        );
        assert_eq!(location_label(&home), ("~".into(), None));
        assert_eq!(
            location_label("/srv/app"),
            ("app".into(), Some("/srv".into()))
        );
        assert_eq!(location_label("/"), ("/".into(), None));
    }

    #[gpui::test]
    fn pills_sit_above_the_input_with_location_after_method(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| Panel::new_preview(PreviewState::Empty, cx));
        let handle = vcx.update(|window, _| window.window_handle());
        for width in [240., 480., 1440.] {
            vcx.simulate_window_resize(handle, gpui::size(px(width), px(600.)));
            panel.update(vcx, |panel, cx| {
                panel.model = Some("claude-opus-4-8".into());
                panel.reasoning_effort = Some("high".into());
                panel.provider = Some("anthropic".into());
                panel.auth_method = Some("oauth".into());
                panel.working_dir = Some("/srv/projects/jcode".into());
                panel.items = vec![Item::User("Hello".into())];
                cx.notify();
            });
            vcx.run_until_parked();
            let input = vcx.debug_bounds("prompt-input").unwrap();
            let location = vcx.debug_bounds("composer-location").unwrap();
            let model = vcx.debug_bounds("panel-model").unwrap();
            let name = vcx.debug_bounds("panel-model-name").unwrap();
            let effort = vcx.debug_bounds("panel-model-effort").unwrap();
            let login = vcx.debug_bounds("panel-login").unwrap();
            let voice = vcx.debug_bounds("voice-toggle").unwrap();
            assert!(
                login.right() <= location.left(),
                "location follows the method pill"
            );
            assert!(
                location.right() <= voice.left(),
                "location stays left of voice at {width}: {location:?} {voice:?} {login:?} {effort:?} {model:?}"
            );
            for (tab_name, tab) in [("model", model), ("login", login), ("voice", voice)] {
                assert!(
                    tab.bottom() < input.top(),
                    "{tab_name} pill is detached from the input at {width}: {tab:?} {input:?}"
                );
                assert!(tab.left() >= input.left() && tab.right() <= input.right());
            }
            assert!(name.right() <= model.right());
            assert!(
                model.right() <= effort.left(),
                "effort is its own pill after the model"
            );
            assert!(effort.right() <= login.left(), "method follows effort");
            assert!(login.right() <= voice.left(), "voice sits on the right");
            assert!(
                voice.size.width > voice.size.height,
                "voice pill shows its shortcut"
            );
        }
    }
}
