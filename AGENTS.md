# Agent Instructions

## Git Workflow

- Always commit and push completed changes. Do this every time unless the user explicitly asks you not to.
- Commit only the changes you made. Do not include unrelated or pre-existing modifications.

## Jcode Repository Ownership

- This project also owns and maintains the adjacent Jcode TUI, SDK, harness API, and supporting crates in `/home/jeremy/jcode`.
- When desktop work requires an SDK, protocol, TUI, or shared-runtime change, make the correct change in the Jcode repository directly rather than adding a desktop-only workaround or asking the user to coordinate it.
- Keep cross-repository behavior aligned and commit and push the changes in each affected repository.

## Testing

- Do not use `niri` for testing or test verification.
- Prefer headless tests that do not open windows, steal focus, move workspaces, or otherwise interfere with the user's active desktop session.
- Use non-`niri` test methods, such as unit tests, integration tests, CLI checks, virtual displays, or isolated test harnesses.

## Rebuild and Reload

- After completing and verifying every change, rebuild and reload the running desktop application so the user immediately receives the updated behavior.
- Use `selfdev build` for the standard self-development rebuild and reload workflow. Treat a successful rebuild and reload as part of completion, not as an optional follow-up.
- If rebuilding or reloading fails, diagnose and fix it before reporting the change complete. Do not silently leave the running application on stale code.
