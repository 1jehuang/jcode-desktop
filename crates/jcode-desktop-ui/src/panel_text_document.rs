//! Logical selectable text, independent of virtual-row layout and painting.
use super::*;
use std::hash::{Hash, Hasher};

type Segments = Vec<(SharedString, SharedString)>;

#[derive(Default)]
pub(super) struct TranscriptTextDocument {
    rows: HashMap<usize, (u64, Segments)>,
    order: Vec<usize>,
}

impl TranscriptTextDocument {
    /// Return a replacement only when text, disclosure state, or row order
    /// changes. Settled markdown is not reparsed on pointer/animation frames.
    pub(super) fn sync(
        &mut self,
        items: &[Item],
        rows: &[TranscriptRenderRow],
        expanded_prompts: &HashSet<(usize, bool)>,
        expanded_tools: &HashSet<String>,
    ) -> Option<Segments> {
        let order: Vec<_> = rows.iter().map(|row| row.index).collect();
        let mut changed = self.order != order;
        for row in rows {
            let item = match &row.source {
                TranscriptRowSource::Settled(index) => &items[*index],
                TranscriptRowSource::Owned(item) => item,
            };
            let prompt_expanded = expanded_prompts.contains(&(row.index, false));
            let tool_expanded =
                matches!(item, Item::Tool { call_id, .. } if expanded_tools.contains(call_id));
            let fingerprint = fingerprint(item, prompt_expanded, tool_expanded);
            if self
                .rows
                .get(&row.index)
                .is_some_and(|entry| entry.0 == fingerprint)
            {
                continue;
            }
            let segments = segments(item, row.index, prompt_expanded, tool_expanded);
            self.rows.insert(row.index, (fingerprint, segments));
            changed = true;
        }
        if !changed {
            return None;
        }
        let retained: HashSet<_> = order.iter().copied().collect();
        self.rows.retain(|index, _| retained.contains(index));
        self.order = order;
        Some(
            self.order
                .iter()
                .flat_map(|index| self.rows[index].1.iter().cloned())
                .collect(),
        )
    }
}

fn fingerprint(item: &Item, prompt_expanded: bool, tool_expanded: bool) -> u64 {
    let mut state = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(item).hash(&mut state);
    prompt_expanded.hash(&mut state);
    tool_expanded.hash(&mut state);
    match item {
        Item::User(text) | Item::Assistant(text) | Item::Reasoning(text) | Item::Error(text) => {
            text.hash(&mut state)
        }
        Item::Tool {
            name,
            input,
            output,
            error,
            ..
        } => {
            name.hash(&mut state);
            input.hash(&mut state);
            output.hash(&mut state);
            error.hash(&mut state);
        }
        Item::BackgroundTask { label, summary, .. } => {
            label.hash(&mut state);
            summary.hash(&mut state);
        }
        Item::Stopped(notice) => {
            notice.detail.hash(&mut state);
            notice.failure.hash(&mut state);
        }
        Item::Image(_) | Item::ResponseStats(_) | Item::Todos(_) => {}
    }
    state.finish()
}

fn segments(item: &Item, index: usize, prompt_expanded: bool, tool_expanded: bool) -> Segments {
    let mut result = Vec::new();
    let mut push = |key: String, text: String| {
        if !text.is_empty() {
            result.push((key.into(), text.into()));
        }
    };
    match item {
        Item::User(text) => {
            let visible = if prompt_expanded {
                text.as_str()
            } else {
                prompt::compact_prompt(text).unwrap_or(text)
            };
            return markdown::selection_segments(visible, index, false, true);
        }
        Item::Assistant(text) => return markdown::selection_segments(text, index, false, false),
        Item::Reasoning(text) => return markdown::selection_segments(text, index, true, false),
        Item::Tool {
            name,
            input,
            output,
            error,
            ..
        } => {
            if name == "todo" {
                return result;
            }
            let files = diff_review::inline_result_files(
                name,
                input,
                if error.is_none() { output } else { "" },
            );
            if !files.is_empty() {
                // Native edit cards currently own their own leaf selection.
                // Keep their full metadata in document order when a selection
                // crosses the whole card, without pretending it has geometry.
                let mut text = format!("{name}\n{input}");
                if !output.is_empty() {
                    text.push('\n');
                    text.push_str(output);
                }
                if let Some(error) = error {
                    text.push('\n');
                    text.push_str(error);
                }
                push(format!("tool-edit-{index}"), text);
            } else {
                push(format!("tool-name-{index}"), name.clone());
                push(format!("tool-summary-{index}"), tool_summary(input));
                if tool_expanded {
                    push(
                        format!("tool-detail-{index}"),
                        tool_detail(name, input, output),
                    );
                }
                if let Some(error) = error {
                    push(
                        format!("tool-error-{index}"),
                        condense(&strip_ansi(error), 300),
                    );
                }
            }
        }
        Item::Error(message) => push(
            format!("{index}-error"),
            recovery::native_error_message(message),
        ),
        Item::Stopped(notice) => {
            if notice.failure {
                push(
                    format!("{index}-error"),
                    recovery::native_error_message(&notice.detail),
                );
            } else {
                push(format!("{index}-stop-reason"), notice.detail.clone());
            }
        }
        Item::BackgroundTask { label, summary, .. } => {
            push(format!("background-label-{index}"), label.clone());
            push(format!("background-summary-{index}"), summary.clone());
        }
        // Pinned todos and non-text chrome are outside the transcript document.
        Item::Todos(_) | Item::Image(_) | Item::ResponseStats(_) => {}
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn drag_observer_cancels_only_preexisting_momentum(cx: &mut gpui::TestAppContext) {
        let (panel, vcx) = cx.add_window_view(|_, cx| {
            let mut panel = Panel::new(
                "selection-momentum".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            );
            panel.items = vec![Item::Assistant("Select this text".into())];
            panel
        });
        vcx.run_until_parked();
        let bounds = vcx.debug_bounds("selectable-text-0-0").unwrap();
        panel.update(vcx, |panel, _| {
            panel.transcript_wheel_glide.remaining = 123.
        });
        vcx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: bounds.center(),
            modifiers: gpui::Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.run_until_parked();
        let selection = panel.read_with(vcx, |panel, _| {
            assert!(panel.transcript_dragging);
            assert!(!panel.stick_to_bottom);
            assert_eq!(panel.transcript_wheel_glide.remaining, 0.);
            panel.transcript_selection.clone()
        });
        // Notifications caused by extending a selection must not discard new
        // wheel input received after the drag began.
        panel.update(vcx, |panel, _| {
            panel.transcript_wheel_glide.remaining = 456.
        });
        selection.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| {
            assert_eq!(panel.transcript_wheel_glide.remaining, 456.);
        });
        selection.update(vcx, |selection, cx| {
            selection.finish();
            cx.notify();
        });
        vcx.run_until_parked();
        panel.read_with(vcx, |panel, _| assert!(!panel.transcript_dragging));
    }

    fn rows(items: &[Item]) -> Vec<TranscriptRenderRow> {
        items
            .iter()
            .enumerate()
            .map(|(index, _)| TranscriptRenderRow {
                index,
                source: TranscriptRowSource::Settled(index),
                role: None,
                show_label: false,
            })
            .collect()
    }

    fn tool() -> Item {
        Item::Tool {
            call_id: "call".into(),
            name: "bash".into(),
            input: r#"{"command":"printf hello"}"#.into(),
            output: "hidden output".into(),
            done: true,
            error: None,
        }
    }

    #[test]
    fn complete_document_caches_rows_and_excludes_collapsed_output() {
        let mut cache = TranscriptTextDocument::default();
        let items = vec![
            Item::User("Prompt".into()),
            tool(),
            Item::Assistant("Answer".into()),
        ];
        let rows = rows(&items);
        let prompts = HashSet::new();
        let mut tools = HashSet::new();
        let doc = cache.sync(&items, &rows, &prompts, &tools).unwrap();
        assert_eq!(doc.first().unwrap().0.as_ref(), "0-0");
        assert_eq!(doc.last().unwrap().0.as_ref(), "2-0");
        assert!(doc.iter().any(|(key, _)| key.as_ref() == "tool-summary-1"));
        assert!(!doc.iter().any(|(_, text)| text.contains("hidden output")));
        assert!(cache.sync(&items, &rows, &prompts, &tools).is_none());
        tools.insert("call".into());
        let expanded = cache.sync(&items, &rows, &prompts, &tools).unwrap();
        assert!(
            expanded.iter().any(
                |(key, text)| key.as_ref() == "tool-detail-1" && text.contains("hidden output")
            )
        );
        assert!(cache.sync(&items, &rows, &prompts, &tools).is_none());
        let trimmed = cache.sync(&items, &rows[..1], &prompts, &tools).unwrap();
        assert_eq!(trimmed.len(), 1);
        assert_eq!(cache.rows.len(), 1);
    }

    #[test]
    fn prompt_document_matches_collapsed_preview_and_expansion() {
        let text = format!("{}\nTAIL", "first line\n".repeat(8));
        let collapsed = segments(&Item::User(text.clone()), 0, false, false);
        assert!(!collapsed.iter().any(|(_, text)| text.contains("TAIL")));
        let expanded = segments(&Item::User(text), 0, true, false);
        assert!(expanded.iter().any(|(_, text)| text.contains("TAIL")));
    }

    #[test]
    fn edit_card_metadata_is_present_when_crossing_without_geometry() {
        let edit = Item::Tool {
            call_id: "edit".into(),
            name: "edit".into(),
            input: r#"{"file_path":"src/main.rs","old_string":"before","new_string":"after"}"#
                .into(),
            output: "Applied".into(),
            done: true,
            error: None,
        };
        let doc = segments(&edit, 5, false, false);
        assert_eq!(doc.len(), 1);
        assert_eq!(doc[0].0.as_ref(), "tool-edit-5");
        assert!(doc[0].1.contains("before"));
        assert!(doc[0].1.contains("after"));
        assert!(doc[0].1.contains("Applied"));
    }
}
