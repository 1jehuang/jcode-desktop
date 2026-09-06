//! A panel-sized image lightbox. The transcript stays mounted behind it, so
//! closing the preview preserves the user's scroll position and draft.
use super::*;

pub(super) fn fixture_image() -> TranscriptImage {
    TranscriptImage::new(
        "image/png".into(),
        base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("../../../assets/previews/image-preview.png")),
        Some("Image preview test chart".into()),
    )
}

impl Panel {
    pub(super) fn open_image_preview(
        &mut self,
        image: TranscriptImage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if image.preview.is_none() {
            return;
        }
        self.image_preview = Some(image);
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub(super) fn close_image_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image_preview = None;
        self.focus_input(window, cx);
        cx.notify();
    }

    pub(super) fn render_image_preview(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let image = self.image_preview.as_ref()?;
        let preview = image.preview.clone()?;
        Some(
            div()
                .id("image-preview")
                .debug_selector(|| "image-preview".into())
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .bg(Theme::global().PANEL_BG)
                .occlude()
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_image_preview(window, cx);
                    cx.stop_propagation();
                }))
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_3()
                        .text_size(px(12.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(
                                    image
                                        .label
                                        .clone()
                                        .unwrap_or_else(|| "Image preview".into()),
                                ),
                        )
                        .child("Click or Esc to close  ×"),
                )
                .child(
                    div().flex_1().min_h_0().w_full().child(
                        img(crate::image_cache::source(preview))
                            .debug_selector(|| "image-preview-full".into())
                            .size_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn image_preview_click_enlarges_and_escape_restores_draft(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("image-session", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.unwrap();
        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Image(fixture_image())];
            panel.input.update(cx, |input, cx| {
                input.set_content("keep this draft".into(), cx)
            });
            cx.notify();
        });
        vcx.run_until_parked();
        let scroll_before = panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y());
        let thumbnail = vcx
            .debug_bounds("transcript-image")
            .expect("thumbnail paints");
        vcx.simulate_click(thumbnail.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_some()));
        let enlarged = vcx
            .debug_bounds("image-preview-full")
            .expect("full image paints");
        assert!(
            enlarged.size.height > thumbnail.size.height,
            "preview must be larger than thumbnail"
        );
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
            "keep this draft"
        );
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
        assert!(vcx.debug_bounds("image-preview").is_none());
        assert_eq!(
            panel.read_with(vcx, |panel, _| panel.test_scroll_offset_y()),
            scroll_before
        );
        vcx.simulate_keystrokes("x");
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).snapshot().content),
            "keep this draftx"
        );
        let thumbnail = vcx.debug_bounds("transcript-image").unwrap();
        vcx.simulate_click(thumbnail.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let overlay = vcx.debug_bounds("image-preview").unwrap();
        vcx.simulate_click(overlay.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(panel.read_with(vcx, |panel, _| panel.image_preview.is_none()));
    }
}
