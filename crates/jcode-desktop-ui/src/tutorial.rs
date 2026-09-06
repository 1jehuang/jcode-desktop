//! A quiet, opt-in tutorial inside the top-left Learn tab.
use super::*;

type TutorialAction = fn(&mut Workspace, &mut Window, &mut Context<Workspace>);
type Lesson = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<TutorialAction>,
);

impl Workspace {
    pub(super) fn render_tutorial_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected = self.sidebar_view == SidebarView::Learn;
        div()
            .id("sidebar-learn-tab")
            .debug_selector(|| "sidebar-learn-tab".into())
            .flex_none()
            .border_1()
            .border_color(Theme::global().PANEL_BORDER)
            .px_2()
            .py_1()
            .rounded_t_md()
            .h(px(if selected { 34.0 } else { 30.0 }))
            .flex()
            .items_center()
            .when(selected, |el| el.border_b_0().bg(Theme::global().HEADER_BG))
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(if selected {
                Theme::global().TEXT
            } else {
                Theme::global().TEXT_DIM
            })
            .hover(|el| el.bg(Theme::global().HEADER_BG))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.sidebar_view = SidebarView::Learn;
                    cx.notify();
                }),
            )
            .child("learn")
            .into_any_element()
    }

    pub(super) fn render_tutorial_guides(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let page = self.tutorial_page.min(2);
        let heading = if self.onboarding_complete() {
            "SHORTCUT REFERENCE"
        } else {
            "GETTING STARTED"
        };
        let modifier = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Super"
        };
        let (title, description) = match page {
            0 => (
                "Find your way",
                "Switch between panels and start a new session.",
            ),
            1 => (
                "Make it yours",
                "Move panels into place and choose how much space they get.",
            ),
            _ => (
                "Take a step back",
                "Adjust your view or see the whole workspace at once.",
            ),
        };
        let lessons: Vec<Lesson> = match page {
            0 => vec![
                (
                    "tutorial-nav-left",
                    "←  Focus left",
                    "H",
                    "focus_left_right",
                    Some(|this, w, cx| this.focus_left(&FocusLeft, w, cx)),
                ),
                (
                    "tutorial-nav-down",
                    "↓  Strip below",
                    "J",
                    "focus_up_down",
                    Some(|this, w, cx| this.focus_down(&FocusDown, w, cx)),
                ),
                (
                    "tutorial-nav-up",
                    "↑  Strip above",
                    "K",
                    "focus_up_down",
                    Some(|this, w, cx| this.focus_up(&FocusUp, w, cx)),
                ),
                (
                    "tutorial-nav-right",
                    "→  Focus right",
                    "L",
                    "focus_left_right",
                    Some(|this, w, cx| this.focus_right(&FocusRight, w, cx)),
                ),
                (
                    "tutorial-new",
                    "New session",
                    "Enter",
                    "new_panel",
                    Some(|this, w, cx| this.new_panel(&NewPanel, w, cx)),
                ),
                (
                    "tutorial-close",
                    "Close panel",
                    "Q",
                    "close_panel",
                    Some(|this, w, cx| this.close_panel(&ClosePanel, w, cx)),
                ),
            ],
            1 => vec![
                (
                    "tutorial-nav-left",
                    "←  Move left",
                    "Shift H",
                    "move_panel",
                    Some(|this, w, cx| this.move_panel_left(&MovePanelLeft, w, cx)),
                ),
                (
                    "tutorial-nav-down",
                    "↓  Move down",
                    "Shift J",
                    "move_panel_strip",
                    Some(|this, w, cx| this.move_panel_down(&MovePanelDown, w, cx)),
                ),
                (
                    "tutorial-nav-up",
                    "↑  Move up",
                    "Shift K",
                    "move_panel_strip",
                    Some(|this, w, cx| this.move_panel_up(&MovePanelUp, w, cx)),
                ),
                (
                    "tutorial-nav-right",
                    "→  Move right",
                    "Shift L",
                    "move_panel",
                    Some(|this, w, cx| this.move_panel_right(&MovePanelRight, w, cx)),
                ),
                (
                    "tutorial-width-presets",
                    "Panel width",
                    "1 2 3 4",
                    "width_presets",
                    None,
                ),
            ],
            _ => vec![
                (
                    "tutorial-resize",
                    "Cycle width",
                    "R",
                    "cycle_width",
                    Some(|this, w, cx| this.cycle_width(&CycleWidth, w, cx)),
                ),
                (
                    "tutorial-overview",
                    "Overview",
                    "O",
                    "overview",
                    Some(|this, w, cx| this.toggle_overview(&ToggleOverview, w, cx)),
                ),
            ],
        };
        let rows = lessons.into_iter().map(|(id, label, key, skill, action)| {
            let practiced = self.coach.trace(skill).practiced();
            div()
                .id(id)
                .debug_selector(move || id.into())
                .w_full()
                .min_h(px(30.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .py_1()
                .text_size(px(12.0))
                .text_color(Theme::global().TEXT)
                .when_some(action, |el, action| {
                    el.cursor_pointer()
                        .hover(|el| el.text_color(Theme::global().ACCENT))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, w, cx| {
                                action(this, w, cx);
                                cx.stop_propagation();
                            }),
                        )
                })
                .child(div().min_w_0().flex_1().child(label))
                .when(practiced, |el| {
                    el.child(
                        div()
                            .debug_selector(move || format!("tutorial-learned-{skill}").into())
                            .text_size(px(10.0))
                            .text_color(Theme::global().OK)
                            .child("✓"),
                    )
                })
                .child(if action.is_some() {
                    div()
                        .flex_none()
                        .px_1()
                        .py(px(2.0))
                        .rounded_sm()
                        .bg(Theme::global().BG)
                        .font_family(Theme::global().FONT_MONO)
                        .text_size(px(10.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(key)
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_none()
                        .gap_1()
                        .children((1..=4).map(|preset| {
                            div()
                                .id(format!("tutorial-width-{preset}"))
                                .debug_selector(move || format!("tutorial-width-{preset}").into())
                                .px_1()
                                .py(px(2.0))
                                .rounded_sm()
                                .bg(Theme::global().BG)
                                .font_family(Theme::global().FONT_MONO)
                                .text_size(px(10.0))
                                .cursor_pointer()
                                .hover(|el| el.text_color(Theme::global().ACCENT))
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_width(preset as f32 / 4.0, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(preset.to_string())
                        }))
                        .into_any_element()
                })
        });
        div()
            .id("tutorial-guides")
            .debug_selector(|| "tutorial-guides".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .px_4()
            .py_4()
            .child(
                div()
                    .id(format!("tutorial-content-{page}"))
                    .debug_selector(|| "tutorial-content".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .debug_selector(|| "tutorial-stage".into())
                            .text_size(px(10.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(format!("{heading}  ·  {} / 3", page + 1)),
                    )
                    .child(div().mt_3().text_size(px(19.0)).child(title))
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(12.0))
                            .line_height(relative(1.5))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(description),
                    )
                    .child(
                        div()
                            .mt_4()
                            .mb_2()
                            .text_size(px(10.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(format!(
                                "Hold {modifier}, then press a key. Or click a row."
                            )),
                    )
                    .children(rows),
            )
            .child(
                div()
                    .flex_none()
                    .mt_3()
                    .pt_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("tutorial-back")
                            .debug_selector(|| "tutorial-back".into())
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .when(page > 0, |el| {
                                el.cursor_pointer()
                                    .hover(|el| el.text_color(Theme::global().TEXT))
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.tutorial_page =
                                                this.tutorial_page.saturating_sub(1);
                                            cx.notify();
                                        }),
                                    )
                                    .child("← Back")
                            }),
                    )
                    .child(
                        div()
                            .id("tutorial-next")
                            .debug_selector(|| "tutorial-next".into())
                            .px_2()
                            .py_1()
                            .text_size(px(11.0))
                            .text_color(Theme::global().ACCENT)
                            .cursor_pointer()
                            .hover(|el| el.text_color(Theme::global().TEXT))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if this.tutorial_page < 2 {
                                        this.tutorial_page += 1;
                                    } else {
                                        this.sidebar_view = SidebarView::Sessions;
                                    }
                                    cx.notify();
                                }),
                            )
                            .child(if page < 2 { "Next →" } else { "Done" }),
                    ),
            )
            .into_any_element()
    }
}
