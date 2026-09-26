use super::*;

fn history(pairs: &[(&str, &str)]) -> crate::persisted_history::History {
    let messages = pairs
        .iter()
        .map(|(role, content)| jcode_sdk::HistoryMessage {
            response_stats: None,
            role: (*role).into(),
            content: (*content).into(),
        })
        .collect();
    (messages, Vec::new())
}

fn texts(panel: &Panel) -> Vec<String> {
    panel
        .items
        .iter()
        .filter_map(|item| match item {
            Item::User(text) | Item::Assistant(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

struct Host(Entity<Panel>);
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

fn open(cx: &mut gpui::TestAppContext) -> (Entity<Panel>, &mut gpui::VisualTestContext) {
    let (host, vcx) = cx.add_window_view(|_, cx| {
        Host(cx.new(|cx| {
            Panel::new(
                "session-provisional".into(),
                None,
                None,
                crate::harness::spawn_inert(),
                cx,
            )
        }))
    });
    let panel = host.read_with(vcx, |host, _| host.0.clone());
    (panel, vcx)
}

#[gpui::test]
fn stored_transcript_paints_before_attach_and_identical_reply_is_a_no_op(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = open(cx);
    let stored = history(&[("user", "question"), ("assistant", "answer")]);
    let reply = stored.clone();
    panel.update(vcx, |panel, cx| {
        panel.prefetch_history_with(move || Ok(stored), cx);
        // No new-chat prompt flashes while the stored copy is read.
        assert!(panel.awaiting_persisted);
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("fresh-session").is_none());
    panel.read_with(vcx, |panel, _| {
        assert_eq!(texts(panel), ["question", "answer"]);
        assert!(!panel.history_loaded());
        assert_eq!(panel.provisional_items, 2);
    });
    panel.update(vcx, |panel, cx| {
        panel.load_history(reply.0, reply.1, cx);
    });
    panel.read_with(vcx, |panel, _| {
        assert_eq!(texts(panel), ["question", "answer"]);
        assert!(panel.history_loaded());
        assert_eq!(panel.provisional_items, 0);
    });
}

#[gpui::test]
fn authoritative_history_replaces_a_stale_stored_copy_and_keeps_local_echo(
    cx: &mut gpui::TestAppContext,
) {
    let (panel, vcx) = open(cx);
    let stored = history(&[("user", "old question"), ("assistant", "old answer")]);
    panel.update(vcx, |panel, cx| {
        panel.prefetch_history_with(move || Ok(stored), cx)
    });
    vcx.run_until_parked();
    // A prompt echoed locally after the provisional paint stays at the end.
    panel.update(vcx, |panel, _| {
        let index = panel.items.len();
        panel.items.push(Item::User("typed while loading".into()));
        panel.pending_users.push_back(index);
    });
    let (messages, images) = history(&[
        ("user", "old question"),
        ("assistant", "old answer"),
        ("user", "newer"),
        ("assistant", "newer answer"),
    ]);
    panel.update(vcx, |panel, cx| panel.load_history(messages, images, cx));
    panel.read_with(vcx, |panel, _| {
        assert_eq!(
            texts(panel),
            [
                "old question",
                "old answer",
                "newer",
                "newer answer",
                "typed while loading"
            ]
        );
        assert_eq!(panel.pending_users.iter().copied().collect::<Vec<_>>(), [4]);
    });
}

#[gpui::test]
fn authoritative_history_that_arrives_first_wins(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = open(cx);
    let stored = history(&[("user", "stale")]);
    let (messages, images) = history(&[("user", "live")]);
    panel.update(vcx, |panel, cx| {
        panel.prefetch_history_with(move || Ok(stored), cx);
        panel.load_history(messages, images, cx);
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, _| {
        assert_eq!(texts(panel), ["live"]);
        assert_eq!(panel.provisional_items, 0);
        assert!(!panel.awaiting_persisted);
    });
}

#[gpui::test]
fn unreadable_stored_copy_falls_back_to_the_normal_empty_state(cx: &mut gpui::TestAppContext) {
    let (panel, vcx) = open(cx);
    panel.update(vcx, |panel, cx| {
        panel.prefetch_history_with(|| Err("missing".into()), cx)
    });
    vcx.run_until_parked();
    panel.read_with(vcx, |panel, _| {
        assert!(panel.items.is_empty());
        assert!(!panel.awaiting_persisted);
    });
}
