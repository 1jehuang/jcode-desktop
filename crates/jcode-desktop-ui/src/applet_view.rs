//! Native GPUI renderer for applet view trees.
//!
//! Providers choose structure and intent only. Every color, font, radius and
//! spacing value comes from the theme, and controls are always pills, so an
//! applet cannot break Desktop's visual language. Unknown nodes render their
//! fallback text. Images reserve their layout box before pixels decode.
use crate::applet_host::AssetCache;
use crate::input::PromptInput;
use crate::text_selection::{self, TextSelection};
use crate::theme::Theme;
use gpui::{prelude::*, *};
use jcode_applet_types::{
    Document, ImageFit, ImageSource,
    view::{
        Action, Align, Axis, ButtonVariant, ImageShape, Node, NodeKind, Space, TextStyle, Tone,
    },
};
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

/// User intents raised by rendered nodes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ViewEvent {
    Action {
        action: Action,
        source_key: Option<String>,
    },
    SetState {
        key: String,
        value: Value,
        /// Fire this action after the state update, e.g. a toggle's on_change.
        then: Option<Action>,
    },
}

pub(crate) type Emit = Rc<dyn Fn(ViewEvent, &mut App)>;

pub(crate) struct RenderCx<'a> {
    /// Stable per-instance prefix for element ids and keyed state.
    pub scope: &'a str,
    pub document: &'a Document,
    pub assets: &'a RefCell<AssetCache>,
    pub selection: &'a Entity<TextSelection>,
    pub emit: Emit,
    /// Compact placements (sidebar, overlay, inline) tighten spacing.
    pub compact: bool,
    /// Capabilities the applet declared AND the user granted. Nodes that
    /// need an ungranted capability render a placeholder instead.
    pub granted: &'a [jcode_applet_types::Capability],
}

fn space(value: Space, compact: bool) -> Pixels {
    let scale = if compact { 0.75 } else { 1.0 };
    px(scale
        * match value {
            Space::None => 0.0,
            Space::Xs => 4.0,
            Space::Sm => 8.0,
            Space::Md => 12.0,
            Space::Lg => 16.0,
            Space::Xl => 24.0,
        })
}

fn tone(value: Tone) -> Rgba {
    let theme = Theme::global();
    match value {
        Tone::Default => theme.TEXT,
        Tone::Dim => theme.TEXT_DIM,
        Tone::Accent => theme.ACCENT,
        Tone::Success => theme.OK,
        Tone::Warning => theme.WARN,
        Tone::Danger => theme.ERROR,
    }
}

fn tone_fill(value: Tone) -> Rgba {
    let theme = Theme::global();
    let mut color = match value {
        Tone::Default | Tone::Dim => return theme.INLINE_CODE_BG,
        Tone::Accent => return theme.ACCENT_DIM,
        Tone::Success => theme.OK,
        Tone::Warning => theme.WARN,
        Tone::Danger => theme.ERROR,
    };
    color.a = 0.16;
    color
}

/// A deterministic element id. Keys win so identity survives reordering.
fn id(cx: &RenderCx, path: &str, node: &Node) -> ElementId {
    match &node.key {
        Some(key) => ElementId::Name(format!("{}:k:{key}", cx.scope).into()),
        None => ElementId::Name(format!("{}:p:{path}", cx.scope).into()),
    }
}

fn state_str(cx: &RenderCx, key: &str) -> String {
    match cx.document.state.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

pub(crate) fn render(cx: &RenderCx, window: &mut Window, app: &mut App) -> AnyElement {
    node(cx, &cx.document.view, "0", window, app)
}

fn children(
    cx: &RenderCx,
    nodes: &[Node],
    path: &str,
    window: &mut Window,
    app: &mut App,
) -> Vec<AnyElement> {
    nodes
        .iter()
        .enumerate()
        .map(|(index, child)| node(cx, child, &format!("{path}.{index}"), window, app))
        .collect()
}

fn on_press(
    cx: &RenderCx,
    action: &Action,
    key: Option<&String>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let emit = cx.emit.clone();
    let action = action.clone();
    let source_key = key.cloned();
    move |_, _, app| {
        app.stop_propagation();
        emit(
            ViewEvent::Action {
                action: action.clone(),
                source_key: source_key.clone(),
            },
            app,
        )
    }
}

fn text_el(
    cx: &RenderCx,
    key: String,
    text: &str,
    selectable: bool,
    window: &Window,
    app: &App,
) -> AnyElement {
    if selectable {
        text_selection::plain(cx.selection.clone(), key, text.to_owned(), window, app)
    } else {
        div().child(text.to_owned()).into_any_element()
    }
}

fn node(
    cx: &RenderCx,
    node_ref: &Node,
    path: &str,
    window: &mut Window,
    app: &mut App,
) -> AnyElement {
    let theme = Theme::global();
    let key = format!("{}:t:{path}", cx.scope);
    match &node_ref.kind {
        NodeKind::Stack {
            direction,
            gap,
            padding,
            align,
            children: kids,
        } => {
            let mut el = div()
                .flex()
                .gap(space(*gap, cx.compact))
                .p(space(*padding, cx.compact))
                .min_w_0();
            el = match direction {
                Axis::Vertical => el.flex_col(),
                Axis::Horizontal => el.flex_row().flex_wrap(),
            };
            el = match (direction, align) {
                (_, Align::Start) => el.items_start(),
                (_, Align::Center) => el.items_center(),
                (_, Align::End) => el.items_end(),
                (Axis::Vertical, Align::Stretch | Align::Between) => el,
                (Axis::Horizontal, Align::Stretch) => el.items_stretch(),
                (Axis::Horizontal, Align::Between) => el.items_center().justify_between(),
            };
            if matches!(direction, Axis::Vertical) && matches!(align, Align::Start) {
                // Vertical stacks stretch children to the column by default so
                // text wraps. Explicit alignment opts out.
                el = el.items_stretch();
            }
            el.children(children(cx, kids, path, window, app))
                .into_any_element()
        }
        NodeKind::Grid {
            min_column_width,
            gap,
            children: kids,
        } => {
            let width = (*min_column_width).clamp(48, 640) as f32;
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(space(*gap, cx.compact))
                .children(
                    children(cx, kids, path, window, app)
                        .into_iter()
                        .map(|child| {
                            div()
                                .flex_grow(1.)
                                .flex_basis(px(width))
                                .min_w(px(width.min(120.)))
                                .child(child)
                        }),
                )
                .into_any_element()
        }
        NodeKind::Scroll {
            max_height,
            children: kids,
        } => {
            let mut el = div()
                .id(id(cx, path, node_ref))
                .flex()
                .flex_col()
                .gap(space(Space::Sm, cx.compact))
                .overflow_y_scroll()
                .min_h_0();
            if let Some(height) = max_height {
                el = el.max_h(px((*height).clamp(40, 4000) as f32));
            } else {
                el = el.flex_1();
            }
            el.children(children(cx, kids, path, window, app))
                .into_any_element()
        }
        NodeKind::Card {
            title,
            children: kids,
        } => div()
            .flex()
            .flex_col()
            .gap(space(Space::Sm, cx.compact))
            .p(space(Space::Md, cx.compact))
            .rounded_xl()
            .bg(theme.TOOL_BG)
            .when_some(title.as_ref(), |el, title| {
                el.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.HEADING)
                        .child(title.clone()),
                )
            })
            .children(children(cx, kids, path, window, app))
            .into_any_element(),
        NodeKind::Tabs {
            bind,
            tabs,
            on_change,
        } => {
            let selected = state_str(cx, bind);
            let active = tabs.iter().position(|tab| tab.id == selected).unwrap_or(0);
            let mut bar = div().flex().flex_row().flex_wrap().gap_1();
            for (index, tab) in tabs.iter().enumerate() {
                let emit = cx.emit.clone();
                let bind = bind.clone();
                let tab_id = tab.id.clone();
                let then = on_change.clone();
                bar = bar.child(pill(
                    ElementId::Name(format!("{}:tab:{path}:{index}", cx.scope).into()),
                    &tab.label,
                    index == active,
                    move |_, _, app| {
                        emit(
                            ViewEvent::SetState {
                                key: bind.clone(),
                                value: Value::String(tab_id.clone()),
                                then: then.clone(),
                            },
                            app,
                        )
                    },
                ));
            }
            let body = tabs
                .get(active)
                .map(|tab| children(cx, &tab.children, &format!("{path}.t{active}"), window, app))
                .unwrap_or_default();
            div()
                .flex()
                .flex_col()
                .gap(space(Space::Md, cx.compact))
                .child(bar)
                .children(body)
                .into_any_element()
        }
        NodeKind::Spacer => div().flex_1().into_any_element(),
        NodeKind::Divider => div()
            .h(px(1.))
            .w_full()
            .bg(theme.PANEL_BORDER)
            .into_any_element(),
        NodeKind::Text {
            text,
            style,
            tone: t,
            max_lines,
            selectable,
        } => {
            let mut el = div().text_color(tone(*t)).min_w_0();
            el = match style {
                TextStyle::Body => el.text_size(px(13.)),
                TextStyle::Title => el
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.HEADING),
                TextStyle::Heading => el
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.HEADING),
                TextStyle::Caption => el.text_size(px(11.)).text_color(theme.TEXT_DIM),
                TextStyle::Mono => el.text_size(px(12.)).font_family(theme.FONT_MONO),
            };
            if matches!(t, Tone::Default | Tone::Dim) && matches!(style, TextStyle::Caption) {
                el = el.text_color(tone(Tone::Dim));
            } else if !matches!(t, Tone::Default) {
                el = el.text_color(tone(*t));
            }
            if let Some(lines) = max_lines {
                el = el.line_clamp((*lines).clamp(1, 200) as usize);
                // Clamped text is laid out as a single styled run.
                return el.child(text.clone()).into_any_element();
            }
            el.child(text_el(cx, key, text, *selectable, window, app))
                .into_any_element()
        }
        NodeKind::Markdown { text } => div()
            .min_w_0()
            .child(crate::markdown::render(
                text,
                stable_row(&key),
                cx.selection,
                window,
                app,
            ))
            .into_any_element(),
        NodeKind::Code { text, language } => {
            crate::markdown::code_block(language.as_deref().unwrap_or(""), text, window)
        }
        NodeKind::Image {
            source,
            alt,
            width,
            height,
            aspect_ratio,
            fit,
            shape,
            on_press: press,
        } => image(
            cx,
            node_ref,
            path,
            source,
            alt,
            *width,
            *height,
            *aspect_ratio,
            *fit,
            *shape,
            press.as_ref(),
        ),
        NodeKind::Icon { name, tone: t } => div()
            .text_color(tone(*t))
            .text_size(px(14.))
            .child(icon_glyph(name))
            .into_any_element(),
        NodeKind::KeyValue { rows } => {
            let mut grid = div().flex().flex_col().gap_1();
            for (index, row) in rows.iter().enumerate() {
                grid = grid.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_3()
                        .text_size(px(13.))
                        .child(
                            div()
                                .w(px(120.))
                                .flex_none()
                                .text_color(theme.TEXT_DIM)
                                .child(row.key.clone()),
                        )
                        .child(div().flex_1().min_w_0().child(text_el(
                            cx,
                            format!("{key}:kv{index}"),
                            &row.value,
                            true,
                            window,
                            app,
                        ))),
                );
            }
            grid.into_any_element()
        }
        NodeKind::Table { columns, rows } => {
            let cell = |text: String, header: bool| {
                div()
                    .flex_1()
                    .min_w(px(60.))
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .when(header, |el| {
                        el.font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.TEXT_DIM)
                    })
                    .child(text)
            };
            let mut table = div()
                .id(id(cx, path, node_ref))
                .flex()
                .flex_col()
                .overflow_x_scroll()
                .rounded_xl()
                .bg(theme.TOOL_BG)
                .py_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .children(columns.iter().map(|c| cell(c.clone(), true))),
                );
            for (index, row) in rows.iter().enumerate() {
                table = table.child(
                    div()
                        .flex()
                        .flex_row()
                        .when(index % 2 == 0, |el| el.bg(theme.TABLE_STRIPE))
                        .children(row.iter().map(|c| cell(c.clone(), false))),
                );
            }
            table.into_any_element()
        }
        NodeKind::Progress { value, label } => {
            let fraction = value.unwrap_or(0.35).clamp(0.0, 1.0);
            div()
                .flex()
                .flex_col()
                .gap_1()
                .when_some(label.as_ref(), |el, label| {
                    el.child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.TEXT_DIM)
                            .child(label.clone()),
                    )
                })
                .child(
                    div()
                        .h(px(6.))
                        .w_full()
                        .rounded_full()
                        .bg(theme.INLINE_CODE_BG)
                        .child(div().h_full().w(relative(fraction)).rounded_full().bg(
                            if value.is_some() {
                                theme.ACCENT
                            } else {
                                theme.ACCENT_MUTED
                            },
                        )),
                )
                .into_any_element()
        }
        NodeKind::Empty {
            title,
            detail,
            icon,
        } => div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py(space(Space::Xl, cx.compact))
            .text_color(theme.TEXT_DIM)
            .when_some(icon.as_ref(), |el, icon| {
                el.child(div().text_size(px(22.)).child(icon_glyph(icon)))
            })
            .child(div().text_color(theme.TEXT).child(title.clone()))
            .when_some(detail.as_ref(), |el, detail| {
                el.child(div().text_size(px(12.)).child(detail.clone()))
            })
            .into_any_element(),
        NodeKind::Error { message, retry } => div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .rounded_full()
            .bg(theme.ERROR_BG)
            .text_size(px(12.))
            .text_color(theme.ERROR)
            .child(div().flex_1().min_w_0().child(message.clone()))
            .when_some(retry.as_ref(), |el, action| {
                el.child(button(
                    ElementId::Name(format!("{}:retry:{path}", cx.scope).into()),
                    "Retry",
                    ButtonVariant::Compact,
                    None,
                    false,
                    on_press(cx, action, node_ref.key.as_ref()),
                ))
            })
            .into_any_element(),
        NodeKind::Button {
            label,
            variant,
            icon,
            on_press: action,
            disabled,
        } => button(
            id(cx, path, node_ref),
            label,
            *variant,
            icon.as_deref(),
            *disabled,
            on_press(cx, action, node_ref.key.as_ref()),
        ),
        NodeKind::Chip {
            label,
            tone: t,
            icon,
            on_press: action,
        } => {
            let mut el = div()
                .id(id(cx, path, node_ref))
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(2.))
                .rounded_full()
                .bg(tone_fill(*t))
                .text_size(px(11.))
                .text_color(if matches!(t, Tone::Default) {
                    theme.TEXT_DIM
                } else {
                    tone(*t)
                })
                .when_some(icon.as_deref(), |el, icon| el.child(icon_glyph(icon)))
                .child(label.clone());
            if let Some(action) = action {
                el = el
                    .cursor_pointer()
                    .hover(|el| el.opacity(0.85))
                    .on_click(on_press(cx, action, node_ref.key.as_ref()));
            }
            el.into_any_element()
        }
        NodeKind::Toggle {
            label,
            bind,
            on_change,
        } => {
            let on = cx
                .document
                .state
                .get(bind)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let emit = cx.emit.clone();
            let bind = bind.clone();
            let then = on_change.clone();
            div()
                .id(id(cx, path, node_ref))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .on_click(move |_, _, app| {
                    app.stop_propagation();
                    emit(
                        ViewEvent::SetState {
                            key: bind.clone(),
                            value: Value::Bool(!on),
                            then: then.clone(),
                        },
                        app,
                    )
                })
                .child(
                    div()
                        .w(px(30.))
                        .h(px(18.))
                        .p(px(2.))
                        .rounded_full()
                        .flex()
                        .when(on, |el| el.justify_end())
                        .bg(if on {
                            theme.ACCENT
                        } else {
                            theme.INLINE_CODE_BG
                        })
                        .child(div().size(px(14.)).rounded_full().bg(theme.PANEL_BG)),
                )
                .child(div().text_size(px(13.)).child(label.clone()))
                .into_any_element()
        }
        NodeKind::Input {
            bind,
            placeholder,
            multiline,
            on_submit,
        } => text_input(
            cx,
            path,
            node_ref,
            bind,
            placeholder,
            *multiline,
            on_submit.clone(),
            window,
            app,
        ),
        NodeKind::Select {
            bind,
            options,
            on_change,
        } => {
            // A segmented pill row: every option is visible and one click away.
            let selected = state_str(cx, bind);
            let mut row = div().flex().flex_row().flex_wrap().gap_1();
            for (index, option) in options.iter().enumerate() {
                let emit = cx.emit.clone();
                let bind = bind.clone();
                let value = option.value.clone();
                let then = on_change.clone();
                row = row.child(pill(
                    ElementId::Name(format!("{}:opt:{path}:{index}", cx.scope).into()),
                    &option.label,
                    option.value == selected,
                    move |_, _, app| {
                        emit(
                            ViewEvent::SetState {
                                key: bind.clone(),
                                value: Value::String(value.clone()),
                                then: then.clone(),
                            },
                            app,
                        )
                    },
                ));
            }
            row.into_any_element()
        }
        NodeKind::List { children: kids } => div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(children(cx, kids, path, window, app))
            .into_any_element(),
        NodeKind::ListItem {
            title,
            subtitle,
            meta,
            leading,
            badges,
            emphasized,
            on_press: action,
        } => {
            let mut row = div()
                .id(id(cx, path, node_ref))
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py(px(if subtitle.is_some() { 6. } else { 4. }))
                .rounded_full()
                .bg(theme.TOOL_BG)
                .when_some(leading.as_deref(), |el, lead| {
                    el.child(div().flex_none().child(node(
                        cx,
                        lead,
                        &format!("{path}.lead"),
                        window,
                        app,
                    )))
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.))
                                .truncate()
                                .when(*emphasized, |el| {
                                    el.font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.HEADING)
                                })
                                .child(title.clone()),
                        )
                        .when_some(subtitle.as_ref(), |el, subtitle| {
                            el.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.TEXT_DIM)
                                    .truncate()
                                    .child(subtitle.clone()),
                            )
                        }),
                )
                .children(badges.iter().map(|badge| {
                    div()
                        .flex_none()
                        .px_2()
                        .rounded_full()
                        .bg(theme.ACCENT_DIM)
                        .text_size(px(10.))
                        .child(badge.clone())
                }))
                .when_some(meta.as_ref(), |el, meta| {
                    el.child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme.TEXT_DIM)
                            .child(meta.clone()),
                    )
                });
            if let Some(action) = action {
                row = row
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.INLINE_CODE_BG))
                    .on_click(on_press(cx, action, node_ref.key.as_ref()));
            }
            row.into_any_element()
        }
        NodeKind::Html { .. } if !cx.granted.contains(&jcode_applet_types::Capability::Html) => {
            div()
                .px_3()
                .py_1()
                .rounded_full()
                .bg(theme.INLINE_CODE_BG)
                .text_size(px(12.))
                .text_color(theme.TEXT_DIM)
                .child("Web content is blocked until you allow it for this applet.")
                .into_any_element()
        }
        NodeKind::Html { source, .. } => {
            crate::html_preview::HtmlPreview::new(source.clone(), stable_row(&key), 0)
                .into_any_element()
        }
        NodeKind::Unknown { type_name, .. } => div()
            .text_size(px(12.))
            .text_color(theme.TEXT_DIM)
            .child(
                node_ref
                    .fallback
                    .clone()
                    .unwrap_or_else(|| format!("Unsupported applet element: {type_name}")),
            )
            .into_any_element(),
    }
}

fn stable_row(key: &str) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    // Keep clear of transcript row indices, which count up from zero.
    (hasher.finish() as usize) | (1 << (usize::BITS - 2))
}

fn pill(
    id: ElementId,
    label: &str,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let theme = Theme::global();
    div()
        .id(id)
        .px_3()
        .py(px(3.))
        .rounded_full()
        .cursor_pointer()
        .text_size(px(12.))
        .bg(if selected {
            theme.ACCENT_DIM
        } else {
            theme.INLINE_CODE_BG
        })
        .text_color(if selected { theme.TEXT } else { theme.TEXT_DIM })
        .hover(|el| el.bg(theme.ACCENT_DIM))
        .on_click(move |event, window, app| {
            app.stop_propagation();
            on_click(event, window, app)
        })
        .child(label.to_owned())
        .into_any_element()
}

fn button(
    id: ElementId,
    label: &str,
    variant: ButtonVariant,
    icon: Option<&str>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let theme = Theme::global();
    let (bg, fg, hover) = match variant {
        ButtonVariant::Primary => (theme.ACCENT, theme.PANEL_BG, theme.ACCENT_MUTED),
        ButtonVariant::Secondary | ButtonVariant::Compact => {
            (theme.INLINE_CODE_BG, theme.TEXT, theme.ACCENT_DIM)
        }
        ButtonVariant::Danger => (theme.ERROR_BG, theme.ERROR, theme.ERROR_BG),
    };
    let compact = matches!(variant, ButtonVariant::Compact);
    div()
        .id(id)
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap_1()
        .px(px(if compact { 8. } else { 14. }))
        .py(px(if compact { 2. } else { 5. }))
        .rounded_full()
        .text_size(px(if compact { 11. } else { 13. }))
        .font_weight(if matches!(variant, ButtonVariant::Primary) {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .bg(bg)
        .text_color(fg)
        .when(disabled, |el| el.opacity(0.45))
        .when(!disabled, |el| {
            el.cursor_pointer()
                .hover(move |el| el.bg(hover))
                .on_click(on_click)
        })
        .when_some(icon, |el, icon| el.child(icon_glyph(icon)))
        .child(label.to_owned())
        .into_any_element()
}

/// A small built-in icon vocabulary. Unknown names render nothing rather than
/// arbitrary provider text.
fn icon_glyph(name: &str) -> &'static str {
    match name {
        "mail" => "✉",
        "check" => "✓",
        "x" | "close" => "✕",
        "plus" => "+",
        "search" => "⌕",
        "star" => "★",
        "clock" => "◷",
        "image" => "▣",
        "link" => "↗",
        "refresh" => "↻",
        "warning" => "⚠",
        "info" => "ⓘ",
        "user" => "●",
        "folder" => "▤",
        "play" => "▶",
        "pause" => "❚❚",
        "arrow-right" => "→",
        "arrow-left" => "←",
        "sparkles" => "✦",
        "pull-request" => "⇄",
        "issue" => "◉",
        "comment" => "✎",
        "branch" => "⑂",
        _ => "",
    }
}

#[allow(clippy::too_many_arguments)]
fn image(
    cx: &RenderCx,
    node_ref: &Node,
    path: &str,
    source: &ImageSource,
    alt: &str,
    width: Option<u32>,
    height: Option<u32>,
    aspect_ratio: Option<f32>,
    fit: ImageFit,
    shape: ImageShape,
    press: Option<&Action>,
) -> AnyElement {
    let theme = Theme::global();
    let decoded = match source {
        ImageSource::Asset(asset_id) => cx
            .document
            .assets
            .iter()
            .find(|asset| &asset.id == asset_id)
            .and_then(|asset| cx.assets.borrow_mut().image(&asset.mime, &asset.data))
            .map(crate::image_cache::source),
        ImageSource::Data(uri) => cx
            .assets
            .borrow_mut()
            .data_uri(uri)
            .map(crate::image_cache::source),
        ImageSource::Path(path)
            if cx
                .granted
                .contains(&jcode_applet_types::Capability::ReadFiles) =>
        {
            Some(gpui::ImageSource::from(std::path::PathBuf::from(path)))
        }
        ImageSource::Url(url)
            if cx
                .granted
                .contains(&jcode_applet_types::Capability::RemoteImages) =>
        {
            Some(gpui::ImageSource::from(url.clone()))
        }
        ImageSource::Path(_) | ImageSource::Url(_) => None,
    };
    let mut frame = div()
        .id(id(cx, path, node_ref))
        .overflow_hidden()
        .flex_none()
        .bg(theme.INLINE_CODE_BG);
    frame = match shape {
        ImageShape::Rounded => frame.rounded_lg(),
        ImageShape::Circle => frame.rounded_full(),
        ImageShape::Square => frame,
    };
    match (width, height) {
        (Some(w), Some(h)) => {
            frame = frame
                .w(px(w.clamp(8, 4096) as f32))
                .h(px(h.clamp(8, 4096) as f32))
        }
        (Some(w), None) => frame = frame.w(px(w.clamp(8, 4096) as f32)),
        (None, Some(h)) => frame = frame.h(px(h.clamp(8, 4096) as f32)),
        (None, None) => frame = frame.w_full(),
    }
    if width.is_none() || height.is_none() {
        // Reserve the box before decode so the layout never jumps.
        frame = frame.aspect_ratio(aspect_ratio.unwrap_or(
            if width.is_none() && height.is_none() {
                16. / 9.
            } else {
                1.
            },
        ));
    }
    let object_fit = match fit {
        ImageFit::Contain => ObjectFit::Contain,
        ImageFit::Cover => ObjectFit::Cover,
        ImageFit::Fill => ObjectFit::Fill,
    };
    let alt_text: SharedString = alt.to_owned().into();
    let placeholder = move |text: SharedString| {
        move || {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_1()
                .text_size(px(11.))
                .text_color(Theme::global().TEXT_DIM)
                .child(text.clone())
                .into_any_element()
        }
    };
    frame = match decoded {
        Some(source) => frame.child(
            match shape {
                // GPUI clips image sprites by their own radii, not the parent's.
                ImageShape::Rounded => img(source).rounded_lg(),
                ImageShape::Circle => img(source).rounded_full(),
                ImageShape::Square => img(source),
            }
            .size_full()
            .object_fit(object_fit)
            .with_fallback(placeholder(alt_text.clone()))
            .with_loading(placeholder(SharedString::default())),
        ),
        None => frame.child(placeholder(alt_text)()),
    };
    if let Some(action) = press {
        frame = frame
            .cursor_pointer()
            .hover(|el| el.opacity(0.9))
            .on_click(on_press(cx, action, node_ref.key.as_ref()));
    }
    frame.into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn text_input(
    cx: &RenderCx,
    path: &str,
    node_ref: &Node,
    bind: &str,
    placeholder: &str,
    multiline: bool,
    on_submit: Option<Action>,
    window: &mut Window,
    app: &mut App,
) -> AnyElement {
    let theme = Theme::global();
    let state_key = match &node_ref.key {
        Some(key) => format!("{}:input:k:{key}", cx.scope),
        None => format!("{}:input:{bind}:{path}", cx.scope),
    };
    let initial = state_str(cx, bind);
    let emit = cx.emit.clone();
    let bind_owned = bind.to_owned();
    let placeholder = placeholder.to_owned();
    let input = window.use_keyed_state(ElementId::Name(state_key.into()), app, move |_, icx| {
        let submit_emit = emit.clone();
        let submit_bind = bind_owned.clone();
        let change_emit = emit.clone();
        let change_bind = bind_owned.clone();
        let mut input = PromptInput::new(icx, placeholder, move |text, _, _, app| {
            // Deferred: the field is still mid-update when Enter fires.
            let emit = submit_emit.clone();
            let bind = submit_bind.clone();
            let then = on_submit.clone();
            app.defer(move |app| {
                emit(
                    ViewEvent::SetState {
                        key: bind,
                        value: Value::String(text),
                        then,
                    },
                    app,
                )
            });
        })
        .without_command_completion()
        .without_chrome()
        .with_on_change(move |text, app| {
            let emit = change_emit.clone();
            let bind = change_bind.clone();
            let text = text.to_owned();
            app.defer(move |app| {
                emit(
                    ViewEvent::SetState {
                        key: bind,
                        value: Value::String(text),
                        then: None,
                    },
                    app,
                )
            });
        });
        if !initial.is_empty() {
            input.set_content(initial, icx);
        }
        input
    });
    // Provider-driven resets (e.g. clearing a field after submit) flow back in.
    let current = state_str(cx, bind);
    if input.read(app).content.as_ref() != current {
        input.update(app, |input, icx| input.set_content(current, icx));
    }
    div()
        .px_3()
        .py(px(if multiline { 6. } else { 3. }))
        .rounded(px(if multiline { 12. } else { 999. }))
        .bg(theme.INPUT_BG)
        .border_1()
        .border_color(theme.INPUT_BORDER)
        .when(multiline, |el| el.min_h(px(64.)))
        .child(input)
        .into_any_element()
}
