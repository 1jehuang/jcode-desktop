//! A per-request Jcode Cloud checklist, driven by observed control-plane and
//! connection phases, not a timer.
//! A later phase proves earlier phases finished. Unknown messages never do.
use gpui::{InteractiveElement, IntoElement, ParentElement, Styled, div, px};
use std::time::{Duration, Instant};

use crate::theme::Theme;

const LABELS: [&str; 5] = [
    "Jcode account",
    "Cloud machine",
    "Jcode installed",
    "Secure connection",
    "Jcode session",
];

#[derive(Clone, Default, Debug)]
pub(super) struct Progress {
    phase: Option<usize>,
    failed: bool,
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
            let next = if line.contains("creating jcode session") {
                Some(4)
            } else if !failed
                && (line.starts_with("connected to ") || line.contains(": connected to "))
            {
                Some(5)
            } else if line.contains("connecting to your jcode cloud machine") {
                Some(3)
            } else if line.contains("installing jcode") || line.contains("starting jcode on") {
                Some(2)
            } else if line.contains("creating your jcode cloud machine")
                || line.contains("recreating your jcode cloud machine")
                || line.contains("waking your jcode cloud machine")
                || line.contains("starting your jcode cloud machine")
                || line.contains("finishing sleep")
            {
                Some(1)
            } else if line.contains("checking your jcode cloud machine") {
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
            Some(phase) if index < phase => ("✓", "Done"),
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
    fn checklist_advances_only_with_observed_managed_phases() {
        let mut progress = Progress::default();
        assert_eq!(progress.row(0), ("○", "Waiting"));
        progress.observe("Something unrelated", false);
        assert!(progress.phase.is_none());
        for (phase, message) in [
            "Checking your Jcode Cloud machine…",
            "Creating your Jcode Cloud machine…",
            "Installing Jcode on your cloud machine…",
            "Connecting to your Jcode Cloud machine…",
            "Connected. Creating Jcode session…",
            "Connected to jcode-cloud",
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
    fn waking_an_existing_machine_skips_straight_to_the_machine_step() {
        let mut progress = Progress::default();
        progress.observe("Waking your Jcode Cloud machine…", false);
        assert_eq!(progress.row(0), ("✓", "Done"));
        assert_eq!(progress.row(1), ("●", "In progress"));
        progress.observe("Checking your Jcode Cloud machine…", false);
        assert_eq!(progress.row(1), ("●", "In progress"), "never regresses");
    }

    #[test]
    fn checklist_failure_retains_finished_steps_and_ignores_late_progress() {
        let mut progress = Progress::default();
        progress.observe("Connecting to your Jcode Cloud machine…", false);
        progress.observe("Remote session failed: connection refused", true);
        assert_eq!(progress.row(2), ("✓", "Done"));
        assert_eq!(progress.row(3), ("!", "Failed"));
        assert_eq!(progress.row(4), ("○", "Waiting"));
        progress.observe("Connected to jcode-cloud", false);
        assert_eq!(progress.row(3), ("!", "Failed"));
    }

    #[test]
    fn checklist_early_error_does_not_claim_machine_ready() {
        let mut progress = Progress::default();
        progress.observe("Sign in to your Jcode account to use Jcode Cloud.", true);
        assert_eq!(progress.row(0), ("!", "Failed"));
        assert_eq!(progress.row(1), ("○", "Waiting"));
        assert_eq!(progress.row(4), ("○", "Waiting"));
    }

    #[test]
    fn elapsed_clock_never_completes_a_step_and_freezes_on_failure() {
        let start = Instant::now();
        let mut progress = Progress::default();
        progress.observe_at("Checking your Jcode Cloud machine…", false, start);
        assert!(!progress.tick_at(start + Duration::from_millis(500)));
        assert!(progress.tick_at(start + Duration::from_secs(12)));
        assert_eq!(progress.row(0), ("●", "In progress"));
        assert_eq!(progress.timing_label(), "12s elapsed · 12s on this step");
        progress.observe_at(
            "Creating your Jcode Cloud machine…",
            false,
            start + Duration::from_secs(13),
        );
        progress.tick_at(start + Duration::from_secs(16));
        assert_eq!(progress.timing_label(), "16s elapsed · 3s on this step");
        progress.observe_at("failed", true, start + Duration::from_secs(17));
        assert!(!progress.tick_at(start + Duration::from_secs(30)));
        assert_eq!(progress.timing_label(), "Stopped after 17s");
    }
}
