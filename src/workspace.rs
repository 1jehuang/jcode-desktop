//! Workspace: four niri-inspired horizontal strips of session panels with a
//! smooth camera and a zoomed-out overview.
//!
//! Panels live on one of four infinite horizontal strips. Focus moves
//! left/right within a strip and up/down between strips.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Window, actions, div, prelude::*, px, relative,
};

use crate::accounts;
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
        OpenFolder,
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
- Ctrl+O: choose a folder and open a session there
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

// Minimap: a rounded square card in the top right that maps every strip to
// scale, preserving the canvas aspect ratio so panels taller than wide on
// screen stay taller than wide on the map.
const MINIMAP_SIZE: f32 = 96.0;
const MINIMAP_PADDING: f32 = 5.0;
const MINIMAP_ROW_GAP: f32 = 3.0;
const MINIMAP_TOP: f32 = 8.0;
const MINIMAP_RIGHT: f32 = 12.0;
const COACH_TOAST_GAP: f32 = 8.0;
const COACH_TOAST_WIDTH: f32 = 288.0;
/// Rows split the square's inner height evenly, one per strip.
const MINIMAP_ROW_HEIGHT: f32 =
    (MINIMAP_SIZE - MINIMAP_PADDING * 2.0 - MINIMAP_ROW_GAP * (STRIP_COUNT as f32 - 1.0))
        / STRIP_COUNT as f32;
/// Vertical inset between a panel rectangle and its track edge.
const MINIMAP_PANEL_INSET: f32 = 1.5;

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
    /// Last focused panel on each strip, so vertical navigation restores the
    /// place the user left instead of choosing by column position.
    row_focus: [Option<gpui::EntityId>; STRIP_COUNT],
    /// Row being animated out and progress of the incoming row.
    outgoing_row: Option<usize>,
    row_progress: AnimatedValue,
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
    /// Configured logins and API keys, refreshed in the background.
    accounts: Vec<accounts::Account>,
    status: String,
    connected: bool,
    focus_handle: FocusHandle,
    /// Focus the active panel's input on the next render (set when panels
    /// appear from background updates, where no Window is available).
    focus_pending: bool,
    /// Directory currently shown by the in-app folder browser. `None` closes it.
    folder_picker_dir: Option<PathBuf>,
    folder_picker_error: Option<String>,
    _poll_task: gpui::Task<()>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::input::bind_keys(cx);
        let bridge = harness::spawn();
        let accounts_feed = accounts::spawn();

        // Poll bridge updates ~60 times per second while anything is pending.
        let poll_bridge = bridge.clone();
        let poll_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let updates = poll_bridge.drain();
                let accounts = accounts_feed.latest();
                if updates.is_empty() && accounts.is_none() {
                    continue;
                }
                let outcome = this.update(cx, |workspace: &mut Workspace, cx| {
                    for update in updates {
                        workspace.apply(update, cx);
                    }
                    if let Some(accounts) = accounts {
                        workspace.accounts = accounts;
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
            row_focus: [None; STRIP_COUNT],
            outgoing_row: None,
            row_progress: AnimatedValue::new(1.0, transition::policy(Transition::Row).duration),
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
            accounts: Vec::new(),
            status: "starting...".into(),
            connected: false,
            focus_handle: cx.focus_handle(),
            focus_pending: false,
            folder_picker_dir: None,
            folder_picker_error: None,
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
            row_focus: [None; STRIP_COUNT],
            outgoing_row: None,
            row_progress: AnimatedValue::new(1.0, transition::policy(Transition::Row).duration),
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
            accounts: Vec::new(),
            status: "test".into(),
            connected: true,
            focus_handle: cx.focus_handle(),
            focus_pending: false,
            folder_picker_dir: None,
            folder_picker_error: None,
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

    /// Seed accounts for tests, bypassing the CLI.
    #[cfg(test)]
    pub fn set_test_accounts(&mut self, accounts: Vec<accounts::Account>) {
        self.accounts = accounts;
    }

    #[cfg(test)]
    pub fn test_coach(&self) -> &learning::Coach {
        &self.coach
    }

    /// The panel entity at `index`, so tests can drive real events through it.
    #[cfg(test)]
    pub fn test_panel(&self, index: usize) -> Option<Entity<Panel>> {
        self.slots.get(index).map(|slot| slot.panel.clone())
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
                        images: Vec::new(),
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
            Update::SessionConnected { session_id } => {
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == session_id {
                        slot.panel.update(cx, |panel, cx| {
                            // A subsequent SessionStatus event will replace this
                            // with idle/running. Clear the stale lost banner now.
                            panel.status = "connected".into();
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
        let width_fraction = spawned_panel_width(self.slots.len());
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
            width_fraction,
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
            .set(width_fraction, Instant::now());
        // The lone panel that had the whole viewport steps back to the default
        // width now that it has company, leaving two equal halves.
        let strip_panels = self.row_indices(self.active_row).count();
        let now = Instant::now();
        for index in self.row_indices(self.active_row).collect::<Vec<_>>() {
            if index == insert_at {
                continue;
            }
            let slot = &mut self.slots[index];
            let demoted = demoted_width(slot.width_fraction, strip_panels);
            if demoted != slot.width_fraction {
                slot.width_fraction = demoted;
                slot.animated_width.set(demoted, now);
                slot.restore_fraction = None;
            }
        }
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
        self.row_focus[self.active_row] = Some(self.slots[index].panel.entity_id());
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
        let remembered = self.row_focus[self.active_row].and_then(|entity_id| {
            self.slots
                .iter()
                .position(|slot| slot.row == self.active_row && slot.panel.entity_id() == entity_id)
        });
        let selected = remembered.or_else(|| {
            self.row_indices(self.active_row)
                .nth(preferred_position)
                .or_else(|| self.row_indices(self.active_row).last())
        });
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
            self.row_focus[self.active_row] = Some(self.slots[index].panel.entity_id());
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
        let outgoing_row = self.active_row;
        let position = self.active_position_in_row();
        let was_active = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.entity_id());
        self.select_row(row, position);
        self.outgoing_row = Some(outgoing_row);
        let now = Instant::now();
        self.row_progress = AnimatedValue::new(0.0, transition::policy(Transition::Row).duration);
        self.row_progress.set(1.0, now);
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
        let outgoing_row = self.active_row;
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
        self.row_focus[self.active_row] = Some(slot.panel.entity_id());
        self.outgoing_row = Some(outgoing_row);
        let now = Instant::now();
        self.row_progress = AnimatedValue::new(0.0, transition::policy(Transition::Row).duration);
        self.row_progress.set(1.0, now);
        self.retarget_camera();
        self.focus_active(window, cx);
        self.learned("move_panel_strip", cx);
        cx.notify();
    }

    fn new_panel(&mut self, _: &NewPanel, _: &mut Window, cx: &mut Context<Self>) {
        self.learned("new_panel", cx);
        self.open_new_session(cx);
    }

    fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let width_fraction = spawned_panel_width(self.slots.len());
        let panel =
            cx.new(|cx| Panel::new_terminal(default_working_dir(), self.bridge.clone(), cx));
        let insert_at = if self.slots.is_empty() {
            0
        } else {
            self.active + 1
        };
        self.slots.insert(
            insert_at,
            Slot {
                panel,
                row: self.active_row,
                width_fraction,
                animated_width: AnimatedValue::new(
                    width_fraction,
                    transition::policy(Transition::PanelOpen).duration,
                ),
                restore_fraction: None,
            },
        );
        self.set_active(insert_at, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Open Jcode's own directory browser. This deliberately does not call the
    /// platform path prompt, so the workflow stays inside the application.
    /// A session's working directory is fixed by the runtime, so choosing a
    /// different folder intentionally opens a new session instead of silently
    /// changing the meaning of an existing transcript.
    fn open_folder(&mut self, _: &OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        self.folder_picker_dir = Some(
            default_working_dir()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
        );
        self.folder_picker_error = None;
        cx.notify();
    }

    fn browse_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match directory_entries(&path) {
            Ok(_) => {
                self.folder_picker_dir = Some(path);
                self.folder_picker_error = None;
            }
            Err(error) => self.folder_picker_error = Some(error),
        }
        cx.notify();
    }

    fn choose_browsed_folder(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.folder_picker_dir.take() else {
            return;
        };
        self.folder_picker_error = None;
        self.bridge.send(Command::CreateSession {
            working_dir: Some(path.to_string_lossy().into_owned()),
        });
        cx.notify();
    }

    fn close_folder_picker(&mut self, cx: &mut Context<Self>) {
        self.folder_picker_dir = None;
        self.folder_picker_error = None;
        cx.notify();
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

    fn render_row(
        &mut self,
        row: usize,
        viewport_w: f32,
        viewport_h: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.row_indices(row).next().is_some() {
            self.render_strip(row, viewport_w, viewport_h, window, cx)
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .items_center()
                .justify_center()
                .text_color(Theme::TEXT_DIM)
                .child(if self.connected {
                    format!("strip {} is empty - super-n opens a session here", row + 1)
                } else {
                    "connecting to jcode...".into()
                })
                .child(div().text_size(px(12.0)).child(self.status.clone()))
                .into_any_element()
        }
    }

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
        row: usize,
        viewport_w: f32,
        viewport_h: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if row == self.active_row && self.camera_dirty[row] {
            self.resolve_camera_target(viewport_w);
        }
        // Animate the camera over CAMERA_DURATION on an ease-out-expo curve,
        // matching niri's animation settings.
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
        let indices = self.row_indices(row).collect::<Vec<_>>();
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
            .left(px(-self.camera_x[row]))
            .flex()
            .flex_row()
            .gap(px(GAP));

        for (index, width) in indices.into_iter().zip(animated_widths) {
            let slot = &self.slots[index];
            let focused = index == self.active;
            strip = strip.child(
                div()
                    .id(("panel", index))
                    // Tagged so a render test can click the real panel element
                    // and exercise the pointer slow-path detection.
                    .debug_selector(move || format!("panel-{index}"))
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
                    // Keep keyboard/input focus attached to what the gesture has
                    // actually brought under the camera. Without this, a swipe
                    // only moved the pixels: the old off-screen panel remained
                    // active and the next keyboard action snapped back to it.
                    if let Some(index) = panel_at_viewport_center(
                        this.slots.iter().enumerate().filter_map(|(index, slot)| {
                            (slot.row == row).then_some((index, slot.width_fraction))
                        }),
                        next,
                        viewport_w,
                    ) && index != this.active
                    {
                        if let Some(outgoing) = this.slots.get(this.active) {
                            this.previous = Some(outgoing.panel.entity_id());
                        }
                        this.active = index;
                        this.active_row = row;
                        this.focus_active(window, cx);
                    }
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
            let (icon, title) = sidebar_session_title(&session);
            let directory = session
                .working_dir
                .as_deref()
                .map(str::trim)
                .filter(|directory| !directory.is_empty())
                .map(compact_working_dir);

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
                            .child(div().text_size(px(14.0)).child(icon))
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
                    .when_some(directory, |row, directory| {
                        row.child(
                            div()
                                .pl(px(36.0))
                                .overflow_hidden()
                                .text_size(px(10.0))
                                .text_color(Theme::TEXT_DIM)
                                .child(directory),
                        )
                    }),
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
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("sidebar-open-folder")
                                    .debug_selector(|| "sidebar-open-folder".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(Theme::TEXT_DIM)
                                    .hover(|el| el.bg(Theme::HEADER_BG).text_color(Theme::TEXT))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _event, window, cx| {
                                            this.open_folder(&OpenFolder, window, cx);
                                        }),
                                    )
                                    .child("folder"),
                            )
                            .child(
                                div()
                                    .id("sidebar-new-session")
                                    // Tagged so a render test can click the real button
                                    // and confirm it counts as a slow path, not as
                                    // knowledge of super-n.
                                    .debug_selector(|| "sidebar-new-session".into())
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
                    ),
            )
            .child(list)
            .when_some(self.render_accounts(), |el, accounts| el.child(accounts))
            .into_any_element()
    }

    /// A mouse-friendly spawn target at the canvas edge. It stays invisible
    /// until the pointer reaches the far right, then reveals the same `+`
    /// affordance as the session sidebar.
    fn render_edge_new_session(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .id("edge-new-session")
            .debug_selector(|| "edge-new-session".into())
            .absolute()
            .right_0()
            .top_0()
            .bottom_0()
            .w(px(32.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .opacity(0.0)
            .hover(|el| el.opacity(1.0))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.missed("new_panel", cx);
                    this.open_new_session(cx);
                }),
            )
            .child(
                div()
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(Theme::PANEL_BORDER)
                    .bg(Theme::HEADER_BG)
                    .text_size(px(20.0))
                    .text_color(Theme::TEXT)
                    .child("+"),
            )
            .into_any_element()
    }

    /// The connected-accounts strip: one row per configured credential, led
    /// by the provider's logo, with the auth method (OAuth / API key) and
    /// state. Only configured providers appear, so the section stays honest
    /// and short: it answers "what am I logged into right now".
    fn render_accounts(&self) -> Option<gpui::AnyElement> {
        if self.accounts.is_empty() {
            return None;
        }

        let mut section = div()
            .flex_none()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(Theme::PANEL_BORDER)
            .py_2()
            .child(
                div()
                    .px_4()
                    .pb_1()
                    .text_size(px(10.0))
                    .text_color(Theme::TEXT_DIM)
                    .child("accounts"),
            );

        for (index, account) in self.accounts.iter().enumerate() {
            let available = account.available();
            let ink = if available {
                Theme::TEXT
            } else {
                Theme::TEXT_FAINT
            };

            let logo: gpui::AnyElement = match accounts::logo(&account.id) {
                Some(bytes) => gpui::svg()
                    .data(bytes)
                    .size(px(16.0))
                    .flex_none()
                    .text_color(ink)
                    .into_any_element(),
                None => div()
                    .size(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(Theme::INLINE_CODE_BG)
                    .text_size(px(10.0))
                    .text_color(ink)
                    .child(accounts::lettermark(&account.display_name))
                    .into_any_element(),
            };

            section = section.child(
                div()
                    .id(("account", index))
                    .debug_selector(|| format!("account-{}", account.id))
                    .mx_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .hover(|el| el.bg(Theme::HEADER_BG))
                    .child(logo)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(11.0))
                            .text_color(ink)
                            .child(account.display_name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(9.0))
                            .text_color(Theme::TEXT_DIM)
                            .child(if available {
                                account.auth_kind.clone()
                            } else {
                                format!("{} · expired", account.auth_kind)
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .size(px(5.0))
                            .rounded_full()
                            .bg(if available {
                                Theme::OK
                            } else {
                                Theme::TEXT_FAINT
                            }),
                    ),
            );
        }

        Some(section.into_any_element())
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

    /// The minimap: a rounded card in the top right that draws every strip to
    /// scale. Panels are proportional rectangles, the focused panel is lit, a
    /// lens shows where the camera is looking, clicking a panel jumps to it,
    /// clicking a track switches strips, and scrolling over the map pans the
    /// active strip.
    fn render_minimap(
        &self,
        viewport_w: f32,
        viewport_h: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let track_w = MINIMAP_SIZE - MINIMAP_PADDING * 2.0;
        let panel_track_h = MINIMAP_ROW_HEIGHT - MINIMAP_PANEL_INSET * 2.0;
        let widest = (0..STRIP_COUNT)
            .map(|row| {
                self.row_indices(row)
                    .map(|index| self.slot_width(index, viewport_w) + GAP)
                    .sum::<f32>()
                    + STRUT * 2.0
            })
            .fold(viewport_w, f32::max);
        let scale = minimap_scale(track_w, panel_track_h, viewport_w, viewport_h, widest);

        let mut card = div()
            .id("minimap")
            .debug_selector(|| "minimap".into())
            .absolute()
            .top(px(MINIMAP_TOP))
            .right(px(MINIMAP_RIGHT))
            .w(px(MINIMAP_SIZE))
            .h(px(MINIMAP_SIZE))
            .p(px(MINIMAP_PADDING))
            .flex()
            .flex_col()
            .gap(px(MINIMAP_ROW_GAP))
            .rounded_lg()
            .bg(Theme::MINIMAP_BG)
            .border_1()
            .border_color(Theme::PANEL_BORDER)
            .occlude()
            // Scrolling over the map pans the active strip's camera, scaled
            // back up to canvas distance so the map and canvas move 1:1.
            .on_scroll_wheel(cx.listener(
                move |this, event: &gpui::ScrollWheelEvent, window, cx| {
                    let row = this.active_row;
                    let delta = event.delta.pixel_delta(window.line_height());
                    let mut dx = f32::from(delta.x);
                    if dx == 0.0 {
                        dx = f32::from(delta.y);
                    }
                    if dx == 0.0 || scale <= f32::EPSILON {
                        return;
                    }
                    let total = this
                        .row_indices(row)
                        .map(|index| this.slot_width(index, viewport_w) + GAP)
                        .sum::<f32>()
                        + STRUT * 2.0;
                    let next = pan_camera(this.camera_x[row], -dx / scale, total, viewport_w);
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
            ));

        for row in 0..STRIP_COUNT {
            let active_row = row == self.active_row;
            let mut track = div()
                .id(("minimap-row", row))
                .debug_selector(move || format!("minimap-row-{row}"))
                .relative()
                .h(px(MINIMAP_ROW_HEIGHT))
                .rounded(px(3.0))
                .cursor_pointer()
                .bg(if active_row {
                    Theme::MINIMAP_TRACK_ACTIVE
                } else {
                    Theme::MINIMAP_TRACK
                })
                .hover(|el| el.bg(Theme::MINIMAP_TRACK_ACTIVE))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _event, window, cx| {
                        let position = this.active_position_in_row();
                        this.select_row(row, position);
                        this.focus_active(window, cx);
                        cx.notify();
                    }),
                );

            for index in self.row_indices(row) {
                let left = self.slot_left(index, viewport_w) * scale;
                let width = (self.slot_width(index, viewport_w) * scale - 1.0).max(2.0);
                // Panels keep the canvas aspect ratio: the height is the real
                // panel height under the same scale, so a panel taller than
                // wide on screen reads taller than wide here too.
                let height = (viewport_h * scale).clamp(3.0, panel_track_h);
                let top = MINIMAP_PANEL_INSET + (panel_track_h - height) / 2.0;
                let focused = index == self.active;
                let busy = self.slots[index].panel.read(cx).is_busy();
                track = track.child(
                    div()
                        .id(("minimap-panel", index))
                        .debug_selector(move || format!("minimap-panel-{index}"))
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(width))
                        .h(px(height))
                        .rounded(px(2.0))
                        .cursor_pointer()
                        .bg(if focused {
                            Theme::ACCENT
                        } else if busy {
                            Theme::MINIMAP_PANEL_BUSY
                        } else {
                            Theme::MINIMAP_PANEL
                        })
                        .hover(|el| el.bg(Theme::ACCENT))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                cx.stop_propagation();
                                this.set_active(index, cx);
                                this.overview = false;
                                this.overview_progress.set(0.0, Instant::now());
                                this.focus_active(window, cx);
                                cx.notify();
                            }),
                        ),
                );
            }

            // The lens: where the camera is looking on the active strip.
            if active_row {
                let lens_left = ((self.camera_x[row] + STRUT) * scale).max(0.0);
                let lens_width = (viewport_w * scale).min(track_w - lens_left).max(3.0);
                track = track.child(
                    div()
                        .absolute()
                        .left(px(lens_left))
                        .top(px(0.0))
                        .w(px(lens_width))
                        .h(px(MINIMAP_ROW_HEIGHT))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(Theme::MINIMAP_VIEWPORT)
                        .bg(gpui::rgba(0xffffff08)),
                );
            }

            card = card.child(track);
        }

        card.into_any_element()
    }

    /// The coach's just-in-time hint. It sits directly below the minimap so the
    /// workspace's transient navigation aids stay together in the top right.
    fn render_coach_toast(
        &self,
        hint: &learning::Hint,
        progress: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .absolute()
            .top(px(MINIMAP_TOP + MINIMAP_SIZE + COACH_TOAST_GAP))
            .right(px(MINIMAP_RIGHT))
            .w(px(COACH_TOAST_WIDTH))
            .min_w_0()
            .overflow_hidden()
            .opacity(progress)
            .child(
                div()
                    .id("coach-toast")
                    // Tagged so a render test can assert the toast actually
                    // painted, rather than only that the coach decided to teach.
                    .debug_selector(|| "coach-toast".into())
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
            .debug_selector(|| "coach-card".into())
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

impl Workspace {
    fn render_folder_picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(directory) = self.folder_picker_dir.clone() else {
            return div().into_any_element();
        };
        let entries = directory_entries(&directory).unwrap_or_default();
        let mut list = div()
            .id("folder-picker-list")
            .debug_selector(|| "folder-picker-list".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .border_t_1()
            .border_b_1()
            .border_color(Theme::PANEL_BORDER);

        if let Some(parent) = directory.parent().map(Path::to_path_buf) {
            list = list.child(
                div()
                    .id("folder-picker-parent")
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .text_color(Theme::TEXT_DIM)
                    .hover(|el| el.bg(Theme::HEADER_BG).text_color(Theme::TEXT))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.browse_to(parent.clone(), cx);
                        }),
                    )
                    .child("↰  .."),
            );
        }
        for (index, path) in entries.into_iter().enumerate() {
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            list = list.child(
                div()
                    .id(("folder-picker-entry", index))
                    .debug_selector(move || format!("folder-picker-entry-{index}").into())
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .hover(|el| el.bg(Theme::HEADER_BG))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.browse_to(path.clone(), cx);
                        }),
                    )
                    .child(format!("▸  {label}")),
            );
        }

        div()
            .id("folder-picker-overlay")
            .debug_selector(|| "folder-picker-overlay".into())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x000000cc))
            .child(
                div()
                    .w(px(680.0))
                    .h(px(560.0))
                    .max_w(relative(0.9))
                    .max_h(relative(0.85))
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .border_1()
                    .border_color(Theme::PANEL_BORDER_FOCUS)
                    .bg(Theme::PANEL_BG)
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(px(14.0)).child("open folder"))
                            .child(
                                div()
                                    .id("folder-picker-cancel")
                                    .cursor_pointer()
                                    .text_color(Theme::TEXT_DIM)
                                    .hover(|el| el.text_color(Theme::TEXT))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.close_folder_picker(cx);
                                        }),
                                    )
                                    .child("cancel"),
                            ),
                    )
                    .child(
                        div()
                            .px_4()
                            .pb_3()
                            .text_size(px(11.0))
                            .text_color(Theme::TEXT_DIM)
                            .child(directory.display().to_string()),
                    )
                    .child(list)
                    .when_some(self.folder_picker_error.clone(), |el, error| {
                        el.child(div().px_4().py_2().text_color(Theme::ERROR).child(error))
                    })
                    .child(
                        div().p_3().flex().justify_end().child(
                            div()
                                .id("folder-picker-open")
                                .debug_selector(|| "folder-picker-open".into())
                                .px_4()
                                .py_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(Theme::ACCENT)
                                .text_color(Theme::BG)
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.choose_browsed_folder(cx);
                                    }),
                                )
                                .child("open this folder"),
                        ),
                    ),
            )
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
        let row_progress = self.row_progress.sample(now);
        if !self.row_progress.is_animating() {
            self.outgoing_row = None;
        }
        if self.overview_progress.is_animating()
            || self.hints_progress.is_animating()
            || self.coach_progress.is_animating()
            || self.row_progress.is_animating()
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
        } else if self.outgoing_row.is_some() {
            let outgoing_row = self.outgoing_row.unwrap();
            let direction = if self.active_row > outgoing_row {
                1.0
            } else {
                -1.0
            };
            let outgoing_y = -direction * row_progress * viewport_h;
            let incoming_y = direction * (1.0 - row_progress) * viewport_h;
            let outgoing = self.render_row(outgoing_row, viewport_w, viewport_h, window, cx);
            let incoming = self.render_row(self.active_row, viewport_w, viewport_h, window, cx);
            div()
                .relative()
                .size_full()
                .overflow_hidden()
                .child(
                    div()
                        .debug_selector(|| "row-transition-outgoing".into())
                        .absolute()
                        .top(px(outgoing_y))
                        .left_0()
                        .size_full()
                        .child(outgoing),
                )
                .child(
                    div()
                        .debug_selector(|| "row-transition-incoming".into())
                        .absolute()
                        .top(px(incoming_y))
                        .left_0()
                        .size_full()
                        .child(incoming),
                )
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
            self.render_strip(self.active_row, viewport_w, viewport_h, window, cx)
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
            .on_action(cx.listener(Self::open_folder))
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
                    .child(self.render_workspace_bar(cx))
                    .when(!self.slots.is_empty() && overview_progress <= 0.0, |el| {
                        el.child(self.render_minimap(viewport_w, viewport_h, cx))
                    })
                    .when(overview_progress <= 0.0, |el| {
                        el.child(self.render_edge_new_session(cx))
                    }),
            )
            .when(hints_progress > 0.0, |root| {
                root.child(self.render_hints_overlay(hints_progress, cx))
            })
            .when(self.folder_picker_dir.is_some(), |root| {
                root.child(self.render_folder_picker(cx))
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

fn directory_entries(path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|path| path.file_name().map(|name| name.to_ascii_lowercase()));
    Ok(entries)
}

fn sidebar_session_title(session: &jcode_sdk::SessionInfo) -> (&'static str, String) {
    let animal = jcode_core::id::extract_session_name(&session.session_id);
    let icon = animal.map(jcode_core::id::session_icon).unwrap_or("💫");
    let title = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .or_else(|| animal.map(str::to_owned))
        .unwrap_or_else(|| session.session_id.chars().take(12).collect());
    (icon, title)
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

/// The first panel owns the viewport. Later panels use the normal column width
/// so opening one reveals the scrolling layout rather than covering it.
fn spawned_panel_width(existing_panels: usize) -> f32 {
    if existing_panels == 0 {
        1.0
    } else {
        DEFAULT_WIDTH
    }
}

/// A lone full-width panel is only full width because it had the viewport to
/// itself. When a second panel joins it, it gives up the extra space so the
/// pair sits side by side at the default width. A width the user chose
/// deliberately (anything other than full) is left alone.
fn demoted_width(width: f32, panels_after_spawn: usize) -> f32 {
    if panels_after_spawn == 2 && width >= 1.0 {
        DEFAULT_WIDTH
    } else {
        width
    }
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

/// Whether a catalog `keys` string shows the given chord to the user. The
/// catalog writes chords for humans ("super-h / super-l", "super-1 .. super-4"),
/// so a range is expanded to the concrete keys it stands for.
#[cfg(test)]
fn advertises(keys: &str, chord: &str) -> bool {
    for part in keys.split('/').map(str::trim) {
        if part == chord {
            return true;
        }
        // "super-1 .. super-4" advertises every key in that inclusive range.
        if let Some((start, end)) = part.split_once("..") {
            let (start, end) = (start.trim(), end.trim());
            if let (Some(first), Some(last), Some(wanted)) = (
                start.rsplit('-').next().and_then(|d| d.parse::<u32>().ok()),
                end.rsplit('-').next().and_then(|d| d.parse::<u32>().ok()),
                chord.rsplit('-').next().and_then(|d| d.parse::<u32>().ok()),
            ) {
                let prefix = &start[..start.len() - 1];
                if chord.starts_with(prefix) && (first..=last).contains(&wanted) {
                    return true;
                }
            }
        }
    }
    false
}

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

/// Pixels-per-canvas-pixel for the minimap: fit the widest strip (never less
/// than one viewport) into the track width, then cap the scale so a panel's
/// mapped height fits the track. One shared scale on both axes keeps every
/// rectangle at the true canvas aspect ratio, so panels taller than wide on
/// screen stay taller than wide on the map.
fn minimap_scale(
    track_width: f32,
    track_height: f32,
    viewport_w: f32,
    viewport_h: f32,
    widest_strip: f32,
) -> f32 {
    let canvas = widest_strip.max(viewport_w).max(1.0);
    (track_width / canvas).min(track_height / viewport_h.max(1.0))
}

/// Choose the panel underneath the camera's focal point after a direct pan.
/// Gaps belong to the closest adjacent panel, which avoids a dead zone and
/// makes small, high-resolution touchpad deltas behave consistently.
fn panel_at_viewport_center(
    panels: impl IntoIterator<Item = (usize, f32)>,
    camera: f32,
    viewport: f32,
) -> Option<usize> {
    let focal_x = camera + viewport / 2.0;
    let mut left = STRUT;
    let mut closest = None;
    let mut closest_distance = f32::INFINITY;

    for (index, width_fraction) in panels {
        let width = Workspace::width_for_fraction(width_fraction, viewport);
        let center = left + width / 2.0;
        let distance = (center - focal_x).abs();
        if distance < closest_distance {
            closest = Some(index);
            closest_distance = distance;
        }
        left += width + GAP;
    }
    closest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The accounts strip must actually paint: a provider with a vendored
    /// logo, and one without (which falls back to a lettermark). Seeding
    /// bypasses the CLI so the test needs no runtime or credentials.
    #[gpui::test]
    fn connected_accounts_paint_in_the_sidebar(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (_workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.set_test_accounts(vec![
                accounts::Account {
                    id: "openai".into(),
                    display_name: "OpenAI".into(),
                    status: "available".into(),
                    auth_kind: "OAuth".into(),
                    method: "OAuth".into(),
                },
                accounts::Account {
                    id: "jcode".into(),
                    display_name: "Jcode".into(),
                    status: "expired".into(),
                    auth_kind: "API key".into(),
                    method: "API key (`JCODE_API_KEY`)".into(),
                },
            ]);
            let _ = window;
            workspace
        });
        vcx.run_until_parked();

        assert!(
            accounts::logo("openai").is_some(),
            "openai must have a vendored logo"
        );
        let with_logo = vcx
            .debug_bounds("account-openai")
            .expect("the OpenAI account row should have painted");
        let with_lettermark = vcx
            .debug_bounds("account-jcode")
            .expect("the Jcode account row should have painted");
        assert!(
            with_logo.origin.y < with_lettermark.origin.y,
            "available accounts should be listed above expired ones"
        );
    }

    #[gpui::test]
    fn ctrl_o_uses_the_in_app_picker_and_creates_a_session(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(move |_window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.bridge = bridge;
            workspace
        });
        vcx.update(|window, cx| {
            window.focus(&workspace.read(cx).focus_handle.clone(), cx);
        });
        vcx.run_until_parked();

        vcx.simulate_keystrokes("ctrl-o");
        vcx.run_until_parked();
        assert!(
            !vcx.did_prompt_for_paths(),
            "directory selection must never invoke an OS path dialog"
        );
        assert!(
            vcx.debug_bounds("folder-picker-overlay").is_some(),
            "the picker should paint inside the Jcode window"
        );

        workspace.update(vcx, |workspace, cx| {
            workspace.folder_picker_dir = Some(PathBuf::from("/tmp/jcode-picker-test"));
            workspace.choose_browsed_folder(cx);
        });
        vcx.run_until_parked();

        match commands
            .try_recv()
            .expect("folder selection should create a session")
        {
            Command::CreateSession { working_dir } => {
                assert_eq!(working_dir.as_deref(), Some("/tmp/jcode-picker-test"));
            }
            _ => panic!("folder selection sent the wrong runtime command"),
        }
    }

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
    fn touchpad_camera_focuses_the_panel_nearest_the_viewport_center() {
        let viewport = 1000.0;
        let panels = || [(0, 0.5), (1, 0.5), (2, 0.5)];

        assert_eq!(panel_at_viewport_center(panels(), 0.0, viewport), Some(0));
        assert_eq!(panel_at_viewport_center(panels(), 400.0, viewport), Some(1));
        assert_eq!(panel_at_viewport_center(panels(), 900.0, viewport), Some(2));
        assert_eq!(panel_at_viewport_center([], 0.0, viewport), None);
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

    /// Every instrumented skill must actually be reachable by the keys the
    /// catalog advertises. This is the check the earlier work never had: it
    /// drives each shortcut through the real keymap and asserts the coach
    /// recognised it, so an instrumented skill that no keystroke can trigger
    /// (a wrong binding, a guard that always returns early) cannot pass.
    #[gpui::test]
    fn every_taught_shortcut_is_reachable_by_its_advertised_keys(cx: &mut gpui::TestAppContext) {
        // Each skill, with keys that should exercise it from a fresh workspace.
        // Ordering matters only in that each sequence must leave enough panels
        // to work with; the assertions are per-skill.
        let cases: &[(&str, &str)] = &[
            ("new_panel", "super-n"),
            ("focus_left_right", "super-h"),
            ("focus_first_last", "super-end"),
            ("focus_previous", "super-tab"),
            ("overview", "super-o"),
            ("move_panel", "super-shift-h"),
            ("move_panel_end", "super-shift-end"),
            ("cycle_width", "super-r"),
            ("maximize", "super-f"),
            ("width_presets", "super-2"),
            ("move_panel_strip", "super-shift-j"),
            ("focus_up_down", "super-j"),
            ("close_panel", "super-q"),
        ];

        for (skill_id, keys) in cases {
            cx.update(|cx| crate::bind_workspace_keys(cx));
            let (workspace, vcx) = cx.add_window_view(|window, cx| {
                let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
                // Enough panels that no shortcut is a no-op, and a second strip
                // populated so cross-strip navigation is meaningful.
                for name in ["one", "two", "three", "four"] {
                    workspace.push_test_panel(name, cx);
                }
                workspace.active_row = 1;
                workspace.push_test_panel("below", cx);
                workspace.active_row = 0;
                workspace.active = 1;
                let _ = window;
                workspace
            });
            vcx.update(|window, cx| {
                let handle = workspace.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
            });
            vcx.run_until_parked();

            // focus_previous needs somewhere to return to.
            if *skill_id == "focus_previous" {
                vcx.simulate_keystrokes("super-l");
                vcx.run_until_parked();
            }

            vcx.simulate_keystrokes(keys);
            vcx.run_until_parked();

            workspace.update(vcx, |workspace, _| {
                let coach = workspace.test_coach();
                let trace = coach.trace(skill_id);
                assert!(
                    trace.recalled > 0,
                    "{keys} should have registered {skill_id} as used, \
                     but the coach recorded no unaided use"
                );
                assert!(
                    coach.mastery(skill_id, learning::now()) > 0.0,
                    "{skill_id} should have nonzero mastery after {keys}"
                );
            });
        }
    }

    /// The catalog's advertised keys must be the keys that are actually bound.
    /// Earlier work only checked that catalog entries existed by name; this
    /// compares each advertised chord against the real keymap, so the coach can
    /// never teach a keystroke the app does not listen for.
    #[gpui::test]
    fn advertised_keys_match_the_real_keymap(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let workspace = Workspace::for_test(learning::Coach::new(), cx);
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();

        // Each advertised chord paired with the action it should invoke. A skill
        // that shows two chords (left/right) maps each to its own action.
        let expected: Vec<(&str, &str, Box<dyn gpui::Action>)> = vec![
            ("focus_left_right", "super-h", Box::new(FocusLeft)),
            ("focus_left_right", "super-l", Box::new(FocusRight)),
            ("focus_up_down", "super-j", Box::new(FocusDown)),
            ("focus_up_down", "super-k", Box::new(FocusUp)),
            ("focus_first_last", "super-home", Box::new(FocusFirst)),
            ("focus_first_last", "super-end", Box::new(FocusLast)),
            ("focus_previous", "super-tab", Box::new(FocusPrevious)),
            ("overview", "super-o", Box::new(ToggleOverview)),
            ("move_panel", "super-shift-h", Box::new(MovePanelLeft)),
            ("move_panel", "super-shift-l", Box::new(MovePanelRight)),
            ("move_panel_strip", "super-shift-j", Box::new(MovePanelDown)),
            ("move_panel_strip", "super-shift-k", Box::new(MovePanelUp)),
            (
                "move_panel_end",
                "super-shift-home",
                Box::new(MovePanelToFirst),
            ),
            (
                "move_panel_end",
                "super-shift-end",
                Box::new(MovePanelToLast),
            ),
            ("cycle_width", "super-r", Box::new(CycleWidth)),
            ("maximize", "super-f", Box::new(MaximizeWidth)),
            ("width_presets", "super-1", Box::new(WidthPreset1)),
            ("width_presets", "super-2", Box::new(WidthPreset2)),
            ("width_presets", "super-3", Box::new(WidthPreset3)),
            ("width_presets", "super-4", Box::new(WidthPreset4)),
            ("new_panel", "super-n", Box::new(NewPanel)),
            ("close_panel", "super-q", Box::new(ClosePanel)),
        ];

        // Every catalog skill must appear, so a new skill cannot skip this check.
        for skill in learning::SKILLS {
            assert!(
                expected.iter().any(|(id, _, _)| *id == skill.id),
                "{} is in the catalog but unchecked against the keymap",
                skill.id
            );
        }

        for (skill_id, chord, action) in &expected {
            let skill = learning::skill(skill_id).expect("catalog entry");
            // The chord must be one the catalog actually shows the user.
            assert!(
                advertises(skill.keys, chord),
                "{skill_id} shows {:?}, which does not include {chord:?}",
                skill.keys
            );
            let bound: Vec<String> = vcx.update(|window, _| {
                window
                    .bindings_for_action(action.as_ref())
                    .iter()
                    .map(|binding| {
                        binding
                            .keystrokes()
                            .iter()
                            .map(|keystroke| keystroke.unparse())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect()
            });
            assert!(
                bound.iter().any(|actual| actual == chord),
                "{skill_id} advertises {chord:?} but that action binds {bound:?}"
            );
        }
    }

    /// Clicking a panel that a keypress would have focused is the pointer slow
    /// path. This clicks the real panel element in a real rendered frame, rather
    /// than testing the classification helper in isolation.
    #[gpui::test]
    fn clicking_a_neighbouring_panel_is_recorded_as_a_missed_shortcut(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for name in ["one", "two", "three"] {
                workspace.push_test_panel(name, cx);
            }
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        cx.run_until_parked();

        // Focus starts on the first panel; the second is its neighbour.
        workspace.update(cx, |workspace, _| {
            assert_eq!(workspace.test_focus_position(), Some(0));
            assert_eq!(workspace.test_coach().effort_wasted, 0);
        });

        let neighbour = cx
            .debug_bounds("panel-1")
            .expect("the second panel should have painted");
        cx.simulate_click(neighbour.center(), gpui::Modifiers::default());
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.test_focus_position(),
                Some(1),
                "the click should have moved focus"
            );
            let coach = workspace.test_coach();
            assert!(
                coach.effort_wasted > 0,
                "clicking a neighbour should count as work done the long way"
            );
            assert_eq!(
                coach.active_hint_id(),
                Some("focus_left_right"),
                "and should teach the navigation keys"
            );
        });
    }

    /// Clicking the sidebar's "+" opens a session, but must never be mistaken
    /// for knowing super-n. Without this check, routing the button through the
    /// keyboard handler would silently teach the coach that the user is fluent
    /// in a shortcut they have never pressed.
    #[gpui::test]
    fn clicking_the_new_session_button_is_not_keyboard_credit(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("one", cx);
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();

        let button = vcx
            .debug_bounds("sidebar-new-session")
            .expect("the new-session button should have painted");
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        workspace.update(vcx, |workspace, _| {
            let coach = workspace.test_coach();
            let trace = coach.trace("new_panel");
            assert_eq!(
                trace.recalled, 0,
                "clicking the button must not count as recalling super-n"
            );
            assert!(
                trace.slow_paths > 0,
                "clicking the button should be recorded as the long way round"
            );
            assert!(coach.effort_wasted > 0, "and should count as wasted effort");
        });
    }

    #[gpui::test]
    fn right_edge_is_a_full_height_click_target_for_a_new_session(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("one", cx);
            workspace
        });
        vcx.run_until_parked();

        let edge = vcx
            .debug_bounds("edge-new-session")
            .expect("the right-edge new-session target should paint");
        assert_eq!(f32::from(edge.size.width), 32.0);
        assert!(f32::from(edge.size.height) > 100.0);
        vcx.simulate_click(edge.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        workspace.update(vcx, |workspace, _| {
            assert!(
                workspace.test_coach().trace("new_panel").slow_paths > 0,
                "clicking the edge target should invoke the pointer spawn path"
            );
        });
    }

    /// Clicking the panel that is already focused is not a missed shortcut: no
    /// keypress would have done anything, so it must not be held against them.
    #[gpui::test]
    fn clicking_the_focused_panel_is_not_a_missed_shortcut(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for name in ["one", "two"] {
                workspace.push_test_panel(name, cx);
            }
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        cx.run_until_parked();

        let focused = cx
            .debug_bounds("panel-0")
            .expect("the focused panel should have painted");
        cx.simulate_click(focused.center(), gpui::Modifiers::default());
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            let coach = workspace.test_coach();
            assert_eq!(
                coach.effort_wasted, 0,
                "clicking into the panel you are already in is not a slow path"
            );
            assert_eq!(coach.active_hint_id(), None, "and should not be lectured");
        });
    }

    /// The toast must actually paint. Earlier live-screenshot attempts always
    /// caught the window after the hint had expired, so this drives the real
    /// render pipeline and asserts the element was laid out on screen.
    #[gpui::test]
    fn the_hint_toast_actually_paints_when_the_coach_teaches(cx: &mut gpui::TestAppContext) {
        let now = learning::now();
        let mut coach = learning::Coach::new();
        for step in 0..8 {
            coach.used_shortcut("focus_left_right", now - (8 - step) * 3 * 86_400);
        }

        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(coach, cx);
            for name in ["one", "two", "three", "four", "five"] {
                workspace.push_test_panel(name, cx);
            }
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        cx.run_until_parked();

        // Nothing is being taught yet, so no toast should be on screen.
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| gpui::div(),
        );
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("coach-toast").is_none(),
            "no toast before anything is taught"
        );

        // Grind along the strip, which is what someone without the jump key does.
        cx.simulate_keystrokes("super-end super-h super-h super-h");
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.test_coach().active_hint_id(),
                Some("focus_first_last"),
                "grinding should have produced a hint to render"
            );
        });

        cx.run_until_parked();
        let bounds = cx
            .debug_bounds("coach-toast")
            .expect("the hint toast should have painted");
        let minimap = cx
            .debug_bounds("minimap")
            .expect("the minimap should have painted above the hint");
        assert!(
            bounds.size.width > px(0.) && bounds.size.height > px(0.),
            "the toast must occupy real space, got {bounds:?}"
        );
        assert!(
            bounds.origin.y >= minimap.origin.y + minimap.size.height,
            "the toast should sit below the minimap: toast={bounds:?}, minimap={minimap:?}"
        );
        assert_eq!(
            bounds.origin.x + bounds.size.width,
            minimap.origin.x + minimap.size.width,
            "the toast and minimap should share their right edge"
        );
        assert!(
            bounds.origin.x >= px(SIDEBAR_WIDTH),
            "the toast should remain inside the workspace instead of spilling into the sidebar"
        );
    }

    /// The coach view must paint too, and only while it is open.
    #[gpui::test]
    fn the_coach_view_paints_only_when_opened(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("only", cx);
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("coach-card").is_none(),
            "the coach view starts closed"
        );

        cx.simulate_keystrokes("super-/");
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("coach-card").is_some(),
            "super-/ should open the coach view"
        );

        // Closing animates out over MODAL_DURATION, so the card is still painted
        // mid-fade; it must be gone once the transition has finished. The
        // animations are driven by the wall clock rather than the test clock, so
        // this waits out the real duration.
        cx.simulate_keystrokes("super-/");
        cx.run_until_parked();
        std::thread::sleep(transition::policy(Transition::Hints).duration * 2);
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| gpui::div(),
        );
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("coach-card").is_none(),
            "super-/ should close it again"
        );
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

    /// This is the public acceptance path for vertical navigation: real bound
    /// keys must reveal a directional transition and returning to either strip
    /// must restore that strip's own last-focused panel.
    #[gpui::test]
    fn vertical_keys_animate_and_restore_each_strips_focus(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("top-left", cx);
            workspace.push_test_panel("top-right", cx);
            workspace.active_row = 1;
            workspace.push_test_panel("bottom-left", cx);
            workspace.push_test_panel("bottom-right", cx);
            workspace.set_active(1, cx);
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();

        // Moving down initially preserves column position. The incoming strip
        // starts below the outgoing strip, proving the visible direction.
        vcx.simulate_keystrokes("super-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.active_row, 1);
            assert_eq!(workspace.test_focus_position(), Some(1));
            assert!(workspace.row_progress.is_animating());
        });
        let outgoing = vcx
            .debug_bounds("row-transition-outgoing")
            .expect("the old strip should remain visible during the transition");
        let incoming = vcx
            .debug_bounds("row-transition-incoming")
            .expect("the new strip should be visible during the transition");
        assert!(
            incoming.origin.y > outgoing.origin.y,
            "moving down should bring the new strip in from below"
        );

        // Give the lower strip a distinct remembered position, then round-trip.
        vcx.simulate_keystrokes("super-h super-k");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.active_row, 0);
            assert_eq!(workspace.test_focus_position(), Some(1));
        });
        vcx.simulate_keystrokes("super-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.active_row, 1);
            assert_eq!(
                workspace.test_focus_position(),
                Some(0),
                "returning should restore the lower strip's last-focused panel"
            );
        });

        // The temporary outgoing layer must be retired after the policy duration.
        std::thread::sleep(transition::policy(Transition::Row).duration * 2);
        workspace.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.outgoing_row, None);
            assert!(!workspace.row_progress.is_animating());
        });
        assert!(
            vcx.debug_bounds("panel-2").is_some(),
            "the remembered lower panel should remain painted after settling"
        );
    }

    #[gpui::test]
    fn vertical_keys_cover_both_animation_directions(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("top-left", cx);
            workspace.push_test_panel("top-right", cx);
            workspace.active_row = 1;
            workspace.push_test_panel("bottom-left", cx);
            workspace.push_test_panel("bottom-right", cx);
            workspace.set_active(1, cx);
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            window.focus(&workspace.read(cx).focus_handle.clone(), cx);
        });
        vcx.run_until_parked();

        vcx.simulate_keystrokes("super-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!((workspace.active_row, workspace.test_focus_position()), (1, Some(1)));
            assert!(workspace.row_progress.is_animating());
        });
        let outgoing = vcx.debug_bounds("row-transition-outgoing").unwrap();
        let incoming = vcx.debug_bounds("row-transition-incoming").unwrap();
        assert!(incoming.origin.y > outgoing.origin.y, "down enters from below");

        vcx.simulate_keystrokes("super-h");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("super-k");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!((workspace.active_row, workspace.test_focus_position()), (0, Some(1)));
        });
        let outgoing = vcx.debug_bounds("row-transition-outgoing").unwrap();
        let incoming = vcx.debug_bounds("row-transition-incoming").unwrap();
        assert!(incoming.origin.y < outgoing.origin.y, "up enters from above");

        vcx.simulate_keystrokes("super-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!((workspace.active_row, workspace.test_focus_position()), (1, Some(0)));
        });

        std::thread::sleep(transition::policy(Transition::Row).duration * 2);
        workspace.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.outgoing_row, None);
            assert!(!workspace.row_progress.is_animating());
        });
    }

    #[gpui::test]
    fn moving_a_panel_between_strips_animates_and_stops_at_boundaries(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("moving", cx);
            workspace.active_row = 1;
            workspace.push_test_panel("already-below", cx);
            workspace.set_active(0, cx);
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            window.focus(&workspace.read(cx).focus_handle.clone(), cx);
        });
        vcx.run_until_parked();

        vcx.simulate_keystrokes("super-shift-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.active_row, 1);
            assert_eq!(workspace.slots[workspace.active].row, 1);
            assert!(workspace.row_progress.is_animating());
        });
        let outgoing = vcx.debug_bounds("row-transition-outgoing").unwrap();
        let incoming = vcx.debug_bounds("row-transition-incoming").unwrap();
        assert!(incoming.origin.y > outgoing.origin.y);

        // Repeated moves stop at the fourth strip without corrupting focus or
        // starting a transition for the impossible fifth move.
        vcx.simulate_keystrokes("super-shift-j super-shift-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| assert_eq!(workspace.active_row, 3));
        std::thread::sleep(transition::policy(Transition::Row).duration * 2);
        workspace.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        vcx.simulate_keystrokes("super-shift-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.active_row, 3);
            assert_eq!(workspace.outgoing_row, None);
            assert!(!workspace.row_progress.is_animating());
        });
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
    fn catalog_chord_parsing_handles_pairs_and_ranges() {
        // The keymap check is only as good as its reading of the catalog's
        // human-facing key strings, so pin that reading down.
        assert!(advertises("super-h / super-l", "super-h"));
        assert!(advertises("super-h / super-l", "super-l"));
        assert!(!advertises("super-h / super-l", "super-j"));
        assert!(advertises("super-tab", "super-tab"));
        // Ranges stand for every key they span, and nothing outside it.
        assert!(advertises("super-1 .. super-4", "super-1"));
        assert!(advertises("super-1 .. super-4", "super-4"));
        assert!(!advertises("super-1 .. super-4", "super-5"));
        assert!(!advertises("super-1 .. super-4", "alt-2"));
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

    #[test]
    fn first_spawn_fills_the_viewport_and_later_spawns_use_the_default_width() {
        assert_eq!(spawned_panel_width(0), 1.0);
        assert_eq!(spawned_panel_width(1), DEFAULT_WIDTH);
        assert_eq!(spawned_panel_width(4), DEFAULT_WIDTH);
    }

    #[test]
    fn the_lone_full_width_panel_halves_when_a_second_one_spawns() {
        // Spawning the second panel leaves two equal halves: the newcomer opens
        // at the default width and the incumbent gives up the extra space.
        assert_eq!(spawned_panel_width(1), DEFAULT_WIDTH);
        assert_eq!(demoted_width(1.0, 2), DEFAULT_WIDTH);
        // Every later spawn leaves the existing panels untouched, so the strip
        // keeps scrolling at the default width.
        assert_eq!(demoted_width(DEFAULT_WIDTH, 3), DEFAULT_WIDTH);
        assert_eq!(demoted_width(1.0, 3), 1.0);
        // A width the user picked is never overridden.
        assert_eq!(demoted_width(0.25, 2), 0.25);
        assert_eq!(demoted_width(0.75, 2), 0.75);
    }

    #[test]
    fn the_minimap_scale_fits_the_widest_strip() {
        // A tall track never constrains these cases, so the width rule alone
        // decides the scale. A canvas narrower than the viewport still maps
        // the full viewport, so the lens can never overflow the track.
        assert_eq!(
            minimap_scale(160.0, 1000.0, 1000.0, 800.0, 500.0),
            160.0 / 1000.0
        );
        // A wide canvas is compressed to fit the track exactly.
        assert_eq!(
            minimap_scale(160.0, 1000.0, 1000.0, 800.0, 4000.0),
            160.0 / 4000.0
        );
        // Degenerate inputs never divide by zero.
        assert!(minimap_scale(160.0, 1.0, 0.0, 0.0, 0.0).is_finite());
    }

    #[test]
    fn the_minimap_keeps_the_canvas_aspect_ratio() {
        // On a common landscape canvas the height cap wins, and a half-width
        // panel maps taller than wide, matching how it looks on screen.
        let scale = minimap_scale(160.0, 32.0, 1656.0, 1000.0, 1656.0);
        assert_eq!(scale, 32.0 / 1000.0);
        let mapped_w = 1656.0 * 0.5 * scale;
        let mapped_h = 1000.0 * scale;
        assert!(
            mapped_w < mapped_h,
            "a half-width panel should read taller than wide ({mapped_w} vs {mapped_h})"
        );
        // A full-width panel is wider than tall on screen, and stays that way.
        assert!(1656.0 * scale > mapped_h);
    }

    /// The minimap must actually paint in the top right and jumping through it
    /// must work: this clicks the real minimap rectangle for the third panel in
    /// a real rendered frame and asserts focus moved there.
    #[gpui::test]
    fn clicking_a_minimap_panel_jumps_focus_there(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for name in ["one", "two", "three"] {
                workspace.push_test_panel(name, cx);
            }
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        cx.run_until_parked();

        let map = cx
            .debug_bounds("minimap")
            .expect("the minimap should paint");
        let window_width = cx.update(|window, _| f32::from(window.viewport_size().width));
        assert!(
            f32::from(map.right()) <= window_width + 1.0
                && f32::from(map.origin.x) > window_width / 2.0,
            "the minimap should sit in the top right"
        );
        assert!(
            f32::from(map.origin.y) < 40.0,
            "the minimap should hug the top edge"
        );

        let target = cx
            .debug_bounds("minimap-panel-2")
            .expect("the third panel should appear on the map");
        cx.simulate_click(target.center(), gpui::Modifiers::default());
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.test_focus_position(),
                Some(2),
                "clicking the minimap rectangle should jump focus to that panel"
            );
        });
    }

    /// Clicking an empty minimap track switches to that strip, mirroring the
    /// workspace bar.
    #[gpui::test]
    fn clicking_a_minimap_track_switches_strips(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("one", cx);
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        cx.run_until_parked();

        let track = cx
            .debug_bounds("minimap-row-2")
            .expect("every strip should have a track on the map");
        cx.simulate_click(track.center(), gpui::Modifiers::default());
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.active_row, 2,
                "clicking the third track should select strip 3"
            );
        });
    }
}
