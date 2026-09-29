//! Composer chrome: the input with one row of compact pills below it.
//!
//! Nothing sits above the input. The bottom row holds the model pill, a
//! separate reasoning effort pill, the credential method pill and the context
//! ring on the left, and the publish button plus the location (repo or
//! directory) on the right. The voice button lives inside the input box.
//! Pills are detached from the input and shaped like the transcript's user
//! prompt cards, so nothing reads as a folder tab.
use super::*;

/// Pill height.
pub(super) const TAB_HEIGHT: f32 = 22.;
/// Gap between the input and the pill row below it.
const PILL_GAP: f32 = 4.;
/// Distance from the input's right edge to the in-box voice button.
const VOICE_INSET: f32 = 8.;
/// Right padding the input reserves so text never runs under the voice
/// button. Linux shows a single-glyph keycap; other platforms spell out the
/// chord, so their button is wider.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) const VOICE_TRAILING_SPACE: f32 = 44.;
#[cfg(target_os = "macos")]
pub(super) const VOICE_TRAILING_SPACE: f32 = 52.;
#[cfg(target_os = "windows")]
pub(super) const VOICE_TRAILING_SPACE: f32 = 128.;

impl Panel {
    /// Prompt input with its pill row below. Every composer location
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
        let remote_machine = crate::harness::remote_host(&self.session_id).map(|host| {
            if host == crate::managed_cloud::HOST {
                "Jcode Cloud".to_string()
            } else {
                host
            }
        });
        let location = self
            .working_dir
            .as_deref()
            .filter(|dir| !dir.is_empty())
            .map(|dir| {
                if remote_machine.is_some() {
                    remote_location_label(dir)
                } else {
                    location_label(dir)
                }
            });
        // Build info, usage limits and status share the pill row, so the
        // panel needs no separate footer bar below the composer.
        let status_line = self.status_line();
        let meta = div()
            .debug_selector(|| "panel-meta".into())
            .flex_shrink(4.)
            .min_w_0()
            .h(px(TAB_HEIGHT))
            .px_1()
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .flex_nowrap()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(10.0))
            .font_family(theme.FONT_MONO)
            .text_color(theme.TEXT_FAINT)
            .child(
                div()
                    .debug_selector(|| "panel-status".into())
                    .min_w_0()
                    .flex_shrink_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .overflow_hidden()
                    .children(self.render_voice_status(status_line))
                    .children(self.render_usage_meters(cx))
                    .children(self.render_image_pane_toggle(cx)),
            )
            .when(self.show_build_footer, |el| {
                el.child(
                    div()
                        .id("panel-build")
                        .debug_selector(|| "panel-build".into())
                        .tooltip(|_, cx| cx.new(|_| crate::build_info::BuildTooltip).into())
                        // Build metadata yields space to the identity pills.
                        .flex_shrink_1()
                        .min_w_0()
                        .truncate()
                        .child(crate::build_info::label()),
                )
            });
        let pills = div()
            .debug_selector(|| "composer-tabs".into())
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .mt(px(PILL_GAP))
            .child(
                div()
                    .debug_selector(|| "panel-identity".into())
                    // Auto basis: the pills claim their width before the
                    // trailing directory, which truncates first.
                    .flex_auto()
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_provider_picker(window, cx);
                                cx.stop_propagation();
                            })),
                    )
                    .children(self.render_context_meter()),
            )
            .child(
                div()
                    .debug_selector(|| "composer-trailing".into())
                    // The directory yields space before the identity pills.
                    .flex_shrink(8.)
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .overflow_hidden()
                    .child(meta)
                    .children(self.render_publish_button(cx))
                    .children(remote_machine.map(|machine| {
                        // Remote sessions name their machine first, so a cloud
                        // chat never reads like a local one.
                        div()
                            .id("composer-machine")
                            .debug_selector(|| "composer-machine".into())
                            .flex_none()
                            .h(px(TAB_HEIGHT))
                            .px_2p5()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_full()
                            .bg(theme.ACCENT_DIM)
                            .text_color(theme.ACCENT)
                            .text_size(px(10.5))
                            .font_family(theme.FONT_MONO)
                            .whitespace_nowrap()
                            .child(machine)
                    }))
                    .children(location.map(|(name, detail)| {
                        div()
                            .debug_selector(|| "composer-location".into())
                            .flex_shrink_1()
                            .min_w_0()
                            .px_1()
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
            );
        let strips = self.render_composer_applets(window, cx);
        div()
            .debug_selector(|| "composer".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .children(strips)
            .children(self.render_provider_picker(window, cx))
            .child(
                // The voice button floats inside the input's right edge. The
                // input reserves matching trailing space for it.
                div()
                    .relative()
                    .w_full()
                    .min_w_0()
                    .child(self.render_voice_input_slot(cx))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(VOICE_INSET))
                            .flex()
                            .items_center()
                            .child(self.render_voice_tab(cx)),
                    ),
            )
            .child(pills)
            .into_any_element()
    }

    /// Composer-placed applet instances for this session, as a compact strip
    /// directly above the pills (suggestions, pickers).
    fn render_composer_applets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let instances: Vec<String> = crate::applet_runtime::get(cx)
            .host
            .borrow()
            .composer_for(&self.session_id)
            .map(|mounted| mounted.instance.id.clone())
            .collect();
        if instances.is_empty() {
            return None;
        }
        let selection = self.applet_selection.clone();
        let cards: Vec<gpui::AnyElement> = instances
            .iter()
            .filter_map(|instance| {
                crate::applet_surface::card(instance, false, &selection, window, cx)
            })
            .collect();
        Some(
            div()
                .debug_selector(|| "composer-applets".into())
                .w_full()
                .min_w_0()
                .px_1()
                .mb(px(PILL_GAP))
                .flex()
                .flex_col()
                .gap_1()
                .children(cards)
                .into_any_element(),
        )
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

/// Repo name plus its full parent path on another machine. The local `$HOME`
/// says nothing about the remote user's home, so nothing is abbreviated
/// except the managed cloud's own home directory.
pub(super) fn remote_location_label(path: &str) -> (String, Option<String>) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return ("/".into(), None);
    }
    let cloud_home = "/home/ec2-user";
    if trimmed == cloud_home {
        return ("~".into(), None);
    }
    let Some((parent, name)) = trimmed.rsplit_once('/') else {
        return (trimmed.into(), None);
    };
    let parent = if parent.is_empty() { "/" } else { parent };
    let parent = match parent.strip_prefix(cloud_home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => parent.to_string(),
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
    fn remote_location_keeps_the_remote_path() {
        assert_eq!(
            remote_location_label("/home/ec2-user/workspaces"),
            ("workspaces".into(), Some("~".into()))
        );
        assert_eq!(
            remote_location_label("/home/ec2-user/workspaces/app/"),
            ("app".into(), Some("~/workspaces".into()))
        );
        assert_eq!(
            remote_location_label("/srv/app"),
            ("app".into(), Some("/srv".into()))
        );
        assert_eq!(remote_location_label("/home/ec2-user"), ("~".into(), None));
        assert_eq!(
            remote_location_label("/home/ec2-userx/a"),
            ("a".into(), Some("/home/ec2-userx".into()))
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
    fn pills_sit_below_the_input_with_location_on_the_right(cx: &mut gpui::TestAppContext) {
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
            let editor = vcx.debug_bounds("prompt-editor").unwrap();
            let model = vcx.debug_bounds("panel-model").unwrap();
            let name = vcx.debug_bounds("panel-model-name").unwrap();
            let effort = vcx.debug_bounds("panel-model-effort").unwrap();
            let login = vcx.debug_bounds("panel-login").unwrap();
            let context = vcx.debug_bounds("panel-context-meter").unwrap();
            let voice = vcx.debug_bounds("voice-toggle").unwrap();
            for (tab_name, tab) in [("model", model), ("login", login), ("context", context)] {
                assert!(
                    tab.top() > input.bottom(),
                    "{tab_name} pill sits below the input at {width}: {tab:?} {input:?}"
                );
                assert!(tab.left() >= input.left() && tab.right() <= input.right());
            }
            assert!(name.right() <= model.right());
            assert!(model.right() <= effort.left(), "effort follows the model");
            assert!(effort.right() <= login.left(), "method follows effort");
            assert!(login.right() <= context.left(), "context ring follows method");
            // The voice button lives inside the input box, on its right, and
            // the editor never runs underneath it.
            assert!(
                voice.left() >= input.left()
                    && voice.right() <= input.right()
                    && voice.top() >= input.top()
                    && voice.bottom() <= input.bottom(),
                "voice sits inside the input at {width}: {voice:?} {input:?}"
            );
            assert!(editor.right() <= voice.left(), "{editor:?} {voice:?}");
            if width >= 480. {
                let location = vcx.debug_bounds("composer-location").unwrap();
                assert!(location.top() > input.bottom(), "location is below the input");
                assert!(context.right() <= location.left(), "location sits right");
                assert!(
                    input.right() - location.right() < px(12.),
                    "location hugs the right edge at {width}: {location:?} {input:?}"
                );
            }
        }
    }
}
