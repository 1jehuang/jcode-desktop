# macOS global shortcut and window lifecycle

## Behavior

The normal macOS Desktop instance registers **Control+Command+I (⌃⌘I)** on
GPUI's main thread using `global-hotkey`'s Carbon `RegisterEventHotKey` path.
It does not install a keyboard monitor, request Accessibility access, change
system shortcuts, register Command+I/J, or register a Linux global shortcut.
Auxiliary `--no-sidebar` / `--workspace` instances do not compete for the chord.
Registration conflicts are logged without preventing application startup.

Only matching key-down events enqueue a show request. The bounded queue
coalesces pending requests and never blocks the native callback. A GPUI task
runs the same `restore_window` function as the Dock and single-instance Show
command. The manager belongs to the stable host, not the reloadable UI.

- Visible window: bring the existing window forward, no new session/window.
- Hidden or minimized window: request native window and application activation.
- Red close button: snapshot the workspace and close the native surface while
  keeping the app and host resources alive. A later show restores the snapshot.
- Full application Quit: native registration is released. Launching a fully
  stopped app requires a separate OS launcher such as macOS Shortcuts.

## Close/reopen failure fix

Previously, `ReloadManager::resume` consumed the suspended snapshot before
knowing whether UI activation succeeded. An error discarded the snapshot and
left an untracked fallback window. Its close callback then failed to snapshot
the expected workspace root and refused to close it.

Resume now preserves the snapshot and previous attachment until activation
succeeds. The host removes a failed replacement surface, so it cannot become
an uncloseable fallback. Retrying show can use the original snapshot. Window
close notifications clear only the matching tracked handle.

A snapshot failure on a **live** workspace still vetoes closing rather than
silently losing unsent drafts. This intentional safety behavior is separate
from the failed-reopen bug. Diagnostics report the failure and closing can be
retried. The fix does not claim that every possible serialization failure is
impossible.

## Regression checks

```sh
cargo test --locked -p jcode-desktop --bin jcode-desktop -- --test-threads=1
cargo build --locked -p jcode-desktop -p jcode-desktop-ui
python3 scripts/accept-window-lifecycle.py target/window-lifecycle
python3 scripts/screenshot.py target/ui-review-window-lifecycle.png --no-build
```

Host tests exercise the exact Control+Command+I key specification, event-ID
and key-up filtering, bounded queue behavior, close callbacks after root
replacement, repeated close/restore, duplicate shows, draft retention,
snapshot failure/retry, and failed activation cleanup/retry. Fake UI roots
inject failures deterministically, while the isolated native acceptance test
uses the real Desktop host and UI with offline fixture data.

The Linux acceptance test uses private Xvfb and Openbox, never the user's
display or compositor. It sets `JCODE_DESKTOP_SCREENSHOT_MACOS_LIFECYCLE=1`
together with offline screenshot mode to select macOS's explicit-quit policy.
Normal Linux launches retain their last-window-closed quit behavior. It checks
the shared close and restore paths with that policy, not Carbon registration
or AppKit traffic lights. A Linux build cannot establish
native macOS shortcut, minimization, Spaces, or traffic-light behavior.

For native macOS acceptance with the updated app:

1. Open Desktop, type an unsent draft, switch to another app, then press ⌃⌘I.
   Expect the same window and draft, with focus in Desktop.
2. Minimize, then press ⌃⌘I. Repeat after hiding the app.
3. Click the red close button. Expect no Desktop window but the app still
   running. Press ⌃⌘I and verify the same workspace and unsent draft return.
4. Repeat step 3 several times. Press the shortcut repeatedly while already
   focused and verify no extra windows or panels appear. Command+J must still
   navigate down within Desktop.
5. In a development build, repeat after a UI reload. Also verify Dock reopening
   and a second app launch use the same restoration behavior.
6. Fully quit. Verify the shortcut is no longer registered. Open Desktop again
   and verify registration returns. Check conflict handling with another app
   explicitly owning the chord, without changing unrelated system bindings.

Host changes require restarting an already-running host. UI-only hot reload
cannot replace the process's native shortcut registration or close callbacks.
