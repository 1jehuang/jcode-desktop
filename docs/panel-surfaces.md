# Workspace layout modes

The focused panel is painted last within its strip. In folder-tab mode, the
shared strip backing uses the inactive panel color, so the focused panel's
rounded corners remain visible above its neighbors. The selected sidebar tab
keeps its raised paper color. Focus does not change panel bounds or text opacity.

After building Desktop, run `python3 scripts/accept-panel-layers.py target/panel-layers`
to check the actual corner pixels while clicking between panels on private Xvfb.

Choose **Settings → Workspace layout → Folder tabs / Normal**. The preference is saved as `desktop.appearance.layout_mode` (`folder_tabs` or `normal`) in the shared Jcode configuration, or `appearance.layout_mode` in a standalone desktop config. It also survives Ctrl+R through `WorkspaceSnapshot`. Existing configurations default to Folder tabs.

## Native Folder tabs

The selected sidebar session and focused panel are **one filled GPUI vector path**, not independently painted panel backgrounds or inverse-corner masks. `folder_surface.rs` collects the real, clipped layout bounds during prepaint, computes the external contour of their connected union, rounds its convex and concave corners, and paints it once beneath the content.

- The selected page uses `PANEL_BG` (`#25221f` in Warm neutral).
- The sidebar and inactive panels share the root backing color, `HEADER_BG` (`#302b27`). Inactive panels do not paint separate rectangles.
- The connector rises from the selected session to the top shoulder and focused panel. It does not extend as an unrelated rail below the selected session.
- Only the sidebar session is a tab. The folder body has one level top edge aligned with the selected navigation tab (18px below the canvas top), and every panel starts inside that body at 48px regardless of focus. A 12px outer right margin and 16px bottom margin retain the page silhouette without a focus ring.
- Empty/reconnecting conversations retain a compact session label. Content, inputs, status indicators, and code keep their semantic colors.
- Overview is a separate card canvas, rather than inheriting the connected folder backing.
- The horizontal navigation scroll thumb is a thin highlight following the actual rounded tab outlines. Folder mode has no detached rail above them. Wheel scrolling and the outline hit strip retain scrolling behavior, while Normal keeps its conventional rail.

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
| Two connected structural tones, no folder focus ring | Real two- and three-panel focus captures check backing/page colors, connected components, margins, and level body edge |
| Independent Normal mode | Native Settings click and raster samples verify separate same-height cards and actual background gaps |
| Saved mode and reload compatibility | Real Settings writes checked on disk, isolated config round trips, snapshot round trip and legacy default, Settings snapshot restoration test |
| Empty panels remain identifiable | Title regression and raster ink check before real sessions receive custom names |
| Focus/overview remain functional | Native keyboard/click focus, overview hover and selection, then Settings round trip to chat |
| Reliable current-state visual inspection | Public screenshot CLI for both modes with native focus clicks and isolation/argument tests |

These checks verify construction and observed behavior, not subjective approval of the final visual design or resolution of live daemon attachment timeouts.

## Observed verification (2026-09-05)

- Real daemon/SDK/native-input runs passed for both two and three sessions in `target/native-modes-pass-2` and `target/native-modes-pass-3`. Each passed all focus positions, overview hover/selection, Settings → Normal → Folder tabs, persisted TOML values, and return to chat without replacing session IDs.
- Raster checks passed for the continuous selected sidebar/page component, continuous inactive backing, two structural tones, no folder ring, body silhouette, and Normal card gaps. Final native folder and Normal screenshots were opened and visually inspected, alongside populated transcript fixture captures during iteration.
- Eight focused folder/config/geometry tests passed. Snapshot and native Settings tests passed separately. Screenshot CLI isolation/argument tests passed.
- The supplementary full UI suite reported 302 passed, 5 failed, and 6 ignored. Its failures were the same scroll-state/gesture and timing-sensitive vertical animation tests seen before this native-mode implementation. Both vertical animation tests passed when rerun alone serially. This is not a claim that the entire suite is green.

### Follow-up: panels belong to the body, not individual tabs

The user clarified that only the sidebar session should be a tab. Removed the focus-dependent panel offset: all panel content now starts at 48px inside the same body whose top edge is at 16px. Focus still changes the page color, never its content height. The navigation backing now matches the surrounding folder backing, removing the pinched upper-left color junction.

The screenshot loop compared `folder-body-flat.png`, `folder-body-left-join.png`, and the final `folder-body-smooth-v2.png` with its 4× `folder-body-smooth-v2-join.png` crop. An intermediate 16px-deep body connection was visually obstructed by the navigation indicator and failed raster connectivity. The final 32px-deep connection remains uninterrupted beneath it. Eight focused tests passed, including equal panel tops at every focus position, both sidebar visibility states, and two viewport sizes. Real two- and three-session runs in `body-flat-v2-2` and `body-flat-v2-3` passed level-body raster samples, matching backing colors across the upper-left join, sidebar/page connectivity, native focus/overview, and Normal/Folder tabs Settings round trips. Live rebuild/reload activated generation 21.


### Follow-up: integrate navigation scrolling into the outline

The top-left scrollbar was interpreted as the horizontal rail above Chat/Learn. Replaced that rail in Folder tabs mode with a 1.5px native stroke along the real visible tab upper borders, including their curved corners. The stroke reads current layout bounds and scroll offset, so it follows tabs when scrolled rather than drawing another floating rectangle. The folder body top now aligns with the selected navigation tab's top edge. Normal mode is unchanged.

Reviewed populated `outline-scroll-v1.png`, its 4× navigation crop, and real-runtime `outline-scroll-wheel-native-4x.png` after wheel scrolling. The old y=5 rail region now contains only backing pixels. Native wheel events visibly moved the tabs, clicking the outline scrolled to Settings, and Settings controls remained independently clickable. Two- and three-session native runs passed mode round trips and folder continuity. The sidebar regression run passed 40 tests with one expected ignored profile, and the Normal-mode regression passed. Final alignment is checked at both viewport sizes with the sidebar visible/hidden.
