//! Opt-in, machine-readable workspace navigation diagnostics.
//!
//! Emitted alongside the legacy strip line in JCODE_DESKTOP_STATE. Stable panel
//! identities let native-input checks distinguish reorder from skipped focus.

use super::*;

impl Workspace {
    pub(super) fn navigation_state(&self, window: &Window, cx: &App) -> serde_json::Value {
        let index_for_id = |id| {
            self.slots
                .iter()
                .position(|slot| slot.panel.entity_id() == id)
        };
        let focused = self
            .slots
            .get(self.active)
            .filter(|slot| slot.row == self.active_row)
            .map(|_| self.active);
        let keyboard_panel = self.slots.iter().position(|slot| {
            slot.panel
                .read(cx)
                .input_focus_handle(cx)
                .is_focused(window)
        });
        let rows: Vec<_> = (0..STRIP_COUNT)
            .map(|row| {
                let panels: Vec<_> = self
                    .row_indices(row)
                    .map(|index| {
                        let slot = &self.slots[index];
                        serde_json::json!({
                            "slot": index,
                            "id": slot.panel.entity_id().as_u64(),
                            "session": slot.panel.read(cx).session_id,
                            "history_loaded": slot.panel.read(cx).history_loaded(),
                            "history_items": slot.panel.read(cx).items.len(),
                            // Length only: the dump must never contain draft text.
                            "draft_chars": slot.panel.read(cx).input.read(cx).snapshot().content.chars().count(),
                            "terminal": slot.panel.read(cx).terminal_debug_snapshot(cx),
                            "width": slot.width_fraction,
                            "focused": focused == Some(index),
                            "closing": slot.closing,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "row": row,
                    "panels": panels,
                    "remembered": self.row_focus[row].and_then(index_for_id)
                        .filter(|index| self.slots[*index].row == row),
                    "camera": self.camera_x[row],
                    "camera_target": self.camera_target[row],
                })
            })
            .collect();
        let mut progress = self.row_progress;
        let map_camera = if self.outgoing_row.is_some() {
            self.map_position(progress.sample(Instant::now()))
        } else {
            (self.camera_x[self.active_row], self.active_row as f32)
        };
        serde_json::json!({
            "version": 1,
            "single_panel": self.single_panel,
            "resume_picker": self.resume.is_some(),
            "visible_panels": if self.single_panel { usize::from(!self.slots.is_empty()) } else { self.row_indices(self.active_row).count() },
            "sidebar_visible": self.show_sidebar && !self.single_panel,
            "viewport": [f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)],
            "compact_sidebar": self.show_sidebar && responsive::is_compact(f32::from(window.viewport_size().width)),
            "sidebar_overlay": self.compact_sidebar_open,
            "canvas_width": self.last_canvas_width,

            "header_height": 0,
            "active_row": self.active_row,
            "focused_slot": focused,
            "keyboard_panel": keyboard_panel,
            "tab_targets": self.live_tabs.hit_targets.iter()
                .map(|(index, x)| (*index, x + self.live_tabs.header_offset))
                .collect::<Vec<_>>(),
            "tab_motion": self.live_tabs.is_animating(),
            "camera_motion": self.camera_started[self.active_row].is_some() || self.row_progress.is_animating(),
            "row_motion": self.row_progress.is_animating(),
            "map_camera": [map_camera.0, map_camera.1],
            "minimap_visible": self.show_minimap,
            "overview": self.overview,
            "rows": rows,
        })
    }
}
