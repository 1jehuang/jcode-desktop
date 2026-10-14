use super::*;

#[gpui::test]
fn workflow_stop_cancels_immediately_and_status_is_read_only(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new("workflow-control".into(), None, None, bridge, cx));
    for name in ["/improve", "/refactor"] {
        panel.update(cx, |panel, cx| {
            assert!(panel.handle_slash_command(&format!("{name} stop"), cx));
            assert!(panel.prompt_queue.paused);
            assert!(panel.pending_users.is_empty());
        });
        assert!(
            matches!(commands.try_recv(), Ok(Command::Cancel { session_id }) if session_id == "workflow-control")
        );
        panel.update(cx, |panel, cx| {
            assert!(panel.handle_slash_command(&format!("{name} status"), cx));
            assert!(panel.pending_users.is_empty());
        });
        assert!(commands.try_recv().is_err());
    }
}

#[gpui::test]
fn skills_is_a_runtime_operation_not_a_model_turn(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new("skills-session".into(), None, None, bridge, cx));
    panel.update(cx, |panel, cx| {
        assert!(panel.handle_slash_command("/skills", cx));
        assert!(panel.pending_users.is_empty());
        assert!(!panel.items.iter().any(|item| matches!(item, Item::User(_))));
    });
    assert!(
        matches!(commands.try_recv(), Ok(Command::RuntimeCommand { session_id, command: jcode_sdk::api::RuntimeCommand::Skills, args }) if session_id == "skills-session" && args.is_empty())
    );
}

#[gpui::test]
fn unknown_commands_stay_local_and_do_not_start_model_work(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new("unknown-session".into(), None, None, bridge, cx));
    panel.update(cx, |panel, cx| {
        assert!(panel.handle_slash_command("/not-a-command", cx));
        assert!(panel.pending_users.is_empty());
        assert!(matches!(panel.items.last(), Some(Item::Error(message)) if message.contains("Unknown command")));
    });
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn workflows_dispatch_expanded_prompts_and_preserve_arguments(cx: &mut gpui::TestAppContext) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| Panel::new("workflow-session".into(), None, None, bridge, cx));
    for name in [
        "/plan",
        "/test",
        "/improve",
        "/refactor",
        "/triage",
        "/feedback",
    ] {
        panel.update(cx, |panel, cx| {
            panel.pending_users.clear();
            panel.handle_slash_command(&format!("{name} target 日本語"), cx);
        });
        let Command::Send {
            session_id,
            content,
            images,
        } = commands.try_recv().unwrap()
        else {
            panic!("{name} did not dispatch a prompt");
        };
        assert_eq!(session_id, "workflow-session");
        assert!(content.contains("target 日本語"));
        assert!(!content.starts_with(name));
        assert!(images.is_empty());
    }
}
