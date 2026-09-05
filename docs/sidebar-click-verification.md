# Session history click focus verification

Verified 2026-09-05.

## Cause and fix

The sidebar session row explicitly focuses the selected session's composer on
mouse-down. GPUI then bubbles that press to the focusable workspace ancestor,
whose default focus handler takes focus away again. The session opens, but typing
and composer shortcuts do not reach it.

The row now prevents default mouse focus before activating the session. This
keeps both newly opened history sessions and existing active sessions focused
without changing session ordering, panel layout, or wheel handling.

## Behavioral evidence

- The new GPUI test first passed its session-identity checks, then failed when
  extended to check keyboard focus: `keyboard_panel` was null after the click.
- With the production fix, the same test passes. It clicks several scrolled
  entries in an 80-session history in both folder-tab and normal layouts, checks
  the actual session identity and keyboard focus, and types into each composer.
- The sidebar suite passed 41 tests with one ignored manual benchmark.
- A native X11 check on a private Xvfb display reproduced the pre-fix null
  keyboard focus after opening `screenshot-history-06`.
- The fixed native check opens that history session, switches to another active
  session and back, verifies session and keyboard focus, and types a draft. The
  screenshot visibly contains `history click typing works` in the selected
  session's composer.

Commands:

```sh
cargo test -p jcode-desktop-ui sidebar -- --nocapture
cargo test -p jcode-desktop-ui clicking_scrolled_history_rows -- --nocapture
python3 scripts/screenshot.py target/ui-review-history-fixed.png --history-interact
python3 scripts/screenshot.py target/ui-review-history-fixed-normal.png --history-interact --layout-mode normal --no-build
```

The native fixture is offline and uses isolated settings, runtime sockets, and
Xvfb. It never clicks or types into the user's desktop.

## Requirement-to-observation mapping

| Requirement or changed output | Concrete check | Observed result |
| --- | --- | --- |
| Clicking a displayed historical session activates that exact session | `clicking_scrolled_history_rows_reliably_activates_the_displayed_session` compares the rendered row's session identity with the active panel after each click | Passed for row indices 30, 50, 20, and 60 in an 80-session catalog in both layouts |
| The click leaves the session ready for typing | The same GPUI test checks `keyboard_panel` and injects `history draft` into the composer | Before the fix, keyboard focus was null. After the fix, focus matches the selected panel and its composer contains the draft |
| Real platform input works, not just direct handler calls | `--history-interact` sends X11 clicks into the actual app on private Xvfb and checks public navigation state | Before the fix, opening history produced `focused_slot=1` but `keyboard_panel=null`. After the fix, both equal 1 |
| Clicking existing active sessions remains usable | Native test clicks history 06, the original active session, then history 06 in the Active section | Every click matched both the expected session ID and keyboard focus. Only two panels remained open |
| Both supported sidebar layouts work | Native acceptance runs with folder tabs and with `--layout-mode normal` | Both exited 0, retained keyboard focus, and produced screenshots |
| Typing is visibly delivered after a native click | Native test types `history click typing works` without clicking the composer first | `target/ui-review-history-fixed.png` was inspected and shows the exact text in History session 06's composer |
| New offline history fixture contains historical, unopened sessions | `JCODE_DESKTOP_SCREENSHOT_HISTORY=1` fixture rendering | Baseline screenshot shows one active panel and a Session history count of 80. After opening one history entry, screenshot shows two active panels and history count 79 |
| New CLI rejects incompatible interaction settings | Run `--history-interact` with multiple panels, nondefault dimensions, Learn stage, explicit focus-panel, and HTML interaction | Each combination exits 2 with the documented history-interact constraint error |
| Native interaction reports a missing input driver safely | Run the script with `shutil.which` mocked to return no executable | Exits 2 with `history-interact requires xdotool` before starting a build, display, or app |
| Existing sidebar behavior does not regress | Sidebar tests exercise wheel scrolling, saved/active grouping, navigation, titles, and hidden sidebar layout | 41 passed, one ignored manual benchmark |
| Only this fix is committed and pushed | Inspect commit `8a63caa` and push result | Three files only: four-line production focus fix, test/fixture additions, screenshot harness, and verification documentation. Remote `main` advanced to the commit |
| The user's running app receives the fix | Send the instance socket's Ctrl+R-equivalent command and inspect host diagnostics | Blocked. Build succeeded, but activation was rejected because the live host was started without hot reload. This requirement is not reported as passed |

The before/after focus observations establish an actual improvement in the
tested build. They do not establish that every possible meaning of unreliable
history clicks is resolved, or that the unchanged running host is improved.

### Broader-suite limits

The complete UI suite was also attempted. It is **not reported as green**:

- Parallel run: 330 passed, four failed, six ignored.
- Serial retry: 331 passed, three failed, six ignored.
- `vertical_keys_cover_both_animation_directions` passed on the serial retry.
- Three failures reproduced when each test ran alone:
  `email_inbox_moves_when_the_user_scrolls`,
  `restored_scroll_is_not_replaced_when_history_reattaches`, and
  `a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`.

The history-click regression passed in both full-suite runs. The three remaining
failures concern other scrolling/gesture paths in the concurrently changing
worktree. Their origin was not established by this task, and no claim is made
that this change fixes them. Full-suite counts do not substitute for the
requirement-level observations above.

## Live delivery boundary

### Post-mapping whole-result rerun

All mapped checks were repeated after the mapping was written, on 2026-09-05
at approximately 22:50–22:54 UTC. Freshly built UI-test and desktop executables
were copied before execution so concurrent builds could not replace them during
the checks. Local logs, screenshots, and `final-results.json` are preserved in
`/home/jeremy/.jcode/scratch/history-mapped-checks-1788648616`.

| Check repeated against the complete result | Actual observation |
| --- | --- |
| Scrolled row identity, keyboard focus, and composer input in both layouts | Regression test passed for all four selected row indices in both layouts |
| Native folder-tab history click, active-session switch, and switch back | Exit 0. History 06 is selected, `focused_slot=1`, `keyboard_panel=1`, and only two panels are open |
| Native normal-layout history click and active-session switches | Exit 0 with the same session identity, keyboard focus, and panel count |
| Native text delivery and opt-in history population | Inspected both new PNGs. Each visibly shows `history click typing works` in History 06's composer, two active sessions, and 79 remaining historical sessions |
| Default fixture without history interaction | Exit 0. Inspected `default.png`: one active session, no historical rows, and an empty focused composer. Navigation confirms one panel with keyboard focus 0 |
| Existing sidebar behavior | 41 passed, one ignored manual benchmark, including wheel/track scrolling, grouping, navigation, and preserved history scroll |
| All five incompatible CLI combinations | Each exits 2 with its constraint error |
| Missing native input driver | Mocked missing `xdotool` produces the actionable error before launching anything |
| Commit scope and remote availability | Reinspected the three-file fix commit. Both `8a63caa` and mapping commit `d4b171d` are ancestors of `origin/main`; unrelated working changes remain uncommitted by this task |
| Full UI suite | 332 passed, three failed, six ignored. The same three scrolling/gesture tests listed above fail. History-click regression passes |
| Live activation | Retried the instance socket reload. It acknowledged the request, rebuilt, then logged `remote UI reload failed after rebuild: hot reload is disabled; launch with --hot-reload` |

The repeated native checks and inspected screenshots confirm the built fix
improves click-to-type behavior. Live delivery remains blocked, and the full UI
suite still has the explicitly recorded failures. Neither limitation is treated
as a passing check.

The application was rebuilt. Its instance socket accepted the Ctrl+R-equivalent
rebuild/reload command, but the running host rejected activation with:
`hot reload is disabled; launch with --hot-reload`.

The running process was not forcibly restarted because unsent drafts are held
in its live UI state. A restart is still needed to activate the fix in that
instance. The tests above verify the fixed build, not the stale running host.
