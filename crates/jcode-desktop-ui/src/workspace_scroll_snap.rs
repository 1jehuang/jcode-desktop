//! Snap scrolling: once a horizontal strip pan comes to rest, the camera
//! glides to the nearest panel edge instead of stopping between panels.
use super::*;

/// Silence after the last pan delta that ends a pan when the platform never
/// sends an Ended phase (mouse wheels, some touchpads).
const SNAP_IDLE: Duration = Duration::from_millis(140);
/// Fingers lifted. Wait briefly so platform momentum, delivered as Moved
/// after Ended, can still reschedule the snap rather than fight it.
const SNAP_AFTER_LIFT: Duration = Duration::from_millis(40);
/// How far ahead the recent pan velocity projects the resting point, so a
/// short flick advances to the next panel instead of springing back.
const SNAP_PROJECTION_MS: f32 = 110.0;

#[derive(Default)]
pub(super) struct ScrollSnap {
    task: Option<gpui::Task<()>>,
    /// Smoothed camera velocity in pixels per millisecond.
    velocity: f32,
    last_pan: Option<Instant>,
}

impl Workspace {
    pub(super) fn snap_scroll_enabled() -> bool {
        crate::config::get().workspace.snap_scroll
    }

    /// Record one direct pan delta and (re)arm the settle timer.
    pub(super) fn note_strip_pan(
        &mut self,
        row: usize,
        camera_delta: f32,
        lifted: bool,
        viewport_w: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::snap_scroll_enabled() {
            return;
        }
        let now = cx.background_executor().now();
        if camera_delta != 0.0 {
            let elapsed = self
                .scroll_snap
                .last_pan
                .map(|last| now.saturating_duration_since(last))
                .filter(|elapsed| *elapsed <= GESTURE_RESET);
            match elapsed {
                Some(elapsed) => {
                    let ms = (elapsed.as_secs_f32() * 1000.0).max(4.0);
                    let sample = camera_delta / ms;
                    self.scroll_snap.velocity = self.scroll_snap.velocity * 0.6 + sample * 0.4;
                }
                None => self.scroll_snap.velocity = 0.0,
            }
            self.scroll_snap.last_pan = Some(now);
        } else if self.scroll_snap.task.is_none() {
            // A lift with no pan in flight has nothing to settle.
            return;
        }
        let delay = if lifted { SNAP_AFTER_LIFT } else { SNAP_IDLE };
        self.scroll_snap.task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.scroll_snap.task = None;
                this.settle_strip_snap(row, viewport_w, window, cx);
            });
        }));
    }

    /// Glide the strip's camera to the nearest panel edge.
    pub(super) fn settle_strip_snap(
        &mut self,
        row: usize,
        viewport_w: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let velocity = std::mem::take(&mut self.scroll_snap.velocity);
        self.scroll_snap.last_pan = None;
        if row != self.active_row || self.outgoing_row.is_some() || viewport_w <= 0.0 {
            return;
        }
        let panels: Vec<(usize, f32)> = self
            .row_indices(row)
            .filter(|&index| !self.slots[index].closing)
            .map(|index| (index, self.slot_width(index, viewport_w)))
            .collect();
        let current = self.camera_target[row];
        let projection = (velocity * SNAP_PROJECTION_MS).clamp(-viewport_w / 2.0, viewport_w / 2.0);
        let Some((target, owner)) = snap_target(&panels, current + projection, viewport_w) else {
            return;
        };
        if (target - current).abs() > 0.5 {
            self.camera_from[row] = self.camera_x[row];
            self.camera_target[row] = target;
            self.camera_started[row] = Some(Instant::now());
            self.camera_touch_pan[row] = false;
            self.camera_dirty[row] = false;
        }
        // Keep focus on a panel the snapped view shows completely.
        let active_visible = panels.iter().any(|&(index, _)| index == self.active)
            && panel_fully_visible(&panels, self.active, target, viewport_w);
        if !active_visible && owner != self.active {
            if let Some(outgoing) = self.slots.get(self.active) {
                self.previous = Some(outgoing.panel.entity_id());
            }
            self.active = owner;
            self.focus_active(window, cx);
        }
        cx.notify();
    }
}

/// Strip-space left edges for `(index, width)` panels laid out in order.
fn panel_lefts(panels: &[(usize, f32)]) -> impl Iterator<Item = (usize, f32, f32)> + '_ {
    panels.iter().scan(STRUT, |left, &(index, width)| {
        let start = *left;
        *left += width + GAP;
        Some((index, start, width))
    })
}

fn panel_fully_visible(panels: &[(usize, f32)], index: usize, camera: f32, viewport: f32) -> bool {
    panel_lefts(panels).any(|(candidate, left, width)| {
        candidate == index && left - camera >= -1.0 && left + width - camera <= viewport + 1.0
    })
}

/// The resting camera nearest `projected`: either a panel's left edge at the
/// viewport's left, or its right edge at the viewport's right. Returns the
/// camera position and the panel that position aligns.
pub(super) fn snap_target(
    panels: &[(usize, f32)],
    projected: f32,
    viewport: f32,
) -> Option<(f32, usize)> {
    let total = panels.iter().map(|(_, width)| width + GAP).sum::<f32>() + STRUT * 2.0;
    let max_scroll = (total - viewport).max(-GAP).max(-STRUT);
    let mut best: Option<(f32, usize)> = None;
    for (index, left, width) in panel_lefts(panels) {
        for candidate in [left - STRUT, left + width + STRUT - viewport] {
            let candidate = candidate.clamp(-STRUT, max_scroll);
            if best.is_none_or(|(best, _)| (candidate - projected).abs() < (best - projected).abs())
            {
                best = Some((candidate, index));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_nearest_panel_edge() {
        let panels = [(0, 500.0), (1, 500.0), (2, 500.0), (3, 500.0)];
        // Just past the first boundary: back to panels 1 and 2 filling the view.
        let (x, _) = snap_target(&panels, 520.0, 1000.0).unwrap();
        assert!((x - 500.0).abs() < 2.0, "{x}");
        // Just before it: also settles there rather than staying offset.
        let (x, _) = snap_target(&panels, 470.0, 1000.0).unwrap();
        assert!((x - 500.0).abs() < 2.0, "{x}");
        // Most of the way to the next boundary: forward.
        let (x, _) = snap_target(&panels, 900.0, 1000.0).unwrap();
        assert!((x - 1000.0).abs() < 1.0, "{x}");
    }

    #[test]
    fn snap_never_leaves_the_pannable_range() {
        let panels = [(0, 600.0), (1, 600.0)];
        let (x, _) = snap_target(&panels, 5000.0, 1000.0).unwrap();
        assert!(x <= 1200.0 + STRUT * 2.0 - 1000.0 + 0.01);
        let (x, _) = snap_target(&panels, -5000.0, 1000.0).unwrap();
        assert_eq!(x, -STRUT);
        assert!(snap_target(&[], 0.0, 1000.0).is_none());
    }

    #[test]
    fn full_width_panels_snap_one_page_at_a_time() {
        let panels = [(0, 1000.0), (1, 1000.0), (2, 1000.0)];
        let (x, owner) = snap_target(&panels, 1400.0, 1001.16).unwrap();
        assert!((x - 1000.0).abs() < 1.0 && owner == 1, "{x} {owner}");
    }
}
