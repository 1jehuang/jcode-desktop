//! Shared rendering and event handling for applet instances, used by every
//! placement: panels, transcript cards, composer strips, sidebar sections and
//! overlays. One code path means an instance behaves identically wherever it
//! is placed, and `Move` never changes how its controls work.
use crate::applet_host::Effect;
use crate::applet_view::{self, RenderCx, ViewEvent};
use crate::text_selection::TextSelection;
use crate::theme::Theme;
use gpui::{prelude::*, *};
use std::rc::Rc;

/// Perform a view event for an instance. Effects that only need the app
/// (links, clipboard, files) run here. Effects that need a workspace (new
/// chats, prompts into sessions) are queued on the runtime for the workspace
/// loop. Returns the effect so a panel can react to `Closed`.
pub(crate) fn handle(instance: &str, event: ViewEvent, app: &mut App) -> Option<Effect> {
    let runtime = crate::applet_runtime::get(app);
    let effect = match event {
        ViewEvent::Action { action, source_key } => runtime.dispatch(instance, &action, source_key),
        ViewEvent::SetState { key, value, then } => {
            runtime.set_state(instance, &key, value);
            then.and_then(|action| runtime.dispatch(instance, &action, None))
        }
    };
    match &effect {
        Some(Effect::OpenUrl(url)) => app.open_url(url),
        Some(Effect::Copy(text)) => app.write_to_clipboard(ClipboardItem::new_string(text.clone())),
        Some(Effect::OpenFile(path)) => app.open_with_system(std::path::Path::new(path)),
        Some(effect @ (Effect::StartChat { .. } | Effect::SendPrompt { .. })) => {
            crate::applet_runtime::get(app)
                .effects
                .borrow_mut()
                .push(effect.clone());
        }
        _ => {}
    }
    app.refresh_windows();
    effect
}

/// An emitter that defers handling until no entity is borrowed.
pub(crate) fn emitter(instance: String) -> applet_view::Emit {
    Rc::new(move |event, app| {
        let instance = instance.clone();
        app.defer(move |app| {
            handle(&instance, event, app);
        });
    })
}

/// Render an instance's document, or `None` when it is not mounted.
pub(crate) fn render_document(
    instance: &str,
    compact: bool,
    selection: &Entity<TextSelection>,
    emit: applet_view::Emit,
    window: &mut Window,
    app: &mut App,
) -> Option<AnyElement> {
    let runtime = crate::applet_runtime::get(app);
    let mounted = runtime.host.borrow().instance(instance).cloned()?;
    // The cache is moved out while rendering so the global is not borrowed
    // across element construction, then moved back.
    let granted = runtime.host.borrow().effective(&mounted.instance.applet);
    let assets = std::cell::RefCell::new(std::mem::take(&mut *runtime.assets.borrow_mut()));
    let render_cx = RenderCx {
        granted: &granted,
        scope: instance,
        document: &mounted.instance.document,
        assets: &assets,
        selection,
        emit,
        compact,
    };
    let element = applet_view::render(&render_cx, window, app);
    *crate::applet_runtime::get(app).assets.borrow_mut() = assets.into_inner();
    Some(element)
}

/// A compact card: title row with busy state and a close pill, the error
/// line, then the document. Used by transcript, composer, sidebar and
/// overlay placements. `chrome` false renders the document alone.
pub(crate) fn card(
    instance: &str,
    chrome: bool,
    selection: &Entity<TextSelection>,
    window: &mut Window,
    app: &mut App,
) -> Option<AnyElement> {
    let theme = Theme::global();
    let mounted = crate::applet_runtime::get(app)
        .host
        .borrow()
        .instance(instance)
        .cloned()?;
    let body = render_document(
        instance,
        true,
        selection,
        emitter(instance.to_owned()),
        window,
        app,
    )?;
    let error = mounted.last_error.clone();
    let busy = mounted.busy;
    let title = mounted.instance.document.title.clone();
    let close_id = instance.to_owned();
    let element_id = ElementId::Name(format!("applet-card:{instance}").into());
    Some(
        div()
            .id(element_id)
            .debug_selector(|| "applet-card".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .when(chrome, |el| {
                el.p_3().rounded_xl().bg(theme.TOOL_BG).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.TEXT_DIM)
                                .child(title),
                        )
                        .when(busy, |el| {
                            el.child(
                                div()
                                    .px_2()
                                    .rounded_full()
                                    .bg(theme.ACCENT_DIM)
                                    .text_size(px(11.))
                                    .child("Working…"),
                            )
                        })
                        .child(
                            div()
                                .id(ElementId::Name(
                                    format!("applet-card-close:{close_id}").into(),
                                ))
                                .debug_selector(|| "applet-card-close".into())
                                .cursor_pointer()
                                .rounded_full()
                                .px_2()
                                .text_size(px(11.))
                                .text_color(theme.TEXT_DIM)
                                .hover(|el| el.bg(theme.ACCENT_DIM).text_color(theme.TEXT))
                                .on_click(move |_, _, app| {
                                    app.stop_propagation();
                                    crate::applet_runtime::get(app).close(&close_id);
                                    app.refresh_windows();
                                })
                                .child("×"),
                        ),
                )
            })
            .when_some(error, |el, error| {
                el.child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .bg(theme.ERROR_BG)
                        .text_size(px(12.))
                        .text_color(theme.ERROR)
                        .child(error),
                )
            })
            .child(body)
            .into_any_element(),
    )
}
