//! Native read-only documents owned by a conversation, never runtime sessions.
use super::*;
use jcode_sdk::SidePanelPage;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SideDocumentSnapshot {
    pub owner_session_id: String,
    pub page: SidePanelPage,
}

pub(super) struct SideDocument {
    pub snapshot: SideDocumentSnapshot,
    pub scroll: ScrollHandle,
    selection: Entity<TextSelection>,
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
            },
            scroll: ScrollHandle::new(),
            selection: Self::new_document_selection(cx),
        });
        panel
    }

    fn new_document_selection(cx: &mut Context<Self>) -> Entity<TextSelection> {
        let selection = cx.new(TextSelection::new);
        cx.observe(&selection, |_, _, cx| cx.notify()).detach();
        selection
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
        self.side_document
            .as_ref()
            .unwrap()
            .scroll
            .set_offset(point(px(snapshot.scroll_x), px(snapshot.scroll_y)));
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
            .track_focus(&self.focus_handle)
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
            .child(
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
                    )),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
