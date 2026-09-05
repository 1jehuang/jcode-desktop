# Workspace layout modes

Choose **Settings → Workspace layout → Folder tabs / Normal**. The preference is saved as `desktop.appearance.layout_mode` (`folder_tabs` or `normal`) in the shared Jcode configuration, or `appearance.layout_mode` in a standalone desktop config. It also survives Ctrl+R through `WorkspaceSnapshot`. Existing configurations default to Folder tabs.

## Native Folder tabs

The selected sidebar session and focused panel are **one filled GPUI vector path**, not independently painted panel backgrounds or inverse-corner masks. `folder_surface.rs` collects the real, clipped layout bounds during prepaint, computes the external contour of their connected union, rounds its convex and concave corners, and paints it once beneath the content.

- The selected page uses `PANEL_BG` (`#25221f` in Warm neutral).
- The sidebar and inactive panels share the root backing color, `HEADER_BG` (`#302b27`). Inactive panels do not paint separate rectangles.
- The connector rises from the selected session to the top shoulder and focused panel. It does not extend as an unrelated rail below the selected session.
- The active tab starts 16px below the canvas top, the shoulder starts at 32px, and inactive content starts at 48px. A 12px outer right margin and 16px bottom margin retain the page silhouette without a focus ring.
- Empty/reconnecting conversations retain a compact session label. Content, inputs, status indicators, and code keep their semantic colors.
- Overview is a separate card canvas, rather than inheriting the connected folder backing.

## Normal

Normal mode renders independent, equally tall rounded panel cards with 12px gaps and a conventional focus border. There is no selected-page connector or native folder surface. Switching modes does not replace sessions or transcripts and recalculates the camera for the changed canvas width.

## Visual feedback loop

```sh
python3 scripts/screenshot.py target/folders.png --panels 2 --size 1644x1008
python3 scripts/screenshot.py target/normal.png --panels 2 --size 1644x1008 --layout-mode normal --no-build
python3 scripts/screenshot.py target/folders-left.png --panels 2 --size 1644x1008 --focus-panel 0 --no-build
```

The first command builds the current app. The others may use `--no-build` only when that binary is current. Captures use real GPUI on private Xvfb/Openbox, not the user's desktop. Read the PNGs after each significant visual change. Reviewed development captures include `native-folders-v1.png`, `native-folders-v2.png`, and `normal-mode-v1.png` under `target/`.

The previous implementation passed color-count and flood-fill checks but was rejected in the user's two-panel screenshot. Those checks did not establish a satisfactory silhouette or native construction. This implementation supersedes that design and the earlier acceptance claims.

## Real runtime and native input acceptance

```sh
cargo build -p jcode-desktop
python3 scripts/accept-folder-panels.py target/native-modes-two --panels 2
python3 scripts/accept-folder-panels.py target/native-modes-three --panels 3
```

Use fresh output directories. The driver starts an isolated real daemon, harness API, and desktop without screenshot fixtures or inherited credentials. Native keyboard input creates SDK-backed sessions. Public API attach/name/detach operations and the real catalog refresh exercise metadata integration. The catalog may omit open sessions, so checks validate the sidebar's merged live panel identities rather than requiring a nonempty catalog reply.

The same process then exercises keyboard focus, pointer focus, overview selection, navigation to Settings, switching to Normal, and switching back. It checks the saved TOML, observable mode state, actual card gaps, and folder surface connectivity. It does not submit inference prompts. All private processes are cleaned up on success or failure.

| Requirement | Check |
| --- | --- |
| One native piece | Contour union/area test, one `paint_path` site, no child background/corner masks, native raster flood-fill from selected sidebar tab to focused panel |
| Two connected structural tones, no folder focus ring | Real two- and three-panel focus captures check backing/page colors, connected components, margins, and raised silhouette |
| Independent Normal mode | Native Settings click and raster samples verify separate same-height cards and actual background gaps |
| Saved mode and reload compatibility | Real Settings writes checked on disk, isolated config round trips, snapshot round trip and legacy default, Settings snapshot restoration test |
| Empty panels remain identifiable | Title regression and raster ink check before real sessions receive custom names |
| Focus/overview remain functional | Native keyboard/click focus, overview hover and selection, then Settings round trip to chat |
| Reliable current-state visual inspection | Public screenshot CLI for both modes with native focus clicks and isolation/argument tests |

These checks verify construction and observed behavior, not subjective approval of the final visual design or resolution of live daemon attachment timeouts.

## Observed verification (2026-09-05)

- Real daemon/SDK/native-input runs passed for both two and three sessions in `target/native-modes-pass-2` and `target/native-modes-pass-3`. Each passed all focus positions, overview hover/selection, Settings → Normal → Folder tabs, persisted TOML values, and return to chat without replacing session IDs.
- Raster checks passed for the continuous selected sidebar/page component, continuous inactive backing, two structural tones, no folder ring, raised silhouette, and Normal card gaps. Final native folder and Normal screenshots were opened and visually inspected, alongside populated transcript fixture captures during iteration.
- Eight focused folder/config/geometry tests passed. Snapshot and native Settings tests passed separately. Screenshot CLI isolation/argument tests passed.
- The supplementary full UI suite reported 302 passed, 5 failed, and 6 ignored. Its failures were the same scroll-state/gesture and timing-sensitive vertical animation tests seen before this native-mode implementation. Both vertical animation tests passed when rerun alone serially. This is not a claim that the entire suite is green.
