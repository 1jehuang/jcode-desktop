# Agent Instructions

## Git Workflow

- Always commit and push completed changes. Do this every time unless the user explicitly asks you not to.
- Commit only the changes you made. Do not include unrelated or pre-existing modifications.

## Testing

- Do not use `niri` for testing or test verification.
- Prefer headless tests that do not open windows, steal focus, move workspaces, or otherwise interfere with the user's active desktop session.
- Use non-`niri` test methods, such as unit tests, integration tests, CLI checks, virtual displays, or isolated test harnesses.
