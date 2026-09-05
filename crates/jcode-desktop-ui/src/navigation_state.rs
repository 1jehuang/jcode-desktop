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
        serde_json::json!({
            "version": 1,
            "active_row": self.active_row,
            "focused_slot": focused,
            "keyboard_panel": keyboard_panel,
            "minimap_visible": self.show_minimap,
            "overview": self.overview,
            "rows": rows,
        })
    }
}
