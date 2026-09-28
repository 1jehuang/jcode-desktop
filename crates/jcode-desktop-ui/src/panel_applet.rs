//! A workspace panel that hosts one applet instance (`applet://<instance>`).
//!
//! The panel owns chrome only: title, busy and error state, and a close pill.
//! Content comes from the shared applet runtime, so the same instance can be
//! moved between placements without losing state.
use super::*;
use crate::applet_host::Effect;
use crate::applet_view::{self, ViewEvent};
use gpui::AnyElement;
use jcode_applet_types::Placement;
use std::rc::Rc;

pub(crate) const APPLET_PREFIX: &str = "applet://";

pub(super) struct AppletPanel {
    pub instance: String,
    pub selection: Entity<TextSelection>,
    pub scroll: ScrollHandle,
    pub seen_generation: u64,
    _pump: Option<gpui::Task<()>>,
}

impl Panel {
    pub fn new_applet(instance: String, bridge: Bridge, cx: &mut Context<Self>) -> Self {
        let title = crate::applet_runtime::get(cx)
            .host
            .borrow()
            .instance(&instance)
            .map(|mounted| mounted.instance.document.title.clone())
            .unwrap_or_else(|| "Applet".into());
        let mut panel = Self::new(
            format!("{APPLET_PREFIX}{instance}"),
            Some(title),
            None,
            bridge,
            cx,
        );
        panel.items.clear();
        panel.todoist = None;
        panel.orchestration = None;
        let selection = cx.new(TextSelection::new);
        cx.observe(&selection, |_, _, cx| cx.notify()).detach();
        // Providers speak asynchronously. Pump their output on a light timer
        // and re-render only when the shared runtime actually changed.
        let pump = (!cfg!(test)).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(33))
                        .await;
                    if this.update(cx, |panel, cx| panel.pump_applet(cx)).is_err() {
                        break;
                    }
                }
            })
        });
        panel.applet = Some(AppletPanel {
            instance,
            selection,
            scroll: ScrollHandle::new(),
            seen_generation: 0,
            _pump: pump,
        });
        panel
    }

    fn pump_applet(&mut self, cx: &mut Context<Self>) {
        let runtime = crate::applet_runtime::get(cx);
        runtime.pump();
        let generation = runtime.generation.get();
        let Some(applet) = self.applet.as_mut() else {
            return;
        };
        if applet.seen_generation != generation {
            applet.seen_generation = generation;
            cx.notify();
        }
    }

    fn handle_applet_event(
        &mut self,
        event: ViewEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(instance) = self.applet.as_ref().map(|applet| applet.instance.clone()) else {
            return;
        };
        if let Some(Effect::Closed) = crate::applet_surface::handle(&instance, event, cx) {
            cx.emit(AppletPanelClosed);
        }
        cx.notify();
    }

    pub(super) fn render_applet(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::global();
        let applet = self.applet.as_ref().expect("applet panel");
        let instance_id = applet.instance.clone();
        let selection = applet.selection.clone();
        let scroll = applet.scroll.clone();
        let weak = cx.weak_entity();
        let emit: applet_view::Emit = Rc::new(move |event, app| {
            let weak = weak.clone();
            // Deferred so no entity is borrowed while the runtime reacts.
            app.defer(move |app| {
                if let Some(panel) = weak.upgrade()
                    && let Some(window) = app.active_window()
                {
                    let _ = window.update(app, |_, window, app| {
                        panel.update(app, |panel, cx| {
                            panel.handle_applet_event(event, window, cx)
                        });
                    });
                }
            });
        });
        let runtime = crate::applet_runtime::get(cx);
        let mounted = runtime.host.borrow().instance(&instance_id).cloned();
        let title = mounted
            .as_ref()
            .map(|mounted| mounted.instance.document.title.clone())
            .unwrap_or_else(|| "Applet".into());
        if self.title.as_ref() != title {
            self.title = title.clone().into();
        }
        let compact = mounted
            .as_ref()
            .is_some_and(|mounted| !matches!(mounted.instance.placement, Placement::Panel { .. }));
        let body: AnyElement = crate::applet_surface::render_document(
            &instance_id,
            compact,
            &selection,
            emit,
            window,
            cx,
        )
        .unwrap_or_else(|| {
            div()
                .p_6()
                .text_color(theme.TEXT_DIM)
                .child("This applet is no longer running.")
                .into_any_element()
        });
        let error = mounted
            .as_ref()
            .and_then(|mounted| mounted.last_error.clone());
        let busy = mounted.as_ref().is_some_and(|mounted| mounted.busy);
        let close_id = instance_id.clone();
        div()
            .debug_selector(|| "applet-panel".into())
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .flex_none()
                    .h(px(52.))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
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
                            .id("applet-close")
                            .debug_selector(|| "applet-close".into())
                            .cursor_pointer()
                            .rounded_full()
                            .px_3()
                            .py(px(3.))
                            .text_size(px(11.))
                            .bg(theme.INLINE_CODE_BG)
                            .text_color(theme.TEXT_DIM)
                            .hover(|el| el.bg(theme.ACCENT_DIM))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                crate::applet_runtime::get(cx).close(&close_id);
                                cx.emit(AppletPanelClosed);
                            }))
                            .child("Close"),
                    ),
            )
            .when_some(error, |el, error| {
                el.child(
                    div()
                        .mx_4()
                        .mb_2()
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .bg(theme.ERROR_BG)
                        .text_size(px(12.))
                        .text_color(theme.ERROR)
                        .child(error),
                )
            })
            .child(
                div()
                    .id("applet-body")
                    .debug_selector(|| "applet-body".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .px_4()
                    .pb_4()
                    .child(body),
            )
            .into_any_element()
    }
}

pub(crate) struct AppletPanelClosed;
impl gpui::EventEmitter<AppletPanelClosed> for Panel {}
