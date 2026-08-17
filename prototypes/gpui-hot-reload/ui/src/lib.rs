use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
};

use gpui::*;
use hot_reload_api::{ACTIVATE_FAILED, ACTIVATE_OK, PluginApi};

struct ReloadableUi {
    clicks: usize,
}

impl Render for ReloadableUi {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap_4()
            .bg(rgb(0x0f172a))
            .text_color(rgb(0xe2e8f0))
            .child(div().text_2xl().child("Reloadable cdylib UI"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x94a3b8))
                    .child("Edit this crate, rebuild it, and press F5."),
            )
            .child(
                div()
                    .id("plugin-counter")
                    .px_4()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0x2563eb))
                    .hover(|style| style.bg(rgb(0x3b82f6)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clicks += 1;
                        cx.notify();
                    }))
                    .child(format!("Plugin callback clicks: {}", self.clicks)),
            )
    }
}

unsafe extern "C-unwind" fn activate(window: *mut c_void, app: *mut c_void) -> i32 {
    if window.is_null() || app.is_null() {
        return ACTIVATE_FAILED;
    }

    // `Window::replace_root` builds the new entity before assigning `root`.
    // Consequently, a panic while building leaves the prior root installed.
    // Catching here also prevents an unwind from escaping the dynamic boundary.
    let activated = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the host guarantees these pointers are live and exclusive for
        // this synchronous activation callback.
        let window = unsafe { &mut *window.cast::<Window>() };
        let app = unsafe { &mut *app.cast::<App>() };
        window.replace_root(app, |_, _| ReloadableUi { clicks: 0 });
    }));

    if activated.is_ok() {
        ACTIVATE_OK
    } else {
        ACTIVATE_FAILED
    }
}

/// Returns the only ABI surface exported by this reloadable library.
///
/// # Safety
///
/// The caller must validate the returned table's ABI version and structure
/// size before invoking any function pointer in it.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn gpui_hot_reload_plugin() -> PluginApi {
    PluginApi::new(activate)
}
