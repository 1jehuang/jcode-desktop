//! Cached suggestion metadata and viewport-only model/command row rendering.
use super::*;

pub(super) struct SuggestionsCache {
    content: SharedString,
    revision: u64,
    minute: u64,
    completion: bool,
    rows: Arc<Vec<CommandSuggestion>>,
}

impl PromptInput {
    pub(super) fn reveal_command(&self, index: usize) {
        if self.command_scroll.bounds_for_item(index).is_some() {
            self.command_scroll.scroll_to_reveal_item(index);
        } else {
            // A distant item's estimated height is not its measured height.
            // Anchor by identity instead of an estimated pixel bottom, which
            // can leave the selected row just outside the virtual viewport.
            self.command_scroll.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: px(0.),
            });
        }
    }

    /// Hover changes selection, not the catalog. Retain rows across repaints and
    /// refresh relative usage labels on the next interaction after a minute.
    pub(super) fn command_suggestions(&self) -> Arc<Vec<CommandSuggestion>> {
        let minute = model_menu::now_unix_secs() / 60;
        let mut cache = self.suggestions_cache.borrow_mut();
        if let Some(cached) = cache.as_ref()
            && cached.content == self.content
            && cached.revision == self.suggestions_revision
            && cached.minute == minute
            && cached.completion == self.command_completion
        {
            return cached.rows.clone();
        }
        let rows = Arc::new(self.build_command_suggestions());
        // Metadata updates must not reset an independently scrolled menu. Only
        // changed identities/order or row geometry require fresh measurements.
        let same_layout = cache.as_ref().is_some_and(|old| {
            old.rows.len() == rows.len()
                && old.rows.iter().zip(rows.iter()).all(|(a, b)| {
                    a.value == b.value
                        && a.header == b.header
                        && a.detail.is_some() == b.detail.is_some()
                })
        });
        if !same_layout {
            self.command_scroll
                .reset_with_uniform_height(rows.len(), px(48.));
        }
        *cache = Some(SuggestionsCache {
            content: self.content.clone(),
            revision: self.suggestions_revision,
            minute,
            completion: self.command_completion,
            rows: rows.clone(),
        });
        rows
    }

    pub(super) fn render_command_row(
        &self,
        index: usize,
        suggestion: CommandSuggestion,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_toggle = suggestion.toggle.is_some();
        let selected = index == self.command_selection;
        let ink = if selected {
            Theme::global().BG
        } else {
            Theme::global().TEXT_DIM
        };
        let model = suggestion.value.strip_prefix("/model ");
        let logo_svg = |provider: &str, fallback: &str, size: f32, color: gpui::Rgba| {
            match crate::accounts::logo(provider) {
                Some(bytes) => gpui::svg()
                    .data(bytes)
                    .size(px(size))
                    .flex_none()
                    .text_color(color)
                    .into_any_element(),
                None => div()
                    .size(px(size))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(size * 0.6))
                    .text_color(color)
                    .child(crate::accounts::lettermark(fallback))
                    .into_any_element(),
            }
        };
        // The row logo names the model's maker (OpenAI, Anthropic), never the
        // service it is routed through. The group header names that service.
        let logo = model.map(|model| {
            let title = model_menu::pretty_title(model, &self.model_details);
            div()
                .debug_selector(move || format!("model-picker-logo-{index}"))
                .child(logo_svg(
                    model_menu::model_logo(model, &self.model_details),
                    &title,
                    18.0,
                    ink,
                ))
        });
        let header = suggestion.header.clone().map(|header| {
            let faint = Theme::global().TEXT_FAINT;
            let base = div()
                .debug_selector(move || format!("model-picker-group-{index}"))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .pt_3()
                .pb_1()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(faint);
            let Some(model) = model else {
                return base.child(header);
            };
            let parts = model_menu::header_parts(model, &self.model_details);
            base.child(logo_svg(parts.logo, &parts.provider, 13.0, faint))
                .child(div().text_color(Theme::global().TEXT_DIM).child(parts.provider))
                .child(
                    div()
                        .debug_selector(move || format!("model-picker-auth-{index}"))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py(px(1.0))
                        .rounded_full()
                        .bg(Theme::global().INPUT_BG)
                        .font_weight(gpui::FontWeight::NORMAL)
                        .child(
                            gpui::svg()
                                .data(parts.auth.icon())
                                .size(px(11.0))
                                .flex_none()
                                .text_color(faint),
                        )
                        .child(parts.auth.label()),
                )
        });
        let label = model
            .map(|model| model_menu::pretty_title(model, &self.model_details))
            .unwrap_or_else(|| suggestion.value.clone());
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .children(header)
            .child(
                div()
                    .id(("slash-command", index))
                    .debug_selector(move || {
                        if is_toggle {
                            format!("model-picker-toggle-{index}")
                        } else {
                            format!("slash-command-row-{index}")
                        }
                    })
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .bg(if selected {
                        Theme::global().TEXT
                    } else {
                        Theme::global().HEADER_BG
                    })
                    .when(!selected, |row| {
                        row.hover(|style| style.bg(Theme::global().INPUT_BG))
                    })
                    .text_size(px(12.0))
                    .text_color(ink)
                    .cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.command_selection != index {
                            this.command_selection = index;
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .items_center()
                            .gap_3()
                            .children(logo)
                            .font_family(if model.is_some() {
                                Theme::global().FONT_UI
                            } else {
                                Theme::global().FONT_MONO
                            })
                            .text_color(ink)
                            .when(selected, |label| {
                                label.font_weight(gpui::FontWeight::SEMIBOLD)
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .gap_1()
                                    .child(div().min_w_0().truncate().child(label))
                                    .children(suggestion.detail.map(|detail| {
                                        div()
                                            .debug_selector(move || {
                                                format!("model-picker-detail-{index}")
                                            })
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(11.0))
                                            .font_weight(gpui::FontWeight::NORMAL)
                                            .text_color(if selected {
                                                ink
                                            } else {
                                                Theme::global().TEXT_FAINT
                                            })
                                            .child(detail)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .max_w(relative(0.4))
                            .min_w_0()
                            .truncate()
                            .text_color(if selected {
                                ink
                            } else {
                                Theme::global().TEXT_FAINT
                            })
                            .child(suggestion.help),
                    )
                    .child(
                        div()
                            .w(px(14.0))
                            .flex_none()
                            .text_color(ink)
                            .when(selected, |marker| {
                                marker
                                    .debug_selector(|| "slash-command-selected".into())
                                    .child("✓")
                            }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            window.focus(&this.focus_handle, cx);
                            if let Some(key) = suggestion.toggle.clone() {
                                this.toggle_model_group(key, cx);
                                return;
                            }
                            if matches!(suggestion.value.as_str(), "/model" | "/models") {
                                this.set_content("/model ".into(), cx);
                                if let Some(on_change) = &this.on_change {
                                    on_change(&this.content, cx);
                                }
                            } else {
                                this.set_content(suggestion.value.clone(), cx);
                                // Search may also match a longer model name. Click must
                                // submit this exact route, not the first substring match.
                                this.command_selection = this
                                    .command_suggestions()
                                    .iter()
                                    .position(|row| row.value == suggestion.value)
                                    .unwrap_or(0);
                                this.submit(&Submit, window, cx);
                            }
                        }),
                    ),
            )
            .into_any_element()
    }
}
