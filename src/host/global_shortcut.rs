//! Process-lifetime global shortcuts, deliberately outside the reloadable UI.
//! macOS uses Carbon hotkey registration, not a keyboard monitor or event tap.
//! Windows uses RegisterHotKey, with key-up detected by global-hotkey 0.7's
//! GetAsyncKeyState poll. No low-level keyboard hook is installed.

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
    VoicePress,
    VoiceRelease,
}

fn activation_hotkey() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyI)
}

/// macOS: Command+Shift+M. Windows: Ctrl+Shift+Space, because Win+Shift
/// chords are largely reserved by the shell and Ctrl+Shift+M is common in apps.
fn voice_hotkey() -> HotKey {
    voice_hotkey_for(cfg!(target_os = "windows"))
}

fn voice_hotkey_for(windows: bool) -> HotKey {
    if windows {
        HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space)
    } else {
        HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyM)
    }
}

pub fn voice_hotkey_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Ctrl+Shift+Space"
    } else {
        "Command+Shift+M"
    }
}

fn shortcut_event(event: GlobalHotKeyEvent) -> Option<ShortcutEvent> {
    if event.id == voice_hotkey().id() {
        Some(match event.state {
            HotKeyState::Pressed => ShortcutEvent::VoicePress,
            HotKeyState::Released => ShortcutEvent::VoiceRelease,
        })
    } else if event.id == activation_hotkey().id() && event.state == HotKeyState::Pressed {
        Some(ShortcutEvent::Activate)
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
    let Some(event) = shortcut_event(event) else {
        return;
    };
    // Carbon delivers key-up separately. Emit one edge per physical hold,
    // ignoring both auto-repeat presses and duplicate/unmatched releases.
    match event {
        ShortcutEvent::VoicePress if state.voice_pressed.swap(true, Ordering::Relaxed) => return,
        ShortcutEvent::VoiceRelease if !state.voice_pressed.swap(false, Ordering::Relaxed) => {
            return;
        }
        _ => {}
    }
    // Coalesce redundant shows, but preserve ordered voice hold edges.
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
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
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
    // The activation chord is a macOS convention. Windows registers voice only.
    if cfg!(target_os = "macos") {
        manager
            .register(activation_hotkey())
            .context("register Control+Command+I (another app may already own it)")?;
    }
    // Carbon/RegisterHotKey consume registered hotkeys, so the matching in-app
    // key binding does not also start voice when this registration succeeds.
    // A voice shortcut conflict must not disable the established app shortcut.
    let label = voice_hotkey_label();
    if let Err(error) = manager.register(voice_hotkey()) {
        eprintln!(
            "could not register global {label} voice shortcut (another app may already own it): {error:#}"
        );
    } else {
        eprintln!("registered global {label} voice shortcut");
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
    if cfg!(target_os = "macos") {
        eprintln!("registered global Control+Command+I desktop shortcut");
    }
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
        let hotkey = voice_hotkey_for(false);
        assert_eq!(hotkey.key, Code::KeyM);
        assert_eq!(hotkey.mods, Modifiers::SUPER | Modifiers::SHIFT);
        assert_ne!(hotkey.id(), activation_hotkey().id());
    }

    #[test]
    fn windows_voice_shortcut_is_ctrl_shift_space_without_win_or_alt() {
        let hotkey = voice_hotkey_for(true);
        assert_eq!(hotkey.key, Code::Space);
        assert_eq!(hotkey.mods, Modifiers::CONTROL | Modifiers::SHIFT);
        assert_ne!(hotkey.id(), activation_hotkey().id());
        assert_ne!(hotkey.id(), voice_hotkey_for(false).id());
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
    fn voice_edges_are_not_dropped_or_coalesced_with_pending_activation() {
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
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::VoicePress));
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::VoiceRelease));
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::VoicePress));
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::VoiceRelease));
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
    fn held_voice_chord_emits_one_press_and_one_release() {
        let (sender, receiver) = async_channel::unbounded();
        let state = ShortcutState::default();
        let press = GlobalHotKeyEvent {
            id: voice_hotkey().id(),
            state: HotKeyState::Pressed,
        };
        forward_shortcut(press, &sender, &state);
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::VoicePress));
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
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::VoiceRelease));
        for _ in 0..3 {
            forward_shortcut(
                GlobalHotKeyEvent {
                    state: HotKeyState::Released,
                    ..press
                },
                &sender,
                &state,
            );
        }
        assert!(receiver.try_recv().is_err());
        forward_shortcut(press, &sender, &state);
        assert_eq!(receiver.try_recv(), Ok(ShortcutEvent::VoicePress));
        assert!(receiver.try_recv().is_err());
    }
}
