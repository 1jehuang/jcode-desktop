//! Compact, session-scoped edit totals shared by regular and swarm sidebar rows.
use gpui::{Context, IntoElement, Render, SharedString, Window, div, prelude::*, px};

use crate::theme::Theme;

pub(super) fn refresh_after(event: &jcode_sdk::ApiEvent) -> bool {
    match event {
        // A failed multi-file patch can still contain successful file mutations.
        jcode_sdk::ApiEvent::ToolDone { name, .. } => matches!(
            name.trim_start_matches("functions."),
            "write" | "edit" | "multiedit" | "patch" | "apply_patch" | "replace" | "batch"
        ),
        jcode_sdk::ApiEvent::TurnDone { .. } => true,
        _ => false,
    }
}

struct EditTooltip(String);

impl Render for EditTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .max_w(px(340.))
            .rounded_md()
            .bg(Theme::global().HEADER_BG)
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .text_size(px(11.))
            .text_color(Theme::global().TEXT_DIM)
            .child(self.0.clone())
    }
}

pub(super) fn render(
    session_id: &str,
    added: u64,
    removed: u64,
    approximate: bool,
) -> gpui::AnyElement {
    let selector = format!("sidebar-edits-{session_id}");
    let detail = format!(
        "{added} lines added, {removed} lines removed by this session's recorded file edits. Cumulative edits, not the repository's uncommitted diff. Shell commands and external edits are not included.{}",
        if approximate {
            " Some historical edits are incomplete, so these counts are approximate."
        } else {
            ""
        }
    );
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .font_family(Theme::global().FONT_MONO)
        .text_size(px(9.))
        .tooltip(move |_, cx| cx.new(|_| EditTooltip(detail.clone())).into())
        .when(approximate, |row| {
            row.child(div().text_color(Theme::global().TEXT_DIM).child("≈"))
        })
        .child(
            div()
                .text_color(Theme::global().OK)
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(Theme::global().ERROR)
                .child(format!("−{removed}")),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::workspace::{Workspace, tests::session_info};
    use crate::{harness::Update, learning};

    #[test]
    fn sidebar_edits_refresh_after_mutations_not_streaming_or_reads() {
        for name in [
            "edit",
            "write",
            "multiedit",
            "patch",
            "apply_patch",
            "batch",
            "functions.edit",
        ] {
            let event = jcode_sdk::ApiEvent::ToolDone {
                session_id: "session".into(),
                call_id: "call".into(),
                name: name.into(),
                output: String::new(),
                error: Some("Partial patch failure".into()),
            };
            assert!(super::refresh_after(&event));
        }
        assert!(super::refresh_after(&jcode_sdk::ApiEvent::TurnDone {
            session_id: "session".into()
        }));
        assert!(!super::refresh_after(&jcode_sdk::ApiEvent::TextDelta {
            message_id: None,
            session_id: "session".into(),
            text: "writing".into()
        }));
        assert!(!super::refresh_after(&jcode_sdk::ApiEvent::ToolDone {
            session_id: "session".into(),
            call_id: "call".into(),
            name: "read".into(),
            output: String::new(),
            error: None,
        }));
    }

    fn stats(added: u64, removed: u64) -> jcode_sdk::SessionEditStats {
        jcode_sdk::SessionEditStats {
            added,
            removed,
            approximate: false,
        }
    }

    #[gpui::test]
    fn sidebar_edit_totals_are_session_scoped_and_refresh_without_losing_navigation(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        let mut first = session_info("first", Some("First session"));
        first.working_dir = Some("/same/repo".into());
        first.edit_stats = Some(stats(128, 37));
        let mut second = session_info("second", Some("Second session"));
        second.working_dir = first.working_dir.clone();
        second.edit_stats = Some(stats(2, 0));
        let unknown = session_info("unknown", Some("No statistics available"));
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::Sessions {
                    sessions: vec![first.clone(), second.clone(), unknown.clone()],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        let first_badge = vcx.debug_bounds("sidebar-edits-first").unwrap();
        let second_badge = vcx.debug_bounds("sidebar-edits-second").unwrap();
        assert_ne!(first_badge.origin.y, second_badge.origin.y);
        assert!(vcx.debug_bounds("sidebar-edits-unknown").is_none());
        let first_row = vcx.debug_bounds("sidebar-session-0").unwrap();
        assert!(first_badge.right() <= first_row.right());
        assert!(first_badge.bottom() <= first_row.bottom());
        vcx.simulate_click(first_badge.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, cx| {
            assert_eq!(w.slots[w.active].panel.read(cx).session_id, "first");
        });
        first.edit_stats = Some(stats(250, 40));
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::Sessions {
                    sessions: vec![first, second, unknown],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        workspace.read_with(vcx, |w, _| {
            assert_eq!(
                w.sessions
                    .iter()
                    .find(|s| s.session_id == "first")
                    .unwrap()
                    .edit_stats
                    .as_ref()
                    .unwrap()
                    .added,
                250
            );
            assert_eq!(
                w.sessions
                    .iter()
                    .find(|s| s.session_id == "second")
                    .unwrap()
                    .edit_stats
                    .as_ref()
                    .unwrap()
                    .added,
                2
            );
        });
        assert!(vcx.debug_bounds("sidebar-edits-first").is_some());
    }

    #[gpui::test]
    fn sidebar_edit_metadata_remeasures_root_without_rendering_swarm_counts(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) =
            cx.add_window_view(|_, cx| Workspace::for_test(learning::Coach::new(), cx));
        let root = session_info("root", Some("Parent"));
        workspace.update(vcx, |w, cx| {
            w.layout_mode = crate::config::LayoutMode::Normal;
            w.apply(
                Update::Sessions {
                    sessions: vec![root.clone()],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        let before = vcx.debug_bounds("sidebar-session-0").unwrap().size.height;
        let mut root_with_stats = root.clone();
        root_with_stats.edit_stats = Some(stats(0, 0));
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::Sessions {
                    sessions: vec![root_with_stats.clone()],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-session-0").unwrap().size.height > before);
        let root_badge = vcx.debug_bounds("sidebar-edits-root").unwrap();
        let root_row = vcx.debug_bounds("sidebar-session-0").unwrap();
        assert!(root_badge.right() <= root_row.right());
        assert!(root_badge.bottom() <= root_row.bottom());
        let mut child = session_info("child", Some("A very long swarm agent task label"));
        child.parent_session_id = Some("root".into());
        child.edit_stats = Some(jcode_sdk::SessionEditStats {
            added: 12000,
            removed: 3400,
            approximate: true,
        });
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::Sessions {
                    sessions: vec![root_with_stats, child],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("sidebar-edits-child").is_none());
        assert!(vcx.debug_bounds("swarm-child-child").is_none());
        assert!(vcx.debug_bounds("sidebar-session-1").is_none());
        assert_eq!(vcx.debug_bounds("sidebar-edits-root").unwrap(), root_badge);
        assert_eq!(vcx.debug_bounds("sidebar-session-0").unwrap(), root_row);
        workspace.update(vcx, |w, cx| {
            w.apply(
                Update::Sessions {
                    sessions: vec![root],
                },
                cx,
            );
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.debug_bounds("sidebar-session-0").unwrap().size.height,
            before
        );
        assert!(vcx.debug_bounds("sidebar-edits-root").is_none());
    }
}
