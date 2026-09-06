# Super-key behavior coverage

This table maps every one of the 42 Linux Super bindings, not just handler
registration. Each row names an observed action outcome and its test boundary.

- **W:** real globally grabbed Wayland keys, `target/super-global-final`.
- **X:** native navigation/reload, `target/settled-focus`.
- **E:** extended native runner, `scripts/shortcut_behavior_acceptance.py`.
  `target/super-all-final` passed 172 native checkpoints across one explicit
  Ctrl+R reload, with real session histories restored in both generations,
  unsaved-parent forks, and clean native Quit.
- **B:** nine public GPUI keystroke behavior tests in
  `super_action_behavior_tests.rs`, all passed from root and composer focus.
- **Enter:** `target/settled-enter`, independent raw daemon creation checks.

| Binding | Action | Check | Observed outcome / remaining boundary |
| --- | --- | --- | --- |
| `super-h` | `FocusLeft` | W/X | Exact previous panel selected, keyboard focus matches, boundary no-op. |
| `super-l` | `FocusRight` | W/X | Exact next panel selected, keyboard focus matches, boundary no-op. |
| `super-j` | `FocusDown` | X/E | Moves to next row, including an empty row with workspace focus. |
| `super-k` | `FocusUp` | X/E | Returns from empty row to the remembered live composer. |
| `super-left` | `FocusLeft` | E | Exact previous panel selected, keyboard focus matches. |
| `super-right` | `FocusRight` | E | Exact next panel selected, keyboard focus matches. |
| `super-down` | `FocusDown` | X/E | Moves to next row, including an empty row with workspace focus. |
| `super-up` | `FocusUp` | X/E | Returns from empty row to the remembered live composer. |
| `super-home` | `FocusFirst` | X/E | Selects first panel without changing session order. |
| `super-end` | `FocusLast` | X/E | Selects last panel without changing session order. |
| `super-u` | `FocusFirst` | X/E | Selects first panel without changing session order. |
| `super-p` | `FocusLast` | X/E | Selects last panel without changing session order. |
| `super-shift-h` | `MovePanelLeft` | E | Selected panel swaps left, retains identity and keyboard focus. |
| `super-shift-l` | `MovePanelRight` | E | Selected panel swaps right, retains identity and keyboard focus. |
| `super-shift-k` | `MovePanelUp` | E | Moved panel returns to original row/order with focus. |
| `super-shift-j` | `MovePanelDown` | E | Selected panel moves to next row, origin retains remaining sessions. |
| `super-shift-home` | `MovePanelToFirst` | E | Selected identity moves to start and remains focused. |
| `super-shift-end` | `MovePanelToLast` | E | Selected identity moves to end and remains focused. |
| `super-n` | `NewPanel` | E | One new focused SDK session, actual daemon working directory is isolated HOME. |
| `super-space` | `ForkPanel` | B/E | One new focused child from an unsaved parent. Saved child has the exact parent ID, inherited directory and fork notice. Runtime fix verified through native key, SDK, daemon and disk. |
| `super-t` | `NewTerminal` | E/B | One terminal panel inserted and focused. Native run uses real host PTY, GPUI test uses inert host. |
| `super-shift-g` | `OpenGmail` | E/B | Opens gmail://inbox and focuses it. Repeating focuses the same panel, without duplicates. No credentials or mailbox mutations. |
| `super-shift-d` | `OpenTodoist` | E/B | Opens todoist://tasks and focuses it. Repeating reuses it. No token or task mutations. |
| `super-enter` | `NewPanelInPinnedDirectory` | W/Enter | One new focused session in /home/jeremy/jcode-desktop, before/after restart and from empty workspace. |
| `super-;` | `NewPanelInPinnedDirectory` | W/Enter | One new focused session in /home/jeremy/jcode-desktop, before/after restart. |
| `super-'` | `NewPanel` | W/E | One new focused SDK session, actual daemon working directory is isolated HOME. |
| `super-q` | `ClosePanel` | W/E | Exactly selected panel removed, app stays alive, keyboard and remembered row focus target survivor. Newly exposed stale-memory bug fixed. |
| `super-tab` | `FocusPrevious` | X | Returns to previous panel after jumping to the last panel. |
| `super-shift-tab` | `ToggleOverview` | X/E | Overview opens/closes with workspace/composer focus restored. |
| `super-o` | `ToggleOverview` | X/E | Overview opens/closes with workspace/composer focus restored. |
| `super-/` | `ToggleHints` | B | Hints flag changes and coach-card gains/loses rendered bounds from root and composer. |
| `super-shift-s` | `ToggleShowcase` | B | Showcase toggles, cue disappears/reappears with expected shortcut and action labels. |
| `super-shift-t` | `CycleTheme` | B | Every theme preset advances and wraps, canvas remains rendered, original preset restored after test. |
| `super-b` | `ToggleSidebar` | B | Sidebar disappears/reappears and canvas width changes without panel identity mutation. |
| `super-shift-/` | `NewHelpSession` | B | Creates one focused help draft, records correlated create and follow-up prompt commands. Provider response not invoked. |
| `super-r` | `CycleWidth` | E | Width cycles from full to0.25, wrap observed in public panel width. |
| `super-f` | `MaximizeWidth` | E | Width0.25 expands to1.0 and returns to0.25. |
| `super-1` | `WidthPreset1` | E | Public selected panel width becomes0.25. |
| `super-2` | `WidthPreset2` | E | Public selected panel width becomes0.5. |
| `super-3` | `WidthPreset3` | E | Public selected panel width becomes0.75. |
| `super-4` | `WidthPreset4` | E | Public selected panel width becomes1.0. |
| `super-shift-q` | `Quit` | E | Native Super+Shift+Q exits the isolated app with status0, after all other checks. |

## Integration boundaries

The native service-panel tests use isolated HOME/JCODE_HOME, direct Gmail
backend, and no inherited API tokens. Opening the unconfigured panels is tested,
not successful mailbox/task API access. Native terminal creation uses the real
host. The GPUI terminal test intentionally has an inert host. Help-session tests
exercise rendered draft creation and the real command contract through a recording
bridge, without sending a provider request. Native fork acceptance is required
in addition to the recording-bridge command test.

The existing 42-binding/84-focus-path registration audit remains a separate
check and is not substituted for any outcome above. User compositor input is
not driven. Global-grab acceptance uses private Sway with its focus query adapted
to the installed helper's CLI shape.

## Verified runtime and live activation

The corrected daemon and bridge used by native acceptance are now active in
the user's session. Non-forced daemon reload preserved all seven connected
client identities. Bridge replacement preserved all 15 then-open Desktop
sessions and drafts. The subsequent live Ctrl+R-equivalent activated UI
generation 2 and preserved all 15 panel identities, order, drafts, and layout,
with every session reattached.

The earlier activation blocker was based on a stale recovery checkpoint, not
live unsaved sessions. Fresh process and daemon checks corrected that finding
before activation. See the evidence and exact executable hashes in
[shortcut-requirements.md](shortcut-requirements.md#verified-live-activation).

Busy sessions with a saved snapshot can fork without waiting for their Agent
lock. A busy unsaved session returns an error rather than blocking or guessing
state. Reconnect retention is bounded to 30 seconds for idle unsaved sessions,
not durable storage across a shared-daemon restart.
