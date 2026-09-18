//! Translate GPUI events to Handterm's window-system-independent keyboard API.
use gpui::{Keystroke, Modifiers, MouseButton};
use handterm_common::{
    input::{Key, KeyEventKind, ModifiersState, NamedKey, key_to_bytes},
    terminal::{KITTY_KBD_REPORT_ALL, Terminal},
};

pub(super) fn key_bytes(
    key: &Keystroke,
    terminal: &Terminal,
    released: bool,
    held: bool,
) -> Option<Vec<u8>> {
    let m = key.modifiers;
    // Workspace shortcuts and clipboard actions must never become PTY input.
    if m.platform
        || m.function
        || (m.control
            && !m.alt
            && (key.key == "r" || (m.shift && matches!(key.key.as_str(), "c" | "v"))))
        || (m.shift && key.key == "insert")
    {
        return None;
    }
    let named = match key.key.as_str() {
        "enter" => Some(NamedKey::Enter),
        "escape" => Some(NamedKey::Escape),
        "backspace" => Some(NamedKey::Backspace),
        "tab" => Some(NamedKey::Tab),
        "up" => Some(NamedKey::ArrowUp),
        "down" => Some(NamedKey::ArrowDown),
        "left" => Some(NamedKey::ArrowLeft),
        "right" => Some(NamedKey::ArrowRight),
        "home" => Some(NamedKey::Home),
        "end" => Some(NamedKey::End),
        "pageup" => Some(NamedKey::PageUp),
        "pagedown" => Some(NamedKey::PageDown),
        "insert" => Some(NamedKey::Insert),
        "delete" => Some(NamedKey::Delete),
        "f1" => Some(NamedKey::F1),
        "f2" => Some(NamedKey::F2),
        "f3" => Some(NamedKey::F3),
        "f4" => Some(NamedKey::F4),
        "f5" => Some(NamedKey::F5),
        "f6" => Some(NamedKey::F6),
        "f7" => Some(NamedKey::F7),
        "f8" => Some(NamedKey::F8),
        "f9" => Some(NamedKey::F9),
        "f10" => Some(NamedKey::F10),
        "f11" => Some(NamedKey::F11),
        "f12" => Some(NamedKey::F12),
        _ => None,
    };
    let logical = if let Some(named) = named {
        Key::Named(named)
    } else {
        // Normal text arrives through GPUI's IME commit path, not both paths.
        if !m.control
            && !m.alt
            && key.key_char.is_some()
            && terminal.kitty_keyboard_flags() & KITTY_KBD_REPORT_ALL == 0
        {
            return None;
        }
        let text = if key.key == "space" {
            " "
        } else {
            key.key.as_str()
        };
        if text.chars().count() != 1 {
            return None;
        }
        Key::Character(text.into())
    };
    let mut modifiers = ModifiersState::default();
    if m.control {
        modifiers = modifiers | ModifiersState::CONTROL;
    }
    if m.alt {
        modifiers = modifiers | ModifiersState::ALT;
    }
    if m.shift {
        modifiers = modifiers | ModifiersState::SHIFT;
    }
    key_to_bytes(
        &logical,
        key.key_char.as_deref(),
        None,
        terminal.application_cursor_keys,
        modifiers,
        terminal.kitty_keyboard_flags(),
        if released {
            KeyEventKind::Release
        } else if held {
            KeyEventKind::Repeat
        } else {
            KeyEventKind::Press
        },
    )
}

pub(super) fn mouse_button(button: MouseButton, m: Modifiers) -> u8 {
    let code = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => 3,
    };
    code | mouse_modifiers(m)
}

pub(super) fn mouse_modifiers(m: Modifiers) -> u8 {
    (if m.shift { 4 } else { 0 }) | (if m.alt { 8 } else { 0 }) | (if m.control { 16 } else { 0 })
}

pub(super) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // ESC in pasted text must not terminate bracketed paste and execute the rest.
    let text = text
        .replace('\x1b', "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace('\n', "\r").into_bytes()
    }
}

/// Temporary compatibility with executable hosts predating engine-owned replies.
/// This handles *complete engine responses*, not shell escape-sequence parsing.
/// The old host answers primary DA, Kitty flags, XTVERSION, OSC11 and two XTGETTCAP
/// probes. Everything else (notably cursor position and graphics ACKs) passes.
pub(super) fn without_legacy_host_replies(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let tail = &bytes[start..];
        let end = if tail.starts_with(b"\x1b[") {
            tail.iter()
                .enumerate()
                .skip(2)
                .find(|(_, b)| (0x40..=0x7e).contains(*b))
                .map(|(i, _)| i + 1)
        } else if tail.starts_with(b"\x1bP")
            || tail.starts_with(b"\x1b]")
            || tail.starts_with(b"\x1b_")
        {
            tail.windows(2)
                .position(|w| w == b"\x1b\\")
                .map(|i| i + 2)
                .or_else(|| tail.iter().position(|b| *b == 7).map(|i| i + 1))
        } else {
            Some(1)
        }
        .unwrap_or(tail.len());
        let response = &tail[..end];
        let legacy = (response.starts_with(b"\x1b[?")
            && matches!(response.last(), Some(b'c' | b'u')))
            || response.starts_with(b"\x1bP>|")
            || response.starts_with(b"\x1b]11;")
            || ((response.starts_with(b"\x1bP0+r") || response.starts_with(b"\x1bP1+r"))
                && [b"696e646e".as_slice(), b"71756572792d6f732d6e616d65"]
                    .iter()
                    .any(|cap| response.windows(cap.len()).any(|w| w == *cap)));
        if !legacy {
            out.extend_from_slice(response);
        }
        start += end;
    }
    out
}
