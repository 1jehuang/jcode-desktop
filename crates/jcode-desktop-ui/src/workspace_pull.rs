//! Direct, resisted workspace movement before a swipe commits to a row.
use super::*;

/// Show a little of the destination well before the switching threshold.
const PREVIEW_ROWS: f32 = 0.10;

pub(super) struct PullPreview {
    motion: AnimatedValue,
    target: f32,
    pub(super) value: f32,
}

impl Default for PullPreview {
    fn default() -> Self {
        Self {
            motion: AnimatedValue::new(0.0, transition::policy(Transition::Row).duration),
            target: 0.0,
            value: 0.0,
        }
    }
}

impl PullPreview {
    fn update(&mut self, target: f32, now: Instant, reduce_motion: bool) -> f32 {
        if reduce_motion {
            *self = Self::default();
            return 0.0;
        }
        if target != self.target {
            if target == 0.0 {
                self.motion.set(0.0, now);
            } else {
                // Follow fingers directly. Only a released pull eases home.
                self.motion =
                    AnimatedValue::new(target, transition::policy(Transition::Row).duration);
            }
            self.target = target;
        }
        self.value = self.motion.sample(now);
        self.value
    }

    pub(super) fn take(&mut self, now: Instant) -> f32 {
        let value = self.motion.sample(now);
        *self = Self::default();
        value
    }

    pub(super) fn is_animating(&self) -> bool {
        self.motion.is_animating()
    }
}

impl Workspace {
    pub(super) fn sample_workspace_pull(&mut self, now: Instant, cx: &App) -> f32 {
        let held = self.gesture_seen.is_some_and(|seen| {
            cx.background_executor().now().duration_since(seen) <= GESTURE_RESET
        });
        let target =
            if held && !self.gesture.switched && self.outgoing_row.is_none() && !self.overview {
                let pull = (self.gesture.pull / STRIP_BREAK).clamp(-1.0, 1.0) * PREVIEW_ROWS;
                // The first/last workspace has no neighboring row to reveal.
                (self.active_row as f32 + pull).clamp(0.0, (STRIP_COUNT - 1) as f32)
                    - self.active_row as f32
            } else {
                0.0
            };
        self.workspace_pull.update(
            target,
            now,
            cx.reduce_motion() || crate::config::get().appearance.reduce_motion,
        )
    }

    pub(super) fn render_workspace_pull(
        &mut self,
        pull: f32,
        width: f32,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let source = self.active_row;
        let entity = cx.entity();
        let total_width = self
            .row_indices(source)
            .map(|index| self.slot_width(index, width) + GAP)
            .sum::<f32>()
            + STRUT * 2.0;
        let mut scene = div().relative().size_full().overflow_hidden().child(
            gpui::canvas(
                move |bounds, window, _| {
                    (
                        bounds,
                        window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                    )
                },
                move |_, (bounds, hitbox), window, _| {
                    window.on_mouse_event(
                        move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                            if phase != gpui::DispatchPhase::Capture
                                || !hitbox.should_handle_scroll(window)
                            {
                                return;
                            }
                            let y = f32::from(event.position.y - bounds.origin.y);
                            // The source row's router owns its visible region.
                            // Capture only the sliver uncovered by the preview,
                            // before its transcript can receive this gesture.
                            if y >= -pull * height && y < (1.0 - pull) * height {
                                return;
                            }
                            entity.update(cx, |this, cx| {
                                if this.active_row == source
                                    && this.route_strip_scroll(
                                        source,
                                        None,
                                        event,
                                        total_width,
                                        width,
                                        window,
                                        cx,
                                    )
                                {
                                    cx.stop_propagation();
                                }
                            });
                        },
                    );
                },
            )
            .absolute()
            .size_full(),
        );
        for row in 0..STRIP_COUNT {
            let top = (row as f32 - self.active_row as f32 - pull) * height;
            if top.abs() >= height {
                continue;
            }
            let active = row == self.active_row;
            let content = self.render_row(row, width, height, window, cx);
            scene = scene.child(
                div()
                    .debug_selector(move || {
                        if active {
                            "row-pull-active".into()
                        } else {
                            "row-pull-neighbor".into()
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

    #[test]
    fn pull_preview_tracks_fingers_then_eases_home_and_stops() {
        let mut preview = PullPreview::default();
        let now = Instant::now();
        assert_eq!(preview.update(0.04, now, false), 0.04);
        assert_eq!(preview.update(0.08, now, false), 0.08);
        assert_eq!(preview.update(0.0, now, false), 0.08);
        let middle = preview.update(0.0, now + Duration::from_millis(75), false);
        assert!(middle > 0.0 && middle < 0.08);
        assert_eq!(
            preview.update(0.0, now + Duration::from_secs(1), false),
            0.0
        );
        assert!(!preview.is_animating());
        assert_eq!(preview.update(-0.04, now, false), -0.04);
        assert_eq!(preview.update(-0.06, now, true), 0.0);
        assert!(
            !preview.is_animating(),
            "reduced motion must not animate the preview"
        );
    }

    #[gpui::test]
    fn pull_preview_moves_rows_early_resists_the_dot_and_commits_without_jumping(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, cx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_sidebar = false;
            w.push_test_panel("source", cx);
            w.active_row = 1;
            w.push_test_panel("destination", cx);
            w.active_row = 0;
            w.active = 0;
            w
        });
        cx.run_until_parked();
        let panel = cx.debug_bounds("panel-0").unwrap();
        let target = panel.center();
        let scroll = |cx: &mut gpui::VisualTestContext, dx, dy, touch_phase| {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: target,
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(dx), px(dy))),
                modifiers: gpui::Modifiers::default(),
                touch_phase,
            });
            cx.run_until_parked();
        };
        scroll(cx, -40.0, 0.0, gpui::TouchPhase::Started);
        scroll(cx, 0.0, -26.0, gpui::TouchPhase::Moved);
        let active = cx
            .debug_bounds("row-pull-active")
            .expect("preview starts at one tenth of the threshold");
        let neighbor = cx
            .debug_bounds("row-pull-neighbor")
            .expect("neighbor is already revealed");
        let reticle = cx.debug_bounds("gesture-reticle").unwrap();
        let preview = workspace.read_with(cx, |w, _| {
            assert_eq!(
                (w.active_row, w.active),
                (0, 0),
                "preview must not change focus"
            );
            assert!(w.outgoing_row.is_none());
            w.workspace_pull.value
        });
        assert!((preview - 0.01).abs() < 0.0001);
        assert!(cx.debug_bounds("panel-0").unwrap().origin.y < panel.origin.y);
        assert!((f32::from(neighbor.origin.y - active.origin.y - active.size.height)).abs() < 1.0);
        // The moving row must not drag the dot in the opposite direction.
        let resting_center =
            f32::from(active.origin.y) + (0.5 + preview) * f32::from(active.size.height);
        let dot_travel = f32::from(reticle.center().y) - resting_center;
        assert!((dot_travel - 26.0 * PULL_RESISTANCE).abs() < 1.0);
        assert!(dot_travel > 0.0 && dot_travel < 26.0 * 0.35);
        scroll(cx, 0.0, -(STRIP_BREAK - 26.0), gpui::TouchPhase::Moved);
        workspace.update(cx, |w, cx| {
            assert_eq!(w.active_row, 1);
            assert!(
                (w.row_origin.1 - preview).abs() < 0.0001,
                "commit begins at the displayed preview, not at rest"
            );
            assert_eq!(w.workspace_pull.value, 0.0);
            w.row_progress = AnimatedValue::new(0.0, Duration::from_secs(3600));
            w.row_progress.set(1.0, Instant::now());
            cx.notify();
        });
        cx.run_until_parked();
        let outgoing = cx.debug_bounds("row-transition-outgoing").unwrap();
        assert!(
            (f32::from(outgoing.origin.y - active.origin.y)).abs() < 1.0,
            "the painted workspace must not snap back on commit"
        );
    }

    #[gpui::test]
    fn pull_preview_keeps_input_when_the_neighbor_moves_under_the_pointer(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, cx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.show_sidebar = false;
            w.push_test_panel("empty-source", cx);
            w.slots[0].panel.update(cx, |p, _| p.items.clear());
            w.active_row = 1;
            w.push_test_panel("populated-destination", cx);
            w.active_row = 0;
            w.active = 0;
            w
        });
        cx.run_until_parked();
        let target = cx.debug_bounds("panel-0").unwrap().center();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: target,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-STRIP_BREAK * 0.5))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Started,
        });
        cx.run_until_parked();
        let neighbor = cx.debug_bounds("row-pull-neighbor").unwrap();
        let exposed = gpui::point(neighbor.center().x, neighbor.origin.y + px(2.0));
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: exposed,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-STRIP_BREAK * 0.5))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        assert_eq!(
            workspace.read_with(cx, |w, _| w.active_row),
            1,
            "the original pull must still complete when the pointer enters the revealed row"
        );
    }

    #[gpui::test]
    fn pull_preview_releases_on_end_cancel_and_idle_without_switching(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        for phase in [gpui::TouchPhase::Ended, gpui::TouchPhase::Cancelled] {
            workspace.update(cx, |w, cx| {
                w.gesture
                    .feed_vertical(-STRIP_BREAK * 0.5, gpui::TouchPhase::Started);
                w.gesture_seen = Some(cx.background_executor().now());
                let now = Instant::now();
                assert!(w.sample_workspace_pull(now, cx) > 0.0);
                w.gesture.feed_vertical(0.0, phase);
                assert!(
                    w.sample_workspace_pull(now, cx) > 0.0,
                    "release must ease rather than teleport"
                );
                assert_eq!(
                    w.sample_workspace_pull(now + Duration::from_secs(1), cx),
                    0.0
                );
                assert!(!w.workspace_pull.is_animating());
                assert_eq!(w.active_row, 0);
            });
        }
        workspace.update(cx, |w, cx| {
            w.gesture
                .feed_vertical(-STRIP_BREAK * 0.5, gpui::TouchPhase::Started);
            w.gesture_seen = Some(cx.background_executor().now());
            assert!(w.sample_workspace_pull(Instant::now(), cx) > 0.0);
        });
        cx.background_executor
            .advance_clock(GESTURE_RESET + Duration::from_millis(1));
        workspace.update(cx, |w, cx| {
            let now = Instant::now();
            assert!(w.sample_workspace_pull(now, cx) > 0.0);
            assert_eq!(
                w.sample_workspace_pull(now + Duration::from_secs(1), cx),
                0.0
            );
            assert_eq!(w.active_row, 0);
        });
        workspace.update(cx, |w, cx| {
            for (row, dy) in [
                (0, STRIP_BREAK * 0.5),
                (STRIP_COUNT - 1, -STRIP_BREAK * 0.5),
            ] {
                w.active_row = row;
                w.gesture.feed_vertical(dy, gpui::TouchPhase::Started);
                w.gesture_seen = Some(cx.background_executor().now());
                assert_eq!(
                    w.sample_workspace_pull(Instant::now(), cx),
                    0.0,
                    "edges must not reveal nonexistent workspaces"
                );
            }
        });
    }
}
