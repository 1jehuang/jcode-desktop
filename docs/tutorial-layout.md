# Tutorial layout safety

Onboarding is a reserved region above `workspace-canvas`, not a transparent
full-window overlay. All lesson controls share a wrapping grid. The grid's
cell, gap, heading, and padding metrics also determine the height removed from
the canvas viewport before sizing panels, row transitions, and the minimap.
Completing onboarding returns that space to the panels.

When adding a lesson:

- Keep it in `render_tutorial_guides` and in normal layout flow. Do not position
  it absolutely over a session, its title, pinned prompt, transcript, or composer.
- Update the stage's control count in `tutorial_dock_height` and the expected
  lesson count in the layout regression. Keep press animations inside the cell.
- Extend the rendered-boundary regression if introducing another protected
  region or control. `assert_no_visual_overlap` checks actual GPUI rectangles,
  not assumed coordinates. Intentionally overlapping modal dialogs should be
  tested separately, rather than exempting tutorial content from these checks.

## Verification

```sh
cargo test -p jcode-desktop-ui tutorial
cargo test -p jcode-desktop-ui workspace::tests:: -- --test-threads=1
python3 scripts/screenshot.py target/ui-review-onboarding.png
python3 scripts/screenshot.py target/ui-review-onboarding-small.png --no-build --size 640x480
```

Use fresh screenshot filenames because the harness refuses to overwrite files.
The layout test renders all three stages at five window sizes, with and without
the sidebar, using a real pinned prompt and task card. It checks pairwise lesson
separation, containment inside the dock, separation from the panel and its text
regions, and continued room for the composer. Separate tests cover clickable
controls, touchpad navigation, stage progression, and space reclamation.

## Measured before/after acceptance (2026-09-05)

The exact same `tutorial_geometry_tests.rs` was compiled against the pre-fix
project (`9352c5e`) in an isolated source archive and against the fixed project
(`f569cfe` plus the acceptance test). The test renders the actual Workspace and
Panel components through GPUI. It does not depend on the new dock's geometry or
assume where the controls should be. Both runs cover 30 layouts and 160 rendered
guide instances, including the step labels, with a pinned previous prompt.

| Observed metric | Before | After |
| --- | ---: | ---: |
| Guide intersections with session panels | 160 | 0 |
| Guide intersections with the pinned previous prompt | 20 | 0 |
| Summed panel intersection area across cases (px²) | 481,380 | 0 |
| Guide-to-guide intersections | 0 | 0 |
| Guides outside the window | 0 | 0 |

The pre-fix run fails the acceptance assertion because all 160 guide instances
cover panel content. The fixed run passes with zero intersections, including
zero intersections with the previous prompt. This establishes an observed
improvement, not just a visual inspection or a test of the sizing formula.
The area is summed over guide instances and test cases, not a unique screen area.

Re-run the measurable acceptance check with:

```sh
cargo test -p jcode-desktop-ui onboarding_geometry_acceptance -- --nocapture --test-threads=1
```

It prints an `ONBOARDING_GEOMETRY` summary before asserting zero collisions.

The fixed full UI suite also ran serially: 295 passed, 3 failed, 6 ignored.
All three residual failures were reproduced on the pre-fix project with the
same assertions: `email_inbox_moves_when_the_user_scrolls`,
`restored_scroll_is_not_replaced_when_history_reattaches`, and
`a_touchpad_swipe_paints_the_gesture_reticle_and_minimap_dot`. They are not
introduced by the reserved tutorial layout. The onboarding acceptance check,
interaction tests, and space-reclamation tests pass on the fixed implementation.
