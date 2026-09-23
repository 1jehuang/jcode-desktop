//! Launch identity shared by the native host and reloadable UI.
use std::ffi::OsStr;

/// Socket identity of the shared single-panel host.
pub const SHARED_SINGLE_PANEL: &str = "single-panel";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LaunchMode {
    #[default]
    Workspace,
    NoSidebar,
    SinglePanel,
}

impl LaunchMode {
    /// Single-panel takes precedence regardless of flag order.
    pub fn from_args(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Self {
        let mut mode = Self::Workspace;
        for arg in args {
            if arg.as_ref() == "--single-panel" || arg.as_ref() == "--resume" {
                return Self::SinglePanel;
            }
            if arg.as_ref() == "--no-sidebar" || arg.as_ref() == "--workspace" {
                mode = Self::NoSidebar;
            }
        }
        mode
    }

    /// Open the session picker only for an explicit fresh resume launch.
    pub fn resume_requested(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> bool {
        args.into_iter().any(|arg| arg.as_ref() == "--resume")
    }

    /// Session to open directly on a fresh launch, from `--session=<id>`.
    pub fn requested_session(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Option<String> {
        args.into_iter().find_map(|arg| {
            let id = arg.as_ref().to_str()?.strip_prefix("--session=")?.trim();
            (!id.is_empty()).then(|| id.to_string())
        })
    }

    /// Auxiliary windows never acquire the main instance's global shortcut.
    pub fn instance_name(self, pid: u32) -> Option<String> {
        match self {
            Self::Workspace => None,
            Self::NoSidebar => Some("no-sidebar".into()),
            Self::SinglePanel => Some(format!("single-panel-{pid}")),
        }
    }

    /// Single-panel windows share one host process unless `--new-process`
    /// asks for an isolated one. The shared host owns a stable socket name
    /// that later launches forward their own window request to.
    pub fn shared_single_panel(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> bool {
        let mut single_panel = false;
        for arg in args {
            if arg.as_ref() == "--new-process" {
                return false;
            }
            single_panel |= arg.as_ref() == "--single-panel" || arg.as_ref() == "--resume";
        }
        single_panel
    }

    /// Instance socket identity for a launch, including the shared host.
    pub fn instance_name_for_args(
        args: impl IntoIterator<Item = impl AsRef<OsStr>> + Clone,
        pid: u32,
    ) -> Option<String> {
        if Self::shared_single_panel(args.clone()) {
            return Some(SHARED_SINGLE_PANEL.into());
        }
        Self::from_args(args).instance_name(pid)
    }

    pub fn initial_window_size(self) -> (u32, u32) {
        match self {
            Self::SinglePanel => (800, 950),
            Self::Workspace | Self::NoSidebar => (1500, 950),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_flags_preserve_existing_modes_and_single_panel_precedence() {
        assert_eq!(LaunchMode::from_args(["desktop"]), LaunchMode::Workspace);
        for alias in ["--no-sidebar", "--workspace"] {
            assert_eq!(LaunchMode::from_args([alias]), LaunchMode::NoSidebar);
            for args in [[alias, "--single-panel"], ["--single-panel", alias]] {
                assert_eq!(LaunchMode::from_args(args), LaunchMode::SinglePanel);
            }
        }
    }

    #[test]
    fn resume_launch_is_standalone_in_any_flag_order() {
        for args in [
            ["--resume", "--workspace"],
            ["--no-sidebar", "--resume"],
            ["--single-panel", "--resume"],
        ] {
            assert_eq!(LaunchMode::from_args(args), LaunchMode::SinglePanel);
            assert!(LaunchMode::resume_requested(args));
        }
        assert!(!LaunchMode::resume_requested([
            "--single-panel",
            "--resume-other"
        ]));
    }

    #[test]
    fn session_flag_names_the_session_to_open() {
        assert_eq!(
            LaunchMode::requested_session(["--single-panel", "--session=session_a"]).as_deref(),
            Some("session_a")
        );
        assert_eq!(LaunchMode::requested_session(["--session="]), None);
        assert_eq!(LaunchMode::requested_session(["--session"]), None);
        assert_eq!(LaunchMode::requested_session(["--resume"]), None);
    }

    #[test]
    fn single_panel_identity_is_process_specific() {
        assert_eq!(LaunchMode::Workspace.instance_name(10), None);
        assert_eq!(
            LaunchMode::NoSidebar.instance_name(10).as_deref(),
            Some("no-sidebar")
        );
        assert_eq!(
            LaunchMode::SinglePanel.instance_name(10).as_deref(),
            Some("single-panel-10")
        );
        assert_ne!(
            LaunchMode::SinglePanel.instance_name(10),
            LaunchMode::SinglePanel.instance_name(11)
        );
    }

    #[test]
    fn single_panel_windows_share_a_host_unless_isolated() {
        assert!(LaunchMode::shared_single_panel(["--single-panel"]));
        assert!(LaunchMode::shared_single_panel(["--resume"]));
        assert!(!LaunchMode::shared_single_panel(["--single-panel", "--new-process"]));
        assert!(!LaunchMode::shared_single_panel(["--new-process", "--single-panel"]));
        assert!(!LaunchMode::shared_single_panel(["--no-sidebar"]));
        assert!(!LaunchMode::shared_single_panel(Vec::<&str>::new()));
        assert_eq!(
            LaunchMode::instance_name_for_args(["--single-panel"], 7).as_deref(),
            Some(SHARED_SINGLE_PANEL)
        );
        assert_eq!(
            LaunchMode::instance_name_for_args(["--single-panel", "--new-process"], 7).as_deref(),
            Some("single-panel-7")
        );
        assert_eq!(LaunchMode::instance_name_for_args(["--workspace"], 7).as_deref(), Some("no-sidebar"));
        assert_eq!(LaunchMode::instance_name_for_args(Vec::<&str>::new(), 7), None);
    }

    #[test]
    fn single_panel_uses_chat_sized_window() {
        assert_eq!(LaunchMode::SinglePanel.initial_window_size(), (800, 950));
        assert_eq!(LaunchMode::Workspace.initial_window_size(), (1500, 950));
        assert_eq!(LaunchMode::NoSidebar.initial_window_size(), (1500, 950));
    }
}
