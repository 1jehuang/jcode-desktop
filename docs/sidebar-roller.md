# Sidebar folder roller

Folder layout uses a cylindrical carousel for the top-left sidebar navigation.
The live-session tabs above the conversation canvas are independent and unchanged
by this feature. Normal layout retains its horizontal navigation strip.

- The centered folder is full size. Neighboring folders recede symmetrically in
  height, width, and vertical position. Far folders paint behind nearer ones.
- Wheel/trackpad input and the two chevrons browse all eleven folders cyclically.
  Browsing does not select a page or launch an action. Click a folder to activate it.
- Tab labels stay inside each exposed face. Tooltips provide the full labels of
  compressed tabs. The centered active folder joins its sidebar page.
- Motion uses the existing Focus transition and reduced-motion policy. Hidden
  rollers do not keep scheduling animation frames. Reload restores the centered
  folder from the existing sidebar-view snapshot without changing its schema.
- Each occluding folder face handles wheel events itself and stops propagation,
  so real native wheel input works on the tabs, not just their backdrop.

## Checks

```sh
cargo test -p jcode-desktop-ui --lib sidebar_roller -- --include-ignored --test-threads=1
python3 scripts/screenshot.py target/roller-review.png
python3 scripts/accept-roller.py target/roller-native
```

The native acceptance runner requires the current desktop binary, Xvfb, Openbox,
xdotool, ImageMagick, Pillow, Tesseract, and Mesa lavapipe. It uses an isolated HOME,
configuration, runtime directory, and offline fixture on a private display. It
does not interact with the user's desktop or account credentials.

## Verified 2026-09-05

- 42 sidebar-filtered GPUI/unit tests passed, including roller curvature, cyclic
  wheel accumulation, clicking every page, session history, and Normal scrolling.
- Five additional checks passed for settings toggles, theme/keyboard selection,
  new-session coaching, opening a todo's chat, and Normal panel geometry.
- Native acceptance passed in `target/roller-acceptance-11`: wheel input advances
  over both the blank header and an occluding tab face. Center clicks activate
  Learn, Files, Accounts, and Theme. Browsing and eleven-tab wraparound preserve
  panel IDs/count, focus, active page, and native-window count.
- Real offline screenshots were inspected at `target/ui-review-roller-final.png`
  (1440×1000, warm-neutral) and `target/ui-review-roller-light.png` (800×600,
  neutral-light). Side labels remain in their exposed faces.
- The running desktop acknowledged the Ctrl+R-equivalent instance command and
  logged successful activation of UI generation 8 after the final input fix.
- The broader UI suite was also attempted, but did not pass: it reported other
  panel/gesture failures and aborted in a Gmail worker's test-scheduler teardown.
  The passing results above are targeted regression evidence, not a full-suite pass.

## Interpretation and outcome audit

The request identified the top-left folder tabs and asked for a curved roller.
I interpreted that as a horizontal cylindrical carousel, not a vertical picker.
I also chose browse-on-scroll and select-on-click, rather than automatic page
selection while scrolling. Neither detail was explicitly specified by the user.
The first investigation also read the separate live-session tab implementation,
but no implementation was changed before identifying the sidebar navigation as
the target. The roller's implementation changes remain confined to that target,
its obsolete outline painter, and supporting tests. Normal navigation is retained.

The requested change is demonstrated by measured output, not just source review:

| Real screenshot measurement | Before | After |
| --- | --- | --- |
| Top edge of three inactive folders | y=22,22,22 (flat) | Four symmetric depth levels |
| Left-to-right sampled top contour | y=18,22,22,22 | y=32,24,19,18,19,24,32 |
| Center-to-outer depth | No progressive inactive depth | 14 pixels |

Measurements use the first pixel differing from the header background by more
than five RGB channel levels in y=17..49. Before samples are x=34,85,142,205 in
`target/startup-ui-review.png`. After samples are x=26,43,71,132,192,220,237 in
`target/ui-review-roller-final.png`. The exact after contour was reproduced in
`target/roller-audit-final/01-initial-sessions.png` on a fresh native run.
The comparison is deliberately restricted to the header because other agents
also changed the conversation canvas during this task.

The fresh `target/roller-audit-final` run passed all 14 native checkpoints again.
A wheel step on both the backdrop and tab face brought Learn to the center,
confirmed by clicking that position and observing the actual sidebar become
Learn. Arrows reached Files, Accounts, and Theme; wrapping returned to Chat.
Panel identity/count, focus, page, and native-window count remained unchanged
while browsing. Numeric observations are saved in `target/roller-audit.json`.

This establishes improvement on the requested flat-to-curved and roller-navigation
criteria. It does not establish that the user prefers this interpretation to a
vertical roller. Neighbors intentionally expose less label text than the old flat
strip; the centered label and full-label tooltips preserve access to their names.

## Complete requirement mapping

The follow-up [requirement-to-check map](sidebar-roller-requirements.md) records
observed results for all three explicit requirements and 22 changed public-output
contracts. It adds tooltip, reduced-motion recovery, hidden-frame scheduling,
snapshot restoration, and all five action-tab checks. The final run passed 46
sidebar tests plus five additional regressions and 17 native checkpoints. A
reduced-motion policy-resumption bug found during this mapping was fixed, rebuilt,
and activated in the running desktop before the final native run.
