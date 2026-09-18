# Desktop login verification

Desktop has two independent login surfaces: the optional Jcode account welcome screen and AI provider accounts. A Jcode account is not required to use your own providers.

## Behavior

- Provider OAuth binds an available loopback redirect before opening the browser. The SDK checks callback path and state, then delegates exchange and PKCE validation to the CLI. A busy port or hosted redirect retains manual code/URL entry.
- Device authorization polls automatically. Browser, copy-code, cancellation, and back controls remain accessible while waiting.
- Enter submits a focused credential field. New prompts receive focus, new provider attempts reset scroll, and completion clears transient input and refreshes available accounts. Failed attempts offer a fresh sign-in.
- The compact account welcome screen includes the Jcode logo, public-link copy fallback, expiry/progress feedback, retry, and optional skip. Provider rows use bundled licensed logos and separate browser sign-ins from API keys.

## Verification performed (2026-09-18)

| Requirement | Evidence | Result |
| --- | --- | --- |
| Native provider entry, navigation, masking, draft preservation | `cargo test -p jcode-desktop-ui login --lib`, 27 tests | Passed |
| Browser callback, pasted code, and automatic device completion | `panel_login_flow_tests.rs` uses the real controller and SDK, isolated CLI fixture subprocesses, and an actual loopback HTTP redirect | Passed. No real provider token exchange |
| Account approval, pending, denial, expiry, 429 slowdown, transient failure, cancellation | `cargo test -p jcode-desktop-ui account_sign_in::tests --lib`, 13 tests including production HTTP/Tokio handling against a loopback server | Passed. Browser opening and credential writes disabled in these UI tests |
| Account persistence and production service contract | Shared Jcode `account_login` tests use an isolated credential home for save, permissions and reload. Live start/pending check uses the deployed API | Passed. No email sent or live approval granted |
| SDK callback security and compatibility | Shared `jcode-sdk` suite and opt-in installed CLI Claude/OpenAI begin/cancel | Passed. Includes invalid callbacks, occupied ports, cancellation, manual completion, and bounds |
| Native provider mouse/keyboard and clipboard | `OMP_THREAD_LIMIT=1 python3 scripts/screenshot.py target/login-acceptance-final.png --transcript empty --login-interact` | Passed on private Xvfb |
| Native account welcome, browser fallback, copy, skip, re-entry and preferences | `python3 scripts/screenshot.py target/account-login-final.png --account-sign-in-interact` | Passed on private Xvfb |
| Compact account UI | 360px GPUI bounds test plus real-app screenshots | Passed |
| Running app receives the changes | Host Ctrl+R `rebuild_and_reload` action via `--reload-ui`, activation log and loaded-library SHA-256 compared with built plugin | Same-process reload verified |

The screenshot fixtures never authenticate. OCR corrections are restricted to observed exact-token ambiguities (`OpenAl` → `OpenAI`, and the copy-link label), with geometry, native clicks and masking checks retained.

**Limit:** Live third-party OAuth approval and the email magic-link click were not completed. Those require the account owner's browser/email confirmation. Isolated transport success, live start/pending compatibility, and native UI acceptance are not a claim that a fresh production login was approved.
