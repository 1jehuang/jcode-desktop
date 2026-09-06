# Diff review

File-edit tools render one clickable metadata row per file, with its path, change kind, and added/removed line counts. The inline preview stays short. Clicking the path, counts, or **Review** opens a panel-sized change review without replacing the underlying transcript.

The review contains a collapsible tree of the files **in that tool call**, the selected file's full diff, previous path for renames, and a **Copy diff** action. **Back to chat** or Escape closes it and restores composer focus. Chat scroll position and draft are retained. Long diffs use the virtualized GPUI list rather than a line-count cutoff.

These are tool-input snapshots, not repository status or a current working-tree diff. Running and failed tools are labeled accordingly. Writes are labeled `Written`, since the input does not establish whether the file already existed. Line numbers are shown only when supplied by unified hunk headers. Unknown deletion contents are not fabricated. Review does not read local files, so remote session paths cannot accidentally resolve against the desktop filesystem.

## Verification

- `cargo test -p jcode-desktop-ui --lib diff -- --test-threads=1`
- `cargo build -p jcode-desktop`
- `python3 scripts/accept-diff.py target/diff-acceptance.png`
- `python3 scripts/screenshot.py target/diff-review.png --transcript diff --no-build`

The native acceptance runner uses the real desktop on a private Xvfb display. It clicks both metadata links, selects another file through the tree, and checks Escape and Back to chat. GPUI interaction tests additionally check copying, directory collapse/expand, draft and scroll preservation, and scrolling beyond the inline preview.
