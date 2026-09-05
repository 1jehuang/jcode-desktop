# Sidebar title truncation verification

The session title uses a single line with an ellipsis when its text exceeds the available width. The directory/metadata line is intentionally unchanged.

## Observed real-app output

- Built and rendered the real desktop on a private Xvfb display with `python3 scripts/screenshot.py target/sidebar-title-overflow.png` (using `CARGO_BUILD_JOBS=1` for the successful retry).
- The standard screenshot fixture now has the title `Review markdown rendering with a very long session title`, so this path exercises actual overflow rather than only a short title.
- Read the resulting 1440 × 1000 PNG and an enlarged crop (`target/sidebar-title-overflow-detail.png`, source rectangle x=8, y=90, width=250, height=48). The title is on one line and ends with an ellipsis before the status dot. The session emoji and status dot remain visible. The second line contains `/workspace/example`, not continuation text.
- The earlier short-title screenshot, `target/sidebar-title-review.png`, displays `Review markdown rendering` fully on one line. Truncation is not applied unnecessarily.
- The fixture's full title remains visible in the panel header. Only the sidebar display is truncated, not stored session data.

## Limits

The app build and real-render checks passed. The sidebar unit-test command did not complete: the test compiler was terminated with SIGTERM after earlier concurrent build errors and build-lock contention. No unit-test pass is claimed.

The running desktop rejected the Ctrl+R-equivalent reload request because it was launched without hot reload. It was not force-restarted, to avoid losing unsaved drafts. The production change is committed and built but this running process remains on its previously loaded UI until a safe restart.
