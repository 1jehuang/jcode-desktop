//! Local session creation never holds up the native editor. Draft IDs also
//! identify replies after concurrent creates, closure, and hot reload.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn next_draft_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Time distinguishes reloaded cdylibs, whose statics start over, while the
    // counter distinguishes requests even on a coarse system clock.
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "startup://draft/{epoch}-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

impl Workspace {
    pub(super) fn open_default_draft(&mut self, directory: Option<String>, cx: &mut Context<Self>) {
        if self.remotes.default_host.is_some() {
            // Remote targets keep their existing connection/status semantics.
            // In particular, never forward a pinned local path to SSH.
            self.create_default_session(directory, None);
        } else {
            self.open_local_draft(directory, cx);
        }
    }

    pub(super) fn open_local_draft(&mut self, directory: Option<String>, cx: &mut Context<Self>) {
        self.open_local_draft_kind(directory, false, cx);
    }

    pub(super) fn open_local_draft_kind(
        &mut self,
        directory: Option<String>,
        help: bool,
        cx: &mut Context<Self>,
    ) {
        let mut request_id = next_draft_id();
        if help {
            request_id = request_id.replacen("startup://draft/", "startup://draft/help/", 1);
        }
        let inserted = self.open_session(
            jcode_sdk::SessionInfo {
                session_id: request_id.clone(),
                title: Some("New session".into()),
                working_dir: directory.clone(),
                status: "starting".into(),
                transcript_bytes: None,
                saved: false,
                updated_at_ms: None,
                last_active_at_ms: None,
                archived: false,
                archived_at_ms: None,
            },
            cx,
        );
        // The editor must be hit-testable and visible in the very next frame,
        // rather than waiting for either backend creation or a width tween.
        // Snap sibling demotions too. Otherwise the old full-width panel can
        // push this full-width editor beyond the clip until its tween finishes.
        for slot in self
            .slots
            .iter_mut()
            .filter(|slot| slot.row == self.active_row)
        {
            slot.animated_width = AnimatedValue::new(
                slot.width_fraction,
                transition::policy(Transition::PanelOpen).duration,
            );
        }
        // Keep a spatial entrance without shrinking the native editor into a
        // sliver. Only offset the newcomer, so rapid spawns preserve earlier
        // entrances. The existing order tween also drives animation frames and
        // hit testing, and respects the reduced-motion PanelOpen policy.
        let slot = &mut self.slots[inserted];
        slot.order_offset =
            AnimatedValue::new(0.12, transition::policy(Transition::PanelOpen).duration);
        slot.order_offset.set(0.0, Instant::now());
        self.set_active(inserted, cx);
        // The viewport width is only known in render_strip. Resolve and snap
        // its camera there instead of interpolating an offscreen editor in.
        self.camera_snap_pending[self.active_row] = true;
        self.focus_pending = true;
        self.bridge.send(Command::CreateSession {
            working_dir: directory,
            request_id: Some(request_id),
        });
        cx.notify();
    }
}
