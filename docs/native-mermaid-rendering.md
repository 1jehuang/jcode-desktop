# Native Mermaid transcript rendering

Mermaid fences now use mmdr's optional `scene` API and paint cached GPUI vector
paths directly into the transcript. Inline rendering does not construct an
`Image`, request image decoding, or allocate a bitmap. Text is represented by
shaped glyph outlines, so labels and curves remain vector geometry at display
scale. Glyph outlines are not selectable transcript text.

## Pipeline

1. Parse source and generate an mmdr `Scene` using the selected semantic palette.
2. Tessellate native paths once and cache by source and effective diagram theme.
3. Lay out at intrinsic dimensions, shrinking proportionally for transcript width
   or a 520 logical-pixel height ceiling. Never upscale small diagrams.
4. Transform and paint paths inside the actual canvas and transcript clip.

The palette uses the selected theme rather than intermediate animation frames,
so switching appearance does not trigger sixteen expensive diagram rebuilds.
The forced dark canvas and decorative image frame are removed.

The upstream scene implementation currently normalizes mmdr's existing SVG output
with usvg. It does not yet emit scene primitives directly from layout. This is an
explicit implementation boundary, not an SVG-to-bitmap path. The typed scene API
allows that intermediary to be removed upstream without changing this consumer.

## Scope and fidelity

The native renderer retains path fill rules and shaped glyph holes. It supports
solid fills, horizontal two-stop linear gradients, and arbitrary vector clipping
(including curved clips and fill-rule holes). GPUI lacks offscreen
vector group compositing, so translucent groups use per-primitive opacity and
multiply groups use ordinary source-over. This can differ from SVG where
translucent shapes overlap, notably in some chart types. Unsupported effects
return to the existing text representation rather than silently using a bitmap.

Click-to-enlarge still uses the existing media viewer. Its SVG is generated lazily
on a click, not for inline rendering. Changing that viewer is separate from the
native transcript path.

## Reproduction and checks

`assets/previews/mermaid-tall.mmd` preserves the original tall six-node diagram.
The isolated screenshot harness accepts `--mermaid-source` with `--transcript
mermaid` so vertical, horizontal, and custom diagrams use the production path.

```sh
cargo test -p jcode-desktop-ui --lib mermaid -- --test-threads=1
python3 scripts/screenshot.py target/mermaid-native-light.png \
  --transcript mermaid --mermaid-source assets/previews/mermaid-tall.mmd \
  --theme neutral-light --size 1920x1080
python3 scripts/screenshot.py target/mermaid-native-dark.png \
  --no-build --transcript mermaid --mermaid-source assets/previews/mermaid-tall.mmd \
  --theme neutral-dark --size 900x1080
```

The GPUI integration tests feed streamed assistant events into a real workspace,
assert the native canvas exists, and compare its actual bounds to the intrinsic
scene dimensions across narrow, wide, and resized transcript layouts.

## Verified result (2026-09-07)

- Pinned upstream scene API: `3726ccbffe0e8032361eb9668694b24f77858060`.
- `cargo test -p jcode-desktop-ui --lib mermaid -- --test-threads=1`:
  **18 passed**, including all 23 diagram families, three real streamed transcript
  sizing/resize tests, scene cache/theme checks, and media preview behavior.
- `cargo build -p jcode-desktop`: passed with the pinned Git dependency.
- Inspected `target/mermaid-native-light-final.png` (1920×1080) and
  `target/mermaid-native-dark-final.png` (900×1080): all six original nodes,
  branch labels, arrowheads, and feedback loop are visible with matching palettes.
- Upstream scene-only unit, integration, and doctests pass. Its broader existing
  suite has a separately reproduced layout-cycle fixture failure without scene
  enabled, so this change does not claim a clean upstream full-suite run.
