//! Row navigation is vertical camera travel over stacked strips.
//!
//! Each strip keeps its own horizontal camera, so the strip above is always
//! directly above: changing rows pans straight up or down and never drags the
//! strips sideways toward the destination's remembered scroll position.
use super::*;

impl Workspace {
    /// Vertical camera position, in rows, at `progress` through the trip.
    pub(super) fn map_row_position(&self, progress: f32) -> f32 {
        self.row_origin + (self.active_row as f32 - self.row_origin) * progress
    }

    /// Capture the current viewpoint before changing the logical destination.
    /// Retargeting starts where we are now, even halfway between two rows.
    pub(super) fn begin_map_navigation(&mut self, row: usize) {
        if row == self.active_row && self.outgoing_row.is_none() {
            return;
        }
        let now = Instant::now();
        let progress = self.row_progress.sample(now);
        self.row_origin = if self.outgoing_row.is_some() {
            self.map_row_position(progress)
        } else {
            // Continue from the resisted pull already on screen, rather than
            // snapping home before starting the committed row transition.
            self.active_row as f32 + self.workspace_pull.take(now)
        };
        self.gesture.pull = 0.0;
        self.outgoing_row = Some(self.active_row);
        self.row_progress = AnimatedValue::new(0.0, transition::policy(Transition::Row).duration);
        self.row_progress.set(1.0, now);
    }

    pub(super) fn render_map_trip(
        &mut self,
        progress: f32,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let y = self.map_row_position(progress);
        let mut scene = div().relative().size_full().overflow_hidden();
        for row in 0..STRIP_COUNT {
            let top = (row as f32 - y) * height;
            // Keep the destination mounted even before the camera reaches it.
            // Its input owns keyboard focus during rapid successive shortcuts.
            if top.abs() > height && row != self.active_row {
                continue;
            }
            let outgoing = self.outgoing_row == Some(row);
            let incoming = self.active_row == row;
            let content = self.render_row(row, width, height, window, cx);
            scene = scene.child(
                div()
                    .debug_selector(move || {
                        if incoming {
                            "row-transition-incoming".into()
                        } else if outgoing {
                            "row-transition-outgoing".into()
                        } else {
                            format!("row-transition-through-{row}")
                        }
                    })
                    .absolute()
                    .top(px(top))
                    .left_0()
                    .size_full()
                    .child(content),
            );
        }
        scene.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold_trip(workspace: &mut Workspace, progress: f32) {
        workspace.row_progress = AnimatedValue::new(progress, Duration::from_secs(3600));
        workspace.row_progress.set(1.0, Instant::now());
    }

    /// Leaving the left panel of a lower row for an upper row that is scrolled
    /// to its right end must pan straight up. Neither strip moves sideways.
    #[gpui::test]
    fn row_change_pans_vertically_with_each_strip_at_its_own_scroll(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            for name in ["top-0", "top-1", "top-2", "top-3"] {
                w.push_test_panel(name, cx);
            }
            w.active_row = 1;
            w.push_test_panel("bottom-left", cx);
            w.active_row = 0;
            w.active = 3;
            w
        });
        vcx.run_until_parked();
        // Settle the top row scrolled to its focused right-most panel.
        let top_camera = workspace.update(vcx, |w, cx| {
            w.camera_x[0] = w.camera_target[0];
            w.camera_started[0] = None;
            w.row_focus[0] = Some(w.slots[3].panel.entity_id());
            w.set_active(4, cx);
            hold_trip(w, 1.0);
            w.outgoing_row = None;
            cx.notify();
            w.camera_target[0]
        });
        vcx.run_until_parked();
        assert!(top_camera > 100.0, "the top row must overflow: {top_camera}");
        let bottom_rest = vcx.debug_bounds("panel-4").unwrap();

        workspace.update(vcx, |w, cx| {
            assert_eq!((w.active_row, w.active), (1, 4));
            w.select_row(0, 0);
            assert_eq!((w.active_row, w.active), (0, 3));
            hold_trip(w, 0.5);
            cx.notify();
        });
        vcx.run_until_parked();
        workspace.update(vcx, |w, _| {
            assert_eq!(w.map_row_position(0.5), 0.5);
            assert!(w.camera_started[0].is_none() && w.camera_started[1].is_none());
            assert!((w.camera_x[0] - top_camera).abs() < 0.5);
            assert!(w.camera_x[1].abs() < STRUT + 0.5);
        });
        let outgoing = vcx.debug_bounds("row-transition-outgoing").unwrap();
        let incoming = vcx.debug_bounds("row-transition-incoming").unwrap();
        assert!(
            (f32::from(outgoing.origin.y - incoming.origin.y - incoming.size.height)).abs() < 1.0
        );
        let bottom = vcx.debug_bounds("panel-4").unwrap();
        assert!(
            (f32::from(bottom.origin.x - bottom_rest.origin.x)).abs() < 0.5,
            "the row being left must not slide sideways"
        );
        let mid = vcx.debug_bounds("panel-3").unwrap();
        workspace.update(vcx, |w, cx| {
            hold_trip(w, 1.0);
            cx.notify();
        });
        vcx.run_until_parked();
        let arrived = vcx.debug_bounds("panel-3").unwrap();
        assert!(
            (f32::from(mid.origin.x - arrived.origin.x)).abs() < 0.5,
            "the destination panel must already sit directly above its resting place"
        );
        assert!(f32::from(mid.origin.y) < f32::from(arrived.origin.y));
    }

    #[gpui::test]
    fn long_map_jump_paints_intermediate_rows_and_retargets_without_teleporting(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            for row in 0..3 {
                w.active_row = row;
                w.push_test_panel(&format!("row-{row}"), cx);
            }
            w.active_row = 0;
            w.active = 0;
            w
        });
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            w.set_active(2, cx);
            assert_eq!(w.row_origin, 0.0);
            hold_trip(w, 0.5);
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("row-transition-through-1").is_some(),
            "a two-row jump must pass through the intervening map row"
        );
        workspace.update(vcx, |w, cx| {
            w.set_active(0, cx);
            assert!(
                (w.row_origin - 1.0).abs() < 0.01,
                "reversing starts at the current intermediate row, not the previous destination"
            );
            assert!((w.map_row_position(0.0) - 1.0).abs() < 0.01);
            assert_eq!(w.map_row_position(1.0), 0.0);
        });
    }

    #[gpui::test]
    fn pointer_pan_during_a_row_trip_leaves_vertical_travel_alone(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("top", cx);
            w.active_row = 1;
            w.push_test_panel("bottom", cx);
            w.active_row = 0;
            w.active = 0;
            w
        });
        vcx.run_until_parked();
        workspace.update_in(vcx, |w, window, cx| {
            w.active_row = 1;
            w.active = 1;
            w.outgoing_row = Some(0);
            w.row_origin = 0.0;
            w.camera_target[1] = 500.0;
            w.camera_x[1] = 500.0;
            hold_trip(w, 0.5);
            w.pan_strip(1, -20.0, 3000.0, 1000.0, false, window, cx);
            assert!((w.camera_x[1] - 520.0).abs() < 1.0);
            assert_eq!(w.row_origin, 0.0);
            assert!((w.map_row_position(0.5) - 0.5).abs() < 0.01);
        });
    }

    #[gpui::test]
    fn selecting_current_row_does_not_invent_vertical_motion(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("one", cx);
            w.push_test_panel("two", cx);
            w
        });
        vcx.run_until_parked();
        workspace.update(vcx, |w, cx| {
            w.select_row(0, 0);
            w.set_active(1, cx);
            assert_eq!(w.outgoing_row, None);
            assert!(!w.row_progress.is_animating());
        });
    }
}
