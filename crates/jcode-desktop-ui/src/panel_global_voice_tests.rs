//! No devices, network, daemon or live windows are used by these regressions.
use super::*;

#[gpui::test]
fn global_voice_final_text_is_draft_only_even_with_stale_routing(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new("global-voice".into(), None, None, bridge, cx));
    panel.update(cx, |panel, cx| {
        panel.input.update(cx, |input, cx| {
            input.set_content("keep my draft".into(), cx)
        });
        panel.voice.sessions = Some(Vec::new());
        panel.prepare_voice_attempt(true);
        panel.voice.dictation_only = true;
        panel.voice.phase = Phase::Transcribing;
        let attempt = panel.voice.canceled.clone();
        panel.finish_global_voice_text(&attempt, "please delete the repository".into(), true, cx);
        assert!(
            panel.voice.phase == Phase::Idle,
            "must not enter intent routing"
        );
        assert!(
            panel.voice.task.is_none(),
            "must not start provider classification"
        );
        assert!(panel.input.read(cx).content.starts_with("keep my draft"));
        assert!(
            panel
                .input
                .read(cx)
                .content
                .ends_with("please delete the repository")
        );
        assert!(
            commands.try_recv().is_err(),
            "must never submit background dictation"
        );
    });
}

#[gpui::test]
fn global_voice_old_attempt_cannot_stop_or_cancel_new_local_hold(cx: &mut gpui::TestAppContext) {
    let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    panel.update(cx, |panel, cx| {
        panel.prepare_voice_attempt(true);
        panel.voice.dictation_only = true;
        let old = panel.voice.canceled.clone();
        panel.prepare_voice_attempt(true);
        let local = panel.voice.canceled.clone();
        panel.end_global_voice_hold(&old, cx);
        panel.cancel_global_voice(&old, cx);
        assert!(Arc::ptr_eq(&local, &panel.voice.canceled));
        assert!(panel.voice.phase == Phase::Checking);
        assert!(!local.load(Ordering::SeqCst));
        assert!(panel.global_voice_snapshot(&old).is_none());
    });
}

#[gpui::test]
fn global_voice_safety_cancel_discards_partial_text_and_ignores_late_completion(
    cx: &mut gpui::TestAppContext,
) {
    let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    panel.update(cx, |panel, cx| {
        panel
            .input
            .update(cx, |input, cx| input.set_content("unchanged".into(), cx));
        panel.prepare_voice_attempt(true);
        panel.voice.dictation_only = true;
        panel.voice.phase = Phase::Transcribing;
        panel.voice.live_transcript = "partial private speech".into();
        let attempt = panel.voice.canceled.clone();
        panel.cancel_global_voice(&attempt, cx);
        assert!(attempt.load(Ordering::SeqCst));
        assert!(panel.voice.phase == Phase::Idle);
        assert!(panel.voice.live_transcript.is_empty());
        assert_eq!(panel.input.read(cx).content.as_ref(), "unchanged");
        panel.finish_voice_startup(&attempt, Err(VoiceError::CaptureFailed), cx);
        assert!(panel.voice.error.is_none());
        assert!(panel.global_voice_snapshot(&attempt).is_none());
    });
}

#[gpui::test]
fn global_voice_release_while_connecting_cancels_and_snapshot_tracks_real_phase(
    cx: &mut gpui::TestAppContext,
) {
    let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    panel.update(cx, |panel, cx| {
        panel.prepare_voice_attempt(true);
        panel.voice.dictation_only = true;
        let attempt = panel.voice.canceled.clone();
        assert_eq!(
            panel.global_voice_snapshot(&attempt).unwrap().title,
            "Connecting…"
        );
        panel.voice.phase = Phase::Recording;
        panel.voice.levels = [0.2; 24];
        assert_eq!(
            panel.global_voice_snapshot(&attempt).unwrap().levels,
            Some([0.2; 24])
        );
        panel.voice.phase = Phase::Checking;
        panel.end_global_voice_hold(&attempt, cx);
        assert!(attempt.load(Ordering::SeqCst));
        assert!(panel.global_voice_snapshot(&attempt).is_none());
    });
}

#[gpui::test]
fn global_voice_final_delivery_requires_fresh_permission_and_is_exactly_once(
    cx: &mut gpui::TestAppContext,
) {
    let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    panel.update(cx, |panel, cx| {
        panel
            .input
            .update(cx, |input, cx| input.set_content("original".into(), cx));
        panel.prepare_voice_attempt(true);
        panel.voice.dictation_only = true;
        panel.voice.phase = Phase::Transcribing;
        let denied = panel.voice.canceled.clone();
        panel.finish_global_voice_text(&denied, "hidden speech".into(), false, cx);
        assert!(denied.load(Ordering::SeqCst));
        assert_eq!(panel.input.read(cx).content.as_ref(), "original");
        panel.prepare_voice_attempt(true);
        panel.voice.dictation_only = true;
        panel.voice.phase = Phase::Transcribing;
        let current = panel.voice.canceled.clone();
        panel.finish_global_voice_text(&denied, "old speech".into(), true, cx);
        panel.finish_global_voice_text(&current, "new speech".into(), true, cx);
        let after = panel.input.read(cx).content.clone();
        panel.finish_global_voice_text(&current, "new speech".into(), true, cx);
        assert_eq!(panel.input.read(cx).content, after);
        assert!(!after.contains("old speech"));
        assert!(after.ends_with("new speech"));
    });
}
