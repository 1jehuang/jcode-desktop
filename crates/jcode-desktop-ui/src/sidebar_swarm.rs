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
        .map(|s| s.session_id.as_str())
        .collect::<HashSet<_>>();
    let mut children_by_parent = HashMap::<&str, Vec<&SessionInfo>>::new();
    let mut roots = Vec::new();
    for session in sessions {
        if let Some(parent) = session
            .parent_session_id
            .as_deref()
            .filter(|parent| by_id.contains(parent))
        {
            children_by_parent.entry(parent).or_default().push(session);
        } else {
            roots.push(session);
        }
    }
    let mut result = Groups::default();
    // A single-parent cycle (and every descendant of it) has no reachable root.
    // Leaving those nodes unvisited preserves their independent sidebar rows.
    // Missing-parent sessions are roots, so their valid descendants still nest.
    for root in roots {
        let Some(first_children) = children_by_parent.get(root.session_id.as_str()) else {
            continue;
        };
        let mut children = Vec::new();
        let mut pending = first_children
            .iter()
            .rev()
            .map(|child| (*child, 1))
            .collect::<Vec<_>>();
        while let Some((session, depth)) = pending.pop() {
            // Catalog IDs are normally unique. Duplicate records must neither
            // create a reachable traversal cycle nor hide this root itself.
            if session.session_id == root.session_id
                || !result.nested.insert(session.session_id.clone())
            {
                continue;
            }
            children.push(Child {
                session: session.clone(),
                depth,
            });
            if let Some(descendants) = children_by_parent.get(session.session_id.as_str()) {
                // Reverse the stack insertion to preserve catalog sibling order.
                // Iteration also avoids recursion depth proportional to the swarm.
                pending.extend(descendants.iter().rev().map(|child| (*child, depth + 1)));
            }
        }
        result.children.insert(root.session_id.clone(), children);
    }
    result
}

fn status(session: &SessionInfo) -> (&str, bool) {
    let status = session.swarm_status.as_deref().unwrap_or(&session.status);
    match status {
        "running" | "working" | "active" | "busy" | "thinking" | "streaming" => ("working", true),
        "waiting_network" => ("blocked", false),
        "completed" | "done" => ("done", false),
        "failed" | "error" | "crashed" => ("failed", false),
        "stopped" | "closed" => ("stopped", false),
        "" => ("idle", false),
        other => (other, false),
    }
}

fn title(session: &SessionInfo) -> String {
    session
        .agent_label
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| sidebar_session_title(session).1)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Keep focus and actionable work visible, retaining catalog/depth-first order
/// within the preview. Expansion always shows the full ownership tree.
fn visible_children<'a>(
    children: &'a [Child],
    expanded: bool,
    active_id: Option<&str>,
) -> Vec<&'a Child> {
    if expanded {
        return children.iter().collect();
    }
    let mut indices = (0..children.len()).collect::<Vec<_>>();
    indices.sort_by_key(|&i| {
        let session = &children[i].session;
        if active_id == Some(session.session_id.as_str()) {
            return 0;
        }
        match status(session).0 {
            "failed" | "blocked" => 1,
            "working" => 2,
            "ready" | "waiting" => 3,
            _ => 4,
        }
    });
    indices.truncate(PREVIEW);
    indices.sort_unstable();
    indices.into_iter().map(|i| &children[i]).collect()
}

fn summary(children: &[Child]) -> String {
    let mut counts = [0; 6];
    for child in children {
        counts[match status(&child.session).0 {
            "working" => 0,
            "failed" | "blocked" => 1,
            "done" => 2,
            "ready" => 3,
            "waiting" => 4,
            _ => 5,
        }] += 1;
    }
    counts
        .into_iter()
        .zip([
            "working",
            "need attention",
            "done",
            "ready",
            "waiting",
            "inactive",
        ])
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

struct SwarmTooltip(String);
impl Render for SwarmTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .max_w(px(360.0))
            .rounded_md()
            .bg(Theme::global().HEADER_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_size(px(11.0))
            .text_color(Theme::global().TEXT)
            .child(self.0.clone())
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
        let root = root_id.to_owned();
        let detail = summary(children);
        let summary_tooltip = format!(
            "{detail}. Status comes from the latest swarm report, not a live connection check."
        );
        let lead = self
            .sessions
            .iter()
            .find(|s| s.session_id == root_id)
            .cloned();
        let mut content = div()
            .mt_1()
            .ml(px(12.0))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(3.0));
        content = content
            .child(
                div()
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(format!("Swarm · {}", children.len())),
                    )
                    .when_some(lead, |row, lead| {
                        let tooltip = format!("Open lead: {}", sidebar_session_title(&lead).1);
                        row.child(
                            div()
                                .id(gpui::SharedString::from(format!("swarm-lead-{root_id}")))
                                .debug_selector({
                                    let root = root.clone();
                                    move || format!("swarm-lead-{root}")
                                })
                                .px_1()
                                .rounded_sm()
                                .text_size(px(10.0))
                                .text_color(Theme::global().ACCENT)
                                .hover(|el| el.bg(Theme::global().HEADER_BG))
                                .cursor_pointer()
                                .tooltip(move |_, cx| {
                                    cx.new(|_| SwarmTooltip(tooltip.clone())).into()
                                })
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
                                        this.activate_session(lead.clone(), window, cx);
                                    }),
                                )
                                .child("Open lead ↗"),
                        )
                    }),
            )
            .child(
                div()
                    .id(gpui::SharedString::from(format!("swarm-summary-{root_id}")))
                    .h(px(15.0))
                    .min_w_0()
                    .truncate()
                    .text_size(px(9.0))
                    .text_color(Theme::global().TEXT_DIM)
                    .tooltip(move |_, cx| cx.new(|_| SwarmTooltip(summary_tooltip.clone())).into())
                    .child(detail),
            );
        for child in visible_children(children, expanded, active_id) {
            let session = child.session.clone();
            let id = session.session_id.clone();
            let selector = id.clone();
            let label = title(&session);
            let (state, _) = status(&session);
            let (icon, color) = match state {
                "failed" => ("!", Theme::global().ERROR),
                "blocked" => ("!", Theme::global().ACCENT),
                "working" => ("●", Theme::global().ACCENT),
                "done" => ("✓", Theme::global().OK),
                "ready" => ("○", Theme::global().OK),
                "waiting" => ("◷", Theme::global().TEXT_DIM),
                _ => ("○", Theme::global().TEXT_DIM),
            };
            let selected = active_id == Some(id.as_str());
            let open = self
                .slots
                .iter()
                .any(|slot| !slot.closing && slot.panel.read(cx).session_id == id);
            let parent = self
                .sessions
                .iter()
                .find(|s| Some(s.session_id.as_str()) == session.parent_session_id.as_deref())
                .map(title)
                .unwrap_or_else(|| "lead".into());
            let tooltip = format!(
                "{label}\nStatus: {state}\nReports to: {parent}\n{}\nClick to {} this agent's conversation.",
                session
                    .working_dir
                    .as_deref()
                    .unwrap_or("No working directory"),
                if open { "focus" } else { "open" }
            );
            let state = state.to_owned();
            let close_id = id.clone();
            let edits = session.edit_stats.as_ref().map(|stats| {
                super::sidebar_edits::render(&id, stats.added, stats.removed, stats.approximate)
            });
            content = content.child(
                div()
                    .id(gpui::SharedString::from(format!("swarm-child-{id}")))
                    .debug_selector(move || format!("swarm-child-{selector}"))
                    .h(px(44.0))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(2.0))
                    .px_2()
                    .rounded_md()
                    // Compact previews can omit ancestors. Only indent the full tree.
                    .ml(px(if expanded {
                        (child.depth.saturating_sub(1).min(3) * 8) as f32
                    } else {
                        0.0
                    }))
                    // Working or open agents are not selected. Only the viewed
                    // conversation gets a fill; all other rows inherit the sidebar.
                    .when(selected, |el| el.bg(Theme::global().ACCENT_DIM))
                    .hover(|el| el.bg(Theme::global().HEADER_BG))
                    .cursor_pointer()
                    .tooltip(move |_, cx| cx.new(|_| SwarmTooltip(tooltip.clone())).into())
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
                    .child(
                        div()
                            .h(px(17.0))
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .text_color(Theme::global().TEXT)
                                    .child(label),
                            )
                            .when(open, |row| {
                                row.child(
                                    div()
                                        .id(gpui::SharedString::from(format!("swarm-close-{id}")))
                                        .debug_selector({
                                            let id = id.clone();
                                            move || format!("swarm-close-{id}")
                                        })
                                        .text_size(px(12.0))
                                        .px_1()
                                        .rounded_sm()
                                        .text_color(Theme::global().TEXT_DIM)
                                        .hover(|el| el.bg(Theme::global().ACCENT_DIM))
                                        .tooltip(|_, cx| {
                                            cx.new(|_| {
                                                SwarmTooltip(
                                                    "Close this view. The agent keeps running."
                                                        .into(),
                                                )
                                            })
                                            .into()
                                        })
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
                                                this.close_sidebar_sessions(
                                                    vec![close_id.clone()],
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child("×"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .h(px(14.0))
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .child(div().text_size(px(9.0)).text_color(color).child(icon))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(9.0))
                                    .text_color(color)
                                    .child(if selected {
                                        format!("{state} · viewing")
                                    } else if open {
                                        format!("{state} · open")
                                    } else {
                                        state
                                    }),
                            )
                            .children(edits),
                    ),
            );
        }
        if children.len() > PREVIEW {
            let selector = root.clone();
            content = content.child(
                div()
                    .id(gpui::SharedString::from(format!("swarm-toggle-{root}")))
                    .debug_selector(move || format!("swarm-toggle-{selector}"))
                    .h(px(23.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .rounded_sm()
                    .text_size(px(10.0))
                    .text_color(Theme::global().ACCENT)
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
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if !this.expanded_swarms.remove(&root) {
                                this.expanded_swarms.insert(root.clone());
                            }
                            cx.notify();
                        }),
                    )
                    .child(if expanded {
                        "Show fewer ▴".into()
                    } else {
                        format!("Show all {} agents ▾", children.len())
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

    fn children_with_states(states: &[&str]) -> Vec<Child> {
        states
            .iter()
            .enumerate()
            .map(|(i, state)| {
                let mut session = session(&format!("child{i}"), Some("root"));
                session.swarm_status = Some((*state).into());
                Child { session, depth: 1 }
            })
            .collect()
    }

    #[test]
    fn sidebar_swarm_preview_keeps_focus_and_attention_without_reordering_tree() {
        let children = children_with_states(&["done", "running", "blocked", "failed", "ready"]);
        let ids = |expanded, active| {
            visible_children(&children, expanded, active)
                .iter()
                .map(|c| c.session.session_id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(false, None), ["child2", "child3"]);
        assert_eq!(ids(false, Some("child4")), ["child2", "child4"]);
        assert_eq!(ids(false, Some("child0")), ["child0", "child2"]);
        assert_eq!(ids(false, Some("missing")), ["child2", "child3"]);
        assert_eq!(
            ids(true, Some("child4")),
            ["child0", "child1", "child2", "child3", "child4"]
        );
        assert!(visible_children(&[], false, None).is_empty());
        let working = children_with_states(&["done", "running", "ready"]);
        assert_eq!(
            visible_children(&working, false, None)[0]
                .session
                .session_id,
            "child1"
        );
    }

    #[test]
    fn sidebar_swarm_status_does_not_confuse_ready_blocked_or_unknown_with_done() {
        let children = children_with_states(&[
            "completed",
            "blocked",
            "ready",
            "waiting",
            "failed",
            "running",
        ]);
        assert_eq!(
            summary(&children),
            "1 working · 2 need attention · 1 done · 1 ready · 1 waiting"
        );
        assert_eq!(
            summary(&children_with_states(&["blocked", "ready"])),
            "1 need attention · 1 ready"
        );
        let mut s = session("agent", None);
        s.status = "running".into();
        assert_eq!(status(&s), ("working", true));
        for value in [
            "running",
            "working",
            "active",
            "busy",
            "thinking",
            "streaming",
        ] {
            s.swarm_status = Some(value.into());
            assert_eq!(status(&s), ("working", true));
        }
        s.swarm_status = Some("waiting_network".into());
        assert_eq!(status(&s), ("blocked", false));
        for value in ["blocked", "waiting", "ready", "new-future-state"] {
            s.swarm_status = Some(value.into());
            assert_eq!(status(&s), (value, false));
        }
        s.agent_label = Some("  API\n reviewer  ".into());
        assert_eq!(title(&s), "API reviewer");
        s.agent_label = Some(" \n ".into());
        assert_eq!(title(&s), sidebar_session_title(&s).1);
    }

    // The pre-optimization algorithm is an independent equivalence oracle and
    // same-binary benchmark control. Keep its ancestry walks and full rescans.
    fn legacy_groups(sessions: &[SessionInfo]) -> Groups {
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
            let root = original.first().map(|child| {
                let mut current = &child.session;
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

    fn assert_equivalent(sessions: &[SessionInfo]) {
        let actual = groups(sessions);
        let expected = legacy_groups(sessions);
        assert_eq!(actual.nested, expected.nested);
        assert_eq!(actual.children.len(), expected.children.len());
        for (root, expected_children) in expected.children {
            let actual_children = &actual.children[&root];
            assert_eq!(actual_children.len(), expected_children.len());
            for (actual, expected) in actual_children.iter().zip(expected_children) {
                assert_eq!(actual.depth, expected.depth);
                assert_eq!(
                    serde_json::to_value(&actual.session).unwrap(),
                    serde_json::to_value(&expected.session).unwrap()
                );
            }
        }
    }

    #[test]
    fn sidebar_swarm_adjacency_matches_legacy_for_all_small_parent_graphs() {
        // Exhaust all four-node single-parent graphs, including missing parents,
        // self cycles, longer cycles and descendants of cycles. Reverse catalog
        // order as well, so child-before-parent inputs and sibling order matter.
        let ids = ["a", "b", "c", "d"];
        let parents = [
            None,
            Some("missing"),
            Some("a"),
            Some("b"),
            Some("c"),
            Some("d"),
        ];
        for mut graph in 0..parents.len().pow(ids.len() as u32) {
            let mut sessions = ids
                .iter()
                .map(|id| {
                    let parent = parents[graph % parents.len()];
                    graph /= parents.len();
                    session(id, parent)
                })
                .collect::<Vec<_>>();
            assert_equivalent(&sessions);
            sessions.reverse();
            assert_equivalent(&sessions);
        }
        assert_equivalent(&[]);
    }

    #[test]
    fn sidebar_swarm_adjacency_preserves_depth_first_sibling_order_and_orphan_roots() {
        let sessions = vec![
            session("grandchild-b", Some("child-b")),
            session("child-b", Some("root")),
            session("orphan-child", Some("orphan")),
            session("grandchild-a", Some("child-a")),
            session("root", None),
            session("child-a", Some("root")),
            session("orphan", Some("missing")),
            session("cycle-a", Some("cycle-b")),
            session("cycle-b", Some("cycle-a")),
            session("cycle-descendant", Some("cycle-a")),
        ];
        assert_equivalent(&sessions);
        let groups = groups(&sessions);
        assert_eq!(
            groups.children["root"]
                .iter()
                .map(|c| (c.session.session_id.as_str(), c.depth))
                .collect::<Vec<_>>(),
            [
                ("child-b", 1),
                ("grandchild-b", 2),
                ("child-a", 1),
                ("grandchild-a", 2)
            ]
        );
        assert_eq!(
            groups.children["orphan"][0].session.session_id,
            "orphan-child"
        );
        assert!(!groups.nested.contains("cycle-descendant"));
    }

    fn scaling_sessions(count: usize, shape: &str) -> Vec<SessionInfo> {
        (0..count)
            .map(|index| {
                let parent = match (shape, index) {
                    ("flat", 1..) => Some("session-0".to_string()),
                    ("chain", 1..) => Some(format!("session-{}", index - 1)),
                    _ => None,
                };
                session(&format!("session-{index}"), parent.as_deref())
            })
            .collect()
    }

    #[test]
    fn sidebar_swarm_adjacency_handles_deep_chains_without_recursive_stack_growth() {
        let sessions = scaling_sessions(10_000, "chain");
        let groups = groups(&sessions);
        assert_eq!(groups.nested.len(), 9_999);
        let last = groups.children["session-0"].last().unwrap();
        assert_eq!(last.session.session_id, "session-9999");
        assert_eq!(last.depth, 9_999);
    }

    #[test]
    fn sidebar_swarm_adjacency_duplicate_ids_cannot_cycle_or_hide_the_root() {
        let sessions = vec![
            session("root", None),
            session("child", Some("root")),
            session("root", Some("child")),
            session("child", Some("child")),
        ];
        let groups = groups(&sessions);
        assert_eq!(groups.nested, HashSet::from(["child".into()]));
        assert_eq!(groups.children["root"].len(), 1);
        assert_eq!(groups.children["root"][0].session.session_id, "child");
        assert_eq!(groups.children["root"][0].depth, 1);
    }

    #[test]
    #[ignore = "manual same-binary component benchmark, not a presentation FPS test"]
    fn sidebar_swarm_grouping_profile() {
        for shape in ["independent", "flat", "chain"] {
            for count in [100, 500, 1_000] {
                let sessions = scaling_sessions(count, shape);
                assert_equivalent(&sessions);
                let mut old = Vec::new();
                let mut new = Vec::new();
                for iteration in 0..23 {
                    let mut sample = |optimized: bool| {
                        let started = std::time::Instant::now();
                        let result = if optimized {
                            groups(std::hint::black_box(&sessions))
                        } else {
                            legacy_groups(std::hint::black_box(&sessions))
                        };
                        std::hint::black_box(result);
                        if iteration >= 3 {
                            if optimized {
                                new.push(started.elapsed().as_micros());
                            } else {
                                old.push(started.elapsed().as_micros());
                            }
                        }
                    };
                    // Alternate order to reduce systematic warm-cache bias.
                    sample(iteration % 2 == 0);
                    sample(iteration % 2 != 0);
                }
                old.sort_unstable();
                new.sort_unstable();
                println!(
                    "SIDEBAR_GROUPS shape={shape} sessions={count} legacy_p50={}us legacy_p95={}us adjacency_p50={}us adjacency_p95={}us",
                    old[9], old[18], new[9], new[18]
                );
            }
        }
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
    fn sidebar_swarm_reopening_during_close_preserves_entity_and_restores_watch(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = harness::spawn_recording();
        let child = session("child", Some("root"));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut w = Workspace::for_test(learning::Coach::new(), cx);
            w.set_test_bridge(bridge);
            w.sessions = vec![session("root", None), child.clone()];
            w.active = w.open_session(child.clone(), cx);
            w
        });
        vcx.run_until_parked();
        while commands.try_recv().is_ok() {}
        vcx.update(|window, cx| {
            workspace.update(cx, |w, cx| {
                let panel = w.slots[w.active].panel.clone();
                let input = panel.read(cx).input.clone();
                w.close_sidebar_sessions(vec!["child".into()], window, cx);
                assert!(w.slots[0].closing);
                w.activate_session(child.clone(), window, cx);
                assert_eq!(w.slots.len(), 1);
                assert!(!w.slots[0].closing);
                assert_eq!(w.slots[0].panel.entity_id(), panel.entity_id());
                assert_eq!(
                    w.slots[0].panel.read(cx).input.entity_id(),
                    input.entity_id()
                );
                w.apply(
                    Update::Event {
                        session_id: "child".into(),
                        event: jcode_sdk::ApiEvent::SessionStatus {
                            session_id: "child".into(),
                            status: "running".into(),
                        },
                    },
                    cx,
                );
                assert_eq!(panel.read(cx).status, "running");
            })
        });
        assert!(
            matches!(commands.try_recv(), Ok(Command::Unwatch { session_id }) if session_id == "child")
        );
        assert!(
            matches!(commands.try_recv(), Ok(Command::Watch { session_id }) if session_id == "child")
        );
        assert!(commands.try_recv().is_err());
    }

    #[gpui::test]
    fn sidebar_swarm_expands_in_parent_and_child_click_opens_only_child(
        cx: &mut gpui::TestAppContext,
    ) {
        let (bridge, commands) = harness::spawn_recording();
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        workspace.update(vcx, |w, cx| {
            w.set_test_bridge(bridge);
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
        // Collapsing must retain the agent being viewed even beyond the first two.
        let toggle = vcx.debug_bounds("swarm-toggle-root").unwrap();
        vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("swarm-child-child3").is_some());
        assert!(vcx.debug_bounds("swarm-child-child2").is_none());
        let lead = vcx.debug_bounds("swarm-lead-root").unwrap();
        vcx.simulate_click(lead.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 2);
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "root");
        });
        let toggle = vcx.debug_bounds("swarm-toggle-root").unwrap();
        vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let child = vcx.debug_bounds("swarm-child-child3").unwrap();
        vcx.simulate_click(child.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.len(), 2, "refocusing must reuse the agent's panel");
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "child3");
        });
        while commands.try_recv().is_ok() {}
        let close = vcx.debug_bounds("swarm-close-child3").unwrap();
        vcx.simulate_click(close.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots.iter().filter(|s| !s.closing).count(), 1);
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "root");
            assert_eq!(
                w.sessions.len(),
                5,
                "closing a view must retain the worker catalog"
            );
        });
        assert!(
            matches!(commands.try_recv(), Ok(Command::Unwatch { session_id }) if session_id == "child3")
        );
        assert!(
            commands.try_recv().is_err(),
            "close must only detach, never cancel or reopen"
        );
    }
}
