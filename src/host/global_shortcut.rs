//! Process-lifetime macOS shortcut, deliberately outside the reloadable UI.
//! Uses Carbon hotkey registration, not a keyboard monitor or event tap.

use global_hotkey::{
    GlobalHotKeyEvent, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};

fn activation_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyI)
}

fn forward_activation(event: GlobalHotKeyEvent, sender: &async_channel::Sender<()>) {
    if event.id == activation_hotkey().id() && event.state == HotKeyState::Pressed {
        // Never block the native event loop. Coalesce redundant pending shows.
        let _ = sender.try_send(());
    }
}

// Compile the integration in Linux unit tests too, without registering it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn install(
    cx: &mut gpui::App,
    mut show: impl FnMut(&mut gpui::App) -> anyhow::Result<()> + 'static,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use global_hotkey::GlobalHotKeyManager;

    // Creation, registration, and destruction all stay on GPUI's main thread.
    struct Registration {
        _manager: GlobalHotKeyManager,
    }
    impl gpui::Global for Registration {}

    let manager = GlobalHotKeyManager::new().context("initialize native hotkey manager")?;
    let (sender, receiver) = async_channel::bounded(1);
    // global-hotkey 0.7's handler is a OnceCell. Install once, before any event.
    GlobalHotKeyEvent::set_event_handler(Some(move |event| forward_activation(event, &sender)));
    manager
        .register(activation_hotkey())
        .context("register Control+Command+I (another app may already own it)")?;
    cx.set_global(Registration { _manager: manager });
    cx.spawn(async move |cx| {
        while receiver.recv().await.is_ok() {
            // Defer out of the Carbon callback before updating GPUI windows.
            if let Err(error) = cx.update(|cx| show(cx)) {
                eprintln!("global Control+Command+I failed to restore desktop window: {error:#}");
            }
        }
    })
    .detach();
    eprintln!("registered global Control+Command+I desktop shortcut");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_is_control_command_i_without_option_or_shift() {
        let hotkey = activation_hotkey();
        assert_eq!(hotkey.key, Code::KeyI);
        assert_eq!(hotkey.mods, Modifiers::CONTROL | Modifiers::SUPER);
    }

    #[test]
    fn only_matching_presses_activate_and_pending_activations_coalesce() {
        let (sender, receiver) = async_channel::bounded(1);
        let press = GlobalHotKeyEvent {
            id: activation_hotkey().id(),
            state: HotKeyState::Pressed,
        };
        forward_activation(
            GlobalHotKeyEvent {
                state: HotKeyState::Released,
                ..press
            },
            &sender,
        );
        forward_activation(
            GlobalHotKeyEvent {
                id: press.id + 1,
                ..press
            },
            &sender,
        );
        assert!(receiver.try_recv().is_err());
        forward_activation(press, &sender);
        forward_activation(press, &sender);
        assert_eq!(receiver.try_recv(), Ok(()));
        assert!(receiver.try_recv().is_err());
        forward_activation(press, &sender);
        assert_eq!(receiver.try_recv(), Ok(()));
        drop(receiver);
        forward_activation(press, &sender); // Shutdown is harmless.
    }
}
