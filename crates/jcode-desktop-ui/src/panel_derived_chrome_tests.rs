use super::*;

fn payload() -> TodoCardPayload {
    TodoCardPayload {
        todos: vec![TodoCardItem {
            content: "Keep chrome allocations bounded".into(),
            status: "pending".into(),
            group: None,
            blocked_by: vec![],
            confidence: None,
            completion_confidence: None,
        }],
        plan: TodoCardPlan::default(),
        goals: vec![],
    }
}

#[gpui::test]
fn derived_chrome_shares_snapshots_and_releases_replaced_payload(cx: &mut gpui::TestAppContext) {
    let panel = cx.update(|cx| {
        cx.new(|cx| {
            Panel::new(
                "derived-chrome".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        })
    });
    panel.update(cx, |panel, cx| {
        panel
            .items
            .extend([Item::User("Prompt".into()), Item::Todos(payload())]);
        let derived = panel.transcript_derived(cx);
        let prompts = derived.prompt_rows.clone();
        let todo = derived.latest_todo.clone().unwrap();
        let headers = derived.tail_headers;
        for _ in 0..20 {
            let derived = panel.transcript_derived(cx);
            assert!(Arc::ptr_eq(&prompts, &derived.prompt_rows));
            assert!(Arc::ptr_eq(&todo, derived.latest_todo.as_ref().unwrap()));
            assert_eq!(headers, derived.tail_headers);
        }
        // Row derivation invalidates, but settled todo state must not be
        // cloned/scanned/formatted again while text is streaming.
        panel.streaming_text.push_str("A live response");
        let derived = panel.transcript_derived(cx);
        assert!(Arc::ptr_eq(&prompts, &derived.prompt_rows));
        assert!(Arc::ptr_eq(&todo, derived.latest_todo.as_ref().unwrap()));
        assert_eq!(headers, derived.tail_headers);
        panel.expanded_tools.insert("disclosure".into());
        let derived = panel.transcript_derived(cx);
        assert!(Arc::ptr_eq(&prompts, &derived.prompt_rows));
        assert!(Arc::ptr_eq(&todo, derived.latest_todo.as_ref().unwrap()));

        panel.applet_rows.push((0, "fixture-applet".into()));
        panel.applet_rows_version += 1;
        let derived = panel.transcript_derived(cx);
        assert!(!Arc::ptr_eq(&prompts, &derived.prompt_rows));
        assert_eq!(derived.prompt_rows.as_slice(), &[(1, 0)]);
        assert!(Arc::ptr_eq(&todo, derived.latest_todo.as_ref().unwrap()));

        let weak = Arc::downgrade(&todo);
        let mut replacement = payload();
        replacement.todos[0].status = "completed".into();
        panel.items.push(Item::Todos(replacement));
        let derived = panel.transcript_derived(cx);
        assert!(!Arc::ptr_eq(&todo, derived.latest_todo.as_ref().unwrap()));
        assert_ne!(headers[0], derived.tail_headers[0]);
        assert_eq!(headers[1], derived.tail_headers[1]);
        drop(todo);
        assert!(
            weak.upgrade().is_none(),
            "cache must not retain past snapshots"
        );
    });
}

#[gpui::test]
fn lowercase_activity_status_borrows_normal_snapshots(cx: &mut gpui::TestAppContext) {
    let panel = cx.update(|cx| {
        cx.new(|cx| {
            Panel::new(
                "status-normalization".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        })
    });
    panel.update(cx, |panel, _| {
        panel.connection_phase.clear();
        for (status, active) in [
            ("idle", false),
            ("streaming", true),
            ("RUNNING_TOOLS", true),
            ("Error: disconnected", false),
            ("CRASHED", false),
            ("Lost: session", false),
            ("café idle", false),
        ] {
            panel.status = status.into();
            let normalized = panel.lowercase_status();
            assert_eq!(normalized, status.to_ascii_lowercase());
            assert_eq!(
                matches!(normalized, std::borrow::Cow::Borrowed(_)),
                !status.bytes().any(|byte| byte.is_ascii_uppercase())
            );
            assert_eq!(panel.activity_active(), active, "{status}");
        }
        panel.status = "idle".into();
        panel.streaming_text = "new live response".into();
        assert!(panel.activity_active());
        panel.status = "ERROR".into();
        assert!(!panel.activity_active());
    });
}

#[test]
fn cached_tail_headers_match_existing_chrome_fingerprints() {
    use std::hash::{Hash, Hasher};
    let filled = payload();
    let empty = TodoCardPayload::default();
    for payload in [None, Some(&empty), Some(&filled)] {
        let headers = transcript_tail_headers(payload);
        for publish_tracker in [false, true] {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            publish_tracker.hash(&mut hasher);
            if let Some(payload) = payload.filter(|p| !p.todos.is_empty() && !publish_tracker) {
                format!("{payload:?}").hash(&mut hasher);
            }
            assert_eq!(headers[usize::from(publish_tracker)], hasher.finish());
        }
    }
}
