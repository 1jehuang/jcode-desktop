//! Workspace: four niri-inspired horizontal strips of session panels with a
//! smooth camera and a zoomed-out overview.
//!
//! Panels live on one of four infinite horizontal strips. Focus moves
//! left/right within a strip and up/down between strips.

use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Window, actions, div, prelude::*, px, relative,
};

use crate::harness::{self, Bridge, Command, Update};
use crate::learning;
use crate::panel::Panel;
use crate::theme::Theme;
use crate::transition::{self, AnimatedValue, Transition};

actions!(
    workspace,
    [
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        FocusFirst,
        FocusLast,
        FocusPrevious,
        MovePanelLeft,
        MovePanelRight,
        MovePanelUp,
        MovePanelDown,
        MovePanelToFirst,
        MovePanelToLast,
        NewPanel,
        ClosePanel,
        ToggleOverview,
        ToggleHints,
        NewHelpSession,
        CycleWidth,
        MaximizeWidth,
        WidthPreset1,
        WidthPreset2,
        WidthPreset3,
        WidthPreset4,
        Quit,
    ]
);

/// niri `animations { window-resize / workspace-switch }` use 150ms with an
/// `ease-out-expo` curve; the strip camera matches that.
const CAMERA_DURATION: Duration = transition::STANDARD_DURATION;
/// niri `layout { gaps 0 }`: columns sit flush against each other.
const GAP: f32 = 0.0;
/// niri `layout { struts { ... 0.58 } }`, the outer gap around the strip.
const STRUT: f32 = 0.58;
const STRIP_PADDING_Y: f32 = STRUT;
const STRIP_COUNT: usize = 4;
/// niri `window-rule { geometry-corner-radius 6 }`.
const CORNER_RADIUS: f32 = 6.0;
/// niri `preset-column-widths`: Alt+R cycles through these in order.
const PRESET_WIDTHS: [f32; 3] = [0.25, 0.5, 0.75];
/// niri `default-column-width { proportion 0.5; }`.
const DEFAULT_WIDTH: f32 = 0.5;
const HELP_SESSION_PROMPT: &str = r#"Act as the in-app Jcode guide. Use the bundled Jcode documentation before answering questions about Jcode features or behavior.

The jcode-desktop shortcuts are:
- Super+H/J/K/L: navigate panels
- Super+Shift+H/J/K/L: move panels
- Super+N: open a session to the right
- Super+Tab: return to the previous panel
- Super+R: cycle panel width
- Super+F: maximize or restore panel width
- Super+O: open the overview
- Super+/ or F1: toggle the hints overlay
- Super+Shift+/: open this documentation-aware help session

Composer shortcuts ported from the TUI:
- Up/Down or Ctrl+K/J: recall older/newer prompts
- Ctrl/Alt+Left/Right or Alt+B/F: move by word
- Ctrl+W, Ctrl/Alt/Super+Backspace, Alt+D: delete by word
- Ctrl+U: delete to the start; Ctrl+E: move to the end
- Ctrl/Cmd+Z and Ctrl+Shift+Z: undo and redo
- Escape: clear the draft

Start with a concise orientation, then invite me to ask how to use Jcode."#;
const SIDEBAR_WIDTH: f32 = 264.0;

struct Slot {
    panel: Entity<Panel>,
    row: usize,
    /// Width as a fraction of the viewport (0.25, 0.5, 0.75, 1.0).
    width_fraction: f32,
    /// Rendered width, retargeted when the configured width changes.
    animated_width: AnimatedValue,
    /// Width to restore when un-maximizing (niri `maximize-column` toggle).
    restore_fraction: Option<f32>,
}

pub struct Workspace {
    bridge: Bridge,
    slots: Vec<Slot>,
    active: usize,
    active_row: usize,
    /// Previously focused panel, for niri's `focus-window-previous`.
    previous: Option<gpui::EntityId>,
    /// Each strip retains its own horizontal camera position.
    camera_x: [f32; STRIP_COUNT],
    camera_target: [f32; STRIP_COUNT],
    /// Where the current camera animation started, and when.
    camera_from: [f32; STRIP_COUNT],
    camera_started: [Option<Instant>; STRIP_COUNT],
    /// Set when a strip's camera target must be recomputed at render time,
    /// once the viewport width is known.
    camera_dirty: [bool; STRIP_COUNT],
    overview: bool,
    overview_progress: AnimatedValue,
    hints_overlay: bool,
    hints_progress: AnimatedValue,
    /// Models which shortcuts the user knows, and teaches the ones they don't.
    coach: learning::Coach,
    /// Fade for the coach's hint toast.
    coach_progress: AnimatedValue,
    pending_help_session: bool,
    /// Every non-archived session offered by the runtime, oldest to newest.
    sessions: Vec<jcode_sdk::SessionInfo>,
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
            active_row: 0,
            previous: None,
            camera_x: [0.0; STRIP_COUNT],
            camera_target: [0.0; STRIP_COUNT],
            camera_from: [0.0; STRIP_COUNT],
            camera_started: [None; STRIP_COUNT],
            camera_dirty: [true; STRIP_COUNT],
            overview: false,
            overview_progress: AnimatedValue::new(
                0.0,
                transition::policy(Transition::Overview).duration,
            ),
            hints_overlay: false,
            hints_progress: AnimatedValue::new(0.0, transition::policy(Transition::Hints).duration),
            coach: learning::load(),
            coach_progress: AnimatedValue::new(0.0, transition::policy(Transition::Coach).duration),
            pending_help_session: false,
            sessions: Vec::new(),
            status: "starting...".into(),
            connected: false,
            focus_handle: cx.focus_handle(),
            focus_pending: false,
            _poll_task: poll_task,
        }
    }

    /// A workspace with no runtime and a caller-supplied coach, for tests that
    /// drive real keystrokes through the real keymap.
    #[cfg(test)]
    pub fn for_test(coach: learning::Coach, cx: &mut Context<Self>) -> Self {
        crate::input::bind_keys(cx);
        Self {
            bridge: harness::spawn_inert(),
            slots: Vec::new(),
            active: 0,
            active_row: 0,
            previous: None,
            camera_x: [0.0; STRIP_COUNT],
            camera_target: [0.0; STRIP_COUNT],
            camera_from: [0.0; STRIP_COUNT],
            camera_started: [None; STRIP_COUNT],
            camera_dirty: [true; STRIP_COUNT],
            overview: false,
            overview_progress: AnimatedValue::new(
                0.0,
                transition::policy(Transition::Overview).duration,
            ),
            hints_overlay: false,
            hints_progress: AnimatedValue::new(0.0, transition::policy(Transition::Hints).duration),
            coach,
            coach_progress: AnimatedValue::new(0.0, transition::policy(Transition::Coach).duration),
            pending_help_session: false,
            sessions: Vec::new(),
            status: "test".into(),
            connected: true,
            focus_handle: cx.focus_handle(),
            focus_pending: false,
            _poll_task: cx.spawn(async move |_, _| {}),
        }
    }

    /// Add a bare panel for tests, bypassing the runtime.
    #[cfg(test)]
    pub fn push_test_panel(&mut self, name: &str, cx: &mut Context<Self>) {
        let bridge = self.bridge.clone();
        let panel =
            cx.new(|cx| Panel::new(name.to_string(), Some(name.to_string()), None, bridge, cx));
        self.slots.push(Slot {
            panel,
            row: self.active_row,
            width_fraction: DEFAULT_WIDTH,
            animated_width: AnimatedValue::new(
                DEFAULT_WIDTH,
                transition::policy(Transition::PanelWidth).duration,
            ),
            restore_fraction: None,
        });
    }

    #[cfg(test)]
    pub fn test_coach(&self) -> &learning::Coach {
        &self.coach
    }

    #[cfg(test)]
    pub fn test_focus_position(&self) -> Option<usize> {
        self.row_indices(self.active_row)
            .position(|index| index == self.active)
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
                self.sessions = sessions;
            }
            Update::SessionCreated { session } => {
                let session_id = session.session_id.clone();
                if !self
                    .sessions
                    .iter()
                    .any(|known| known.session_id == session.session_id)
                {
                    self.sessions.push(session.clone());
                }
                let inserted = self.open_session(session, cx);
                self.set_active(inserted, cx);
                self.focus_pending = true;
                if self.pending_help_session {
                    self.pending_help_session = false;
                    self.bridge.send(Command::Send {
                        session_id,
                        content: HELP_SESSION_PROMPT.into(),
                    });
                }
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
            Update::Event { session_id, event } => {
                if let jcode_sdk::ApiEvent::SessionRenamed { display_title, .. } = &event
                    && let Some(session) = self
                        .sessions
                        .iter_mut()
                        .find(|session| session.session_id == session_id)
                {
                    // SessionInfo carries the effective display title, so keep
                    // the sidebar's cached copy in sync with the live panel.
                    session.title = Some(display_title.clone());
                }
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == session_id {
                        slot.panel.update(cx, |panel, cx| panel.apply(&event, cx));
                        break;
                    }
                }
            }
            Update::SendFailed { session_id, reason } => {
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == session_id {
                        slot.panel.update(cx, |panel, cx| {
                            panel.message_failed(format!("message failed: {reason}"), cx);
                        });
                        break;
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

    /// Open `session` as a panel immediately to the right of the focused panel,
    /// mirroring niri's "new column opens right of the focused column".
    /// Returns the index of the new slot.
    fn open_session(&mut self, session: jcode_sdk::SessionInfo, cx: &mut Context<Self>) -> usize {
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
        let slot = Slot {
            panel,
            row: self.active_row,
            width_fraction: DEFAULT_WIDTH,
            animated_width: AnimatedValue::new(
                0.0,
                transition::policy(Transition::PanelOpen).duration,
            ),
            restore_fraction: None,
        };
        let active_is_on_strip = self
            .slots
            .get(self.active)
            .is_some_and(|slot| slot.row == self.active_row);
        let row_last = self.row_indices(self.active_row).last();
        let insert_at = insert_index(self.active, active_is_on_strip, row_last, self.slots.len());
        self.slots.insert(insert_at, slot);
        self.slots[insert_at]
            .animated_width
            .set(DEFAULT_WIDTH, Instant::now());
        // Inserting shifts every later index, including the focused one.
        if self.active >= insert_at {
            self.active += 1;
        }
        insert_at
    }

    fn activate_session(
        &mut self,
        session: jcode_sdk::SessionInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self
            .slots
            .iter()
            .position(|slot| slot.panel.read(cx).session_id == session.session_id)
            .unwrap_or_else(|| self.open_session(session, cx));
        self.set_active(index, cx);
        self.overview = false;
        self.overview_progress.set(0.0, Instant::now());
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Focus the slot at `index`, remembering the outgoing panel so
    /// `FocusPrevious` (niri's Alt+Tab) can return to it.
    fn set_active(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.slots.len() {
            return;
        }
        let outgoing = self
            .slots
            .get(self.active)
            .filter(|_| self.active != index)
            .map(|slot| slot.panel.entity_id());
        if let Some(outgoing) = outgoing {
            self.previous = Some(outgoing);
        }
        self.active = index;
        self.active_row = self.slots[index].row;
        self.retarget_camera();
        cx.notify();
    }

    // --- Geometry -------------------------------------------------------

    fn slot_width(&self, index: usize, viewport: f32) -> f32 {
        let fraction = self.slots[index].width_fraction;
        Self::width_for_fraction(fraction, viewport)
    }

    fn width_for_fraction(fraction: f32, viewport: f32) -> f32 {
        // niri sizes a column as a proportion of the working area, which is the
        // output minus the struts.
        ((viewport - STRUT * 2.0) * fraction - GAP).max(320.0)
    }

    fn slot_left(&self, index: usize, viewport: f32) -> f32 {
        let mut x = STRUT;
        for i in self.row_indices(self.slots[index].row) {
            if i == index {
                break;
            }
            x += self.slot_width(i, viewport) + GAP;
        }
        x
    }

    fn row_indices(&self, row: usize) -> impl Iterator<Item = usize> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(move |(index, slot)| (slot.row == row).then_some(index))
    }

    fn active_position_in_row(&self) -> usize {
        self.row_indices(self.active_row)
            .position(|index| index == self.active)
            .unwrap_or(0)
    }

    fn select_row(&mut self, row: usize, preferred_position: usize) {
        self.active_row = row.min(STRIP_COUNT - 1);
        let selected = self
            .row_indices(self.active_row)
            .nth(preferred_position)
            .or_else(|| self.row_indices(self.active_row).last());
        if let Some(index) = selected {
            if let Some(outgoing) = self
                .slots
                .get(self.active)
                .map(|slot| slot.panel.entity_id())
                && self.active != index
            {
                self.previous = Some(outgoing);
            }
            self.active = index;
        }
        self.retarget_camera();
    }

    fn retarget_camera(&mut self) {
        // Camera target is resolved during render when the viewport width is
        // known; setting a sentinel forces recomputation.
        self.camera_dirty[self.active_row] = true;
    }

    /// Resolve the strip's scroll offset. This follows niri's
    /// `center-focused-column "never"`: the camera only scrolls far enough to
    /// bring the focused panel fully on screen, keeping it at the left or right
    /// edge rather than centering it.
    fn resolve_camera_target(&mut self, viewport: f32) {
        self.camera_dirty[self.active_row] = false;
        if self.row_indices(self.active_row).next().is_none() {
            self.camera_target[self.active_row] = 0.0;
            return;
        }
        let active = self
            .slots
            .get(self.active)
            .filter(|slot| slot.row == self.active_row)
            .map(|_| self.active)
            .unwrap_or_else(|| self.row_indices(self.active_row).next().unwrap());
        let left = self.slot_left(active, viewport);
        let width = self.slot_width(active, viewport);
        let total = self
            .row_indices(self.active_row)
            .map(|index| self.slot_width(index, viewport) + GAP)
            .sum::<f32>()
            + STRUT * 2.0;
        let current = self.camera_target[self.active_row];
        let target = scroll_into_view(current, left, width, viewport);
        let max_scroll = (total - viewport).max(-GAP);
        let target = target.clamp(-STRUT, max_scroll.max(-STRUT));
        let row = self.active_row;
        if (target - self.camera_target[row]).abs() > 0.01 {
            self.camera_from[row] = self.camera_x[row];
            self.camera_started[row] = Some(Instant::now());
            self.camera_target[row] = target;
        }
    }

    // --- Actions --------------------------------------------------------

    // --- Learning -------------------------------------------------------

    /// Record that the user reached an outcome by its keyboard shortcut.
    fn learned(&mut self, skill_id: &str, cx: &mut Context<Self>) {
        self.coach.used_shortcut(skill_id, learning::now());
        self.after_coach_update(cx);
    }

    /// Record that the user reached the same outcome the long way, which is the
    /// evidence that they do not know (or have forgotten) the shortcut.
    fn missed(&mut self, skill_id: &str, cx: &mut Context<Self>) {
        self.coach.used_slow_path(skill_id, learning::now());
        self.after_coach_update(cx);
    }

    /// Reveal or retire the hint toast and persist the model when it changed.
    fn after_coach_update(&mut self, cx: &mut Context<Self>) {
        let now = learning::now();
        let visible = self.coach.active_hint(now).is_some();
        self.coach_progress
            .set(if visible { 1.0 } else { 0.0 }, Instant::now());
        if self.coach.take_dirty() {
            learning::save(&self.coach);
        }
        cx.notify();
    }

    fn dismiss_coach_hint(&mut self, cx: &mut Context<Self>) {
        self.coach.dismiss_hint();
        self.coach_progress.set(0.0, Instant::now());
        cx.notify();
    }

    /// Clicking a panel to focus it. Only counted as a slow path when the
    /// keyboard would genuinely have done the same job: clicking the already
    /// focused panel, or reaching into another strip, is not a missed shortcut.
    /// Distance matters too, since crossing a strip is what `super-end` is for.
    fn clicked_to_focus(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.active {
            return;
        }
        let Some(target_row) = self.slots.get(index).map(|slot| slot.row) else {
            return;
        };
        if target_row != self.active_row {
            // Another strip: the pointer is a reasonable way to get there.
            return;
        }
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        let from = indices.iter().position(|&i| i == self.active);
        let to = indices.iter().position(|&i| i == index);
        let Some((from, to)) = from.zip(to) else {
            return;
        };
        let steps = from.abs_diff(to);
        self.missed(click_skill(steps, to, indices.len()), cx);
    }

    fn focus_left(&mut self, _: &FocusLeft, window: &mut Window, cx: &mut Context<Self>) {
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position > 0
        {
            self.set_active(indices[position - 1], cx);
            self.focus_active(window, cx);
            // Credit only when the key did something: pressing into the edge of
            // a strip is a no-op and proves nothing either way.
            self.learned("focus_left_right", cx);
            cx.notify();
        }
    }

    fn focus_right(&mut self, _: &FocusRight, window: &mut Window, cx: &mut Context<Self>) {
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position + 1 < indices.len()
        {
            self.set_active(indices[position + 1], cx);
            self.focus_active(window, cx);
            self.learned("focus_left_right", cx);
            cx.notify();
        }
    }

    /// niri `focus-column-first`.
    fn focus_first(&mut self, _: &FocusFirst, window: &mut Window, cx: &mut Context<Self>) {
        let first = self.row_indices(self.active_row).next();
        if let Some(index) = first.filter(|index| *index != self.active) {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            self.learned("focus_first_last", cx);
            cx.notify();
        }
    }

    /// niri `focus-column-last`.
    fn focus_last(&mut self, _: &FocusLast, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.row_indices(self.active_row).last();
        if let Some(index) = last.filter(|index| *index != self.active) {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            self.learned("focus_first_last", cx);
            cx.notify();
        }
    }

    /// niri `focus-window-previous` (the user's Alt+Tab). Returns to the last
    /// focused panel wherever it now lives, including on another strip.
    fn focus_previous(&mut self, _: &FocusPrevious, window: &mut Window, cx: &mut Context<Self>) {
        let Some(previous) = self.previous else {
            return;
        };
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.panel.entity_id() == previous)
        else {
            self.previous = None;
            return;
        };
        self.set_active(index, cx);
        self.focus_active(window, cx);
        self.learned("focus_previous", cx);
        cx.notify();
    }

    fn focus_up(&mut self, _: &FocusUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_row > 0 {
            self.change_row(self.active_row - 1, window, cx);
        }
    }

    fn focus_down(&mut self, _: &FocusDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_row + 1 < STRIP_COUNT {
            self.change_row(self.active_row + 1, window, cx);
        }
    }

    /// Move focus to another strip, crediting the shortcut only when the move
    /// was meaningful. Stepping onto an empty strip when there is nothing else
    /// open shows the user pressed a key, not that they navigated anywhere, so
    /// it must not be taken as evidence of skill.
    fn change_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let position = self.active_position_in_row();
        let was_active = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.entity_id());
        self.select_row(row, position);
        self.focus_active(window, cx);
        let is_active = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.entity_id());
        let landed_somewhere = self.row_indices(self.active_row).next().is_some();
        if landed_somewhere && was_active != is_active {
            self.learned("focus_up_down", cx);
        }
        cx.notify();
    }

    fn move_panel_left(&mut self, _: &MovePanelLeft, _: &mut Window, cx: &mut Context<Self>) {
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position > 0
        {
            let previous = indices[position - 1];
            self.slots.swap(self.active, previous);
            self.active = previous;
            self.retarget_camera();
            self.learned("move_panel", cx);
            cx.notify();
        }
    }

    fn move_panel_right(&mut self, _: &MovePanelRight, _: &mut Window, cx: &mut Context<Self>) {
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position + 1 < indices.len()
        {
            let next = indices[position + 1];
            self.slots.swap(self.active, next);
            self.active = next;
            self.retarget_camera();
            self.learned("move_panel", cx);
            cx.notify();
        }
    }

    fn move_panel_up(&mut self, _: &MovePanelUp, window: &mut Window, cx: &mut Context<Self>) {
        self.move_panel_to_row(-1, window, cx);
    }

    /// niri `move-column-to-first`.
    fn move_panel_to_first(
        &mut self,
        _: &MovePanelToFirst,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let first = self.row_indices(self.active_row).next();
        let on_strip = self
            .slots
            .get(self.active)
            .is_some_and(|slot| slot.row == self.active_row);
        let Some(first) = first.filter(|first| on_strip && *first != self.active) else {
            return;
        };
        let slot = self.slots.remove(self.active);
        self.slots.insert(first, slot);
        self.active = first;
        self.retarget_camera();
        self.learned("move_panel_end", cx);
        cx.notify();
    }

    /// niri `move-column-to-last`.
    fn move_panel_to_last(&mut self, _: &MovePanelToLast, _: &mut Window, cx: &mut Context<Self>) {
        let last = self.row_indices(self.active_row).last();
        let on_strip = self
            .slots
            .get(self.active)
            .is_some_and(|slot| slot.row == self.active_row);
        let Some(last) = last.filter(|last| on_strip && *last != self.active) else {
            return;
        };
        let slot = self.slots.remove(self.active);
        self.slots.insert(last, slot);
        self.active = last;
        self.retarget_camera();
        self.learned("move_panel_end", cx);
        cx.notify();
    }

    fn move_panel_down(&mut self, _: &MovePanelDown, window: &mut Window, cx: &mut Context<Self>) {
        self.move_panel_to_row(1, window, cx);
    }

    fn move_panel_to_row(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.active_row as isize + delta;
        if !(0..STRIP_COUNT as isize).contains(&target) {
            return;
        }
        let Some(slot) = self.slots.get_mut(self.active) else {
            return;
        };
        if slot.row != self.active_row {
            return;
        }
        slot.row = target as usize;
        self.active_row = target as usize;
        self.retarget_camera();
        self.focus_active(window, cx);
        self.learned("move_panel_strip", cx);
        cx.notify();
    }

    fn new_panel(&mut self, _: &NewPanel, _: &mut Window, cx: &mut Context<Self>) {
        self.learned("new_panel", cx);
        self.open_new_session(cx);
    }

    /// Open a session without attributing the choice to the keyboard. Pointer
    /// paths call this directly, so clicking never earns keyboard credit.
    fn open_new_session(&mut self, _cx: &mut Context<Self>) {
        self.bridge.send(Command::CreateSession {
            working_dir: default_working_dir(),
        });
    }

    fn close_panel(&mut self, _: &ClosePanel, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .slots
            .get(self.active)
            .is_none_or(|slot| slot.row != self.active_row)
        {
            return;
        }
        self.learned("close_panel", cx);
        let removed = self.slots.remove(self.active);
        let session_id = removed.panel.read(cx).session_id.clone();
        self.bridge.send(Command::Unwatch { session_id });
        if self.previous == Some(removed.panel.entity_id()) {
            self.previous = None;
        }
        let remaining: Vec<_> = self.row_indices(self.active_row).collect();
        self.active = focus_after_close(self.active, &remaining);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn toggle_overview(&mut self, _: &ToggleOverview, _: &mut Window, cx: &mut Context<Self>) {
        self.learned("overview", cx);
        self.overview = !self.overview;
        self.overview_progress
            .set(if self.overview { 1.0 } else { 0.0 }, Instant::now());
        self.hints_overlay = false;
        self.hints_progress.set(0.0, Instant::now());
        cx.notify();
    }

    fn toggle_hints(&mut self, _: &ToggleHints, _: &mut Window, cx: &mut Context<Self>) {
        self.hints_overlay = !self.hints_overlay;
        self.hints_progress
            .set(if self.hints_overlay { 1.0 } else { 0.0 }, Instant::now());
        cx.notify();
    }

    fn new_help_session(&mut self, _: &NewHelpSession, _: &mut Window, cx: &mut Context<Self>) {
        self.pending_help_session = true;
        self.hints_overlay = false;
        self.hints_progress.set(0.0, Instant::now());
        self.bridge.send(Command::CreateSession {
            working_dir: default_working_dir(),
        });
        cx.notify();
    }

    fn set_width(&mut self, fraction: f32, cx: &mut Context<Self>) {
        if let Some(slot) = self
            .slots
            .get_mut(self.active)
            .filter(|slot| slot.row == self.active_row)
        {
            slot.width_fraction = fraction;
            slot.animated_width.set(fraction, Instant::now());
            slot.restore_fraction = None;
            self.retarget_camera();
            self.learned("width_presets", cx);
        }
    }

    /// niri `switch-preset-column-width` (Alt+R): step to the next preset,
    /// wrapping around.
    fn cycle_width(&mut self, _: &CycleWidth, _: &mut Window, cx: &mut Context<Self>) {
        let Some(slot) = self
            .slots
            .get_mut(self.active)
            .filter(|slot| slot.row == self.active_row)
        else {
            return;
        };
        slot.width_fraction = next_preset(slot.width_fraction);
        slot.animated_width.set(slot.width_fraction, Instant::now());
        slot.restore_fraction = None;
        self.retarget_camera();
        self.learned("cycle_width", cx);
    }

    /// niri `maximize-column` (Alt+F): fill the viewport, or restore the
    /// previous width when already maximized.
    fn maximize_width(&mut self, _: &MaximizeWidth, _: &mut Window, cx: &mut Context<Self>) {
        let Some(slot) = self
            .slots
            .get_mut(self.active)
            .filter(|slot| slot.row == self.active_row)
        else {
            return;
        };
        let (width, restore) = toggle_maximize(slot.width_fraction, slot.restore_fraction);
        slot.width_fraction = width;
        slot.animated_width.set(width, Instant::now());
        slot.restore_fraction = restore;
        self.retarget_camera();
        self.learned("maximize", cx);
    }

    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(slot) = self
            .slots
            .get(self.active)
            .filter(|slot| slot.row == self.active_row)
        {
            let panel = slot.panel.clone();
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        } else {
            window.focus(&self.focus_handle, cx);
        }
    }

    // --- Rendering ------------------------------------------------------

    /// A one-line snapshot of the strip layout. Written to the path in
    /// `JCODE_DESKTOP_STATE` on every render so an automated check can observe
    /// what the running window is actually doing.
    fn dump_state(&self) {
        let Ok(path) = std::env::var("JCODE_DESKTOP_STATE") else {
            return;
        };
        let widths: Vec<f32> = self
            .row_indices(self.active_row)
            .map(|index| self.slots[index].width_fraction)
            .collect();
        let focus = self
            .row_indices(self.active_row)
            .position(|index| index == self.active);
        let line = describe_strip(&widths, focus, self.active_row);
        // The coach's decisions are also dumped, so an automated check can see
        // what the running app believes the user knows and what it is teaching.
        let now = learning::now();
        let coach = describe_coach(
            self.coach.overall_mastery(now),
            self.coach.effort_saved,
            self.coach.effort_wasted,
            self.coach.active_hint_id(),
        );
        let _ = std::fs::write(path, format!("{line}\n{coach}\n"));
    }

    fn render_strip(
        &mut self,
        viewport_w: f32,
        viewport_h: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.camera_dirty[self.active_row] {
            self.resolve_camera_target(viewport_w);
        }
        // Animate the camera over CAMERA_DURATION on an ease-out-expo curve,
        // matching niri's animation settings.
        let row = self.active_row;
        match self.camera_started[row] {
            Some(started) => {
                let elapsed = started.elapsed();
                if elapsed >= CAMERA_DURATION {
                    self.camera_x[row] = self.camera_target[row];
                    self.camera_started[row] = None;
                } else {
                    let t = elapsed.as_secs_f32() / CAMERA_DURATION.as_secs_f32();
                    let eased = ease_out_expo(t);
                    let from = self.camera_from[row];
                    self.camera_x[row] = from + (self.camera_target[row] - from) * eased;
                    window.request_animation_frame();
                }
            }
            None => self.camera_x[row] = self.camera_target[row],
        }

        let panel_h = viewport_h - STRIP_PADDING_Y * 2.0;
        let indices = self.row_indices(self.active_row).collect::<Vec<_>>();
        let now = Instant::now();
        let mut animated_widths = Vec::with_capacity(indices.len());
        for &index in &indices {
            let fraction = self.slots[index].animated_width.sample(now);
            if self.slots[index].animated_width.is_animating() {
                window.request_animation_frame();
            }
            animated_widths.push(Self::width_for_fraction(fraction, viewport_w));
        }
        let mut strip = div()
            .absolute()
            .top(px(STRIP_PADDING_Y))
            .left(px(-self.camera_x[self.active_row]))
            .flex()
            .flex_row()
            .gap(px(GAP));

        for (index, width) in indices.into_iter().zip(animated_widths) {
            let slot = &self.slots[index];
            let focused = index == self.active;
            strip = strip.child(
                div()
                    .id(("panel", index))
                    .w(px(width))
                    .h(px(panel_h))
                    .flex_none()
                    .bg(Theme::PANEL_BG)
                    .border_1()
                    .border_color(if focused {
                        Theme::PANEL_BORDER_FOCUS
                    } else {
                        Theme::PANEL_BORDER_IDLE
                    })
                    .rounded(px(CORNER_RADIUS))
                    .overflow_hidden()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            this.clicked_to_focus(index, cx);
                            this.set_active(index, cx);
                            this.focus_active(window, cx);
                            cx.notify();
                        }),
                    )
                    .child(slot.panel.clone()),
            );
        }

        // Two-finger touchpad swipes (and horizontal mouse wheels) pan the
        // strip directly, like grabbing the canvas. Vertical deltas are left
        // for the panel transcripts, except when Shift redirects them here.
        let total_width = self
            .row_indices(row)
            .map(|index| self.slot_width(index, viewport_w) + GAP)
            .sum::<f32>()
            + STRUT * 2.0;
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .pl(px(GAP))
            .on_scroll_wheel(cx.listener(
                move |this, event: &gpui::ScrollWheelEvent, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    let mut dx = f32::from(delta.x);
                    if dx == 0.0 && event.modifiers.shift {
                        dx = f32::from(delta.y);
                    }
                    if dx == 0.0 {
                        return;
                    }
                    // Natural scrolling: content follows the fingers, so the
                    // camera moves opposite the delta. Panning cancels any
                    // in-flight camera animation and becomes the new target,
                    // otherwise the next frame would snap back.
                    let next = pan_camera(this.camera_x[row], -dx, total_width, viewport_w);
                    if (next - this.camera_x[row]).abs() < f32::EPSILON {
                        return;
                    }
                    this.camera_x[row] = next;
                    this.camera_target[row] = next;
                    this.camera_from[row] = next;
                    this.camera_started[row] = None;
                    this.camera_dirty[row] = false;
                    cx.notify();
                },
            ))
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
                            this.set_active(index, cx);
                            this.overview = false;
                            this.overview_progress.set(0.0, Instant::now());
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
                            .child(div().size(px(7.0)).rounded_full().bg(if busy {
                                Theme::WARN
                            } else {
                                Theme::OK
                            }))
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
                        this.missed("new_panel", cx);
                        this.open_new_session(cx);
                        this.overview = false;
                        this.overview_progress.set(0.0, Instant::now());
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

    fn render_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let active_id = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.read(cx).session_id.clone());
        let open_ids = self
            .slots
            .iter()
            .map(|slot| slot.panel.read(cx).session_id.clone())
            .collect::<Vec<_>>();
        let mut list = div()
            .id("sidebar-session-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .py_2();

        for (sidebar_index, session) in self.sessions.iter().rev().cloned().enumerate() {
            let selected = active_id.as_deref() == Some(session.session_id.as_str());
            let open = open_ids.contains(&session.session_id);
            let title = session
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| format!("session {}", short_session_id(&session.session_id)));
            let directory = session
                .working_dir
                .as_deref()
                .map(compact_working_dir)
                .unwrap_or_else(|| "unknown folder".into());

            list = list.child(
                div()
                    .id(("sidebar-session", sidebar_index))
                    .mx_2()
                    .mb_1()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(if selected {
                        Theme::ACCENT_DIM
                    } else {
                        Theme::BG
                    })
                    .border_1()
                    .border_color(if selected {
                        Theme::PANEL_BORDER_FOCUS
                    } else {
                        Theme::PANEL_BORDER_IDLE
                    })
                    .hover(|el| el.bg(Theme::HEADER_BG).border_color(Theme::PANEL_BORDER))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            this.activate_session(session.clone(), window, cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(6.0)).rounded_full().bg(if open {
                                Theme::TEXT
                            } else {
                                Theme::TEXT_DIM
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_size(px(12.0))
                                    .child(title),
                            ),
                    )
                    .child(
                        div()
                            .pl(px(14.0))
                            .overflow_hidden()
                            .text_size(px(10.0))
                            .text_color(Theme::TEXT_DIM)
                            .child(directory),
                    ),
            );
        }

        if self.sessions.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_size(px(11.0))
                    .text_color(Theme::TEXT_DIM)
                    .child(if self.connected {
                        "no previous sessions"
                    } else {
                        "loading sessions..."
                    }),
            );
        }

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(Theme::BG)
            .border_r_1()
            .border_color(Theme::PANEL_BORDER)
            .child(
                div()
                    .h(px(54.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(Theme::PANEL_BORDER)
                    .child(div().text_size(px(13.0)).child("sessions"))
                    .child(
                        div()
                            .id("sidebar-new-session")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(16.0))
                            .text_color(Theme::TEXT_DIM)
                            .hover(|el| el.bg(Theme::HEADER_BG).text_color(Theme::TEXT))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.missed("new_panel", cx);
                                    this.open_new_session(cx);
                                }),
                            )
                            .child("+"),
                    ),
            )
            .child(list)
            .into_any_element()
    }

    /// Compact workspace switcher modeled after the user's Waybar module.
    /// Each visible group is a strip and each vertical mark is a session. The
    /// focused session is solid and wider, while the remembered session in an
    /// inactive strip is a half-strength mark. Empty inactive strips stay out
    /// of the way; the active empty strip remains available as a dot.
    fn render_workspace_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut workspaces = div().flex().items_center().gap(px(8.0));

        for row in 0..STRIP_COUNT {
            let indices: Vec<_> = self.row_indices(row).collect();
            let active_row = row == self.active_row;
            if indices.is_empty() && !active_row {
                continue;
            }

            let mut workspace = div()
                .id(("workspace-row", row))
                .h(px(18.0))
                .px(px(3.0))
                .flex()
                .items_center()
                .gap(px(2.0))
                .rounded_full()
                .cursor_pointer()
                .when(active_row, |el| el.bg(Theme::ACCENT_DIM))
                .hover(|el| el.bg(Theme::MINIMAP_TRACK_ACTIVE))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _event, window, cx| {
                        let position = this.active_position_in_row();
                        this.select_row(row, position);
                        this.overview = false;
                        this.overview_progress.set(0.0, Instant::now());
                        this.focus_active(window, cx);
                        cx.notify();
                    }),
                );

            if indices.is_empty() {
                workspace = workspace.child(
                    div()
                        .w(px(4.0))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(Theme::TEXT_DIM),
                );
            }

            for index in indices {
                let focused = index == self.active;
                let busy = self.slots[index].panel.read(cx).is_busy();
                workspace = workspace.child(
                    div()
                        .id(("workspace-session", index))
                        .w(px(if focused { 6.0 } else { 2.0 }))
                        .h(px(12.0))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if focused {
                            Theme::ACCENT
                        } else if active_row || busy {
                            Theme::MINIMAP_PANEL_BUSY
                        } else {
                            Theme::MINIMAP_PANEL
                        })
                        .hover(|el| el.w(px(6.0)).bg(Theme::ACCENT))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                this.set_active(index, cx);
                                this.overview = false;
                                this.overview_progress.set(0.0, Instant::now());
                                this.focus_active(window, cx);
                                cx.notify();
                            }),
                        ),
                );
            }

            workspaces = workspaces.child(workspace);
        }

        div()
            .absolute()
            .top(px(8.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("workspace-bar")
                    .h(px(26.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(Theme::HEADER_BG)
                    .child(workspaces),
            )
            .into_any_element()
    }

    /// The coach's just-in-time hint. It appears next to the status bar rather
    /// than over the transcript, so a suggestion never covers the work that
    /// prompted it, and it states what was observed so the advice is legible.
    fn render_coach_toast(
        &self,
        hint: &learning::Hint,
        progress: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .absolute()
            .bottom(px(44.0))
            .right(px(16.0))
            .w(px(320.0))
            .opacity(progress)
            .child(
                div()
                    .id("coach-toast")
                    .relative()
                    .top(px((1.0 - progress) * 10.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(Theme::PANEL_BG)
                    .border_1()
                    .border_color(Theme::PANEL_BORDER_FOCUS)
                    .rounded_lg()
                    .shadow_lg()
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.dismiss_coach_hint(cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(Theme::HEADER_BG)
                                    .text_size(px(12.0))
                                    .font_family(Theme::FONT_MONO)
                                    .text_color(Theme::TEXT)
                                    .child(hint.keys),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.0))
                                    .text_color(Theme::TEXT)
                                    .child(hint.label),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(Theme::TEXT_DIM)
                            .child(hint.because.clone()),
                    ),
            )
            .into_any_element()
    }

    /// A single skill row: the keys, what they do, and a bar showing how well
    /// the model believes this shortcut is known right now.
    fn render_skill_row(&self, skill: &learning::Skill, mastery: f32) -> gpui::AnyElement {
        let known = mastery >= 0.7;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(150.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .font_family(Theme::FONT_MONO)
                    .text_color(if known { Theme::TEXT_DIM } else { Theme::TEXT })
                    .child(skill.keys),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.0))
                    .text_color(if known { Theme::TEXT_DIM } else { Theme::TEXT })
                    .child(skill.label),
            )
            // The bar is the model's belief, not a usage count: it decays when
            // a shortcut goes unused, so it reads as "how well you know this".
            .child(
                div()
                    .w(px(56.0))
                    .h(px(4.0))
                    .flex_none()
                    .rounded_full()
                    .bg(Theme::PANEL_BORDER)
                    .child(
                        div()
                            .w(relative(mastery.clamp(0.02, 1.0)))
                            .h_full()
                            .rounded_full()
                            .bg(if known { Theme::OK } else { Theme::ACCENT }),
                    ),
            )
            .into_any_element()
    }

    /// The coach view: what the user knows, what they do not, and what to learn
    /// next. This replaces a flat cheat sheet, because the useful information is
    /// which of these the user has not yet made their own.
    fn render_hints_overlay(&self, progress: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let now = learning::now();
        let overall = self.coach.overall_mastery(now);
        let next = self.coach.next_lesson(now);

        let mut card = div()
            .id("hints-card")
            .relative()
            .top(px((1.0 - progress) * 12.0))
            .w(px(560.0))
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .bg(Theme::PANEL_BG)
            .border_1()
            .border_color(Theme::PANEL_BORDER_FOCUS)
            .rounded_xl()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_size(px(18.0)).child("Your workspace fluency"))
                    .child(
                        div()
                            .text_color(Theme::TEXT_DIM)
                            .text_size(px(11.0))
                            .child("Super+/ or F1 to close"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .text_size(px(11.0))
                    .text_color(Theme::TEXT_DIM)
                    .child(format!("{}% learned", (overall * 100.0).round() as u32))
                    .child(format!("{} keystrokes saved", self.coach.effort_saved))
                    .child(format!("{} spent the long way", self.coach.effort_wasted)),
            );

        if let Some(next) = next {
            card = card.child(
                div()
                    .p_2p5()
                    .rounded_lg()
                    .bg(Theme::HEADER_BG)
                    .border_1()
                    .border_color(Theme::PANEL_BORDER)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(Theme::TEXT_DIM)
                            .child("learn next"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_family(Theme::FONT_MONO)
                                    .text_size(px(12.0))
                                    .child(next.keys),
                            )
                            .child(div().text_size(px(12.0)).child(next.label)),
                    ),
            );
        }

        card = card.child(
            div()
                .p_2p5()
                .rounded_lg()
                .bg(Theme::HEADER_BG)
                .text_size(px(11.0))
                .text_color(Theme::TEXT_DIM)
                .child("Composer: ↑/↓ history · Ctrl+K/J prompts · Ctrl+W word delete · Alt+B/F word move · Ctrl+U delete to start · Ctrl/Cmd+Z undo · Esc clear"),
        );

        for (area, rows) in self.coach.report(now) {
            card = card.child(
                div()
                    .mt_1()
                    .text_size(px(10.0))
                    .text_color(Theme::TEXT_DIM)
                    .child(area.label()),
            );
            for (skill, mastery) in rows {
                card = card.child(self.render_skill_row(skill, mastery));
            }
        }

        card = card.child(
            div()
                .id("new-help-session")
                .mt_3()
                .p_3()
                .rounded_lg()
                .bg(Theme::HEADER_BG)
                .border_1()
                .border_color(Theme::PANEL_BORDER)
                .cursor_pointer()
                .hover(|el| el.border_color(Theme::ACCENT))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _event, window, cx| {
                        this.new_help_session(&NewHelpSession, window, cx);
                    }),
                )
                .child("Ask Jcode about the app")
                .child(
                    div()
                        .mt_1()
                        .text_size(px(11.0))
                        .text_color(Theme::TEXT_DIM)
                        .child("Opens a new session with the hints and bundled docs loaded into context."),
                ),
        );

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .opacity(progress)
            .bg(gpui::rgba(0x000000b8))
            .child(card)
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.dump_state();
        if self.focus_pending && !self.slots.is_empty() {
            self.focus_pending = false;
            self.focus_active(window, cx);
        }
        let viewport = window.viewport_size();
        let viewport_w = (f32::from(viewport.width) - SIDEBAR_WIDTH).max(320.0);
        let viewport_h = f32::from(viewport.height);

        let now = Instant::now();
        let overview_progress = self.overview_progress.sample(now);
        let hints_progress = self.hints_progress.sample(now);
        // Expire the hint on a schedule of its own, so a suggestion the user
        // ignores fades without needing another input to clear it.
        let coach_hint = self.coach.active_hint(learning::now());
        if coach_hint.is_none() {
            self.coach_progress.set(0.0, now);
        }
        let coach_progress = self.coach_progress.sample(now);
        if self.overview_progress.is_animating()
            || self.hints_progress.is_animating()
            || self.coach_progress.is_animating()
            // Keep ticking while a hint is up so it can expire on its own.
            || coach_hint.is_some()
        {
            window.request_animation_frame();
        }

        let content = if overview_progress > 0.0 {
            div()
                .size_full()
                .opacity(overview_progress)
                .child(self.render_overview(cx))
                .into_any_element()
        } else if self.row_indices(self.active_row).next().is_none() {
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .items_center()
                .justify_center()
                .text_color(Theme::TEXT_DIM)
                .child(if self.connected {
                    format!(
                        "strip {} is empty - super-n opens a session here",
                        self.active_row + 1
                    )
                } else {
                    "connecting to jcode...".into()
                })
                .child(div().text_size(px(12.0)).child(self.status.clone()))
                .into_any_element()
        } else {
            self.render_strip(viewport_w, viewport_h, window, cx)
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(Theme::BG)
            .font_family(Theme::FONT_UI)
            .text_size(px(14.0))
            .text_color(Theme::TEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .on_action(cx.listener(Self::focus_first))
            .on_action(cx.listener(Self::focus_last))
            .on_action(cx.listener(Self::focus_previous))
            .on_action(cx.listener(Self::move_panel_left))
            .on_action(cx.listener(Self::move_panel_right))
            .on_action(cx.listener(Self::move_panel_up))
            .on_action(cx.listener(Self::move_panel_down))
            .on_action(cx.listener(Self::move_panel_to_first))
            .on_action(cx.listener(Self::move_panel_to_last))
            .on_action(cx.listener(Self::new_panel))
            .on_action(cx.listener(Self::close_panel))
            .on_action(cx.listener(Self::toggle_overview))
            .on_action(cx.listener(Self::toggle_hints))
            .on_action(cx.listener(Self::new_help_session))
            .on_action(cx.listener(Self::cycle_width))
            .on_action(cx.listener(Self::maximize_width))
            .on_action(cx.listener(|this, _: &WidthPreset1, _w, cx| this.set_width(0.25, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset2, _w, cx| this.set_width(0.5, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset3, _w, cx| this.set_width(0.75, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset4, _w, cx| this.set_width(1.0, cx)))
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(content)
                    .child(self.render_workspace_bar(cx)),
            )
            .when(hints_progress > 0.0, |root| {
                root.child(self.render_hints_overlay(hints_progress, cx))
            })
            .when_some(coach_hint.filter(|_| coach_progress > 0.0), |root, hint| {
                root.child(self.render_coach_toast(&hint, coach_progress, cx))
            })
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

fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

fn compact_working_dir(path: &str) -> String {
    let home = std::env::var("HOME").ok();
    if let Some(home) = home.as_deref()
        && let Some(relative) = path.strip_prefix(home)
    {
        return format!("~{relative}");
    }
    path.to_owned()
}

/// A one-line description of the strip layout, for tests and for the
/// `JCODE_DESKTOP_STATE` debug dump: `strip=<row> focus=<pos> widths=a,b,c`.
fn describe_strip(widths: &[f32], focus_position: Option<usize>, row: usize) -> String {
    let widths = widths
        .iter()
        .map(|w| format!("{w:.2}"))
        .collect::<Vec<_>>()
        .join(",");
    let focus = focus_position
        .map(|p| p.to_string())
        .unwrap_or_else(|| "-".into());
    format!("strip={row} focus={focus} widths={widths}")
}

/// The coach's observable state, for the diagnostic dump.
fn describe_coach(mastery: f32, saved: u32, wasted: u32, teaching: Option<&'static str>) -> String {
    format!(
        "coach mastery={:.2} saved={saved} wasted={wasted} teaching={}",
        mastery,
        teaching.unwrap_or("-")
    )
}

/// CSS `ease-out-expo`, the curve used across the user's niri animations.
fn ease_out_expo(t: f32) -> f32 {
    if t >= 1.0 {
        1.0
    } else {
        1.0 - 2f32.powf(-10.0 * t)
    }
}

/// Where a newly created panel is inserted: directly right of the focused
/// panel when it is on this strip, otherwise after the strip's last panel.
/// `row_last` is the index of the strip's rightmost panel, if any.
/// A click that jumps this many panels or more, landing on an end of the strip,
/// is the work `super-home`/`super-end` exists for. Shorter hops are ordinary
/// left/right navigation.
const LONG_HOP: usize = 3;

/// Which shortcut a click-to-focus bypassed, given how far it jumped and where
/// it landed.
fn click_skill(steps: usize, landed_at: usize, strip_len: usize) -> &'static str {
    let at_end = landed_at == 0 || landed_at + 1 == strip_len;
    if steps >= LONG_HOP && at_end {
        "focus_first_last"
    } else {
        "focus_left_right"
    }
}

fn insert_index(
    active: usize,
    active_is_on_strip: bool,
    row_last: Option<usize>,
    slot_count: usize,
) -> usize {
    if active_is_on_strip {
        active + 1
    } else {
        row_last.map(|last| last + 1).unwrap_or(slot_count)
    }
}

/// After closing the panel at `closed`, niri focuses the panel that slid into
/// its place (the right neighbour), falling back to the new rightmost panel.
/// `remaining` are the strip's indices after removal.
fn focus_after_close(closed: usize, remaining: &[usize]) -> usize {
    match remaining.iter().find(|&&index| index >= closed) {
        Some(&right) => right,
        None => remaining.last().copied().unwrap_or(0),
    }
}

/// niri `switch-preset-column-width`: the next preset above `current`,
/// wrapping back to the narrowest.
fn next_preset(current: f32) -> f32 {
    PRESET_WIDTHS
        .iter()
        .copied()
        .find(|preset| *preset > current + 0.01)
        .unwrap_or(PRESET_WIDTHS[0])
}

/// niri `maximize-column`: fill the viewport, or return to the stored width.
/// Returns the new `(width, restore)` pair. A panel that is already full width
/// with nothing stored (for example after `super-4`) toggles to the default
/// width, so the key is never a no-op.
fn toggle_maximize(width: f32, restore: Option<f32>) -> (f32, Option<f32>) {
    match restore {
        Some(restore) => (restore, None),
        None if width >= 1.0 => (DEFAULT_WIDTH, None),
        None => (1.0, Some(width)),
    }
}

/// niri `center-focused-column "never"`: scroll the least amount that brings
/// `[left, left + width]` fully into a `viewport`-wide window at `current`.
fn scroll_into_view(current: f32, left: f32, width: f32, viewport: f32) -> f32 {
    if width + STRUT * 2.0 >= viewport {
        return left - STRUT;
    }
    if left - STRUT < current {
        left - STRUT
    } else if left + width + STRUT > current + viewport {
        left + width + STRUT - viewport
    } else {
        current
    }
}

/// Two-finger pan: move the camera by `delta`, clamped to the same range the
/// keyboard camera uses, so a swipe can never fling the strip off into space.
fn pan_camera(current: f32, delta: f32, total_width: f32, viewport: f32) -> f32 {
    let max_scroll = (total_width - viewport).max(-GAP).max(-STRUT);
    (current + delta).clamp(-STRUT, max_scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_panel_lands_right_of_the_focused_one() {
        // Focused panel is index 1 of a three-panel strip: the new panel takes
        // index 2, pushing the old index 2 to the right.
        assert_eq!(insert_index(1, true, Some(2), 3), 2);
        // Focused panel is the rightmost: append.
        assert_eq!(insert_index(2, true, Some(2), 3), 3);
        // Focus is on another strip: append after that strip's last panel.
        assert_eq!(insert_index(0, false, Some(4), 6), 5);
        // Empty strip: append at the end of the slot list.
        assert_eq!(insert_index(0, false, None, 3), 3);
    }

    #[test]
    fn closing_focuses_the_right_neighbour() {
        // Closed index 1 of [0,1,2]; remaining strip indices are [0,1] and the
        // panel formerly at 2 now sits at 1, so focus stays at 1.
        assert_eq!(focus_after_close(1, &[0, 1]), 1);
        // Closed the rightmost: fall back to the new rightmost.
        assert_eq!(focus_after_close(2, &[0, 1]), 1);
        // Closed the only panel.
        assert_eq!(focus_after_close(0, &[]), 0);
    }

    #[test]
    fn width_presets_cycle_and_wrap() {
        assert_eq!(next_preset(0.25), 0.5);
        assert_eq!(next_preset(0.5), 0.75);
        assert_eq!(next_preset(0.75), 0.25);
        // A maximized panel wraps to the narrowest preset.
        assert_eq!(next_preset(1.0), 0.25);
    }

    #[test]
    fn maximize_toggles_back_to_the_previous_width() {
        // Maximize a half-width panel, then restore it.
        let (width, restore) = toggle_maximize(0.5, None);
        assert_eq!((width, restore), (1.0, Some(0.5)));
        assert_eq!(toggle_maximize(width, restore), (0.5, None));
        // A quarter-width panel restores to a quarter.
        let (width, restore) = toggle_maximize(0.25, None);
        assert_eq!(toggle_maximize(width, restore), (0.25, None));
        // Already full width with nothing stored: toggle to the default so the
        // key is never a no-op.
        assert_eq!(toggle_maximize(1.0, None), (DEFAULT_WIDTH, None));
        // Round trip from there still works.
        let (width, restore) = toggle_maximize(DEFAULT_WIDTH, None);
        assert_eq!(width, 1.0);
        assert_eq!(toggle_maximize(width, restore).0, DEFAULT_WIDTH);
    }

    #[test]
    fn strip_description_reports_focus_and_widths() {
        assert_eq!(
            describe_strip(&[0.5, 0.25], Some(1), 2),
            "strip=2 focus=1 widths=0.50,0.25"
        );
        // An empty strip has no focused position.
        assert_eq!(describe_strip(&[], None, 0), "strip=0 focus=- widths=");
    }

    #[test]
    fn two_finger_pan_moves_and_clamps_the_camera() {
        // Total content 2000 wide in a 1000 viewport: pan freely inside range.
        assert_eq!(pan_camera(100.0, 250.0, 2000.0, 1000.0), 350.0);
        // Panning left of the start clamps at the strut.
        assert_eq!(pan_camera(50.0, -500.0, 2000.0, 1000.0), -STRUT);
        // Panning past the end clamps at the last panel's right edge.
        assert_eq!(pan_camera(900.0, 500.0, 2000.0, 1000.0), 1000.0);
        // Content narrower than the viewport cannot pan at all.
        assert_eq!(pan_camera(0.0, 300.0, 500.0, 1000.0), -GAP);
    }

    #[test]
    fn camera_never_centers_and_scrolls_minimally() {
        let viewport = 1000.0;
        // Fully visible: do not move.
        assert_eq!(scroll_into_view(0.0, 100.0, 400.0, viewport), 0.0);
        // Off to the right: bring its right edge to the viewport edge, which is
        // not the centered position (that would be 400.0).
        let target = scroll_into_view(0.0, 900.0, 400.0, viewport);
        assert!((target - (900.0 + 400.0 + STRUT - viewport)).abs() < 0.01);
        // Off to the left: bring its left edge to the viewport edge.
        let target = scroll_into_view(900.0, 100.0, 400.0, viewport);
        assert!((target - (100.0 - STRUT)).abs() < 0.01);
        // Wider than the viewport: pin to its left edge.
        let target = scroll_into_view(0.0, 500.0, 1200.0, viewport);
        assert!((target - (500.0 - STRUT)).abs() < 0.01);
    }

    #[test]
    fn camera_easing_is_monotonic_and_settles() {
        assert_eq!(ease_out_expo(0.0), 0.0);
        assert_eq!(ease_out_expo(1.0), 1.0);
        let mut previous = 0.0;
        for step in 1..=10 {
            let value = ease_out_expo(step as f32 / 10.0);
            assert!(value > previous, "easing must increase at {step}");
            previous = value;
        }
        // Ease-out: half the distance is covered in the first 10%, and three
        // quarters within 20%.
        assert!((ease_out_expo(0.1) - 0.5).abs() < 0.001);
        assert!((ease_out_expo(0.2) - 0.75).abs() < 0.001);
    }

    #[test]
    fn help_session_prompt_carries_docs_instruction_and_visible_hints() {
        assert!(HELP_SESSION_PROMPT.contains("bundled Jcode documentation"));
        assert!(HELP_SESSION_PROMPT.contains("Super+H/J/K/L"));
        assert!(HELP_SESSION_PROMPT.contains("Super+/ or F1"));
        assert!(HELP_SESSION_PROMPT.contains("Super+Shift+/"));
    }

    /// Drive real keystrokes through the real keymap and confirm the coach
    /// draws the right conclusions. This is the acceptance path: it exercises
    /// the bindings the user actually presses, not the handlers directly.
    #[gpui::test]
    fn real_keystrokes_teach_the_jump_shortcut_when_the_user_grinds(cx: &mut gpui::TestAppContext) {
        // A user who knows super-h/l well but has never used super-home/end.
        let now = learning::now();
        let mut coach = learning::Coach::new();
        for step in 0..8 {
            coach.used_shortcut("focus_left_right", now - (8 - step) * 3 * 86_400);
        }
        assert!(coach.mastery("focus_left_right", now) >= 0.7);

        let window = cx.update(|cx| {
            crate::bind_workspace_keys(cx);
            cx.open_window(gpui::WindowOptions::default(), |window, cx| {
                cx.new(|cx| {
                    let mut workspace = Workspace::for_test(coach, cx);
                    for name in ["one", "two", "three", "four", "five"] {
                        workspace.push_test_panel(name, cx);
                    }
                    let _ = window;
                    workspace
                })
            })
            .unwrap()
        });
        window
            .update(cx, |workspace, window, cx| {
                window.focus(&workspace.focus_handle, cx);
            })
            .unwrap();

        // Start at the right-hand end of the strip.
        cx.simulate_keystrokes(*window, "super-end");
        window
            .update(cx, |workspace, _, _| {
                assert_eq!(workspace.test_focus_position(), Some(4));
            })
            .unwrap();

        // Now grind back one panel at a time, which is what someone who does
        // not know super-home does.
        cx.simulate_keystrokes(*window, "super-h super-h super-h");
        window
            .update(cx, |workspace, _, _| {
                assert_eq!(workspace.test_focus_position(), Some(1));
                let coach = workspace.test_coach();
                assert_eq!(
                    coach.active_hint_id(),
                    Some("focus_first_last"),
                    "grinding should teach the jump shortcut"
                );
                assert!(
                    coach.effort_wasted > 0,
                    "grinding should be counted as wasted effort"
                );
            })
            .unwrap();
    }

    /// Moving onto a populated strip is real navigation and should be credited,
    /// which is the counterpart to the no-op case below.
    #[gpui::test]
    fn moving_to_a_populated_strip_is_credited(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            crate::bind_workspace_keys(cx);
            cx.open_window(gpui::WindowOptions::default(), |window, cx| {
                cx.new(|cx| {
                    let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
                    workspace.push_test_panel("top", cx);
                    // A panel on the strip below.
                    workspace.active_row = 1;
                    workspace.push_test_panel("below", cx);
                    workspace.active_row = 0;
                    workspace.active = 0;
                    let _ = window;
                    workspace
                })
            })
            .unwrap()
        });
        window
            .update(cx, |workspace, window, cx| {
                window.focus(&workspace.focus_handle, cx);
            })
            .unwrap();

        cx.simulate_keystrokes(*window, "super-j");
        window
            .update(cx, |workspace, _, _| {
                assert_eq!(workspace.active_row, 1);
                assert!(
                    workspace
                        .test_coach()
                        .mastery("focus_up_down", learning::now())
                        > 0.0,
                    "landing on a populated strip should count"
                );
            })
            .unwrap();
    }

    /// The same harness, confirming a no-op keypress teaches nothing and earns
    /// nothing: pressing into the edge of a strip is not evidence either way.
    #[gpui::test]
    fn a_keypress_that_does_nothing_changes_no_belief(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            crate::bind_workspace_keys(cx);
            cx.open_window(gpui::WindowOptions::default(), |window, cx| {
                cx.new(|cx| {
                    let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
                    workspace.push_test_panel("only", cx);
                    let _ = window;
                    workspace
                })
            })
            .unwrap()
        });
        window
            .update(cx, |workspace, window, cx| {
                window.focus(&workspace.focus_handle, cx);
            })
            .unwrap();

        // One panel: every navigation key is a no-op.
        cx.simulate_keystrokes(*window, "super-h super-l super-j super-k");
        window
            .update(cx, |workspace, _, _| {
                let coach = workspace.test_coach();
                assert_eq!(coach.overall_mastery(learning::now()), 0.0);
                assert_eq!(coach.effort_saved, 0);
                assert_eq!(coach.effort_wasted, 0);
                assert_eq!(coach.active_hint_id(), None);
            })
            .unwrap();
    }

    /// Using a shortcut for real should register as knowledge, through the same
    /// keymap the user types on.
    #[gpui::test]
    fn real_keystrokes_build_recognized_mastery(cx: &mut gpui::TestAppContext) {
        let window = cx.update(|cx| {
            crate::bind_workspace_keys(cx);
            cx.open_window(gpui::WindowOptions::default(), |window, cx| {
                cx.new(|cx| {
                    let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
                    for name in ["one", "two", "three"] {
                        workspace.push_test_panel(name, cx);
                    }
                    let _ = window;
                    workspace
                })
            })
            .unwrap()
        });
        window
            .update(cx, |workspace, window, cx| {
                window.focus(&workspace.focus_handle, cx);
            })
            .unwrap();

        cx.simulate_keystrokes(*window, "super-l super-l super-f super-r");
        window
            .update(cx, |workspace, _, _| {
                let coach = workspace.test_coach();
                let now = learning::now();
                assert!(
                    coach.mastery("focus_left_right", now) > 0.0,
                    "navigating should register"
                );
                assert!(
                    coach.mastery("maximize", now) > 0.0,
                    "super-f should register"
                );
                assert!(
                    coach.mastery("cycle_width", now) > 0.0,
                    "super-r should register"
                );
                assert!(coach.effort_saved > 0);
                assert_eq!(coach.effort_wasted, 0, "no slow paths were taken");
            })
            .unwrap();
    }

    #[test]
    fn a_short_click_hop_is_read_as_plain_navigation() {
        // Clicking the neighbour is what super-h/l would have done.
        assert_eq!(click_skill(1, 1, 5), "focus_left_right");
        assert_eq!(click_skill(2, 2, 5), "focus_left_right");
        // A long hop that stops mid-strip is still ordinary navigation: no
        // single "jump" key would have landed there.
        assert_eq!(click_skill(4, 4, 9), "focus_left_right");
    }

    #[test]
    fn a_long_click_hop_to_an_end_is_read_as_a_missed_jump() {
        // Crossing the strip to its far end is what super-end exists for.
        assert_eq!(click_skill(4, 4, 5), "focus_first_last");
        assert_eq!(click_skill(3, 0, 5), "focus_first_last");
    }

    #[test]
    fn every_instrumented_skill_id_exists_in_the_catalog() {
        // The instrumentation refers to skills by string, so a typo would
        // silently stop teaching. Keep the two in step.
        for id in [
            "focus_left_right",
            "focus_up_down",
            "focus_first_last",
            "focus_previous",
            "overview",
            "move_panel",
            "move_panel_strip",
            "move_panel_end",
            "cycle_width",
            "maximize",
            "width_presets",
            "new_panel",
            "close_panel",
        ] {
            assert!(learning::skill(id).is_some(), "unknown skill id {id}");
        }
    }

    #[test]
    fn the_catalog_covers_every_bound_workspace_action() {
        // Every shortcut the app binds should be teachable, or the coach will
        // report fluency it never actually measured.
        let taught: Vec<&str> = learning::SKILLS.iter().map(|skill| skill.keys).collect();
        for keys in [
            "super-h / super-l",
            "super-j / super-k",
            "super-home / super-end",
            "super-tab",
            "super-o",
            "super-shift-h / super-shift-l",
            "super-shift-j / super-shift-k",
            "super-shift-home / super-shift-end",
            "super-r",
            "super-f",
            "super-1 .. super-4",
            "super-n",
            "super-q",
        ] {
            assert!(taught.contains(&keys), "{keys} is bound but never taught");
        }
    }
}
