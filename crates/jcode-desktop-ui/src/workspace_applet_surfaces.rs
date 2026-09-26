//! Workspace placements for applets beyond panels and transcripts: the
//! sidebar section with manifest launcher pills, corner overlays, the
//! capability consent prompt, applet toasts, and the applet settings list.
use super::applets::SHOWCASE_ID;
use super::*;
use gpui::{ElementId, FontWeight};
use jcode_applet_types::{
    Capability, Placement,
    manifest::Trigger,
    placement::{Corner, Scope},
};

/// How long an applet toast stays up.
const TOAST_DURATION: Duration = Duration::from_secs(4);
const OVERLAY_WIDTH: f32 = 320.0;

impl Workspace {
    fn applet_scope(&self, cx: &App) -> (Option<String>, Option<String>) {
        let active = self.slots.get(self.active).map(|slot| slot.panel.read(cx));
        (
            active.and_then(|panel| panel.working_dir.clone()),
            active.map(|panel| panel.session_id.clone()),
        )
    }

    fn visible_applets(&self, filter: impl Fn(&Placement) -> bool, cx: &App) -> Vec<String> {
        let (dir, session) = self.applet_scope(cx);
        let Some(runtime) = cx.try_global::<crate::applet_runtime::Runtime>() else {
            return Vec::new();
        };
        runtime
            .host
            .borrow()
            .at(filter)
            .filter(|mounted| {
                crate::applet_host::AppletHost::in_scope(
                    mounted,
                    dir.as_deref(),
                    session.as_deref(),
                )
            })
            .map(|mounted| mounted.instance.id.clone())
            .collect()
    }

    /// Launcher pills for manifests with a sidebar trigger, then compact
    /// cards for sidebar-placed instances. `None` when there is nothing.
    pub(super) fn render_sidebar_applets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = Theme::global();
        let launchers: Vec<(String, usize, String)> = cx
            .try_global::<crate::applet_runtime::Runtime>()
            .map(|runtime| {
                let host = runtime.host.borrow();
                host.launchers()
                    .into_iter()
                    .filter(|(_, _, launcher)| matches!(launcher.trigger, Trigger::Sidebar))
                    .map(|(applet, index, _)| {
                        let title = host
                            .manifest(&applet)
                            .map(|manifest| manifest.title.clone())
                            .unwrap_or_else(|| applet.clone());
                        (applet, index, title)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let instances = self.visible_applets(|p| matches!(p, Placement::Sidebar), cx);
        if launchers.is_empty() && instances.is_empty() {
            return None;
        }
        let selection = self.applet_selection(cx);
        let mut section = div()
            .debug_selector(|| "sidebar-applets".into())
            .flex_none()
            .w_full()
            .min_w_0()
            .pl_2()
            .pr(px(crate::scrollbar::GUTTER + 4.0))
            .pt_2()
            .flex()
            .flex_col()
            .gap_2();
        if !launchers.is_empty() {
            let mut row = div().flex().flex_wrap().gap_1();
            for (applet, index, title) in launchers {
                let id = format!("applet-launcher:{applet}:{index}");
                row = row.child(
                    div()
                        .id(ElementId::Name(id.into()))
                        .debug_selector(|| "applet-launcher".into())
                        .h(px(24.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(theme.TOOL_BG)
                        .text_size(px(11.0))
                        .text_color(theme.TEXT_DIM)
                        .hover(|el| el.bg(theme.PANEL_BG).text_color(theme.TEXT))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.fire_applet_launcher(&applet, index, window, cx);
                        }))
                        .child(title),
                );
            }
            section = section.child(row);
        }
        for instance in instances {
            if let Some(card) = crate::applet_surface::card(&instance, true, &selection, window, cx)
            {
                section = section.child(card);
            }
        }
        Some(section.into_any_element())
    }

    /// One text selection scope for workspace-level applet surfaces.
    fn applet_selection(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<crate::text_selection::TextSelection> {
        if let Some(selection) = &self.applet_text_selection {
            return selection.clone();
        }
        let selection = cx.new(crate::text_selection::TextSelection::new);
        self.applet_text_selection = Some(selection.clone());
        selection
    }

    /// Floating corner cards for overlay-placed instances.
    pub(super) fn render_applet_overlays(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let instances = self.visible_applets(|p| matches!(p, Placement::Overlay { .. }), cx);
        if instances.is_empty() {
            return None;
        }
        let selection = self.applet_selection(cx);
        let mut corners: Vec<(Corner, Vec<gpui::AnyElement>)> = Vec::new();
        for instance in instances {
            let corner = crate::applet_runtime::get(cx)
                .host
                .borrow()
                .instance(&instance)
                .and_then(|mounted| match mounted.instance.placement {
                    Placement::Overlay { corner } => Some(corner),
                    _ => None,
                })
                .unwrap_or_default();
            let Some(card) = crate::applet_surface::card(&instance, true, &selection, window, cx)
            else {
                continue;
            };
            let card = div()
                .w(px(OVERLAY_WIDTH))
                .max_h(px(420.0))
                .rounded_xl()
                .bg(Theme::global().PANEL_BG)
                .shadow_lg()
                .occlude()
                .child(card)
                .into_any_element();
            match corners.iter_mut().find(|(c, _)| *c == corner) {
                Some((_, cards)) => cards.push(card),
                None => corners.push((corner, vec![card])),
            }
        }
        let mut layer = div()
            .debug_selector(|| "applet-overlays".into())
            .absolute()
            .inset_0();
        for (corner, cards) in corners {
            let mut stack = div().absolute().flex().flex_col().gap_2();
            stack = match corner {
                Corner::TopRight => stack.top(px(48.0)).right(px(16.0)),
                Corner::BottomRight => stack.bottom(px(16.0)).right(px(16.0)),
                Corner::BottomLeft => stack.bottom(px(16.0)).left(px(16.0)),
                Corner::TopLeft => stack.top(px(48.0)).left(px(16.0)),
            };
            layer = layer.child(stack.children(cards));
        }
        Some(layer.into_any_element())
    }

    /// Ask once per applet before it may use declared capabilities. Built-in
    /// applets are trusted and never prompt.
    pub(super) fn render_applet_consent(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let pending = cx
            .try_global::<crate::applet_runtime::Runtime>()?
            .host
            .borrow()
            .pending_consent();
        let (manifest, capabilities) = pending
            .into_iter()
            .find(|(manifest, _)| !self.applet_consent_dismissed.contains(&manifest.id))?;
        let theme = Theme::global();
        let list = capabilities
            .iter()
            .map(|capability| crate::applet_host::capability_label(*capability))
            .collect::<Vec<_>>()
            .join(", ");
        let allow_id = manifest.id.clone();
        let allow_caps = capabilities.clone();
        let deny_id = manifest.id.clone();
        let later_id = manifest.id.clone();
        let pill = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px_3()
                .h(px(26.0))
                .flex()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.0))
                .when(primary, |el| el.bg(theme.ACCENT).text_color(theme.BG))
                .when(!primary, |el| {
                    el.bg(theme.TOOL_BG)
                        .text_color(theme.TEXT_DIM)
                        .hover(|el| el.text_color(theme.TEXT))
                })
                .child(label)
        };
        Some(
            div()
                .debug_selector(|| "applet-consent".into())
                .absolute()
                .bottom(px(16.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .max_w(px(520.0))
                        .p_4()
                        .rounded_xl()
                        .bg(theme.PANEL_BG)
                        .shadow_lg()
                        .occlude()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("Allow {} to {list}?", manifest.title)),
                        )
                        .child(div().text_size(px(12.0)).text_color(theme.TEXT_DIM).child(
                            "The applet can only do these things when you press its \
                                 controls. You can revoke this in Settings.",
                        ))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .justify_end()
                                .child(pill("applet-consent-later", "Not now", false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.applet_consent_dismissed.insert(later_id.clone());
                                        cx.notify();
                                    }),
                                ))
                                .child(pill("applet-consent-deny", "Deny", false).on_click(
                                    cx.listener(move |_, _, _, cx| {
                                        crate::applet_runtime::get(cx)
                                            .decide(&deny_id, Default::default());
                                        cx.notify();
                                    }),
                                ))
                                .child(pill("applet-consent-allow", "Allow", true).on_click(
                                    cx.listener(move |_, _, _, cx| {
                                        crate::applet_runtime::get(cx).decide(
                                            &allow_id,
                                            allow_caps.iter().copied().collect(),
                                        );
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }

    /// A transient applet toast pill near the top of the window.
    pub(super) fn render_applet_toast(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let (text, at) = self.applet_toast.clone()?;
        if at.elapsed() > TOAST_DURATION {
            self.applet_toast = None;
            return None;
        }
        let remaining = TOAST_DURATION - at.elapsed();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(remaining).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        let theme = Theme::global();
        Some(
            div()
                .debug_selector(|| "applet-toast".into())
                .absolute()
                .top(px(12.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .px_4()
                        .py_1()
                        .rounded_full()
                        .bg(theme.PANEL_BG)
                        .shadow_md()
                        .text_size(px(12.0))
                        .child(text),
                )
                .into_any_element(),
        )
    }

    /// Settings rows: each third-party applet with its granted capabilities
    /// and a Revoke pill.
    pub(super) fn render_applet_settings(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let runtime = cx.try_global::<crate::applet_runtime::Runtime>()?;
        let host = runtime.host.borrow();
        let rows: Vec<(String, String, Vec<Capability>, bool)> = host
            .manifests()
            .filter(|manifest| !manifest.capabilities.is_empty())
            .filter(|manifest| {
                manifest.id != SHOWCASE_ID && manifest.id != jcode_applet_types::agent::APPLET_ID
            })
            .map(|manifest| {
                (
                    manifest.id.clone(),
                    manifest.title.clone(),
                    host.effective(&manifest.id),
                    host.grants().contains_key(&manifest.id),
                )
            })
            .collect();
        drop(host);
        if rows.is_empty() {
            return None;
        }
        let theme = Theme::global();
        let mut list = div()
            .debug_selector(|| "settings-applets".into())
            .flex()
            .flex_col()
            .gap_1()
            .child(div().pt_2().text_color(theme.TEXT_DIM).child("Applets"));
        for (id, title, granted, decided) in rows {
            let summary = if !decided {
                "Not asked yet".to_owned()
            } else if granted.is_empty() {
                "Denied".to_owned()
            } else {
                granted
                    .iter()
                    .map(|capability| crate::applet_host::capability_label(*capability))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let revoke_id = id.clone();
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(theme.PANEL_BG)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(div().text_color(theme.TEXT_DIM).truncate().child(summary))
                    .when(decided, |el| {
                        el.child(
                            div()
                                .id(ElementId::Name(format!("applet-revoke:{id}").into()))
                                .px_2()
                                .rounded_full()
                                .cursor_pointer()
                                .bg(theme.TOOL_BG)
                                .hover(|el| el.bg(theme.ACCENT_DIM))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    crate::applet_runtime::get(cx).revoke(&revoke_id);
                                    this.applet_consent_dismissed.remove(&revoke_id);
                                    cx.notify();
                                }))
                                .child("Revoke"),
                        )
                    }),
            );
        }
        Some(list.into_any_element())
    }
}

/// Instances scoped to a session that no longer exists are harmless, but
/// scope checks need the panel's session id even for remote panels.
#[allow(dead_code)]
fn session_scope(session_id: &str) -> Scope {
    Scope::Session {
        session_id: session_id.to_owned(),
    }
}
