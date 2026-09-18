//! Read-only update summary and versioned history, with opt-in diagnostics.
use super::*;
use crate::update_notes::{self, View};

fn update_entries(entries: Vec<String>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .children(entries.into_iter().map(|entry| {
            div()
                .flex()
                .gap_3()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(Theme::global().TEXT_FAINT)
                        .child("•"),
                )
                .child(div().flex_1().min_w_0().child(entry))
        }))
}

impl Panel {
    fn update_view_button(
        &self,
        view: View,
        label: &'static str,
        id: &'static str,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = self.changelog_view == view;
        div()
            .id(id)
            .debug_selector(move || id.into())
            .cursor_pointer()
            .rounded_md()
            .px_3()
            .py_2()
            .text_size(px(12.))
            .text_color(if selected {
                Theme::global().TEXT
            } else {
                Theme::global().TEXT_DIM
            })
            .when(selected, |el| {
                el.bg(Theme::global().HEADER_BG)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .hover(|el| el.bg(Theme::global().HEADER_BG))
            .on_click(cx.listener(move |panel, _, _, cx| {
                panel.changelog_view = view;
                cx.notify();
            }))
            .child(label)
    }

    pub(super) fn render_changelog(&self, window: &Window, cx: &Context<Self>) -> gpui::AnyElement {
        let content_id = match self.changelog_view {
            View::Latest => "update-summary",
            View::History => "update-history",
            View::Build => "update-build",
        };
        let mut content = div()
            .id(content_id)
            .debug_selector(move || content_id.into())
            .w_full()
            .max_w(px(760.))
            .mx_auto()
            .flex()
            .flex_col()
            .gap_5();
        match self.changelog_view {
            View::Latest => {
                let summary = update_notes::summary();
                content = content
                    .child(div().flex().flex_col().gap_2()
                        .child(div().text_size(px(20.)).font_weight(gpui::FontWeight::SEMIBOLD).child(summary.heading))
                        .child(div().text_color(Theme::global().TEXT_DIM).child(summary.description)))
                    .child(update_entries(summary.entries.clone()))
                    .when(summary.entries.is_empty(), |el| el
                        .child(div().text_color(Theme::global().TEXT_DIM).child("Git history wasn’t included in this build. Here are the bundled release notes."))
                        .child(markdown::render(update_notes::fallback(), 0, &self.transcript_selection, window, cx)))
                    .child(div()
                        .id("update-see-all")
                        .debug_selector(|| "update-see-all".into())
                        .cursor_pointer()
                        .text_color(Theme::global().LINK)
                        .hover(|el| el.text_color(Theme::global().TEXT))
                        .on_click(cx.listener(|panel, _, _, cx| {
                            panel.changelog_view = View::History;
                            cx.notify();
                        }))
                        .child(if summary.remaining > 0 {
                            format!("{} more in release history →", summary.remaining)
                        } else {
                            "View release history →".into()
                        }));
            }
            View::History => {
                let groups = update_notes::history();
                content = content
                    .child(div().text_color(Theme::global().TEXT_DIM).child("Included commit history, newest first. Grouped by release, just like the TUI."))
                    .when(groups.is_empty(), |el| el.child(markdown::render(update_notes::fallback(), 0, &self.transcript_selection, window, cx)))
                    .children(groups.into_iter().map(|group| {
                        div().flex().flex_col().gap_4().pb_4()
                            .child(div().flex().flex_col().gap_1()
                                .child(div().text_size(px(17.)).font_weight(gpui::FontWeight::SEMIBOLD).child(group.version))
                                .child(div().text_size(px(12.)).text_color(Theme::global().TEXT_DIM).child(group.date)))
                            .child(update_entries(group.entries))
                    }));
            }
            View::Build => {
                content = content.child(markdown::render(
                    &update_notes::build_details(),
                    0,
                    &self.transcript_selection,
                    window,
                    cx,
                ));
            }
        }
        div()
            .id("desktop-changelog")
            .debug_selector(|| "desktop-changelog".into())
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_size(px(14.))
            .text_color(Theme::global().TEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(
                cx.listener(|panel, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        window.dispatch_action(Box::new(crate::workspace::ClosePanel), cx);
                        cx.stop_propagation();
                    } else if event.keystroke.modifiers == gpui::Modifiers::default() {
                        let views = if update_notes::development() {
                            &[View::Latest, View::History, View::Build][..]
                        } else {
                            &[View::Latest, View::History][..]
                        };
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_5()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("What’s new in Jcode Desktop"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(Theme::global().TEXT_DIM)
                                            .child(format!(
                                                "v{} · {}",
                                                env!("JCODE_DESKTOP_VERSION"),
                                                if update_notes::development() {
                                                    "Development build"
                                                } else {
                                                    "Installed version"
                                                }
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-changelog")
                                    .debug_selector(|| "close-changelog".into())
                                    .cursor_pointer()
                                    .rounded_md()
                                    .px_3()
                                    .py_1()
                                    .text_size(px(12.))
                                    .text_color(Theme::global().TEXT_DIM)
                                    .hover(|el| {
                                        el.bg(Theme::global().HEADER_BG)
                                            .text_color(Theme::global().TEXT)
                                    })
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(crate::workspace::ClosePanel),
                                            cx,
                                        )
                                    })
                                    .child("Close"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(self.update_view_button(
                                View::Latest,
                                "Latest updates",
                                "updates-latest",
                                cx,
                            ))
                            .child(self.update_view_button(
                                View::History,
                                "Release history",
                                "updates-history",
                                cx,
                            ))
                            .when(update_notes::development(), |el| {
                                el.child(self.update_view_button(
                                    View::Build,
                                    "Build details",
                                    "updates-build",
                                    cx,
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    // Separate scroll state per view avoids landing halfway down
                    // the summary when returning from a long release history.
                    .id(match self.changelog_view {
                        View::Latest => "updates-scroll-latest",
                        View::History => "updates-scroll-history",
                        View::Build => "updates-scroll-build",
                    })
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .py_4()
                    .child(content),
            )
            .child(
                div()
                    .px_5()
                    .py_3()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(Theme::global().TEXT_DIM)
                    .child("←/→ switch views · Esc to close · /changelog to reopen"),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(vcx.debug_bounds("update-history").is_none());
        assert!(vcx.debug_bounds("update-build").is_none());
        for (control, target, view) in [
            ("update-see-all", "update-history", View::History),
            ("updates-build", "update-build", View::Build),
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
        vcx.simulate_keystrokes("right right");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("update-build").is_some());
    }
}
