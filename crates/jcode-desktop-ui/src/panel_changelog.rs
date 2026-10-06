//! Read-only update summary and versioned history.
use super::*;
use crate::update_notes::{self, View};
use gpui::{HighlightStyle, StyledText};

/// Comfortable reading measure. Wider lines in a monospace face are hard to
/// track back to the next line, which made the notes feel like a wall of text.
const MEASURE: f32 = 680.;

/// Split an entry into a short lead and its detail, so each bullet scans by
/// its first words. "Applets: sandboxed UI" leads with "Applets", and longer
/// entries lead with their first sentence when more text follows.
fn lead_len(entry: &str) -> Option<usize> {
    if let Some(colon) = entry.find(": ")
        && colon <= 40
        && !entry[..colon].contains('`')
    {
        return Some(colon + 1);
    }
    let end = entry.find(". ")? + 1;
    (end < entry.len().saturating_sub(1)).then_some(end)
}

fn entry_text(entry: String, emphasize: bool) -> StyledText {
    let theme = Theme::global();
    let lead = if emphasize { lead_len(&entry) } else { None };
    let mut highlights = Vec::new();
    match lead {
        Some(lead) => {
            highlights.push((
                0..lead,
                HighlightStyle {
                    color: Some(theme.TEXT.into()),
                    font_weight: Some(gpui::FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            ));
            highlights.push((
                lead..entry.len(),
                HighlightStyle {
                    color: Some(theme.TEXT_DIM.into()),
                    ..Default::default()
                },
            ));
        }
        None => highlights.push((
            0..entry.len(),
            HighlightStyle {
                color: Some(theme.TEXT.into()),
                ..Default::default()
            },
        )),
    }
    StyledText::new(entry).with_highlights(highlights)
}

// Keep the release notes editorial rather than turning every commit into a card.
// Quiet bullets, hanging indentation, and air between items keep long lists
// scannable without boxes.
fn update_entries(entries: Vec<String>, emphasize: bool) -> gpui::Div {
    let theme = Theme::global();
    div()
        .flex()
        .flex_col()
        .gap(px(if emphasize { 10. } else { 6. }))
        .children(entries.into_iter().map(move |entry| {
            div()
                .flex()
                .items_start()
                .gap_3()
                .text_size(px(14.))
                .line_height(px(22.))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(theme.TEXT_FAINT)
                        .child("•"),
                )
                .child(div().flex_1().min_w_0().child(entry_text(entry, emphasize)))
        }))
}

fn section_label(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(Theme::global().TEXT_DIM)
        .child(label.into())
}

fn release_heading(version: String, date: String) -> gpui::Div {
    let theme = Theme::global();
    div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(version),
        )
        .when(!date.is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.TEXT_DIM)
                    .child(date),
            )
        })
}

fn running_build() -> gpui::Div {
    div()
        .debug_selector(|| "update-running-build".into())
        .flex()
        .flex_wrap()
        .gap_1()
        .text_size(px(12.))
        .text_color(Theme::global().TEXT_DIM)
        .child(
            div()
                .debug_selector(|| "update-version".into())
                .child(format!("Running {}", crate::build_info::version())),
        )
        .child(format!(
            "· {} · {}",
            crate::build_info::revision(),
            crate::build_info::age(),
        ))
}

fn pill(id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .cursor_pointer()
        .rounded_full()
        .px_3()
        .py(px(3.))
        .text_size(px(12.))
}

impl Panel {
    fn update_view_button(
        &self,
        view: View,
        label: &'static str,
        id: &'static str,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = Theme::global();
        let selected = self.changelog_view == view;
        pill(id)
            .text_color(if selected { theme.TEXT } else { theme.TEXT_DIM })
            .when(selected, |el| el.bg(theme.HEADER_BG))
            .hover(|el| el.bg(theme.HEADER_BG).text_color(theme.TEXT))
            .on_click(cx.listener(move |panel, _, _, cx| {
                panel.changelog_view = view;
                cx.notify();
            }))
            .child(label)
    }

    pub(super) fn render_changelog(&self, window: &Window, cx: &Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global();
        let content_id = match self.changelog_view {
            View::Latest => "update-summary",
            View::History => "update-history",
        };
        let mut content = div()
            .id(content_id)
            .debug_selector(move || content_id.into())
            .w_full()
            .max_w(px(MEASURE))
            .mx_auto()
            .flex()
            .flex_col()
            .gap_8();
        match self.changelog_view {
            View::Latest => {
                let summary = update_notes::summary();
                let overview = update_notes::release_overview();
                let has_overview = overview.is_some();
                if let Some(overview) = overview {
                    content = content.child(
                        div()
                            .debug_selector(|| "update-release-overview".into())
                            .flex()
                            .flex_col()
                            .gap_8()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(section_label(overview.release))
                                    .when_some(overview.headline, |el, headline| {
                                        el.child(
                                            div()
                                                .text_size(px(20.))
                                                .line_height(px(28.))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .child(headline),
                                        )
                                    })
                                    .child(running_build()),
                            )
                            .children(overview.sections.into_iter().map(|section| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(section_label(section.label))
                                    .child(update_entries(section.entries, true))
                            })),
                    );
                } else {
                    content = content.child(running_build());
                }
                content = content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(section_label(summary.heading))
                        .when(!summary.entries.is_empty(), |el| {
                            el.child(update_entries(summary.entries.clone(), false))
                        })
                        .when(summary.entries.is_empty(), |el| {
                            el.child(div().text_color(theme.TEXT_DIM).child(
                                "Git history wasn’t included in this build.",
                            ))
                            .when(!has_overview, |el| {
                                el.child(markdown::render(
                                    update_notes::fallback(),
                                    0,
                                    &self.transcript_selection,
                                    window,
                                    cx,
                                ))
                            })
                        })
                        .child(
                            div().flex().child(
                                pill("update-see-all")
                                    .text_color(theme.TEXT_DIM)
                                    .bg(theme.HEADER_BG)
                                    .hover(|el| el.text_color(theme.TEXT))
                                    .on_click(cx.listener(|panel, _, _, cx| {
                                        panel.changelog_view = View::History;
                                        cx.notify();
                                    }))
                                    .child(if summary.remaining > 0 {
                                        format!("See all changes · {} more", summary.remaining)
                                    } else {
                                        "See all changes".into()
                                    }),
                            ),
                        ),
                );
            }
            View::History => {
                let groups = update_notes::history();
                content = content
                    .when(groups.is_empty(), |el| {
                        el.child(markdown::render(
                            update_notes::fallback(),
                            0,
                            &self.transcript_selection,
                            window,
                            cx,
                        ))
                    })
                    .children(groups.into_iter().map(|group| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(release_heading(group.version, group.date))
                            .child(update_entries(group.entries, false))
                    }));
            }
        }
        div()
            .id("desktop-changelog")
            .debug_selector(|| "desktop-changelog".into())
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(theme.FONT_AI)
            .text_size(px(14.))
            .text_color(theme.TEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(
                cx.listener(|panel, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        window.dispatch_action(Box::new(crate::workspace::ClosePanel), cx);
                        cx.stop_propagation();
                    } else if event.keystroke.modifiers == gpui::Modifiers::default() {
                        let views = [View::Latest, View::History];
                        let current = views
                            .iter()
                            .position(|view| *view == panel.changelog_view)
                            .unwrap_or(0);
                        let next = match event.keystroke.key.as_str() {
                            "right" => Some((current + 1) % views.len()),
                            "left" => Some((current + views.len() - 1) % views.len()),
                            _ => None,
                        };
                        if let Some(next) = next {
                            panel.changelog_view = views[next];
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }
                }),
            )
            // One quiet row: title, view switch, close. Keyboard hints live in
            // the shortcut overlay rather than a permanent footer.
            .child(
                div().px_5().py_3().flex_shrink_0().child(
                    div()
                        .w_full()
                        .max_w(px(MEASURE))
                        .mx_auto()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .mr_2()
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Changelog"),
                        )
                        .child(self.update_view_button(
                            View::Latest,
                            "Highlights",
                            "updates-latest",
                            cx,
                        ))
                        .child(self.update_view_button(
                            View::History,
                            "All changes",
                            "updates-history",
                            cx,
                        ))
                        .child(div().flex_1())
                        .child(
                            pill("close-changelog")
                                .text_color(theme.TEXT_DIM)
                                .hover(|el| el.bg(theme.HEADER_BG).text_color(theme.TEXT))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(crate::workspace::ClosePanel),
                                        cx,
                                    )
                                })
                                .child("Close"),
                        ),
                ),
            )
            .child(
                div()
                    // Separate scroll state per view avoids landing halfway down
                    // the summary when returning from a long release history.
                    .id(match self.changelog_view {
                        View::Latest => "updates-scroll-latest",
                        View::History => "updates-scroll-history",
                    })
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_4()
                    .pb_10()
                    .child(content),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_lead_with_a_label_or_first_sentence() {
        fn lead(s: &str) -> Option<&str> {
            lead_len(s).map(|n| &s[..n])
        }
        assert_eq!(lead("Applets: sandboxed custom UI."), Some("Applets:"));
        assert_eq!(
            lead("Model picker with logos. The method pill switches."),
            Some("Model picker with logos.")
        );
        assert_eq!(lead("A single sentence."), None);
        assert_eq!(lead("`/save [label]`: names"), None);
        assert_eq!(lead("Plain commit subject"), None);
    }

    #[gpui::test]
    fn update_views_switch_without_editing_or_using_runtime(cx: &mut gpui::TestAppContext) {
        cx.update(crate::bind_workspace_keys);
        let (bridge, commands) = crate::harness::spawn_recording();
        let (panel, vcx) = cx.add_window_view(|window, cx| {
            let panel = Panel::new(Panel::CHANGELOG_SESSION_ID.into(), None, None, bridge, cx);
            window.focus(&panel.focus_handle, cx);
            panel
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("update-summary").is_some());
        assert!(vcx.debug_bounds("update-version").is_some());
        assert!(vcx.debug_bounds("update-running-build").is_some());
        assert_eq!(
            vcx.debug_bounds("update-release-overview").is_some(),
            update_notes::release_overview().is_some(),
        );
        assert!(vcx.debug_bounds("update-history").is_none());
        assert!(vcx.debug_bounds("update-build").is_none());
        assert!(vcx.debug_bounds("updates-build").is_none());
        for (control, target, view) in [
            ("updates-history", "update-history", View::History),
            ("updates-latest", "update-summary", View::Latest),
            ("updates-history", "update-history", View::History),
        ] {
            let bounds = vcx
                .debug_bounds(control)
                .expect("update navigation control");
            vcx.simulate_click(bounds.center(), gpui::Modifiers::default());
            vcx.run_until_parked();
            assert!(
                vcx.debug_bounds(target).is_some(),
                "{target} did not render"
            );
            panel.read_with(vcx, |panel, _| assert_eq!(panel.changelog_view, view));
        }
        vcx.simulate_input("must not submit");
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(
            commands.try_recv().is_err(),
            "update controls must remain local"
        );
        vcx.simulate_keystrokes("left");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("update-summary").is_some());
        vcx.simulate_keystrokes("right");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("update-history").is_some());
        panel.read_with(vcx, |panel, _| {
            assert_eq!(panel.changelog_view, View::History)
        });
        vcx.simulate_keystrokes("right");
        vcx.run_until_parked();
        // Both development and packaged builds cycle through only two views.
        assert!(vcx.debug_bounds("update-summary").is_some());
        panel.read_with(vcx, |panel, _| {
            assert_eq!(panel.changelog_view, View::Latest)
        });
        vcx.simulate_keystrokes("left");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("update-history").is_some());
        panel.read_with(vcx, |panel, _| {
            assert_eq!(panel.changelog_view, View::History)
        });
        assert!(vcx.debug_bounds("updates-build").is_none());
        assert!(vcx.debug_bounds("update-build").is_none());
        assert!(
            commands.try_recv().is_err(),
            "keyboard navigation must remain local"
        );
    }
}
