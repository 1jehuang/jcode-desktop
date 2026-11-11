//! Pinch navigation belongs to the canvas, after media has had first refusal.
use super::*;
use gpui::{Pixels, Point};

#[cfg(test)]
#[path = "workspace_map_gesture_tests.rs"]
mod tests;

pub(super) struct MapGesture {
    scale: f32,
    switched: bool,
    pan: (f32, f32),
    drag: Option<Point<Pixels>>,
}

impl Default for MapGesture {
    fn default() -> Self {
        Self {
            scale: 1.0,
            switched: false,
            pan: (0.0, 0.0),
            drag: None,
        }
    }
}

impl MapGesture {
    pub(super) fn reset_view(&mut self) {
        self.pan = (0.0, 0.0);
        self.drag = None;
    }

    pub(super) fn focus_session(&mut self, column: usize, row: usize) {
        self.pan = (-(column as f32 * 316.0), -(row as f32 * 206.0));
    }

    pub(super) fn card_position(&self, column: usize, row: usize) -> (f32, f32) {
        (
            32.0 + column as f32 * 316.0 + self.pan.0,
            64.0 + row as f32 * 206.0 + self.pan.1,
        )
    }

    fn update(&mut self, delta: f32, overview: bool) -> bool {
        if self.switched || !delta.is_finite() || delta <= -1.0 {
            return false;
        }
        self.scale = (self.scale * (1.0 + delta)).clamp(0.1, 10.0);
        let switch = if overview {
            self.scale >= 1.18
        } else {
            self.scale <= 0.82
        };
        self.switched = switch;
        switch
    }
}

impl Workspace {
    pub(super) fn release_map_pinch(
        &mut self,
        event: &gpui::PinchEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if matches!(
            event.phase,
            gpui::TouchPhase::Started | gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled
        ) {
            self.map_gesture.scale = 1.0;
            self.map_gesture.switched = false;
        }
    }

    pub(super) fn handle_map_pinch(
        &mut self,
        event: &gpui::PinchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            event.phase,
            gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled
        ) {
            return;
        }
        if self.map_gesture.update(event.delta, self.overview) {
            // Use the same focus and animation path as the keyboard shortcut.
            self.toggle_overview(&ToggleOverview, window, cx);
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn move_session_map(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let width = self.last_canvas_width.unwrap_or(800.0);
        let columns = (0..STRIP_COUNT)
            .map(|row| self.row_indices(row).count() + usize::from(row == self.active_row))
            .max()
            .unwrap_or(1);
        // Leave enough room to bring every row/column to the leading edge,
        // without letting the entire map disappear off screen.
        let min_x = -((columns.saturating_sub(1)) as f32 * 316.0);
        let min_y = -((STRIP_COUNT.saturating_sub(1)) as f32 * 206.0);
        self.map_gesture.pan.0 =
            (self.map_gesture.pan.0 + dx).clamp(min_x, (width - 364.0).max(0.0));
        self.map_gesture.pan.1 = (self.map_gesture.pan.1 + dy).clamp(min_y, 0.0);
        cx.notify();
    }

    pub(super) fn pan_session_map(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(window.line_height());
        self.move_session_map(f32::from(delta.x), f32::from(delta.y), cx);
        window.prevent_default();
        cx.stop_propagation();
    }

    pub(super) fn start_map_drag(
        &mut self,
        event: &gpui::MouseDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.map_gesture.drag = Some(event.position);
    }

    pub(super) fn end_map_drag(
        &mut self,
        _: &gpui::MouseUpEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.map_gesture.drag = None;
    }

    pub(super) fn drag_session_map(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(gpui::MouseButton::Left) {
            self.map_gesture.drag = None;
            return;
        }
        if let Some(previous) = self.map_gesture.drag {
            self.map_gesture.drag = Some(event.position);
            self.move_session_map(
                f32::from(event.position.x - previous.x),
                f32::from(event.position.y - previous.y),
                cx,
            );
        }
    }
}
