//! Private-compositor rendering fixture. No keyboard, microphone or voice API.
use super::*;

#[derive(serde::Deserialize)]
struct Request {
    sequence: u64,
    state: String,
}

impl Workspace {
    pub(super) fn start_global_voice_fixture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !harness::screenshot_mode() || cfg!(test) {
            return false;
        }
        let Some(path) = std::env::var_os("JCODE_DESKTOP_GLOBAL_VOICE_OVERLAY_FIXTURE") else {
            return false;
        };
        let path = PathBuf::from(path);
        cx.on_release(|this, cx| this.global_voice.shutdown(cx))
            .detach();
        self.global_voice.task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut applied = None;
            loop {
                cx.background_executor().timer(Duration::from_millis(40)).await;
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let Ok(request) = serde_json::from_slice::<Request>(&bytes) else { continue };
                if applied == Some(request.sequence) { continue; }
                let state = request.state.clone();
                let result = this.update(cx, |this, cx| -> anyhow::Result<()> {
                    let snapshot = match state.as_str() {
                        "listening" => Some(Snapshot { title: "Listening".into(), levels: Some(std::array::from_fn(|i| ((i * 7 % 23) as f32) / 120.0)), decided: false }),
                        "transcribing" => Some(Snapshot { title: "Transcribing…".into(), levels: None, decided: false }),
                        "routing" => Some(Snapshot { title: "Jev is choosing…".into(), levels: None, decided: false }),
                        "decided" => Some(Snapshot { title: "Jev → Coding agent".into(), levels: None, decided: true }),
                        "complete" => None,
                        _ => anyhow::bail!("unknown fixture state"),
                    };
                    if let Some(snapshot) = snapshot {
                        if let Some(handle) = this.global_voice.overlay {
                            handle.update(cx, |overlay, _, cx| overlay.set_snapshot(snapshot, cx))?;
                        } else {
                            this.global_voice.overlay = Some(overlay::open(snapshot, cx)?);
                        }
                    } else {
                        this.global_voice.close_overlay(cx);
                    }
                    Ok(())
                });
                let ack = match result {
                    Ok(Ok(())) => serde_json::json!({"sequence": request.sequence, "state": request.state}),
                    error => serde_json::json!({"sequence": request.sequence, "state": request.state, "error": format!("{error:?}")}),
                };
                let ack_path = PathBuf::from(format!("{}.ack", path.display()));
                let tmp = ack_path.with_extension("tmp");
                if std::fs::write(&tmp, ack.to_string()).is_ok() {
                    let _ = std::fs::rename(&tmp, &ack_path);
                }
                applied = Some(request.sequence);
            }
        }));
        true
    }
}
