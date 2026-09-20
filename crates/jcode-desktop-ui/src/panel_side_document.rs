//! Native read-only documents owned by a conversation, never runtime sessions.
use super::*;
use crate::pdf_viewer::{PdfViewState, PdfViewer};
use jcode_sdk::SidePanelPage;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SideDocumentSnapshot {
    pub owner_session_id: String,
    pub page: SidePanelPage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pdf_view: Option<PdfViewState>,
}

pub(super) struct SideDocument {
    pub snapshot: SideDocumentSnapshot,
    pub scroll: ScrollHandle,
    selection: Entity<TextSelection>,
    pdf: Option<Entity<PdfViewer>>,
}

impl Panel {
    pub fn new_side_document(
        owner_session_id: &str,
        page: &SidePanelPage,
        bridge: Bridge,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(
            format!("side-panel://{owner_session_id}/{}", page.id),
            Some(page.title.clone()),
            None,
            bridge,
            cx,
        );
        panel.items.clear();
        panel.streaming_text.clear();
        panel.streaming_reasoning.clear();
        panel.todoist = None;
        // Even a detached input submitted programmatically must never send to
        // the synthetic document identity.
        panel.input = cx.new(|cx| PromptInput::new(cx, "Read-only document", |_, _, _, _| {}));
        panel.side_document = Some(SideDocument {
            snapshot: SideDocumentSnapshot {
                owner_session_id: owner_session_id.into(),
                page: page.clone(),
                pdf_view: None,
            },
            scroll: ScrollHandle::new(),
            selection: Self::new_document_selection(cx),
            pdf: Self::new_document_pdf(
                page,
                PdfViewState::default(),
                panel.focus_handle.clone(),
                cx,
            ),
        });
        panel
    }

    fn new_document_selection(cx: &mut Context<Self>) -> Entity<TextSelection> {
        let selection = cx.new(TextSelection::new);
        cx.observe(&selection, |_, _, cx| cx.notify()).detach();
        selection
    }

    fn new_document_pdf(
        page: &SidePanelPage,
        state: PdfViewState,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Option<Entity<PdfViewer>> {
        if page.format != jcode_sdk::SidePanelPageFormat::Pdf {
            return None;
        }
        let viewer = cx.new(|cx| PdfViewer::new(page.pdf_data.clone(), state, focus, cx));
        cx.observe(&viewer, |_, _, cx| cx.notify()).detach();
        Some(viewer)
    }

    pub(super) fn document_snapshot(&self, cx: &App) -> Option<SideDocumentSnapshot> {
        let document = self.side_document.as_ref()?;
        let mut snapshot = document.snapshot.clone();
        snapshot.pdf_view = document.pdf.as_ref().map(|viewer| viewer.read(cx).state());
        Some(snapshot)
    }

    pub(super) fn document_scroll(&self, cx: &App) -> Option<ScrollHandle> {
        let document = self.side_document.as_ref()?;
        Some(
            document
                .pdf
                .as_ref()
                .map(|viewer| viewer.read(cx).scroll.clone())
                .unwrap_or_else(|| document.scroll.clone()),
        )
    }

    pub(super) fn document_scroll_offset(&self, cx: &App) -> Option<gpui::Point<gpui::Pixels>> {
        let document = self.side_document.as_ref()?;
        Some(
            document
                .pdf
                .as_ref()
                .map(|viewer| viewer.read(cx).scroll_offset())
                .unwrap_or_else(|| document.scroll.offset()),
        )
    }

    pub(super) fn document_focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        self.side_document
            .as_ref()?
            .pdf
            .as_ref()
            .map(|viewer| viewer.read(cx).focus_handle())
    }

    pub(crate) fn update_side_document(&mut self, page: &SidePanelPage, cx: &mut Context<Self>) {
        let Some(document) = self.side_document.as_mut() else {
            return;
        };
        if document.snapshot.page.id != page.id || document.snapshot.page == *page {
            return;
        }
        if document.snapshot.page.content != page.content {
            // Old selection ranges and clipboard text must not survive edits.
            document.selection = Self::new_document_selection(cx);
        }
        if document.snapshot.page.format != page.format
            || document.snapshot.page.pdf_data != page.pdf_data
        {
            // Replacing the entity drops obsolete tasks and prevents stale pixels after an update.
            let state = document
                .pdf
                .as_ref()
                .map(|viewer| viewer.read(cx).state())
                .unwrap_or_default();
            document.pdf = Self::new_document_pdf(page, state, self.focus_handle.clone(), cx);
        }
        document.snapshot.page = page.clone();
        self.title = page.title.clone().into();
        cx.notify();
    }

    pub(crate) fn is_side_document(&self) -> bool {
        self.side_document.is_some()
    }

    pub(crate) fn side_document_owner(&self) -> Option<&str> {
        self.side_document
            .as_ref()
            .map(|document| document.snapshot.owner_session_id.as_str())
    }

    pub(crate) fn side_document_page_id(&self) -> Option<&str> {
        self.side_document
            .as_ref()
            .map(|document| document.snapshot.page.id.as_str())
    }

    pub(crate) fn side_document_from_snapshot(
        snapshot: &PanelSnapshot,
        bridge: Bridge,
        cx: &mut Context<Self>,
    ) -> Option<Self> {
        let document = snapshot.side_document.as_ref()?;
        let mut panel =
            Self::new_side_document(&document.owner_session_id, &document.page, bridge, cx);
        panel.restore_side_document_snapshot(snapshot, cx);
        Some(panel)
    }

    pub(super) fn restore_side_document_snapshot(
        &mut self,
        snapshot: &PanelSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(saved) = snapshot.side_document.as_ref() else {
            return false;
        };
        if self.side_document_owner() != Some(saved.owner_session_id.as_str())
            || self.side_document_page_id() != Some(saved.page.id.as_str())
        {
            return false;
        }
        self.update_side_document(&saved.page, cx);
        if let Some(state) = &saved.pdf_view {
            self.side_document.as_mut().unwrap().pdf =
                Self::new_document_pdf(&saved.page, state.clone(), self.focus_handle.clone(), cx);
        }
        if let Some(pdf) = self
            .side_document
            .as_ref()
            .and_then(|document| document.pdf.as_ref())
        {
            pdf.update(cx, |viewer, _| {
                viewer.restore_scroll(point(px(snapshot.scroll_x), px(snapshot.scroll_y)))
            });
        } else if let Some(scroll) = self.document_scroll(cx) {
            scroll.set_offset(point(px(snapshot.scroll_x), px(snapshot.scroll_y)));
        }
        true
    }

    pub(super) fn render_side_document(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let document = self.side_document.as_ref().expect("document panel");
        let page = &document.snapshot.page;
        let selection = document.selection.clone();
        let selection_focus = selection.read(cx).focus_handle();
        let source = if page.file_path.is_empty() {
            format!("{} · read-only", page.source.as_str())
        } else {
            format!("{} · read-only", page.file_path)
        };
        div()
            .id("side-document")
            .debug_selector(|| "side-document".into())
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(document.pdf.is_none(), |view| {
                view.track_focus(&self.focus_handle)
            })
            .text_size(px(13.5))
            .text_color(Theme::global().TEXT)
            .child(
                div()
                    .flex_none()
                    .min_w_0()
                    .overflow_hidden()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| "side-document-title".into())
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(page.title.clone()),
                    )
                    .child(
                        div()
                            .debug_selector(|| "side-document-source".into())
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(source),
                    ),
            )
            .child(if let Some(pdf) = &document.pdf {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(pdf.clone())
                    .into_any_element()
            } else {
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("side-document-contents")
                            .debug_selector(|| "side-document-contents".into())
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&document.scroll)
                            .key_context(TextSelection::key_context())
                            .track_focus(&selection_focus)
                            .on_action({
                                let selection = selection.clone();
                                move |_: &text_selection::Copy, _, cx| {
                                    selection.update(cx, |selection, cx| selection.copy(cx));
                                }
                            })
                            .on_mouse_up(gpui::MouseButton::Left, move |_, _, cx| {
                                selection.update(cx, |selection, cx| {
                                    selection.finish();
                                    cx.notify();
                                });
                            })
                            .p_4()
                            .child(markdown::render(
                                &page.content,
                                0,
                                &document.selection,
                                window,
                                cx,
                            )),
                    )
                    .child(crate::scrollbar::vertical(
                        &document.scroll,
                        "side-document-scrollbar",
                    ))
                    .into_any_element()
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn pdf_document_renders_pages_navigates_by_keyboard_and_restores_scroll(
        cx: &mut gpui::TestAppContext,
    ) {
        use base64::Engine as _;
        if std::process::Command::new("pdfinfo")
            .arg("-v")
            .output()
            .is_err()
            || std::process::Command::new("pdftoppm")
                .arg("-v")
                .output()
                .is_err()
        {
            eprintln!(
                "PDF visual regression requires Poppler; skipped because utilities are unavailable"
            );
            return;
        }
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let mut pdf = page();
            pdf.format = jcode_sdk::SidePanelPageFormat::Pdf;
            pdf.pdf_data = Some(
                base64::engine::general_purpose::STANDARD
                    .encode(include_bytes!("../../../assets/previews/pdf-preview.pdf")),
            );
            Panel::new_side_document("owner", &pdf, crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("pdf-page-image").is_some(),
            "actual PDF image must be laid out"
        );
        let image = vcx.debug_bounds("pdf-page-image").unwrap();
        let viewport = vcx.debug_bounds("pdf-viewport").unwrap();
        let ratio = f32::from(image.size.width) / f32::from(image.size.height);
        assert!(
            (ratio - 600. / 800.).abs() < 0.01,
            "image must use page aspect, not intrinsic raster height: {ratio}"
        );
        assert!(
            image.origin.y - viewport.origin.y < px(20.),
            "PDF starts immediately below the toolbar"
        );
        panel.update(vcx, |panel, cx| {
            let mut saved = panel.snapshot(cx);
            saved.side_document.as_mut().unwrap().pdf_view =
                Some(PdfViewState { page: 1, zoom: 2. });
            saved.scroll_x = -40.;
            saved.scroll_y = -120.;
            panel.restore_snapshot(saved, cx);
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, cx| {
            let saved = panel.snapshot(cx);
            assert_eq!(saved.scroll_x, -40.);
            assert_eq!(saved.scroll_y, -120.);
        });
        vcx.update(|window, cx| panel.read(cx).input_focus_handle(cx).focus(window, cx));
        vcx.simulate_keystrokes("pagedown");
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, cx| {
            assert_eq!(
                panel.document_snapshot(cx).unwrap().pdf_view,
                Some(PdfViewState { page: 2, zoom: 2. })
            );
            assert_eq!(panel.snapshot(cx).scroll_y, 0.);
        });
        let image = vcx
            .debug_bounds("pdf-page-image")
            .expect("second PDF page renders");
        assert!(
            image.size.width > image.size.height,
            "landscape page keeps its aspect ratio"
        );
        vcx.simulate_keystrokes("pageup");
        vcx.run_until_parked();
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel
                .document_snapshot(cx)
                .unwrap()
                .pdf_view
                .unwrap()
                .page),
            1
        );
        panel.update(vcx, |panel, cx| {
            let mut updated = panel.document_snapshot(cx).unwrap().page;
            let mut bytes = include_bytes!("../../../assets/previews/pdf-preview.pdf").to_vec();
            bytes.extend_from_slice(b"\n% updated panel\n");
            updated.pdf_data = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
            panel.update_side_document(&updated, cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("pagedown");
        vcx.run_until_parked();
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel
                .document_snapshot(cx)
                .unwrap()
                .pdf_view
                .unwrap()
                .page),
            2,
            "replacing PDF pixels must not drop keyboard focus"
        );
    }

    #[gpui::test]
    fn pdf_document_restores_view_state_and_can_be_replaced_with_markdown(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let mut pdf = page();
            pdf.format = jcode_sdk::SidePanelPageFormat::Pdf;
            pdf.file_path = "/remote/document.pdf".into();
            // Missing data must show an error, not try to open the remote path locally.
            Panel::new_side_document("owner", &pdf, crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pdf-viewer").is_some());
        assert!(vcx.debug_bounds("pdf-retry").is_some());
        assert!(vcx.debug_bounds("side-document-contents").is_none());
        panel.update(vcx, |panel, cx| {
            let mut saved = panel.snapshot(cx);
            saved.side_document.as_mut().unwrap().pdf_view =
                Some(PdfViewState { page: 2, zoom: 1.5 });
            let json = serde_json::to_string(&saved).unwrap();
            let saved: PanelSnapshot = serde_json::from_str(&json).unwrap();
            let restored = cx.new(|cx| {
                Panel::side_document_from_snapshot(&saved, crate::harness::spawn_inert(), cx)
                    .unwrap()
            });
            assert_eq!(
                restored.read(cx).document_snapshot(cx).unwrap().pdf_view,
                Some(PdfViewState { page: 2, zoom: 1.5 })
            );
            let old_pdf = panel
                .side_document
                .as_ref()
                .unwrap()
                .pdf
                .as_ref()
                .unwrap()
                .entity_id();
            let mut changed = panel.side_document.as_ref().unwrap().snapshot.page.clone();
            changed.title = "Title only".into();
            panel.update_side_document(&changed, cx);
            assert_eq!(
                panel
                    .side_document
                    .as_ref()
                    .unwrap()
                    .pdf
                    .as_ref()
                    .unwrap()
                    .entity_id(),
                old_pdf,
                "title updates must preserve PDF page and zoom"
            );
            panel.update_side_document(&page(), cx);
            assert!(panel.side_document.as_ref().unwrap().pdf.is_none());
            assert!(panel.document_snapshot(cx).unwrap().pdf_view.is_none());
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("pdf-viewer").is_none());
        assert!(vcx.debug_bounds("side-document-contents").is_some());
    }

    fn page() -> SidePanelPage {
        SidePanelPage {
            id: "notes".into(), title: "Working notes".into(),
            file_path: "/workspace/notes.md".into(),
            content: "# Native document\n\nA **bold** paragraph with `code`.\n\n- First item\n- Second item".into(),
            ..Default::default()
        }
    }

    #[gpui::test]
    fn side_document_paints_native_markdown_without_composer(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let mut page = page();
            page.file_path = format!("/workspace/{}/notes.md", "long-directory-name/".repeat(80));
            Panel::new_side_document("owner", &page, crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        for selector in [
            "side-document",
            "side-document-title",
            "side-document-source",
            "side-document-contents",
            "selectable-text-0-0",
            "selectable-text-0-1",
        ] {
            assert!(vcx.debug_bounds(selector).is_some(), "missing {selector}");
        }
        assert!(vcx.debug_bounds("transcript").is_none());
        assert!(vcx.debug_bounds("prompt-input").is_none());
        assert!(vcx.debug_bounds("prompt-editor").is_none());
        let document_bounds = vcx.debug_bounds("side-document").unwrap();
        let source_bounds = vcx.debug_bounds("side-document-source").unwrap();
        assert!(source_bounds.size.width <= document_bounds.size.width);
        assert!(
            source_bounds.size.height < px(40.),
            "long paths stay on one line"
        );
        panel.read_with(vcx, |panel, cx| {
            assert!(panel.is_side_document());
            assert!(!panel.can_fork());
            assert!(!panel.can_refresh_account_runtime());
            assert_eq!(panel.input_focus_handle(cx), panel.focus_handle);
            assert_eq!(panel.session_id, "side-panel://owner/notes");
        });
    }

    #[gpui::test]
    fn side_document_updates_in_place_and_restores_snapshot(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new_side_document("owner", &page(), crate::harness::spawn_inert(), cx)
        });
        vcx.run_until_parked();
        let previous = panel.read_with(vcx, |panel, _| {
            panel.side_document.as_ref().unwrap().selection.entity_id()
        });
        panel.update(vcx, |panel, cx| {
            let mut updated = page();
            updated.title = "Updated notes".into();
            updated.content = "# Updated\n\nNew text\n\nThird paragraph".into();
            panel.update_side_document(&updated, cx);
            assert_eq!(panel.title.as_ref(), "Updated notes");
            assert_ne!(
                panel.side_document.as_ref().unwrap().selection.entity_id(),
                previous
            );
            let saved = panel.snapshot(cx);
            let json = serde_json::to_string(&saved).unwrap();
            let saved: PanelSnapshot = serde_json::from_str(&json).unwrap();
            let restored = cx.new(|cx| {
                Panel::side_document_from_snapshot(&saved, crate::harness::spawn_inert(), cx)
                    .unwrap()
            });
            assert_eq!(
                restored.read(cx).snapshot(cx).side_document,
                saved.side_document
            );
            assert_eq!(restored.read(cx).side_document_owner(), Some("owner"));
            assert_eq!(restored.read(cx).side_document_page_id(), Some("notes"));
            panel.apply(
                &ApiEvent::TextDelta {
                    message_id: None,
                    session_id: panel.session_id.clone(),
                    text: "must not become chat".into(),
                },
                cx,
            );
            assert!(panel.items.is_empty());
            assert!(panel.streaming_text.is_empty());
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("selectable-text-0-2").is_some());
    }
    #[gpui::test]
    fn side_document_is_display_only_and_rejects_another_pages_update(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            Panel::new_side_document("owner", &page(), crate::harness::spawn_inert(), cx)
        });
        let input_id = panel.read_with(vcx, |panel, _| panel.input.entity_id());
        vcx.update(|_, cx| Panel::connect_input(&panel, cx));
        panel.update(vcx, |panel, cx| {
            assert_eq!(
                panel.input.entity_id(),
                input_id,
                "must not wire a runtime sender"
            );
            let selection = panel.side_document.as_ref().unwrap().selection.entity_id();
            panel.update_side_document(&page(), cx);
            assert_eq!(
                panel.side_document.as_ref().unwrap().selection.entity_id(),
                selection
            );
            let mut other = page();
            other.id = "other-page".into();
            other.content = "wrong page".into();
            panel.update_side_document(&other, cx);
            assert_eq!(panel.side_document.as_ref().unwrap().snapshot.page, page());
            panel.choose_account_model(cx);
            panel.open_model_picker(cx);
            for command in ["/cancel", "/model some-model", "/login", "/clear"] {
                assert!(panel.handle_slash_command(command, cx));
            }
            panel.run_session_operation(SessionOperation::Clear, "must not clear");
            panel.submit_command_prompt("must not send", cx);
            assert!(!panel.recovery_picker_open);
            assert!(!panel.model_picker_open);
            assert!(panel.login.is_none());
            assert!(panel.items.is_empty());
            assert!(panel.pending_users.is_empty());
            assert!(panel.has_scrollable_conversation());
            assert!(!panel.can_fork());
        });
    }
}
