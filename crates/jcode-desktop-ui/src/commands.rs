//! Slash-command catalog ported from the Jcode TUI. Keep command names and help in sync.

#[derive(Clone, Copy)]
pub(crate) struct RegisteredCommand {
    pub(crate) name: &'static str,
    pub(crate) help: &'static str,
    pub(crate) hidden: bool,
}

impl RegisteredCommand {
    const fn public(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            hidden: false,
        }
    }

    const fn remote(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            hidden: false,
        }
    }

    const fn hidden(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            hidden: true,
        }
    }
}

pub(crate) const REGISTERED_COMMANDS: &[RegisteredCommand] = &[
    RegisteredCommand::public("/help", "Show help and keyboard shortcuts"),
    RegisteredCommand::public("/?", "Show help and keyboard shortcuts"),
    RegisteredCommand::public("/commands", "Alias for /help"),
    RegisteredCommand::public("/model", "List or switch models"),
    RegisteredCommand::public("/models", "Alias for /model"),
    RegisteredCommand::public(
        "/provider-test-coverage",
        "Show live-test evidence for the current provider/model",
    ),
    RegisteredCommand::hidden("/model-status", "Alias for /provider-test-coverage"),
    RegisteredCommand::public("/refresh-model-list", "Refresh provider model catalogs"),
    RegisteredCommand::public("/agents", "Configure models for agent roles"),
    RegisteredCommand::public(
        "/swarm-prompt",
        "Open the active swarm routing prompt in your editor",
    ),
    RegisteredCommand::public("/subagent", "Launch a subagent manually"),
    RegisteredCommand::public("/observe", "Show the latest tool context in the side panel"),
    RegisteredCommand::public("/todos", "Show the session todo list as a card in the chat"),
    RegisteredCommand::hidden("/todo", "Alias for /todos"),
    RegisteredCommand::public("/splitview", "Mirror the current chat in the side panel"),
    RegisteredCommand::public("/split-view", "Alias for /splitview"),
    RegisteredCommand::public("/btw", "Ask a side question in the side panel"),
    RegisteredCommand::public("/ssh", "Connect to a remote machine using system SSH"),
    RegisteredCommand::public("/git", "Show git status for the session working directory"),
    RegisteredCommand::public("/colors", "List, configure, and score every TUI color"),
    RegisteredCommand::hidden("/color", "Alias for /colors"),
    RegisteredCommand::public("/hotkeys", "List hotkeys with your personal usage"),
    RegisteredCommand::public("/terminal-setup", "Fix Shift+Enter newlines"),
    RegisteredCommand::public("/commit", "Make logical commits from current changes"),
    RegisteredCommand::public(
        "/commit-push",
        "Make logical commits from current changes, then push",
    ),
    RegisteredCommand::hidden("/commit-and-push", "Alias for /commit-push"),
    RegisteredCommand::public(
        "/fast-release",
        "Publish Linux immediately from the warm selfdev cache; CI adds other platforms",
    ),
    RegisteredCommand::public(
        "/fast-macos-release",
        "Publish a prepared macOS arm64 build immediately; CI adds other platforms",
    ),
    RegisteredCommand::public("/remote", "Reach this session from another machine"),
    RegisteredCommand::public(
        "/remote-release",
        "Push the release tag immediately; CI builds and publishes every platform",
    ),
    RegisteredCommand::hidden("/cut-release", "Alias for /fast-release"),
    RegisteredCommand::hidden("/commit-push-release", "Alias for /cut-release"),
    RegisteredCommand::public(
        "/triage",
        "Triage new GitHub issues and autonomously fix the safe ones",
    ),
    RegisteredCommand::public("/transcript", "Open the current session transcript file"),
    RegisteredCommand::public("/subagent-model", "Show/change subagent model policy"),
    RegisteredCommand::public("/autoreview", "Show/toggle automatic end-of-turn review"),
    RegisteredCommand::public("/autojudge", "Show/toggle automatic end-of-turn judging"),
    RegisteredCommand::public("/review", "Launch a one-shot headed review session"),
    RegisteredCommand::public("/judge", "Launch a one-shot headed judge session"),
    RegisteredCommand::public(
        "/effort",
        "Set reasoning effort (none/minimal/low/medium/high/xhigh/max)",
    ),
    RegisteredCommand::public("/fast", "Toggle fast mode"),
    RegisteredCommand::public("/transport", "Show/change connection transport"),
    RegisteredCommand::public("/alignment", "Show/change default text alignment"),
    RegisteredCommand::public(
        "/compact-notifications",
        "Show/toggle single-line swarm/file-activity notifications",
    ),
    RegisteredCommand::public(
        "/show-agentgrep-output",
        "Show/toggle full agentgrep search output inline in chat",
    ),
    RegisteredCommand::public(
        "/tool-call-details",
        "Show/toggle dimmed technical details on tool rows with an intent",
    ),
    RegisteredCommand::public(
        "/thinking-display",
        "Show/hide the model's thinking text (off/full/current)",
    ),
    RegisteredCommand::hidden("/thinking", "Alias for /thinking-display"),
    RegisteredCommand::hidden("/reasoning", "Alias for /thinking-display"),
    RegisteredCommand::public("/cancel", "Cancel the current prompt or operation"),
    RegisteredCommand::public("/clear", "Clear conversation history"),
    RegisteredCommand::public("/cls", "Clear the view only, keeping context"),
    RegisteredCommand::hidden("/clear-view", "Alias for /cls"),
    RegisteredCommand::public("/rewind", "Rewind conversation to previous message"),
    RegisteredCommand::public("/poke", "Poke model to resume with incomplete todos"),
    RegisteredCommand::public("/plan", "Create a plan-only response as a plan card"),
    RegisteredCommand::public("/improve", "Autonomously improve the repository"),
    RegisteredCommand::public("/refactor", "Run a safe refactor loop"),
    RegisteredCommand::public("/compact", "Compact context"),
    RegisteredCommand::public("/fix", "Recover when the model cannot continue"),
    RegisteredCommand::public("/dictate", "Run configured external dictation command"),
    RegisteredCommand::public("/dictation", "Alias for /dictate"),
    RegisteredCommand::public("/memory", "Toggle memory feature"),
    RegisteredCommand::public("/test", "Verify a claim/current changes with layered tests"),
    RegisteredCommand::public(
        "/initiatives",
        "Open initiatives overview / resume tracked initiatives",
    ),
    RegisteredCommand::public("/goals", "Legacy alias for /initiatives"),
    RegisteredCommand::public("/swarm", "Toggle swarm feature"),
    RegisteredCommand::public("/overnight", "Run a supervised overnight coordinator"),
    RegisteredCommand::public("/context", "Show the full session context snapshot"),
    RegisteredCommand::public(
        "/skills",
        "Show loaded skills and jcode-endorsed recommendations",
    ),
    RegisteredCommand::public("/version", "Show current version"),
    RegisteredCommand::public("/changelog", "Show recent changes in this build"),
    RegisteredCommand::public("/info", "Show session info and tokens"),
    RegisteredCommand::public("/usage", "Show connected provider usage limits"),
    RegisteredCommand::public(
        "/productivity",
        "Generate a shareable usage report + dashboard image",
    ),
    RegisteredCommand::public("/wrapped", "Alias for /productivity"),
    RegisteredCommand::public("/feedback", "Send feedback about jcode"),
    RegisteredCommand::public("/telemetry", "Show or change what jcode sends"),
    RegisteredCommand::public("/support", "Email support with diagnostics prefilled"),
    RegisteredCommand::public("/subscription", "Show jcode subscription status"),
    RegisteredCommand::public("/subscribe", "Why and how to subscribe to jcode"),
    RegisteredCommand::public("/config", "Show or edit configuration"),
    RegisteredCommand::public("/log", "Mark the current location in the jcode logs"),
    RegisteredCommand::public(
        "/keys",
        "Show keybinding conflicts with your terminal and OS (/keys refresh to rescan)",
    ),
    RegisteredCommand::hidden("/keybindings", "Alias for /keys"),
    RegisteredCommand::public(
        "/diff",
        "Cycle or set diff display mode (off/inline/full/pinned/file)",
    ),
    RegisteredCommand::public(
        "/onboarding-preview",
        "Preview the first-run onboarding screen",
    ),
    RegisteredCommand::public(
        "/onboarding-sim",
        "Walk through every first-run onboarding screen (Alt+5 reset, Cmd+5 toggle)",
    ),
    RegisteredCommand::public("/reload", "Reload into newest available binary"),
    RegisteredCommand::public("/restart", "Restart with current binary"),
    RegisteredCommand::public("/rebuild", "Background rebuild and auto reload"),
    RegisteredCommand::public("/selfdev", "Open a new self-dev jcode session"),
    RegisteredCommand::public("/update", "Background update and auto reload"),
    RegisteredCommand::public("/resume", "Open session picker"),
    RegisteredCommand::public("/sessions", "Alias for /resume"),
    RegisteredCommand::public("/session", "Alias for /resume"),
    RegisteredCommand::public("/active", "Manage live sessions (working vs ready)"),
    RegisteredCommand::public("/catchup", "Open Catch Up picker"),
    RegisteredCommand::public("/back", "Return to the previous Catch Up session"),
    RegisteredCommand::public("/save", "Bookmark session for easy access"),
    RegisteredCommand::public("/unsave", "Remove bookmark from session"),
    RegisteredCommand::public("/rename", "Rename current session"),
    RegisteredCommand::public("/fork", "Fork session into a new window (optional prompt)"),
    RegisteredCommand::hidden("/split", "Alias for /fork"),
    RegisteredCommand::public("/transfer", "Compact context into a fresh handoff session"),
    RegisteredCommand::public("/workspace", "Niri-style session workspace"),
    RegisteredCommand::public("/quit", "Exit jcode"),
    RegisteredCommand::public("/auth", "Show authentication status"),
    RegisteredCommand::public("/login", "Login to a provider"),
    RegisteredCommand::public("/logout", "Log out of a provider"),
    RegisteredCommand::public("/account", "Open the combined account picker"),
    RegisteredCommand::public("/accounts", "Alias for /account"),
    RegisteredCommand::public("/cache", "Show cache stats or set cache TTL"),
    RegisteredCommand::public("/debug-visual", "Toggle visual debug overlay"),
    RegisteredCommand::public("/screenshot-mode", "Toggle screenshot capture mode"),
    RegisteredCommand::public("/screenshot", "Capture a screenshot debug state"),
    RegisteredCommand::public("/record", "Record a demo capture"),
    RegisteredCommand::remote("/client-reload", "Force reload client binary"),
    RegisteredCommand::remote("/server-reload", "Force reload server binary"),
    RegisteredCommand::remote(
        "/continue",
        "Continue every interrupted live session that would auto-resume",
    ),
    RegisteredCommand::remote("/resumeall", "Alias for /continue"),
    RegisteredCommand::hidden("/resume-all", "Alias for /continue"),
    RegisteredCommand::hidden("/z", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zzz", "Secret premium-mode command"),
    RegisteredCommand::hidden("/zstatus", "Secret premium-mode status command"),
];

/// Every non-hidden slash command with its one-line description, in
/// registration order. The `/help` overlay uses this to list commands its
/// hand-written sections have not covered, so a newly registered command can
/// never be invisible to users.
pub(crate) fn registered_command_entries() -> impl Iterator<Item = (&'static str, &'static str)> {
    REGISTERED_COMMANDS
        .iter()
        .filter(|command| !command.hidden)
        .map(|command| (command.name, command.help))
}

pub(crate) fn registered_command(name: &str) -> Option<RegisteredCommand> {
    REGISTERED_COMMANDS
        .iter()
        .copied()
        .find(|command| command.name == name)
}

pub(crate) fn help_markdown() -> String {
    let mut help = String::from("**Commands**\n\n");
    for (name, description) in registered_command_entries() {
        help.push_str(&format!("- `{name}` — {description}\n"));
    }
    help
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_catalog_contains_the_full_tui_command_surface() {
        assert!(REGISTERED_COMMANDS.len() >= 100);
        for command in [
            "/help", "/model", "/agents", "/commit", "/effort", "/clear", "/rewind", "/compact",
            "/test", "/resume", "/rename", "/auth", "/quit",
        ] {
            assert!(registered_command(command).is_some(), "missing {command}");
        }
        assert!(!registered_command_entries().any(|(name, _)| name == "/zzz"));
    }
}
