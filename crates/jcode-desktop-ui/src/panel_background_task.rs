//! Running work stays quiet. Finished work leaves a compact, readable result card.
use crate::text_selection::{self, TextSelection};
use crate::theme::Theme;
use gpui::{prelude::*, *};

struct TaskTooltip(String);
impl Render for TaskTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(420.0))
            .p_2()
            .rounded_md()
            .bg(Theme::global().HEADER_BG)
            .text_size(px(12.0))
            .text_color(Theme::global().TEXT)
            .child(self.0.clone())
    }
}

pub(super) fn render(
    index: usize,
    task_id: &str,
    label: &str,
    summary: &str,
    percent: Option<f32>,
    done: bool,
    selection: &Entity<TextSelection>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = Theme::global();
    if done {
        return completed_card(index, task_id, label, summary, selection, window, cx)
            .into_any_element();
    }
    let details = format!(
        "{label}\n{}\n{summary}",
        if done {
            "Finished"
        } else {
            "Running in background"
        }
    );
    div()
        .id(("background-task", index))
        .debug_selector(|| "background-task-card".into())
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .min_w_0()
        .h(px(24.0))
        .px_1()
        .text_size(px(11.0))
        .tooltip(move |_, cx| cx.new(|_| TaskTooltip(details.clone())).into())
        .child(
            div()
                .flex_none()
                .text_color(if done { theme.OK } else { theme.WARN })
                .child(if done { "✓" } else { "●" }),
        )
        .child(
            div()
                .max_w(relative(0.4))
                .min_w_0()
                .truncate()
                .text_color(theme.TOOL_TEXT)
                .child(text_selection::plain(
                    selection.clone(),
                    format!("background-label-{index}"),
                    label.to_owned(),
                    window,
                    cx,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.TEXT_DIM)
                .child(text_selection::plain(
                    selection.clone(),
                    format!("background-summary-{index}"),
                    summary.to_owned(),
                    window,
                    cx,
                )),
        )
        .when_some(percent.filter(|value| value.is_finite()), |el, percent| {
            el.child(
                div()
                    .flex_none()
                    .w(px(40.0))
                    .h(px(2.0))
                    .rounded_full()
                    .bg(theme.TOOL_BORDER)
                    .child(
                        div()
                            .h_full()
                            .w(relative((percent / 100.0).clamp(0.0, 1.0)))
                            .rounded_full()
                            .bg(if done { theme.OK } else { theme.ACCENT }),
                    ),
            )
        })
        .into_any_element()
}

fn completion_color(summary: &str) -> Rgba {
    let theme = Theme::global();
    let status = summary.split('·').next().unwrap_or(summary).to_lowercase();
    if status.contains("failed") || status.contains("error") {
        theme.ERROR
    } else if status.contains("cancel") || status.contains("superseded") {
        theme.WARN
    } else if status.contains("completed") {
        theme.OK
    } else {
        theme.TEXT_DIM
    }
}

fn completed_card(
    index: usize,
    task_id: &str,
    label: &str,
    summary: &str,
    selection: &Entity<TextSelection>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = Theme::global();
    div()
        .id(("background-task", index))
        .debug_selector(|| "background-task-card".into())
        .flex()
        .flex_none()
        .flex_col()
        .min_w_0()
        .rounded_lg()
        .overflow_hidden()
        .bg(theme.CODE_BG)
        .px_3()
        .py_2()
        .gap_1()
        .child(
            div()
                .debug_selector(|| "background-task-completed".into())
                .flex()
                .items_center()
                .gap_2()
                .child(crate::tool_icon::render("bg"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.TOOL_TEXT)
                        .child(text_selection::plain(
                            selection.clone(),
                            format!("background-label-{index}"),
                            label.to_owned(),
                            window,
                            cx,
                        )),
                ),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(completion_color(summary))
                .child(text_selection::plain(
                    selection.clone(),
                    format!("background-summary-{index}"),
                    summary.to_owned(),
                    window,
                    cx,
                )),
        )
        .child(
            div()
                .text_size(px(10.0))
                .font_family(theme.FONT_MONO)
                .text_color(theme.TEXT_FAINT)
                .child(format!("Background task · {task_id}")),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn completion_status_does_not_assume_success() {
        let theme = Theme::global();
        assert_eq!(completion_color("✓ completed · 8.2s · exit 0"), theme.OK);
        assert_eq!(completion_color("✗ failed · 2s · exit 1"), theme.ERROR);
        assert_eq!(completion_color("cancelled · 2s"), theme.WARN);
        assert_eq!(completion_color("finished"), theme.TEXT_DIM);
    }
}
