//! Pure parser shared by the workspace action and headless regression tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceCommand {
    Observe(String),
    Mirror(String),
    Fork(Option<String>),
    Machines(Option<String>),
    Remote(String),
    ServerReload,
    Resume { active: bool, query: String },
    Catchup(bool),
    Back,
    Workspace,
    Quit,
    Reload,
    Restart,
    Selfdev(String),
    Transfer,
    Publish,
    Invalid(&'static str),
}

pub(crate) fn parse(content: &str) -> Option<WorkspaceCommand> {
    use WorkspaceCommand::*;
    let content = content.trim();
    let split = content.find(char::is_whitespace).unwrap_or(content.len());
    let (name, tail) = content.split_at(split);
    let tail = tail.trim();
    Some(match name {
        "/observe" => Observe(tail.into()),
        "/splitview" | "/split-view" => Mirror(tail.into()),
        "/btw" if tail.is_empty() => Invalid("Usage: /btw <side question>"),
        "/btw" | "/fork" | "/split" => Fork((!tail.is_empty()).then(|| tail.into())),
        "/ssh" => Machines((!tail.is_empty()).then(|| tail.into())),
        "/resume" | "/active" => Resume {
            active: name == "/active",
            query: tail.into(),
        },
        "/catchup" if matches!(tail, "" | "list" | "next") => Catchup(tail == "next"),
        "/catchup" => Invalid("Usage: /catchup [list|next]"),
        "/remote" => Remote(tail.into()),
        "/transfer" if tail.is_empty() => Transfer,
        "/transfer" => Invalid("Usage: /transfer"),
        "/server-reload" if tail.is_empty() => ServerReload,
        "/server-reload" => Invalid("Usage: /server-reload"),
        "/selfdev" => Selfdev(tail.into()),
        "/back" if tail.is_empty() => Back,
        "/workspace" if tail.is_empty() => Workspace,
        "/quit" if tail.is_empty() => Quit,
        "/reload" | "/rebuild" | "/client-reload" if tail.is_empty() => Reload,
        "/restart" if tail.is_empty() => Restart,
        "/fast-release"
        | "/fast-macos-release"
        | "/remote-release"
        | "/cut-release"
        | "/commit-push-release"
            if tail.is_empty() =>
        {
            Publish
        }
        "/back"
        | "/workspace"
        | "/quit"
        | "/reload"
        | "/rebuild"
        | "/client-reload"
        | "/restart"
        | "/fast-release"
        | "/fast-macos-release"
        | "/remote-release"
        | "/cut-release"
        | "/commit-push-release" => Invalid("This Desktop command does not accept arguments."),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_names_only() {
        for command in ["/forked", "/sshfoo", "/resume-me", "hello", ""] {
            assert_eq!(parse(command), None);
        }
    }
    #[test]
    fn whitespace_and_aliases() {
        assert_eq!(
            parse(" /split-view\ton \n"),
            Some(WorkspaceCommand::Mirror("on".into()))
        );
        assert_eq!(parse("/split question"), parse("/fork question"));
        assert_eq!(parse("/client-reload"), parse("/rebuild"));
        assert_eq!(
            parse("/active query"),
            Some(WorkspaceCommand::Resume {
                active: true,
                query: "query".into()
            })
        );
    }
    #[test]
    fn invalid_arguments_are_not_prompts() {
        assert!(matches!(
            parse("/quit now"),
            Some(WorkspaceCommand::Invalid(_))
        ));
        assert!(matches!(parse("/btw"), Some(WorkspaceCommand::Invalid(_))));
        assert_eq!(parse("/transfer"), Some(WorkspaceCommand::Transfer));
    }
    #[test]
    fn every_requested_name_is_owned() {
        for name in [
            "observe",
            "splitview",
            "split-view",
            "btw",
            "ssh",
            "remote",
            "resume",
            "active",
            "catchup",
            "back",
            "fork",
            "split",
            "transfer",
            "workspace",
            "quit",
            "reload",
            "restart",
            "rebuild",
            "selfdev",
            "client-reload",
            "server-reload",
            "fast-release",
            "fast-macos-release",
            "remote-release",
            "cut-release",
            "commit-push-release",
        ] {
            assert!(parse(&format!("/{name}")).is_some(), "{name}");
        }
    }
}
