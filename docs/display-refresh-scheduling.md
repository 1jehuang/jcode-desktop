# Display cadence audit, 2026-09-21

## Finding

There is no blanket 120 FPS cap in the checked Desktop source or its pinned
production GPUI scheduler. Do not introduce an application-level 120 FPS cap to
fix a low measured frame rate: it would restrict faster displays and would not
remove rendering or event-loop delays. Native animation callbacks already follow
the platform frame source. This is pacing, not a guarantee of one completed frame
per display refresh.

This audit examined upstream Zed revision
`bc538def4545534201bbfcac4e95ac34ea6501b6` and the Desktop Linux platform patch at
`a659883259f2b490714b58e3ab6ff05d26d08a3a`, as pinned in `Cargo.toml`.
Line references below describe those revisions, not upstream HEAD.

## Source evidence

Paths are relative to the Zed checkout unless marked Desktop.

| Area | Source | Behavior |
| --- | --- | --- |
| Public display metadata | `crates/gpui/src/platform.rs:332`, `PlatformDisplay` | ID, UUID and geometry, but no refresh-rate or refresh-interval API. |
| Animation demand | `crates/gpui/src/window.rs:2314-2335` | `on_next_frame` queues a callback and wakes the platform. `request_animation_frame` uses this callback to notify the current entity. No 120 Hz timer. |
| Shared throttling | `crates/gpui/src/window.rs:1550-1564` | Inactive windows without high-rate input are limited to roughly 30 FPS. Serious/critical thermal pressure limits to roughly 60 FPS. Active, non-thermal animation demand has no additional rate cap here. Required presentation and some non-animation requests bypass these throttles. |
| Wayland callback dispatch | `crates/gpui_linux/src/linux/wayland/client.rs:1380-1400` | `wl_callback::Done` invokes the window's `frame()`. |
| Wayland scheduling | `crates/gpui_linux/src/linux/wayland/window.rs:841-857,1739-1749` | `frame()` requests the next `wl_surface.frame`, then invokes the GPUI frame callback. `completed_frame()` commits when rendering did not already present. The compositor supplies the cadence. |
| Wayland output metadata | `crates/gpui_linux/src/linux/wayland/client.rs:1463`, `display.rs:13-43` | Output mode handling records width/height and discards the mode's refresh field. `WaylandDisplay` has no Hz field. This does not prevent compositor-paced callbacks. |
| X11 cadence | `crates/gpui_linux/src/linux/x11/client.rs:1938-2026,2161-2172` | A periodic timer uses RandR mode timing. Missing mode info falls back to 60 Hz. Zero mode clock/totals fall back to 16 ms (62.5 Hz), not 120 Hz. |
| macOS cadence | `crates/gpui_macos/src/window.rs:670-689,2649-2654,2835-2844` | A display-specific frame source invokes GPUI. Moving screens restarts the link for the new screen. Invisible windows do not start the link. |
| macOS display link | `crates/gpui_macos/src/display_link.rs:1-38,412-437` | Uses `CVDisplayLink` and binds it to the display ID. The source documents CoreVideo dynamically looking up timing after mode changes. This was inspected, not tested on macOS hardware. |

The GPUI benchmark context has a `DEFAULT_FPS = 120`, but this belongs to
`crates/gpui/src/app/bench_context.rs`, not the production frame source.
Wayland occurrences of `120` in fractional scaling and wheel delta handling are
protocol units, not FPS limits.

## Desktop-specific scheduling

`Workspace::ensure_animation_tick` in Desktop
`crates/jcode-desktop-ui/src/workspace.rs` uses a coalesced **8 ms fallback wake**
while transitions or pending action captures need progress. This is approximately
125 wakeups/second before execution costs, not a 120 FPS cap. Native frame demand
is also requested elsewhere in the render path. The fallback exists for delayed
compositor callbacks. It does not guarantee an 8 ms presentation interval and
should not be used to infer a monitor's refresh rate.

Two explicit animation-local limits are intentional:

- `workspace_beta_notice.rs`: 30 FPS countdown animation.
- `tool_icon.rs`: 20 FPS icon animation.

Neither imposes a whole-window cap. Idle/demand-driven windows need not continually
produce 120 frames per second. Low presented cadence must be correlated with
active animation demand, focus/thermal throttles, render cost and wake latency
before assigning a cause. A frame counter is not a display refresh API.

## Real gaps and recommended scope

1. **X11 multi-monitor/mode changes:** the current backend selects the first
   usable CRTC mode, not necessarily the monitor containing the window. Its own
   comment says mode timing is not re-queried on screen/configuration changes.
   Correcting this belongs in GPUI Linux, with monitor-overlap selection and RandR
   change handling, rather than a Desktop-only global cap. This does not explain
   cadence in the currently reported Wayland session.
2. **Explicit refresh metadata:** an app-visible numeric display target would
   need a GPUI API extension and backend implementations. Wayland's mode refresh
   can provide nominal metadata but is not a promise of delivered callback
   cadence, particularly with occlusion or variable refresh.
3. **Fallback wake policy:** replacing the 8 ms workaround with an estimated Hz
   risks confusing delayed frames with a low-refresh display. No such heuristic
   was introduced without measured evidence. Keep native pacing as the default.

## Validation and limits

Added
`fractional_and_changing_refresh_cadence_preserves_elapsed_time_motion` to the
existing `panel_scroll_motion.rs` tests. It covers both wheel and precise input,
59.94 Hz, 165 Hz and a 240/60/144/30 Hz changing cadence. It verifies equal motion
for equal elapsed time, conserved travel and exact settling. Existing 30 through
240 Hz tests remain intact.

All **8 scrolling module tests passed** through the real crate with
`cargo test -p jcode-desktop-ui scroll_motion::tests`, including the new changing
cadence test. This verifies motion math, not GPUI's native event loop, actual
monitor movement, variable-refresh hardware or live presented FPS.
`git diff --check` passed for the edited test source. No production scheduling
code or dependency pins were changed.

The scheduling audit itself requires no reload. A future GPUI platform
scheduler change is **host-linked and requires a coordinated host rebuild and
safe restart**. A Ctrl+R UI hot reload cannot replace the running platform loop.
A public GPUI display API change also requires compatible host/UI builds and a
safe restart. No host restart, live-window input, compositor IPC or `niri` was
used for this investigation. No screenshot is claimed as timing evidence.
