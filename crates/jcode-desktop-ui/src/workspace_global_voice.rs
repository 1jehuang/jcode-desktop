//! Global holds belong to the last-focused chat, not to the foreground app.
//! This is UI-owned so reload/close drops the input descriptors and microphone.
//!
//! Edges come from one of two platform sources, both feeding the same
//! dictation-only capture owner:
//! * Linux: the opt-in evdev listener (any display server, X11 or Wayland).
//! * macOS/Windows: the host's registered global shortcut, delivered as the
//!   `workspace::BeginGlobalVoiceHold` / `EndGlobalVoiceHold` actions without
//!   activating the window.
use super::*;
#[cfg(target_os = "linux")]
use crate::global_voice_input::{Edge, Listener};
use crate::global_voice_overlay::{self as overlay, Snapshot, VoiceOverlay};
use gpui::{Task, WeakEntity, WindowHandle};
use std::sync::{Arc, atomic::AtomicBool};

#[path = "workspace_global_voice_fixture.rs"]
mod fixture;

struct Capture {
    panel: WeakEntity<Panel>,
    attempt: Arc<AtomicBool>,
}

/// Host-delivered holds have no kernel-side deadline, so bound them here too.
const HOST_HOLD_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(super) struct State {
    #[cfg(target_os = "linux")]
    listener: Option<Listener>,
    /// Set while a host shortcut hold is down. Used for the safety deadline.
    host_held_since: Option<Instant>,
    task: Option<Task<()>>,
    owner: Option<Capture>,
    overlay: Option<WindowHandle<VoiceOverlay>>,
    finished_at: Option<Instant>,
    permission_task: Option<Task<()>>,
    last_permission_check: Option<Instant>,
    press_serial: u64,
    held: bool,
    pending_target: Option<WeakEntity<Panel>>,
    /// Whether this Jcode window has focus. The chat's own voice pill is the
    /// single source of truth. The OS pill mirrors it only while unfocused.
    window_active: bool,
}

/// The OS-level pill is only for when the chat itself is not in front of you.
/// A focused window already shows the in-panel pill, so never duplicate it.
fn os_pill_wanted(window_active: bool, has_status: bool) -> bool {
    has_status && !window_active
}

/// Whether a host global-shortcut press should use the dictation-only global
/// owner. A focused window keeps its native hold (with intent routing),
/// exactly as the in-app shortcut would, so nothing changes for focused use.
fn host_press_uses_global_owner(window_active: bool, overlay_available: bool) -> bool {
    !window_active && overlay_available
}

impl State {
    #[cfg(target_os = "linux")]
    pub(super) fn ready(&self) -> bool {
        self.listener.as_ref().is_some_and(Listener::ready)
    }

    pub(super) fn set_active(&mut self, active: bool) {
        self.window_active = active;
        #[cfg(target_os = "linux")]
        if let Some(listener) = self.listener.as_mut() {
            listener.set_active(active);
        }
    }

    /// Stop accepting global edges after the OS pill proved impossible, so a
    /// Linux evdev hold returns to native focused-key handling.
    fn disable_capture_source(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.listener = None;
        }
    }

    fn hide_overlay(&mut self, cx: &mut App) {
        if let Some(handle) = self.overlay.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn close_overlay(&mut self, cx: &mut App) {
        self.hide_overlay(cx);
        self.owner = None;
        self.finished_at = None;
    }

    fn shutdown(&mut self, cx: &mut App) {
        self.task = None;
        self.permission_task = None;
        #[cfg(target_os = "linux")]
        {
            self.listener = None;
        }
        self.host_held_since = None;
        if let Some(panel) = self.owner.as_ref().and_then(|owner| owner.panel.upgrade()) {
            panel.update(cx, |panel, cx| {
                panel.cancel_global_voice(&self.owner.as_ref().unwrap().attempt, cx)
            });
        }
        self.close_overlay(cx);
    }
}

impl Workspace {
    pub(super) fn start_global_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.start_global_voice_fixture(window, cx) {
            return;
        }
        #[cfg(target_os = "linux")]
        self.start_global_voice_listener(window, cx);
        #[cfg(not(target_os = "linux"))]
        let _ = (window, cx);
    }

    /// Opt-in evdev capture. Works under both X11 and Wayland because it reads
    /// the kernel, but only where a non-focusing OS pill backend exists.
    #[cfg(target_os = "linux")]
    fn start_global_voice_listener(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Never inspect live input in tests, screenshots or fixture processes.
        if cfg!(test)
            || harness::screenshot_mode()
            || !overlay::available(cx)
            || !crate::config::get().voice.global_hold
            || crate::config::get().voice.global_devices.is_empty()
            || std::env::var("JCODE_DESKTOP_GLOBAL_VOICE").as_deref() == Ok("0")
        {
            return;
        }
        let listener = match Listener::new(&crate::config::get().voice.global_devices) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("global voice unavailable: {error}");
                return;
            }
        };
        eprintln!(
            "global voice: listening on {} configured device(s)",
            crate::config::get().voice.global_devices.len()
        );
        self.global_voice.listener = Some(listener);
        self.global_voice.set_active(window.is_window_active());
        self.ensure_global_voice_task(window, cx);
    }

    /// One UI-owned poll loop per workspace, shared by both edge sources.
    /// Dropping the workspace (reload/close) drops the task and microphone.
    fn ensure_global_voice_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.global_voice.task.is_some() {
            return;
        }
        cx.on_release(|this, cx| this.global_voice.shutdown(cx))
            .detach();
        self.global_voice.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                let result = cx.update(|window, cx| {
                    this.update(cx, |this, cx| this.poll_global_voice(window, cx))
                });
                if !matches!(result, Ok(Ok(()))) {
                    let _ = this.update(cx, |this, cx| this.global_voice.shutdown(cx));
                    break;
                }
            }
        }));
    }

    fn poll_global_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.global_voice.set_active(window.is_window_active());
        #[cfg(target_os = "linux")]
        {
            let eligible = !self.account_sign_in.visible && self.voice_target(cx).is_some();
            if let Some(listener) = self.global_voice.listener.as_mut() {
                listener.set_eligible(eligible);
            }
            let edges = self
                .global_voice
                .listener
                .as_mut()
                .map(Listener::poll)
                .unwrap_or_default();
            for edge in edges {
                match edge {
                    Edge::Press => self.global_voice_press(cx),
                    Edge::Release => self.global_voice_release(cx),
                    Edge::Cancel => {
                        eprintln!("global voice: canceled by input listener");
                        self.cancel_global_voice_capture(cx)
                    }
                }
            }
        }
        if self
            .global_voice
            .host_held_since
            .is_some_and(|since| since.elapsed() >= HOST_HOLD_DEADLINE)
        {
            eprintln!("global voice: host shortcut hold exceeded its deadline");
            self.cancel_global_voice_capture(cx);
        }
        if self.global_voice.owner.is_some()
            && self.global_voice.permission_task.is_none()
            && self
                .global_voice
                .last_permission_check
                .is_none_or(|last| last.elapsed() >= Duration::from_millis(250))
        {
            self.check_global_voice_permission(false, cx);
        }
        self.update_global_voice_overlay(cx);
    }

    fn global_voice_press(&mut self, cx: &mut Context<Self>) {
        eprintln!("global voice: press");
        self.global_voice.pending_target = self
            .voice_target(cx)
            .map(|index| self.slots[index].panel.downgrade());
        self.global_voice.held = true;
        self.global_voice.press_serial = self.global_voice.press_serial.wrapping_add(1);
        self.check_global_voice_permission(true, cx);
    }

    fn global_voice_release(&mut self, cx: &mut Context<Self>) {
        eprintln!("global voice: release");
        self.global_voice.held = false;
        self.global_voice.host_held_since = None;
        self.global_voice.pending_target = None;
        if let Some(panel) = self
            .global_voice
            .owner
            .as_ref()
            .and_then(|owner| owner.panel.upgrade())
        {
            panel.update(cx, |panel, cx| {
                panel.end_global_voice_hold(&self.global_voice.owner.as_ref().unwrap().attempt, cx)
            });
        }
    }

    /// Host global shortcut press (macOS Carbon / Windows RegisterHotKey).
    /// The host never activates the window for this action.
    pub(super) fn begin_global_voice_hold_action(
        &mut self,
        _: &BeginGlobalVoiceHold,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let active = window.is_window_active();
        self.global_voice.set_active(active);
        if !host_press_uses_global_owner(active, overlay::available(cx)) {
            if !active {
                // Never record invisibly: no OS pill exists on this backend.
                eprintln!(
                    "global voice: no non-focusing overlay on the {:?} backend; focus Jcode to dictate",
                    cx.compositor_name()
                );
                return;
            }
            // Focused: behave exactly like the in-app hold shortcut.
            self.begin_voice_hold(&BeginVoiceHold, window, cx);
            return;
        }
        if self.global_voice.held {
            return;
        }
        self.ensure_global_voice_task(window, cx);
        self.global_voice.host_held_since = Some(Instant::now());
        self.global_voice_press(cx);
    }

    pub(super) fn end_global_voice_hold_action(
        &mut self,
        _: &EndGlobalVoiceHold,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        // Release both possible owners. Neither path ever refocuses.
        self.end_voice_hold(&EndVoiceHold, window, cx);
        if self.global_voice.host_held_since.is_some() || self.global_voice.held {
            self.global_voice_release(cx);
        }
    }

    fn cancel_global_voice_capture(&mut self, cx: &mut Context<Self>) {
        self.global_voice.held = false;
        self.global_voice.host_held_since = None;
        self.global_voice.pending_target = None;
        self.global_voice.press_serial = self.global_voice.press_serial.wrapping_add(1);
        self.global_voice.permission_task = None;
        if let Some(owner) = self.global_voice.owner.as_ref() {
            if let Some(panel) = owner.panel.upgrade() {
                panel.update(cx, |panel, cx| {
                    panel.cancel_global_voice(&owner.attempt, cx)
                });
            }
        }
        self.global_voice.close_overlay(cx);
    }

    fn check_global_voice_permission(&mut self, start: bool, cx: &mut Context<Self>) {
        let serial = self.global_voice.press_serial;
        self.global_voice.last_permission_check = Some(Instant::now());
        let check = cx
            .background_executor()
            .spawn(crate::global_voice_session::permitted());
        self.global_voice.permission_task = Some(cx.spawn(async move |this, cx| {
            let allowed = check.await;
            let _ = this.update(cx, |this, cx| {
                if this.global_voice.press_serial != serial {
                    return;
                }
                this.global_voice.permission_task = None;
                if !allowed {
                    eprintln!("global voice: denied by session check");
                    this.cancel_global_voice_capture(cx);
                } else if start && this.global_voice.held {
                    this.begin_global_voice(cx);
                }
            });
        }));
    }

    fn begin_global_voice(&mut self, cx: &mut Context<Self>) {
        if self.account_sign_in.visible {
            return;
        }
        if let Some(owner) = self
            .global_voice
            .owner
            .as_ref()
            .and_then(|owner| owner.panel.upgrade())
        {
            if owner.read(cx).voice_active() {
                return;
            }
        }
        self.global_voice.close_overlay(cx);
        let Some(panel) = self
            .global_voice
            .pending_target
            .take()
            .and_then(|panel| panel.upgrade())
        else {
            eprintln!("global voice: no eligible chat to receive the transcript");
            return;
        };
        if !self
            .slots
            .iter()
            .any(|slot| !slot.closing && slot.panel == panel)
        {
            return;
        }
        if panel.read(cx).voice_active() {
            return;
        }
        // Prove a visible indicator exists before opening the microphone.
        // Focused: the chat's in-panel pill. Unfocused: a non-focusing OS pill.
        // Unsupported compositors must not record invisibly.
        if os_pill_wanted(self.global_voice.window_active, true)
            && !self.show_global_voice_overlay(
                Snapshot {
                    title: "Connecting…".into(),
                    levels: None,
                },
                cx,
            )
        {
            // Restore native focused-key handling instead of swallowing
            // every future Copilot press on an unsupported compositor.
            self.global_voice.disable_capture_source();
            return;
        }
        self.global_voice.finished_at = None;
        // No activate_window, set_active or focus_input here. The source draft
        // receives the result while the user's other application keeps focus.
        if let Some(attempt) = panel.update(cx, |panel, cx| panel.begin_global_voice_hold(cx)) {
            self.global_voice.owner = Some(Capture {
                panel: panel.downgrade(),
                attempt,
            });
        } else {
            self.global_voice.close_overlay(cx);
        }
    }

    fn update_global_voice_overlay(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self
            .global_voice
            .owner
            .as_ref()
            .and_then(|owner| owner.panel.upgrade())
        else {
            self.global_voice.close_overlay(cx);
            return;
        };
        if !self
            .slots
            .iter()
            .any(|slot| !slot.closing && slot.panel == panel)
        {
            panel.update(cx, |panel, cx| {
                panel.cancel_global_voice(&self.global_voice.owner.as_ref().unwrap().attempt, cx)
            });
            self.global_voice.close_overlay(cx);
            return;
        }
        let snapshot = panel
            .read(cx)
            .global_voice_snapshot(&self.global_voice.owner.as_ref().unwrap().attempt);
        if snapshot.is_none() {
            self.global_voice.close_overlay(cx);
            return;
        }
        if !panel.read(cx).voice_active() {
            let since = self
                .global_voice
                .finished_at
                .get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(5) {
                self.global_voice.close_overlay(cx);
                return;
            }
        }
        let Some(snapshot) = snapshot else { return };
        if !os_pill_wanted(self.global_voice.window_active, true) {
            // Focus moved to this window mid-hold: the in-panel pill takes over.
            self.global_voice.hide_overlay(cx);
            return;
        }
        let shown = match self.global_voice.overlay {
            Some(handle) => handle
                .update(cx, |overlay, _, cx| overlay.set_snapshot(snapshot, cx))
                .is_ok(),
            // Focus left mid-hold: mirror the in-panel pill at the OS level.
            None => self.show_global_voice_overlay(snapshot, cx),
        };
        if !shown {
            // Never keep recording without a visible indicator.
            panel.update(cx, |panel, cx| {
                panel.cancel_global_voice(&self.global_voice.owner.as_ref().unwrap().attempt, cx)
            });
            self.global_voice.close_overlay(cx);
        }
    }

    fn show_global_voice_overlay(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) -> bool {
        match overlay::open(snapshot, cx) {
            Ok(handle) => {
                eprintln!("global voice: OS pill shown");
                self.global_voice.overlay = Some(handle);
                true
            }
            Err(error) => {
                eprintln!("global voice overlay unavailable: {error:#}");
                false
            }
        }
    }
}

#[cfg(test)]
mod pill_tests {
    use super::{host_press_uses_global_owner, os_pill_wanted};

    #[test]
    fn host_shortcut_only_takes_global_path_while_unfocused_with_a_pill() {
        assert!(host_press_uses_global_owner(false, true));
        assert!(!host_press_uses_global_owner(true, true), "focused keeps its native hold");
        assert!(!host_press_uses_global_owner(false, false), "never record invisibly");
        assert!(!host_press_uses_global_owner(true, false));
    }

    #[test]
    fn os_pill_only_mirrors_the_chat_pill_while_unfocused() {
        assert!(os_pill_wanted(false, true));
        assert!(!os_pill_wanted(true, true), "focused chat already shows its own pill");
        assert!(!os_pill_wanted(false, false));
        assert!(!os_pill_wanted(true, false));
    }
}

#[cfg(test)]
mod host_action_tests {
    use super::*;

    fn workspace_with_two_chats(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
        cx.add_window_view(|_, cx| {
            let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
            workspace.push_test_panel("first", cx);
            workspace.push_test_panel("second", cx);
            // Preview panels never open a microphone.
            for slot in &mut workspace.slots {
                slot.panel = cx.new(|cx| Panel::new_preview(crate::preview_state::PreviewState::Empty, cx));
            }
            workspace
        })
    }

    // On Linux, GPUI's fake platform reports no compositor, so no overlay
    // backend exists, like GNOME/Mutter Wayland. macOS/Windows always have one.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn unfocused_host_press_without_overlay_never_records_or_steals_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = workspace_with_two_chats(cx);
        vcx.update(|window, cx| workspace.update(cx, |workspace, cx| {
            workspace.set_active(1, cx);
            workspace.focus_active(window, cx);
        }));
        vcx.deactivate_window();
        vcx.update(|window, cx| {
            assert!(!window.is_window_active());
            assert!(!overlay::available(cx));
            let press = cx.build_action("workspace::BeginGlobalVoiceHold", None).unwrap();
            window.focus_next(cx);
            assert!(window.is_action_available(press.as_ref(), cx));
            window.dispatch_action(press, cx);
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            assert!(!window.is_window_active(), "host press must not activate the window");
            workspace.read_with(cx, |workspace, cx| {
                assert!(!workspace.global_voice.held);
                assert!(workspace.global_voice.owner.is_none());
                assert!(workspace.global_voice.overlay.is_none(), "no OS pill");
                assert_eq!(workspace.active, 1, "press must not move the active chat");
                assert!(workspace.slots.iter().all(|slot| !slot.panel.read(cx).voice_active()));
            });
            let release = cx.build_action("workspace::EndGlobalVoiceHold", None).unwrap();
            assert!(window.is_action_available(release.as_ref(), cx));
            window.dispatch_action(release, cx);
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            assert!(workspace.global_voice.host_held_since.is_none());
        });
    }

    #[gpui::test]
    fn focused_host_press_uses_in_panel_pill_not_os_pill(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = workspace_with_two_chats(cx);
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            assert!(window.is_window_active());
            let press = cx.build_action("workspace::BeginGlobalVoiceHold", None).unwrap();
            window.focus_next(cx);
            window.dispatch_action(press, cx);
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |workspace, _| {
            // Focused: the native hold path ran. No global owner, no OS pill.
            assert!(workspace.global_voice.window_active);
            assert!(!workspace.global_voice.held);
            assert!(workspace.global_voice.host_held_since.is_none());
            assert!(workspace.global_voice.owner.is_none());
            assert!(workspace.global_voice.overlay.is_none());
        });
    }
}
