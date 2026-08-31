//! Workspace: four niri-inspired horizontal strips of session panels with a
//! smooth camera and a zoomed-out overview.
//!
//! Panels live on one of four infinite horizontal strips. Focus moves
//! left/right within a strip and up/down between strips.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, App, Context, Entity, FocusHandle, Focusable, ScrollHandle, Window,
    actions, div, prelude::*, px, relative,
};
use jcode_desktop_api::HostHandle;
use serde::{Deserialize, Serialize};

use crate::accounts;
use crate::harness::{self, Bridge, Command, Update};
use crate::input::{PromptInput, PromptInputSnapshot};
use crate::learning;
use crate::panel::{Panel, PanelSnapshot};
use crate::performance::{
    ActionCapture, GpuiSnapshot as GpuiPerformanceSnapshot, Health as PerformanceHealth,
    Profile as PerformanceProfile,
};
use crate::theme::Theme;
use crate::transition::{self, AnimatedValue, Transition};
use crate::updates;

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
        ForkPanel,
        NewTerminal,
        OpenGmail,
        OpenTodoist,
        NewUnfinishedWork,
        OpenFolder,
        ClosePanel,
        ToggleOverview,
        ToggleHints,
        ToggleShowcase,
        ToggleSidebar,
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

/// Spatial transitions settle in 150 ms. Cubic easing keeps motion visible
/// across the available frames instead of concentrating it at the start.
const CAMERA_DURATION: Duration = transition::STANDARD_DURATION;
/// A tiny amount of presentation smoothing removes the one-frame stepping
/// caused by touchpad events arriving between compositor frames without making
/// the canvas feel detached from the fingers.
const TOUCH_PAN_DURATION: Duration = Duration::from_millis(42);
/// niri `layout { gaps 0 }`: columns sit flush against each other.
const GAP: f32 = 0.0;
/// niri `layout { struts { ... 0.58 } }`, the outer gap around the strip.
const STRUT: f32 = 0.58;
const STRIP_PADDING_Y: f32 = STRUT;
const STRIP_COUNT: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShowcaseCue {
    shortcut: String,
    action: &'static str,
    tutorial_group: &'static str,
}

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
- Super+Shift+S: toggle showcase mode for on-screen workspace motions
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
/// Height of the macOS titlebar the window draws through. The window uses a
/// transparent system titlebar, so the app's own chrome has to leave this much
/// room at the top or it renders underneath the traffic lights.
const TITLEBAR_HEIGHT: f32 = 52.0;
/// Horizontal space occupied by the macOS close, minimize, and zoom controls.
/// Keep sidebar navigation out of this region while retaining Linux's current
/// left alignment.
const MACOS_TRAFFIC_LIGHTS_WIDTH: f32 = 76.0;

fn content_top_inset(show_sidebar: bool, fullscreen: bool) -> f32 {
    // Fullscreen macOS windows have no titlebar and the traffic lights are
    // hidden (they only float in on hover, above our content), so reserving
    // room for them would leave a dead strip across the top.
    if cfg!(target_os = "macos") && !show_sidebar && !fullscreen {
        TITLEBAR_HEIGHT
    } else {
        0.0
    }
}

fn sidebar_header_left_padding(fullscreen: bool) -> f32 {
    // Same reasoning as `content_top_inset`: in fullscreen the traffic lights
    // are not in the header row, so keep the normal navigation alignment.
    if cfg!(target_os = "macos") && !fullscreen {
        MACOS_TRAFFIC_LIGHTS_WIDTH
    } else {
        12.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SidebarView {
    #[default]
    Sessions,
    Files,
}

// Minimap: a compact card in the top right that maps every strip to
// scale, preserving the canvas aspect ratio so panels taller than wide on
// screen stay taller than wide on the map.
const MINIMAP_WIDTH: f32 = 112.0;
const MINIMAP_HEIGHT: f32 = 96.0;
const MINIMAP_PADDING: f32 = 5.0;
const MINIMAP_ROW_GAP: f32 = 3.0;
const MINIMAP_TOP: f32 = 8.0;
const MINIMAP_RIGHT: f32 = 12.0;
/// The update chip sits above the workspace bar in the bottom-right corner,
/// out of the reading path but always in view.
const UPDATE_CHIP_BOTTOM: f32 = 44.0;
const COACH_TOAST_GAP: f32 = 8.0;
const COACH_TOAST_WIDTH: f32 = 288.0;
/// The coach keeps hints for nine seconds. Wake once after that deadline instead
/// of rebuilding every transcript at display refresh rate for the full lifetime.
const COACH_EXPIRY_WAKE: Duration = Duration::from_secs(10);
const SHOWCASE_DURATION: Duration = Duration::from_millis(1800);
/// The small, hands-on curriculum shown to a new user. Once every item has
/// been practiced, onboarding gets out of the way permanently because the
/// learning model is persisted across launches.
const ONBOARDING_SKILLS: &[&str] = &[
    "focus_left_right",
    "focus_up_down",
    "new_panel",
    "close_panel",
    "move_panel",
    "move_panel_strip",
    "width_presets",
    "cycle_width",
    "overview",
];
/// Rows split the square's inner height evenly, one per strip.
const MINIMAP_ROW_HEIGHT: f32 =
    (MINIMAP_HEIGHT - MINIMAP_PADDING * 2.0 - MINIMAP_ROW_GAP * (STRIP_COUNT as f32 - 1.0))
        / STRIP_COUNT as f32;
/// Vertical inset between a panel rectangle and its track edge.
const MINIMAP_PANEL_INSET: f32 = 1.5;
/// The gesture reticle: how long it stays fully lit after the last touchpad
/// scroll delta, and how long the fade-out takes once the fingers lift.
const GESTURE_HOLD: Duration = Duration::from_millis(150);
const GESTURE_FADE: Duration = Duration::from_millis(300);
const GESTURE_RETICLE_SIZE: f32 = 44.0;
const MINIMAP_GESTURE_DOT: f32 = 7.0;
/// Travel before a touchpad gesture locks to its dominant axis. Below this,
/// horizontal deltas pan and vertical deltas scroll, like before.
const AXIS_LOCK: f32 = 12.0;
/// Vertical travel, after a horizontal lock, that breaks the sticky axis and
/// hops to the neighbouring strip. Resets per hop so a long drag steps
/// through strips one threshold at a time.
const STRIP_BREAK: f32 = 130.0;
/// A pause this long between deltas ends the gesture, since not every
/// platform reliably delivers an Ended touch phase.
const GESTURE_RESET: Duration = Duration::from_millis(250);
/// Fraction of the vertical pull the reticle follows while the sticky axis
/// resists, so the rubber band is visible before it snaps.
const PULL_RESISTANCE: f32 = 0.35;

/// Which axis a touchpad gesture has committed to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum GestureAxis {
    #[default]
    Undecided,
    Horizontal,
    Vertical,
}

/// The sticky axis lock for two-finger strip gestures. A gesture that starts
/// horizontally owns the strip: it pans, and once enough vertical travel
/// accumulates it breaks the stickiness and hops strips. A gesture that
/// starts vertically belongs to whatever is under the pointer and never pans.
#[derive(Clone, Copy, Debug, Default)]
struct StripGesture {
    axis: GestureAxis,
    travel_x: f32,
    travel_y: f32,
    /// Vertical pull accumulated while horizontally locked. Positive pulls
    /// toward the strip below (natural: fingers up reveal what is beneath).
    pull: f32,
}

/// Where one scroll delta goes, decided by the sticky axis lock.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Routed {
    /// The event belongs to whatever is under the pointer (a transcript).
    ToPanel,
    /// The strip takes it: pan by `dx`, hop `switch` strips, and when
    /// `exclusive` the event must not also reach the transcript.
    Strip {
        dx: f32,
        switch: i8,
        exclusive: bool,
    },
}

impl StripGesture {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advance the state machine by one precise (touchpad) delta.
    fn feed(&mut self, dx: f32, dy: f32, phase: gpui::TouchPhase) -> Routed {
        use gpui::TouchPhase::{Cancelled, Ended, Started};
        if matches!(phase, Started | Cancelled) {
            self.reset();
        }
        let routed = match self.axis {
            GestureAxis::Undecided => {
                self.travel_x += dx.abs();
                self.travel_y += dy.abs();
                if self.travel_x >= AXIS_LOCK || self.travel_y >= AXIS_LOCK {
                    self.axis = if self.travel_x >= self.travel_y {
                        GestureAxis::Horizontal
                    } else {
                        GestureAxis::Vertical
                    };
                }
                match self.axis {
                    GestureAxis::Vertical => Routed::ToPanel,
                    // Pan horizontal motion immediately, but keep the event
                    // shared until the lock resolves so the first few small
                    // vertical pixels still reach the transcript.
                    axis => Routed::Strip {
                        dx,
                        switch: 0,
                        exclusive: axis == GestureAxis::Horizontal,
                    },
                }
            }
            GestureAxis::Vertical => Routed::ToPanel,
            GestureAxis::Horizontal => {
                self.pull += -dy;
                let switch = if self.pull.abs() >= STRIP_BREAK {
                    let direction = if self.pull > 0.0 { 1 } else { -1 };
                    self.pull = 0.0;
                    direction
                } else {
                    0
                };
                Routed::Strip {
                    dx,
                    switch,
                    exclusive: true,
                }
            }
        };
        if phase == Ended {
            self.reset();
        }
        routed
    }
}

struct Slot {
    panel: Entity<Panel>,
    row: usize,
    /// Width as a fraction of the viewport (0.25, 0.5, 0.75, 1.0).
    width_fraction: f32,
    /// Rendered width, retargeted when the configured width changes.
    animated_width: AnimatedValue,
    /// Signed progress from the panel's former horizontal position to its new
    /// one. The distance uses the swapped neighbour's width at render time.
    order_offset: AnimatedValue,
    order_distance_fraction: f32,
    /// Visibility during dismissal. Closed panels remain mounted until this
    /// reaches zero so their exit can actually be painted.
    close_progress: AnimatedValue,
    closing: bool,
    /// Width to restore when un-maximizing (niri `maximize-column` toggle).
    restore_fraction: Option<f32>,
}

const SNAPSHOT_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct SlotSnapshot {
    panel: PanelSnapshot,
    row: usize,
    width_fraction: f32,
    restore_fraction: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
enum FocusSnapshot {
    Panel(usize),
    FolderSearch,
    #[default]
    Workspace,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkspaceSnapshot {
    format_version: u32,
    slots: Vec<SlotSnapshot>,
    active: usize,
    active_row: usize,
    row_focus: [Option<usize>; STRIP_COUNT],
    previous: Option<usize>,
    camera_x: [f32; STRIP_COUNT],
    camera_target: [f32; STRIP_COUNT],
    overview: bool,
    hints_overlay: bool,
    folder_picker_dir: Option<PathBuf>,
    folder_picker_error: Option<String>,
    folder_search: Option<PromptInputSnapshot>,
    focus: FocusSnapshot,
}

impl WorkspaceSnapshot {
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(Into::into)
    }

    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let snapshot: Self = serde_json::from_slice(bytes)?;
        anyhow::ensure!(
            snapshot.format_version == SNAPSHOT_FORMAT_VERSION,
            "unsupported workspace snapshot format {}",
            snapshot.format_version
        );
        anyhow::ensure!(
            snapshot.active_row < STRIP_COUNT,
            "active row is out of range"
        );
        anyhow::ensure!(
            snapshot.slots.iter().all(|slot| {
                slot.row < STRIP_COUNT
                    && slot.width_fraction.is_finite()
                    && (0.1..=1.0).contains(&slot.width_fraction)
                    && slot
                        .restore_fraction
                        .is_none_or(|width| width.is_finite() && (0.1..=1.0).contains(&width))
            }),
            "snapshot contains an invalid panel layout"
        );
        anyhow::ensure!(
            snapshot.slots.is_empty() || snapshot.active < snapshot.slots.len(),
            "active panel is out of range"
        );
        Ok(snapshot)
    }
}

pub struct Workspace {
    bridge: Bridge,
    host: HostHandle,
    show_sidebar: bool,
    sidebar_view: SidebarView,
    expanded_directories: HashSet<PathBuf>,
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
    camera_touch_pan: [bool; STRIP_COUNT],
    /// Set when a strip's camera target must be recomputed at render time,
    /// once the viewport width is known.
    camera_dirty: [bool; STRIP_COUNT],
    overview: bool,
    overview_progress: AnimatedValue,
    hints_overlay: bool,
    hints_progress: AnimatedValue,
    /// Presenter-friendly mode that briefly explains workspace actions on screen.
    showcase_mode: bool,
    showcase_cue: Option<ShowcaseCue>,
    showcase_task: Option<gpui::Task<()>>,
    /// Models which shortcuts the user knows, and teaches the ones they don't.
    coach: learning::Coach,
    learning_persistence: Option<learning::Persistence>,
    /// Fade for the coach's hint toast.
    coach_progress: AnimatedValue,
    coach_expiry_task: Option<gpui::Task<()>>,
    pending_help_session: bool,
    /// Every non-archived session offered by the runtime, oldest to newest.
    sessions: Vec<jcode_sdk::SessionInfo>,
    /// Configured logins and API keys, refreshed in the background.
    accounts: Vec<accounts::Account>,
    status: String,
    connected: bool,
    focus_handle: FocusHandle,
    sidebar_scroll: ScrollHandle,
    /// Focus the active panel's input on the next render (set when panels
    /// appear from background updates, where no Window is available).
    focus_pending: bool,
    /// When the last touchpad pan delta arrived. Drives the gesture reticle
    /// that marks where focus will land while a swipe is in flight.
    gesture_last: Option<Instant>,
    /// The sticky axis lock for the gesture currently on the strip.
    gesture: StripGesture,
    /// When the last precise delta of any axis arrived, so a stale gesture
    /// can be ended by silence when no Ended phase is delivered.
    gesture_seen: Option<Instant>,
    /// Directory currently shown by the in-app folder browser. `None` closes it.
    folder_picker_dir: Option<PathBuf>,
    folder_picker_error: Option<String>,
    folder_search: Option<Entity<PromptInput>>,
    focus_restore: FocusSnapshot,
    performance: Option<PerformanceProfile>,
    gpui_performance: GpuiPerformanceSnapshot,
    action_capture: Option<ActionCapture>,
    animation_tick_task: Option<gpui::Task<()>>,
    _bridge_task: gpui::Task<()>,
    _housekeeping_task: gpui::Task<()>,
    _performance_task: gpui::Task<()>,
}

impl Workspace {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        host: HostHandle,
        snapshot: Option<WorkspaceSnapshot>,
    ) -> Self {
        crate::input::bind_keys(cx);
        let bridge = harness::spawn();
        let accounts_feed = accounts::spawn();
        let performance_enabled = crate::performance::enabled(std::env::args_os());

        // Wake immediately when a bridge update arrives rather than polling an
        // empty channel at the display refresh rate.
        let update_bridge = bridge.clone();
        let bridge_task = cx.spawn(async move |this, cx| {
            while let Some(first) = update_bridge.recv().await {
                let mut updates = vec![first];
                updates.extend(update_bridge.drain());
                let outcome = this.update(cx, |workspace: &mut Workspace, cx| {
                    let mut changed = false;
                    for update in updates {
                        changed |= workspace.apply(update, cx);
                    }
                    if changed {
                        cx.notify();
                    }
                });
                if outcome.is_err() {
                    break;
                }
            }
        });

        // Account snapshots and cross-process session reconciliation are both
        // slow-changing data and only need a low-frequency housekeeping wake.
        let housekeeping_bridge = bridge.clone();
        let session_refresh_interval = crate::config::get().session_refresh_interval();
        let housekeeping_task = cx.spawn(async move |this, cx| {
            let mut last_session_refresh = Instant::now();
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let accounts = accounts_feed.latest();
                if last_session_refresh.elapsed() >= session_refresh_interval {
                    housekeeping_bridge.send(Command::RefreshSessions);
                    last_session_refresh = Instant::now();
                }
                let Some(accounts) = accounts else {
                    continue;
                };
                let outcome = this.update(cx, |workspace: &mut Workspace, cx| {
                    if workspace.accounts != accounts {
                        workspace.accounts = accounts;
                        cx.notify();
                    }
                });
                if outcome.is_err() {
                    break;
                }
            }
        });

        let performance_task = cx.spawn(async move |this, cx| {
            if !performance_enabled {
                return;
            }
            const SAMPLE_INTERVAL: Duration = Duration::from_millis(50);
            const DISPLAY_INTERVAL: Duration = Duration::from_millis(250);
            let mut last_wake = Instant::now();
            let mut last_display = Instant::now();
            loop {
                cx.background_executor().timer(SAMPLE_INTERVAL).await;
                let now = Instant::now();
                let wake_lag = now
                    .duration_since(last_wake)
                    .saturating_sub(SAMPLE_INTERVAL);
                last_wake = now;
                let refresh_display = last_display.elapsed() >= DISPLAY_INTERVAL;
                if refresh_display {
                    last_display = now;
                }
                let outcome = this.update(cx, |workspace: &mut Workspace, cx| {
                    if let Some(profile) = workspace.performance.as_mut() {
                        profile.observe_wake_lag(wake_lag);
                        if refresh_display {
                            cx.notify();
                        }
                    }
                });
                if outcome.is_err() {
                    break;
                }
            }
        });

        let _ = window;
        let mut workspace = Self {
            bridge,
            host,
            show_sidebar: sidebar_enabled(
                std::env::args_os(),
                crate::config::get().workspace.sidebar,
            ),
            sidebar_view: SidebarView::Sessions,
            expanded_directories: HashSet::new(),
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
            camera_touch_pan: [false; STRIP_COUNT],
            camera_dirty: [true; STRIP_COUNT],
            overview: false,
            overview_progress: AnimatedValue::new(
                0.0,
                transition::policy(Transition::Overview).duration,
            ),
            hints_overlay: false,
            hints_progress: AnimatedValue::new(0.0, transition::policy(Transition::Hints).duration),
            showcase_mode: crate::config::get().workspace.showcase_keys,
            showcase_cue: None,
            showcase_task: None,
            coach: learning::load(),
            learning_persistence: Some(learning::Persistence::spawn()),
            coach_progress: AnimatedValue::new(0.0, transition::policy(Transition::Coach).duration),
            coach_expiry_task: None,
            pending_help_session: false,
            sessions: Vec::new(),
            accounts: Vec::new(),
            status: "starting...".into(),
            connected: false,
            focus_handle: cx.focus_handle(),
            sidebar_scroll: ScrollHandle::new(),
            focus_pending: false,
            gesture_last: None,
            gesture: StripGesture::default(),
            gesture_seen: None,
            folder_picker_dir: None,
            folder_picker_error: None,
            folder_search: None,
            focus_restore: FocusSnapshot::Workspace,
            performance: performance_enabled.then(PerformanceProfile::default),
            gpui_performance: GpuiPerformanceSnapshot::default(),
            action_capture: ActionCapture::from_env(),
            animation_tick_task: None,
            _bridge_task: bridge_task,
            _housekeeping_task: housekeeping_task,
            _performance_task: performance_task,
        };
        if let Some(snapshot) = snapshot {
            workspace.apply_snapshot(snapshot, cx);
        }
        workspace
    }

    /// A workspace with no runtime and a caller-supplied coach, for tests that
    /// drive real keystrokes through the real keymap.
    #[cfg(test)]
    pub fn for_test(coach: learning::Coach, cx: &mut Context<Self>) -> Self {
        crate::input::bind_keys(cx);
        Self {
            bridge: harness::spawn_inert(),
            host: HostHandle::inert(),
            show_sidebar: true,
            sidebar_view: SidebarView::Sessions,
            expanded_directories: HashSet::new(),
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
            camera_touch_pan: [false; STRIP_COUNT],
            camera_dirty: [true; STRIP_COUNT],
            overview: false,
            overview_progress: AnimatedValue::new(
                0.0,
                transition::policy(Transition::Overview).duration,
            ),
            hints_overlay: false,
            hints_progress: AnimatedValue::new(0.0, transition::policy(Transition::Hints).duration),
            showcase_mode: true,
            showcase_cue: None,
            showcase_task: None,
            coach,
            learning_persistence: None,
            coach_progress: AnimatedValue::new(0.0, transition::policy(Transition::Coach).duration),
            coach_expiry_task: None,
            pending_help_session: false,
            sessions: Vec::new(),
            accounts: Vec::new(),
            status: "test".into(),
            connected: true,
            focus_handle: cx.focus_handle(),
            sidebar_scroll: ScrollHandle::new(),
            focus_pending: false,
            gesture_last: None,
            gesture: StripGesture::default(),
            gesture_seen: None,
            folder_picker_dir: None,
            folder_picker_error: None,
            folder_search: None,
            focus_restore: FocusSnapshot::Workspace,
            performance: None,
            gpui_performance: GpuiPerformanceSnapshot::default(),
            action_capture: None,
            animation_tick_task: None,
            _bridge_task: cx.spawn(async move |_, _| {}),
            _housekeeping_task: cx.spawn(async move |_, _| {}),
            _performance_task: cx.spawn(async move |_, _| {}),
        }
    }

    pub fn snapshot(&self, window: &Window, cx: &App) -> anyhow::Result<WorkspaceSnapshot> {
        let index_for_id = |id: gpui::EntityId| {
            self.slots
                .iter()
                .position(|slot| slot.panel.entity_id() == id)
        };
        let focus = if self
            .folder_search
            .as_ref()
            .is_some_and(|search| search.read(cx).focus_handle.is_focused(window))
        {
            FocusSnapshot::FolderSearch
        } else if let Some(index) = self.slots.iter().position(|slot| {
            slot.panel
                .read(cx)
                .input_focus_handle(cx)
                .is_focused(window)
        }) {
            FocusSnapshot::Panel(index)
        } else {
            FocusSnapshot::Workspace
        };
        let folder_search = self
            .folder_search
            .as_ref()
            .map(|search| search.read(cx).snapshot());
        Ok(WorkspaceSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            slots: self
                .slots
                .iter()
                .map(|slot| SlotSnapshot {
                    panel: slot.panel.read(cx).snapshot(cx),
                    row: slot.row,
                    width_fraction: slot.width_fraction,
                    restore_fraction: slot.restore_fraction,
                })
                .collect(),
            active: self.active,
            active_row: self.active_row,
            row_focus: self.row_focus.map(|id| id.and_then(index_for_id)),
            previous: self.previous.and_then(index_for_id),
            camera_x: self.camera_x,
            camera_target: self.camera_target,
            overview: self.overview,
            hints_overlay: self.hints_overlay,
            folder_picker_dir: self.folder_picker_dir.clone(),
            folder_picker_error: self.folder_picker_error.clone(),
            folder_search,
            focus,
        })
    }

    fn apply_snapshot(&mut self, snapshot: WorkspaceSnapshot, cx: &mut Context<Self>) {
        self.slots.clear();
        for saved in snapshot.slots {
            let panel_state = saved.panel;
            let terminal =
                panel_state.terminal_resource_id.is_some() || panel_state.session_id == "terminal";
            let panel = if panel_state.session_id == "unfinished-work" {
                let workspace = cx.weak_entity();
                cx.new(|cx| {
                    Panel::new_unfinished_work(
                        harness::unfinished_sessions(&self.sessions),
                        std::sync::Arc::new(move |session, window, cx| {
                            workspace
                                .update(cx, |workspace, cx| {
                                    workspace.activate_unfinished_session(session, window, cx);
                                })
                                .expect("unfinished-work workspace is still available");
                        }),
                        self.bridge.clone(),
                        cx,
                    )
                })
            } else if terminal {
                let working_dir = panel_state.working_dir.clone();
                let resource_id = panel_state.terminal_resource_id;
                cx.new(|cx| {
                    Panel::new_terminal(
                        working_dir,
                        self.bridge.clone(),
                        self.host,
                        resource_id,
                        cx,
                    )
                })
            } else if let Some(path) = panel_state.session_id.strip_prefix("file://") {
                let path = PathBuf::from(path);
                cx.new(|cx| Panel::new_code_file(path, self.bridge.clone(), cx))
            } else if panel_state.session_id == "gmail://inbox" {
                cx.new(|cx| Panel::new_gmail(self.bridge.clone(), cx))
            } else if panel_state.session_id == "todoist://tasks" {
                cx.new(|cx| Panel::new_todoist(self.bridge.clone(), cx))
            } else {
                let session_id = panel_state.session_id.clone();
                let title = Some(panel_state.title.clone());
                let working_dir = panel_state.working_dir.clone();
                let panel = cx.new(|cx| {
                    Panel::new(
                        session_id.clone(),
                        title,
                        working_dir,
                        self.bridge.clone(),
                        cx,
                    )
                });
                self.bridge.send(Command::Watch { session_id });
                panel
            };
            Panel::connect_input(&panel, cx);
            panel.update(cx, |panel, cx| panel.restore_snapshot(panel_state, cx));
            self.slots.push(Slot {
                panel,
                row: saved.row,
                width_fraction: saved.width_fraction,
                animated_width: AnimatedValue::new(
                    saved.width_fraction,
                    transition::policy(Transition::PanelWidth).duration,
                ),
                order_offset: AnimatedValue::new(
                    0.0,
                    transition::policy(Transition::PanelOrder).duration,
                ),
                order_distance_fraction: saved.width_fraction,
                close_progress: AnimatedValue::new(
                    1.0,
                    transition::policy(Transition::PanelClose).duration,
                ),
                closing: false,
                restore_fraction: saved.restore_fraction,
            });
        }
        self.active = snapshot.active.min(self.slots.len().saturating_sub(1));
        self.active_row = snapshot.active_row;
        self.row_focus = snapshot.row_focus.map(|index| {
            index.and_then(|index| self.slots.get(index).map(|slot| slot.panel.entity_id()))
        });
        self.previous = snapshot
            .previous
            .and_then(|index| self.slots.get(index).map(|slot| slot.panel.entity_id()));
        self.camera_x = snapshot.camera_x;
        self.camera_target = snapshot.camera_target;
        self.camera_from = snapshot.camera_x;
        self.camera_started = [None; STRIP_COUNT];
        self.camera_touch_pan = [false; STRIP_COUNT];
        self.camera_dirty = [false; STRIP_COUNT];
        self.overview = snapshot.overview;
        self.overview_progress = AnimatedValue::new(
            if snapshot.overview { 1.0 } else { 0.0 },
            transition::policy(Transition::Overview).duration,
        );
        self.hints_overlay = snapshot.hints_overlay;
        self.hints_progress = AnimatedValue::new(
            if snapshot.hints_overlay { 1.0 } else { 0.0 },
            transition::policy(Transition::Hints).duration,
        );
        self.folder_picker_dir = snapshot.folder_picker_dir;
        self.folder_picker_error = snapshot.folder_picker_error;
        self.focus_restore = snapshot.focus;
        if let Some(search_state) = snapshot.folder_search {
            let search = self.create_folder_search(cx);
            search.update(cx, |search, cx| search.restore(search_state, cx));
            self.folder_search = Some(search);
        }
    }

    fn create_folder_search(&self, cx: &mut Context<Self>) -> Entity<PromptInput> {
        let weak = cx.weak_entity();
        let change_weak = weak.clone();
        cx.new(|cx| {
            PromptInput::new(
                cx,
                "type a folder name or path, then press enter",
                move |query, _, _, app| {
                    let _ = weak.update(app, |this, cx| this.open_searched_folder(&query, cx));
                },
            )
            .with_on_change(move |_, app| {
                let _ = change_weak.update(app, |_, cx| cx.notify());
            })
        })
    }

    pub fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.focus_restore.clone() {
            FocusSnapshot::Panel(index) => {
                if let Some(slot) = self.slots.get(index) {
                    let handle = slot.panel.read(cx).input_focus_handle(cx);
                    window.focus(&handle, cx);
                } else {
                    window.focus(&self.focus_handle, cx);
                }
            }
            FocusSnapshot::FolderSearch => {
                if let Some(search) = &self.folder_search {
                    let handle = search.read(cx).focus_handle.clone();
                    window.focus(&handle, cx);
                } else {
                    self.focus_active(window, cx);
                }
            }
            FocusSnapshot::Workspace => self.focus_active(window, cx),
        }
    }

    /// Add a bare panel for tests, bypassing the runtime.
    #[cfg(test)]
    pub fn push_test_panel(&mut self, name: &str, cx: &mut Context<Self>) {
        let bridge = self.bridge.clone();
        let panel =
            cx.new(|cx| Panel::new(name.to_string(), Some(name.to_string()), None, bridge, cx));
        Panel::connect_input(&panel, cx);
        self.slots.push(Slot {
            panel,
            row: self.active_row,
            width_fraction: DEFAULT_WIDTH,
            animated_width: AnimatedValue::new(
                DEFAULT_WIDTH,
                transition::policy(Transition::PanelWidth).duration,
            ),
            order_offset: AnimatedValue::new(
                0.0,
                transition::policy(Transition::PanelOrder).duration,
            ),
            order_distance_fraction: DEFAULT_WIDTH,
            close_progress: AnimatedValue::new(
                1.0,
                transition::policy(Transition::PanelClose).duration,
            ),
            closing: false,
            restore_fraction: None,
        });
    }

    #[cfg(test)]
    pub fn set_test_bridge(&mut self, bridge: Bridge) {
        self.bridge = bridge;
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

    #[cfg(test)]
    pub fn test_coach_mut(&mut self) -> &mut learning::Coach {
        &mut self.coach
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

    /// Apply a bridge update and report whether it changed visible workspace state.
    ///
    /// Session refreshes arrive every two seconds. Most contain the exact same
    /// SDK snapshot, so treating them as changes would rebuild the complete
    /// sidebar and every open panel for no user-visible result.
    fn apply(&mut self, mut update: Update, cx: &mut Context<Self>) -> bool {
        if let Update::Sessions { sessions } = &mut update {
            // A desktop window can disappear while its daemon-owned sessions keep
            // running. Those panels are restored from the workspace snapshot before
            // the asynchronous session catalog arrives. Older runtimes and bounded
            // catalogs can omit them, so never let a refresh erase an open session
            // from the sidebar.
            for slot in &self.slots {
                let panel = slot.panel.read(cx);
                if !panel.session_id.starts_with("session_")
                    || sessions
                        .iter()
                        .any(|session| session.session_id == panel.session_id)
                {
                    continue;
                }
                sessions.push(jcode_sdk::SessionInfo {
                    session_id: panel.session_id.clone(),
                    working_dir: panel.working_dir.clone(),
                    title: Some(panel.title.to_string()),
                    status: "active".into(),
                    transcript_bytes: None,
                    saved: false,
                    updated_at_ms: None,
                    last_active_at_ms: None,
                    archived: false,
                    archived_at_ms: None,
                });
            }
        }
        if let Update::Sessions { sessions } = &update
            && sessions == &self.sessions
        {
            return false;
        }

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
                for session in &sessions {
                    let Some(title) = session.title.as_ref() else {
                        continue;
                    };
                    for slot in &self.slots {
                        let (same_session, same_title) = {
                            let panel = slot.panel.read(cx);
                            (
                                panel.session_id == session.session_id,
                                panel.title.as_ref() == title,
                            )
                        };
                        if same_session {
                            if same_title {
                                break;
                            }
                            slot.panel.update(cx, |panel, cx| {
                                panel.title = title.clone().into();
                                cx.notify();
                            });
                            break;
                        }
                    }
                }
                self.sessions = sessions;
                let unfinished = harness::unfinished_sessions(&self.sessions);
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == "unfinished-work" {
                        slot.panel.update(cx, |panel, cx| {
                            panel.set_unfinished_work(unfinished.clone(), cx)
                        });
                    }
                }
            }
            Update::SessionCreated { session } => {
                let session_id = session.session_id.clone();
                // A brand-new panel is only a local draft until its first prompt.
                // The persisted/runtime session refresh adds it after activity.
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
            Update::SessionForked { session } => {
                let session_id = session.session_id.clone();
                if !self
                    .sessions
                    .iter()
                    .any(|known| known.session_id == session_id)
                {
                    self.sessions.push(session.clone());
                }
                let inserted = self.open_session(session, cx);
                self.set_active(inserted, cx);
                self.focus_pending = true;
            }
            Update::History {
                session_id,
                messages,
                images,
            } => {
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == session_id {
                        slot.panel.update(cx, |panel, cx| {
                            panel.load_history(messages, images, cx);
                        });
                        break;
                    }
                }
            }
            Update::Event { session_id, event } => {
                if let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|session| session.session_id == session_id)
                {
                    // Live SDK events arrive before the next persisted metadata
                    // refresh. Advance recency immediately so active work moves
                    // exactly as it does in the TUI picker.
                    session.updated_at_ms = Some(unix_now_ms());
                }
                let updated_title = match &event {
                    jcode_sdk::ApiEvent::SessionRenamed { display_title, .. } => {
                        Some(display_title.clone())
                    }
                    jcode_sdk::ApiEvent::ToolDone { name, error, .. }
                        if name == "todo" && error.is_none() =>
                    {
                        // Todo-derived titles are SDK session metadata rather
                        // than explicit RenameSession events. Refresh the list
                        // as soon as the SDK reports a successful todo write.
                        self.bridge.send(Command::RefreshSessions);
                        None
                    }
                    _ => None,
                };
                if let Some(display_title) = updated_title
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
            Update::MessageSubmitted { .. } => {}
            Update::CommandFailed { session_id, reason } => {
                for slot in &self.slots {
                    if slot.panel.read(cx).session_id == session_id {
                        slot.panel.update(cx, |panel, cx| {
                            panel.items.push(crate::panel::Item::Error(reason.clone()));
                            cx.notify();
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
        true
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
            order_offset: AnimatedValue::new(
                0.0,
                transition::policy(Transition::PanelOrder).duration,
            ),
            order_distance_fraction: width_fraction,
            close_progress: AnimatedValue::new(
                1.0,
                transition::policy(Transition::PanelClose).duration,
            ),
            closing: false,
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

    fn activate_unfinished_session(
        &mut self,
        unfinished: harness::UnfinishedSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self
            .sessions
            .iter()
            .find(|session| session.session_id == unfinished.session_id)
            .cloned()
            .unwrap_or_else(|| jcode_sdk::SessionInfo {
                session_id: unfinished.session_id,
                working_dir: unfinished.working_dir,
                title: Some(unfinished.title),
                status: "idle".into(),
                transcript_bytes: None,
                saved: false,
                updated_at_ms: None,
                last_active_at_ms: None,
                archived: false,
                archived_at_ms: None,
            });
        // This can run from a click listener on the active unfinished-work
        // panel. Do not read that same entity while GPUI is updating it.
        let index = self
            .slots
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.active)
            .find(|(_, slot)| slot.panel.read(cx).session_id == session.session_id)
            .map(|(index, _)| index)
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

    /// Onboarding is complete after the user has successfully exercised every
    /// control it presents. `practiced` intentionally accepts prompted use: the
    /// full coach can continue building recall later without keeping the
    /// onboarding chrome on screen.
    fn onboarding_complete(&self) -> bool {
        ONBOARDING_SKILLS
            .iter()
            .all(|skill| self.coach.trace(skill).practiced())
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
        let visible =
            crate::config::get().workspace.coaching_hints && self.coach.active_hint(now).is_some();
        self.coach_progress
            .set(if visible { 1.0 } else { 0.0 }, Instant::now());
        if visible {
            self.coach_expiry_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(COACH_EXPIRY_WAKE).await;
                let _ = this.update(cx, |workspace, cx| workspace.after_coach_update(cx));
            }));
        } else {
            self.coach_expiry_task = None;
        }
        if self.coach.take_dirty() {
            if let Some(persistence) = &self.learning_persistence {
                persistence.save(&self.coach);
            }
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
        self.showcase_motion("H", false, "Focus left", cx);
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position > 0
        {
            self.set_active(indices[position - 1], cx);
            self.begin_action_capture("focus_left", window);
            self.focus_active(window, cx);
            // Credit only when the key did something: pressing into the edge of
            // a strip is a no-op and proves nothing either way.
            self.learned("focus_left_right", cx);
            cx.notify();
        }
    }

    fn focus_right(&mut self, _: &FocusRight, window: &mut Window, cx: &mut Context<Self>) {
        self.showcase_motion("L", false, "Focus right", cx);
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position + 1 < indices.len()
        {
            self.set_active(indices[position + 1], cx);
            self.begin_action_capture("focus_right", window);
            self.focus_active(window, cx);
            self.learned("focus_left_right", cx);
            cx.notify();
        }
    }

    /// niri `focus-column-first`.
    fn focus_first(&mut self, _: &FocusFirst, window: &mut Window, cx: &mut Context<Self>) {
        self.showcase_motion("Home", false, "Focus first panel", cx);
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
        self.showcase_motion("End", false, "Focus last panel", cx);
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
        self.showcase_motion("Tab", false, "Return to previous panel", cx);
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
        self.showcase_motion("K", false, "Focus strip above", cx);
        if self.active_row > 0 {
            if self.change_row(self.active_row - 1, window, cx) {
                self.begin_action_capture("focus_up", window);
            }
        } else {
            // Pressing into the top edge cannot navigate, but the chord was
            // still produced by hand, so the tutorial lesson must not linger.
            self.coach.used_shortcut_without_effect("focus_up_down");
            self.after_coach_update(cx);
        }
    }

    fn focus_down(&mut self, _: &FocusDown, window: &mut Window, cx: &mut Context<Self>) {
        self.showcase_motion("J", false, "Focus strip below", cx);
        if self.active_row + 1 < STRIP_COUNT {
            if self.change_row(self.active_row + 1, window, cx) {
                self.begin_action_capture("focus_down", window);
            }
        } else {
            self.coach.used_shortcut_without_effect("focus_up_down");
            self.after_coach_update(cx);
        }
    }

    /// Move focus to another strip, crediting the shortcut only when the move
    /// was meaningful. Stepping onto an empty strip when there is nothing else
    /// open shows the user pressed a key, not that they navigated anywhere, so
    /// it must not be taken as evidence of skill. It is still hands-on
    /// practice, though: the tutorial asked for the chord and the user
    /// produced it, so the lesson must clear rather than linger forever on a
    /// workspace whose other strips happen to be empty.
    fn change_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let meaningful = self.switch_row_animated(row, window, cx);
        if meaningful {
            self.learned("focus_up_down", cx);
        } else {
            self.coach.used_shortcut_without_effect("focus_up_down");
            self.after_coach_update(cx);
        }
        cx.notify();
        meaningful
    }

    fn begin_action_capture(&mut self, name: &'static str, window: &Window) {
        if let Some(capture) = self.action_capture.as_mut() {
            capture.begin(
                name,
                Instant::now(),
                window.frame_duration_snapshot(),
                window.input_latency_snapshot(),
            );
        }
    }

    /// Switch strips with the row transition animation. Returns whether the
    /// move actually landed on a different panel, so callers can decide if it
    /// counts as navigation.
    fn switch_row_animated(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
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
        landed_somewhere && was_active != is_active
    }

    fn move_panel_left(&mut self, _: &MovePanelLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.showcase_motion("H", true, "Move panel left", cx);
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position > 0
        {
            let previous = indices[position - 1];
            let moved_width = self.slots[self.active].width_fraction;
            let neighbour_width = self.slots[previous].width_fraction;
            self.slots.swap(self.active, previous);
            self.active = previous;
            self.start_order_animation(self.active, 1.0, neighbour_width);
            self.start_order_animation(indices[position], -1.0, moved_width);
            self.retarget_camera();
            self.learned("move_panel", cx);
            cx.notify();
        }
    }

    fn move_panel_right(&mut self, _: &MovePanelRight, _: &mut Window, cx: &mut Context<Self>) {
        self.showcase_motion("L", true, "Move panel right", cx);
        let indices: Vec<_> = self.row_indices(self.active_row).collect();
        if let Some(position) = indices.iter().position(|&index| index == self.active)
            && position + 1 < indices.len()
        {
            let next = indices[position + 1];
            let moved_width = self.slots[self.active].width_fraction;
            let neighbour_width = self.slots[next].width_fraction;
            self.slots.swap(self.active, next);
            self.active = next;
            self.start_order_animation(self.active, -1.0, neighbour_width);
            self.start_order_animation(indices[position], 1.0, moved_width);
            self.retarget_camera();
            self.learned("move_panel", cx);
            cx.notify();
        }
    }

    fn start_order_animation(&mut self, index: usize, offset: f32, distance_fraction: f32) {
        let duration = transition::policy(Transition::PanelOrder).duration;
        self.slots[index].order_offset = AnimatedValue::new(offset, duration);
        self.slots[index].order_offset.set(0.0, Instant::now());
        self.slots[index].order_distance_fraction = distance_fraction;
    }

    fn move_panel_up(&mut self, _: &MovePanelUp, window: &mut Window, cx: &mut Context<Self>) {
        self.showcase_motion("K", true, "Move panel up", cx);
        self.move_panel_to_row(-1, window, cx);
    }

    /// niri `move-column-to-first`.
    fn move_panel_to_first(
        &mut self,
        _: &MovePanelToFirst,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.showcase_motion("Home", true, "Move panel to start", cx);
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
        self.showcase_motion("End", true, "Move panel to end", cx);
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
        self.showcase_motion("J", true, "Move panel down", cx);
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
        self.tutorial_cue("N", "New session", "new", cx);
        self.learned("new_panel", cx);
        self.open_new_session(cx);
    }

    fn fork_panel(&mut self, _: &ForkPanel, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self.slots.get(self.active).and_then(|slot| {
            let panel = slot.panel.read(cx);
            panel.can_fork().then(|| panel.session_id.clone())
        }) else {
            return;
        };
        self.bridge.send(Command::Fork { session_id });
    }

    fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let width_fraction = spawned_panel_width(self.slots.len());
        let panel = cx.new(|cx| {
            Panel::new_terminal(
                default_working_dir(),
                self.bridge.clone(),
                self.host,
                None,
                cx,
            )
        });
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
                order_offset: AnimatedValue::new(
                    0.0,
                    transition::policy(Transition::PanelOrder).duration,
                ),
                order_distance_fraction: width_fraction,
                close_progress: AnimatedValue::new(
                    1.0,
                    transition::policy(Transition::PanelClose).duration,
                ),
                closing: false,
                restore_fraction: None,
            },
        );
        self.set_active(insert_at, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn open_gmail(&mut self, _: &OpenGmail, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.panel.read(cx).session_id == "gmail://inbox")
        {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            return;
        }
        let width_fraction = spawned_panel_width(self.slots.len());
        let panel = cx.new(|cx| Panel::new_gmail(self.bridge.clone(), cx));
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
                order_offset: AnimatedValue::new(
                    0.0,
                    transition::policy(Transition::PanelOrder).duration,
                ),
                order_distance_fraction: width_fraction,
                close_progress: AnimatedValue::new(
                    1.0,
                    transition::policy(Transition::PanelClose).duration,
                ),
                closing: false,
                restore_fraction: None,
            },
        );
        self.set_active(insert_at, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn open_code_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let session_id = format!("file://{}", path.display());
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.panel.read(cx).session_id == session_id)
        {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            return;
        }

        let width_fraction = spawned_panel_width(self.slots.len());
        let panel = cx.new(|cx| Panel::new_code_file(path, self.bridge.clone(), cx));
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
                order_offset: AnimatedValue::new(
                    0.0,
                    transition::policy(Transition::PanelOrder).duration,
                ),
                order_distance_fraction: width_fraction,
                close_progress: AnimatedValue::new(
                    1.0,
                    transition::policy(Transition::PanelClose).duration,
                ),
                closing: false,
                restore_fraction: None,
            },
        );
        self.set_active(insert_at, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn new_unfinished_work(
        &mut self,
        _: &NewUnfinishedWork,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.panel.read(cx).session_id == "unfinished-work")
        {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            return;
        }
        let width_fraction = spawned_panel_width(self.slots.len());
        let workspace = cx.weak_entity();
        let panel = cx.new(|cx| {
            Panel::new_unfinished_work(
                harness::unfinished_sessions(&self.sessions),
                std::sync::Arc::new(move |session, window, cx| {
                    workspace
                        .update(cx, |workspace, cx| {
                            workspace.activate_unfinished_session(session, window, cx);
                        })
                        .expect("unfinished-work workspace is still available");
                }),
                self.bridge.clone(),
                cx,
            )
        });
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
                order_offset: AnimatedValue::new(
                    0.0,
                    transition::policy(Transition::PanelOrder).duration,
                ),
                order_distance_fraction: width_fraction,
                close_progress: AnimatedValue::new(
                    1.0,
                    transition::policy(Transition::PanelClose).duration,
                ),
                closing: false,
                restore_fraction: None,
            },
        );
        self.set_active(insert_at, cx);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn open_todoist(&mut self, _: &OpenTodoist, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.panel.read(cx).session_id == "todoist://tasks")
        {
            self.set_active(index, cx);
            self.focus_active(window, cx);
            return;
        }
        let width_fraction = spawned_panel_width(self.slots.len());
        let panel = cx.new(|cx| Panel::new_todoist(self.bridge.clone(), cx));
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
                order_offset: AnimatedValue::new(
                    0.0,
                    transition::policy(Transition::PanelOpen).duration,
                ),
                order_distance_fraction: width_fraction,
                close_progress: AnimatedValue::new(
                    1.0,
                    transition::policy(Transition::PanelClose).duration,
                ),
                closing: false,
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
    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        self.folder_picker_dir = Some(
            default_working_dir()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
        );
        self.folder_picker_error = None;
        let weak = cx.weak_entity();
        let change_weak = weak.clone();
        let search = cx.new(|cx| {
            PromptInput::new(
                cx,
                "type a folder name or path, then press enter",
                move |query, _, _, app| {
                    let _ = weak.update(app, |this, cx| this.open_searched_folder(&query, cx));
                },
            )
            .with_on_change(move |_, app| {
                let _ = change_weak.update(app, |_, cx| cx.notify());
            })
        });
        self.folder_search = Some(search);
        cx.defer_in(window, |this, window, cx| {
            if let Some(search) = &this.folder_search {
                let focus_handle = search.read(cx).focus_handle.clone();
                window.focus(&focus_handle, cx);
            }
        });
        cx.notify();
    }

    fn open_searched_folder(&mut self, query: &str, cx: &mut Context<Self>) {
        let Some(base) = self.folder_picker_dir.as_deref() else {
            return;
        };
        let expanded = if query == "~" {
            default_working_dir().map(PathBuf::from)
        } else if let Some(rest) = query.strip_prefix("~/") {
            default_working_dir().map(|home| PathBuf::from(home).join(rest))
        } else {
            let path = PathBuf::from(query);
            Some(if path.is_absolute() {
                path
            } else {
                base.join(path)
            })
        };
        let matched = expanded.filter(|path| path.is_dir()).or_else(|| {
            let needle = query.to_lowercase();
            let direct = base.join(query);
            if direct.is_file() {
                return direct.parent().map(Path::to_path_buf);
            }
            ranked_folder_matches_for_sessions(&self.sessions, base, &needle)
                .into_iter()
                .next()
                .map(|(path, _)| path)
        });
        if let Some(path) = matched {
            self.folder_picker_dir = Some(path);
            self.choose_browsed_folder(cx);
        } else {
            self.folder_picker_error = Some(format!("no folder matches ‘{query}’"));
            cx.notify();
        }
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
        self.folder_search = None;
        self.bridge.send(Command::CreateSession {
            working_dir: Some(path.to_string_lossy().into_owned()),
        });
        cx.notify();
    }

    fn close_folder_picker(&mut self, cx: &mut Context<Self>) {
        self.folder_picker_dir = None;
        self.folder_picker_error = None;
        self.folder_search = None;
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
            .is_none_or(|slot| slot.row != self.active_row || slot.closing)
        {
            return;
        }
        self.learned("close_panel", cx);
        let closed = self.active;
        let closed_id = self.slots[closed].panel.entity_id();
        self.slots[closed].closing = true;
        self.slots[closed].close_progress.set(0.0, Instant::now());
        let session_id = self.slots[closed].panel.read(cx).session_id.clone();
        if session_id != "terminal" {
            self.bridge.send(Command::Unwatch { session_id });
        }
        if self.previous == Some(closed_id) {
            self.previous = None;
        }
        let remaining: Vec<_> = self
            .row_indices(self.active_row)
            .filter(|&index| index != closed)
            .collect();
        self.active = focus_after_close(closed, &remaining);
        self.retarget_camera();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn remove_finished_closing_panels(
        &mut self,
        now: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_id = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.entity_id());
        let mut finished = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let progress = slot.close_progress.sample(now);
            if slot.closing && !slot.close_progress.is_animating() && progress <= f32::EPSILON {
                finished.push(index);
            }
        }
        if finished.is_empty() {
            return;
        }
        for index in finished.into_iter().rev() {
            self.slots.remove(index);
        }
        self.active = active_id
            .and_then(|id| {
                self.slots
                    .iter()
                    .position(|slot| slot.panel.entity_id() == id)
            })
            .unwrap_or_else(|| self.slots.len().saturating_sub(1));
        if self.slots.is_empty() {
            window.focus(&self.focus_handle, cx);
        }
        self.retarget_camera();
    }

    fn toggle_overview(&mut self, _: &ToggleOverview, _: &mut Window, cx: &mut Context<Self>) {
        self.tutorial_cue("O", "Toggle overview", "overview", cx);
        self.learned("overview", cx);
        self.overview = !self.overview;
        self.overview_progress
            .set(if self.overview { 1.0 } else { 0.0 }, Instant::now());
        self.hints_overlay = false;
        self.hints_progress.set(0.0, Instant::now());
        cx.notify();
    }

    fn toggle_hints(&mut self, _: &ToggleHints, _: &mut Window, cx: &mut Context<Self>) {
        self.tutorial_cue("/", "Show all shortcuts", "help", cx);
        self.hints_overlay = !self.hints_overlay;
        self.hints_progress
            .set(if self.hints_overlay { 1.0 } else { 0.0 }, Instant::now());
        cx.notify();
    }

    fn toggle_showcase(&mut self, _: &ToggleShowcase, _: &mut Window, cx: &mut Context<Self>) {
        self.showcase_mode = !self.showcase_mode;
        self.showcase_cue = self.showcase_mode.then(|| ShowcaseCue {
            shortcut: if cfg!(target_os = "macos") {
                "Cmd + Shift + S".to_owned()
            } else {
                "Super + Shift + S".to_owned()
            },
            action: "Showcase mode on",
            tutorial_group: "",
        });
        self.schedule_showcase_expiry(cx);
        cx.notify();
    }

    fn showcase_motion(
        &mut self,
        key: &str,
        shifted: bool,
        action: &'static str,
        cx: &mut Context<Self>,
    ) {
        if !self.showcase_mode {
            return;
        }
        let modifier = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Super"
        };
        self.showcase_cue = Some(ShowcaseCue {
            shortcut: if shifted {
                format!("{modifier} + Shift + {key}")
            } else {
                format!("{modifier} + {key}")
            },
            action,
            tutorial_group: if shifted { "move" } else { "navigate" },
        });
        self.schedule_showcase_expiry(cx);
        cx.notify();
    }

    fn tutorial_cue(
        &mut self,
        key: &str,
        action: &'static str,
        tutorial_group: &'static str,
        cx: &mut Context<Self>,
    ) {
        if !self.showcase_mode {
            return;
        }
        let modifier = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Super"
        };
        self.showcase_cue = Some(ShowcaseCue {
            shortcut: format!("{modifier} + {key}"),
            action,
            tutorial_group,
        });
        self.schedule_showcase_expiry(cx);
        cx.notify();
    }

    fn schedule_showcase_expiry(&mut self, cx: &mut Context<Self>) {
        if self.showcase_cue.is_none() {
            self.showcase_task = None;
            return;
        }
        self.showcase_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOWCASE_DURATION).await;
            let _ = this.update(cx, |workspace, cx| {
                workspace.showcase_cue = None;
                workspace.showcase_task = None;
                cx.notify();
            });
        }));
    }

    /// The sidebar is a persistent chrome column, so hiding it is a view
    /// preference rather than a workspace mutation: panels keep their slots and
    /// only the horizontal budget in `render` changes.
    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar = !self.show_sidebar;
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
        self.tutorial_cue("R", "Cycle panel width", "resize", cx);
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
        self.tutorial_cue("F", "Maximize or restore panel", "resize", cx);
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
            let handle = panel.read(cx).input_focus_handle(cx);
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
                .text_color(Theme::global().TEXT_DIM)
                // An empty strip has no transcripts to protect, so any
                // vertical touchpad travel past the break threshold hops
                // strips directly. Without this, a gesture that lands on an
                // empty strip is stranded there until a keyboard shortcut.
                .on_scroll_wheel(cx.listener(
                    move |this, event: &gpui::ScrollWheelEvent, window, cx| {
                        if this.active_row != row || !event.delta.precise() {
                            return;
                        }
                        let delta = event.delta.pixel_delta(window.line_height());
                        let dy = f32::from(delta.y);
                        if dy == 0.0 {
                            return;
                        }
                        let now = Instant::now();
                        if this
                            .gesture_seen
                            .is_none_or(|seen| seen.elapsed() > GESTURE_RESET)
                        {
                            this.gesture.reset();
                        }
                        this.gesture_seen = Some(now);
                        this.gesture.pull += -dy;
                        if this.gesture.pull.abs() >= STRIP_BREAK {
                            let target = if this.gesture.pull > 0.0 {
                                (this.active_row + 1).min(STRIP_COUNT - 1)
                            } else {
                                this.active_row.saturating_sub(1)
                            };
                            this.gesture.pull = 0.0;
                            if target != this.active_row {
                                this.switch_row_animated(target, window, cx);
                            }
                        }
                        cx.notify();
                    },
                ))
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
        let performance = self.performance.as_ref().map(|profile| {
            let snapshot = profile.snapshot();
            let gpui = self.gpui_performance;
            format!(
                "perf animation_fps={:.1} frame_p95_ms={:.1} missed={} view_p95_ms={:.1} draw_p95_ms={:.1} present_p95_ms={:.1} input_to_frame_p95_ms={:.1} coalesced={} mid_draw={}",
                snapshot.animation_fps,
                snapshot.animation_frame_p95_ms,
                snapshot.missed_animation_frames,
                snapshot.render_p95_ms,
                gpui.draw_p95_ms,
                gpui.present_p95_ms,
                gpui.input_to_frame_p95_ms,
                gpui.input_events_per_frame_p95,
                gpui.mid_draw_inputs,
            )
        });
        let suffix = performance
            .map(|line| format!("\n{line}"))
            .unwrap_or_default();
        let _ = std::fs::write(path, format!("{line}\n{coach}{suffix}\n"));
    }

    fn animation_active(&self) -> bool {
        self.row_progress.is_animating()
            || self.overview_progress.is_animating()
            || self.hints_progress.is_animating()
            || self.coach_progress.is_animating()
            || self.camera_started.iter().any(Option::is_some)
            || self
                .slots
                .iter()
                .any(|slot| slot.animated_width.is_animating() || slot.order_offset.is_animating())
    }

    /// GPUI's next-frame callback is presentation-driven. Some Wayland
    /// compositors can delay that callback despite accepting 60 Hz presents,
    /// which leaves a lightweight workspace transition visibly stepping at
    /// 20–30 Hz. A single coalesced timer wake keeps animation state advancing;
    /// the compositor still decides when the resulting frame is presented.
    fn ensure_animation_tick(&mut self, cx: &mut Context<Self>) {
        if self.animation_tick_task.is_some() {
            return;
        }
        self.animation_tick_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(8))
                .await;
            let _ = this.update(cx, |workspace, cx| {
                workspace.animation_tick_task = None;
                if workspace.animation_active() {
                    cx.notify();
                }
            });
        }));
    }

    fn render_strip(
        &mut self,
        row: usize,
        viewport_w: f32,
        viewport_h: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = Instant::now();
        self.remove_finished_closing_panels(now, window, cx);
        if row == self.active_row && self.camera_dirty[row] {
            self.resolve_camera_target(viewport_w);
        }
        // Touchpad pans use a much shorter interpolation than navigation. This
        // coalesces irregular input delivery into presentation frames while
        // remaining close enough to the fingers to feel direct.
        let camera_duration = if self.camera_touch_pan[row] {
            TOUCH_PAN_DURATION
        } else {
            CAMERA_DURATION
        };
        match self.camera_started[row] {
            Some(started) => {
                let elapsed = started.elapsed();
                if elapsed >= camera_duration {
                    self.camera_x[row] = self.camera_target[row];
                    self.camera_started[row] = None;
                    self.camera_touch_pan[row] = false;
                } else {
                    self.camera_x[row] = smoothed_camera_position(
                        self.camera_from[row],
                        self.camera_target[row],
                        elapsed,
                        camera_duration,
                    );
                    window.request_animation_frame();
                }
            }
            None => self.camera_x[row] = self.camera_target[row],
        }

        let panel_h = viewport_h - STRIP_PADDING_Y * 2.0;
        let indices = self.row_indices(row).collect::<Vec<_>>();
        let mut animated_widths = Vec::with_capacity(indices.len());
        let mut order_offsets = Vec::with_capacity(indices.len());
        let mut close_progresses = Vec::with_capacity(indices.len());
        for &index in &indices {
            let fraction = self.slots[index].animated_width.sample(now);
            if self.slots[index].animated_width.is_animating() {
                window.request_animation_frame();
            }
            let close_progress = self.slots[index].close_progress.sample(now);
            if self.slots[index].close_progress.is_animating() {
                window.request_animation_frame();
            }
            close_progresses.push(close_progress);
            animated_widths.push(Self::width_for_fraction(fraction, viewport_w) * close_progress);
            let progress = self.slots[index].order_offset.sample(now);
            if self.slots[index].order_offset.is_animating() {
                window.request_animation_frame();
            }
            let distance =
                Self::width_for_fraction(self.slots[index].order_distance_fraction, viewport_w)
                    + GAP;
            order_offsets.push(progress * distance);
        }
        let mut strip = div()
            .absolute()
            .top(px(STRIP_PADDING_Y))
            .left(px(-self.camera_x[row]))
            .flex()
            .flex_row()
            .gap(px(GAP));

        for (((index, width), order_offset), close_progress) in indices
            .into_iter()
            .zip(animated_widths)
            .zip(order_offsets)
            .zip(close_progresses)
        {
            let slot = &self.slots[index];
            let focused = index == self.active;
            strip = strip.child(
                div()
                    .id(("panel", index))
                    .relative()
                    .left(px(order_offset))
                    .opacity(close_progress)
                    // Tagged so a render test can click the real panel element
                    // and exercise the pointer slow-path detection.
                    .debug_selector(move || format!("panel-{index}"))
                    .w(px(width))
                    .h(px(panel_h))
                    .flex_none()
                    .bg(Theme::global().PANEL_BG)
                    .border_1()
                    .border_color(if focused {
                        Theme::global().PANEL_BORDER_FOCUS
                    } else {
                        Theme::global().PANEL_BORDER_IDLE
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
                    // Selectable transcript text intentionally consumes mouse-down
                    // so it can retain keyboard focus for copy. Mouse-up still
                    // bubbles, which lets an inactive panel become active without
                    // stealing that focus. This is especially important after a
                    // hot reload, when every restored panel contains a fresh text
                    // selection model.
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.clicked_to_focus(index, cx);
                            this.set_active(index, cx);
                        }),
                    )
                    .child(slot.panel.clone()),
            );
        }

        // Two-finger touchpad swipes (and horizontal mouse wheels) pan the
        // strip directly, like grabbing the canvas. The sticky axis lock in
        // `StripGesture` decides, per gesture, whether deltas pan the strip,
        // scroll the transcript under the pointer, or hop strips vertically.
        let total_width = self
            .row_indices(row)
            .map(|index| self.slot_width(index, viewport_w) + GAP)
            .sum::<f32>()
            + STRUT * 2.0;
        let reticle_alpha = (row == self.active_row)
            .then(|| self.gesture_reticle_alpha())
            .flatten();
        if reticle_alpha.is_some() {
            // Keep painting so the reticle's hold and fade actually play out
            // after the last scroll delta.
            window.request_animation_frame();
        }
        // The reticle leans into an unbroken vertical pull, so the rubber
        // band toward the next strip is visible before it snaps.
        let reticle_pull = if self.gesture.axis == GestureAxis::Horizontal {
            (self.gesture.pull * PULL_RESISTANCE).clamp(-viewport_h / 3.0, viewport_h / 3.0)
        } else {
            0.0
        };
        let entity = cx.entity();
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .pl(px(GAP))
            // A capture-phase router: it sees every scroll event over the
            // strip before the transcripts do, so a gesture that committed
            // to panning can consume its vertical deltas instead of leaking
            // them into whichever panel happens to sit under the pointer.
            // The hitbox respects occlusion, so the minimap keeps its own
            // scroll behavior.
            .child(
                gpui::canvas(
                    move |bounds, window, _| {
                        window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal)
                    },
                    move |_, hitbox, window, _| {
                        window.on_mouse_event(
                            move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                                if phase != gpui::DispatchPhase::Capture
                                    || !hitbox.should_handle_scroll(window)
                                {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    // During a row transition both rows paint
                                    // a router; only the active row's routes,
                                    // and it also stops propagation for the
                                    // outgoing row's transcripts.
                                    if this.active_row != row {
                                        return;
                                    }
                                    if this.route_strip_scroll(
                                        row,
                                        event,
                                        total_width,
                                        viewport_w,
                                        window,
                                        cx,
                                    ) {
                                        cx.stop_propagation();
                                    }
                                });
                            },
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(strip)
            // The gesture reticle: while a touchpad swipe is panning this
            // strip, a ring at the camera's focal point shows exactly where
            // focus will land when the gesture settles. It fades right after
            // the fingers lift so it never lingers over content.
            .when_some(reticle_alpha, |el, alpha| {
                el.child(
                    div()
                        .debug_selector(|| "gesture-reticle".into())
                        .absolute()
                        .left(px((viewport_w - GESTURE_RETICLE_SIZE) / 2.0))
                        .top(px((viewport_h - GESTURE_RETICLE_SIZE) / 2.0 + reticle_pull))
                        .w(px(GESTURE_RETICLE_SIZE))
                        .h(px(GESTURE_RETICLE_SIZE))
                        .rounded_full()
                        .border_2()
                        .border_color(Theme::global().ACCENT)
                        .bg(gpui::rgba(0xffffff14))
                        .opacity(alpha)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(px(5.0))
                                .h(px(5.0))
                                .rounded_full()
                                .bg(Theme::global().ACCENT),
                        ),
                )
            })
            .into_any_element()
    }

    /// Opacity of the gesture reticle right now, or `None` once it has fully
    /// faded. Held bright while deltas keep arriving, then a short fade.
    fn gesture_reticle_alpha(&self) -> Option<f32> {
        self.gesture_last
            .and_then(|last| gesture_alpha(last.elapsed()))
    }

    /// Route one scroll event over the strip through the sticky axis lock.
    /// Returns true when the strip consumed the event and the transcripts
    /// underneath must not also scroll.
    fn route_strip_scroll(
        &mut self,
        row: usize,
        event: &gpui::ScrollWheelEvent,
        total_width: f32,
        viewport_w: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let delta = event.delta.pixel_delta(window.line_height());
        let mut dx = f32::from(delta.x);
        let mut dy = f32::from(delta.y);
        if event.modifiers.shift {
            // Shift redirects vertical scrolling into a horizontal pan.
            if dx == 0.0 {
                dx = dy;
            }
            dy = 0.0;
        }
        let empty_panel = self
            .slots
            .get(self.active)
            .filter(|slot| slot.row == row)
            .is_some_and(|slot| !slot.panel.read(cx).has_scrollable_conversation());
        if empty_panel && dx == 0.0 && dy != 0.0 {
            // With no conversation beneath the pointer, vertical scrolling is
            // workspace navigation. A wheel notch moves one row immediately;
            // precise touchpad deltas accumulate to the same deliberate
            // threshold used by a vertical breakout from a horizontal pan.
            let switch = if event.delta.precise() {
                if event.touch_phase == gpui::TouchPhase::Started
                    || self.gesture.axis != GestureAxis::Vertical
                {
                    self.gesture.reset();
                    self.gesture.axis = GestureAxis::Vertical;
                }
                self.gesture.pull += dy;
                if self.gesture.pull.abs() >= STRIP_BREAK {
                    let switch = if self.gesture.pull < 0.0 { 1 } else { -1 };
                    self.gesture.pull = 0.0;
                    switch
                } else {
                    0
                }
            } else if dy < 0.0 {
                1
            } else {
                -1
            };
            if switch != 0 {
                let target = if switch > 0 {
                    (self.active_row + 1).min(STRIP_COUNT - 1)
                } else {
                    self.active_row.saturating_sub(1)
                };
                if target != self.active_row {
                    self.switch_row_animated(target, window, cx);
                }
            }
            if event.touch_phase == gpui::TouchPhase::Ended {
                self.gesture.reset();
            }
            cx.notify();
            return true;
        }
        if !event.delta.precise() {
            // Mouse wheels have no gesture continuity: horizontal clicks pan,
            // vertical ones stay with the panel under the pointer.
            if dx != 0.0 {
                self.pan_strip(row, dx, total_width, viewport_w, false, window, cx);
                cx.notify();
            }
            return false;
        }
        // Not every platform delivers an Ended phase, so silence also ends
        // the gesture: a fresh burst of deltas starts a fresh axis decision.
        let now = Instant::now();
        if self
            .gesture_seen
            .is_none_or(|seen| seen.elapsed() > GESTURE_RESET)
        {
            self.gesture.reset();
        }
        self.gesture_seen = Some(now);
        match self.gesture.feed(dx, dy, event.touch_phase) {
            Routed::ToPanel => false,
            Routed::Strip {
                dx,
                switch,
                exclusive,
            } => {
                // The reticle explains the resisted vertical pull that hops
                // strips. A purely horizontal pan needs no visualization.
                if self.gesture.axis == GestureAxis::Horizontal && dy != 0.0 {
                    self.gesture_last = Some(now);
                }
                if dx != 0.0 {
                    self.pan_strip(row, dx, total_width, viewport_w, true, window, cx);
                }
                if switch != 0 {
                    // The vertical pull broke the sticky axis: hop to the
                    // neighbouring strip, keeping the gesture alive so a
                    // longer drag steps through several strips.
                    let target = if switch > 0 {
                        (self.active_row + 1).min(STRIP_COUNT - 1)
                    } else {
                        self.active_row.saturating_sub(1)
                    };
                    if target != self.active_row {
                        self.switch_row_animated(target, window, cx);
                    }
                }
                cx.notify();
                exclusive
            }
        }
    }

    /// Pan a strip's camera directly, cancelling any in-flight animation, and
    /// keep focus attached to what the gesture has actually brought under the
    /// camera. Without the focus follow, a swipe only moved the pixels: the
    /// old off-screen panel remained active and the next keyboard action
    /// snapped back to it.
    fn pan_strip(
        &mut self,
        row: usize,
        dx: f32,
        total_width: f32,
        viewport_w: f32,
        smooth: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Natural scrolling: content follows the fingers, so the camera
        // moves opposite the delta.
        // Accumulate precise deltas onto the target, not the lagging painted
        // position. This preserves every pixel of travel while the short
        // interpolation filters event/compositor timing jitter.
        let base = if smooth {
            self.camera_target[row]
        } else {
            self.camera_x[row]
        };
        let next = pan_camera(base, -dx, total_width, viewport_w);
        if (next - base).abs() < f32::EPSILON {
            return;
        }
        self.camera_target[row] = next;
        if smooth {
            self.camera_from[row] = self.camera_x[row];
            self.camera_started[row] = Some(Instant::now());
            self.camera_touch_pan[row] = true;
        } else {
            self.camera_x[row] = next;
            self.camera_from[row] = next;
            self.camera_started[row] = None;
            self.camera_touch_pan[row] = false;
        }
        self.camera_dirty[row] = false;
        if let Some(index) = panel_at_viewport_center(
            self.slots.iter().enumerate().filter_map(|(index, slot)| {
                (slot.row == row).then_some((index, slot.width_fraction))
            }),
            next,
            viewport_w,
        ) && index != self.active
        {
            if let Some(outgoing) = self.slots.get(self.active) {
                self.previous = Some(outgoing.panel.entity_id());
            }
            self.active = index;
            self.active_row = row;
            self.focus_active(window, cx);
        }
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
                    .bg(Theme::global().PANEL_BG)
                    .border_2()
                    .border_color(if focused {
                        Theme::global().PANEL_BORDER_FOCUS
                    } else {
                        Theme::global().PANEL_BORDER
                    })
                    .rounded_xl()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|el| el.border_color(Theme::global().ACCENT))
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
                            .bg(Theme::global().HEADER_BG)
                            .child(div().size(px(7.0)).rounded_full().bg(if busy {
                                Theme::global().WARN
                            } else {
                                Theme::global().OK
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.0))
                                    .text_color(Theme::global().TEXT)
                                    .overflow_hidden()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(Theme::global().TEXT_DIM)
                                    .child(status),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .p_3()
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
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
                .border_color(Theme::global().PANEL_BORDER)
                .rounded_xl()
                .cursor_pointer()
                .text_color(Theme::global().TEXT_DIM)
                .hover(|el| {
                    el.border_color(Theme::global().ACCENT)
                        .text_color(Theme::global().ACCENT)
                })
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

    fn file_browser_root(&self, cx: &App) -> PathBuf {
        self.slots
            .get(self.active)
            .and_then(|slot| {
                slot.panel
                    .read(cx)
                    .working_dir
                    .as_deref()
                    .map(PathBuf::from)
            })
            .filter(|path| path.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    fn render_file_entries(
        &self,
        directory: &Path,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let Ok(read_dir) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut entries = read_dir
            .filter_map(Result::ok)
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            let is_file = entry.file_type().map_or(true, |kind| !kind.is_dir());
            (is_file, entry.file_name().to_ascii_lowercase())
        });

        let mut rows = Vec::new();
        for entry in entries.into_iter().take(500) {
            let path = entry.path();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            let expanded = is_dir && self.expanded_directories.contains(&path);
            let row_path = path.clone();
            let label = entry.file_name().to_string_lossy().into_owned();
            rows.push(
                div()
                    .id(gpui::SharedString::from(format!(
                        "file-tree-row:{}",
                        path.display()
                    )))
                    .debug_selector(move || format!("file-tree-{label}").into())
                    .pl(px(10.0 + depth as f32 * 14.0))
                    .pr_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, window, cx| {
                            if is_dir {
                                if !this.expanded_directories.remove(&row_path) {
                                    this.expanded_directories.insert(row_path.clone());
                                }
                                cx.notify();
                            } else {
                                this.open_code_file(row_path.clone(), window, cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .w(px(12.0))
                            .flex_none()
                            .text_color(Theme::global().TEXT_DIM)
                            .child(if is_dir {
                                if expanded { "▾" } else { "▸" }
                            } else {
                                "·"
                            }),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(entry.file_name().to_string_lossy().into_owned()),
                    )
                    .into_any_element(),
            );
            if expanded {
                rows.extend(self.render_file_entries(&path, depth + 1, cx));
            }
        }
        rows
    }

    fn render_files_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let root = self.file_browser_root(cx);
        div()
            .id("sidebar-file-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .font_family(Theme::global().FONT_MONO)
                    .text_size(px(10.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(root.display().to_string()),
            )
            .children(self.render_file_entries(&root, 0, cx))
            .into_any_element()
    }

    fn render_sidebar(&self, fullscreen: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let active_id = self
            .slots
            .get(self.active)
            .map(|slot| slot.panel.read(cx).session_id.clone());
        let open_statuses = self
            .slots
            .iter()
            .map(|slot| {
                let panel = slot.panel.read(cx);
                (
                    panel.session_id.clone(),
                    panel.sidebar_runtime_status().to_string(),
                )
            })
            .collect::<HashMap<_, _>>();
        let open_titles = self
            .slots
            .iter()
            .map(|slot| {
                let panel = slot.panel.read(cx);
                (panel.session_id.clone(), panel.title.to_string())
            })
            .collect::<HashMap<_, _>>();
        let mut list = div()
            .id("sidebar-session-list")
            .debug_selector(|| "sidebar-session-list".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(&self.sidebar_scroll)
            .py_2();

        // A session which already has a panel is navigation. Everything else
        // is an invitation to open another panel, so keep those two actions in
        // visibly separate sections. Preserve the TUI saved/recency ordering
        // within each section.
        let (mut open_sessions, other_sessions): (Vec<_>, Vec<_>) =
            sidebar_session_order(&self.sessions)
                .into_iter()
                .partition(|session| open_statuses.contains_key(&session.session_id));
        let mut panel_positions = self.slots.iter().enumerate().collect::<Vec<_>>();
        panel_positions.sort_by_key(|(slot_index, slot)| (slot.row, *slot_index));
        let panel_positions = panel_positions
            .into_iter()
            .enumerate()
            .map(|(position, (_, slot))| (slot.panel.read(cx).session_id.clone(), position))
            .collect::<HashMap<_, _>>();
        open_sessions.sort_by_key(|session| {
            panel_positions
                .get(&session.session_id)
                .copied()
                .unwrap_or(usize::MAX)
        });
        let open_session_count = open_sessions.len();
        let other_session_count = other_sessions.len();
        let ordered_sessions = open_sessions
            .into_iter()
            .map(|session| (true, session))
            .chain(other_sessions.into_iter().map(|session| (false, session)))
            .collect::<Vec<_>>();
        let mut previous_section = None;
        let mut previous_saved = None;
        for (sidebar_index, (is_open, session)) in ordered_sessions.into_iter().enumerate() {
            if previous_section != Some(is_open) {
                previous_section = Some(is_open);
                previous_saved = None;
                let (id, label, count, accent) = if is_open {
                    (
                        "sidebar-open-panels-heading",
                        "Live panels",
                        open_session_count,
                        Theme::global().AI_ACCENT,
                    )
                } else {
                    (
                        "sidebar-other-sessions-heading",
                        "Session history",
                        other_session_count,
                        Theme::global().TEXT_DIM,
                    )
                };
                list = list.child(
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .mx_2()
                        .mt(if is_open { px(4.0) } else { px(12.0) })
                        .mb_2()
                        .px_2()
                        .pt(if is_open { px(4.0) } else { px(10.0) })
                        .when(!is_open, |heading| {
                            heading
                                .border_t_1()
                                .border_color(Theme::global().PANEL_BORDER)
                        })
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(10.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(div().size(px(6.0)).rounded_full().bg(accent))
                        .child(
                            div()
                                .flex_1()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(label),
                        )
                        .child(
                            div()
                                .min_w(px(18.0))
                                .px_1()
                                .rounded_full()
                                .bg(Theme::global().HEADER_BG)
                                .text_center()
                                .text_size(px(9.0))
                                .child(count.to_string()),
                        ),
                );
            }
            if previous_saved == Some(true) && !session.saved {
                list = list.child(
                    div()
                        .id("sidebar-session-divider")
                        .debug_selector(|| "sidebar-session-divider".into())
                        .mx_4()
                        .my_2()
                        .border_t_1()
                        .border_color(Theme::global().PANEL_BORDER),
                );
            }
            previous_saved = Some(session.saved);
            let selected = active_id.as_deref() == Some(session.session_id.as_str());
            let (icon, mut title) = sidebar_session_title(&session);
            if session
                .title
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
                && let Some(open_title) = open_titles.get(&session.session_id)
                && custom_sidebar_title(&session.session_id, open_title)
            {
                title.clone_from(open_title);
            }
            let directory = sidebar_session_directory(&session);
            let meta = sidebar_session_meta(&session);
            let details = match (directory, meta) {
                (Some(directory), Some(meta)) => Some(format!("{directory} · {meta}")),
                (Some(directory), None) => Some(directory),
                (None, Some(meta)) => Some(meta),
                (None, None) => None,
            };
            let (status_icon, _status_label, status_kind) = sidebar_session_status(
                &session.status,
                open_statuses.get(&session.session_id).map(String::as_str),
            );
            let status_color = match status_kind {
                SidebarStatusKind::Good => Theme::global().AI_ACCENT,
                SidebarStatusKind::Busy => Theme::global().WARN,
                SidebarStatusKind::Bad => Theme::global().ERROR,
                SidebarStatusKind::Dim => Theme::global().TEXT_DIM,
            };

            list = list.child(
                div()
                    .id(("sidebar-session", sidebar_index))
                    .debug_selector(move || format!("sidebar-session-{sidebar_index}").into())
                    .mx_2()
                    .mb_1()
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .rounded_lg()
                    .cursor_pointer()
                    .bg(if selected {
                        Theme::global().ACCENT_DIM
                    } else {
                        Theme::global().BG
                    })
                    .border_1()
                    .border_color(if selected {
                        Theme::global().PANEL_BORDER_FOCUS
                    } else {
                        Theme::global().PANEL_BORDER_IDLE
                    })
                    .hover(|el| {
                        el.bg(Theme::global().HEADER_BG)
                            .border_color(Theme::global().PANEL_BORDER)
                    })
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
                            .gap_1()
                            .child(div().text_size(px(12.0)).child(icon))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_size(px(12.0))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(status_color)
                                    .child(status_icon),
                            ),
                    )
                    .when_some(details, |row, details| {
                        row.child(
                            div()
                                .pl(px(20.0))
                                .overflow_hidden()
                                .text_size(px(9.0))
                                .text_color(Theme::global().TEXT_DIM)
                                .child(details),
                        )
                    }),
            );
        }

        if self.sessions.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_size(px(11.0))
                    .text_color(Theme::global().TEXT_DIM)
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
            .bg(Theme::global().BG)
            .border_r_1()
            .border_color(Theme::global().PANEL_BORDER)
            .child(
                div()
                    .h(px(TITLEBAR_HEIGHT))
                    // The transparent macOS titlebar puts the traffic lights in
                    // this row. Start navigation after them; on other platforms
                    // preserve the existing 12px inset.
                    .pl(px(sidebar_header_left_padding(fullscreen)))
                    .pr_3()
                    .flex()
                    .items_center()
                    .justify_start()
                    .border_b_1()
                    .border_color(Theme::global().PANEL_BORDER)
                    .child(
                        div()
                            .id("sidebar-navigation-tabs")
                            .debug_selector(|| "sidebar-navigation-tabs".into())
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_x_scroll()
                            .restrict_scroll_to_axis()
                            .child(
                                div()
                                    .id("sidebar-sessions-tab")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(if self.sidebar_view == SidebarView::Sessions {
                                        Theme::global().TEXT
                                    } else {
                                        Theme::global().TEXT_DIM
                                    })
                                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.sidebar_view = SidebarView::Sessions;
                                            cx.notify();
                                        }),
                                    )
                                    .child("chat"),
                            )
                            .child(
                                div()
                                    .id("sidebar-files-tab")
                                    .debug_selector(|| "sidebar-files-tab".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(if self.sidebar_view == SidebarView::Files {
                                        Theme::global().TEXT
                                    } else {
                                        Theme::global().TEXT_DIM
                                    })
                                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.sidebar_view = SidebarView::Files;
                                            cx.notify();
                                        }),
                                    )
                                    .child("files"),
                            )
                            .child(
                                div()
                                    .id("sidebar-unfinished-work")
                                    .debug_selector(|| "sidebar-unfinished-work".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(Theme::global().TEXT_DIM)
                                    .hover(|el| {
                                        el.bg(Theme::global().HEADER_BG)
                                            .text_color(Theme::global().TEXT)
                                    })
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _event, window, cx| {
                                            this.new_unfinished_work(
                                                &NewUnfinishedWork,
                                                window,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child("todos"),
                            )
                            .child(
                                div()
                                    .id("open-todoist")
                                    .debug_selector(|| "open-todoist".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(Theme::global().TEXT_DIM)
                                    .hover(|el| {
                                        el.bg(Theme::global().HEADER_BG)
                                            .text_color(Theme::global().TEXT)
                                    })
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.open_todoist(&OpenTodoist, window, cx)
                                        }),
                                    )
                                    .child("todoist"),
                            )
                            .child(
                                div()
                                    .id("open-gmail")
                                    .debug_selector(|| "open-gmail".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(Theme::global().TEXT_DIM)
                                    .hover(|el| {
                                        el.bg(Theme::global().HEADER_BG)
                                            .text_color(Theme::global().TEXT)
                                    })
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.open_gmail(&OpenGmail, window, cx)
                                        }),
                                    )
                                    .child("email"),
                            )
                            .child(
                                div()
                                    .id("sidebar-open-folder")
                                    .debug_selector(|| "sidebar-open-folder".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(Theme::global().TEXT_DIM)
                                    .hover(|el| {
                                        el.bg(Theme::global().HEADER_BG)
                                            .text_color(Theme::global().TEXT)
                                    })
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
                                    .text_color(Theme::global().TEXT_DIM)
                                    .hover(|el| {
                                        el.bg(Theme::global().HEADER_BG)
                                            .text_color(Theme::global().TEXT)
                                    })
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
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .relative()
                    .child(match self.sidebar_view {
                        SidebarView::Sessions => list.into_any_element(),
                        SidebarView::Files => self.render_files_sidebar(cx),
                    })
                    .child(crate::scrollbar::vertical_with_track(
                        &self.sidebar_scroll,
                        "sidebar-scrollbar",
                    )),
            )
            .when_some(self.render_accounts(), |el, accounts| el.child(accounts))
            .into_any_element()
    }

    /// The automatic-update chip.
    ///
    /// Sparkle used to update entirely in the background, so a user running a
    /// broken build had no way to learn that a fix already existed. The chip
    /// makes each phase visible: a pulse while a check or download is in
    /// flight, and a solid, clickable prompt once a build is staged and one
    /// restart away.
    fn render_update_chip(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = updates::current();
        let label = state.label()?;
        let busy = state.is_busy();
        let actionable = state.is_actionable();
        let ink = if actionable {
            Theme::global().ACCENT
        } else {
            Theme::global().TEXT_DIM
        };

        // Keep this indicator still. Repeating element animations schedule a
        // repaint of the complete workspace, which is especially expensive in a
        // source/debug build and while several markdown transcripts are visible.
        // State transitions still notify the workspace and update the label.
        let dot = div().size(px(6.0)).flex_none().rounded_full().bg(ink);
        let dot: gpui::AnyElement = dot
            .opacity(if busy { 0.65 } else { 1.0 })
            .into_any_element();

        let mut chip = div()
            .id("update-chip")
            // Tagged so a render test can prove the chip painted and can click
            // the real element, rather than only asserting on updater state.
            .debug_selector(|| "update-chip".into())
            .absolute()
            .bottom(px(UPDATE_CHIP_BOTTOM))
            .right(px(MINIMAP_RIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2p5()
            .py_1()
            .rounded_md()
            .bg(Theme::global().MINIMAP_BG)
            .border_1()
            .border_color(if actionable {
                Theme::global().PANEL_BORDER_FOCUS
            } else {
                Theme::global().PANEL_BORDER
            })
            .text_size(px(10.5))
            .font_family(Theme::global().FONT_MONO)
            .text_color(ink)
            .occlude()
            .child(dot)
            .child(label);

        if actionable {
            chip = chip
                .cursor_pointer()
                .hover(|el| el.text_color(Theme::global().TEXT))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|_this, _event, _window, _cx| {
                        // Source builds have no Sparkle framework, so this is a
                        // no-op there rather than a crash.
                        updates::install_now();
                    }),
                );
        }

        Some(chip.into_any_element())
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
                    .border_color(Theme::global().PANEL_BORDER)
                    .bg(Theme::global().HEADER_BG)
                    .text_size(px(20.0))
                    .text_color(Theme::global().TEXT)
                    .child("+"),
            )
            .into_any_element()
    }

    /// The connected-accounts strip stays compact by keeping quota details to
    /// one row and avoiding a second line when usage data is unavailable.
    fn render_accounts(&self) -> Option<gpui::AnyElement> {
        if self.accounts.is_empty() {
            return None;
        }

        let mut section = div()
            .flex_none()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(Theme::global().PANEL_BORDER)
            .py_1()
            .child(
                div()
                    .px_4()
                    .pb(px(2.0))
                    .text_size(px(10.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .child("accounts"),
            );

        for (index, account) in self.accounts.iter().enumerate() {
            let available = account.available();
            let ink = if available {
                Theme::global().TEXT
            } else {
                Theme::global().TEXT_FAINT
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
                    .bg(Theme::global().INLINE_CODE_BG)
                    .text_size(px(10.0))
                    .text_color(ink)
                    .child(accounts::lettermark(&account.display_name))
                    .into_any_element(),
            };

            let mut details = div().flex().flex_col().flex_1().min_w_0().child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
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
                            .text_color(Theme::global().TEXT_DIM)
                            .child(if available {
                                account.auth_kind.clone()
                            } else {
                                format!("{} · expired", account.auth_kind)
                            }),
                    ),
            );

            if !account.limits.is_empty() {
                // A single two-column quota row retains the useful summary
                // without letting one account grow into a card.
                let limit_count = if account.id == "antigravity" {
                    1
                } else {
                    account.limits.len().min(2)
                };
                let visible_limits = &account.limits[..limit_count];
                let mut limits = div().mt(px(3.0)).flex().flex_col().gap(px(4.0));
                for (row_index, row) in visible_limits.chunks(2).enumerate() {
                    let mut limit_row = div().flex().gap(px(6.0));
                    for (column_index, limit) in row.iter().enumerate() {
                        let limit_index = row_index * 2 + column_index;
                        let used = limit.usage_percent.clamp(0.0, 100.0);
                        let label = match &limit.reset_in {
                            Some(reset) => format!("{} · {:.0}% · {reset}", limit.name, used),
                            None => format!("{} · {:.0}%", limit.name, used),
                        };
                        limit_row = limit_row.child(
                            div()
                                .id(("account-limit", index * 1000 + limit_index))
                                .debug_selector({
                                    let id = account.id.clone();
                                    move || format!("account-{id}-limit-{limit_index}")
                                })
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .text_size(px(8.0))
                                        .text_color(Theme::global().TEXT_DIM)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(3.0))
                                        .rounded_full()
                                        .overflow_hidden()
                                        .bg(Theme::global().INLINE_CODE_BG)
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(used / 100.0))
                                                .rounded_full()
                                                .bg(if used >= 90.0 {
                                                    Theme::global().ERROR
                                                } else if used >= 70.0 {
                                                    Theme::global().WARN
                                                } else {
                                                    Theme::global().ACCENT
                                                }),
                                        ),
                                ),
                        );
                    }
                    limits = limits.child(limit_row);
                }
                details = details.child(limits);
            }

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
                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                    .child(logo)
                    .child(details)
                    .child(
                        div()
                            .flex_none()
                            .size(px(5.0))
                            .rounded_full()
                            .bg(if available {
                                Theme::global().OK
                            } else {
                                Theme::global().TEXT_FAINT
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
                .when(active_row, |el| el.bg(Theme::global().ACCENT_DIM))
                .hover(|el| el.bg(Theme::global().MINIMAP_TRACK_ACTIVE))
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
                        .bg(Theme::global().TEXT_DIM),
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
                            Theme::global().ACCENT
                        } else if active_row || busy {
                            Theme::global().MINIMAP_PANEL_BUSY
                        } else {
                            Theme::global().MINIMAP_PANEL
                        })
                        .hover(|el| el.w(px(6.0)).bg(Theme::global().ACCENT))
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
                    .bg(Theme::global().HEADER_BG)
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
        let track_w = MINIMAP_WIDTH - MINIMAP_PADDING * 2.0;
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
            .w(px(MINIMAP_WIDTH))
            .h(px(MINIMAP_HEIGHT))
            .p(px(MINIMAP_PADDING))
            .flex()
            .flex_col()
            .gap(px(MINIMAP_ROW_GAP))
            .rounded_lg()
            .bg(Theme::global().MINIMAP_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
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
                    if (next - this.camera_x[row]).abs() >= f32::EPSILON {
                        this.camera_x[row] = next;
                        this.camera_target[row] = next;
                        this.camera_from[row] = next;
                        this.camera_started[row] = None;
                        this.camera_dirty[row] = false;
                    }
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
                    Theme::global().USER_BG
                } else {
                    Theme::global().MINIMAP_TRACK
                })
                .when(active_row, |el| {
                    el.border_1().border_color(Theme::global().USER_ACCENT)
                })
                .hover(|el| el.bg(Theme::global().MINIMAP_TRACK_ACTIVE))
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
                let (state, todo_progress) = {
                    let panel = self.slots[index].panel.read(cx);
                    let progress = panel.latest_todo_progress().and_then(|(done, total)| {
                        (total > 0).then_some(done as f32 / total as f32)
                    });
                    (panel.minimap_state(), progress)
                };
                let (state_name, state_color) = match state {
                    crate::panel::MinimapSessionState::Idle => {
                        ("idle", Theme::global().MINIMAP_PANEL)
                    }
                    crate::panel::MinimapSessionState::Working => ("working", Theme::global().WARN),
                    crate::panel::MinimapSessionState::Streaming => {
                        ("streaming", Theme::global().ACCENT)
                    }
                    crate::panel::MinimapSessionState::Complete => ("complete", Theme::global().OK),
                    crate::panel::MinimapSessionState::Error => ("error", Theme::global().ERROR),
                };
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
                        .bg(state_color)
                        .child(
                            div()
                                .debug_selector(move || {
                                    format!("minimap-panel-{index}-{state_name}")
                                })
                                .absolute()
                                .size_full()
                                .rounded(px(2.0))
                                .bg(state_color),
                        )
                        // The green footline is a literal completion meter for
                        // the latest todo card. Session color remains visible
                        // above it, so progress and live state do not compete.
                        .when_some(todo_progress, |panel, progress| {
                            panel.child(
                                div()
                                    .debug_selector(move || {
                                        format!("minimap-panel-{index}-todo-progress")
                                    })
                                    .absolute()
                                    .bottom_0()
                                    .left_0()
                                    .h(px(2.0))
                                    .w(relative(progress))
                                    .bg(Theme::global().OK),
                            )
                        })
                        .when(focused, |el| {
                            el.border_2().border_color(Theme::global().USER_ACCENT)
                        })
                        .hover(|el| el.bg(Theme::global().USER_ACCENT))
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
                        .border_color(Theme::global().MINIMAP_VIEWPORT)
                        .bg(gpui::rgba(0xffffff08)),
                );

                // A persistent pin marks the exact focused panel. Unlike the
                // viewport lens, it remains unambiguous when several panels
                // overlap the visible camera region.
                if let Some(index) = self.row_indices(row).find(|index| *index == self.active) {
                    let panel_left = self.slot_left(index, viewport_w) * scale;
                    let panel_width = (self.slot_width(index, viewport_w) * scale - 1.0).max(2.0);
                    track = track.child(
                        div()
                            .debug_selector(|| "minimap-you-pin".into())
                            .absolute()
                            .left(px(panel_left + panel_width / 2.0 - 3.0))
                            .top(px(MINIMAP_ROW_HEIGHT / 2.0 - 3.0))
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .border_1()
                            .border_color(Theme::global().TEXT)
                            .bg(Theme::global().USER_ACCENT),
                    );
                }

                // The gesture dot: the same focal point the canvas reticle
                // marks, mirrored onto the map at the lens center so the eye
                // can track the swipe in either place. The dark ring keeps it
                // legible even over the lit focused-panel rectangle.
                if let Some(alpha) = self.gesture_reticle_alpha() {
                    let dot_left = lens_left + lens_width / 2.0 - MINIMAP_GESTURE_DOT / 2.0;
                    track = track.child(
                        div()
                            .debug_selector(|| "minimap-gesture-dot".into())
                            .absolute()
                            .left(px(dot_left))
                            .top(px((MINIMAP_ROW_HEIGHT - MINIMAP_GESTURE_DOT) / 2.0))
                            .w(px(MINIMAP_GESTURE_DOT))
                            .h(px(MINIMAP_GESTURE_DOT))
                            .rounded_full()
                            .border_2()
                            .border_color(gpui::rgba(0x000000cc))
                            .bg(Theme::global().ACCENT)
                            .opacity(alpha),
                    );
                }
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
            .top(px(MINIMAP_TOP + MINIMAP_HEIGHT + COACH_TOAST_GAP))
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
                    .min_w_0()
                    .overflow_hidden()
                    .top(px((1.0 - progress) * 10.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(Theme::global().PANEL_BG)
                    .border_1()
                    .border_color(Theme::global().PANEL_BORDER_FOCUS)
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
                                    .bg(Theme::global().HEADER_BG)
                                    .text_size(px(12.0))
                                    .font_family(Theme::global().FONT_MONO)
                                    .text_color(Theme::global().TEXT)
                                    .child(hint.keys),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_size(px(12.0))
                                    .text_color(Theme::global().TEXT)
                                    .child(hint.label),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
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
                    .font_family(Theme::global().FONT_MONO)
                    .text_color(if known {
                        Theme::global().TEXT_DIM
                    } else {
                        Theme::global().TEXT
                    })
                    .child(skill.keys),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.0))
                    .text_color(if known {
                        Theme::global().TEXT_DIM
                    } else {
                        Theme::global().TEXT
                    })
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
                    .bg(Theme::global().PANEL_BORDER)
                    .child(
                        div()
                            .w(relative(mastery.clamp(0.02, 1.0)))
                            .h_full()
                            .rounded_full()
                            .bg(if known {
                                Theme::global().OK
                            } else {
                                Theme::global().ACCENT
                            }),
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
            .bg(Theme::global().PANEL_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER_FOCUS)
            .rounded_xl()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_size(px(18.0)).child("Your workspace fluency"))
                    .child(
                        div()
                            .text_color(Theme::global().TEXT_DIM)
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
                    .text_color(Theme::global().TEXT_DIM)
                    .child(format!("{}% learned", (overall * 100.0).round() as u32))
                    .child(format!("{} keystrokes saved", self.coach.effort_saved))
                    .child(format!("{} spent the long way", self.coach.effort_wasted)),
            );

        if let Some(next) = next {
            card = card.child(
                div()
                    .p_2p5()
                    .rounded_lg()
                    .bg(Theme::global().HEADER_BG)
                    .border_1()
                    .border_color(Theme::global().PANEL_BORDER)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
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
                                    .font_family(Theme::global().FONT_MONO)
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
                .bg(Theme::global().HEADER_BG)
                .text_size(px(11.0))
                .text_color(Theme::global().TEXT_DIM)
                .child("Composer: ↑/↓ history · Ctrl+K/J prompts · Ctrl+W word delete · Alt+B/F word move · Ctrl+U delete to start · Ctrl/Cmd+Z undo · Esc clear"),
        );

        for (area, rows) in self.coach.report(now) {
            card = card.child(
                div()
                    .mt_1()
                    .text_size(px(10.0))
                    .text_color(Theme::global().TEXT_DIM)
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
                .bg(Theme::global().HEADER_BG)
                .border_1()
                .border_color(Theme::global().PANEL_BORDER)
                .cursor_pointer()
                .hover(|el| el.border_color(Theme::global().ACCENT))
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
                        .text_color(Theme::global().TEXT_DIM)
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

    fn render_showcase_cue(&self, cue: &ShowcaseCue) -> gpui::AnyElement {
        div()
            .id("showcase-shortcut")
            .debug_selector(|| "showcase-shortcut".into())
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(56.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .id("showcase-card")
                    .debug_selector(|| "showcase-card".into())
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_2()
                    .rounded_lg()
                    .bg(Theme::global().PANEL_BG)
                    .border_1()
                    .border_color(Theme::global().PANEL_BORDER_FOCUS)
                    .shadow_lg()
                    .text_color(Theme::global().TEXT)
                    .with_animation(
                        "showcase-cue-in",
                        Animation::new(Duration::from_millis(180)),
                        |el, delta| el.opacity(delta),
                    )
                    .child(
                        div()
                            .id("showcase-action")
                            .debug_selector(|| "showcase-action".into())
                            .text_size(px(15.0))
                            .child(cue.action),
                    )
                    .child(
                        div()
                            .id("showcase-key")
                            .debug_selector(|| "showcase-key".into())
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(Theme::global().HEADER_BG)
                            .border_1()
                            .border_color(Theme::global().PANEL_BORDER_FOCUS)
                            .font_family(Theme::global().FONT_MONO)
                            .text_color(Theme::global().ACCENT)
                            .text_size(px(12.0))
                            .child(cue.shortcut.clone()),
                    ),
            )
            .into_any_element()
    }

    /// Contextual, clickable tutorial controls. Rather than collecting abstract
    /// English descriptions in a footer, each lesson lives beside the part of
    /// the canvas it affects and depicts the resulting motion directly.
    fn render_tutorial_guides(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let modifier = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Super"
        };
        let active = self.showcase_cue.as_ref().map(|cue| cue.tutorial_group);
        let cue_animation = self
            .showcase_cue
            .as_ref()
            .map(|cue| format!("tutorial-trigger-{}", cue.shortcut));
        // A tutorial control turns green once it has been practiced. The
        // learning model still treats prompted use as weaker than unaided
        // recall, but leaving prompted navigation unmarked made the tutorial
        // appear to ignore successful use.
        let learned = |skill: &str| self.coach.trace(skill).practiced();
        let stage_one_complete = [
            "focus_left_right",
            "focus_up_down",
            "new_panel",
            "close_panel",
        ]
        .into_iter()
        .all(learned);
        let stage_two_complete = ["move_panel", "move_panel_strip", "width_presets"]
            .into_iter()
            .all(learned);
        let tutorial_stage = if !stage_one_complete {
            1
        } else if !stage_two_complete {
            2
        } else {
            3
        };

        let arrow = |id: &'static str,
                     glyph: &'static str,
                     key: &'static str,
                     skill: &'static str,
                     group: &'static str,
                     action: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let is_learned = learned(skill);
            let is_pressed = self.showcase_cue.as_ref().is_some_and(|cue| {
                cue.tutorial_group == group && cue.shortcut.ends_with(&format!(" + {key}"))
            });
            let animation_id = format!(
                "{id}-{}",
                cue_animation.as_deref().unwrap_or("tutorial-idle")
            );
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(5.0))
                .rounded_md()
                .border_1()
                .border_color(if is_learned {
                    Theme::global().OK
                } else if active == Some(group) {
                    Theme::global().ACCENT
                } else {
                    Theme::global().PANEL_BORDER
                })
                .bg(if is_learned {
                    gpui::rgba(0x64c86424)
                } else if active == Some(group) {
                    Theme::global().ACCENT_DIM
                } else {
                    Theme::global().HEADER_BG
                })
                .text_size(px(11.0))
                .text_color(if is_learned {
                    Theme::global().OK
                } else {
                    Theme::global().TEXT
                })
                .cursor_pointer()
                .occlude()
                .hover(|el| {
                    el.border_color(Theme::global().ACCENT)
                        .text_color(Theme::global().ACCENT)
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, window, cx| action(this, window, cx)),
                )
                .child(div().text_size(px(17.0)).child(glyph).with_animation(
                    animation_id,
                    Animation::new(Duration::from_millis(220)),
                    move |el, delta| {
                        let press = if is_pressed {
                            (std::f32::consts::PI * delta).sin() * 4.0
                        } else {
                            0.0
                        };
                        el.mt(px(press))
                    },
                ))
                .child(format!("{modifier} {key}"))
                .when(is_learned, |el| {
                    el.child(
                        div()
                            .debug_selector(move || format!("tutorial-learned-{skill}").into())
                            .child("✓"),
                    )
                })
        };
        let navigation = div()
            // This is only a positioning layer. Giving the full-window layer
            // an id creates a hitbox above the strip's capture-phase gesture
            // router, which makes touchpad swipes disappear while tutorial
            // mode is enabled. The individual controls remain interactive.
            .absolute()
            .inset_0()
            .child(
                arrow(
                    "tutorial-nav-up",
                    "↑",
                    if tutorial_stage == 2 { "Shift K" } else { "K" },
                    if tutorial_stage == 2 {
                        "move_panel_strip"
                    } else {
                        "focus_up_down"
                    },
                    if tutorial_stage == 2 {
                        "move"
                    } else {
                        "navigate"
                    },
                    if tutorial_stage == 2 {
                        |this, window, cx| this.move_panel_up(&MovePanelUp, window, cx)
                    } else {
                        |this, window, cx| this.focus_up(&FocusUp, window, cx)
                    },
                )
                .absolute()
                .top(px(12.0))
                .left(relative(0.5)),
            )
            .child(
                arrow(
                    "tutorial-nav-left",
                    "←",
                    if tutorial_stage == 2 { "Shift H" } else { "H" },
                    if tutorial_stage == 2 {
                        "move_panel"
                    } else {
                        "focus_left_right"
                    },
                    if tutorial_stage == 2 {
                        "move"
                    } else {
                        "navigate"
                    },
                    if tutorial_stage == 2 {
                        |this, window, cx| this.move_panel_left(&MovePanelLeft, window, cx)
                    } else {
                        |this, window, cx| this.focus_left(&FocusLeft, window, cx)
                    },
                )
                .absolute()
                .left(px(12.0))
                .top(relative(0.5)),
            )
            .child(
                arrow(
                    "tutorial-nav-right",
                    "→",
                    if tutorial_stage == 2 { "Shift L" } else { "L" },
                    if tutorial_stage == 2 {
                        "move_panel"
                    } else {
                        "focus_left_right"
                    },
                    if tutorial_stage == 2 {
                        "move"
                    } else {
                        "navigate"
                    },
                    if tutorial_stage == 2 {
                        |this, window, cx| this.move_panel_right(&MovePanelRight, window, cx)
                    } else {
                        |this, window, cx| this.focus_right(&FocusRight, window, cx)
                    },
                )
                .absolute()
                .right(px(12.0))
                .top(relative(0.5)),
            )
            .child(
                arrow(
                    "tutorial-nav-down",
                    "↓",
                    if tutorial_stage == 2 { "Shift J" } else { "J" },
                    if tutorial_stage == 2 {
                        "move_panel_strip"
                    } else {
                        "focus_up_down"
                    },
                    if tutorial_stage == 2 {
                        "move"
                    } else {
                        "navigate"
                    },
                    if tutorial_stage == 2 {
                        |this, window, cx| this.move_panel_down(&MovePanelDown, window, cx)
                    } else {
                        |this, window, cx| this.focus_down(&FocusDown, window, cx)
                    },
                )
                .absolute()
                // Keep the down lesson clear of the prompt composer, which is
                // anchored to the bottom of every session panel.
                .bottom(px(104.0))
                .left(relative(0.5)),
            );

        let layout = div()
            .id("tutorial-layout")
            .debug_selector(|| "tutorial-layout".into())
            .absolute()
            .top(px(MINIMAP_TOP + MINIMAP_HEIGHT + 8.0))
            .right(px(MINIMAP_RIGHT))
            .flex()
            .gap(px(5.0))
            .child(
                div()
                    .id("tutorial-resize")
                    .debug_selector(|| "tutorial-resize".into())
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active == Some("resize") {
                        Theme::global().ACCENT_DIM
                    } else {
                        Theme::global().HEADER_BG
                    })
                    .border_1()
                    .border_color(if active == Some("resize") {
                        Theme::global().ACCENT
                    } else {
                        Theme::global().PANEL_BORDER
                    })
                    .hover(|el| el.border_color(Theme::global().ACCENT))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.cycle_width(&CycleWidth, window, cx)
                        }),
                    )
                    .child(format!("↔  {modifier} R"))
                    .with_animation(
                        format!(
                            "tutorial-resize-{}",
                            cue_animation.as_deref().unwrap_or("idle")
                        ),
                        Animation::new(Duration::from_millis(220)),
                        move |el, delta| {
                            el.mt(px(if active == Some("resize") {
                                (std::f32::consts::PI * delta).sin() * 4.0
                            } else {
                                0.0
                            }))
                        },
                    ),
            )
            .child(
                div()
                    .id("tutorial-overview")
                    .debug_selector(|| "tutorial-overview".into())
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active == Some("overview") {
                        Theme::global().ACCENT_DIM
                    } else {
                        Theme::global().HEADER_BG
                    })
                    .border_1()
                    .border_color(if active == Some("overview") {
                        Theme::global().ACCENT
                    } else {
                        Theme::global().PANEL_BORDER
                    })
                    .hover(|el| el.border_color(Theme::global().ACCENT))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.toggle_overview(&ToggleOverview, window, cx)
                        }),
                    )
                    .child(format!("▦  {modifier} O"))
                    .with_animation(
                        format!(
                            "tutorial-overview-{}",
                            cue_animation.as_deref().unwrap_or("idle")
                        ),
                        Animation::new(Duration::from_millis(220)),
                        move |el, delta| {
                            el.mt(px(if active == Some("overview") {
                                (std::f32::consts::PI * delta).sin() * 4.0
                            } else {
                                0.0
                            }))
                        },
                    ),
            );

        let new_session = div()
            .id("tutorial-new")
            .debug_selector(|| "tutorial-new".into())
            .absolute()
            .right(px(10.0))
            // The right navigation lesson also lives at mid-height. Keep the
            // creation lesson in the upper tool cluster instead of stacking it
            // over Super+L.
            .top(px(MINIMAP_TOP + MINIMAP_HEIGHT + 42.0))
            .px_2()
            .py_1()
            .rounded_lg()
            .cursor_pointer()
            .bg(if learned("new_panel") {
                gpui::rgba(0x64c86424)
            } else if active == Some("new") {
                Theme::global().ACCENT_DIM
            } else {
                gpui::rgba(0x111318e8)
            })
            .border_1()
            .border_color(if learned("new_panel") {
                Theme::global().OK
            } else if active == Some("new") {
                Theme::global().ACCENT
            } else {
                Theme::global().PANEL_BORDER
            })
            .text_color(if learned("new_panel") {
                Theme::global().OK
            } else {
                Theme::global().TEXT
            })
            .hover(|el| el.border_color(Theme::global().ACCENT))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.new_panel(&NewPanel, window, cx)),
            )
            .child(format!("＋  {modifier} N"))
            .when(learned("new_panel"), |el| {
                el.child(
                    div()
                        .debug_selector(|| "tutorial-learned-new_panel".into())
                        .child("✓"),
                )
            })
            .with_animation(
                format!(
                    "tutorial-new-{}",
                    cue_animation.as_deref().unwrap_or("idle")
                ),
                Animation::new(Duration::from_millis(220)),
                move |el, delta| {
                    el.mt(px(if active == Some("new") {
                        (std::f32::consts::PI * delta).sin() * 4.0
                    } else {
                        0.0
                    }))
                },
            );

        let close_session = div()
            .id("tutorial-close")
            .debug_selector(|| "tutorial-close".into())
            .absolute()
            .left(px(10.0))
            .top(px(MINIMAP_TOP + MINIMAP_HEIGHT + 42.0))
            .px_2()
            .py_1()
            .rounded_lg()
            .cursor_pointer()
            .bg(if learned("close_panel") {
                gpui::rgba(0x64c86424)
            } else {
                gpui::rgba(0x111318e8)
            })
            .border_1()
            .border_color(if learned("close_panel") {
                Theme::global().OK
            } else {
                Theme::global().PANEL_BORDER
            })
            .text_color(if learned("close_panel") {
                Theme::global().OK
            } else {
                Theme::global().TEXT
            })
            .hover(|el| el.border_color(Theme::global().ACCENT))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_panel(&ClosePanel, window, cx)),
            )
            .child(format!("×  {modifier} Q"));

        let width_presets = div()
            .id("tutorial-width-presets")
            .debug_selector(|| "tutorial-width-presets".into())
            .absolute()
            .top(px(MINIMAP_TOP + MINIMAP_HEIGHT + 8.0))
            .right(px(MINIMAP_RIGHT))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(if learned("width_presets") {
                Theme::global().OK
            } else {
                Theme::global().PANEL_BORDER
            })
            .bg(if learned("width_presets") {
                gpui::rgba(0x64c86424)
            } else {
                Theme::global().HEADER_BG
            })
            .text_color(if learned("width_presets") {
                Theme::global().OK
            } else {
                Theme::global().TEXT
            })
            .child(format!("↔  {modifier} 1 2 3 4"));

        let stage_label = div()
            .id("tutorial-stage")
            .debug_selector(|| "tutorial-stage".into())
            .absolute()
            .top(px(12.0))
            .left(px(12.0))
            .text_size(px(10.0))
            .text_color(Theme::global().TEXT_DIM)
            .child(format!("onboarding · step {tutorial_stage} of 3"));

        div()
            .id("tutorial-guides")
            .debug_selector(|| "tutorial-guides".into())
            .absolute()
            .inset_0()
            .text_size(px(10.0))
            .font_family(Theme::global().FONT_MONO)
            .child(stage_label)
            .when(tutorial_stage <= 2, |el| el.child(navigation))
            .when(tutorial_stage == 1, |el| {
                el.child(new_session).child(close_session)
            })
            .when(tutorial_stage == 2, |el| el.child(width_presets))
            .when(tutorial_stage == 3, |el| el.child(layout))
            .into_any_element()
    }
}

impl Workspace {
    fn ranked_folder_matches(&self, base: &Path, query: &str) -> Vec<(PathBuf, String)> {
        ranked_folder_matches_for_sessions(&self.sessions, base, query)
    }

    fn render_folder_picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(directory) = self.folder_picker_dir.clone() else {
            return div().into_any_element();
        };
        let query = self
            .folder_search
            .as_ref()
            .map(|search| search.read(cx).content.trim().to_lowercase())
            .unwrap_or_default();
        let entries = self.ranked_folder_matches(&directory, &query);
        let mut list = div()
            .id("folder-picker-list")
            .debug_selector(|| "folder-picker-list".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .border_t_1()
            .border_b_1()
            .border_color(Theme::global().PANEL_BORDER);

        if let Some(parent) = directory.parent().map(Path::to_path_buf) {
            list = list.child(
                div()
                    .id("folder-picker-parent")
                    .debug_selector(|| "folder-picker-parent".into())
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .text_color(Theme::global().TEXT_DIM)
                    .hover(|el| {
                        el.bg(Theme::global().HEADER_BG)
                            .text_color(Theme::global().TEXT)
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.browse_to(parent.clone(), cx);
                        }),
                    )
                    .child("↰  .."),
            );
        }
        for (index, (path, reason)) in entries.into_iter().enumerate() {
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
                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.browse_to(path.clone(), cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(format!("▸  {label}"))
                            .when(!reason.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(Theme::global().TEXT_DIM)
                                        .child(reason),
                                )
                            }),
                    ),
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
                    .border_color(Theme::global().PANEL_BORDER_FOCUS)
                    .bg(Theme::global().PANEL_BG)
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
                                    .debug_selector(|| "folder-picker-cancel".into())
                                    .cursor_pointer()
                                    .text_color(Theme::global().TEXT_DIM)
                                    .hover(|el| el.text_color(Theme::global().TEXT))
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
                            .text_color(Theme::global().TEXT_DIM)
                            .child(directory.display().to_string()),
                    )
                    .child(
                        div()
                            .px_4()
                            .pb_3()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("folder-picker-home")
                                    .debug_selector(|| "folder-picker-home".into())
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(Theme::global().HEADER_BG)
                                    .hover(|el| el.text_color(Theme::global().TEXT))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            if let Some(home) = default_working_dir() {
                                                this.browse_to(PathBuf::from(home), cx);
                                            }
                                        }),
                                    )
                                    .child("⌂  home"),
                            )
                            .child(
                                div()
                                    .id("folder-picker-computer")
                                    .debug_selector(|| "folder-picker-computer".into())
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(Theme::global().HEADER_BG)
                                    .hover(|el| el.text_color(Theme::global().TEXT))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.browse_to(filesystem_root(), cx);
                                        }),
                                    )
                                    .child("▣  computer"),
                            ),
                    )
                    .when_some(self.folder_search.clone(), |el, search| {
                        el.child(
                            div()
                                .id("folder-picker-search")
                                .debug_selector(|| "folder-picker-search".into())
                                .mx_4()
                                .mb_3()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(Theme::global().INPUT_BORDER)
                                .bg(Theme::global().INPUT_BG)
                                .child(search),
                        )
                    })
                    .child(list)
                    .when_some(self.folder_picker_error.clone(), |el, error| {
                        el.child(
                            div()
                                .px_4()
                                .py_2()
                                .text_color(Theme::global().ERROR)
                                .child(error),
                        )
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
                                .bg(Theme::global().ACCENT)
                                .text_color(Theme::global().BG)
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
        let render_started = Instant::now();
        self.dump_state();
        if self.focus_pending && !self.slots.is_empty() {
            self.focus_pending = false;
            self.focus_active(window, cx);
        }
        let viewport = window.viewport_size();
        let sidebar_width = if self.show_sidebar {
            SIDEBAR_WIDTH
        } else {
            0.0
        };
        let viewport_w = (f32::from(viewport.width) - sidebar_width).max(320.0);
        // On macOS the sidebar header covers the transparent titlebar strip.
        // Without the sidebar, leave room for the traffic lights. Other platforms
        // do not draw through a system titlebar, so an inset would be a visible gap.
        // In native macOS fullscreen the titlebar and traffic lights are hidden,
        // so no chrome should reserve space for them.
        let fullscreen = window.is_fullscreen();
        let content_top_inset = content_top_inset(self.show_sidebar, fullscreen);
        let viewport_h = (f32::from(viewport.height) - content_top_inset).max(240.0);

        let now = Instant::now();
        let overview_progress = self.overview_progress.sample(now);
        let hints_progress = self.hints_progress.sample(now);
        // Expire the hint on a schedule of its own, so a suggestion the user
        // ignores fades without needing another input to clear it.
        let coach_hint = crate::config::get()
            .workspace
            .coaching_hints
            .then(|| self.coach.active_hint(learning::now()))
            .flatten();
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
        } else {
            // One path for both the populated and the empty strip, so the
            // empty strip's gesture handling exists everywhere it paints.
            self.render_row(self.active_row, viewport_w, viewport_h, window, cx)
        };

        if self.performance.is_some() {
            let frame = window.frame_duration_snapshot();
            let input = window.input_latency_snapshot();
            let milliseconds = |nanoseconds: u64| nanoseconds as f64 / 1_000_000.0;
            self.gpui_performance = GpuiPerformanceSnapshot {
                draw_p95_ms: milliseconds(frame.draw_duration_histogram.value_at_quantile(0.95)),
                present_p95_ms: milliseconds(
                    frame.present_interval_histogram.value_at_quantile(0.95),
                ),
                input_to_frame_p95_ms: milliseconds(
                    input.latency_histogram.value_at_quantile(0.95),
                ),
                input_events_per_frame_p95: input
                    .events_per_frame_histogram
                    .value_at_quantile(0.95),
                mid_draw_inputs: input.mid_draw_events_dropped,
            };
        }
        let action_settled = !self.row_progress.is_animating()
            && !self.camera_dirty[self.active_row]
            && self.camera_started[self.active_row].is_none();
        if let Some(capture) = self.action_capture.as_mut() {
            capture.observe(
                Instant::now(),
                action_settled,
                window.frame_duration_snapshot(),
                window.input_latency_snapshot(),
            );
        }
        let gpui_performance = self.gpui_performance;
        let performance = self.performance.as_ref().map(|profile| {
            let snapshot = profile.snapshot();
            let (label, color) = match snapshot.health {
                PerformanceHealth::Good => ("GOOD", gpui::rgb(0x52d273)),
                PerformanceHealth::Degraded => ("SLOW", gpui::rgb(0xf2c94c)),
                PerformanceHealth::Bad => ("BAD", gpui::rgb(0xff6b6b)),
            };
            div()
                .debug_selector(|| "selfdev-performance-profile".into())
                .absolute()
                .top(px(content_top_inset + 10.0))
                .right(px(12.0))
                .rounded_md()
                .px_3()
                .py_2()
                .bg(gpui::rgba(0x111318e8))
                .border_1()
                .border_color(color)
                .text_size(px(11.0))
                .text_color(gpui::rgb(0xe5e7eb))
                .child(format!(
                    "PERF {label}  presented {:.0} fps / p95 {:.1} ms  input→frame {:.1} ms  draw {:.1} ms  view {:.1} ms / worst {:.1} ms  wake {:.1} ms  coalesced {}  mid-draw {}",
                    if gpui_performance.present_p95_ms > 0.0 {
                        1_000.0 / gpui_performance.present_p95_ms
                    } else {
                        0.0
                    },
                    gpui_performance.present_p95_ms,
                    gpui_performance.input_to_frame_p95_ms,
                    gpui_performance.draw_p95_ms,
                    snapshot.render_p95_ms,
                    snapshot.worst_ms,
                    snapshot.wake_p95_ms,
                    gpui_performance.input_events_per_frame_p95,
                    gpui_performance.mid_draw_inputs,
                ))
        });

        let root = div()
            .size_full()
            .flex()
            .flex_row()
            // A slight warm lift across the black canvas suggests matte paper
            // while preserving every workspace interaction above it.
            .bg(gpui::linear_gradient(
                138.0,
                gpui::linear_color_stop(gpui::rgb(0x080808), 0.0),
                gpui::linear_color_stop(gpui::rgb(0x141311), 1.0),
            ))
            .font_family(Theme::global().FONT_UI)
            .text_size(px(14.0 * crate::config::get().appearance.text_scale))
            .text_color(Theme::global().TEXT)
            .track_focus(&self.focus_handle)
            // Navigation belongs to the canvas, regardless of which control in
            // the active panel currently owns keyboard focus. Capture these at
            // the workspace boundary so a terminal, composer, picker, or other
            // focused child cannot consume the action before it reaches us.
            .capture_action(cx.listener(Self::focus_left))
            .capture_action(cx.listener(Self::focus_right))
            .capture_action(cx.listener(Self::focus_up))
            .capture_action(cx.listener(Self::focus_down))
            .capture_action(cx.listener(Self::focus_first))
            .capture_action(cx.listener(Self::focus_last))
            .capture_action(cx.listener(Self::focus_previous))
            .on_action(cx.listener(Self::move_panel_left))
            .on_action(cx.listener(Self::move_panel_right))
            .on_action(cx.listener(Self::move_panel_up))
            .on_action(cx.listener(Self::move_panel_down))
            .on_action(cx.listener(Self::move_panel_to_first))
            .on_action(cx.listener(Self::move_panel_to_last))
            .on_action(cx.listener(Self::new_panel))
            .on_action(cx.listener(Self::fork_panel))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::open_gmail))
            .on_action(cx.listener(Self::open_todoist))
            .on_action(cx.listener(Self::new_unfinished_work))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::close_panel))
            .on_action(cx.listener(Self::toggle_overview))
            .on_action(cx.listener(Self::toggle_hints))
            .on_action(cx.listener(Self::toggle_showcase))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::new_help_session))
            .on_action(cx.listener(Self::cycle_width))
            .on_action(cx.listener(Self::maximize_width))
            .on_action(cx.listener(|this, _: &WidthPreset1, _w, cx| this.set_width(0.25, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset2, _w, cx| this.set_width(0.5, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset3, _w, cx| this.set_width(0.75, cx)))
            .on_action(cx.listener(|this, _: &WidthPreset4, _w, cx| this.set_width(1.0, cx)))
            .when(self.show_sidebar, |root| {
                root.child(self.render_sidebar(fullscreen, cx))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .pt(px(content_top_inset))
                    .child(content)
                    .child(self.render_workspace_bar(cx))
                    .when(!self.slots.is_empty() && overview_progress <= 0.0, |el| {
                        el.child(self.render_minimap(viewport_w, viewport_h, cx))
                    })
                    .when_some(coach_hint.filter(|_| coach_progress > 0.0), |el, hint| {
                        el.child(self.render_coach_toast(&hint, coach_progress, cx))
                    })
                    .when(overview_progress <= 0.0, |el| {
                        el.child(self.render_edge_new_session(cx))
                    })
                    // Showcase feedback for tutorial actions lives on the
                    // corresponding lesson itself. Keep the standalone card
                    // only for mode/help feedback that has no tutorial icon.
                    // The tutorial is first-run onboarding, not a permanent
                    // workspace mode. Persisted practice makes it disappear as
                    // soon as the final lesson is learned and keeps it gone on
                    // future launches.
                    .when(
                        self.showcase_mode
                            && (!self.onboarding_complete() || self.showcase_cue.is_some()),
                        |el| el.child(self.render_tutorial_guides(cx)),
                    )
                    // Paint non-tutorial feedback last so it remains above the
                    // canvas without covering an animated tutorial control.
                    .when_some(
                        self.showcase_cue
                            .as_ref()
                            .filter(|cue| matches!(cue.tutorial_group, "" | "help")),
                        |el, cue| el.child(self.render_showcase_cue(cue)),
                    )
                    // Update status stays visible in every mode, including
                    // overview: a user whose build cannot render text still
                    // needs to see that a fix is on its way.
                    .when_some(self.render_update_chip(cx), |el, chip| el.child(chip)),
            )
            .when_some(performance, |root, performance| root.child(performance))
            .when(hints_progress > 0.0, |root| {
                root.child(self.render_hints_overlay(hints_progress, cx))
            })
            .when(self.folder_picker_dir.is_some(), |root| {
                root.child(self.render_folder_picker(cx))
            });
        let animation_active = self.animation_active();
        let action_capture_pending = self
            .action_capture
            .as_ref()
            .is_some_and(ActionCapture::is_pending);
        if animation_active || action_capture_pending {
            self.ensure_animation_tick(cx);
        }
        if let Some(profile) = self.performance.as_mut() {
            profile.observe_render(render_started.elapsed());
            profile.observe_frame(Instant::now(), animation_active);
        }
        root
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn sidebar_enabled(
    arguments: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
    configured_default: bool,
) -> bool {
    configured_default
        && !arguments.into_iter().any(|argument| {
            let argument = argument.as_ref();
            argument == "--no-sidebar" || argument == "--workspace"
        })
}

fn default_working_dir() -> Option<String> {
    std::env::var("HOME").ok()
}

fn filesystem_root() -> PathBuf {
    PathBuf::from(std::path::MAIN_SEPARATOR.to_string())
}

fn contains_dot_directory(path: &Path, base: &Path) -> bool {
    path.strip_prefix(base).unwrap_or(path).components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.to_string_lossy().starts_with('.'))
    })
}

fn directory_entries(path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if entry.file_name().to_string_lossy().starts_with('.') {
                return None;
            }
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

fn ranked_folder_matches_for_sessions(
    sessions: &[jcode_sdk::SessionInfo],
    base: &Path,
    query: &str,
) -> Vec<(PathBuf, String)> {
    let mut usage: HashMap<PathBuf, (usize, usize)> = HashMap::new();
    for (recency, session) in sessions.iter().rev().enumerate() {
        let Some(path) = session
            .working_dir
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| path.is_dir() && !contains_dot_directory(path, base))
        else {
            continue;
        };
        let entry = usage.entry(path).or_insert((0, recency));
        entry.0 += 1;
        entry.1 = entry.1.min(recency);
    }

    let common_names = [
        "projects",
        "code",
        "dev",
        "src",
        "workspace",
        "documents",
        "desktop",
    ];
    let children = directory_entries(base).unwrap_or_default();
    let mut reasons = HashMap::<PathBuf, String>::new();
    let mut candidates = usage.keys().cloned().collect::<Vec<_>>();
    candidates.extend(children);
    if !query.is_empty()
        && let Ok(entries) = std::fs::read_dir(base)
    {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(query)
            {
                reasons.insert(
                    base.to_path_buf(),
                    format!("contains file · {}", entry.file_name().to_string_lossy()),
                );
                candidates.push(base.to_path_buf());
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates.retain(|path| {
        let searchable = path.to_string_lossy().to_lowercase();
        if !query.is_empty() {
            searchable.contains(query) || reasons.contains_key(path)
        } else {
            // An empty search is a real filesystem browser, not only a list of
            // guesses. Keeping every direct child lets the user walk from home
            // to its parent and all the way to the filesystem root.
            true
        }
    });
    candidates.sort_by_key(|path| {
        let file_match = reasons.contains_key(path);
        let (count, recency) = usage.get(path).copied().unwrap_or((0, usize::MAX));
        let likely = path.file_name().is_some_and(|name| {
            common_names.contains(&name.to_string_lossy().to_lowercase().as_str())
        });
        (
            std::cmp::Reverse(file_match),
            std::cmp::Reverse(count),
            recency,
            std::cmp::Reverse(likely),
            path.clone(),
        )
    });
    candidates
        .into_iter()
        .take(if query.is_empty() { 200 } else { 30 })
        .map(|path| {
            let reason = reasons
                .remove(&path)
                .unwrap_or_else(|| match usage.get(&path).copied() {
                    Some((count, _)) if count > 1 => format!("frequent · {count} sessions"),
                    Some(_) => "recent".into(),
                    None if path.file_name().is_some_and(|name| {
                        common_names.contains(&name.to_string_lossy().to_lowercase().as_str())
                    }) =>
                    {
                        "likely".into()
                    }
                    None => String::new(),
                });
            (path, reason)
        })
        .collect()
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

fn custom_sidebar_title(session_id: &str, title: &str) -> bool {
    let title = title.trim();
    !title.is_empty()
        && title != jcode_core::id::extract_session_name(session_id).unwrap_or(session_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarStatusKind {
    Good,
    Busy,
    Bad,
    Dim,
}

fn sidebar_session_status(
    persisted: &str,
    runtime: Option<&str>,
) -> (&'static str, &'static str, SidebarStatusKind) {
    if let Some(runtime) = runtime {
        return match runtime.to_ascii_lowercase().as_str() {
            "generating" | "running" | "busy" | "thinking" | "streaming" => {
                ("●", "working", SidebarStatusKind::Busy)
            }
            status if status.starts_with("lost:") => ("!", "lost", SidebarStatusKind::Bad),
            _ => ("●", "ready", SidebarStatusKind::Good),
        };
    }

    match persisted.to_ascii_lowercase().as_str() {
        "active" | "attached" => ("▶", "active", SidebarStatusKind::Good),
        "crashed" => ("💥", "crashed", SidebarStatusKind::Bad),
        "error" | "errored" => ("✕", "errored", SidebarStatusKind::Bad),
        "reloaded" => ("↻", "reloaded", SidebarStatusKind::Good),
        "compacted" => ("▣", "compacted", SidebarStatusKind::Busy),
        "ratelimited" | "rate_limited" | "rate limited" => {
            ("⌛", "rate limited", SidebarStatusKind::Busy)
        }
        _ => ("✓", "closed", SidebarStatusKind::Dim),
    }
}

fn sidebar_session_order(sessions: &[jcode_sdk::SessionInfo]) -> Vec<jcode_sdk::SessionInfo> {
    let mut ordered = sessions.to_vec();
    ordered.sort_by(|a, b| {
        b.saved
            .cmp(&a.saved)
            .then_with(|| sidebar_session_recency_ms(b).cmp(&sidebar_session_recency_ms(a)))
    });
    ordered
}

fn sidebar_session_recency_ms(session: &jcode_sdk::SessionInfo) -> i64 {
    session
        .updated_at_ms
        .or_else(|| {
            sidebar_session_created_ms(&session.session_id)
                .and_then(|value| i64::try_from(value).ok())
        })
        .unwrap_or_default()
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn sidebar_session_directory(session: &jcode_sdk::SessionInfo) -> Option<String> {
    session
        .working_dir
        .as_deref()
        .map(str::trim)
        .filter(|directory| !directory.is_empty())
        .map(compact_working_dir)
}

/// Creation timestamp embedded in modern session ids (`session_fox_<ms>_...`).
fn sidebar_session_created_ms(session_id: &str) -> Option<u64> {
    session_id
        .split('_')
        .filter_map(|part| part.parse::<u64>().ok())
        .find(|value| (1_000_000_000_000..10_000_000_000_000).contains(value))
}

/// `3m ago` / `2h ago` / `5d ago`, matching the TUI picker's time labels.
fn format_time_ago(created_ms: u64, now_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(created_ms) / 1_000;
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{days}d ago");
    }
    if days < 30 {
        return format!("{}w ago", days / 7);
    }
    format!("{}mo ago", days / 30)
}

/// `~1.2k tok`, matching the TUI picker's token display. The desktop only has
/// the stored transcript size, so estimate at ~4 bytes per token.
fn format_estimated_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return format!("~{tokens} tok");
    }
    const UNITS: &[(f64, &str)] = &[(1.0, ""), (1_000.0, "k"), (1_000_000.0, "M")];
    let value = tokens as f64;
    let mut index = 0;
    while index + 1 < UNITS.len() && value >= UNITS[index + 1].0 {
        index += 1;
    }
    let scaled = value / UNITS[index].0;
    if scaled >= 100.0 {
        format!("~{:.0}{} tok", scaled, UNITS[index].1)
    } else {
        format!("~{:.1}{} tok", scaled, UNITS[index].1)
    }
}

/// The TUI picker's per-session metadata line, from the fields the desktop
/// API actually carries: `12m ago · ~4.2k tok`.
fn sidebar_session_meta(session: &jcode_sdk::SessionInfo) -> Option<String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    let created = sidebar_session_created_ms(&session.session_id)
        .map(|created_ms| format_time_ago(created_ms, now_ms));
    let tokens = session
        .transcript_bytes
        .filter(|bytes| *bytes > 0)
        .map(|bytes| format_estimated_tokens(bytes / 4));
    match (created, tokens) {
        (Some(created), Some(tokens)) => Some(format!("{created} · {tokens}")),
        (Some(created), None) => Some(created),
        (None, Some(tokens)) => Some(tokens),
        (None, None) => None,
    }
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

/// Shared cubic ease-out used by the strip camera.
fn ease_out_cubic(t: f32) -> f32 {
    transition::ease_out_cubic(t)
}

/// Where a newly created panel is inserted: directly right of the focused
/// panel when it is on this strip, otherwise after the strip's last panel.
/// `row_last` is the index of the strip's rightmost panel, if any.
/// A click that jumps this many panels or more, landing on an end of the strip,
/// is the work `super-home`/`super-end` exists for. Shorter hops are ordinary
/// left/right navigation.
const LONG_HOP: usize = 3;

/// The chord that closes the focused panel. Cmd/Super+Q everywhere; macOS also
/// accepts Cmd+W, and quitting the app moved to Cmd+Shift+Q.
#[cfg(test)]
const CLOSE_PANEL_CHORD: &str = "super-q";

/// Normalizes a chord's platform modifier so advertised catalog chords compare
/// equal to what GPUI reports on the running platform. GPUI prints the platform
/// modifier as "cmd-" on macOS and "super-" on Linux for the same binding.
#[cfg(test)]
fn platform_chord(chord: &str) -> String {
    chord.replace("cmd-", "super-").replace("win-", "super-")
}

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

fn smoothed_camera_position(from: f32, target: f32, elapsed: Duration, duration: Duration) -> f32 {
    if elapsed >= duration {
        return target;
    }
    let t = elapsed.as_secs_f32() / duration.as_secs_f32();
    from + (target - from) * ease_out_cubic(t)
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

/// Opacity of the gesture reticle a given time after the last touchpad delta:
/// fully lit through the hold window, a linear fade after, `None` once gone.
fn gesture_alpha(since_last_delta: Duration) -> Option<f32> {
    if since_last_delta <= GESTURE_HOLD {
        return Some(1.0);
    }
    let fading = since_last_delta - GESTURE_HOLD;
    if fading >= GESTURE_FADE {
        return None;
    }
    Some(1.0 - fading.as_secs_f32() / GESTURE_FADE.as_secs_f32())
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

    #[test]
    fn sidebar_never_adds_a_workspace_top_inset() {
        assert_eq!(content_top_inset(true, false), 0.0);
        assert_eq!(content_top_inset(true, true), 0.0);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn sidebar_header_keeps_its_linux_alignment() {
        assert_eq!(sidebar_header_left_padding(false), 12.0);
        assert_eq!(sidebar_header_left_padding(true), 12.0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn sidebar_header_clears_macos_traffic_lights() {
        assert_eq!(
            sidebar_header_left_padding(false),
            MACOS_TRAFFIC_LIGHTS_WIDTH
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn fullscreen_hides_traffic_lights_so_the_header_realigns() {
        assert_eq!(sidebar_header_left_padding(true), 12.0);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn hidden_sidebar_does_not_leave_a_top_gap_off_macos() {
        assert_eq!(content_top_inset(false, false), 0.0);
        assert_eq!(content_top_inset(false, true), 0.0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn hidden_sidebar_preserves_room_for_macos_traffic_lights() {
        assert_eq!(content_top_inset(false, false), TITLEBAR_HEIGHT);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn fullscreen_reclaims_the_titlebar_strip() {
        assert_eq!(content_top_inset(false, true), 0.0);
    }

    /// Opt-in micro-profiler for the complete keymap -> workspace state path.
    ///
    /// Run with:
    /// `cargo test --release -p jcode-desktop-ui interaction_latency_profile -- --ignored --nocapture`
    ///
    /// This deliberately does not call `run_until_parked` inside the measured
    /// section. The resulting number is input-to-state latency, while visual
    /// settling time is the transition policy duration reported alongside it.
    #[gpui::test]
    #[ignore = "manual latency profiler"]
    fn interaction_latency_profile(cx: &mut gpui::TestAppContext) {
        const WARMUP: usize = 50;
        const SAMPLES: usize = 500;

        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for row in 0..STRIP_COUNT {
                workspace.active_row = row;
                for column in 0..8 {
                    workspace.push_test_panel(&format!("{row}-{column}"), cx);
                }
            }
            workspace.active_row = 0;
            workspace.active = 3;
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            window.focus(&workspace.read(cx).focus_handle.clone(), cx);
        });
        vcx.run_until_parked();

        // Prove every timed key pair mutates the intended state before trusting
        // its distribution. This prevents a broken binding or edge no-op from
        // looking implausibly fast and being reported as a latency improvement.
        let initial_active = workspace.read_with(vcx, |workspace, _| workspace.active);
        vcx.simulate_keystrokes("super-h");
        assert_ne!(
            workspace.read_with(vcx, |workspace, _| workspace.active),
            initial_active
        );
        vcx.simulate_keystrokes("super-l");
        assert_eq!(
            workspace.read_with(vcx, |workspace, _| workspace.active),
            initial_active
        );

        vcx.simulate_keystrokes("super-j");
        assert_eq!(
            workspace.read_with(vcx, |workspace, _| workspace.active_row),
            1
        );
        vcx.simulate_keystrokes("super-k");
        assert_eq!(
            workspace.read_with(vcx, |workspace, _| workspace.active_row),
            0
        );

        let initial_order = workspace.read_with(vcx, |workspace, _| {
            workspace
                .slots
                .iter()
                .map(|slot| slot.panel.entity_id())
                .collect::<Vec<_>>()
        });
        vcx.simulate_keystrokes("super-shift-h");
        assert_ne!(
            workspace.read_with(vcx, |workspace, _| workspace
                .slots
                .iter()
                .map(|slot| slot.panel.entity_id())
                .collect::<Vec<_>>()),
            initial_order
        );
        vcx.simulate_keystrokes("super-shift-l");
        assert_eq!(
            workspace.read_with(vcx, |workspace, _| workspace
                .slots
                .iter()
                .map(|slot| slot.panel.entity_id())
                .collect::<Vec<_>>()),
            initial_order
        );

        vcx.simulate_keystrokes("super-1");
        assert_eq!(
            workspace.read_with(vcx, |workspace, _| workspace.slots[workspace.active]
                .width_fraction),
            0.25
        );
        vcx.simulate_keystrokes("super-2");
        assert_eq!(
            workspace.read_with(vcx, |workspace, _| workspace.slots[workspace.active]
                .width_fraction),
            0.5
        );

        fn measure(
            cx: &mut gpui::VisualTestContext,
            first: &str,
            second: &str,
        ) -> (u128, u128, u128, u128) {
            let mut samples = Vec::with_capacity(SAMPLES);
            for iteration in 0..WARMUP + SAMPLES {
                let key = if iteration % 2 == 0 { first } else { second };
                let started = Instant::now();
                cx.simulate_keystrokes(key);
                let elapsed = started.elapsed().as_nanos();
                if iteration >= WARMUP {
                    samples.push(elapsed);
                }
            }
            samples.sort_unstable();
            let percentile = |numerator: usize| samples[(samples.len() - 1) * numerator / 100];
            let mean = samples.iter().sum::<u128>() / samples.len() as u128;
            (mean, percentile(50), percentile(95), percentile(99))
        }

        for (name, first, second, transition) in [
            ("focus_horizontal", "super-h", "super-l", Transition::Focus),
            ("focus_vertical", "super-j", "super-k", Transition::Row),
            (
                "move_horizontal",
                "super-shift-h",
                "super-shift-l",
                Transition::PanelOrder,
            ),
            ("resize", "super-1", "super-2", Transition::PanelWidth),
        ] {
            let (mean, p50, p95, p99) = measure(vcx, first, second);
            println!(
                "LATENCY {name:>16} mean={:.1}us p50={:.1}us p95={:.1}us p99={:.1}us settle={}ms",
                mean as f64 / 1_000.0,
                p50 as f64 / 1_000.0,
                p95 as f64 / 1_000.0,
                p99 as f64 / 1_000.0,
                transition::policy(transition).duration.as_millis(),
            );
        }
    }

    /// Loaded key-to-first-render profiler. Unlike `interaction_latency_profile`,
    /// this lets GPUI construct the next frame for 32 live panel entities before
    /// stopping the clock. Presentation cadence is measured by the opt-in runtime
    /// profile because the test platform intentionally has no compositor.
    #[gpui::test]
    #[ignore = "manual loaded animation profiler"]
    fn loaded_animation_first_frame_profile(cx: &mut gpui::TestAppContext) {
        const WARMUP: usize = 10;
        const SAMPLES: usize = 100;

        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for row in 0..STRIP_COUNT {
                workspace.active_row = row;
                for column in 0..8 {
                    workspace.push_test_panel(&format!("loaded-{row}-{column}"), cx);
                }
            }
            workspace.active_row = 0;
            workspace.active = 3;
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| window.focus(&workspace.read(cx).focus_handle.clone(), cx));
        vcx.run_until_parked();

        let mut samples = Vec::with_capacity(SAMPLES);
        for iteration in 0..WARMUP + SAMPLES {
            let key = if iteration % 2 == 0 {
                "super-h"
            } else {
                "super-l"
            };
            let started = Instant::now();
            vcx.simulate_keystrokes(key);
            vcx.run_until_parked();
            if iteration >= WARMUP {
                samples.push(started.elapsed().as_nanos());
            }
        }
        samples.sort_unstable();
        let percentile = |value: usize| samples[(samples.len() - 1) * value / 100];
        println!(
            "LOADED_ANIMATION first_frame p50={:.1}us p95={:.1}us p99={:.1}us panels=32",
            percentile(50) as f64 / 1_000.0,
            percentile(95) as f64 / 1_000.0,
            percentile(99) as f64 / 1_000.0,
        );
        assert_eq!(workspace.read_with(vcx, |workspace, _| workspace.active), 3);
    }

    #[test]
    fn sidebar_free_launch_flags_hide_the_sidebar() {
        assert!(sidebar_enabled(["jcode-desktop"], true));
        assert!(!sidebar_enabled(["jcode-desktop"], false));
        assert!(!sidebar_enabled(["jcode-desktop", "--no-sidebar"], true));
        assert!(!sidebar_enabled(["jcode-desktop", "--workspace"], true));
    }

    #[test]
    fn workspace_snapshot_round_trips_layout_focus_drafts_scroll_and_terminal_id() {
        let draft = PromptInputSnapshot {
            content: "unfinished prompt".into(),
            selection_start: 3,
            selection_end: 9,
            selection_reversed: true,
            history: vec!["older prompt".into()],
            history_index: None,
            live_draft: "unfinished prompt".into(),
            attachments: vec![crate::input::AttachmentSnapshot {
                media_type: "image/png".into(),
                encoded: "aW1hZ2U=".into(),
                label: "diagram.png".into(),
            }],
        };
        let snapshot = WorkspaceSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            slots: vec![SlotSnapshot {
                panel: PanelSnapshot {
                    session_id: "terminal".into(),
                    title: "build shell".into(),
                    working_dir: Some("/workspace".into()),
                    draft,
                    scroll_x: -4.0,
                    scroll_y: -128.5,
                    stick_to_bottom: false,
                    terminal_resource_id: Some(42),
                },
                row: 2,
                width_fraction: 0.75,
                restore_fraction: Some(0.5),
            }],
            active: 0,
            active_row: 2,
            row_focus: [None, None, Some(0), None],
            previous: Some(0),
            camera_x: [0.0, 10.0, 20.0, 30.0],
            camera_target: [1.0, 11.0, 21.0, 31.0],
            overview: true,
            hints_overlay: true,
            folder_picker_dir: Some(PathBuf::from("/workspace/src")),
            folder_picker_error: Some("example".into()),
            folder_search: None,
            focus: FocusSnapshot::Panel(0),
        };

        let encoded = snapshot.encode().expect("encode workspace snapshot");
        let decoded = WorkspaceSnapshot::decode(&encoded).expect("decode workspace snapshot");
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn workspace_snapshot_rejects_invalid_layout_before_root_replacement() {
        let invalid = serde_json::json!({
            "format_version": SNAPSHOT_FORMAT_VERSION,
            "slots": [],
            "active": 0,
            "active_row": STRIP_COUNT,
            "row_focus": [null, null, null, null],
            "previous": null,
            "camera_x": [0.0, 0.0, 0.0, 0.0],
            "camera_target": [0.0, 0.0, 0.0, 0.0],
            "overview": false,
            "hints_overlay": false,
            "folder_picker_dir": null,
            "folder_picker_error": null,
            "folder_search": null,
            "focus": "Workspace"
        });
        let error = WorkspaceSnapshot::decode(&serde_json::to_vec(&invalid).unwrap())
            .expect_err("out-of-range row must be rejected");
        assert!(error.to_string().contains("active row"));
    }

    fn session_info(id: &str, title: Option<&str>) -> jcode_sdk::SessionInfo {
        jcode_sdk::SessionInfo {
            session_id: id.into(),
            working_dir: None,
            title: title.map(str::to_owned),
            status: "idle".into(),
            transcript_bytes: None,
            saved: false,
            updated_at_ms: None,
            last_active_at_ms: None,
            archived: false,
            archived_at_ms: None,
        }
    }

    #[test]
    fn sidebar_matches_tui_saved_then_recency_order() {
        let mut old = session_info("old", None);
        old.updated_at_ms = Some(10);
        let mut recent = session_info("recent", None);
        recent.updated_at_ms = Some(30);
        let mut saved_old = session_info("saved-old", None);
        saved_old.saved = true;
        saved_old.updated_at_ms = Some(5);
        let mut saved_recent = session_info("saved-recent", None);
        saved_recent.saved = true;
        saved_recent.updated_at_ms = Some(20);

        let ids = sidebar_session_order(&[old, saved_old, recent, saved_recent])
            .into_iter()
            .map(|session| session.session_id)
            .collect::<Vec<_>>();

        assert_eq!(ids, ["saved-recent", "saved-old", "recent", "old"]);
    }

    #[test]
    fn sidebar_prefers_the_sdk_title_and_uses_the_session_animal_icon() {
        let session = session_info(
            "session_tigress_1234567890_deadbeef",
            Some("  Release planning  "),
        );
        assert_eq!(
            sidebar_session_title(&session),
            ("🐅", "Release planning".into())
        );
    }

    #[gpui::test]
    fn identical_sdk_session_snapshots_do_not_invalidate_the_sidebar(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, cx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        let sessions = vec![session_info(
            "session_fox_1234567890_deadbeef",
            Some("Release planning"),
        )];

        workspace.update(cx, |workspace, cx| {
            assert!(workspace.apply(
                Update::Sessions {
                    sessions: sessions.clone(),
                },
                cx,
            ));
            assert!(!workspace.apply(Update::Sessions { sessions }, cx));
        });
    }

    #[gpui::test]
    fn session_catalog_refresh_keeps_a_restored_open_session_in_the_sidebar(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.apply_snapshot(
                WorkspaceSnapshot {
                    format_version: SNAPSHOT_FORMAT_VERSION,
                    slots: vec![SlotSnapshot {
                        panel: PanelSnapshot {
                            session_id: "session_fox_1234567890000_deadbeef".into(),
                            title: "Still running".into(),
                            working_dir: Some("/home/example/project".into()),
                            draft: PromptInputSnapshot {
                                content: String::new(),
                                selection_start: 0,
                                selection_end: 0,
                                selection_reversed: false,
                                history: Vec::new(),
                                history_index: None,
                                live_draft: String::new(),
                                attachments: Vec::new(),
                            },
                            scroll_x: 0.0,
                            scroll_y: 0.0,
                            stick_to_bottom: true,
                            terminal_resource_id: None,
                        },
                        row: 0,
                        width_fraction: 1.0,
                        restore_fraction: None,
                    }],
                    active: 0,
                    active_row: 0,
                    row_focus: [Some(0), None, None, None],
                    previous: None,
                    camera_x: [0.0; STRIP_COUNT],
                    camera_target: [0.0; STRIP_COUNT],
                    overview: false,
                    hints_overlay: false,
                    folder_picker_dir: None,
                    folder_picker_error: None,
                    folder_search: None,
                    focus: FocusSnapshot::Workspace,
                },
                cx,
            );
            assert!(workspace.apply(Update::Sessions { sessions: vec![] }, cx));

            assert_eq!(workspace.sessions.len(), 1);
            let session = &workspace.sessions[0];
            assert_eq!(session.session_id, "session_fox_1234567890000_deadbeef");
            assert_eq!(session.title.as_deref(), Some("Still running"));
            assert_eq!(
                session.working_dir.as_deref(),
                Some("/home/example/project")
            );
            assert_eq!(session.status, "active");
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("sidebar-session-0").is_some(),
            "the snapshot-restored running session must paint a sidebar row"
        );

        workspace.update(vcx, |workspace, cx| {
            let mut canonical = session_info(
                "session_fox_1234567890000_deadbeef",
                Some("Canonical runtime title"),
            );
            canonical.saved = true;
            canonical.updated_at_ms = Some(1_787_695_000_000);
            assert!(workspace.apply(
                Update::Sessions {
                    sessions: vec![canonical],
                },
                cx,
            ));
            assert_eq!(
                workspace.sessions.len(),
                1,
                "the catalog must not duplicate it"
            );
            assert_eq!(
                workspace.sessions[0].title.as_deref(),
                Some("Canonical runtime title"),
                "canonical metadata must replace the restored placeholder"
            );
            assert!(workspace.sessions[0].saved);
        });
    }

    #[gpui::test]
    fn live_sdk_events_advance_sidebar_recency(cx: &mut gpui::TestAppContext) {
        let (workspace, cx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        let mut session = session_info("session_fox_1234567890000_deadbeef", None);
        session.updated_at_ms = Some(1);

        workspace.update(cx, |workspace, cx| {
            workspace.apply(
                Update::Sessions {
                    sessions: vec![session],
                },
                cx,
            );
            workspace.apply(
                Update::Event {
                    session_id: "session_fox_1234567890000_deadbeef".into(),
                    event: jcode_sdk::ApiEvent::TextDelta {
                        session_id: "session_fox_1234567890000_deadbeef".into(),
                        text: "working".into(),
                    },
                },
                cx,
            );
            assert!(
                workspace.sessions[0]
                    .updated_at_ms
                    .is_some_and(|value| value > 1)
            );
        });
    }

    #[test]
    fn sidebar_status_distinguishes_crashes_and_live_work() {
        assert_eq!(
            sidebar_session_status("crashed", None),
            ("💥", "crashed", SidebarStatusKind::Bad)
        );
        assert_eq!(
            sidebar_session_status("closed", Some("generating")),
            ("●", "working", SidebarStatusKind::Busy)
        );
        assert_eq!(
            sidebar_session_status("closed", Some("idle")),
            ("●", "ready", SidebarStatusKind::Good)
        );
    }

    /// The sidebar row's metadata line mirrors the TUI `/resume` picker:
    /// relative creation time plus an estimated token count.
    #[test]
    fn sidebar_meta_shows_relative_age_and_estimated_tokens() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let mut session = session_info(
            &format!("session_fox_{}_deadbeef", now_ms - 5 * 60 * 1_000),
            None,
        );
        session.transcript_bytes = Some(48_000);
        let meta = sidebar_session_meta(&session).expect("meta line");
        assert_eq!(meta, "5m ago · ~12.0k tok");
    }

    #[test]
    fn sidebar_meta_is_omitted_when_no_metadata_exists() {
        assert_eq!(sidebar_session_meta(&session_info("legacy-id", None)), None);
    }

    #[test]
    fn sidebar_time_ago_matches_the_tui_picker_buckets() {
        let now = 1_800_000_000_000_u64;
        let minute = 60 * 1_000;
        assert_eq!(format_time_ago(now - 30 * 1_000, now), "30s ago");
        assert_eq!(format_time_ago(now - 12 * minute, now), "12m ago");
        assert_eq!(format_time_ago(now - 3 * 60 * minute, now), "3h ago");
        assert_eq!(format_time_ago(now - 2 * 24 * 60 * minute, now), "2d ago");
        assert_eq!(format_time_ago(now - 10 * 24 * 60 * minute, now), "1w ago");
        assert_eq!(format_time_ago(now - 65 * 24 * 60 * minute, now), "2mo ago");
    }

    #[test]
    fn sidebar_token_estimate_uses_tui_style_units() {
        assert_eq!(format_estimated_tokens(500), "~500 tok");
        assert_eq!(format_estimated_tokens(4_200), "~4.2k tok");
        assert_eq!(format_estimated_tokens(250_000), "~250k tok");
        assert_eq!(format_estimated_tokens(1_500_000), "~1.5M tok");
    }

    #[test]
    fn untitled_sidebar_session_falls_back_to_its_memorable_animal() {
        let session = session_info("session_fox_1234567890_deadbeef", Some("  "));
        assert_eq!(sidebar_session_title(&session), ("🦊", "fox".into()));
    }

    #[test]
    fn missing_sidebar_directory_is_omitted_instead_of_rendering_a_placeholder() {
        let mut session = session_info("session_fox_1234567890_deadbeef", None);
        assert_eq!(sidebar_session_directory(&session), None);

        session.working_dir = Some("   ".into());
        assert_eq!(sidebar_session_directory(&session), None);
    }

    #[gpui::test]
    fn persisted_session_record_paints_as_a_sidebar_row(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let home = std::env::temp_dir().join(format!(
            "jcode-desktop-sidebar-ui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        std::fs::write(
            home.join("sessions/session_fox_disk.json"),
            r#"{"working_dir":"/persisted/project","custom_title":"Persisted work"}"#,
        )
        .unwrap();
        let sessions = crate::harness::merge_persisted_sessions(Vec::new(), Some(&home));
        assert_eq!(
            sessions.len(),
            1,
            "the disk record must cross the harness boundary"
        );

        let (workspace, vcx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.apply(Update::Sessions { sessions }, cx);
            cx.notify();
        });
        vcx.run_until_parked();

        assert!(
            vcx.debug_bounds("sidebar-session-0").is_some(),
            "an on-disk session must paint through the real sidebar renderer"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[gpui::test]
    fn sidebar_divides_saved_sessions_from_other_sessions(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            let mut saved = session_info("session_fox_saved", Some("saved work"));
            saved.saved = true;
            workspace.apply(
                Update::Sessions {
                    sessions: vec![
                        saved,
                        session_info("session_owl_previous", Some("previous work")),
                    ],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();

        assert!(
            vcx.debug_bounds("sidebar-session-divider").is_some(),
            "a horizontal rule should separate saved sessions from other sessions"
        );
    }

    #[gpui::test]
    fn sidebar_separates_open_panels_from_other_sessions(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.push_test_panel("session_fox_open", cx);
            workspace.apply(
                Update::Sessions {
                    sessions: vec![
                        session_info("session_owl_closed", Some("previous work")),
                        session_info("session_fox_open", Some("visible work")),
                    ],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();

        assert!(
            vcx.debug_bounds("sidebar-open-panels-heading").is_some(),
            "sessions represented by a visible panel should have their own section"
        );
        assert!(
            vcx.debug_bounds("sidebar-other-sessions-heading").is_some(),
            "sessions without a panel should have their own section"
        );
    }

    #[gpui::test]
    fn sidebar_session_history_moves_when_the_user_scrolls(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.apply(
                Update::Sessions {
                    sessions: (0..80)
                        .map(|index| {
                            session_info(
                                Box::leak(format!("session_fox_history_{index}").into_boxed_str()),
                                Some("previous work"),
                            )
                        })
                        .collect(),
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();

        let before = workspace.read_with(vcx, |workspace, _| workspace.sidebar_scroll.offset().y);
        let list = vcx
            .debug_bounds("sidebar-session-list")
            .expect("session list should paint");
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: list.center(),
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -4.0)),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();

        let after = workspace.read_with(vcx, |workspace, _| workspace.sidebar_scroll.offset().y);
        assert_ne!(after, before, "wheel input must move the session history");
        assert!(
            vcx.debug_bounds("sidebar-scrollbar").is_some(),
            "overflowing session history should paint a scrollbar"
        );
    }

    #[gpui::test]
    fn open_sidebar_sessions_follow_strips_top_to_bottom(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.active_row = 1;
            workspace.push_test_panel("session_fox_lower", cx);
            workspace.active_row = 0;
            workspace.push_test_panel("session_owl_upper_left", cx);
            workspace.push_test_panel("session_hare_upper_right", cx);
            workspace.apply(
                Update::Sessions {
                    sessions: vec![
                        session_info("session_fox_lower", Some("lower")),
                        session_info("session_hare_upper_right", Some("upper right")),
                        session_info("session_owl_upper_left", Some("upper left")),
                    ],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();

        for (selector, expected_session) in [
            ("sidebar-session-0", "session_owl_upper_left"),
            ("sidebar-session-1", "session_hare_upper_right"),
            ("sidebar-session-2", "session_fox_lower"),
        ] {
            let row = vcx
                .debug_bounds(selector)
                .expect("open session row should paint");
            vcx.simulate_click(row.center(), gpui::Modifiers::default());
            workspace.update(vcx, |workspace, cx| {
                assert_eq!(
                    workspace.slots[workspace.active].panel.read(cx).session_id,
                    expected_session
                );
            });
        }
    }

    #[gpui::test]
    fn sidebar_todos_button_spawns_a_persistent_movable_panel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(crate::learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.push_test_panel("chat", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        let button = vcx
            .debug_bounds("sidebar-unfinished-work")
            .expect("todos launcher paints in the sidebar");
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        assert!(
            vcx.debug_bounds("unfinished-work-list").is_some(),
            "clicking todos should spawn the dedicated panel"
        );

        vcx.simulate_keystrokes("super-h");
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _cx| {
            assert_eq!(
                workspace.active, 0,
                "focus-left should leave the focused todos panel"
            );
        });

        vcx.simulate_keystrokes("super-l");
        vcx.run_until_parked();

        // The todos view is a normal workspace slot, not an overlay. It must
        // therefore follow the same ordering and strip movement commands as a
        // chat panel and be included in the workspace snapshot.
        vcx.simulate_keystrokes("super-shift-h super-shift-j");
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, cx| {
            assert_eq!(workspace.slots.len(), 2);
            assert_eq!(
                workspace.slots[0].panel.read(cx).session_id,
                "unfinished-work"
            );
            assert_eq!(workspace.slots[0].row, 1);
            let panel_snapshot = workspace.slots[0].panel.read(cx).snapshot(cx);
            assert_eq!(panel_snapshot.session_id, "unfinished-work");
        });
    }

    #[gpui::test]
    fn clicking_an_unfinished_work_card_opens_its_chat_session(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(crate::learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.push_test_panel("current-chat", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        let button = vcx
            .debug_bounds("sidebar-unfinished-work")
            .expect("todos launcher paints in the sidebar");
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        workspace.update(vcx, |workspace, cx| {
            let panel = workspace.slots[workspace.active].panel.clone();
            panel.update(cx, |panel, cx| {
                panel.set_unfinished_work(
                    vec![crate::harness::UnfinishedSession {
                        session_id: "todo-chat".into(),
                        title: "Todo chat".into(),
                        working_dir: None,
                        todos: vec![crate::harness::UnfinishedTodo {
                            content: "Finish this work".into(),
                            status: "pending".into(),
                            group: None,
                        }],
                    }],
                    cx,
                );
            });
        });
        vcx.run_until_parked();

        workspace.update(vcx, |workspace, _cx| {
            workspace
                .sessions
                .push(session_info("todo-chat", Some("Todo chat")));
            assert!(
                workspace
                    .sessions
                    .iter()
                    .any(|session| session.session_id == "todo-chat")
            );
        });

        let card = vcx
            .debug_bounds("unfinished-session-0")
            .expect("unfinished session card paints");
        vcx.simulate_click(card.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        workspace.read_with(vcx, |workspace, cx| {
            assert!(
                workspace
                    .slots
                    .iter()
                    .any(|slot| slot.panel.read(cx).session_id == "todo-chat"),
                "clicking the card should open the chat panel"
            );
            assert_eq!(
                workspace.slots[workspace.active].panel.read(cx).session_id,
                "todo-chat"
            );
        });
    }

    #[gpui::test]
    fn clicking_a_file_opens_it_in_a_new_panel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let project = std::env::temp_dir().join(format!(
            "jcode-desktop-files-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("main.rs"), "fn main() {}\n").unwrap();

        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(crate::learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.push_test_panel("project", cx);
            workspace.slots[0].panel.update(cx, |panel, _| {
                panel.working_dir = Some(project.display().to_string());
            });
            cx.notify();
        });
        vcx.run_until_parked();

        let files = vcx
            .debug_bounds("sidebar-files-tab")
            .expect("files tab paints in the sidebar");
        vcx.simulate_click(files.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let source = vcx
            .debug_bounds("file-tree-main.rs")
            .expect("source file paints in the file tree");
        vcx.simulate_click(source.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        assert!(
            vcx.debug_bounds("code-file-contents").is_some(),
            "clicking a source file should open its read-only panel"
        );
        workspace.read_with(vcx, |workspace, cx| {
            assert_eq!(workspace.slots.len(), 2);
            assert!(
                workspace.slots[1]
                    .panel
                    .read(cx)
                    .session_id
                    .ends_with("/main.rs")
            );
        });
        std::fs::remove_dir_all(project).unwrap();
    }

    /// Clicking a sidebar row must open and activate that session through the
    /// real painted element, not merely paint a row that looks clickable.
    #[gpui::test]
    fn clicking_a_sidebar_row_activates_that_session(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |workspace, cx| {
            workspace.apply(
                Update::Sessions {
                    sessions: vec![
                        session_info("session_fox_1234567890_deadbeef", Some("fox work")),
                        session_info("session_owl_1234567890_deadbeef", Some("owl work")),
                    ],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();

        let bounds = vcx
            .debug_bounds("sidebar-session-1")
            .expect("the sidebar must paint a second session row");
        vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        workspace.update(vcx, |workspace, cx| {
            let active = workspace
                .slots
                .get(workspace.active)
                .expect("clicking a row must open a panel")
                .panel
                .read(cx)
                .session_id
                .clone();
            assert_eq!(
                active, "session_owl_1234567890_deadbeef",
                "clicking the row must activate the session it displays"
            );
        });
    }

    /// The accounts strip must actually paint both a regular provider logo and
    /// Jcode's donut. Seeding bypasses the CLI so the test needs no runtime or
    /// credentials.
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
                    limits: vec![
                        accounts::UsageLimit {
                            name: "5 hour".into(),
                            usage_percent: 25.0,
                            reset_in: Some("2h".into()),
                        },
                        accounts::UsageLimit {
                            name: "Weekly".into(),
                            usage_percent: 80.0,
                            reset_in: Some("4d".into()),
                        },
                        accounts::UsageLimit {
                            name: "Gemini 3 Pro".into(),
                            usage_percent: 35.0,
                            reset_in: Some("1h".into()),
                        },
                    ],
                },
                accounts::Account {
                    id: "jcode".into(),
                    display_name: "Jcode".into(),
                    status: "expired".into(),
                    auth_kind: "API key".into(),
                    method: "API key (`JCODE_API_KEY`)".into(),
                    limits: Vec::new(),
                },
                accounts::Account {
                    id: "antigravity".into(),
                    display_name: "Antigravity".into(),
                    status: "available".into(),
                    auth_kind: "OAuth".into(),
                    method: "OAuth".into(),
                    limits: vec![
                        accounts::UsageLimit {
                            name: "Claude".into(),
                            usage_percent: 10.0,
                            reset_in: Some("5h".into()),
                        },
                        accounts::UsageLimit {
                            name: "Gemini".into(),
                            usage_percent: 10.0,
                            reset_in: Some("5h".into()),
                        },
                    ],
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
        assert!(
            accounts::logo("jcode").is_some(),
            "Jcode Subscription must use the donut logo, not a lettermark"
        );
        let openai_row = vcx
            .debug_bounds("account-openai")
            .expect("the OpenAI account row should have painted");
        let jcode_row = vcx
            .debug_bounds("account-jcode")
            .expect("the Jcode account row and donut should have painted");
        let first_limit = vcx
            .debug_bounds("account-openai-limit-0")
            .expect("the first usage limit should paint");
        let second_limit = vcx
            .debug_bounds("account-openai-limit-1")
            .expect("the second usage limit should paint");
        assert!(
            vcx.debug_bounds("account-openai-limit-2").is_none(),
            "additional quotas should be hidden to keep the account compact"
        );
        assert!(
            vcx.debug_bounds("account-jcode-limit-unavailable")
                .is_none(),
            "missing quota data should not add a second line"
        );
        assert!(
            vcx.debug_bounds("account-antigravity-limit-0").is_some(),
            "Antigravity should show one representative quota"
        );
        assert!(
            vcx.debug_bounds("account-antigravity-limit-1").is_none(),
            "Antigravity's duplicate per-model quotas should be hidden"
        );
        assert_eq!(
            first_limit.origin.y, second_limit.origin.y,
            "usage limits should share one compact horizontal row"
        );
        assert!(
            first_limit.origin.x < second_limit.origin.x,
            "usage limits should occupy separate horizontal columns"
        );
        assert_eq!(
            first_limit.size.width, second_limit.size.width,
            "each quota column should retain a readable width"
        );
        assert!(
            openai_row.origin.y < jcode_row.origin.y,
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

        let root = std::env::temp_dir().join(format!(
            "jcode-picker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let selected = root.join("alpha");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::create_dir(root.join("beta")).unwrap();
        std::fs::write(root.join("not-a-directory.txt"), "ignored").unwrap();

        workspace.update(vcx, |workspace, cx| {
            workspace.folder_picker_dir = Some(root.clone());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("folder-picker-parent").is_some(),
            "the parent directory must be available without a search"
        );
        assert!(vcx.debug_bounds("folder-picker-home").is_some());
        assert!(vcx.debug_bounds("folder-picker-computer").is_some());
        vcx.simulate_keystrokes("a l p h a");
        vcx.run_until_parked();
        let first_folder = vcx
            .debug_bounds("folder-picker-entry-0")
            .expect("the first child directory should paint");
        vcx.simulate_click(first_folder.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(
                workspace.folder_picker_dir.as_deref(),
                Some(selected.as_path())
            );
        });

        let cancel = vcx
            .debug_bounds("folder-picker-cancel")
            .expect("cancel should paint");
        vcx.simulate_click(cancel.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert!(workspace.folder_picker_dir.is_none());
        });

        vcx.simulate_keystrokes("ctrl-o");
        workspace.update(vcx, |workspace, cx| {
            workspace.folder_picker_dir = Some(selected.clone());
            cx.notify();
        });
        vcx.run_until_parked();
        let open = vcx
            .debug_bounds("folder-picker-open")
            .expect("open this folder should paint");
        vcx.simulate_click(open.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        match commands
            .try_recv()
            .expect("folder selection should create a session")
        {
            Command::CreateSession { working_dir } => {
                assert_eq!(
                    working_dir.as_deref(),
                    Some(selected.to_string_lossy().as_ref())
                );
            }
            _ => panic!("folder selection sent the wrong runtime command"),
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_browser_lists_only_sorted_directories_and_reports_missing_paths() {
        let root = std::env::temp_dir().join(format!("jcode-picker-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("zeta")).unwrap();
        std::fs::create_dir(root.join("Alpha")).unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        std::fs::write(root.join("file.txt"), "ignored").unwrap();

        let names = directory_entries(&root)
            .unwrap()
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Alpha", "zeta"]);
        assert!(directory_entries(&root.join("missing")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_picker_search_exposes_every_directory_not_only_likely_ones() {
        let root =
            std::env::temp_dir().join(format!("jcode-picker-unrestricted-{}", std::process::id()));
        let ordinary = root.join("an-arbitrary-folder");
        let hidden = root.join(".hidden").join("recent-project");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&ordinary).unwrap();
        std::fs::create_dir_all(&hidden).unwrap();

        let mut hidden_session = session_info("session_fox_hidden", None);
        hidden_session.working_dir = Some(hidden.to_string_lossy().into_owned());
        let entries = ranked_folder_matches_for_sessions(&[hidden_session], &root, "");
        assert!(entries.contains(&(ordinary, String::new())));
        assert!(!entries.iter().any(|(path, _)| path == &hidden));
        assert!(filesystem_root().is_absolute());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn ctrl_o_focuses_search_and_enter_opens_the_matching_folder(cx: &mut gpui::TestAppContext) {
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

        let root = std::env::temp_dir().join(format!("jcode-picker-search-{}", std::process::id()));
        let selected = root.join("alpha-project");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::write(root.join("project-notes.md"), "test").unwrap();

        vcx.simulate_keystrokes("ctrl-o");
        workspace.update(vcx, |workspace, cx| {
            workspace.folder_picker_dir = Some(root.clone());
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("folder-picker-search").is_some());
        vcx.update(|window, cx| {
            let search = workspace.read(cx).folder_search.clone().unwrap();
            assert!(search.read(cx).focus_handle.is_focused(window));

            window.focus(&workspace.read(cx).focus_handle.clone(), cx);
        });
        let search_bounds = vcx.debug_bounds("folder-picker-search").unwrap();
        vcx.simulate_click(search_bounds.center(), gpui::Modifiers::default());
        vcx.update(|window, cx| {
            let search = workspace.read(cx).folder_search.clone().unwrap();
            assert!(search.read(cx).focus_handle.is_focused(window));
        });

        vcx.simulate_keystrokes("n o t e s enter");
        vcx.run_until_parked();
        match commands
            .try_recv()
            .expect("enter should open the search match")
        {
            Command::CreateSession { working_dir } => {
                assert_eq!(
                    working_dir.as_deref(),
                    Some(root.to_string_lossy().as_ref())
                );
            }
            _ => panic!("search submitted the wrong runtime command"),
        }
        workspace.update(vcx, |workspace, _| {
            assert!(workspace.folder_picker_dir.is_none());
            assert!(workspace.folder_search.is_none());
        });
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn picker_starts_with_ranked_likely_recent_and_frequent_directories() {
        let root = std::env::temp_dir().join(format!("jcode-picker-ranked-{}", std::process::id()));
        let projects = root.join("projects");
        let recent = root.join("recent-repo");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&recent).unwrap();
        std::fs::write(root.join("important-notes.md"), "test").unwrap();

        let mut sessions = Vec::new();
        for id in ["one", "two"] {
            let mut session = session_info(id, None);
            session.working_dir = Some(recent.to_string_lossy().into_owned());
            sessions.push(session);
        }
        let workspace = WorkspaceRankFixture { sessions };
        let entries = ranked_folder_matches_for_sessions(&workspace.sessions, &root, "");
        assert_eq!(entries[0], (recent.clone(), "frequent · 2 sessions".into()));
        assert!(entries.contains(&(projects, "likely".into())));

        let searched = ranked_folder_matches_for_sessions(&workspace.sessions, &root, "proj");
        assert!(searched.iter().any(|(path, _)| path.ends_with("projects")));
        let file_match = ranked_folder_matches_for_sessions(&workspace.sessions, &root, "notes");
        assert_eq!(file_match[0].0, root);
        assert!(file_match[0].1.starts_with("contains file"));
        std::fs::remove_dir_all(root).unwrap();
    }

    struct WorkspaceRankFixture {
        sessions: Vec<jcode_sdk::SessionInfo>,
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
    fn touchpad_pan_filters_frame_jitter_then_settles_exactly() {
        let target = 100.0;
        let first_frame =
            smoothed_camera_position(0.0, target, Duration::from_millis(8), TOUCH_PAN_DURATION);
        let second_frame =
            smoothed_camera_position(0.0, target, Duration::from_millis(16), TOUCH_PAN_DURATION);

        assert!(first_frame > 0.0 && first_frame < target);
        assert!(second_frame > first_frame && second_frame < target);
        assert_eq!(
            smoothed_camera_position(0.0, target, TOUCH_PAN_DURATION, TOUCH_PAN_DURATION),
            target,
            "smoothing must never lose touchpad travel"
        );
    }

    /// Public acceptance path: a precise scroll event enters through GPUI and
    /// the rendered panel position advances over frames instead of jumping.
    #[gpui::test]
    fn precise_horizontal_scroll_moves_rendered_panels_smoothly(cx: &mut gpui::TestAppContext) {
        let (_workspace, cx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            for name in ["one", "two", "three"] {
                workspace.push_test_panel(name, cx);
            }
            workspace
        });
        cx.run_until_parked();

        let initial = cx
            .debug_bounds("panel-0")
            .expect("the first panel should paint");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: initial.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-120.), px(0.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Started,
        });

        std::thread::sleep(Duration::from_millis(8));
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| gpui::div(),
        );
        let first_frame = cx
            .debug_bounds("panel-0")
            .expect("the panning panel should remain rendered");
        let first_travel = f32::from(initial.origin.x - first_frame.origin.x);
        assert!(
            first_travel > 0.0 && first_travel < 120.0,
            "the first presented frame should interpolate rather than jump: {first_travel}"
        );

        std::thread::sleep(TOUCH_PAN_DURATION);
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| gpui::div(),
        );
        let settled = cx
            .debug_bounds("panel-0")
            .expect("the settled panel should remain rendered");
        let settled_travel = f32::from(initial.origin.x - settled.origin.x);
        assert!(
            (settled_travel - 120.0).abs() < 0.5,
            "the rendered canvas must settle at the full gesture distance: {settled_travel}"
        );
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

    /// The reticle holds fully lit while deltas keep arriving, fades linearly
    /// once they stop, and disappears entirely after the fade.
    #[test]
    fn the_gesture_reticle_holds_then_fades_then_vanishes() {
        assert_eq!(gesture_alpha(Duration::ZERO), Some(1.0));
        assert_eq!(gesture_alpha(GESTURE_HOLD), Some(1.0));
        let mid =
            gesture_alpha(GESTURE_HOLD + GESTURE_FADE / 2).expect("mid-fade must still be visible");
        assert!(
            (mid - 0.5).abs() < 0.01,
            "halfway through the fade should be about half lit: {mid}"
        );
        assert_eq!(gesture_alpha(GESTURE_HOLD + GESTURE_FADE), None);
        assert_eq!(gesture_alpha(Duration::from_secs(60)), None);
    }

    /// A gesture that starts vertically belongs to the transcript for its
    /// whole lifetime: no pan, no strip hop, even if it later drifts sideways.
    #[test]
    fn a_vertical_first_gesture_stays_with_the_panel() {
        let mut gesture = StripGesture::default();
        assert_eq!(
            gesture.feed(0.0, -40.0, gpui::TouchPhase::Started),
            Routed::ToPanel
        );
        assert_eq!(gesture.axis, GestureAxis::Vertical);
        // Sideways drift after the lock must not start panning.
        assert_eq!(
            gesture.feed(-80.0, -10.0, gpui::TouchPhase::Moved),
            Routed::ToPanel
        );
        assert_eq!(
            gesture.feed(0.0, -300.0, gpui::TouchPhase::Moved),
            Routed::ToPanel
        );
    }

    /// A gesture that starts horizontally owns the strip exclusively, and
    /// enough vertical pull breaks the sticky axis and hops one strip per
    /// threshold, in the natural direction (fingers up reveal the strip
    /// below), without ever leaking a delta into the transcript.
    #[test]
    fn a_horizontal_gesture_breaks_out_vertically_one_strip_per_threshold() {
        let mut gesture = StripGesture::default();
        assert_eq!(
            gesture.feed(-40.0, 0.0, gpui::TouchPhase::Started),
            Routed::Strip {
                dx: -40.0,
                switch: 0,
                exclusive: true
            }
        );
        assert_eq!(gesture.axis, GestureAxis::Horizontal);
        // Pull short of the threshold: consumed, but no hop yet.
        assert_eq!(
            gesture.feed(0.0, -STRIP_BREAK * 0.6, gpui::TouchPhase::Moved),
            Routed::Strip {
                dx: 0.0,
                switch: 0,
                exclusive: true
            }
        );
        // Crossing the threshold hops down and re-arms.
        assert_eq!(
            gesture.feed(0.0, -STRIP_BREAK * 0.6, gpui::TouchPhase::Moved),
            Routed::Strip {
                dx: 0.0,
                switch: 1,
                exclusive: true
            }
        );
        // A second full pull, this time downward fingers, hops back up.
        assert_eq!(
            gesture.feed(0.0, STRIP_BREAK * 1.2, gpui::TouchPhase::Moved),
            Routed::Strip {
                dx: 0.0,
                switch: -1,
                exclusive: true
            }
        );
        // Lifting the fingers ends the gesture: the next one decides afresh.
        gesture.feed(0.0, 0.0, gpui::TouchPhase::Ended);
        assert_eq!(gesture.axis, GestureAxis::Undecided);
        assert_eq!(
            gesture.feed(0.0, -40.0, gpui::TouchPhase::Moved),
            Routed::ToPanel
        );
    }

    /// Tiny diagonal jitter below the lock threshold keeps the event shared:
    /// horizontal motion pans but the transcript still sees the event.
    #[test]
    fn an_undecided_gesture_shares_its_deltas() {
        let mut gesture = StripGesture::default();
        assert_eq!(
            gesture.feed(-4.0, 3.0, gpui::TouchPhase::Started),
            Routed::Strip {
                dx: -4.0,
                switch: 0,
                exclusive: false
            }
        );
        assert_eq!(gesture.axis, GestureAxis::Undecided);
    }

    /// The full breakout on a real rendered workspace: a horizontal swipe
    /// commits the gesture to the strip, continued vertical pull within the
    /// same gesture hops focus to the strip below, and the panel transcript
    /// never scrolls.
    #[gpui::test]
    fn a_committed_pan_breaks_out_vertically_and_switches_strips(cx: &mut gpui::TestAppContext) {
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
        // A long transcript in the first panel, so a leaked vertical delta
        // would visibly scroll it.
        let panel = workspace
            .read_with(cx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");
        panel.update(cx, |panel, cx| {
            for n in 0..80 {
                panel
                    .items
                    .push(crate::panel::Item::Assistant(format!("message {n}")));
            }
            cx.notify();
        });
        cx.run_until_parked();

        let target = cx
            .debug_bounds("panel-0")
            .expect("the first panel should paint")
            .center();
        let swipe = |cx: &mut gpui::VisualTestContext, dx: f32, dy: f32, phase| {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: target,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(dx), px(dy))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: phase,
            });
            cx.run_until_parked();
        };

        // Commit the gesture horizontally, then pull up past the threshold.
        swipe(cx, -40.0, 0.0, gpui::TouchPhase::Started);
        let scroll_before = panel.read_with(cx, |panel, _| panel.test_scroll_offset_y());
        swipe(cx, 0.0, -STRIP_BREAK * 0.6, gpui::TouchPhase::Moved);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.active_row, 0,
                "short of the threshold the sticky axis must hold"
            );
        });
        swipe(cx, 0.0, -STRIP_BREAK * 0.6, gpui::TouchPhase::Moved);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.active_row, 1,
                "enough vertical pull must break the axis and hop down a strip"
            );
        });
        let scroll_after = panel.read_with(cx, |panel, _| panel.test_scroll_offset_y());
        assert_eq!(
            scroll_before, scroll_after,
            "a committed pan must consume its vertical deltas, not scroll the transcript"
        );

        // A fresh vertical gesture, after the reset gap, stays with the
        // panel: no further strip hop.
        workspace.update(cx, |workspace, _| {
            workspace.gesture.reset();
            workspace.gesture_seen = None;
        });
        swipe(cx, 0.0, -STRIP_BREAK * 2.0, gpui::TouchPhase::Started);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.active_row, 1,
                "a vertical-first gesture must never hop strips"
            );
        });
    }

    #[gpui::test]
    fn vertical_scroll_over_an_empty_panel_moves_between_strips(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("empty", cx);
            workspace
        });
        let panel = workspace
            .read_with(cx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");
        panel.update(cx, |panel, cx| {
            panel.items.clear();
            cx.notify();
        });
        cx.run_until_parked();

        let target = cx
            .debug_bounds("panel-0")
            .expect("the empty panel should paint")
            .center();
        for phase in [gpui::TouchPhase::Started, gpui::TouchPhase::Moved] {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: target,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-STRIP_BREAK * 0.6))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: phase,
            });
            cx.run_until_parked();
        }
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.active_row, 1,
                "vertical travel over an empty conversation should navigate strips"
            );
        });
    }

    /// Only the vertical pull during a horizontally locked touchpad gesture
    /// paints the reticle and minimap dot. Horizontal movement alone stays
    /// visually quiet.
    #[gpui::test]
    fn a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot(cx: &mut gpui::TestAppContext) {
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
        assert!(
            cx.debug_bounds("gesture-reticle").is_none(),
            "no reticle before any gesture"
        );

        let panel = cx
            .debug_bounds("panel-0")
            .expect("the first panel should paint");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-120.), px(0.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("gesture-reticle").is_none(),
            "a horizontal swipe must not paint the canvas reticle"
        );
        assert!(
            cx.debug_bounds("minimap-gesture-dot").is_none(),
            "a horizontal swipe must not paint the minimap dot"
        );

        // Once the gesture is horizontally locked, vertical travel pulls
        // toward another strip and should make both indicators visible.
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-20.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("gesture-reticle").is_some(),
            "vertical pull should paint the canvas reticle"
        );
        assert!(
            cx.debug_bounds("minimap-gesture-dot").is_some(),
            "the swipe should paint the minimap dot"
        );

        // The promise of the reticle: the panel under it is the panel that
        // holds focus. Swipe far enough to hand focus to the second panel
        // (but not to the clamped end of the strip, where the focal point
        // sits exactly on a panel boundary) and the ring must sit inside the
        // focused panel's rectangle.
        for _ in 0..4 {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: panel.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(-120.), px(0.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
        }
        cx.run_until_parked();
        let focused = workspace.read_with(cx, |workspace, _| workspace.active);
        assert_eq!(
            focused, 1,
            "a 600px swipe over half-width panels should hand focus to the second panel"
        );
        let reticle = cx
            .debug_bounds("gesture-reticle")
            .expect("the reticle stays lit mid-swipe");
        let focused_panel = cx
            .debug_bounds("panel-1")
            .expect("the focused panel should paint");
        assert!(
            focused_panel.contains(&reticle.center()),
            "the reticle must sit over the panel that holds focus: \
             reticle={reticle:?}, panel={focused_panel:?}"
        );

        workspace.update(cx, |workspace, _| {
            workspace.gesture_last =
                Some(Instant::now() - GESTURE_HOLD - GESTURE_FADE - Duration::from_millis(50));
        });
        cx.run_until_parked();
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| gpui::div(),
        );
        assert!(
            cx.debug_bounds("gesture-reticle").is_none(),
            "the reticle must vanish after the fade"
        );
        assert!(
            cx.debug_bounds("minimap-gesture-dot").is_none(),
            "the minimap dot must vanish after the fade"
        );
    }

    /// Neither ordinary transcript scrolling nor horizontal minimap panning
    /// is a vertical strip pull, so neither should light the indicators.
    #[gpui::test]
    fn transcript_scrolls_and_minimap_swipes_do_not_light_the_indicators(
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

        // A vertical wheel over the panel belongs to the transcript, not the
        // pan gesture: no reticle.
        let panel = cx
            .debug_bounds("panel-0")
            .expect("the first panel should paint");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(60.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("gesture-reticle").is_none(),
            "vertical transcript scrolling must not light the reticle"
        );

        // A swipe over the minimap pans horizontally without a vertical strip
        // pull, so the indicators remain hidden.
        let map = cx
            .debug_bounds("minimap")
            .expect("the minimap should paint");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: map.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-30.), px(0.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("gesture-reticle").is_none(),
            "a minimap swipe must not light the canvas reticle"
        );
        assert!(
            cx.debug_bounds("minimap-gesture-dot").is_none(),
            "a minimap swipe must not light the map dot"
        );
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
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        let mut previous = 0.0;
        for step in 1..=10 {
            let value = ease_out_cubic(step as f32 / 10.0);
            assert!(value > previous, "easing must increase at {step}");
            previous = value;
        }
        // Cubic ease-out keeps the movement responsive while distributing it
        // across substantially more of the 150 ms transition.
        assert!((ease_out_cubic(0.1) - 0.271).abs() < 0.001);
        assert!((ease_out_cubic(0.2) - 0.488).abs() < 0.001);
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
            ("close_panel", CLOSE_PANEL_CHORD),
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
            ("focus_first_last", "super-u", Box::new(FocusFirst)),
            ("focus_first_last", "super-p", Box::new(FocusLast)),
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
            ("close_panel", CLOSE_PANEL_CHORD, Box::new(ClosePanel)),
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
            // GPUI unparses the platform modifier per platform: "super-" on
            // Linux, "cmd-" on macOS. The catalog writes one chord for humans,
            // so compare on a normalized form rather than the literal string.
            let expected_chord = platform_chord(chord);
            assert!(
                bound
                    .iter()
                    .any(|actual| platform_chord(actual) == expected_chord),
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

    /// Hot reload reconstructs each panel and its selectable-text focus model.
    /// Selectable text consumes mouse-down to preserve copy focus, so panel
    /// activation must still happen when the corresponding mouse-up bubbles.
    #[gpui::test]
    fn clicking_selectable_text_in_a_restored_panel_activates_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("one", cx);
            workspace.push_test_panel("restored", cx);
            workspace
                .test_panel(1)
                .expect("second test panel")
                .update(cx, |panel, cx| panel.append_test_error("restored text", cx));
            workspace
        });
        cx.run_until_parked();

        let text = cx
            .debug_bounds("selectable-text-0-error")
            .expect("the restored panel's selectable text should paint");
        cx.simulate_click(text.center(), gpui::Modifiers::default());
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.test_focus_position(),
                Some(1),
                "mouse-up should activate the panel even when text consumed mouse-down"
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
    fn inbox_button_lives_in_the_sidebar_and_opens_gmail(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("one", cx);
            workspace
        });
        vcx.run_until_parked();

        let button = vcx
            .debug_bounds("open-gmail")
            .expect("the Inbox button should have painted");
        assert!(
            f32::from(button.center().x) < SIDEBAR_WIDTH,
            "the Inbox button should be inside the left sidebar"
        );
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        workspace.update(vcx, |workspace, cx| {
            assert_eq!(
                workspace.slots[workspace.active].panel.read(cx).session_id,
                "gmail://inbox"
            );
        });
    }

    #[gpui::test]
    fn right_edge_is_a_full_height_click_target_for_a_new_session(cx: &mut gpui::TestAppContext) {
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

    /// The update chip must actually paint, and only while the updater has
    /// something to say. This is the surface that would have told Bruno a fix
    /// for the blank-text build was already downloading, so it is asserted
    /// through the real render path rather than on state alone.
    #[gpui::test]
    fn the_update_chip_paints_only_while_the_updater_has_something_to_say(
        cx: &mut gpui::TestAppContext,
    ) {
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

        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(1200.), px(800.)),
                |_, _| gpui::div(),
            );
            cx.run_until_parked();
        };

        // A current app says nothing at all.
        updates::set(updates::UpdateState::Idle);
        draw(cx);
        assert!(
            cx.debug_bounds("update-chip").is_none(),
            "a current app should not paint an update chip"
        );

        // A download in flight is visible while it happens.
        updates::set(updates::UpdateState::Available {
            version: "0.1.0-beta.15".to_owned(),
        });
        draw(cx);
        let downloading = cx
            .debug_bounds("update-chip")
            .expect("a download in flight should paint the chip");
        assert!(
            downloading.size.width > px(0.) && downloading.size.height > px(0.),
            "the chip must occupy real space, got {downloading:?}"
        );
        assert!(
            downloading.origin.x >= px(SIDEBAR_WIDTH),
            "the chip should stay inside the workspace instead of spilling into the sidebar"
        );

        // And the staged build keeps offering the restart that applies it.
        updates::set(updates::UpdateState::ReadyToRestart {
            version: "0.1.0-beta.15".to_owned(),
        });
        draw(cx);
        let ready = cx
            .debug_bounds("update-chip")
            .expect("a staged update should keep the chip on screen");
        assert_eq!(
            ready.origin.x + ready.size.width,
            downloading.origin.x + downloading.size.width,
            "the chip should hold its right edge as its label changes"
        );

        // Clicking it must reach the installer. No Sparkle framework is loaded
        // in a test, so the click is a no-op rather than a relaunch, and the
        // chip must survive it.
        cx.simulate_click(ready.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        draw(cx);
        assert!(
            cx.debug_bounds("update-chip").is_some(),
            "clicking the chip must not tear it down"
        );

        updates::set(updates::UpdateState::Idle);
        draw(cx);
        assert!(
            cx.debug_bounds("update-chip").is_none(),
            "the chip should disappear once the updater goes quiet again"
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

    /// The tutorial bug this pins down: on a fresh workspace with one panel,
    /// every other strip is empty, so super-j/super-k never counted as
    /// practiced and the ⌘J/⌘K lesson chips never went away. Pressing the
    /// chord must clear the lesson even when the strip switch is a no-op,
    /// while still granting no mastery and showing no hint. Asserted at the
    /// rendered surface: the chip's completion checkmark must appear.
    #[gpui::test]
    fn pressing_into_empty_strips_clears_the_lesson_without_teaching(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("only", cx);
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("tutorial-learned-focus_up_down").is_none(),
            "the strip lesson should start incomplete"
        );

        // Down onto an empty strip, and up against the top edge: both are
        // no-ops as navigation, both are the user producing the chord.
        vcx.simulate_keystrokes("super-j super-k super-k");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            let coach = workspace.test_coach();
            assert!(
                coach.trace("focus_up_down").practiced(),
                "the lesson should clear once the chord has been pressed"
            );
            assert_eq!(
                coach.mastery("focus_up_down", learning::now()),
                0.0,
                "a no-op press proves the chord, not the navigation"
            );
            assert_eq!(coach.effort_saved, 0);
            assert_eq!(coach.effort_wasted, 0);
            assert_eq!(coach.active_hint_id(), None);
        });
        assert!(
            vcx.debug_bounds("tutorial-learned-focus_up_down").is_some(),
            "the J/K chips should render their green completion state"
        );
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
            assert_eq!(
                (workspace.active_row, workspace.test_focus_position()),
                (1, Some(1))
            );
            assert!(workspace.row_progress.is_animating());
        });
        let outgoing = vcx.debug_bounds("row-transition-outgoing").unwrap();
        let incoming = vcx.debug_bounds("row-transition-incoming").unwrap();
        assert!(
            incoming.origin.y > outgoing.origin.y,
            "down enters from below"
        );

        vcx.simulate_keystrokes("super-h");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("super-k");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(
                (workspace.active_row, workspace.test_focus_position()),
                (0, Some(1))
            );
        });
        let outgoing = vcx.debug_bounds("row-transition-outgoing").unwrap();
        let incoming = vcx.debug_bounds("row-transition-incoming").unwrap();
        assert!(
            incoming.origin.y < outgoing.origin.y,
            "up enters from above"
        );

        vcx.simulate_keystrokes("super-j");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(
                (workspace.active_row, workspace.test_focus_position()),
                (1, Some(0))
            );
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
    fn horizontal_panel_moves_animate_both_swapped_panels(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("left", cx);
            workspace.push_test_panel("right", cx);
            workspace.set_active(1, cx);
            let _ = window;
            workspace
        });
        vcx.update(|window, cx| {
            window.focus(&workspace.read(cx).focus_handle.clone(), cx);
        });
        vcx.run_until_parked();

        vcx.simulate_keystrokes("super-shift-h");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.active, 0);
            assert!(workspace.slots[0].order_offset.is_animating());
            assert!(workspace.slots[1].order_offset.is_animating());
        });

        std::thread::sleep(transition::policy(Transition::PanelOrder).duration * 2);
        workspace.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert!(!workspace.slots[0].order_offset.is_animating());
            assert!(!workspace.slots[1].order_offset.is_animating());
        });

        vcx.simulate_keystrokes("super-shift-l");
        vcx.run_until_parked();
        workspace.update(vcx, |workspace, _| {
            assert_eq!(workspace.active, 1);
            assert!(workspace.slots[0].order_offset.is_animating());
            assert!(workspace.slots[1].order_offset.is_animating());
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
            "super-u / super-p",
            "super-tab",
            "super-o",
            "super-shift-h / super-shift-l",
            "super-shift-j / super-shift-k",
            "super-shift-home / super-shift-end",
            "super-r",
            "super-f",
            "super-1 .. super-4",
            "super-n",
            CLOSE_PANEL_CHORD,
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
        let initial_pin = cx
            .debug_bounds("minimap-you-pin")
            .expect("the focused panel should have a persistent visual pin");
        let initial_panel = cx
            .debug_bounds("minimap-panel-0")
            .expect("the focused panel should appear on the map");
        assert!(
            initial_panel.contains(&initial_pin.center()),
            "the location pin should start inside the focused panel"
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
        let moved_pin = cx
            .debug_bounds("minimap-you-pin")
            .expect("the location pin should remain visible after focus moves");
        let focused_panel = cx
            .debug_bounds("minimap-panel-2")
            .expect("the newly focused panel should remain on the map");
        assert!(
            focused_panel.contains(&moved_pin.center()),
            "the location pin should move into the newly focused panel"
        );
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

    /// The platform spelling of the workspace modifier. macOS binds `cmd`
    /// explicitly because ScrollWM owns the Option-key motions there.
    #[cfg(target_os = "macos")]
    const MOD: &str = "cmd";
    #[cfg(not(target_os = "macos"))]
    const MOD: &str = "super";

    fn focused_workspace(
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::Entity<Workspace>, &mut gpui::VisualTestContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) =
            cx.add_window_view(|_window, cx| Workspace::for_test(learning::Coach::new(), cx));
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        (workspace, cx)
    }

    #[gpui::test]
    fn enter_opens_a_terminal_directly_right_of_the_focused_panel(cx: &mut gpui::TestAppContext) {
        let (workspace, cx) = focused_workspace(cx);

        cx.simulate_keystrokes(&format!("{MOD}-t"));
        cx.run_until_parked();
        cx.simulate_keystrokes(&format!("{MOD}-enter"));
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.slots.len(), 2, "Enter should open a second panel");
            assert!(
                workspace
                    .slots
                    .iter()
                    .all(|slot| slot.panel.read(cx).session_id == "terminal"),
                "both panels should be terminals"
            );
            assert_eq!(
                workspace.active, 1,
                "the new terminal lands right of the focused panel and takes focus"
            );
        });
    }

    #[gpui::test]
    fn semicolon_requests_a_session_panel(cx: &mut gpui::TestAppContext) {
        let (workspace, cx) = focused_workspace(cx);

        cx.simulate_keystrokes(&format!("{MOD}-;"));
        cx.run_until_parked();

        workspace.update(cx, |workspace, _cx| {
            assert!(
                workspace.test_coach().trace("new_panel").recalled > 0,
                "the session shortcut should register as the new_panel action"
            );
        });
    }

    #[gpui::test]
    fn the_sidebar_toggles_and_gives_its_width_back_to_the_panels(cx: &mut gpui::TestAppContext) {
        let (workspace, cx) = focused_workspace(cx);
        let visible_first = workspace.update(cx, |workspace, _| workspace.show_sidebar);

        cx.simulate_keystrokes(&format!("{MOD}-b"));
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.show_sidebar, !visible_first,
                "the shortcut should flip sidebar visibility"
            );
        });

        cx.simulate_keystrokes(&format!("{MOD}-b"));
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.show_sidebar, visible_first,
                "toggling twice should restore the original state"
            );
        });

        // Ctrl+Shift+E is the explorer-style alias and must drive the same
        // action, not merely exist in the keymap.
        cx.simulate_keystrokes("ctrl-shift-e");
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.show_sidebar, !visible_first,
                "the ctrl-shift-e alias should toggle the sidebar too"
            );
        });
    }

    #[gpui::test]
    fn contextual_tutorial_controls_drive_the_actions_they_depict(cx: &mut gpui::TestAppContext) {
        let (workspace, cx) = focused_workspace(cx);
        cx.run_until_parked();

        let new_session = cx.debug_bounds("tutorial-new").expect("new session guide");
        let right_guide = cx
            .debug_bounds("tutorial-nav-right")
            .expect("right navigation guide");
        assert!(
            new_session.origin.y + new_session.size.height <= right_guide.origin.y
                || right_guide.origin.y + right_guide.size.height <= new_session.origin.y,
            "the Super+N and Super+L lessons must not overlap"
        );

        let down_guide = cx
            .debug_bounds("tutorial-nav-down")
            .expect("down navigation guide");
        let tutorial = cx
            .debug_bounds("tutorial-guides")
            .expect("tutorial guide canvas");
        assert!(
            down_guide.origin.y + down_guide.size.height
                <= tutorial.origin.y + tutorial.size.height - px(96.0),
            "the Super+J lesson must leave the bottom input area unobstructed"
        );

        cx.simulate_click(new_session.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert!(
                workspace.test_coach().trace("new_panel").recalled > 0,
                "the + guide should dispatch the same learned action as the shortcut"
            );
        });

        // Terminals appear synchronously, giving the arrow controls two real
        // neighbours without depending on an external session bridge response.
        cx.simulate_keystrokes(&format!("{MOD}-enter {MOD}-enter"));
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.active, 1);
            workspace.set_active(0, cx);
            cx.notify();
        });
        cx.run_until_parked();

        let tutorial = cx
            .debug_bounds("tutorial-guides")
            .expect("tutorial guide canvas");
        let camera_before = workspace.update(cx, |workspace, _| workspace.camera_x[0]);
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: tutorial.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-100.0), px(0.0))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        workspace.update(cx, |workspace, _| {
            assert!(
                workspace.camera_x[0] > camera_before,
                "the tutorial positioning layer must not block touchpad gestures"
            );
        });

        let right = cx
            .debug_bounds("tutorial-nav-right")
            .expect("right arrow guide");
        cx.simulate_click(right.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(workspace.active, 1, "the right arrow should focus right");
        });

        let left = cx
            .debug_bounds("tutorial-nav-left")
            .expect("left arrow guide");
        cx.simulate_click(left.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(workspace.active, 0, "the left arrow should focus left");
        });

        cx.simulate_keystrokes(&format!("{MOD}-l"));
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.active, 1,
                "the shortcut should restore right focus"
            );
        });

        // Put the focused panel on the second strip, then traverse both ways
        // using the rendered tutorial arrows rather than keyboard dispatch.
        cx.simulate_keystrokes(&format!("{MOD}-shift-j"));
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| assert_eq!(workspace.active_row, 1));

        let up = cx.debug_bounds("tutorial-nav-up").expect("up arrow guide");
        cx.simulate_click(up.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.active_row, 0,
                "the up arrow should focus the strip above"
            );
        });

        let down = cx
            .debug_bounds("tutorial-nav-down")
            .expect("down arrow guide");
        cx.simulate_click(down.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.active_row, 1,
                "the down arrow should focus the strip below"
            );
        });
    }

    #[gpui::test]
    fn tutorial_advances_from_basics_to_arrangement_and_marks_learning(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, cx) = focused_workspace(cx);
        cx.run_until_parked();

        assert!(cx.debug_bounds("tutorial-new").is_some());
        assert!(cx.debug_bounds("tutorial-close").is_some());
        assert!(cx.debug_bounds("tutorial-width-presets").is_none());
        assert!(cx.debug_bounds("tutorial-resize").is_none());

        let now = learning::now();
        workspace.update(cx, |workspace, cx| {
            workspace
                .test_coach_mut()
                .used_shortcut("focus_left_right", now);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("tutorial-learned-focus_left_right")
                .is_some(),
            "learned stage controls should expose their green completion state"
        );

        workspace.update(cx, |workspace, cx| {
            for skill in ["focus_up_down", "new_panel", "close_panel"] {
                workspace.test_coach_mut().used_shortcut(skill, now);
            }
            cx.notify();
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("tutorial-new").is_none());
        assert!(cx.debug_bounds("tutorial-close").is_none());
        assert!(cx.debug_bounds("tutorial-width-presets").is_some());
        assert!(cx.debug_bounds("tutorial-nav-left").is_some());
        assert!(cx.debug_bounds("tutorial-resize").is_none());
    }

    #[gpui::test]
    fn onboarding_disappears_after_every_presented_skill_is_practiced(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, cx) = focused_workspace(cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("tutorial-guides").is_some());

        let now = learning::now();
        workspace.update(cx, |workspace, cx| {
            for skill in ONBOARDING_SKILLS {
                workspace.test_coach_mut().used_shortcut(skill, now);
            }
            assert!(workspace.onboarding_complete());
            cx.notify();
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("tutorial-guides").is_none(),
            "completed first-run onboarding should no longer cover the workspace"
        );
    }

    #[gpui::test]
    fn showcase_is_on_by_default_and_only_paints_workspace_motions(cx: &mut gpui::TestAppContext) {
        let (workspace, cx) = focused_workspace(cx);
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| assert!(workspace.showcase_mode));

        workspace.update(cx, |workspace, cx| {
            workspace.showcase_motion("L", false, "Focus right", cx);
            let cue = workspace.showcase_cue.as_ref().expect("showcase cue");
            assert_eq!(cue.action, "Focus right");
            assert_eq!(cue.tutorial_group, "navigate");
            assert_eq!(
                cue.shortcut,
                if cfg!(target_os = "macos") {
                    "Cmd + L"
                } else {
                    "Super + L"
                }
            );
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("showcase-shortcut").is_none(),
            "tutorial motions should animate their contextual controls instead of covering them"
        );
        assert!(
            cx.debug_bounds("tutorial-guides").is_some(),
            "tutorial mode should keep contextual controls visible"
        );
        assert!(
            cx.debug_bounds("tutorial-nav-left").is_some()
                && cx.debug_bounds("tutorial-nav-right").is_some(),
            "navigation should use directional arrow controls"
        );
        assert!(
            cx.debug_bounds("tutorial-new").is_some()
                && cx.debug_bounds("tutorial-close").is_some()
                && cx.debug_bounds("tutorial-resize").is_none(),
            "the first stage should show only navigation and basic session controls"
        );
        assert!(
            cx.debug_bounds("showcase-key").is_some(),
            "the overlay should show the complete keybinding"
        );
        assert!(
            cx.debug_bounds("showcase-action").is_some(),
            "the overlay should explain what the shortcut did"
        );

        // The user reads the action first, then the keybinding: the action
        // text must paint above the keybinding pill.
        let action = cx.debug_bounds("showcase-action").expect("action bounds");
        let key = cx.debug_bounds("showcase-key").expect("keybinding bounds");
        let card = cx
            .debug_bounds("showcase-card")
            .expect("showcase card bounds");
        assert!(
            card.size.width < px(320.0) && card.size.height <= px(80.0),
            "the showcase card should shrink-wrap its content, got {:?}",
            card.size
        );
        assert!(
            action.origin.x - card.origin.x <= px(13.0) && key.origin.x - card.origin.x <= px(13.0),
            "the card border should closely fit the content"
        );
        assert!(
            action.origin.y < key.origin.y,
            "the action should be displayed above the keybinding"
        );

        cx.simulate_keystrokes(&format!("{MOD}-b"));
        workspace.update(cx, |workspace, _| {
            assert_eq!(
                workspace.showcase_cue.as_ref().map(|cue| cue.action),
                Some("Focus right"),
                "an unrelated shortcut must not replace the displayed motion"
            );
        });

        // A real focus-left keystroke, through the actual keymap, must produce
        // the matching cue even when focus cannot move (no panels exist).
        cx.simulate_keystrokes(&format!("{MOD}-h"));
        workspace.update(cx, |workspace, _| {
            let cue = workspace.showcase_cue.as_ref().expect("focus-left cue");
            assert_eq!(cue.action, "Focus left");
            assert_eq!(
                cue.shortcut,
                if cfg!(target_os = "macos") {
                    "Cmd + H"
                } else {
                    "Super + H"
                }
            );
        });

        cx.simulate_keystrokes(&format!("{MOD}-shift-s"));
        workspace.update(cx, |workspace, _| {
            assert!(!workspace.showcase_mode);
            assert!(workspace.showcase_cue.is_none());
        });
    }

    #[gpui::test]
    fn super_t_opens_and_paints_a_plain_terminal_panel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let workspace = Workspace::for_test(learning::Coach::new(), cx);
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });

        cx.simulate_keystrokes("super-t");
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.slots.len(), 1);
            assert_eq!(workspace.slots[0].panel.read(cx).session_id, "terminal");
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| gpui::div(),
        );
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("plain-terminal").is_some(),
            "the terminal surface should paint in the newly created panel"
        );

        // UI-only tests use an inert host. The host resource suite separately
        // creates a real PTY, reattaches a second generation, writes a command,
        // and verifies replayed output without opening a window.
        workspace.update(cx, |workspace, cx| {
            assert!(
                workspace.slots[0]
                    .panel
                    .read(cx)
                    .test_terminal_contents(cx)
                    .is_some(),
                "terminal parser should exist even when the test host has no PTY"
            );
        });

        // Global workspace actions must continue to bubble while the terminal
        // owns keyboard focus rather than being swallowed as shell input.
        cx.simulate_keystrokes("super-o");
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert!(
                workspace.overview,
                "Super+O should still open overview from a terminal"
            );
        });
    }

    #[gpui::test]
    fn super_shift_g_opens_and_paints_the_gmail_inbox_panel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            let workspace = Workspace::for_test(learning::Coach::new(), cx);
            let _ = window;
            workspace
        });
        cx.update(|window, cx| {
            let handle = workspace.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });

        cx.simulate_keystrokes("super-shift-g");

        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.slots.len(), 1);
            assert_eq!(
                workspace.slots[0].panel.read(cx).session_id,
                "gmail://inbox"
            );
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| gpui::div(),
        );
        assert!(
            cx.debug_bounds("gmail-inbox").is_some(),
            "the Gmail inbox surface should paint in the newly created panel"
        );
        // Opening the public shortcut again focuses the existing inbox instead
        // of creating duplicate panels.
        cx.simulate_keystrokes("super-shift-g");
        workspace.update(cx, |workspace, _| {
            assert_eq!(workspace.slots.len(), 1);
        });
    }
}
