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
