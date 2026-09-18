//! Navigation is camera travel over one map, not a swap of independent pages.
use super::*;

impl Workspace {
    pub(super) fn map_position(&self, progress: f32) -> (f32, f32) {
        (
            self.row_origin.0
                + (self.camera_target[self.active_row] - self.row_origin.0) * progress,
            self.row_origin.1 + (self.active_row as f32 - self.row_origin.1) * progress,
        )
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
            self.map_position(progress)
        } else {
            let old_row = self.active_row;
            let x = self.camera_started[old_row].map_or(self.camera_x[old_row], |started| {
                smoothed_camera_position(
                    self.camera_from[old_row],
                    self.camera_target[old_row],
                    now.saturating_duration_since(started),
                    if self.camera_touch_pan[old_row] {
                        TOUCH_PAN_DURATION
                    } else {
                        transition::policy(Transition::Focus).duration
                    },
                )
            });
            // Continue from the resisted pull already on screen, rather than
            // snapping home before starting the committed row transition.
            (x, old_row as f32 + self.workspace_pull.take(now))
        };
        self.gesture.pull = 0.0;
        self.outgoing_row = Some(self.active_row);
        self.row_progress = AnimatedValue::new(0.0, transition::policy(Transition::Row).duration);
        self.row_progress.set(1.0, now);
    }

    /// Pointer input takes ownership from the camera's current map position,
    /// not from the old destination. Keep Y continuous while X follows input.
    pub(super) fn begin_map_pan(&mut self, row: usize, smooth: bool) -> bool {
        if self.outgoing_row.is_none() {
            return false;
        }
        self.begin_map_navigation(self.active_row);
        let x = self.row_origin.0;
        self.camera_x[row] = x;
        self.camera_target[row] = x;
        if smooth {
            self.row_progress = AnimatedValue::new(0.0, TOUCH_PAN_DURATION);
            self.row_progress.set(1.0, Instant::now());
        }
        true
    }

    pub(super) fn render_map_trip(
        &mut self,
        progress: f32,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Resolve first: both axes must share one easing curve and destination.
        // Otherwise the incoming row pans from its remembered X while the old
        // row sits still, breaking the illusion of a fixed two-dimensional map.
        if self.camera_dirty[self.active_row] {
            self.resolve_camera_target(width);
        }
        let (x, y) = self.map_position(progress);
        self.map_camera_x = Some(x);
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
        self.map_camera_x = None;
        // The destination's cached camera must meet the map camera exactly at
        // arrival, rather than finish a second, independently timed pan.
        self.camera_x[self.active_row] = x;
        self.camera_from[self.active_row] = x;
        self.camera_started[self.active_row] = None;
        self.camera_touch_pan[self.active_row] = false;
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

    #[gpui::test]
    fn tab_click_travels_diagonally_in_shared_map_coordinates(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.push_test_panel("top", cx);
            w.active_row = 1;
            w.push_test_panel("bottom-left", cx);
            w.push_test_panel("bottom-right", cx);
            w.active_row = 0;
            w.active = 0;
            w
        });
        vcx.run_until_parked();
        // Give the destination a different horizontal viewpoint. A page slide
        // would leave the two strips at different X coordinates throughout.
        workspace.update(vcx, |w, _| {
            w.camera_target[1] = 300.0;
            w.camera_x[1] = 300.0;
        });
        let tab = vcx.debug_bounds("live-session-tab-2").unwrap();
        vcx.simulate_click(tab.center(), gpui::Modifiers::default());
        workspace.update(vcx, |w, cx| {
            assert_eq!((w.active_row, w.active), (1, 2));
            assert_eq!(w.outgoing_row, Some(0));
            assert!(w.row_progress.is_animating());
            // Freeze an exact shared destination for the geometric assertion.
            w.camera_target[1] = 300.0;
            w.camera_dirty[1] = false;
            hold_trip(w, 0.5);
            cx.notify();
        });
        vcx.run_until_parked();
        let outgoing = vcx.debug_bounds("row-transition-outgoing").unwrap();
        let incoming = vcx.debug_bounds("row-transition-incoming").unwrap();
        assert!(
            (f32::from(incoming.origin.y - outgoing.origin.y - incoming.size.height)).abs() < 1.0
        );
        let top = vcx.debug_bounds("panel-0").unwrap();
        let bottom = vcx.debug_bounds("panel-1").unwrap();
        assert!(
            (f32::from(top.origin.x - bottom.origin.x)).abs() < 1.0,
            "rows must use the same world X, not independent remembered cameras"
        );
        workspace.update(vcx, |w, cx| {
            let (x, y) = w.map_position(0.5);
            assert!((x - 150.0).abs() < 1.0);
            assert_eq!(y, 0.5);
            // Reverse while in flight. The viewpoint must not jump to row 1.
            w.set_active(0, cx);
            assert!((w.row_origin.0 - x).abs() < 1.0);
            assert!((w.row_origin.1 - y).abs() < 0.01);
        });
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
            assert_eq!(w.row_origin.1, 0.0);
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
                (w.row_origin.1 - 1.0).abs() < 0.01,
                "reversing starts at the current intermediate row, not the previous destination"
            );
            assert!((w.map_position(0.0).1 - 1.0).abs() < 0.01);
            assert_eq!(w.map_position(1.0).1, 0.0);
        });
    }

    #[gpui::test]
    fn pointer_pan_rebases_an_in_flight_diagonal_trip(cx: &mut gpui::TestAppContext) {
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
        for smooth in [false, true] {
            workspace.update_in(vcx, |w, window, cx| {
                w.active_row = 1;
                w.active = 1;
                w.outgoing_row = Some(0);
                w.row_origin = (0.0, 0.0);
                w.camera_target[1] = 1000.0;
                w.camera_x[1] = 500.0;
                hold_trip(w, 0.5);
                w.pan_strip(1, -20.0, 3000.0, 1000.0, smooth, window, cx);
                let (x, y) = w.map_position(0.0);
                assert!((x - if smooth { 500.0 } else { 520.0 }).abs() < 1.0);
                assert!((y - 0.5).abs() < 0.01, "pan must not teleport vertically");
                assert!((w.map_position(1.0).0 - 520.0).abs() < 1.0);
            });
        }
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
