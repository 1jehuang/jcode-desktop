//! Panel-owned helpers for workspace commands. This module never imports workspace.
use super::*;
#[path = "workspace_command_parse.rs"]
pub(crate) mod parse;
use crate::ui_actions::WorkspaceSlashCommand;
use jcode_sdk::SidePanelPage;

impl Panel {
    /// Call from the submit callback before pending-session queuing and the legacy
    /// resume shortcut. A Window is necessary for normal GPUI action routing.
    pub(crate) fn dispatch_workspace_command(
        panel: &Entity<Self>,
        content: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        if parse::parse(content).is_none() {
            return false;
        }
        if panel.read(cx).preview_state.is_some() || panel.read(cx).is_side_document() {
            return false;
        }
        window.dispatch_action(
            Box::new(WorkspaceSlashCommand {
                source: panel.entity_id(),
                content: content.into(),
            }),
            cx,
        );
        true
    }

    pub(crate) fn workspace_command_notice(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.items.push(Item::Assistant(message.into()));
        self.transcript_measurements.dirty = true;
        cx.notify();
    }

    pub(crate) fn workspace_command_page(&self, observe: bool) -> SidePanelPage {
        let mut content = String::new();
        if observe {
            if let Some(Item::Tool {
                name,
                input,
                output,
                error,
                ..
            }) = self
                .items
                .iter()
                .rev()
                .find(|item| matches!(item, Item::Tool { .. }))
            {
                content = format!(
                    "# {name}\n\n## Input\n\n{}\n\n## Output\n\n{}",
                    fenced(input),
                    fenced(output)
                );
                if let Some(error) = error {
                    content.push_str(&format!("\n\n## Error\n\n{}", fenced(error)));
                }
            } else {
                content.push_str("Waiting for the next tool call…");
            }
        } else {
            for item in self.items.iter() {
                match item {
                    Item::User(text) => content.push_str(&format!("## You\n\n{text}\n\n")),
                    Item::Assistant(text) => {
                        content.push_str(&format!("## Assistant\n\n{text}\n\n"))
                    }
                    Item::Reasoning(text) => {
                        content.push_str(&format!("## Reasoning\n\n{text}\n\n"))
                    }
                    Item::Tool {
                        name,
                        input,
                        output,
                        ..
                    } => content.push_str(&format!(
                        "### Tool: {name}\n\n{}\n\n{}\n\n",
                        fenced(input),
                        fenced(output)
                    )),
                    Item::Error(text) => content.push_str(&format!("## Error\n\n{text}\n\n")),
                    Item::Image(_) => content.push_str("*[Image in source conversation]*\n\n"),
                    Item::Todos(payload) => content.push_str(&format!(
                        "## Todos\n\n{}\n\n",
                        fenced(&serde_json::to_string_pretty(payload).unwrap_or_default())
                    )),
                    _ => {}
                }
            }
            if !self.streaming_reasoning.is_empty() {
                content.push_str(&format!("## Reasoning\n\n{}\n\n", self.streaming_reasoning));
            }
            if !self.streaming_text.is_empty() {
                content.push_str(&format!("## Assistant\n\n{}", self.streaming_text));
            }
        }
        SidePanelPage {
            id: if observe {
                "desktop-observe"
            } else {
                "desktop-splitview"
            }
            .into(),
            title: if observe {
                "Observe tool context"
            } else {
                "Live conversation mirror"
            }
            .into(),
            file_path: String::new(),
            format: Default::default(),
            source: Default::default(),
            content,
            pdf_data: None,
            updated_at_ms: 0,
        }
    }

    /// The source observation is scoped to the document entity's lifetime.
    /// Updating the document does not reset its independent ScrollHandle.
    pub(crate) fn observe_workspace_source(
        &mut self,
        source: Entity<Panel>,
        observe: bool,
        cx: &mut Context<Self>,
    ) {
        cx.observe(&source, move |this, source, cx| {
            let page = source.read(cx).workspace_command_page(observe);
            this.update_side_document(&page, cx);
        })
        .detach();
    }
}

fn fenced(text: &str) -> String {
    let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{text}\n{fence}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn tool_output_cannot_escape_code_fence() {
        assert_eq!(
            super::fenced("```\n# heading"),
            "````\n```\n# heading\n````"
        );
    }
}
