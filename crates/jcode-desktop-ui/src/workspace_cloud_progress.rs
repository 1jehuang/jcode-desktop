//! A per-request checklist, driven by observed helper/SSH phases, not a timer.
//! A later phase proves earlier phases finished. Unknown messages never do.
use gpui::{InteractiveElement, IntoElement, ParentElement, Styled, div, px};
use std::time::{Duration, Instant};

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
    started: Option<Instant>,
    phase_started: Option<Instant>,
    elapsed: Duration,
    phase_elapsed: Duration,
}

impl Progress {
    pub(super) fn observe(&mut self, message: &str, failed: bool) {
        self.observe_at(message, failed, Instant::now());
    }

    fn observe_at(&mut self, message: &str, failed: bool, now: Instant) {
        // Wake and SSH results use separate channels. Late helper phases must
        // not regress a completed SSH phase or erase a terminal failure.
        if self.failed || self.phase == Some(LABELS.len()) {
            return;
        }
        self.started.get_or_insert(now);
        self.tick_at(now);
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
                || line.contains("ssh bootstrap ready")
            {
                Some(3)
            } else if line.contains("starting the shared cloud vm")
                || line.contains("shared cloud vm is starting")
                || line.contains("shared cloud vm is already running")
                || line.contains("starting your cloud virtual machine")
                || line.contains("waiting for shared cloud vm to become reachable")
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
                if self.phase.is_none_or(|current| next > current) {
                    self.phase = Some(next);
                    self.phase_started = Some(now);
                    self.phase_elapsed = Duration::ZERO;
                }
            }
        }
        if failed {
            self.failed = true;
            // A configuration/access failure can happen before the first
            // helper phase. Keep later rows uncompleted in that case.
            self.phase.get_or_insert(0);
        }
    }

    /// Refresh text once per second, without ever advancing a checklist step.
    pub(super) fn tick(&mut self) -> bool {
        self.tick_at(Instant::now())
    }

    fn tick_at(&mut self, now: Instant) -> bool {
        if self.failed || self.phase == Some(LABELS.len()) {
            return false;
        }
        let previous = (self.elapsed.as_secs(), self.phase_elapsed.as_secs());
        if let Some(started) = self.started {
            self.elapsed = now.saturating_duration_since(started);
        }
        if let Some(started) = self.phase_started {
            self.phase_elapsed = now.saturating_duration_since(started);
        }
        previous != (self.elapsed.as_secs(), self.phase_elapsed.as_secs())
    }

    fn timing_label(&self) -> String {
        if self.failed {
            format!("Stopped after {}s", self.elapsed.as_secs())
        } else if self.phase == Some(LABELS.len()) {
            format!("Connected in {}s", self.elapsed.as_secs())
        } else if self.phase.is_some() {
            format!(
                "{}s elapsed · {}s on this step",
                self.elapsed.as_secs(),
                self.phase_elapsed.as_secs()
            )
        } else {
            format!(
                "{}s elapsed · waiting for readiness checks",
                self.elapsed.as_secs()
            )
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
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.TEXT_DIM)
                    .child(self.timing_label()),
            )
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

    #[test]
    fn elapsed_clock_never_completes_a_step_and_freezes_on_failure() {
        let start = Instant::now();
        let mut progress = Progress::default();
        progress.observe_at("Checking AWS sign-in...", false, start);
        assert!(!progress.tick_at(start + Duration::from_millis(500)));
        assert!(progress.tick_at(start + Duration::from_secs(12)));
        assert_eq!(progress.row(0), ("●", "In progress"));
        assert_eq!(progress.timing_label(), "12s elapsed · 12s on this step");
        progress.observe_at(
            "Checking cloud runtime guard and allowance...",
            false,
            start + Duration::from_secs(13),
        );
        progress.tick_at(start + Duration::from_secs(16));
        assert_eq!(progress.timing_label(), "16s elapsed · 3s on this step");
        progress.observe_at("failed", true, start + Duration::from_secs(17));
        assert!(!progress.tick_at(start + Duration::from_secs(30)));
        assert_eq!(progress.timing_label(), "Stopped after 17s");
    }

    #[test]
    fn boot_wait_does_not_claim_vm_is_running_and_late_phases_do_not_reset_timer() {
        let start = Instant::now();
        let mut progress = Progress::default();
        progress.observe_at(
            "Waiting for shared cloud VM to become reachable...",
            false,
            start,
        );
        progress.tick_at(start + Duration::from_secs(20));
        assert_eq!(progress.row(2), ("●", "In progress"));
        assert_eq!(progress.row(3), ("○", "Waiting"));
        progress.observe_at(
            "Checking AWS sign-in...",
            false,
            start + Duration::from_secs(21),
        );
        assert_eq!(progress.timing_label(), "21s elapsed · 21s on this step");
        progress.observe_at(
            "Shared cloud VM is running. SSH bootstrap ready.",
            false,
            start + Duration::from_secs(22),
        );
        assert_eq!(progress.row(2), ("✓", "Done"));
        progress.observe_at(
            "Connected to jcode-cloud-alpha",
            false,
            start + Duration::from_secs(23),
        );
        assert!(!progress.tick_at(start + Duration::from_secs(40)));
        assert_eq!(progress.timing_label(), "Connected in 23s");
    }
}
