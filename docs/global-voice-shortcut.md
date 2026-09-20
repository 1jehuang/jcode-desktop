# Global Copilot voice shortcut

The Desktop host accepts `jcode-desktop --toggle-voice`. This command only
forwards an explicit request to the already-running Desktop through its private
instance socket. It does not start a new Desktop, recover a workspace, open a
microphone, or fall back to replacing an old host's socket when forwarding fails.
Use `--no-sidebar --toggle-voice` to explicitly target the separate workspace
instance. The default targets the main instance.

The host restores its existing window and requests activation, resolves the current UI's
`workspace::ToggleVoice` action by name, and dispatches it to that window. The
workspace stops/cancels an existing voice owner first, even if a different panel
is now selected. Otherwise it starts in the selected chat, remembered last chat,
or first remaining chat. Utility panels cannot receive dictation. A last-chat
identity survives UI snapshots, but microphone state never does. No voice action
is queued automatically on startup, recovery, or hot reload.

The socket acknowledgement means **queued**, not successful microphone startup.
Check the in-app voice status for credential, permission, or connection errors.
See [voice transcription](voice-transcription.md) for provider setup.

Wayland foreground activation is best-effort. The pinned backend requests a
new activation token using its previous mouse-press serial, which compositor
focus-stealing policy can reject. The command still reaches the selected draft
while the window is in the background. This implementation does not forward the
short-lived CLI's `XDG_ACTIVATION_TOKEN` or promise foreground focus.

## Dell hardware and GPUI normalization

The pinned GPUI source is Zed `bc538def4545534201bbfcac4e95ac34ea6501b6`.
Both Linux backends use `crates/gpui_linux/src/linux/platform.rs`'s
`keystroke_from_xkb`. Unknown named keysyms are lowercased. Effective Super maps
to `Modifiers.platform`. Shift is retained for multi-character named keys, and
this conversion does not subtract XKB consumed modifiers.

| XKB event delivered to GPUI | Normalized GPUI chord |
| --- | --- |
| Super + Shift + F23 | `super-shift-f23` |
| Super + Shift + XF86Assistant | `super-shift-xf86assistant` |
| Dedicated/remapped XF86Assistant without modifiers | `xf86assistant` |

`super`, `cmd`, and `win` are equivalent modifier spellings in GPUI bindings.
Do not use a Windows browser-style `Meta` spelling or `Copilot` as the key name.
The app accepts the three exact variants above, not bare F23 or TouchpadOff.

The installed `/usr/share/X11/xkb/symbols/inet` includes both a `<FK23>` mapping
with `[ XF86TouchpadOff, XF86Assistant ]` and type `PC_SHIFT_SUPER_LEVEL2`, and an
`<I201>` mapping with F23. `keycodes/evdev` aliases I201 to FK23, keycode 201
(Linux KEY_F23 193 plus the XKB offset). Final symbol choice depends on compiled
keymap merge/order, not the Dell key's printed legend. The XKB type selects the
Assistant level for Super+Shift. A compositor can match this using consumed
modifiers differently from GPUI. Offline compilation of this Dell's custom
`keyboard.xkb` (which includes `pc+us+inet(evdev)`) resolved the merge to:

```text
key <FK23> {
    type= "PC_SHIFT_SUPER_LEVEL2",
    symbols[1]= [ F23, XF86Assistant ]
};
```

This was obtained with `xkbcli compile-keymap --from-xkb` on the existing file,
without connecting to a compositor or input device. On that map the hardware's
Super+Shift+F23 chord therefore selects **XF86Assistant**, and GPUI retains the
modifiers as `super-shift-xf86assistant`. Source/static-map inspection is not a
physical key trace. No input devices were monitored.

## Global Wayland configuration proposal

Wayland clients do not receive another application's keyboard events. Configure
the compositor to **consume one non-repeating press** and spawn the tiny
forwarding command. This is explicit shortcut registration, not an unattended
keyboard monitor. Do not add an on-release invocation as well.

For example, a niri configuration can use the following candidate keys inside
its existing `binds` block. Replace the executable path with the installed,
updated Desktop binary, and choose the variant matching the compiled keymap.
These are configuration examples, not a claim of live compositor verification.
No niri command is needed for the headless application tests below.

```kdl
Super+Shift+F23 repeat=false hotkey-overlay-title="Jcode: Start / stop dictation" {
    spawn "/absolute/path/to/jcode-desktop" "--toggle-voice";
}
```

For niri, upstream source inspection of
[`src/input/mod.rs`](https://github.com/YaLTeR/niri/blob/main/src/input/mod.rs)
shows keyboard matching uses `raw_latin_sym_or_raw_current_sym()` and the full
modifier state. On the static map above, that means **Super+Shift+F23** globally,
even though the focused GPUI window sees **super-shift-xf86assistant**. Register
only the F23 global binding for this setup, not speculative Assistant aliases.
This source-based recommendation is not verification of the installed compositor
version or a physical keypress.

Other compositors may instead use the effective XF86Assistant symbol, with or
without consumed modifiers. In that case use that compositor's documented
matching convention and retain `repeat=false`. A single physical press must
match one action. Do not create a second listener or pass the same key through
to the application while also issuing the global command.

On X11 a window-manager binding can invoke the same command. Its binding must
also suppress auto-repeat. Global X11 key grabbing is not implemented by this
change. The desktop still handles the normalized key locally when focused.

## Repeat handling and deployment

* Wayland GPUI marks timer-generated repeats `KeyDownEvent.is_held=true`.
* Pinned X11 GPUI filters synthetic release/press pairs but reports every down
  with `is_held=false`. A down/up latch is therefore also required.
* The workspace consumes Copilot key-down/up events directly. It toggles only
  on the first non-held down and rearms on release. A release resolving to
  XF86TouchpadOff after synthetic modifiers lift is accepted only for rearming.
* Ordinary GPUI action bindings dispatch **before** `capture_key_down`. Do not
  add an unguarded Copilot action binding alongside this raw handler.
* Global IPC has no physical key-up/repeat information. `repeat=false` is the
  correct global guard. A time-only debounce cannot distinguish a held key
  from two intentional quick presses and is not a replacement.
* Ctrl+Shift+V and the voice button use the same workspace owner routing, so
  switching chats cannot create a second concurrent recording.

The new socket command changes the stable host executable, not the plugin ABI.
An old live host must be upgraded with a deliberate, safe restart before global
forwarding works. Hot-reloading only the UI cannot install the new host command.
Do not kill a live host to achieve this: running PTYs are host-owned and are not
transferred by a UI snapshot. Coordinate shutdown/restart after preserving work.
An unsupported old host returns an error to the new forwarding command without
losing its socket, windows, or terminals.

## Headless verification

```sh
cargo test -p jcode-desktop --bin jcode-desktop host::instance
cargo test -p jcode-desktop --bin jcode-desktop voice_command_tests
cargo test -p jcode-desktop-ui workspace::voice
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/accept-voice.py target/voice-acceptance
```

These exercise private IPC, absent/older-host failure, dynamic action routing,
key normalization, Wayland/X11 repeat sequences, release/repress, owner routing,
and last-chat fallback without opening a microphone or querying a compositor.
The acceptance script launches the real production host on a private Xvfb
with an allowlisted environment and microphone-disabled offline fixture. It
invokes the real `--toggle-voice` binary while that private window is minimized,
checks visible status with OCR, and verifies the draft, window, and private
socket survive. It also checks that a
missing-host request cannot launch the app. Before/after PNGs, OCR, logs, and
JSON results remain in the new evidence directory.

These tests do not prove physical firmware emission, compositor activation policy, or
live global key registration. A user-attended press/hold/release/repress check
is still needed after the explicit configuration and safe host upgrade.
