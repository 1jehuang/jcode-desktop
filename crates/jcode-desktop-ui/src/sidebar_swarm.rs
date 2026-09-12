//! Swarm ownership is distinct from transcript ancestry: user forks stay roots.
use super::*;
use jcode_sdk::SessionInfo;

pub(super) const PREVIEW: usize = 2;

#[derive(Clone)]
pub(super) struct Child {
    pub session: SessionInfo,
    pub depth: usize,
}

#[derive(Default)]
pub(super) struct Groups {
    pub children: HashMap<String, Vec<Child>>,
    pub nested: HashSet<String>,
}

pub(super) fn groups(sessions: &[SessionInfo]) -> Groups {
    let by_id = sessions
        .iter()
        .map(|s| (s.session_id.as_str(), s))
        .collect::<HashMap<_, _>>();
    let mut result = Groups::default();
    for session in sessions {
        let mut current = session;
        let mut seen = HashSet::from([session.session_id.as_str()]);
        let mut depth = 0;
        let mut cycle = false;
        while let Some(parent) = current
            .parent_session_id
            .as_deref()
            .and_then(|id| by_id.get(id))
        {
            if !seen.insert(parent.session_id.as_str()) {
                cycle = true;
                break;
            }
            depth += 1;
            current = parent;
        }
        // Missing parents remain visible. Malformed cycles must never hide sessions.
        if depth > 0 && !cycle {
            result.nested.insert(session.session_id.clone());
            result
                .children
                .entry(current.session_id.clone())
                .or_default()
                .push(Child {
                    session: session.clone(),
                    depth,
                });
        }
    }
    // Depth-first order keeps grandchildren beside their immediate coordinator.
    for children in result.children.values_mut() {
        let original = std::mem::take(children);
        fn append(parent: &str, source: &[Child], target: &mut Vec<Child>) {
            for child in source
                .iter()
                .filter(|c| c.session.parent_session_id.as_deref() == Some(parent))
            {
                target.push(child.clone());
                append(&child.session.session_id, source, target);
            }
        }
        let root = original.first().map(|c| {
            let mut current = &c.session;
            while let Some(parent) = current
                .parent_session_id
                .as_deref()
                .and_then(|id| by_id.get(id))
            {
                current = parent;
            }
            current.session_id.clone()
        });
        if let Some(root) = root {
            append(&root, &original, children);
        }
    }
    result
}

fn status(session: &SessionInfo) -> (&str, bool) {
    let status = session.swarm_status.as_deref().unwrap_or(&session.status);
    match status {
        "running" | "working" | "active" | "busy" => ("working", true),
        "completed" | "done" => ("done", false),
        "ready" => ("ready", false),
        "blocked" | "waiting" => ("waiting", false),
        "failed" | "error" | "crashed" => ("failed", false),
        "stopped" | "closed" => ("stopped", false),
        _ => ("idle", false),
    }
}

impl Workspace {
    pub(super) fn render_swarm_children(
        &self,
        root_id: &str,
        children: &[Child],
        active_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let expanded = self.expanded_swarms.contains(root_id);
        let working = children.iter().filter(|c| status(&c.session).1).count();
        let root = root_id.to_owned();
        let mut content = div()
            .mt_1()
            .ml(px(20.0))
            .pl_2()
            .border_l_1()
            .border_color(Theme::global().PANEL_BORDER)
            .flex()
            .flex_col()
            .gap(px(2.0));
        content = content.child(
            div()
                .text_size(px(9.0))
                .text_color(Theme::global().TEXT_DIM)
                .child(format!("{} agents · {} working", children.len(), working)),
        );
        for child in children
            .iter()
            .take(if expanded { usize::MAX } else { PREVIEW })
        {
            let session = child.session.clone();
            let id = session.session_id.clone();
            let selector = id.clone();
            let title = session
                .agent_label
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| sidebar_session_title(&session).1)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let (state, working) = status(&session);
            let color = if state == "failed" {
                Theme::global().ERROR
            } else if working {
                Theme::global().ACCENT
            } else {
                Theme::global().TEXT_DIM
            };
            let state = state.to_owned();
            content = content.child(
                div()
                    .id(gpui::SharedString::from(format!("swarm-child-{id}")))
                    .debug_selector(move || format!("swarm-child-{selector}"))
                    .h(px(19.0))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .ml(px((child.depth.saturating_sub(1).min(3) * 8) as f32))
                    .when(active_id == Some(id.as_str()), |el| {
                        el.bg(Theme::global().ACCENT_DIM)
                    })
                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.sidebar_gesture = None;
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.activate_session(session.clone(), window, cx);
                        }),
                    )
                    .child(div().text_size(px(8.0)).text_color(color).child("●"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(10.0))
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(9.0))
                            .text_color(color)
                            .child(state),
                    ),
            );
        }
        if children.len() > PREVIEW {
            let selector = root.clone();
            content = content.child(
                div()
                    .id(gpui::SharedString::from(format!("swarm-toggle-{root}")))
                    .debug_selector(move || format!("swarm-toggle-{selector}"))
                    .text_size(px(9.0))
                    .text_color(Theme::global().ACCENT)
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.sidebar_gesture = None;
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if !this.expanded_swarms.remove(&root) {
                                this.expanded_swarms.insert(root.clone());
                            }
                            cx.notify();
                        }),
                    )
                    .child(if expanded {
                        "Show less".into()
                    } else {
                        format!("Show {} more", children.len() - PREVIEW)
                    }),
            );
        }
        content.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(id: &str, parent: Option<&str>) -> SessionInfo {
        let mut session = super::super::tests::session_info(id, Some(id));
        session.parent_session_id = parent.map(str::to_owned);
        session
    }

    #[test]
    fn sidebar_swarm_groups_owned_agents_recursively_without_hiding_orphans_or_cycles() {
        let sessions = vec![
            session("root", None),
            session("grandchild", Some("child")),
            session("child", Some("root")),
            session("fork", None),
            session("orphan", Some("missing")),
            session("a", Some("b")),
            session("b", Some("a")),
            session("self", Some("self")),
        ];
        let groups = groups(&sessions);
        assert_eq!(
            groups.children["root"]
                .iter()
                .map(|c| (c.session.session_id.as_str(), c.depth))
                .collect::<Vec<_>>(),
            vec![("child", 1), ("grandchild", 2)]
        );
        assert_eq!(
            groups.nested,
            HashSet::from(["child".into(), "grandchild".into()])
        );
    }

    #[gpui::test]
    fn sidebar_swarm_metadata_refresh_grows_and_shrinks_measured_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        let sessions = vec![session("root", None), session("child", None)];
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::Sessions {
                    sessions: sessions.clone(),
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        let normal = vcx.debug_bounds("sidebar-session-0").unwrap().size.height;
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("swarm.json"), r#"{"members":[{"session_id":"child","report_back_to_session_id":"root","task_label":"Verify nested sidebar","status":"running"}]}"#).unwrap();
        let mut enriched = sessions.clone();
        jcode_sdk::enrich_sessions_from_swarm_state(&mut enriched, directory.path());
        assert_eq!(
            enriched[1].agent_label.as_deref(),
            Some("Verify nested sidebar")
        );
        assert_eq!(status(&enriched[1]), ("working", true));
        workspace.update(vcx, |w, cx| {
            w.apply(Update::Sessions { sessions: enriched }, cx);
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("swarm-child-child").is_some());
        assert!(vcx.debug_bounds("sidebar-session-0").unwrap().size.height > normal);
        workspace.update(vcx, |w, cx| {
            w.apply(Update::Sessions { sessions }, cx);
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("swarm-child-child").is_none());
        assert_eq!(
            vcx.debug_bounds("sidebar-session-0").unwrap().size.height,
            normal
        );
    }

    #[gpui::test]
    fn sidebar_swarm_expands_in_parent_and_child_click_opens_only_child(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |w, cx| {
            w.sessions = vec![
                session("root", None),
                session("child1", Some("root")),
                session("child2", Some("root")),
                session("child3", Some("root")),
                session("other", None),
            ];
            cx.notify();
        });
        vcx.run_until_parked();
        let root = vcx.debug_bounds("sidebar-session-0").unwrap();
        let other = vcx.debug_bounds("sidebar-session-1").unwrap();
        assert!(root.size.height > other.size.height * 2.0);
        assert!(
            vcx.debug_bounds("sidebar-session-2").is_none(),
            "children must not duplicate root rows"
        );
        let child = vcx.debug_bounds("swarm-child-child1").unwrap();
        assert!(child.left() > root.left() && child.bottom() <= root.bottom());
        assert!(vcx.debug_bounds("swarm-child-child3").is_none());
        let toggle = vcx.debug_bounds("swarm-toggle-root").unwrap();
        vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let child = vcx.debug_bounds("swarm-child-child3").unwrap();
        assert!(vcx.debug_bounds("sidebar-session-0").unwrap().size.height > root.size.height);
        vcx.simulate_click(child.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 1);
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "child3");
        });
    }
}
