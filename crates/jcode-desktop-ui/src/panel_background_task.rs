//! Background work is a quiet status row, not a full transcript card.
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
    label: &str,
    summary: &str,
    percent: Option<f32>,
    done: bool,
    selection: &Entity<TextSelection>,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = Theme::global();
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
}
