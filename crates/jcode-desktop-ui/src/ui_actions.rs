//! Actions that views dispatch to their containers.
//!
//! Panels and the composer ask the workspace to open or close things by
//! dispatching these. They live here, not in `workspace`, so `panel` and
//! `input` do not depend on `workspace` (which depends on both). Keeping the
//! view layers acyclic is what lets them move into separate crates later.
//! `workspace` and `panel::shortcuts` re-export them under their old paths.
//!
//! Action names are unchanged: unit actions keep their `workspace` and
//! `panel_shortcuts` namespaces, and the data actions are un-namespaced.

use std::sync::Arc;

/// Route Desktop-local slash commands to the workspace containing the source.
/// Raw input is retained so validation never silently drops command arguments.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct WorkspaceSlashCommand {
    pub source: gpui::EntityId,
    pub content: String,
}

/// Open the Accounts panel, optionally starting a login command.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct OpenAccounts {
    pub source: gpui::EntityId,
    pub login_command: Option<String>,
}

/// Open the read-only usage reset review for one provider, from `/reset`.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct OpenUsageReset {
    pub provider: crate::accounts::ResetProvider,
}

/// Commit, push, release and publish Desktop from the source panel's checkout.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct PublishDesktop {
    pub source: gpui::EntityId,
}

/// Open the change review for one tool call's proposed edits.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct OpenChangeReview {
    pub source: gpui::EntityId,
    // Each file header and its click handler share one tool-call payload.
    pub name: Arc<str>,
    pub input: Arc<str>,
    pub output: Arc<str>,
    pub selected: usize,
    pub done: bool,
    pub failed: bool,
}

impl OpenChangeReview {
    pub(crate) fn for_file(&self, selected: usize) -> Self {
        Self {
            selected,
            ..self.clone()
        }
    }
}

#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub(crate) struct CloseChangeReview {
    pub panel: gpui::EntityId,
}

gpui::actions!(
    workspace,
    [
        OpenAppletShowcase,
        ClosePanel,
        ToggleOnboardingSimulator,
        OpenChangelog,
        OpenResume,
    ]
);

gpui::actions!(
    panel_shortcuts,
    [JumpToLatest, JumpToLatestIfEmpty, BackgroundRunningTool]
);

/// Alt+B/Ctrl+B move the running tool to the background, matching the TUI.
/// Both chords are also composer word-motion keys, so the handler propagates
/// when no tool is running. Registered from `input::bind_keys` right after the
/// composer's own bindings so it outranks them at the same context depth.
pub(crate) fn background_tool_bindings() -> [gpui::KeyBinding; 4] {
    [
        gpui::KeyBinding::new("alt-b", BackgroundRunningTool, Some("ChatPanel")),
        gpui::KeyBinding::new("ctrl-b", BackgroundRunningTool, Some("ChatPanel")),
        gpui::KeyBinding::new(
            "alt-b",
            BackgroundRunningTool,
            Some("ChatPanel > PromptInput"),
        ),
        gpui::KeyBinding::new(
            "ctrl-b",
            BackgroundRunningTool,
            Some("ChatPanel > PromptInput"),
        ),
    ]
}
