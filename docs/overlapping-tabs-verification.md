# Panel-attached overlapping session tabs

The folder strip now keeps full folder silhouettes rather than squeezing every
folder to the same width. The selected tab is centered over its own visible
panel, not over the overall strip. Siblings fan out on either side, using
asymmetric overlap when the active panel is away from the middle. The selected folder is painted last, lifts by 4px, and
gets up to 208px for its emoji and title. Inactive folders stay up to 112px wide,
with their outer edges exposed on the appropriate side of the selection.

Focus changes animate title expansion and lift over the existing 150ms focus
transition. The tab follows the actual rendered panel coordinates, including
camera, width, and reorder motion, without a second position tween that could
leave it trailing behind its panel. Borders use 50% opacity when focused and
45% otherwise, while text and emoji stay opaque. Retargeting a moving tab is continuous. Window resizing and reduced
motion snap to the new geometry instead. Stable panel entity IDs preserve motion
identity across changes to the slot list. Full titles remain available on hover.

There is no minimum exposed width that pushes tabs off the strip. This keeps all
tabs in the layout, but no finite display can make an unlimited number of edges
individually distinguishable. At extreme counts, physical pixels are the limit.

## Verification

- `cargo test -p jcode-desktop-ui live_tabs --lib -- --nocapture`: nine passed.
  The integrated rerun also passed with the concurrently developed working-emoji
  renderer. Geometry checks cover 1–200 tabs, widths 0–2400px, and first/middle/last
  selections. Motion checks cover intermediate expansion/lift, reversal without
  a jump, settling, reduced motion, and resize snapping.
- GPUI window tests exercise actual rendered bounds at 480, 640, 800, 1000, and
  1440px, tooltip appearance, keyboard navigation, exposed-edge clicks, composer
  focus, minimap reservation, empty rows, and the new-session hit-area boundary.
- `python3 scripts/screenshot.py target/ui-review-overlap-crowded.png --panels 6 --size 800x700`
  shows all six silhouettes, the selected full title, and neighboring emoji edges.
- `python3 scripts/screenshot.py target/ui-review-overlap-centered.png --no-build --panels 3 --size 1440x900`
  shows the three-folder group centered over the conversation area, with balanced
  empty space on each side and the active folder raised above its neighbors.
- The real daemon/SDK/native-desktop acceptance uses twelve actual sessions in an
  800×700 private Xvfb window. It sends native keyboard and mouse events, not
  fixture data or injected application actions. Forward, backward, and large
  jumps click each folder's exposed edge. Each checkpoint checks the selected
  real session and composer focus, with no tab scrolling or extra session creation.
  The initial runs passed all 26 exposed-edge clicks before reload. One reload
  attempt hit an unrelated in-progress host compile error, fixed locally. The
  next attempt was interrupted by the agent server replacement during a queued
  rebuild. Neither interrupted reload attempt is counted as a passing reload.

## Panel-attachment refinement

- A rendered-window regression changes selection among three panels and pans the
  camera through 0, 80, and 160px. On every frame the selected tab's left/right
  bounds remain inside its visible panel, and its bottom exactly meets the
  panel's top. This checks the actual GPUI elements, not only layout arithmetic.
- Additional geometry tests cover partially clipped panels and 1, 3, 12, and
  200 tabs at 40, 192, 512, and 1152px. Every sibling keeps a positive exposed
  edge while the visible active folder stays over its panel.
- `target/ui-review-attached-tabs.png` shows the selected folder above the
  right-side panel, rather than straddling the preceding panel as the centered
  version did. `target/ui-review-attached-tabs-wide.png` confirms the wide layout.
  Both real-app fixture captures were inspected. The softened borders remain
  distinguishable without the previous high-contrast outline.
- Native acceptance now waits for initial async activation before asking for a
  subsequent Ctrl+R, identifies exited child processes, and removes unloaded
  temporary plugin copies after each run. Earlier repeated tests filled the
  disk with plugin copies. Only finished private-test copies were removed,
  restoring 8.6GB of free space. No user data or build outputs were removed.

## Final end-user acceptance

`python3 scripts/accept-navigation.py target/attached-tabs-accepted --compact-tabs --reloads 1`
passed with **157 state checkpoints and 52 native exposed-edge clicks**, covering
all twelve real sessions in both directions, first/last/middle jumps, keyboard
navigation, empty-row return, session identity preservation, and composer focus.
The full sequence passed both before and after actual UI generation 2 activation.
The check reads the renderer's actual exposed-edge targets after motion settles,
rather than assuming fixed tab geometry. A readiness/read race in the diagnostic
consumer was fixed by retaining the validated snapshot. Temporary plugin copies
were absent after successful cleanup. The final nine tab tests also passed.

The user's desktop acknowledged the Ctrl+R action and activated UI generation 6
with the panel-attached, softened-border design. A later optional refresh of the
new diagnostic fields found no running desktop process and a stale socket
(connection refused). The application was not restarted or refocused. The
visible implementation had already been loaded successfully before it exited.
