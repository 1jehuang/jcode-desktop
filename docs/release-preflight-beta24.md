# Desktop beta 24 release preflight

Date: 2026-09-05

## Source snapshots

- Release tag: `desktop-v0.1.0-beta.24`
- Tagged desktop commit: `face07e`
- Clean desktop source archive tested: `d4b171db7fd624e2ef24808ee1c7236370350bcc`
- Release runtime pin: `f11adb5996c541592e28519018709eebebc9fce4`
- Initial compatibility build runtime: `4437e10f86558d552499ce92f78f2be561d48541`

The initial runtime commit was local-only, so the workflows were changed to the fetchable `origin/master` commit `f11adb599`. The desktop-facing SDK, core, base, harness API, root manifest, and lockfile have no differences between those two runtime snapshots.

## Release checks

- Clean committed desktop source compiled successfully with `cargo test --workspace --locked` against the modern runtime.
- Release package verifier: 4 passed.
- `scripts/package-linux.sh`: `bash -n` passed.
- Release workflows parsed as YAML.
- Linux checksum output was changed to contain artifact basenames instead of absolute build paths, with a regression test.
- No pre-existing dirty source was included in either release-preflight commit.

## Existing clean-source UI test failures

The workspace test run completed compilation and reported 327 passed, 6 ignored, and 5 failed in `jcode-desktop-ui`:

1. `panel::tests::email_inbox_moves_when_the_user_scrolls`
   - Isolated rerun failed.
   - Expected the Email inbox scroll offset to change, but both offsets were `0px`.
2. `panel::tests::restored_scroll_is_not_replaced_when_history_reattaches`
   - Isolated rerun failed.
   - Expected restored offset `-137.0`, observed `-0.0`.
3. `workspace::tests::a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`
   - Isolated rerun failed.
   - The expected canvas gesture reticle was not painted.
4. `workspace::tests::vertical_keys_animate_and_restore_each_strips_focus`
   - Failed in the parallel workspace run because row animation was no longer active.
   - Passed in an isolated rerun.
5. `workspace::tests::vertical_keys_cover_both_animation_directions`
   - Failed in the parallel workspace run because transition debug bounds were absent.
   - Passed in an isolated rerun.

The first three are deterministic in-process GPUI `TestAppContext` interaction/state failures. The last two are parallel timing flakes. None launch or call the Jcode runtime, inspect packaged binaries, or exercise platform packaging. They therefore do not indicate a compatibility regression from the runtime pin, but the full clean-source test suite must not be described as green.
