# Sidebar wheel regression verification

Verified 2026-09-05 for the sidebar gutter wheel change in `21b935d`.

## Behavioral before/after check

The same GPUI interaction test was run against an isolated copy of the parent
revision and then rerun with only the two production hunks from `21b935d` applied.
The test was unchanged between those runs. It targets coordinates relative to
`sidebar-tab-body`, not the newly introduced gutter selector, so the pre-fix UI
receives the same input at the same sidebar location.

- Before: the actual interaction assertion failed. A downward 37-pixel wheel
  event left the offset at **0px**, instead of the expected **-37px**.
- After applying only the production fix: **1 passed, 0 failed**. The same event
  moves the sidebar exactly **37px**. Reversing by 12 pixels leaves it at -25px,
  and a large upward event returns it to 0px.
- The test exercises session history and files, folder-tab and normal layouts,
  pixel and line wheel events, direction reversal, the top boundary, and
  isolation from the horizontal folder-tab scroll offset.
- The main checkout also passed the selector-independent test before the
  isolated before/after run. The sidebar suite passed **38 tests**, with one
  expected ignored manual profiler.

Command:

```sh
cargo test -p jcode-desktop-ui \
  sidebar_gutter_scrolls_sessions_and_files_without_moving_tabs -- --nocapture
```

An initial isolated build was terminated during dependency compilation. That
exit was **not** counted as the baseline failure. The successful retry compiled
and ran the test, which failed its wheel-offset assertion as reported above.

## Native rendering and live activation

`python3 scripts/screenshot.py target/ui-review-sidebar-wheel.png` successfully
built and rendered the real app on a private Xvfb display. The PNG was inspected.
This checks rendering, not native input behavior.

The running desktop rejected a host-control rebuild/reload request with
`hot reload is disabled; launch with --hot-reload`. The build succeeded, but the
running window has not activated this change. Restart was deferred because it
could discard unsent drafts. These checks demonstrate the gutter fix, not a
resolution of any broader live-window pointer/input failure.
