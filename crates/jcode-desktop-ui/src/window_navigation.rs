//! Recover workspace navigation when keyboard focus has no mounted target.
use super::*;

impl Workspace {
    pub(super) fn install_navigation_fallback(cx: &mut Context<Self>) -> gpui::Subscription {
        let workspace = cx.weak_entity();
        cx.intercept_keystrokes(move |event, window, cx| {
            // Interceptors are app-wide. Only the current root of this event's
            // window may handle it, never another window or a retained old UI.
            if window
                .root::<Workspace>()
                .flatten()
                .map(|root| root.entity_id())
                != Some(workspace.entity_id())
            {
                return;
            }
            let _ = workspace.update(cx, |this, cx| {
                if this.focus_handle.contains_focused(window, cx) {
                    return;
                }
                // Use GPUI's actual keymap, including aliases, disabled bindings,
                // and precedence. Do not maintain another list of key strings.
                let (bindings, pending) = cx.key_bindings().borrow().bindings_for_input(
                    std::slice::from_ref(&event.keystroke),
                    &event.context_stack,
                );
                if pending {
                    return;
                }
                let Some(binding) = bindings.first() else {
                    return;
                };
                let action = binding.action().as_any();
                macro_rules! recover {
                    ($action:ty, $handler:ident) => {
                        if let Some(action) = action.downcast_ref::<$action>() {
                            // Restore even on a boundary no-op. The handler stops
                            // propagation, so rebinding after reload cannot repeat
                            // the movement for this same physical keypress.
                            this.focus_active(window, cx);
                            this.$handler(action, window, cx);
                            return;
                        }
                    };
                }
                recover!(FocusLeft, focus_left);
                recover!(FocusRight, focus_right);
                recover!(FocusUp, focus_up);
                recover!(FocusDown, focus_down);
                recover!(FocusFirst, focus_first);
                recover!(FocusLast, focus_last);
                recover!(FocusPrevious, focus_previous);
            });
        })
    }
}
