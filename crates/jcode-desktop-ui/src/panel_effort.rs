//! Reasoning effort switching for a chat panel: `/effort`, the effort pill
//! menu, and the fast increase/decrease keys all funnel through here.
//!
//! A switch is shown immediately (the pill moves on the keypress) and then
//! settled by the runtime: a refusal restores the previous level, and the
//! identity broadcast that follows a success carries the level the provider
//! actually applied.
use super::*;
use crate::effort;

/// An effort request the runtime has not answered yet.
#[derive(Clone, Debug)]
pub(super) struct PendingEffort {
    /// What the pill showed before the first unanswered request.
    previous: Option<String>,
    /// Requests sent and not yet settled, oldest first.
    requested: VecDeque<String>,
}

impl Panel {
    /// Levels the serving model accepts.
    pub(super) fn effort_ladder(&self) -> Vec<&'static str> {
        effort::ladder(self.provider.as_deref(), self.model.as_deref())
    }

    /// Keep the composer's `/effort` menu on the serving model's ladder.
    /// Deferred: `/effort` runs inside the composer's own submit callback.
    pub(super) fn sync_effort_menu(&self, cx: &mut Context<Self>) {
        let ladder = self.effort_ladder();
        let current = self.reasoning_effort.clone();
        let input = self.input.clone();
        cx.defer(move |cx| {
            input.update(cx, |input, cx| input.set_effort_state(ladder, current, cx))
        });
    }

    /// Validate and send one effort change. Returns false, after reporting
    /// why in the transcript, when the level is not offered.
    pub(super) fn request_effort(&mut self, level: &str, cx: &mut Context<Self>) -> bool {
        if self.is_side_document() || self.preview_state.is_some() {
            return false;
        }
        let ladder = self.effort_ladder();
        if ladder.is_empty() {
            self.items.push(Item::Error(format!(
                "Reasoning effort is not available for {}.",
                self.model.as_deref().unwrap_or("this model")
            )));
            return false;
        }
        let Some(level) = ladder.iter().copied().find(|candidate| *candidate == level) else {
            self.items.push(Item::Error(format!(
                "Usage: `/effort <{}>`",
                ladder.join("|")
            )));
            return false;
        };
        let pending = self.pending_effort.get_or_insert_with(|| PendingEffort {
            previous: self.reasoning_effort.clone(),
            requested: VecDeque::new(),
        });
        pending.requested.push_back(level.to_string());
        self.reasoning_effort = Some(level.to_string());
        self.bridge.send(Command::SessionOperation {
            session_id: self.session_id.clone(),
            operation: SessionOperation::SetEffort(level.to_string()),
        });
        self.sync_effort_menu(cx);
        cx.notify();
        true
    }

    /// One rung up (`direction > 0`) or down, from the fast effort keys.
    pub(super) fn step_effort(&mut self, direction: i8, cx: &mut Context<Self>) {
        let ladder = self.effort_ladder();
        if ladder.is_empty() {
            self.effort_notice("Reasoning effort not available for this model".into(), cx);
            return;
        }
        match effort::step(&ladder, self.reasoning_effort.as_deref(), direction) {
            Some(next) => {
                if self.request_effort(next, cx) {
                    self.effort_notice(format!("Effort: {}", effort::label(next)), cx);
                }
            }
            None => {
                let current = self
                    .reasoning_effort
                    .as_deref()
                    .map(effort::label)
                    .unwrap_or("default");
                let end = if direction > 0 {
                    "strongest"
                } else {
                    "weakest"
                };
                self.effort_notice(format!("Effort: {current} (already {end})"), cx);
            }
        }
    }

    /// A quiet confirmation under the composer, like the TUI status notice.
    fn effort_notice(&mut self, notice: String, cx: &mut Context<Self>) {
        let input = self.input.clone();
        cx.defer(move |cx| input.update(cx, |input, cx| input.set_transient_notice(notice, cx)));
    }

    /// The runtime answered one effort request.
    pub(crate) fn effort_settled(
        &mut self,
        effort: &str,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_effort.as_mut() else {
            if let Some(error) = error {
                self.items.push(Item::Error(format!(
                    "Failed to set effort `{effort}`: {error}"
                )));
            }
            cx.notify();
            return;
        };
        if let Some(index) = pending.requested.iter().position(|level| level == effort) {
            pending.requested.remove(index);
        }
        let settled = pending.requested.is_empty();
        if let Some(error) = error {
            self.items.push(Item::Error(format!(
                "Failed to set effort `{effort}`: {error}"
            )));
            crate::sounds::play(crate::sounds::Cue::Error, cx);
            if settled {
                // Nothing newer is in flight, so the pill must return to the
                // level the provider is still using.
                self.reasoning_effort = pending.previous.clone();
            }
        }
        if settled {
            self.pending_effort = None;
        }
        self.sync_effort_menu(cx);
        cx.notify();
    }

    /// Identity reported an effort. While requests are in flight, an older
    /// broadcast must not snap the pill back between rapid keypresses.
    pub(super) fn identity_effort(&mut self, reported: Option<String>) {
        match &mut self.pending_effort {
            Some(pending) => {
                // Remember the runtime's truth for a later refusal, but keep
                // showing the newest request.
                pending.previous = reported;
            }
            None => self.reasoning_effort = reported,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Command;
    use std::sync::mpsc::Receiver;

    fn setup<'a>(
        cx: &'a mut gpui::TestAppContext,
        provider: &str,
        model: &str,
        effort: Option<&str>,
    ) -> (
        Entity<Panel>,
        &'a mut gpui::VisualTestContext,
        Receiver<Command>,
    ) {
        cx.update(crate::input::bind_keys);
        let (bridge, commands) = crate::harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::ModelInfo {
                    session_id: "session-a".into(),
                    provider: Some(provider.into()),
                    model: Some(model.into()),
                    reasoning_effort: effort.map(str::to_string),
                },
                cx,
            );
        });
        vcx.update(|window, cx| {
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();
        (panel, vcx, commands)
    }

    fn sent_efforts(commands: &Receiver<Command>) -> Vec<String> {
        commands
            .try_iter()
            .filter_map(|command| match command {
                Command::SessionOperation {
                    operation: SessionOperation::SetEffort(effort),
                    ..
                } => Some(effort),
                _ => None,
            })
            .collect()
    }

    fn effort_of(panel: &Entity<Panel>, vcx: &mut gpui::VisualTestContext) -> Option<String> {
        panel.read_with(vcx, |panel, _| panel.reasoning_effort.clone())
    }

    /// The configured chords (Alt+Left/Right by default) step one rung at a
    /// time from the focused composer, stop at the ends, and update the pill
    /// immediately without waiting for the runtime.
    #[gpui::test]
    fn fast_effort_keys_step_the_ladder_from_the_composer(cx: &mut gpui::TestAppContext) {
        let keys = crate::effort::EffortKeys::from_config();
        let (Some(up), Some(down)) = (keys.increase.first(), keys.decrease.first()) else {
            return; // Effort keys disabled in this user's config.
        };
        let (panel, vcx, commands) = setup(cx, "openai", "gpt-5.6-sol", Some("high"));

        vcx.simulate_keystrokes(up);
        vcx.run_until_parked();
        assert_eq!(effort_of(&panel, vcx).as_deref(), Some("xhigh"));
        vcx.simulate_keystrokes(up);
        vcx.simulate_keystrokes(down);
        vcx.simulate_keystrokes(down);
        vcx.run_until_parked();
        assert_eq!(effort_of(&panel, vcx).as_deref(), Some("high"));
        assert_eq!(sent_efforts(&commands), ["xhigh", "max", "xhigh", "high"]);
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel
                .input
                .read(cx)
                .transient_notice()
                .map(str::to_string)),
            Some("Effort: High".into())
        );
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).content.to_string()),
            "",
            "effort keys must not type or move text in the composer"
        );
    }

    #[gpui::test]
    fn fast_effort_keys_stop_at_the_strongest_level(cx: &mut gpui::TestAppContext) {
        let keys = crate::effort::EffortKeys::from_config();
        let Some(up) = keys.increase.first() else {
            return;
        };
        let (panel, vcx, commands) = setup(cx, "anthropic", "claude-sonnet-4-6", None);
        let top = *panel
            .read_with(vcx, |panel, _| panel.effort_ladder())
            .last()
            .unwrap();
        panel.update(vcx, |panel, _| panel.reasoning_effort = Some(top.into()));
        vcx.simulate_keystrokes(up);
        vcx.run_until_parked();
        assert_eq!(effort_of(&panel, vcx).as_deref(), Some(top));
        assert!(
            sent_efforts(&commands).is_empty(),
            "nothing to send at the top"
        );
        assert!(
            panel
                .read_with(vcx, |panel, cx| panel
                    .input
                    .read(cx)
                    .transient_notice()
                    .map(str::to_string))
                .is_some_and(|notice| notice.contains("already strongest"))
        );
    }

    /// A refused request restores the level the provider is still using, and
    /// an identity broadcast between rapid presses does not snap the pill back.
    #[gpui::test]
    fn a_refusal_restores_the_previous_effort(cx: &mut gpui::TestAppContext) {
        let (panel, vcx, commands) = setup(cx, "openai", "gpt-5.6-sol", Some("medium"));
        panel.update(vcx, |panel, cx| {
            assert!(panel.request_effort("high", cx));
            assert!(panel.request_effort("xhigh", cx));
            // The first change's broadcast arrives while xhigh is in flight.
            panel.apply(
                &ApiEvent::ModelInfo {
                    session_id: "session-a".into(),
                    provider: Some("openai".into()),
                    model: Some("gpt-5.6-sol".into()),
                    reasoning_effort: Some("high".into()),
                },
                cx,
            );
            assert_eq!(panel.reasoning_effort.as_deref(), Some("xhigh"));
            panel.effort_settled("high", None, cx);
            panel.effort_settled("xhigh", Some("not supported"), cx);
            assert_eq!(panel.reasoning_effort.as_deref(), Some("high"));
            assert!(matches!(panel.items.last(), Some(Item::Error(error)) if error.contains("not supported")));
            // Settled: later identity is authoritative again.
            panel.apply(
                &ApiEvent::ModelInfo {
                    session_id: "session-a".into(),
                    provider: Some("openai".into()),
                    model: Some("gpt-5.6-sol".into()),
                    reasoning_effort: None,
                },
                cx,
            );
            assert_eq!(panel.reasoning_effort, None);
        });
        assert_eq!(sent_efforts(&commands), ["high", "xhigh"]);
    }

    /// `/effort` offers and accepts only the serving model's levels.
    #[gpui::test]
    fn slash_effort_follows_the_serving_model(cx: &mut gpui::TestAppContext) {
        let (panel, vcx, commands) = setup(cx, "anthropic", "claude-sonnet-4-6", Some("high"));
        vcx.simulate_input("/effort minimal");
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(
            sent_efforts(&commands).is_empty(),
            "Claude has no minimal level"
        );
        assert!(panel.read_with(vcx, |panel, _| matches!(
            panel.items.last(),
            Some(Item::Error(error)) if error.contains("low|medium|high")
        )));

        panel.update(vcx, |panel, cx| {
            panel
                .input
                .update(cx, |input, cx| input.set_content(String::new(), cx))
        });
        vcx.simulate_input("/effort Low");
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert_eq!(sent_efforts(&commands), ["low"]);
        assert_eq!(effort_of(&panel, vcx).as_deref(), Some("low"));
    }

    /// The pill menu opens on the current level, so Enter keeps it.
    #[gpui::test]
    fn effort_menu_opens_on_the_current_level(cx: &mut gpui::TestAppContext) {
        let (panel, vcx, commands) = setup(cx, "openai", "gpt-5.6-sol", Some("xhigh"));
        vcx.update(|window, cx| {
            panel.update(cx, |panel, cx| panel.toggle_effort_picker(window, cx))
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("down enter");
        vcx.run_until_parked();
        assert_eq!(sent_efforts(&commands), ["max"]);
        assert_eq!(
            panel.read_with(vcx, |panel, cx| panel.input.read(cx).content.to_string()),
            "",
            "the menu closes after choosing"
        );
    }
}
