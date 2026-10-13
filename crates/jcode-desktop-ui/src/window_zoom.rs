use gpui::{App, KeyBinding, Window};
use jcode_desktop_ui_core::zoom;

gpui::actions!(zoom, [ZoomIn, ZoomOut, ResetZoom]);

pub(crate) fn bindings() -> Vec<KeyBinding> {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    vec![
        KeyBinding::new(&format!("{modifier}-="), ZoomIn, None),
        KeyBinding::new(&format!("{modifier}-+"), ZoomIn, None),
        KeyBinding::new(&format!("{modifier}-shift-="), ZoomIn, None),
        KeyBinding::new(&format!("{modifier}--"), ZoomOut, None),
        KeyBinding::new(&format!("{modifier}-0"), ResetZoom, None),
    ]
}

pub(crate) fn install(cx: &mut App) {
    cx.on_action(|_: &ZoomIn, cx| set(zoom::zoomed_in(zoom::current()), cx));
    cx.on_action(|_: &ZoomOut, cx| set(zoom::zoomed_out(zoom::current()), cx));
    cx.on_action(|_: &ResetZoom, cx| set(zoom::DEFAULT, cx));
}

pub(crate) fn apply(window: &mut Window, cx: &mut App) {
    window.set_ui_scale(zoom::current(), cx);
}

fn set(scale: f32, cx: &mut App) {
    if scale == zoom::current() {
        return;
    }
    let scale = zoom::set_current(scale);
    if let Err(error) = crate::config::persist_ui_scale(scale) {
        eprintln!("failed to persist desktop zoom: {error}");
    }
    cx.defer(move |cx| {
        for handle in cx.windows() {
            if handle
                .downcast::<crate::global_voice_overlay::VoiceOverlay>()
                .is_some()
            {
                continue;
            }
            let _ = handle.update(cx, |_, window, cx| window.set_ui_scale(scale, cx));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AppContext as _, Context, IntoElement, Render, TestAppContext, VisualContext as _, div,
    };

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn shortcuts_zoom_every_window_in_steps_and_reset(cx: &mut TestAppContext) {
        zoom::set_current(zoom::DEFAULT);
        cx.update(|cx| {
            cx.bind_keys(bindings());
            install(cx);
            install(cx);
        });
        let (_, first) = cx.add_window_view(|_, _| Empty);
        let first = first.window_handle();
        let (_, second) = cx.add_window_view(|_, _| Empty);
        let second = second.window_handle();
        let scales = |cx: &mut TestAppContext| {
            [first, second].map(|handle| {
                cx.update_window(handle, |_, window, _| window.ui_scale())
                    .unwrap()
            })
        };
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };

        for (keys, expected) in [
            ("=", 1.1),
            ("=", 1.25),
            ("shift-=", 1.5),
            ("-", 1.25),
            ("0", 1.0),
            ("-", 0.9),
        ] {
            cx.simulate_keystrokes(first, &format!("{modifier}-{keys}"));
            cx.run_until_parked();
            assert_eq!(zoom::current(), expected, "{modifier}-{keys}");
            assert_eq!(scales(cx), [expected; 2], "{modifier}-{keys}");
        }
        zoom::set_current(zoom::DEFAULT);
    }
}
