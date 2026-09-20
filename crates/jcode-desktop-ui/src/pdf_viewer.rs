//! Native, on-demand PDF pages. Parsing and rasterization never run on the UI thread.
use crate::{image_cache, pdf_render::PdfDocument, theme::Theme};
use gpui::{
    Context, FocusHandle, Image, IntoElement, Render, ScrollHandle, Task, Window, div, img, point,
    prelude::*, px, relative,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct PdfViewState {
    pub page: usize,
    pub zoom: f32,
}
impl Default for PdfViewState {
    fn default() -> Self {
        Self { page: 1, zoom: 1.0 }
    }
}
impl PdfViewState {
    fn normalized(mut self) -> Self {
        self.page = self.page.max(1);
        self.zoom = if self.zoom.is_finite() {
            self.zoom.clamp(0.5, 3.0)
        } else {
            1.0
        };
        self
    }
}

pub(crate) struct PdfViewer {
    payload: Option<Arc<str>>,
    document: Option<Arc<PdfDocument>>,
    image: Option<Arc<Image>>,
    ratio: f32,
    state: PdfViewState,
    pub(crate) scroll: ScrollHandle,
    pending_scroll: Option<gpui::Point<gpui::Pixels>>,
    focus: FocusHandle,
    busy: bool,
    error: Option<String>,
    task: Option<Task<()>>,
}

impl PdfViewer {
    pub(crate) fn new(
        payload: Option<String>,
        state: PdfViewState,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut viewer = Self {
            payload: payload.map(Arc::from),
            document: None,
            image: None,
            ratio: 1.0,
            state: state.normalized(),
            scroll: ScrollHandle::new(),
            pending_scroll: None,
            focus,
            busy: false,
            error: None,
            task: None,
        };
        viewer.load(cx);
        viewer
    }

    pub(crate) fn state(&self) -> PdfViewState {
        self.state.clone()
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn restore_scroll(&mut self, offset: gpui::Point<gpui::Pixels>) {
        if self.image.is_some() {
            self.scroll.set_offset(offset);
        } else {
            self.pending_scroll = Some(offset);
        }
    }

    pub(crate) fn scroll_offset(&self) -> gpui::Point<gpui::Pixels> {
        self.pending_scroll.unwrap_or_else(|| self.scroll.offset())
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        let payload = self.payload.clone();
        let work = cx.background_executor().spawn(async move {
            let payload = payload.ok_or_else(|| {
                anyhow::anyhow!("PDF data is unavailable. Ask the agent to reload this panel.")
            })?;
            PdfDocument::from_base64(&payload).map(Arc::new)
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |viewer, cx| {
                viewer.busy = false;
                match result {
                    Ok(document) => {
                        viewer.state.page = viewer.state.page.min(document.page_count());
                        viewer.document = Some(document);
                        viewer.render_page(cx);
                    }
                    Err(error) => viewer.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
    }

    fn render_page(&mut self, cx: &mut Context<Self>) {
        let Some(document) = self.document.clone() else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.image = None; // Never label a previous page's image with the new page number.
        let page = self.state.page;
        // A single bounded, high-resolution page at a time, not an entire PDF in GPU memory.
        let work = cx.background_executor().spawn(async move {
            let bytes = document.render_page(page, 2400)?;
            let reader = image::ImageReader::with_format(
                std::io::Cursor::new(&bytes),
                image::ImageFormat::Png,
            );
            let (width, height) = reader.into_dimensions()?;
            anyhow::ensure!(
                width > 0 && height > 0 && width <= 4096 && height <= 4096,
                "Invalid PDF page dimensions"
            );
            Ok::<_, anyhow::Error>((bytes, width as f32 / height as f32))
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |viewer, cx| {
                viewer.busy = false;
                match result {
                    Ok((bytes, ratio)) => {
                        viewer.image = Some(image_cache::encoded(gpui::ImageFormat::Png, bytes));
                        viewer.ratio = ratio;
                        if let Some(offset) = viewer.pending_scroll.take() {
                            viewer.scroll.set_offset(offset);
                        }
                    }
                    Err(error) => viewer.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn navigate(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(document) = &self.document else {
            return;
        };
        let next = self
            .state
            .page
            .saturating_add_signed(delta)
            .clamp(1, document.page_count());
        if next == self.state.page {
            return;
        }
        self.state.page = next;
        self.pending_scroll = None;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.render_page(cx);
    }

    fn zoom(&mut self, value: f32, cx: &mut Context<Self>) {
        self.pending_scroll = None;
        self.state.zoom = if value.is_finite() {
            value.clamp(0.5, 3.0)
        } else {
            1.0
        };
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }
}

impl Render for PdfViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.document.as_ref().map(|document| document.page_count());
        let toolbar = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .text_size(px(12.))
            .text_color(Theme::global().TEXT)
            .children(
                [
                    ("pdf-previous", "Previous", -1isize),
                    ("pdf-next", "Next", 1),
                ]
                .into_iter()
                .map(|(id, label, delta)| {
                    let enabled = !self.busy
                        && count.is_some_and(|count| {
                            if delta < 0 {
                                self.state.page > 1
                            } else {
                                self.state.page < count
                            }
                        });
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .when(enabled, |view| {
                            view.cursor_pointer()
                                .hover(|style| style.bg(Theme::global().QUOTE_BG))
                        })
                        .when(!enabled, |view| {
                            view.text_color(Theme::global().TEXT_DIM).opacity(0.5)
                        })
                        .child(label)
                        .on_click(cx.listener(move |viewer, _, window, cx| {
                            viewer.focus.focus(window, cx);
                            if enabled {
                                viewer.navigate(delta, cx);
                            }
                            cx.stop_propagation();
                        }))
                }),
            )
            .child(
                div().id("pdf-page-number").px_2().child(
                    count
                        .map(|count| format!("Page {} of {count}", self.state.page))
                        .unwrap_or_else(|| "PDF".into()),
                ),
            )
            .children(
                [
                    ("pdf-zoom-out", "−", -0.25f32),
                    ("pdf-fit-width", "Fit width", 0.),
                    ("pdf-zoom-in", "+", 0.25),
                ]
                .into_iter()
                .map(|(id, label, delta)| {
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|style| style.bg(Theme::global().QUOTE_BG))
                        .child(label)
                        .on_click(cx.listener(move |viewer, _, window, cx| {
                            viewer.focus.focus(window, cx);
                            viewer.zoom(
                                if delta == 0. {
                                    1.
                                } else {
                                    viewer.state.zoom + delta
                                },
                                cx,
                            );
                            cx.stop_propagation();
                        }))
                }),
            )
            .child(format!("{:.0}%", self.state.zoom * 100.));
        let mut contents = div()
            .id("pdf-viewport")
            .debug_selector(|| "pdf-viewport".into())
            .size_full()
            .overflow_scroll()
            .track_scroll(&self.scroll)
            .p_3();
        if let Some(image) = &self.image {
            contents = contents.child(
                // GPUI substitutes the raster's intrinsic height when an img has
                // percentage width and auto height, even with aspect_ratio set.
                // Size the container by page ratio, then fill both image axes.
                div()
                    .w(relative(self.state.zoom))
                    .aspect_ratio(self.ratio)
                    .flex_none()
                    .child(
                        img(image_cache::source(image.clone()))
                            .debug_selector(|| "pdf-page-image".into())
                            .size_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    ),
            );
        } else if let Some(error) = &self.error {
            contents = contents.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_color(Theme::global().TEXT)
                    .child("Cannot display this PDF")
                    .child(error.clone())
                    .child(
                        div()
                            .id("pdf-retry")
                            .debug_selector(|| "pdf-retry".into())
                            .cursor_pointer()
                            .child("Retry")
                            .on_click(cx.listener(|viewer, _, _, cx| {
                                if viewer.document.is_some() {
                                    viewer.render_page(cx)
                                } else {
                                    viewer.load(cx)
                                }
                            })),
                    ),
            );
        } else {
            contents = contents.child(
                div()
                    .text_color(Theme::global().TEXT_DIM)
                    .child("Rendering PDF page…"),
            );
        }
        div()
            .id("pdf-viewer")
            .debug_selector(|| "pdf-viewer".into())
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|viewer, _, window, cx| viewer.focus.focus(window, cx)),
            )
            .on_key_down(cx.listener(|viewer, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "pagedown" => viewer.navigate(1, cx),
                    "pageup" => viewer.navigate(-1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(toolbar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(contents)
                    .child(crate::scrollbar::vertical(&self.scroll, "pdf-scrollbar")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restored_pdf_state_is_bounded() {
        assert_eq!(
            PdfViewState {
                page: 0,
                zoom: f32::NAN
            }
            .normalized(),
            PdfViewState::default()
        );
        assert_eq!(
            PdfViewState {
                page: 2,
                zoom: 900.
            }
            .normalized()
            .zoom,
            3.
        );
    }
    #[gpui::test]
    fn missing_payload_has_visible_error_and_retry(cx: &mut gpui::TestAppContext) {
        let (viewer, vcx) = cx.add_window_view(|_, cx| {
            PdfViewer::new(None, PdfViewState::default(), cx.focus_handle(), cx)
        });
        vcx.run_until_parked();
        assert!(viewer.read_with(vcx, |viewer, _| viewer.error.is_some()));
        assert!(vcx.debug_bounds("pdf-retry").is_some());
        assert!(vcx.debug_bounds("pdf-page-image").is_none());
    }
}
