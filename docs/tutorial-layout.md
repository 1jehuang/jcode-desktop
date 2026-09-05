# Learn panel

The tutorial lives in the **learn** tab immediately after **chat** in the
upper-left sidebar. It is opt-in: normal sessions show no onboarding banner,
floating shortcut controls, or extra reserved space above the canvas.

The panel shows one of three short stages, using text rows and small key labels:

1. Find your way: navigate strips and panels, create and close sessions.
2. Make it yours: move panels and select width presets.
3. Take a step back: cycle width and toggle overview.

**Next** and **Back** change stages without requiring every shortcut to be
practiced. **Done** returns to Chat. Reopening Learn preserves the stage, and
both the selected tab and stage survive workspace reload. Old snapshots without
these fields default to Chat and the first stage. Practiced shortcuts retain
subtle checkmarks, and clicking a lesson invokes the same workspace action as
the advertised shortcut. Learn remains available after completion as a reference.

## Layout contract

- Render tutorial content only inside `SidebarView::Learn`, never over a panel.
- Changing tabs or stages must not change the session canvas size or position.
- Keep stage navigation outside the scrollable lesson content so it stays
  reachable in short windows.
- Keep Learn early in the tab strip so it is visible without horizontal scrolling.
- Do not reintroduce a global dock or automatic onboarding chrome.

## Verification

```sh
cargo test -p jcode-desktop-ui tutorial -- --test-threads=1
cargo test -p jcode-desktop-ui onboarding -- --nocapture --test-threads=1
python3 scripts/screenshot.py target/learn-stage1.png --learn-stage 1
python3 scripts/screenshot.py target/learn-stage2.png --no-build --learn-stage 2
python3 scripts/screenshot.py target/learn-small.png --no-build --learn-stage 1 --size 640x480
```

Use fresh screenshot names. The isolated harness never opens a window on the
user's desktop. Geometry tests render all stages at five window sizes, verify
separation from panels and pinned prompts, and check that hiding the sidebar
hides the tutorial. Interaction tests click Learn, Next, Back, Done, and lesson
controls, compare canvas bounds before and after, and round-trip snapshot state.

### Verified result (2026-09-05)

- Tutorial tests: 7 passed. Onboarding tests: 2 passed (one overlaps the tutorial filter).
- Rendered geometry: 15 stage/window combinations, 95 control regions, zero
  panel, pinned-prompt, or control collisions, and zero reserved top pixels.
- Real-app screenshots inspected for all three stages at 1440×1000 and the
  first stage at 640×480. Reduced row spacing after the compact screenshot
  exposed clipping, then confirmed all six rows and Next were visible.
- Screenshot helper tests: 5 passed. Final full UI suite: 300 passed, 6 ignored,
  with the same three pre-existing failures in email inbox scrolling, restored
  history scroll position, and touchpad reticle rendering.
- Live desktop rebuild/reload confirmed by UI generation activation in the log.

### Before/after minimalism measurement

The exact same `tutorial_minimalism_tests.rs` was run against archived revisions
`74e1dec` (top dock) and `cad62f4` (Learn tab), rendering actual Workspace views
with the same session fixture and viewport sizes. Both measurement runs passed.

| Window | Previous canvas height | Learn canvas height | Recovered height | Default tutorial controls |
| --- | ---: | ---: | ---: | --- |
| 640×480 | 326 px | 480 px | 154 px (+47.2%) | 6 → 0 |
| 1440×1000 | 922 px | 1000 px | 78 px (+8.5%) | 6 → 0 |

The canvas top moved from 154/78 px to 0 px. The Learn tab was present in both
new layouts. Clicking it preserved the entire canvas rectangle and kept the
tutorial outside that rectangle. This measures the requested reduction in
default chrome, rather than relying on a visual preference judgment.

To collect the current measurements:
`cargo test -p jcode-desktop-ui tutorial_minimalism_measurement -- --nocapture --test-threads=1`.
For a baseline comparison, copy this version-independent test file into an
archived revision and register it as a test module under `workspace.rs`.
