//! Opt-in, paid-network acceptance test. Only Jev sees these synthetic fixtures.
//! The GPUI panel uses a recording bridge: no daemon, microphone, real session,
//! native window, or user's draft is touched.
//!
//! Run with configured Jcode or Typesafe credentials (the same auth as Desktop):
//! cargo test -p jcode-desktop-ui live_voice_transcript_to_jev_to_panel -- --ignored --nocapture --test-threads=1
//! The full test path is printed by `cargo test -p jcode-desktop-ui -- --list`.
use super::*;
use std::cell::RefCell;
use std::rc::Rc;

#[gpui::test]
#[ignore = "live Jev API: requires Jcode or Typesafe credentials and sends synthetic inference requests"]
fn live_voice_transcript_to_jev_to_panel(cx: &mut gpui::TestAppContext) {
    let client = jcode_base::jev::JevClient::for_voice()
        .expect("configure real Jcode or Typesafe credentials before opting into this test");
    let provider = client.provider_name();
    let model = client.model_id();
    match provider {
        "typesafe" => assert_eq!(model, "jev-latest"),
        "jcode" => assert_eq!(model, "typesafe/jev-1.13"),
        other => panic!("voice must not consume another provider account: {other}"),
    }

    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| {
        let mut panel = Panel::new("voice-live-recording-only".into(), None, None, bridge, cx);
        panel.history_loaded = true;
        panel.status = "idle".into();
        panel
    });
    let mut sessions: Vec<jcode_sdk::SessionInfo> = serde_json::from_value(serde_json::json!([
        {"session_id": "voice-fixture-orchid", "status": "idle", "title": "Orchid greenhouse irrigation planning", "working_dir": "/synthetic/greenhouse"},
        {"session_id": "voice-fixture-database", "status": "idle", "title": "Database migration debugging", "working_dir": "/synthetic/database"}
    ])).unwrap();
    // A full recent-session list used to exceed the aggregate 64 KiB guard
    // even with a short utterance because every question repeated the policy.
    for index in 2..20 {
        sessions.push(serde_json::from_value(serde_json::json!({
            "session_id": format!("voice-fixture-unrelated-{index}"),
            "status": "idle",
            "title": format!("Unrelated project notes topic {index}: documentation inventory, maintenance schedules, workspace organization, release bookkeeping, dependency catalogs, meeting summaries, asset naming conventions, routine housekeeping, and general administrative reference material."),
            "working_dir": format!("/synthetic/projects/unrelated-{index}")
        })).unwrap());
    }
    let navigations = Rc::new(RefCell::new(Vec::new()));
    let actions = Rc::new(RefCell::new(Vec::new()));
    panel.update(cx, |panel, cx| {
        panel.input.update(cx, |input, cx| {
            input.set_content(
                "unfinished typed work, not part of the voice request".into(),
                cx,
            );
        });
        panel.voice.sessions = Some(sessions.clone());
        let recorded = navigations.clone();
        panel.voice.navigation = Some(cx.subscribe(
            &cx.entity(),
            move |_, _, event: &VoiceSessionRequested, _| {
                recorded
                    .borrow_mut()
                    .push((event.0.clone(), event.1.clone()));
            },
        ));
        let recorded = actions.clone();
        panel.voice.actions = Some(cx.subscribe(
            &cx.entity(),
            move |_, _, event: &VoiceActionRequested, _| {
                recorded.borrow_mut().push((event.0, event.1.clone()));
            },
        ));
    });
    let draft = panel.read_with(cx, |panel, cx| panel.input.read(cx).content.to_string());

    for (transcript, expected) in [
        (
            "Open my existing Jcode conversation about orchid greenhouse irrigation planning",
            VoiceIntent::OpenSession(sessions[0].session_id.clone()),
        ),
        (
            "Start a new Jcode conversation",
            VoiceIntent::QuickAction(voice_intent::QuickAction::NewSession),
        ),
        (
            "Open a new session and implement login",
            VoiceIntent::CodingAgent,
        ),
    ] {
        navigations.borrow_mut().clear();
        actions.borrow_mut().clear();
        assert!(
            commands.try_recv().is_err(),
            "unexpected command before fixture"
        );
        panel.update(cx, |panel, cx| {
            panel.prepare_voice_attempt(true);
            panel.voice.phase = Phase::Transcribing;
            // This is the production final-transcript boundary. Do NOT inject a
            // VoiceIntent/report or call resolve_voice_for_test here.
            panel.finish_voice(Ok(transcript.into()), cx);
            assert!(panel.voice.phase == Phase::Routing);
            assert!(panel.voice.task.is_some());
            let trace = panel
                .voice
                .trace
                .as_ref()
                .expect("production request trace");
            assert_eq!(trace.transcript, transcript);
            assert!(trace.answers.is_empty(), "network has not completed yet");
        });

        // GPUI's deterministic executor runs the actual background task, whose
        // production Tokio runtime performs bounded real HTTPS requests, then
        // runs the foreground finish_voice_report continuation and UI events.
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert!(panel.voice.phase == Phase::Idle, "production routing did not finish");
            if let Some(trace) = &panel.voice.trace {
                eprintln!("LIVE DESKTOP {provider}/{model}: {transcript:?} answers={:?}", trace.answers);
            }
            assert!(panel.voice.error.is_none(), "live Jev routing failed: {:?}", panel.voice.error);
            let trace = panel.voice.trace.as_ref().expect("actual report retained in UI");
            assert_eq!(trace.transcript, transcript);
            assert_eq!(trace.candidates.len(), sessions.len());
            assert_eq!(trace.questions.len(), 7 + sessions.len());
            assert_eq!(trace.answers.len(), trace.questions.len());
            for question in &trace.questions {
                let matching: Vec<_> = trace.answers.iter().filter(|answer| answer.id == question.id).collect();
                assert_eq!(matching.len(), 1, "exactly one real answer per question");
                assert!((0.0..=1.0).contains(&matching[0].probability));
            }
            assert_eq!(panel.input.read(cx).content.as_ref(), draft);
            assert!(panel.voice.live_transcript.is_empty());
            let decision = panel.voice.decision.as_deref().expect("UI decision label");
            assert!(decision.starts_with("Jev chose:"));
            eprintln!("LIVE DESKTOP {provider}/{model}: {transcript:?} => {decision}, {} validated answers", trace.answers.len());
        });
        match expected {
            VoiceIntent::OpenSession(id) => {
                let events = navigations.borrow();
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].0.session_id, id);
                assert_eq!(events[0].0.working_dir, sessions[0].working_dir);
                assert_eq!(events[0].1, transcript);
                assert!(actions.borrow().is_empty());
                assert!(
                    commands.try_recv().is_err(),
                    "navigation must not send to coding daemon"
                );
            }
            VoiceIntent::QuickAction(action) => {
                assert_eq!(&*actions.borrow(), &[(action, transcript.to_string())]);
                assert!(navigations.borrow().is_empty());
                assert!(
                    commands.try_recv().is_err(),
                    "quick action must not send to coding daemon"
                );
            }
            VoiceIntent::CodingAgent => {
                assert!(navigations.borrow().is_empty());
                assert!(
                    actions.borrow().is_empty(),
                    "mixed coding request must not navigate"
                );
                assert!(
                    matches!(commands.try_recv(), Ok(Command::Send { session_id, content, images })
                    if session_id == "voice-live-recording-only" && content == transcript && images.is_empty()),
                    "recording bridge must receive only the spoken coding request"
                );
                assert!(commands.try_recv().is_err(), "exactly one recorded send");
            }
            unexpected => panic!("unsupported fixture {unexpected:?}"),
        }
        cx.run_until_parked();
        assert!(commands.try_recv().is_err(), "no delayed duplicate send");
    }
}
