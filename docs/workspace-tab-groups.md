# Workspace tab groups and composer spacing

## Behavior

- Session tabs use a single one-pixel top/side outline. The former additional
  3px selected / 2px inactive accent strip is gone.
- Tabs in the active workspace keep their full labels and normal joined-folder
  layout. Other nonempty workspaces use compact emoji/number tabs, ordered by
  workspace number on either side, with 16px gaps between groups.
- Compact groups reserve at most 35% of the available tab width. Narrow windows
  scale gaps and tab widths without losing numerical order or hit targets.
- A non-scrolling 12px inset separates the transcript viewport from the composer
  or its metadata footer. Startup content-height accounting includes this gap,
  retaining the initial composer position and keeping the last card visible.

## Verification (2026-09-07)

| Requirement | Check and result |
| --- | --- |
| Thin tab outline | Before/after parchment pixels changed from four solid rows to one. Native grouped captures assert a visible single-pixel focused stroke. |
| Workspace order, gaps, compact neighbors | `scripts/accept-live-tabs.py` uses real keys/clicks to move six sessions into three rows and switch between them. Dark and parchment runs passed. Raster measurements were 88/392/88px for compact/active/compact groups, with 16px gaps. |
| Existing tab interaction | Same acceptance script passed six-panel resizing, keyboard navigation and clicks on offscreen sessions without changing session identities. |
| Crowding and animation | All 16 `live_tabs::tests` passed, including grouped narrow geometry, empty active workspace, click navigation, continuous retargeting, reduced motion and resize snapping. |
| Composer clearance | Panel suite: 97 passed, 2 ignored. Fresh and resumed tests verify viewport clearance and visible final row at 640×480, 800×600 and 1440×1000. |
| Real composer behavior | Private-Xvfb `--fresh-interact` passed typing, submission and growth through 16 messages. Initial submission moved the composer 0px. Growth screenshot visibly retains the 12px gap. |
| Delivery | Live instance acknowledged the Ctrl+R-equivalent rebuild/reload and activated UI generation 1 without replacing its native host. |

Reproduce the main native check after building:

```sh
cargo build -p jcode-desktop
python3 scripts/accept-live-tabs.py target/tab-groups-review --theme parchment
python3 -m unittest discover -s scripts -p test_accept_live_tabs.py
python3 scripts/screenshot.py target/composer-review.png --no-build --transcript empty --fresh-interact
```

Local evidence is under `target/tab-groups-dark`, `target/tab-groups-parchment`,
`target/tab-groups-final`, `target/composer-spacing-*` and
`target/tab-groups-live-reload.log`.

The full UI suite is not green: its Gmail background-thread cleanup aborted,
while two gesture tests failed in the combined run but passed individually.
The unrelated pure theme contrast test also fails independently (4.2836 against
its required 4.5). Workspace-wide formatting checks encounter existing changes
outside this work. No unrelated palette, Gmail or formatting changes were made.
