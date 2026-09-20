//! Process-lifetime macOS shortcuts, deliberately outside the reloadable UI.
//! Uses Carbon hotkey registration, not a keyboard monitor or event tap.

use global_hotkey::{
    GlobalHotKeyEvent, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutEvent {
    Activate,
    ToggleVoice,
}

fn activation_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyI)
}

fn voice_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyM)
}

fn shortcut_event(event: GlobalHotKeyEvent) -> Option<ShortcutEvent> {
    if event.state != HotKeyState::Pressed {
        return None;
    }
    if event.id == activation_hotkey().id() {
        Some(ShortcutEvent::Activate)
    } else if event.id == voice_hotkey().id() {
        Some(ShortcutEvent::ToggleVoice)
    } else {
        None
    }
}

#[derive(Default)]
struct ShortcutState {
    activation_pending: AtomicBool,
    voice_pressed: AtomicBool,
}

fn forward_shortcut(
    event: GlobalHotKeyEvent,
    sender: &async_channel::Sender<ShortcutEvent>,
    state: &ShortcutState,
) {
    // Carbon delivers key-up separately. Latch the physical chord so holding
    // it cannot start then immediately stop recording via auto-repeat.
    if event.id == voice_hotkey().id() && event.state == HotKeyState::Released {
        state.voice_pressed.store(false, Ordering::Relaxed);
        return;
    }
    let Some(event) = shortcut_event(event) else {
        return;
    };
    if event == ShortcutEvent::ToggleVoice && state.voice_pressed.swap(true, Ordering::Relaxed) {
        return;
    }
    // Coalesce redundant shows, but never coalesce toggles: two voice presses
    // must start then stop, even when GPUI has not processed the first yet.
    if event == ShortcutEvent::Activate && state.activation_pending.swap(true, Ordering::Relaxed) {
        return;
    }
    // An unbounded channel never blocks the native event loop and prevents an
    // activation waiting in the queue from swallowing a voice command.
    if sender.try_send(event).is_err() && event == ShortcutEvent::Activate {
        state.activation_pending.store(false, Ordering::Relaxed);
    }
}

// Compile the integration in Linux unit tests too, without registering it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn install(
    cx: &mut gpui::App,
    mut dispatch: impl FnMut(ShortcutEvent, &mut gpui::App) -> anyhow::Result<()> + 'static,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use global_hotkey::GlobalHotKeyManager;

    // Creation, registration, and destruction all stay on GPUI's main thread.
    struct Registration {
        _manager: GlobalHotKeyManager,
    }
    impl gpui::Global for Registration {}

    let manager = GlobalHotKeyManager::new().context("initialize native hotkey manager")?;
    let (sender, receiver) = async_channel::unbounded();
    let state = Arc::new(ShortcutState::default());
    // global-hotkey 0.7's handler is a OnceCell. Install once, before any event.
    GlobalHotKeyEvent::set_event_handler(Some({
        let state = state.clone();
        move |event| forward_shortcut(event, &sender, &state)
    }));
    manager
        .register(activation_hotkey())
        .context("register Control+Command+I (another app may already own it)")?;
    // Carbon consumes registered hotkeys, so the matching in-app key binding
    // does not also toggle voice when this registration succeeds.
    // A voice shortcut conflict must not disable the established app shortcut.
    if let Err(error) = manager.register(voice_hotkey()) {
        eprintln!(
            "could not register global Command+Shift+M voice shortcut (another app may already own it): {error:#}"
        );
    } else {
        eprintln!("registered global Command+Shift+M voice shortcut");
    }
    cx.set_global(Registration { _manager: manager });
    cx.spawn(async move |cx| {
        while let Ok(event) = receiver.recv().await {
            if event == ShortcutEvent::Activate {
                state.activation_pending.store(false, Ordering::Relaxed);
            }
            // Defer out of the Carbon callback before updating GPUI windows.
            if let Err(error) = cx.update(|cx| dispatch(event, cx)) {
                eprintln!("global {event:?} shortcut failed: {error:#}");
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
    fn voice_shortcut_is_command_shift_m_without_control_or_option() {
        let hotkey = voice_hotkey();
        assert_eq!(hotkey.key, Code::KeyM);
        assert_eq!(hotkey.mods, Modifiers::SUPER | Modifiers::SHIFT);
        assert_ne!(hotkey.id(), activation_hotkey().id());
    }

    #[test]
    fn only_matching_presses_activate_and_pending_activations_coalesce() {
        let (sender, receiver) = async_channel::unbounded();
        let pending = ShortcutState::default();
        let press = GlobalHotKeyEvent {
            id: activation_hotkey().id(),
            state: HotKeyState::Pressed,
        };
        for hotkey in [activation_hotkey(), voice_hotkey()] {
            forward_shortcut(
                GlobalHotKeyEvent {
                    id: hotkey.id(),
                    state: HotKeyState::Released,
                },
                &sender,
                &pending,
            );
        }
        forward_shortcut(
            GlobalHotKeyEvent {
                id: HotKey::new(None, Code::KeyA).id(),
                ..press
            },
            &sender,
            &pending,
        );
        assert!(receiver.try_recv().is_err());
        forward_shortcut(press, &sender, &pending);
        forward_shortcut(press, &sender, &pending);
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::Activate));
        pending.activation_pending.store(false, Ordering::Relaxed);
        assert!(receiver.try_recv().is_err());
        forward_shortcut(press, &sender, &pending);
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::Activate));
        pending.activation_pending.store(false, Ordering::Relaxed);
        drop(receiver);
        forward_shortcut(press, &sender, &pending); // Shutdown is harmless.
        assert!(!pending.activation_pending.load(Ordering::Relaxed));
    }

    #[test]
    fn voice_toggles_are_not_dropped_or_coalesced_with_pending_activation() {
        let (sender, receiver) = async_channel::unbounded();
        let pending = ShortcutState::default();
        for hotkey in [
            activation_hotkey(),
            voice_hotkey(),
            voice_hotkey(),
            activation_hotkey(),
        ] {
            forward_shortcut(
                GlobalHotKeyEvent {
                    id: hotkey.id(),
                    state: HotKeyState::Pressed,
                },
                &sender,
                &pending,
            );
            forward_shortcut(
                GlobalHotKeyEvent {
                    id: hotkey.id(),
                    state: HotKeyState::Released,
                },
                &sender,
                &pending,
            );
        }
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::Activate));
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::ToggleVoice));
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::ToggleVoice));
        assert!(receiver.try_recv().is_err());
        drop(receiver);
        forward_shortcut(
            GlobalHotKeyEvent {
                id: voice_hotkey().id(),
                state: HotKeyState::Pressed,
            },
            &sender,
            &pending,
        );
    }

    #[test]
    fn held_voice_chord_toggles_once_until_its_own_release() {
        let (sender, receiver) = async_channel::unbounded();
        let state = ShortcutState::default();
        let press = GlobalHotKeyEvent {
            id: voice_hotkey().id(),
            state: HotKeyState::Pressed,
        };
        forward_shortcut(press, &sender, &state);
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::ToggleVoice));
        for _ in 0..3 {
            forward_shortcut(press, &sender, &state);
        }
        forward_shortcut(
            GlobalHotKeyEvent {
                id: activation_hotkey().id(),
                state: HotKeyState::Released,
            },
            &sender,
            &state,
        );
        forward_shortcut(press, &sender, &state);
        assert!(receiver.try_recv().is_err());
        forward_shortcut(
            GlobalHotKeyEvent {
                state: HotKeyState::Released,
                ..press
            },
            &sender,
            &state,
        );
        assert!(receiver.try_recv().is_err());
        forward_shortcut(press, &sender, &state);
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::ToggleVoice));
        assert!(receiver.try_recv().is_err());
    }
}
