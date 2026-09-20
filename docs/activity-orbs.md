# Native Thinking Orbs activity indicator

Transcript, latest-output, and sidebar activity indicators use the **Working**
animation from [gpui-thinking-orbs](https://github.com/FrancoEscob/gpui-thinking-orbs),
at its purpose-designed **20px Inline** size. This replaces the custom 14px,
8fps halftone donut, not the canonical Jcode logo or application icon.

## Dependency and attribution

- Rust port: Franco Escobar, MIT, pinned to
  `8bb92031040688bfd24e4d6bd96c76b61ed86ad1`.
- Original animation design: Jakub Antalik's
  [Thinking Orbs](https://github.com/Jakubantalik/thinking-orbs), MIT.
- A root Cargo patch resolves the library's crates.io GPUI dependency to the
  exact same Zed revision used by the desktop host and UI. There is one GPUI
  package in the lockfile, not two incompatible sets of entity/window types.

The dependency owns the animation math, inline density, projection, depth order,
dot radii, alpha, and preset speed. Jcode does not maintain a copied engine or
substitute its old torus geometry.

## Native adapter

`panel_activity.rs` uses the library's public geometry API with a retained `Frame`.
The inline design contains 39 dots on tilted orbits. It is not the larger avatar
design shrunk down, and has no fixed-frame animation cache or raymarcher.

The host adapter keeps two application-specific behaviors:

- A 30fps timer is armed only from visible paint, with at most one timer in
  flight. It never rearms itself. A clipped or unmounted indicator stops after
  the pending tick, without refreshing the parent transcript.
- Reduced motion uses the upstream static representative pose. Ink is mapped
  between the desktop theme's text and panel-paper colors, retaining the
  upstream per-dot depth and alpha.

Small circles use fractional-coordinate native paths in one paint layer. The
pinned GPUI implementation snaps quad bounds to device pixels, so drawing the
upstream dots as rounded quads would introduce position/diameter jitter. Each
dot keeps its own color and compositing order rather than merging overlapping
contours into a differently shaded shape.

## Validation

```sh
cargo test -p jcode-desktop-ui --lib panel::activity -- --nocapture
cargo test -p jcode-desktop-ui --lib sidebar_spinner
cargo build -p jcode-desktop
python3 scripts/verify-activity-orb.py target/activity-orb-review
python3 scripts/screenshot.py target/activity-orb.png --no-build --transcript streaming
```

The native acceptance script uses private Xvfb displays and offline fixture
data. It retains eight screenshots per case and checks the sidebar activity
region for motion in light/dark themes, and stability in reduced-motion/idle
states. It waits for real presentation rather than accepting a blank image just
because a layout-state file exists. Inspect the full images and enlarged crops
as well as the automated pixel checks. Eight distinct samples establish visible
motion, not measured 30fps delivery. `fixture_cpu_percent` measures the entire
debug application under software rendering, including full-window compositing.
It is not a hardware-GPU measurement or an isolated spinner cost.
