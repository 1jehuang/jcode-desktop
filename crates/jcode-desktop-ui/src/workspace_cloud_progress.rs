//! A per-request checklist, driven by observed helper/SSH phases, not a timer.
//! A later phase proves earlier phases finished. Unknown messages never do.
use gpui::{InteractiveElement, IntoElement, ParentElement, Styled, div, px};

use crate::theme::Theme;

const LABELS: [&str; 5] = [
    "AWS access",
    "Runtime guard and allowance",
    "Shared VM running",
    "Private SSH connection",
    "Jcode session",
];

#[derive(Clone, Default, Debug)]
pub(super) struct Progress {
    phase: Option<usize>,
    failed: bool,
    reused: bool,
}

impl Progress {
    pub(super) fn observe(&mut self, message: &str, failed: bool) {
        // Wake and SSH results use separate channels. Late helper phases must
        // not regress a completed SSH phase or erase a terminal failure.
        if self.failed {
            return;
        }
        for line in message.lines() {
            let line = line.to_ascii_lowercase();
            let next = if line.contains("reusing recently verified cloud vm") {
                self.reused = true;
                Some(3)
            } else if line.contains("creating jcode session") {
                Some(4)
            } else if !failed
                && (line.starts_with("connected to ") || line.contains(": connected to "))
            {
                Some(5)
            } else if line.contains("private ssm connection")
                || line.contains("verifying ssh connection")
                || line.contains("over ssh")
                || line.contains("is awake. connecting")
            {
                Some(3)
            } else if line.contains("starting the shared cloud vm")
                || line.contains("shared cloud vm is starting")
                || line.contains("shared cloud vm is already running")
                || line.contains("starting your cloud virtual machine")
            {
                Some(2)
            } else if line.contains("checking cloud runtime guard and allowance") {
                Some(1)
            } else if line.contains("checking aws sign-in") {
                Some(0)
            } else {
                None
            };
            if let Some(next) = next {
                self.phase = Some(self.phase.map_or(next, |current| current.max(next)));
            }
        }
        if failed {
            self.failed = true;
            // A configuration/access failure can happen before the first
            // helper phase. Keep later rows uncompleted in that case.
            self.phase.get_or_insert(0);
        }
    }

    fn row(&self, index: usize) -> (&'static str, &'static str) {
        match self.phase {
            Some(phase) if index < phase => (
                "✓",
                if self.reused && index < 3 {
                    "Recently verified"
                } else {
                    "Done"
                },
            ),
            Some(phase) if index == phase && self.failed => ("!", "Failed"),
            Some(phase) if index == phase => ("●", "In progress"),
            _ => ("○", "Waiting"),
        }
    }

    pub(super) fn render(&self) -> gpui::AnyElement {
        let theme = Theme::global();
        div()
            .debug_selector(|| "cloud-startup-checklist".into())
            .flex()
            .flex_col()
            .gap_1()
            .children(LABELS.iter().enumerate().map(|(index, label)| {
                let (marker, state) = self.row(index);
                let color = match state {
                    "Failed" => theme.ERROR,
                    "In progress" => theme.TEXT,
                    "Done" | "Recently verified" => theme.OK,
                    _ => theme.TEXT_DIM,
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(color)
                    .child(div().w(px(14.)).flex_none().child(marker))
                    .child(*label)
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.TEXT_DIM)
                            .child(state),
                    )
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_advances_only_with_observed_phases() {
        let mut progress = Progress::default();
        assert_eq!(progress.row(0), ("○", "Waiting"));
        progress.observe("Waiting for shared VM jcode-cloud-alpha…", false);
        assert!(progress.phase.is_none());
        for (phase, message) in [
            "Checking AWS sign-in...",
            "Checking cloud runtime guard and allowance...",
            "Starting the shared cloud VM...",
            "Waiting for cloud host and private SSM connection...",
            "SSH connected. Creating Jcode session...",
            "Connected to jcode-cloud-alpha",
        ]
        .into_iter()
        .enumerate()
        {
            progress.observe(message, false);
            assert_eq!(progress.phase, Some(phase));
            for earlier in 0..phase.min(5) {
                assert_eq!(progress.row(earlier), ("✓", "Done"));
            }
            if phase < 5 {
                assert_eq!(progress.row(phase), ("●", "In progress"));
            }
        }
    }

    #[test]
    fn checklist_failure_retains_finished_steps_and_ignores_late_progress() {
        let mut progress = Progress::default();
        progress.observe(
            "Verifying SSH connection and cloud bootstrap readiness...",
            false,
        );
        progress.observe("Remote session failed: connection refused", true);
        assert_eq!(progress.row(2), ("✓", "Done"));
        assert_eq!(progress.row(3), ("!", "Failed"));
        assert_eq!(progress.row(4), ("○", "Waiting"));
        progress.observe("Checking AWS sign-in...", false);
        progress.observe("Connected to jcode-cloud-alpha", false);
        assert_eq!(progress.row(3), ("!", "Failed"));
    }

    #[test]
    fn checklist_warm_path_labels_recent_checks_honestly() {
        let mut progress = Progress::default();
        progress.observe("Reusing recently verified cloud VM...", false);
        assert_eq!(progress.row(0), ("✓", "Recently verified"));
        assert_eq!(progress.row(2), ("✓", "Recently verified"));
        assert_eq!(progress.row(3), ("●", "In progress"));
        assert_eq!(progress.row(4), ("○", "Waiting"));
        progress.observe("SSH connected. Creating Jcode session...", false);
        progress.observe("jcode-cloud-alpha is awake. Connecting…", false);
        assert_eq!(progress.row(4), ("●", "In progress"));
    }

    #[test]
    fn checklist_early_error_does_not_claim_vm_or_access_ready() {
        let mut progress = Progress::default();
        progress.observe("Personal cloud is not configured", true);
        assert_eq!(progress.row(0), ("!", "Failed"));
        assert_eq!(progress.row(2), ("○", "Waiting"));
        assert_eq!(progress.row(4), ("○", "Waiting"));
    }
}
