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
