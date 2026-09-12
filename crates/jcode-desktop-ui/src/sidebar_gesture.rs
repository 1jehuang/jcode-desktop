//! Pointer-only dismissal. Wheel scrolling never counts as a swipe.
use super::*;

const CLICK_SLOP: f32 = 6.0;
const CLOSE_DISTANCE: f32 = 72.0;
const VERTICAL_SLOP: f32 = 16.0;

#[derive(Debug, PartialEq)]
enum Outcome {
    Click,
    Close,
    Cancel,
}

struct Motion {
    origin: (f32, f32),
    moved: bool,
    vertical: bool,
}

impl Motion {
    fn new(x: f32, y: f32) -> Self {
        Self {
            origin: (x, y),
            moved: false,
            vertical: false,
        }
    }

    fn observe(&mut self, x: f32, y: f32) {
        let dx = (x - self.origin.0).abs();
        let dy = (y - self.origin.1).abs();
        self.moved |= dx > CLICK_SLOP || dy > CLICK_SLOP;
        // Latch cancellation, so a vertical scroll/drag cannot become a close
        // simply by returning to its original row before release.
        self.vertical |= dy > VERTICAL_SLOP || (dy > CLICK_SLOP && dy > dx);
    }

    fn finish(&mut self, x: f32, y: f32, inside: bool, closable: bool) -> Outcome {
        self.observe(x, y);
        if self.vertical {
            Outcome::Cancel
        } else if closable && self.origin.0 - x >= CLOSE_DISTANCE {
            Outcome::Close
        } else if inside && !self.moved {
            Outcome::Click
        } else {
            Outcome::Cancel
        }
    }
}

pub(super) struct Pending {
    session: jcode_sdk::SessionInfo,
    order: Vec<String>,
    is_open: bool,
    modifiers: gpui::Modifiers,
    motion: Motion,
}

impl Workspace {
    pub(super) fn begin_sidebar_gesture(
        &mut self,
        session: jcode_sdk::SessionInfo,
        order: Vec<String>,
        is_open: bool,
        event: &gpui::MouseDownEvent,
    ) {
        self.sidebar_gesture = Some(Pending {
            session,
            order,
            is_open,
            modifiers: event.modifiers,
            motion: Motion::new(event.position.x.into(), event.position.y.into()),
        });
    }

    pub(super) fn move_sidebar_gesture(&mut self, event: &gpui::MouseMoveEvent) {
        if event.pressed_button != Some(gpui::MouseButton::Left) {
            self.sidebar_gesture = None;
        } else if let Some(pending) = &mut self.sidebar_gesture {
            pending
                .motion
                .observe(event.position.x.into(), event.position.y.into());
        }
    }

    pub(super) fn finish_sidebar_gesture(
        &mut self,
        id: &str,
        inside: bool,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .sidebar_gesture
            .as_ref()
            .is_some_and(|p| p.session.session_id == id)
        {
            return;
        }
        let mut pending = self.sidebar_gesture.take().unwrap();
        let modifiers = pending.modifiers;
        let modified = modifiers.shift || modifiers.control || modifiers.platform || modifiers.alt;
        let outcome = pending.motion.finish(
            event.position.x.into(),
            event.position.y.into(),
            inside,
            pending.is_open && !modified,
        );
        cx.stop_propagation();
        match outcome {
            Outcome::Close => {
                self.close_sidebar_sessions(vec![pending.session.session_id], window, cx)
            }
            Outcome::Click => {
                if pending.is_open {
                    self.sidebar_selection.click(
                        &pending.session.session_id,
                        &pending.order,
                        modifiers.shift,
                        modifiers.control || modifiers.platform,
                    );
                    if modifiers.shift || modifiers.control || modifiers.platform {
                        cx.notify();
                        return;
                    }
                } else {
                    self.sidebar_selection = Default::default();
                }
                self.activate_session(pending.session, window, cx);
            }
            Outcome::Cancel => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_and_jitter_do_not_close() {
        assert_eq!(
            Motion::new(150., 50.).finish(150., 50., true, true),
            Outcome::Click
        );
        assert_eq!(
            Motion::new(150., 50.).finish(145., 54., true, true),
            Outcome::Click
        );
    }

    #[test]
    fn only_deliberate_left_drag_closes_even_outside_row() {
        assert_eq!(
            Motion::new(150., 50.).finish(79., 50., true, true),
            Outcome::Cancel
        );
        assert_eq!(
            Motion::new(150., 50.).finish(78., 50., true, true),
            Outcome::Close
        );
        assert_eq!(
            Motion::new(150., 50.).finish(-10., 50., false, true),
            Outcome::Close
        );
        assert_eq!(
            Motion::new(150., 50.).finish(250., 50., true, true),
            Outcome::Cancel
        );
    }

    #[test]
    fn vertical_and_diagonal_motion_never_closes() {
        assert_eq!(
            Motion::new(150., 50.).finish(50., 67., true, true),
            Outcome::Cancel
        );
        let mut motion = Motion::new(150., 50.);
        motion.observe(149., 58.);
        assert_eq!(motion.finish(50., 50., true, true), Outcome::Cancel);
    }

    #[test]
    fn drag_returning_to_origin_is_not_a_click() {
        let mut motion = Motion::new(150., 50.);
        motion.observe(60., 50.);
        assert_eq!(motion.finish(150., 50., true, true), Outcome::Cancel);
    }

    #[test]
    fn history_modified_drags_and_outside_clicks_cancel() {
        assert_eq!(
            Motion::new(150., 50.).finish(50., 50., true, false),
            Outcome::Cancel
        );
        assert_eq!(
            Motion::new(150., 50.).finish(150., 50., false, true),
            Outcome::Cancel
        );
    }

    fn fixture(cx: &mut Context<Workspace>) -> Workspace {
        let mut workspace = Workspace::for_test(learning::Coach::new(), cx);
        for (row, id) in [(0, "upper"), (1, "lower")] {
            workspace.active_row = row;
            workspace.push_test_panel(id, cx);
            workspace.sessions.push(jcode_sdk::SessionInfo {
                session_id: id.into(),
                title: Some(id.into()),
                working_dir: None,
                status: "idle".into(),
                transcript_bytes: None,
                saved: false,
                updated_at_ms: None,
                last_active_at_ms: None,
                archived: false,
                archived_at_ms: None,
                parent_session_id: None,
                agent_label: None,
                swarm_status: None,
            });
        }
        workspace.set_active(0, cx);
        workspace
    }

    #[gpui::test]
    fn rendered_sidebar_drag_closes_other_workspace_without_activating_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| fixture(cx));
        vcx.run_until_parked();
        let bounds = vcx.debug_bounds("sidebar-session-1").unwrap();
        let start = gpui::point(bounds.right() - px(50.), bounds.center().y);
        let end = start - gpui::point(px(80.), px(0.));
        vcx.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.active, 0, "press must not activate drag target")
        });
        vcx.simulate_mouse_move(end, gpui::MouseButton::Left, Default::default());
        vcx.simulate_mouse_up(end, gpui::MouseButton::Left, Default::default());
        workspace.read_with(vcx, |w, cx| {
            assert!(
                w.slots
                    .iter()
                    .find(|s| s.panel.read(cx).session_id == "lower")
                    .unwrap()
                    .closing
            );
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "upper");
            assert_eq!(w.active_row, 0);
            assert!(w.sidebar_gesture.is_none());
        });
    }

    #[gpui::test]
    fn rendered_sidebar_short_vertical_and_wheel_drags_neither_close_nor_activate(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| fixture(cx));
        vcx.run_until_parked();
        let bounds = vcx.debug_bounds("sidebar-session-1").unwrap();
        let start = gpui::point(bounds.right() - px(50.), bounds.center().y);
        for (dx, dy, wheel) in [(30., 0., false), (80., 30., false), (80., 0., true)] {
            vcx.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
            if wheel {
                vcx.simulate_event(gpui::ScrollWheelEvent {
                    position: start,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-10.))),
                    ..Default::default()
                });
            }
            let end = start - gpui::point(px(dx), px(dy));
            vcx.simulate_mouse_move(end, gpui::MouseButton::Left, Default::default());
            vcx.simulate_mouse_up(end, gpui::MouseButton::Left, Default::default());
            workspace.read_with(vcx, |w, _| {
                assert_eq!(w.active, 0);
                assert!(w.slots.iter().all(|s| !s.closing));
                assert!(w.sidebar_gesture.is_none());
            });
        }
        vcx.simulate_click(start, Default::default());
        workspace.read_with(vcx, |w, _| {
            assert_eq!(w.active, 1, "normal click still activates")
        });
    }

    #[gpui::test]
    fn rendered_sidebar_close_button_does_not_start_a_row_gesture(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| fixture(cx));
        vcx.run_until_parked();
        let close = vcx.debug_bounds("sidebar-close-1").unwrap().center();
        vcx.simulate_mouse_move(close, None, Default::default());
        vcx.simulate_click(close, Default::default());
        workspace.read_with(vcx, |w, cx| {
            assert!(
                w.slots
                    .iter()
                    .find(|s| s.panel.read(cx).session_id == "lower")
                    .unwrap()
                    .closing
            );
            assert_eq!(w.active, 0);
            assert!(w.sidebar_gesture.is_none());
        });
    }
}
