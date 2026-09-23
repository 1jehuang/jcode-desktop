//! Session-local image browser. Images retain their transcript anchors, while
//! pane mode replaces large inline previews with compact navigation links.
use super::*;

impl Panel {
    pub(super) fn session_image_indices(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| matches!(item, Item::Image(_)).then_some(index))
            .collect()
    }

    pub(super) fn selected_session_image_index(&self) -> Option<usize> {
        self.image_pane_selected
            .filter(|index| matches!(self.items.get(*index), Some(Item::Image(_))))
            .or_else(|| {
                self.items
                    .iter()
                    .rposition(|item| matches!(item, Item::Image(_)))
            })
    }

    pub(super) fn set_image_pane_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.image_pane_open = open;
        self.cancel_transcript_momentum();
        self.transcript_measurements.dirty = true;
        cx.notify();
    }

    /// Icon-only toggle: the footer never shows an image count.
    pub(super) fn render_image_pane_toggle(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let color = if self.image_pane_open {
            theme.TEXT
        } else {
            theme.TEXT_DIM
        };
        div()
            .id("panel-images")
            .debug_selector(|| "panel-images".into())
            .flex_none()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .when(self.image_pane_open, |el| el.bg(theme.ACCENT_DIM))
            .hover(|el| el.bg(theme.QUOTE_BG))
            .tooltip(|_, cx| {
                cx.new(|_| super::usage::MeterTooltip("Session images".into()))
                    .into()
            })
            .child(
                gpui::svg()
                    .data(include_bytes!("../../../assets/icons/image.svg") as &'static [u8])
                    .text_color(color)
                    .size(px(13.)),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.set_image_pane_open(!this.image_pane_open, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    pub(super) fn render_image_pane_link(
        &self,
        index: usize,
        image: &TranscriptImage,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let label = image
            .label
            .clone()
            .unwrap_or_else(|| "Image attachment".into());
        div()
            .id(("transcript-image-link", index))
            .debug_selector(move || format!("transcript-image-link-{index}"))
            .px_2()
            .py_1()
            .rounded_md()
            .text_size(px(11.))
            .text_color(Theme::global().TEXT_DIM)
            .bg(Theme::global().USER_BG)
            .cursor_pointer()
            .hover(|el| el.text_color(Theme::global().TEXT))
            .child(format!(
                "{} · {label} · View in Images ↗",
                image.model_input_caption().unwrap_or("Image attachment")
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.image_pane_selected = Some(index);
                cx.notify();
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    pub(super) fn render_image_pane(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.image_pane_open {
            return None;
        }
        let theme = Theme::global();
        let indices = self.session_image_indices();
        let selected = self.selected_session_image_index();
        let image = selected.and_then(|index| match &self.items[index] {
            Item::Image(image) => Some(image),
            _ => None,
        });
        let pane = div()
            .id("session-image-pane")
            .debug_selector(|| "session-image-pane".into())
            .flex()
            .flex_col()
            .flex_none()
            .min_w_0()
            .min_h_0()
            .when(self.image_pane_stacked, |el| {
                el.w_full().h(gpui::relative(0.45)).border_t_1()
            })
            .when(!self.image_pane_stacked, |el| {
                el.w(gpui::relative(0.4)).h_full().border_l_1()
            })
            .border_color(theme.PANEL_BORDER)
            .bg(theme.PANEL_BG)
            .p_3()
            .gap_2()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(theme.TEXT)
                            .child(format!("Session images · {}", indices.len())),
                    )
                    .child(
                        div()
                            .id("session-image-close")
                            .debug_selector(|| "session-image-close".into())
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_size(px(11.))
                            .text_color(theme.TEXT_DIM)
                            .cursor_pointer()
                            .hover(|el| el.bg(theme.QUOTE_BG))
                            .child("Inline ×")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_image_pane_open(false, cx);
                                this.focus_input(window, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .when_some(image, |el, image| {
                let preview_image = image.clone();
                el.child(
                    div()
                        .id("session-image-main")
                        .debug_selector(|| "session-image-main".into())
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .rounded_md()
                        .overflow_hidden()
                        .bg(theme.USER_BG)
                        .when_some(image.preview.clone(), |el, preview| {
                            el.cursor_pointer()
                                .child(
                                    img(crate::image_cache::source(preview))
                                        .absolute()
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Contain),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_image_preview(preview_image.clone(), window, cx);
                                    cx.stop_propagation();
                                }))
                        })
                        .when(image.preview.is_none(), |el| {
                            el.flex().items_center().justify_center().child(
                                div()
                                    .p_3()
                                    .text_size(px(12.))
                                    .text_color(theme.TEXT_DIM)
                                    .child(format!("Could not display {}", image.media_type)),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .min_w_0()
                        .text_size(px(11.))
                        .text_color(theme.TEXT_DIM)
                        .child(
                            image
                                .label
                                .clone()
                                .unwrap_or_else(|| "Image attachment".into()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(theme.TEXT_FAINT)
                        .child(image.model_input_caption().unwrap_or("Session attachment"))
                        .child(
                            div()
                                .id("session-image-latest")
                                .debug_selector(|| "session-image-latest".into())
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|el| el.bg(theme.QUOTE_BG))
                                .child(if self.image_pane_selected.is_none() {
                                    "Following latest"
                                } else {
                                    "Follow latest"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.image_pane_selected = None;
                                    cx.notify();
                                    cx.stop_propagation();
                                })),
                        ),
                )
            })
            .when(image.is_none(), |el| {
                el.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(theme.TEXT_DIM)
                        .child("No images in this session yet")
                        .child("Images read by the model and your attachments will appear here."),
                )
            })
            .when(!indices.is_empty(), |el| {
                el.child(
                    div()
                        .id("session-image-thumbnails")
                        .debug_selector(|| "session-image-thumbnails".into())
                        .flex_none()
                        .h(px(72.))
                        .w_full()
                        .overflow_x_scroll()
                        .track_scroll(&self.image_pane_scroll)
                        .flex()
                        .gap_2()
                        .children(indices.into_iter().enumerate().map(|(number, index)| {
                            let Item::Image(image) = &self.items[index] else {
                                unreachable!()
                            };
                            div()
                                .id(("session-image-thumb", index))
                                .debug_selector(move || format!("session-image-thumb-{index}"))
                                .flex_none()
                                .w(px(80.))
                                .h(px(64.))
                                .rounded_md()
                                .overflow_hidden()
                                .border_1()
                                .border_color(if selected == Some(index) {
                                    theme.ACCENT
                                } else {
                                    theme.PANEL_BORDER
                                })
                                .bg(theme.USER_BG)
                                .cursor_pointer()
                                .hover(|el| el.bg(theme.QUOTE_BG))
                                .when_some(image.preview.clone(), |el, preview| {
                                    el.child(
                                        img(crate::image_cache::source(preview))
                                            .size_full()
                                            .object_fit(gpui::ObjectFit::Contain),
                                    )
                                })
                                .when(image.preview.is_none(), |el| {
                                    el.flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(11.))
                                        .text_color(theme.TEXT_DIM)
                                        .child(format!("Image {}", number + 1))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.image_pane_selected = Some(index);
                                    cx.notify();
                                    cx.stop_propagation();
                                }))
                        })),
                )
            });
        Some(pane.into_any_element())
    }

    /// Use this panel's bounds, not the window's: split workspaces can be narrow.
    pub(super) fn image_pane_layout_observer(&self, cx: &Context<Self>) -> impl IntoElement {
        let panel = cx.entity().downgrade();
        let was_stacked = self.image_pane_stacked;
        gpui::canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                let stacked = f32::from(bounds.size.width) < 760.;
                if was_stacked != stacked {
                    let panel = panel.clone();
                    cx.defer(move |cx| {
                        let _ = panel.update(cx, |panel, cx| {
                            if panel.image_pane_stacked != stacked {
                                panel.image_pane_stacked = stacked;
                                cx.notify();
                            }
                        });
                    });
                }
            },
        )
        .absolute()
        .size_full()
    }
}
