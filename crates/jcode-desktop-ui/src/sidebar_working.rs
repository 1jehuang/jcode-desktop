//! Live "working for" timers for sidebar sessions that are mid-turn.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{Context, Entity, IntoElement, Render, Task, Window, div, prelude::*};

/// Elapsed time since a session started working, ticking once per second.
pub(super) struct WorkingTimer {
    since: Instant,
    _tick: Task<()>,
}

impl WorkingTimer {
    fn new(since: Instant, cx: &mut Context<Self>) -> Self {
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self { since, _tick: tick }
    }
}

impl Render for WorkingTimer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(format_working(self.since.elapsed()))
    }
}

/// `working 42s` / `working 3m 12s` / `working 1h 5m`.
pub(super) fn format_working(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        return format!("working {seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("working {minutes}m {}s", seconds % 60);
    }
    format!("working {}h {}m", minutes / 60, minutes % 60)
}

/// Timers keyed by session. A timer starts when a session is first seen
/// working and is dropped as soon as it stops, so the next turn restarts at 0.
#[derive(Default)]
pub(super) struct Timers(HashMap<String, Entity<WorkingTimer>>);

impl Timers {
    pub(super) fn sync<T: 'static>(&mut self, working: &HashSet<String>, cx: &mut Context<T>) {
        self.0.retain(|id, _| working.contains(id));
        for id in working {
            if !self.0.contains_key(id) {
                let now = Instant::now();
                self.0.insert(id.clone(), cx.new(|cx| WorkingTimer::new(now, cx)));
            }
        }
    }

    pub(super) fn get(&self, session_id: &str) -> Option<Entity<WorkingTimer>> {
        self.0.get(session_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::format_working;
    use std::time::Duration;

    #[test]
    fn formats_elapsed_working_time() {
        assert_eq!(format_working(Duration::from_secs(5)), "working 5s");
        assert_eq!(format_working(Duration::from_secs(192)), "working 3m 12s");
        assert_eq!(format_working(Duration::from_secs(3900)), "working 1h 5m");
    }
}
