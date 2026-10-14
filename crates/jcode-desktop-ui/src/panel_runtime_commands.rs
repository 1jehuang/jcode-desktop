//! Runtime slash commands execute on the selected session's own connection.
use super::*;
use jcode_sdk::api::RuntimeCommand;

fn parse_runtime_command(content: &str) -> Option<(RuntimeCommand, Vec<String>)> {
    let mut words = content.split_whitespace();
    let command = match words.next()? {
        "/auth" => RuntimeCommand::Auth,
        "/logout" => RuntimeCommand::Logout,
        "/review" => RuntimeCommand::Review,
        "/judge" => RuntimeCommand::Judge,
        "/subagent" => RuntimeCommand::Subagent,
        "/skills" => RuntimeCommand::Skills,
        "/context" => RuntimeCommand::Context,
        "/info" => RuntimeCommand::Info,
        "/cache" => RuntimeCommand::Cache,
        "/memory" => RuntimeCommand::Memory,
        "/swarm" => RuntimeCommand::Swarm,
        "/agents" => RuntimeCommand::Agents,
        "/subagent-model" => RuntimeCommand::SubagentModel,
        "/autoreview" => RuntimeCommand::Autoreview,
        "/autojudge" => RuntimeCommand::Autojudge,
        "/transport" => RuntimeCommand::Transport,
        "/fast" => RuntimeCommand::Fast,
        "/telemetry" => RuntimeCommand::Telemetry,
        "/subscription" => RuntimeCommand::Subscription,
        "/provider-test-coverage" | "/model-status" => RuntimeCommand::ProviderTestCoverage,
        "/refresh" | "/refresh-model-list" => RuntimeCommand::RefreshModelList,
        "/continue" => RuntimeCommand::Continue,
        "/resumeall" | "/resume-all" => RuntimeCommand::ResumeAll,
        _ => return None,
    };
    Some((command, words.map(str::to_owned).collect()))
}

impl Panel {
    pub(super) fn handle_runtime_command(&mut self, content: &str, cx: &mut Context<Self>) -> bool {
        let Some((command, args)) = parse_runtime_command(content) else {
            return false;
        };
        if self.is_side_document() || self.preview_state.is_some() {
            self.items.push(Item::Error(
                "Runtime commands require a live session.".into(),
            ));
        } else {
            self.bridge.send(Command::RuntimeCommand {
                session_id: self.session_id.clone(),
                command,
                args,
            });
        }
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_all_runtime_commands_exactly() {
        for name in [
            "auth",
            "logout",
            "review",
            "judge",
            "subagent",
            "skills",
            "context",
            "info",
            "cache",
            "memory",
            "swarm",
            "agents",
            "subagent-model",
            "autoreview",
            "autojudge",
            "transport",
            "fast",
            "telemetry",
            "subscription",
            "provider-test-coverage",
            "model-status",
            "refresh-model-list",
            "refresh",
            "continue",
            "resumeall",
            "resume-all",
        ] {
            assert!(
                parse_runtime_command(&format!("/{name}")).is_some(),
                "{name}"
            );
            assert!(
                parse_runtime_command(&format!("/{name}-extra")).is_none(),
                "{name}"
            );
        }
        for content in ["skills", "/Skills", "/skill", "/help", "", "/memoryish"] {
            assert!(parse_runtime_command(content).is_none(), "{content}");
        }
    }

    #[test]
    fn aliases_route_to_identical_runtime_operations() {
        assert_eq!(
            parse_runtime_command("/auth").unwrap().0,
            RuntimeCommand::Auth
        );
        assert_eq!(
            parse_runtime_command("/logout").unwrap().0,
            RuntimeCommand::Logout
        );
        assert_eq!(
            parse_runtime_command("/review").unwrap().0,
            RuntimeCommand::Review
        );
        assert_eq!(
            parse_runtime_command("/judge").unwrap().0,
            RuntimeCommand::Judge
        );
        assert_eq!(
            parse_runtime_command("/subagent").unwrap().0,
            RuntimeCommand::Subagent
        );
        assert_eq!(
            parse_runtime_command("/provider-test-coverage"),
            parse_runtime_command("/model-status")
        );
        assert_eq!(
            parse_runtime_command("/resumeall"),
            parse_runtime_command("/resume-all")
        );
        assert_eq!(
            parse_runtime_command("/refresh"),
            parse_runtime_command("/refresh-model-list")
        );
        assert_eq!(
            parse_runtime_command("/refresh-model-list").unwrap().0,
            RuntimeCommand::RefreshModelList
        );
        assert_eq!(
            parse_runtime_command("/continue").unwrap().0,
            RuntimeCommand::Continue
        );
    }

    #[test]
    fn preserves_arguments_without_local_interpretation() {
        let (command, args) = parse_runtime_command("  /memory   search  remote/path  ").unwrap();
        assert_eq!(command, RuntimeCommand::Memory);
        assert_eq!(args, vec!["search", "remote/path"]);
        let (role, args) =
            parse_runtime_command("/review openai:model Check remote state").unwrap();
        assert_eq!(role, RuntimeCommand::Review);
        assert_eq!(args, vec!["openai:model", "Check", "remote", "state"]);
        assert_eq!(
            parse_runtime_command("/info").unwrap().1,
            Vec::<String>::new()
        );
    }
}
