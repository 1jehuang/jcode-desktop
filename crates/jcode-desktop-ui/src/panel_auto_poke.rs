//! Desktop-owned continuation of locally submitted work, not attached sessions.
use super::*;

const MAX_FOLLOW_UPS: usize = 8;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AutoPoke {
    start_index: Option<usize>,
    ready: bool,
    attempts: usize,
    seen: Vec<Vec<(String, String)>>,
}

impl AutoPoke {
    pub(super) fn start(&mut self, index: usize) {
        *self = Self {
            start_index: Some(index),
            ..Self::default()
        };
    }

    pub(super) fn observe(&mut self, event: &ApiEvent) {
        match event {
            ApiEvent::TurnDone { .. } => self.ready = self.start_index.is_some(),
            ApiEvent::Error { .. } => *self = Self::default(),
            ApiEvent::SessionStatus { status, .. }
                if matches!(status.as_str(), "cancelled" | "canceled" | "disconnected") =>
            {
                *self = Self::default();
            }
            _ => {}
        }
    }

    fn claim(&mut self, enabled: bool, todos: &[TodoCardItem]) -> Option<String> {
        if !std::mem::take(&mut self.ready) || !enabled || self.attempts >= MAX_FOLLOW_UPS {
            return None;
        }
        // Only explicitly actionable states qualify. Unknown, blocked, finished,
        // and cancelled work must not create a retry loop.
        let mut remaining: Vec<_> = todos
            .iter()
            .filter(|todo| {
                matches!(
                    todo.status.trim().to_ascii_lowercase().as_str(),
                    "pending" | "in_progress"
                ) && todo.blocked_by.is_empty()
            })
            .map(|todo| {
                (
                    todo.content.clone(),
                    todo.status.trim().to_ascii_lowercase(),
                )
            })
            .collect();
        remaining.sort();
        if remaining.is_empty() || self.seen.contains(&remaining) {
            return None;
        }
        let message = jcode_base::todo::build_auto_poke_message(remaining.len());
        self.seen.push(remaining);
        self.attempts += 1;
        Some(message)
    }
}

impl Panel {
    pub(super) fn send_auto_poke(&mut self, cx: &mut Context<Self>) {
        if self.prompt_queue.paused
            || self.prompt_queue.waiting_for_connection
            || !self.prompt_queue.prompts.is_empty()
            || !self.pending_users.is_empty()
            || self.status != "idle"
            || self.activity_active()
            || !self.history_loaded
            || self.is_pending_session()
        {
            self.prompt_queue.auto_poke.ready = false;
            return;
        }
        let Some(start) = self.prompt_queue.auto_poke.start_index else {
            return;
        };
        // Ignore todo cards from older user requests and restored history.
        let todos = self
            .items
            .get(start..)
            .unwrap_or_default()
            .iter()
            .rev()
            .find_map(|item| match item {
                Item::Todos(payload) => Some(payload.clone()),
                Item::Tool {
                    name,
                    output,
                    done: true,
                    error: None,
                    ..
                } if name == "todo" => parse_todo_tool_output(output),
                _ => None,
            })
            .unwrap_or_default();
        if let Some(message) = self.prompt_queue.auto_poke.claim(
            jcode_base::config::config().features.auto_poke,
            &todos.todos,
        ) {
            // Keep the bounded cycle state. This is not a new user request.
            self.send_prompt(message, Vec::new(), cx);
            cx.notify();
        }
    }
}

#[cfg(test)]
#[path = "panel_auto_poke_tests.rs"]
mod tests;
