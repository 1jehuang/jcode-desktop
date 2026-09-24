//! No devices, network, daemon or live windows are used by these regressions.
use super::*;

#[gpui::test]
fn global_voice_sends_transcript_and_pill_says_so(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new("global-voice".into(), None, None, bridge, cx));
    panel.update(cx, |panel, cx| {
        panel.input.update(cx, |input, cx| {
            input.set_content("keep my draft".into(), cx)
        });
        panel.prepare_voice_attempt(true);
        panel.voice.global_capture = true;
        panel.voice.phase = Phase::Transcribing;
        let attempt = panel.voice.canceled.clone();
        panel.finish_global_voice_text(&attempt, "note this down".into(), None, true, cx);
        assert!(panel.voice.phase == Phase::Idle);
        assert_eq!(panel.input.read(cx).content.as_ref(), "keep my draft");
        assert!(
            panel
                .items
                .iter()
                .any(|item| matches!(item, Item::User(text) if text == "note this down"))
        );
        let _ = commands;
        let pill = panel.global_voice_snapshot(&attempt).unwrap();
        assert_eq!(pill.title, "Sent to agent");
        assert!(pill.decided);
    });
}

#[gpui::test]
fn global_voice_pill_labels_jev_decisions(cx: &mut gpui::TestAppContext) {
    let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    panel.update(cx, |panel, cx| {
        panel.prepare_voice_attempt(true);
        panel.voice.global_capture = true;
        let attempt = panel.voice.canceled.clone();
        panel.voice.phase = Phase::Routing;
        let pill = panel.global_voice_snapshot(&attempt).unwrap();
        assert_eq!(pill.title, "Jev is choosing…");
        assert!(!pill.decided);

        panel.voice.phase = Phase::Idle;
        for (decision, title) in [
            (
                "Jev chose: Coding agent · Sent now",
                "Jev → Coding agent",
            ),
            ("Jev chose: Quick action · Navigation", "Jev → Quick action"),
            ("Jev chose: Quick action · New session", "Jev → New session"),
            (
                "Jev chose: Quick action · Open session",
                "Jev → Open session",
            ),
        ] {
            panel.voice.error = None;
            panel.voice.decision = Some(decision.into());
            let pill = panel.global_voice_snapshot(&attempt).unwrap();
            assert_eq!(pill.title, title);
            assert!(pill.decided);
        }
        panel.voice.decision = Some("Jev chose: Quick action · Navigation".into());
        panel.set_global_voice_decision("Jev chose: Quick action · Previous session".into(), cx);
        assert_eq!(
            panel.global_voice_snapshot(&attempt).unwrap().title,
            "Jev → Previous session"
        );
    });
}

#[gpui::test]
fn focused_hold_ignores_global_decision_refinement(cx: &mut gpui::TestAppContext) {
    let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    panel.update(cx, |panel, cx| {
        panel.voice.decision = Some("Jev chose: Quick action · Navigation".into());
        panel.set_global_voice_decision("Jev chose: Quick action · New session".into(), cx);
        assert_eq!(
            panel.voice.decision.as_deref(),
            Some("Jev chose: Quick action · Navigation")
        );
    });
}

#[gpui::test]
fn global_voice_old_attempt_cannot_stop_or_cancel_new_local_hold(cx: &mut gpui::TestAppContext) {
    let panel = cx.new(|cx| Panel::new_preview(PreviewState::Empty, cx));
    panel.update(cx, |panel, cx| {
        panel.prepare_voice_attempt(true);
        panel.voice.global_capture = true;
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
        panel.voice.global_capture = true;
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
        panel.voice.global_capture = true;
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
        panel.voice.global_capture = true;
        panel.voice.phase = Phase::Transcribing;
        let denied = panel.voice.canceled.clone();
        panel.finish_global_voice_text(&denied, "hidden speech".into(), None, false, cx);
        assert!(denied.load(Ordering::SeqCst));
        assert_eq!(panel.input.read(cx).content.as_ref(), "original");
        panel.prepare_voice_attempt(true);
        panel.voice.global_capture = true;
        panel.voice.phase = Phase::Transcribing;
        let current = panel.voice.canceled.clone();
        panel.finish_global_voice_text(&denied, "old speech".into(), None, true, cx);
        panel.finish_global_voice_text(&current, "new speech".into(), None, true, cx);
        let after = panel.items.len();
        panel.finish_global_voice_text(&current, "new speech".into(), None, true, cx);
        assert_eq!(panel.items.len(), after, "sent exactly once");
        assert_eq!(panel.input.read(cx).content.as_ref(), "original");
        let sent: Vec<_> = panel
            .items
            .iter()
            .filter_map(|item| match item {
                Item::User(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(sent.contains(&"new speech"));
        assert!(!sent.contains(&"old speech") && !sent.contains(&"hidden speech"));
    });
}
