# Diff presentation

The shared diff model (`diff_model.rs`) keeps file identity, hunk boundaries,
context, old/new line positions, change counts, and UTF-8-safe intraline ranges.
The native viewer (`diff_view.rs`) offers:

- Unified and aligned before/after layouts.
- Syntax-colored code, tinted added/removed rows, explicit change markers, and
  stronger backgrounds on the characters that changed.
- Wrapping by default, with horizontal scrolling available when wrapping is off.
- Per-file collapse, revealable unchanged context, and bounded pages of 80 rows
  and eight files. Paging never changes what the copy controls include.
  Pathological single lines show a Unicode-safe 4,096-character preview with an
  explicit notice. Copy diff still includes their complete contents.
- Selectable code, copyable review text and file paths, and copy feedback.
- The same viewer for clickable tool file review and fenced `diff`/`patch` Markdown blocks.
- Per-file layout and scroll positions retained while browsing the review tree.

## Accuracy

Tool inputs are **requested changes**, not a working-tree diff. A successful
call is labeled completed, not proof that every displayed proposal was applied.
An `edit` only provides a snippet. Its line numbers are explicitly relative to
that snippet. `replace_all` counts cannot claim the number of actual filesystem
matches. A `write` shows proposed content without pretending the old file was
empty. Unified patch positions come from their hunk headers. Malformed positions
remain unknown, and missing content is called out rather than fabricated.

Copy diff copies readable review text with file identity, notes, and all supplied
hunks. It does not claim to produce an independently applicable patch from a
snippet. No file is read, changed, or opened as a side effect of rendering.

## Verification

```sh
cargo test -p jcode-desktop-ui --lib diff -- --test-threads=1
python3 scripts/screenshot.py target/diff-rich-review.png --transcript diff-rich
python3 scripts/accept-rich-diff.py target/diff-rich-acceptance.png
```

The screenshot runner builds the real app and renders offline data on private
Xvfb. The acceptance runner requires a current binary and uses native X11 clicks
to check layout switching, wrapping, clipboard feedback, and collapse/expand.
Use unique output paths. It deliberately refuses to overwrite screenshots.

The GPUI tests additionally exercise hidden-line/file pagination, copying all
content, context reveal, parent click isolation, narrow panels, and Markdown
state retention across repaint. Parser tests cover multi-file Codex/unified
patches, batch wrappers, binary/metadata changes, malformed input, newline-only
edits, Unicode, and bounded computation on large edits.
