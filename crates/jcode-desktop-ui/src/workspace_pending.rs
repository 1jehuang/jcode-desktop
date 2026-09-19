//! Session creation never holds up the native editor. Draft IDs also
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

pub(super) fn next_remote_draft_id(host: &str) -> String {
    next_draft_id().replacen(
        "startup://draft/",
        &format!("startup://draft/remote/{host}/"),
        1,
    )
}

/// The destination travels with the draft through reloads and default-machine
/// changes. Valid SSH destinations cannot contain '/', so no escaping is needed.
pub(super) fn remote_draft_host(id: &str) -> Option<&str> {
    let (host, request) = id
        .strip_prefix("startup://draft/remote/")?
        .split_once('/')?;
    (!request.is_empty() && crate::remote_targets::validate_host(host).is_ok()).then_some(host)
}

impl Workspace {
    pub(super) fn open_default_draft(&mut self, directory: Option<String>, cx: &mut Context<Self>) {
        if let Some(host) = self.remotes.default_host.clone() {
            self.open_remote_draft(host, cx);
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
        self.mount_session_draft(
            request_id.clone(),
            directory.clone(),
            "New session".into(),
            cx,
        );
        self.bridge.send(Command::CreateSession {
            working_dir: directory,
            request_id: Some(request_id),
        });
    }

    pub(super) fn open_remote_draft(&mut self, host: String, cx: &mut Context<Self>) {
        let request_id = next_remote_draft_id(&host);
        self.remotes.failed = false;
        self.remotes.status = Some(format!("Preparing connection to {host}…"));
        self.mount_session_draft(
            request_id.clone(),
            None,
            format!("Connecting to {host}…"),
            cx,
        );
        let panel = self.slots[self.active].panel.clone();
        panel.update(cx, |panel, cx| {
            panel.status = format!("Preparing connection to {host}…");
            cx.notify();
        });
        self.create_remote_draft_session(host, request_id, cx);
    }

    fn mount_session_draft(
        &mut self,
        request_id: String,
        directory: Option<String>,
        title: String,
        cx: &mut Context<Self>,
    ) {
        let inserted = self.open_session(
            jcode_sdk::SessionInfo {
                session_id: request_id.clone(),
                title: Some(title),
                working_dir: directory.clone(),
                status: "starting".into(),
                transcript_bytes: None,
                saved: false,
                updated_at_ms: None,
                last_active_at_ms: None,
                archived: false,
                archived_at_ms: None,
                parent_session_id: None,
                agent_label: None,
                swarm_status: None,
                edit_stats: None,
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
        cx.notify();
    }
}
