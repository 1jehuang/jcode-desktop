//! Workspace: a niri-inspired horizontal strip of session panels with a
//! smooth camera and a zoomed-out overview.
//!
//! Panels live on an infinite horizontal strip. Focus moves left/right;
//! the camera glides so the focused panel is visible. The overview zooms
//! out to show every panel; clicking one focuses it.

use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Window, actions, div, prelude::*, px,
    relative,
};

use crate::harness::{self, Bridge, Command, Update};
use crate::panel::Panel;
use crate::theme::Theme;

actions!(
    workspace,
    [
        FocusLeft, FocusRight, FocusUp, FocusDown, MovePanelLeft, MovePanelRight, NewPanel,
        ClosePanel, ToggleOverview, WidthPreset1, WidthPreset2, WidthPreset3, WidthPreset4, Quit,
    ]
);

/// Camera animation speed: fraction of remaining distance covered per frame
/// at 60 fps. Exponential ease-out, niri-like.
const CAMERA_LERP: f32 = 0.22;
const GAP: f32 = 16.0;
const STRIP_PADDING_Y: f32 = 20.0;

struct Slot {
    panel: Entity<Panel>,
    /// Width as a fraction of the viewport (0.25, 0.5, 0.75, 1.0).
    width_fraction: f32,
}

pub struct Workspace {
    bridge: Bridge,
    slots: Vec<Slot>,
    active: usize,
    /// Camera offset in strip pixels; animated toward `camera_target`.
    camera_x: f32,
    camera_target: f32,
    overview: bool,
    /// Sessions offered on connect that have no panel yet.
    available_sessions: Vec<jcode_sdk::SessionInfo>,
    status: String,
    connected: bool,
    focus_handle: FocusHandle,
    /// Focus the active panel's input on the next render (set when panels
    /// appear from background updates, where no Window is available).
    focus_pending: bool,
    _poll_task: gpui::Task<()>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::input::bind_keys(cx);
        let bridge = harness::spawn();

        // Poll bridge updates ~60 times per second while anything is pending.
        let poll_bridge = bridge.clone();
        let poll_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let updates = poll_bridge.drain();
                if updates.is_empty() {
                    continue;
                }
                let outcome = this.update(cx, |workspace: &mut Workspace, cx| {
                    for update in updates {
                        workspace.apply(update, cx);
                    }
                    cx.notify();
                });
                if outcome.is_err() {
                    break;
                }
            }
        });

        let _ = window;
        Self {
            bridge,
            slots: Vec::new(),
            active: 0,
            camera_x: 0.0,
            camera_target: 0.0,
            overview: false,
            available_sessions: Vec::new(),
            status: "starting...".into(),
            connected: false,
            focus_handle: cx.focus_handle(),
            focus_pending: false,
            _poll_task: poll_task,
        }
    }

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Status(status) => self.status = status,
            Update::Connected => {
                self.connected = true;
                self.status = "connected".into();
                if self.slots.is_empty() {
                    // Start with a fresh session immediately; recent sessions
                    // arrive asynchronously and open as panels when listed.
                    self.bridge.send(Command::CreateSession {
                        working_dir: default_working_dir(),
                    });
                }
            }
            Update::Sessions { sessions } => {
                // Open the two most recent sessions alongside whatever is
                // already on the strip; keep the rest for a resume list.
                let mut sessions = sessions;
                let open_now: Vec<_> = sessions.split_off(sessions.len().saturating_sub(2));
                self.available_sessions = sessions;
                for session in open_now {
                    let already_open = self
                        .slots
                        .iter()
                        .any(|slot| slot.panel.read(cx).session_id == session.session_id);
                    if !already_open {
                        self.open_session(session, cx);
                    }
                }
            }
            Update::SessionCreated { session } => {
                self.open_session(session, cx);
                self.active = self.slots.len().saturating_sub(1);
                self.retarget_camera();
                self.focus_pending = true;
            }
            Update::History {
                session_id,
                messages,
            } => {
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == session_id {
                        slot.panel.update(cx, |panel, cx| {
                            panel.load_history(messages, cx);
                        });
                        break;
                    }
                }
            }
            Update::Event(event) => {
                if let Some(session_id) = event_session_id(&event) {
                    for slot in &self.slots {
                        if slot.panel.read(cx).session_id == session_id {
                            slot.panel.update(cx, |panel, cx| panel.apply(&event, cx));
                            break;
                        }
                    }
                }
            }
            Update::Disconnected { reason } => {
                self.connected = false;
                self.status = format!("disconnected: {reason} (retrying)");
            }
            Update::SessionLost { session_id, reason } => {
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == session_id {
                        slot.panel.update(cx, |panel, cx| {
                            panel.status = format!("lost: {reason}");
                            cx.notify();
                        });
                        break;
                    }
                }
            }
        }
    }

    fn open_session(&mut self, session: jcode_sdk::SessionInfo, cx: &mut Context<Self>) {
        let bridge = self.bridge.clone();
        let session_id = session.session_id.clone();
        let panel = cx.new(|cx| {
            Panel::new(
                session.session_id.clone(),
                session.title.clone(),
                session.working_dir.clone(),
                bridge,
                cx,
            )
        });
        Panel::connect_input(&panel, cx);
        self.bridge.send(Command::Watch { session_id });
        self.slots.push(Slot {
            panel,
            width_fraction: 0.5,
        });
    }

    // --- Geometry -------------------------------------------------------

    fn slot_width(&self, index: usize, viewport: f32) -> f32 {
        let fraction = self.slots[index].width_fraction;
        (viewport * fraction - GAP * 2.0).max(320.0)
    }

    fn slot_left(&self, index: usize, viewport: f32) -> f32 {
        let mut x = GAP;
        for i in 0..index {
            x += self.slot_width(i, viewport) + GAP;
        }
        x
    }

    fn retarget_camera(&mut self) {
        // Camera target is resolved during render when the viewport width is
        // known; setting a sentinel forces recomputation.
        self.camera_target = f32::NAN;
    }

    fn resolve_camera_target(&mut self, viewport: f32) {
        if self.slots.is_empty() {
            self.camera_target = 0.0;
            return;
        }
        let active = self.active.min(self.slots.len() - 1);
        let left = self.slot_left(active, viewport);
        let width = self.slot_width(active, viewport);
        // Center the active panel, clamped to strip bounds.
        let total = self.slot_left(self.slots.len(), viewport);
        let centered = left - (viewport - width) / 2.0;
        self.camera_target = centered.clamp(-GAP, (total - viewport).max(-GAP));
    }

    // --- Actions --------------------------------------------------------

    fn focus_left(&mut self, _: &FocusLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.active > 0 {
            self.active -= 1;
            self.retarget_camera();
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn focus_right(&mut self, _: &FocusRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.active + 1 < self.slots.len() {
            self.active += 1;
            self.retarget_camera();
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn focus_up(&mut self, _: &FocusUp, _window: &mut Window, cx: &mut Context<Self>) {
        // Vertical focus maps to overview until vertical stacks exist.
        self.overview = true;
        cx.notify();
    }

    fn focus_down(&mut self, _: &FocusDown, _window: &mut Window, cx: &mut Context<Self>) {
        self.overview = false;
        cx.notify();
    }

    fn move_panel_left(&mut self, _: &MovePanelLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.active > 0 {
            self.slots.swap(self.active, self.active - 1);
            self.active -= 1;
            self.retarget_camera();
            cx.notify();
        }
    }

    fn move_panel_right(&mut self, _: &MovePanelRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.active + 1 < self.slots.len() {
            self.slots.swap(self.active, self.active + 1);
            self.active += 1;
            self.retarget_camera();
            cx.notify();
        }
    }

    fn new_panel(&mut self, _: &NewPanel, _: &mut Window, _cx: &mut Context<Self>) {
        self.bridge.send(Command::CreateSession {
            working_dir: default_working_dir(),
        });
    }

    fn close_panel(&mut self, _: &ClosePanel, window: &mut Window, cx: &mut Context<Self>) {
        if self.slots.is_empty() {
            return;
        }
        let removed = self.slots.remove(self.active);
        let session_id = removed.panel.read(cx).session_id.clone();
        self.bridge.send(Command::Unwatch { session_id });
        if self.active >= self.slots.len() && self.active > 0 {
            self.active -= 1;
        }
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn toggle_overview(&mut self, _: &ToggleOverview, _: &mut Window, cx: &mut Context<Self>) {
        self.overview = !self.overview;
        cx.notify();
    }

    fn set_width(&mut self, fraction: f32, cx: &mut Context<Self>) {
        if let Some(slot) = self.slots.get_mut(self.active) {
            slot.width_fraction = fraction;
            self.retarget_camera();
            cx.notify();
        }
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(slot) = self.slots.get(self.active) {
            let panel = slot.panel.clone();
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        } else {
            window.focus(&self.focus_handle, cx);
        }
    }

    // --- Rendering ------------------------------------------------------

    fn render_strip(&mut self, viewport_w: f32, viewport_h: f32, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.camera_target.is_nan() {
            self.resolve_camera_target(viewport_w);
        }
        // Animate the camera.
        let delta = self.camera_target - self.camera_x;
        if delta.abs() > 0.5 {
            self.camera_x += delta * CAMERA_LERP;
            window.request_animation_frame();
        } else {
            self.camera_x = self.camera_target;
        }

        let panel_h = viewport_h - STRIP_PADDING_Y * 2.0;
        let mut strip = div()
            .absolute()
            .top(px(STRIP_PADDING_Y))
            .left(px(-self.camera_x))
            .flex()
            .flex_row()
            .gap(px(GAP));

        for (index, slot) in self.slots.iter().enumerate() {
            let width = self.slot_width(index, viewport_w);
            let focused = index == self.active;
            strip = strip.child(
                div()
                    .id(("panel", index))
                    .w(px(width))
                    .h(px(panel_h))
                    .flex_none()
                    .bg(Theme::PANEL_BG)
                    .border_2()
                    .border_color(if focused {
                        Theme::PANEL_BORDER_FOCUS
                    } else {
                        Theme::PANEL_BORDER
                    })
                    .rounded_xl()
                    .overflow_hidden()
                    .when(focused, |el| el.shadow_lg())
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            this.active = index;
                            this.retarget_camera();
                            this.focus_active(window, cx);
                            cx.notify();
                        }),
                    )
                    .child(slot.panel.clone()),
            );
        }

        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .pl(px(GAP))
            .child(strip)
            .into_any_element()
    }

    fn render_overview(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut grid = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_4()
            .p_8()
            .justify_center()
            .items_center()
            .content_center()
            .size_full();

        for (index, slot) in self.slots.iter().enumerate() {
            let panel = slot.panel.read(cx);
            let focused = index == self.active;
            let title = panel.title.clone();
            let status = panel.status.clone();
            let busy = panel.is_busy();
            let preview: String = panel
                .items
                .iter()
                .rev()
                .find_map(|item| match item {
                    crate::panel::Item::Assistant(text) | crate::panel::Item::User(text) => {
                        Some(text.chars().take(220).collect())
                    }
                    _ => None,
                })
                .unwrap_or_default();

            grid = grid.child(
                div()
                    .id(("overview-panel", index))
                    .w(px(300.0))
                    .h(px(190.0))
                    .flex()
                    .flex_col()
                    .bg(Theme::PANEL_BG)
                    .border_2()
                    .border_color(if focused {
                        Theme::PANEL_BORDER_FOCUS
                    } else {
                        Theme::PANEL_BORDER
                    })
                    .rounded_xl()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|el| el.border_color(Theme::ACCENT))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            this.active = index;
                            this.overview = false;
                            this.retarget_camera();
                            this.focus_active(window, cx);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .bg(Theme::HEADER_BG)
                            .child(
                                div()
                                    .size(px(7.0))
                                    .rounded_full()
                                    .bg(if busy { Theme::WARN } else { Theme::OK }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.0))
                                    .text_color(Theme::TEXT)
                                    .overflow_hidden()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(Theme::TEXT_DIM)
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .p_3()
                            .text_size(px(11.0))
                            .text_color(Theme::TEXT_DIM)
                            .overflow_hidden()
                            .line_height(relative(1.4))
                            .child(preview),
                    ),
            );
        }

        // New session card.
        grid = grid.child(
            div()
                .id("overview-new")
                .w(px(300.0))
                .h(px(190.0))
                .flex()
                .items_center()
                .justify_center()
                .border_2()
                .border_dashed()
                .border_color(Theme::PANEL_BORDER)
                .rounded_xl()
                .cursor_pointer()
                .text_color(Theme::TEXT_DIM)
                .hover(|el| el.border_color(Theme::ACCENT).text_color(Theme::ACCENT))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.bridge.send(Command::CreateSession {
                            working_dir: default_working_dir(),
                        });
                        this.overview = false;
                        cx.notify();
                    }),
                )
                .child("+ new session"),
        );

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(grid)
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending && !self.slots.is_empty() {
            self.focus_pending = false;
            self.focus_active(window, cx);
        }
        let viewport = window.viewport_size();
        let viewport_w = f32::from(viewport.width);
        let viewport_h = f32::from(viewport.height) - 28.0; // status bar

        let content = if self.overview {
            self.render_overview(cx)
        } else if self.slots.is_empty() {
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .items_center()
                .justify_center()
                .text_color(Theme::TEXT_DIM)
                .child(if self.connected {
                    "no sessions - super-n opens one"
                } else {
                    "connecting to jcode..."
                })
                .child(
                    div()
                        .text_size(px(12.0))
                        .child(self.status.clone()),
                )
                .into_any_element()
        } else {
            self.render_strip(viewport_w, viewport_h, window, cx)
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::BG)
            .font_family(Theme::FONT_UI)
            .text_size(px(14.0))
            .text_color(Theme::TEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::move_panel_left))
            .on_action(cx.listener(Self::move_panel_right))
            .on_action(cx.listener(Self::new_panel))
            .on_action(cx.listener(Self::close_panel))
            .on_action(cx.listener(Self::toggle_overview))
            .on_action(cx.listener(|this, _: &WidthPreset1, _w, cx| this.set_width(0.25, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset2, _w, cx| this.set_width(0.5, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset3, _w, cx| this.set_width(0.75, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset4, _w, cx| this.set_width(1.0, cx)))
            .child(div().flex_1().min_h_0().child(content))
            // Status bar
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .h(px(28.0))
                    .px_3()
                    .bg(Theme::HEADER_BG)
                    .border_t_1()
                    .border_color(Theme::PANEL_BORDER)
                    .text_size(px(11.0))
                    .text_color(Theme::TEXT_DIM)
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(if self.connected { Theme::OK } else { Theme::WARN }),
                    )
                    .child(self.status.clone())
                    .child(div().flex_1())
                    .child(format!(
                        "{} panel{}",
                        self.slots.len(),
                        if self.slots.len() == 1 { "" } else { "s" }
                    ))
                    .child("super-hjkl move · super-n new · super-tab overview · super-1..4 width"),
            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn default_working_dir() -> Option<String> {
    std::env::var("HOME").ok()
}

fn event_session_id(event: &jcode_sdk::ApiEvent) -> Option<&str> {
    use jcode_sdk::ApiEvent::*;
    match event {
        TextDelta { session_id, .. }
        | ReasoningDelta { session_id, .. }
        | ReasoningDone { session_id, .. }
        | ToolStart { session_id, .. }
        | ToolInputDelta { session_id, .. }
        | ToolExec { session_id, .. }
        | ToolDone { session_id, .. }
        | TokenUsage { session_id, .. }
        | TurnDone { session_id, .. }
        | BackgroundProgress { session_id, .. }
        | MessageAccepted { session_id }
        | PermissionRequest { session_id, .. }
        | SessionStatus { session_id, .. }
        | ConnectionPhase { session_id, .. }
        | ModelInfo { session_id, .. }
        | Models { session_id, .. }
        | RuntimeInfo { session_id, .. }
        | FileContent { session_id, .. }
        | Files { session_id, .. }
        | TextMatches { session_id, .. }
        | FileStatus { session_id, .. }
        | Compacted { session_id, .. }
        | SessionRenamed { session_id, .. } => Some(session_id),
        _ => None,
    }
}
