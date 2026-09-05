# Inline tool-call acceptance

## Interpretation

The request was to remove the card around tool calls and make them feel more
seamless. We interpreted the target as **Jcode Desktop**. That surface was inferred
from the running app and its explicit tool-card renderer, not explicitly named by
the user. Keeping status indicators, expansion, errors, and edit previews was also
an implementation assumption. Native todo and background-task cards were not
changed. Work before settling on this interpretation consisted only of docs and
repository inspection, so no earlier edits need to be reconciled.

The objective acceptance criteria are no contrasting tool-call surface or outer
border, and no regression in access to tool details. These checks do not establish
that the user personally prefers the aesthetics.

## Reproduce

```sh
CARGO_BUILD_JOBS=1 cargo build -p jcode-desktop
python3 scripts/accept-inline-tools.py target/tool-inline-acceptance.png
CARGO_BUILD_JOBS=1 cargo test -p jcode-desktop-ui --lib tool -- --test-threads=1
```

Choose a new output filename for each run. The acceptance script uses the real
application on screenshot.py's isolated Xvfb display, with real X11 clicks. It does
not interact with the user's window. Its pixel coordinates and status color are
specific to the 1440x1600 warm-neutral fixture.

## Observed results, 2026-09-05

- All 11 tool-related tests passed, including expansion/collapse, ANSI stripping,
  intent summaries, error and edit-preview placement, and overflow row height.
- **99,294 rendered pixels** across collapsed/expanded tool interiors and former
  left edges exactly matched the surrounding transcript background, RGB
  `(37, 34, 31)`. Card fill or border paint in these regions fails the check.
- A real click expanded the running read tool: **98,635 transcript pixels changed**
  and its header moved **97px** as the added detail took up space in the
  bottom-aligned transcript.
- A second real click restored the collapsed layout with **zero differing pixels**
  in the checked transcript region. Status and expansion therefore remain usable,
  not just present in source code.
- After low-memory protection interrupted builds and closed Desktop, a single-job
  restart succeeded. The application confirmed activation of the rebuilt UI plugin.

The visual measurement covers the default theme and sample running/finished calls.
Error and edit-preview placement are additionally covered by native GPUI tests.

## Whole-result rerun, 23:22–23:33 UTC

Every mapped check was rerun after the implementation and acceptance runner were
complete. This pass is not reported as an entirely successful fresh build.

| Requirement / public output | Check and observed result |
| --- | --- |
| No tool card fill or outer border | Native pixel acceptance passed again: all 99,294 sampled interior/edge pixels match the surrounding transcript. |
| Expandable details remain accessible | Real X11 click revealed details, changed 98,635 pixels, and added 97px of height. Second click restored zero-difference collapsed pixels. |
| Running, failed, and edit-tool rows remain visible | `inline_tool_rows_keep_status_errors_and_edit_previews` passed for all three states, including error/preview visibility and 24px text indentation. |
| Intent summary remains readable | Both summary tests passed, including painting an intent supplied by a legacy input event in the header. |
| Tool output formatting is preserved | ANSI stripping, pretty-printing/clipping, and header expansion/collapse tests passed. |
| Edit and patch previews remain meaningful | Both edit and patch tests passed for removed and added lines. |
| Long transcripts do not squash rows | Overflow test passed with unchanged intrinsic row height with 30 calls. |
| Dedicated todo output is unchanged | Todo parsing and native-card painting tests passed. |
| Tools coexist with other transcript content | `the_demo_transcript_paints_every_item_shape` passed on the current source. |
| Fresh complete application build | Attempted, first interrupted by the shared server reload, then retried. Compilation reached linking, but mold failed with `Disk full?`; the filesystem had only 152 MiB available. **Blocked by disk capacity, not a passing build.** |
| Native behavior after the attempted build | Re-ran the real-app pixel/click acceptance using the available successful 23:31 build. It passed. This does not substitute for the blocked fresh relink. |

The rerun totaled 11 tool tests plus the mixed-transcript test, all passing.
The native fixture is `target/tool-inline-whole-result-existing-build.png` with
expanded/collapsed companions. Earlier successful build and live activation
remain valid historical evidence, but the latest fresh relink is explicitly
unverified until disk capacity is available. No unrelated files were deleted to
force that check through.
