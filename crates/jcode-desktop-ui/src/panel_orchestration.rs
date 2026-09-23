//! Orchestration: every live session, whether it is running, and its todos.
//! Deliberately shows nothing from the conversation itself.
use std::time::Duration;

use gpui::{Context, FontWeight, ScrollHandle, Task, Window, div, prelude::*, px};

use super::{Panel, SessionOpener};
use crate::harness::{LiveSession, UnfinishedTodo};
use crate::theme::Theme;

pub(crate) const SESSION_ID: &str = "orchestration://sessions";
const POLL: Duration = Duration::from_secs(1);

pub(super) struct State {
    pub(super) sessions: Vec<LiveSession>,
    loaded: bool,
    scroll: ScrollHandle,
    opener: Option<SessionOpener>,
    _poll: Option<Task<()>>,
}

impl State {
    /// Offline screenshot fixture. Never reads the real ~/.jcode.
    pub(super) fn fixture() -> Self {
        Self {
            sessions: fixture_sessions(),
            loaded: true,
            scroll: ScrollHandle::new(),
            opener: None,
            _poll: None,
        }
    }
}

impl Panel {
    pub fn new_orchestration(
        opener: SessionOpener,
        bridge: crate::harness::Bridge,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(
            SESSION_ID.into(),
            Some("orchestration".into()),
            None,
            bridge,
            cx,
        );
        panel.items.clear();
        let fixture = crate::harness::screenshot_mode();
        // Tests and offline screenshots never read the real ~/.jcode.
        let poll = (!cfg!(test) && !fixture).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    let sessions = cx
                        .background_executor()
                        .spawn(async { crate::harness::live_sessions() })
                        .await;
                    if this
                        .update(cx, |panel, cx| panel.set_live_sessions(sessions, cx))
                        .is_err()
                    {
                        break;
                    }
                    cx.background_executor().timer(POLL).await;
                }
            })
        });
        panel.orchestration = Some(State {
            sessions: if fixture { fixture_sessions() } else { Vec::new() },
            loaded: fixture,
            scroll: ScrollHandle::new(),
            opener: Some(opener),
            _poll: poll,
        });
        panel
    }

    pub(crate) fn set_live_sessions(&mut self, sessions: Vec<LiveSession>, cx: &mut Context<Self>) {
        let Some(state) = self.orchestration.as_mut() else {
            return;
        };
        if !state.loaded || state.sessions != sessions {
            state.sessions = sessions;
            state.loaded = true;
            cx.notify();
        }
    }

    pub(super) fn render_orchestration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let state = self.orchestration.as_ref().expect("orchestration state");
        let running = state.sessions.iter().filter(|s| s.running).count();
        let mut list = div()
            .id("orchestration-list")
            .debug_selector(|| "orchestration-list".into())
            .size_full()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(&state.scroll)
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_2();
        if state.sessions.is_empty() {
            list = list.child(
                div()
                    .mt_4()
                    .text_size(px(12.))
                    .text_color(Theme::global().TEXT_DIM)
                    .child(if state.loaded {
                        "No live sessions."
                    } else {
                        "Looking for live sessions…"
                    }),
            );
        }
        for (index, session) in state.sessions.iter().enumerate() {
            list = list.child(render_session(index, session, state.opener.clone()));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .debug_selector(|| "orchestration-header".into())
                    .flex_none()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Orchestration"),
                    )
                    .child(
                        div()
                            .debug_selector(|| "orchestration-summary".into())
                            .text_size(px(11.))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(format!(
                                "{} live · {running} running",
                                state.sessions.len()
                            )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(list)
                    .child(crate::scrollbar::vertical(
                        &state.scroll,
                        "orchestration-scrollbar",
                    )),
            )
            .into_any_element()
    }
}

fn render_session(
    index: usize,
    session: &LiveSession,
    opener: Option<SessionOpener>,
) -> impl IntoElement {
    let theme = Theme::global();
    let total = session.todos.len();
    let done = session.completed_todos();
    let target = session.as_unfinished();
    let mut card = div()
        .id(("orchestration-session", index))
        .debug_selector(move || format!("orchestration-session-{index}"))
        .px_3()
        .py_2()
        .rounded_xl()
        .bg(theme.HEADER_BG)
        .cursor_pointer()
        .hover(|el| el.bg(Theme::global().ACCENT_DIM))
        .on_mouse_down(gpui::MouseButton::Left, move |_event, window, cx| {
            // Routing into the workspace may read this panel. Never hold a
            // Panel update lease while invoking the opener.
            if let Some(open) = &opener {
                open(target.clone(), window, cx);
                cx.stop_propagation();
            }
        })
        .on_mouse_up(gpui::MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_pill(index, session.running))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.TEXT)
                        .child(session.title.clone()),
                )
                .when(total > 0, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme.TEXT_DIM)
                            .child(format!("{done}/{total}")),
                    )
                }),
        );
    if let Some(dir) = session.working_dir.as_deref() {
        card = card.child(
            div()
                .truncate()
                .text_size(px(10.))
                .text_color(theme.TEXT_FAINT)
                .child(short_dir(dir)),
        );
    }
    if total == 0 {
        return card.child(
            div()
                .text_size(px(11.))
                .text_color(theme.TEXT_FAINT)
                .child("No todos"),
        );
    }
    let mut todos = div().pt_1().flex().flex_col().gap(px(3.));
    for todo in &session.todos {
        todos = todos.child(render_todo(todo));
    }
    card.child(todos)
}

fn status_pill(index: usize, running: bool) -> impl IntoElement {
    let theme = Theme::global();
    let color = if running { theme.PANEL_BG } else { theme.TEXT_FAINT };
    div()
        .debug_selector(move || {
            format!(
                "orchestration-status-{index}-{}",
                if running { "running" } else { "idle" }
            )
        })
        .flex_none()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .bg(if running { theme.ACCENT } else { theme.PANEL_BG })
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(10.))
        .text_color(color)
        .child(div().size(px(6.)).rounded_full().bg(color))
        .child(if running { "running" } else { "idle" })
}

fn render_todo(todo: &UnfinishedTodo) -> impl IntoElement {
    let theme = Theme::global();
    let status = todo.status.to_ascii_lowercase();
    let (color, text) = match status.as_str() {
        "completed" => (theme.OK, theme.TEXT_FAINT),
        "in_progress" => (theme.ACCENT, theme.TEXT),
        "cancelled" => (theme.TEXT_FAINT, theme.TEXT_FAINT),
        _ => (theme.TEXT_FAINT, theme.TEXT_DIM),
    };
    let filled = matches!(status.as_str(), "completed" | "in_progress");
    div()
        .flex()
        .items_start()
        .gap_2()
        .text_size(px(12.))
        .text_color(text)
        .child(
            div()
                .flex_none()
                .mt(px(2.))
                .size(px(11.))
                .rounded_full()
                .border_1()
                .border_color(color)
                .p(px(2.))
                .when(filled, |marker| {
                    marker.child(div().size_full().rounded_full().bg(color))
                }),
        )
        .child(div().flex_1().min_w_0().child(todo.content.clone()))
}

fn short_dir(dir: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && dir.starts_with(&home) => {
            format!("~{}", &dir[home.len()..])
        }
        _ => dir.to_owned(),
    }
}

fn fixture_sessions() -> Vec<LiveSession> {
    let todo = |content: &str, status: &str| UnfinishedTodo {
        content: content.into(),
        status: status.into(),
        group: None,
    };
    vec![
        LiveSession {
            session_id: "session_fox_1789000000000_a".into(),
            title: "Orchestration panel".into(),
            working_dir: Some("/home/user/jcode-desktop".into()),
            running: true,
            todos: vec![
                todo("Load live sessions from presence markers", "completed"),
                todo("Render status pills and todos", "in_progress"),
                todo("Wire shortcut and sidebar action", "pending"),
            ],
        },
        LiveSession {
            session_id: "session_owl_1788000000000_b".into(),
            title: "Sidebar spacing".into(),
            working_dir: Some("/home/user/jcode-desktop".into()),
            running: false,
            todos: vec![
                todo("Tighten row spacing", "completed"),
                todo("Fix selection highlight", "completed"),
            ],
        },
        LiveSession {
            session_id: "session_elk_1787000000000_c".into(),
            title: "api reviewer".into(),
            working_dir: Some("/home/user/jcode".into()),
            running: true,
            todos: Vec::new(),
        },
    ]
}
