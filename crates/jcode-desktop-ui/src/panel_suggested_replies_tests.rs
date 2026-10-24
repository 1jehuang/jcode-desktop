use super::*;
fn panel_for_test(
    cx: &mut gpui::TestAppContext,
) -> (Entity<Panel>, std::sync::mpsc::Receiver<Command>) {
    let (bridge, commands) = crate::harness::spawn_recording();
    let panel = cx.new(|cx| {
        let mut p = Panel::new("suggestions-test".into(), None, None, bridge, cx);
        p.history_loaded = true;
        p.prompt_queue.set_auto_poke_enabled_for_test(false);
        p.suggested_replies.enabled_override = Some(true);
        p
    });
    cx.update(|cx| Panel::connect_input(&panel, cx));
    (panel, commands)
}

fn finish_turn_with_assistant_reply(p: &mut Panel, reply: &str, cx: &mut Context<Panel>) {
    p.pending_users.clear();
    p.items.push(Item::Assistant(reply.to_string()));
    p.apply(
        &ApiEvent::TurnDone {
            session_id: p.session_id.clone(),
            pending_soft_interrupts: None,
        },
        cx,
    );
}

fn drain(commands: &std::sync::mpsc::Receiver<Command>) -> Vec<Command> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

fn sent(commands: &[Command]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Send { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn turn_done_populates_suggested_replies_and_clears_on_next_submission(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, commands) = panel_for_test(cx);
    panel.update(cx, |p, cx| {
        p.submit_or_queue(
            "Implement the parser and ask me which option to run next.".into(),
            vec![],
            false,
            cx,
        );
        finish_turn_with_assistant_reply(
            p,
            "Choose next step:\n1. Run cargo test on parser\n2. Inspect benchmark results",
            cx,
        );
    });
    cx.run_until_parked();
    let _ = drain(&commands);

    panel.read_with(cx, |p, _| {
        let pills = p.displayed_suggestion_pills();
        assert!(
            !pills.is_empty(),
            "expected suggested reply pills after TurnDone"
        );
        assert!(
            pills
                .iter()
                .any(|pill| pill.text.contains("Run cargo test on parser")),
            "expected extracted option in pills, got: {pills:?}"
        );
    });

    panel.update(cx, |p, cx| {
        p.submit_or_queue("Manual follow-up".into(), vec![], false, cx);
    });
    panel.read_with(cx, |p, _| {
        assert!(
            p.displayed_suggestion_pills().is_empty(),
            "submitting a new prompt must clear active suggested replies"
        );
    });
}

#[gpui::test]
fn staging_editing_detaching_and_canceling_suggestion_focus(cx: &mut gpui::TestAppContext) {
    let (panel, _commands) = panel_for_test(cx);
    panel.update(cx, |p, cx| {
        p.items.push(Item::User("Check status".into()));
        finish_turn_with_assistant_reply(
            p,
            "Next options:\n1. Run unit tests\n2. Update documentation",
            cx,
        );
    });
    cx.run_until_parked();

    let first_text = panel.read_with(cx, |p, _| {
        let pills = p.displayed_suggestion_pills();
        assert!(!pills.is_empty());
        pills[0].text.clone()
    });

    // Simulate staging pill 0 and then editing input text: focus/staging must detach.
    panel.update(cx, |p, cx| {
        p.suggested_replies.focused_index = Some(0);
        p.suggested_replies.staged_text = Some(first_text.clone());
        p.input.update(cx, |input, cx| {
            input.set_content(first_text.clone(), cx);
        });
    });

    // Unmodified cancel clears input and focus.
    panel.update(cx, |p, cx| {
        assert!(p.cancel_suggested_reply_focus(cx));
    });
    panel.read_with(cx, |p, cx| {
        assert_eq!(p.suggested_replies.focused_index, None);
        assert_eq!(p.suggested_replies.staged_text, None);
        assert_eq!(p.input.read(cx).content.as_ref(), "");
    });

    // Stage again, then edit text: detach_if_edited drops focus while keeping edited text.
    panel.update(cx, |p, cx| {
        p.suggested_replies.focused_index = Some(0);
        p.suggested_replies.staged_text = Some(first_text.clone());
        let edited = format!("{first_text} --nocapture");
        p.input.update(cx, |input, cx| {
            input.set_content(edited.clone(), cx);
        });
        p.on_input_changed_for_suggestions(&edited, cx);
    });
    panel.read_with(cx, |p, cx| {
        assert_eq!(p.suggested_replies.focused_index, None);
        assert_eq!(p.suggested_replies.staged_text, None);
        assert!(p.input.read(cx).content.ends_with("--nocapture"));
    });
}

#[gpui::test]
fn suggestions_slash_command_supports_on_off_toggle_status_and_refresh(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, _commands) = panel_for_test(cx);
    panel.update(cx, |p, cx| {
        p.items.push(Item::User("Fix bug".into()));
        finish_turn_with_assistant_reply(p, "1. Run tests\n2. Commit fix", cx);
    });
    cx.run_until_parked();

    panel.update(cx, |p, cx| {
        assert!(p.handle_slash_command_for_test("/suggestions status", cx));
        assert!(p.handle_slash_command_for_test("/suggestions off", cx));
    });
    panel.read_with(cx, |p, _| {
        assert_eq!(p.suggested_replies.enabled_override, Some(false));
        assert!(p.displayed_suggestion_pills().is_empty());
    });

    panel.update(cx, |p, cx| {
        assert!(p.handle_slash_command_for_test("/suggestions on", cx));
    });
    panel.read_with(cx, |p, _| {
        assert_eq!(p.suggested_replies.enabled_override, Some(true));
        assert!(!p.displayed_suggestion_pills().is_empty());
    });
}

#[gpui::test]
fn two_step_tab_and_pill_click_stage_then_submit_in_window(cx: &mut gpui::TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (bridge, commands) = crate::harness::spawn_recording();
    let (panel, vcx) = cx.add_window_view(|_, cx| {
        let mut p = Panel::new("window-suggestions-test".into(), None, None, bridge, cx);
        p.history_loaded = true;
        p.prompt_queue.set_auto_poke_enabled_for_test(false);
        p.suggested_replies.enabled_override = Some(true);
        p
    });
    vcx.update(|_, cx| Panel::connect_input(&panel, cx));
    vcx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            let focus = p.input.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            p.items.push(Item::User("What next?".into()));
            finish_turn_with_assistant_reply(
                p,
                "Options:\n1. Run cargo check\n2. Inspect logs",
                cx,
            );
        });
    });
    vcx.run_until_parked();
    let _ = drain(&commands);

    let (expected_first, expected_second) = panel.read_with(vcx, |p, _| {
        let pills = p.displayed_suggestion_pills();
        assert!(
            pills.len() >= 2,
            "expected at least 2 pills, got: {pills:?}"
        );
        (pills[0].text.clone(), pills[1].text.clone())
    });

    // 1) Real GPUI "tab" keystroke stages the Default Action into PromptInput without sending.
    vcx.update(|window, cx| {
        window.dispatch_keystroke(gpui::Keystroke::parse("tab").unwrap(), cx);
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |p, cx| {
        assert_eq!(p.input.read(cx).content.as_ref(), expected_first);
        assert_eq!(p.suggested_replies.focused_index, Some(0));
    });
    assert!(sent(&drain(&commands)).is_empty());

    // 2) Real GPUI "escape" keystroke cancels suggestion focus and clears the unmodified staged text.
    vcx.update(|window, cx| {
        window.dispatch_keystroke(gpui::Keystroke::parse("escape").unwrap(), cx);
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |p, cx| {
        assert_eq!(p.input.read(cx).content.as_ref(), "");
        assert_eq!(p.suggested_replies.focused_index, None);
    });

    // 3) First click on secondary pill (index 1) stages it; second click on the same pill submits it.
    vcx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.handle_suggested_reply_pill_click(1, window, cx);
        });
    });
    panel.read_with(vcx, |p, cx| {
        assert_eq!(p.input.read(cx).content.as_ref(), expected_second);
        assert_eq!(p.suggested_replies.focused_index, Some(1));
    });
    assert!(sent(&drain(&commands)).is_empty());

    vcx.update(|window, cx| {
        panel.update(cx, |p, cx| {
            p.handle_suggested_reply_pill_click(1, window, cx);
        });
    });
    let submitted = sent(&drain(&commands));
    assert_eq!(submitted, vec![expected_second]);

    // 4) CJK display-cell truncation bounds wide glyphs by cell width rather than char count.
    assert_eq!(truncate_pill_text("日本語テスト文字列", 10), "日本語テ…");
}
