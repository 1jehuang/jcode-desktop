# Read-only text selection

GPUI text does not become selectable merely by rendering a string. Content-bearing text should use the shared `text_selection` controller rather than a raw string child.

Supported surfaces include assistant/user/reasoning prose, fenced code, tool summaries and expanded invocation/output/error text, expanded task descriptions and intentions, background task labels, source-file contents and paths, and side-document headings and source paths.

- Drag selects characters. Double-click selects a word, triple-click selects a line, and Shift-click extends a selection within a leaf.
- Ctrl+C, Cmd+C, and Super+C copy the focused selection. Each selectable leaf owns its copy context, so content outside the transcript body works too.
- Code fences and source files use a single shaped text leaf for multiline selection. Line-number gutters are not copied.
- Plain leaves resolve inherited typography during layout, not before parent styles are applied.
- Task detail text consumes selection gestures. The task summary remains the expand/collapse control, and tool output pills retain their disclosure behavior.

Selection is currently scoped to one shaped text leaf, not a range spanning separate paragraphs, table cells, or diff rows. Images and rasterized PDF pages do not provide text selection.

## Regression checks

`cargo test -p jcode-desktop-ui --lib selection_tests` exercises real mouse down/move/up events followed by keyboard copy for tool text, tasks, background work, file contents, and document headings. It also checks that selection does not collapse tasks and that tool disclosure still works.

`cargo test -p jcode-desktop-ui --lib fenced_code_selection` covers Unicode, multiline selection without gutters, headerless code, and the existing copy button.
