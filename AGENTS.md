# Agent Instructions

## Git Workflow

- Always commit and push completed changes. Do this every time unless the user explicitly asks you not to.
- Commit only the changes you made. Do not include unrelated or pre-existing modifications.

## Jcode Repository Ownership

- This project also owns and maintains the adjacent Jcode TUI, SDK, harness API, and supporting crates in `/home/jeremy/jcode`.
- When desktop work requires an SDK, protocol, TUI, or shared-runtime change, make the correct change in the Jcode repository directly rather than adding a desktop-only workaround or asking the user to coordinate it.
- Keep cross-repository behavior aligned and commit and push the changes in each affected repository.

## Visual Design

- Never use decorative left-hand vertical lines, accent bars, or tree rails to mark or group content, including sidebar swarm agents. Use indentation, spacing, typography, or subtle background fills instead.
- This rule does not prohibit functional scrollbars, pane dividers, or complete control outlines.

## Testing

- For visual inspection, run `python3 scripts/screenshot.py target/ui-review.png`.
  This renders the real app with offline fixture data on a private Xvfb display.
  Use `--no-build` only when the binary is current. Read the resulting PNG with
  the image tool. Do not ask the user for a screenshot before trying this path.

- Do not use `niri` for testing or test verification.
- Prefer headless tests that do not open windows, steal focus, move workspaces, or otherwise interfere with the user's active desktop session.
- Use non-`niri` test methods, such as unit tests, integration tests, CLI checks, virtual displays, or isolated test harnesses.

## Rebuild and Reload

- After every significant edit, verify the change, then rebuild and reload the running desktop application so the user immediately receives the updated behavior. Do this throughout the task, not only at the end.
- In a hot-reload desktop session, use the application's **Ctrl+R** rebuild-and-reload action after verification. Treat a successful rebuild and reload as part of completion, not as an optional follow-up. Do not use `selfdev build`, which rebuilds the Jcode CLI rather than Jcode Desktop.
- If rebuilding or reloading fails, diagnose and fix it before reporting the change complete. Do not silently leave the running application on stale code.
