# Agent Instructions

## Testing

- Do not use `niri` for testing or test verification.
- Prefer headless tests that do not open windows, steal focus, move workspaces, or otherwise interfere with the user's active desktop session.
- Use non-`niri` test methods, such as unit tests, integration tests, CLI checks, virtual displays, or isolated test harnesses.
