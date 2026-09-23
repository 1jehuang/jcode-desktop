# Native Thinking Orbs activity indicator

Open conversation sessions retain a **20px Inline** dotted Jcode donut in the
sidebar. When activity starts, its particles morph into a state-specific
animation from [gpui-thinking-orbs](https://github.com/FrancoEscob/gpui-thinking-orbs).
When work finishes, the same entity morphs back to the resting donut.
The canonical Jcode SVG and packaged application icon are unchanged.

| Session activity | Animation |
| --- | --- |
| Idle, completed, disconnected, or failed | Static dotted Jcode donut |
| Working | Working: tilted particle orbits |
| Thinking | Reasoning: counter-rotating gyroscope loops |
| Responding | Composing: undulating ribbon |
| Running tools | Solving: scrambling/solving bands |

Transcript and latest-output indicators use the same activity selection, but
remain active-only so completed conversations do not retain a busy status row.
Unopened historical sessions do not get a mark. Tab emoji behavior is unchanged.

## Dependency and attribution

- Rust port: Franco Escobar, MIT, pinned to
  `8bb92031040688bfd24e4d6bd96c76b61ed86ad1`.
- Original animation design: Jakub Antalik's
  [Thinking Orbs](https://github.com/Jakubantalik/thinking-orbs), MIT.
- A root Cargo patch resolves the library's crates.io GPUI dependency to the
  exact same Zed revision used by the desktop host and UI. There is one GPUI
  package in the lockfile, not two incompatible sets of entity/window types.

The dependency owns the active animation geometry, inline density, depth order,
dot radii, alpha, and preset speed. `activity_donut.rs` adds only Jcode's branded
72-dot torus and a particle interpolation adapter using upstream `Frame`/`Dot`
types. It does not copy the upstream animation engine or use a raymarcher.

## Native adapter

`panel_activity.rs` observes its owning panel and keeps a retained entity and
geometry buffers across activity changes, including while the conversation is
clipped but its sidebar row is visible. Every selected preset is dot-only.

- Transitions use a 350ms smoothstep interpolation of particle position, depth,
  radius, ink, and opacity. Different particle counts fade/shrink the unmatched
  dots instead of popping them in or out.
- The destination pose is frozen during the morph, then resumes its animation.
  Retargeting captures the currently displayed pose, including an interrupted
  transition, so a quick finish or new activity does not jump back to an endpoint.
- A 30fps timer is armed only from visible paint, with at most one timer in
  flight. It never rearms itself. A clipped or unmounted indicator stops after
  the pending tick, without refreshing the parent transcript. The resting donut
  stops scheduling frames when its return transition completes.
- Reduced motion skips transitions and uses static representative poses. Ink
  maps between theme text and panel-paper colors, preserving dot shading/alpha.

Small circles use fractional-coordinate native paths in one paint layer. The
pinned GPUI implementation snaps quad bounds to device pixels, so drawing the
dots as rounded quads would introduce position/diameter jitter. Each dot keeps
its own compositing order rather than merging overlapping contours.

## Validation

```sh
cargo test -p jcode-desktop-ui --lib panel::activity -- --nocapture
cargo test -p jcode-desktop-ui --lib sidebar_mark_persists
cargo build -p jcode-desktop
python3 scripts/verify-activity-orb.py target/activity-orb-review
python3 scripts/screenshot.py target/activity-orb.png --no-build --transcript orb-thinking
```

The native acceptance script uses private Xvfb displays and offline fixture
data. It retains eight screenshots per case and checks the sidebar region for
motion in all four activity presets, light/dark rendering, and stability in
reduced-motion/idle states. Additional offline fixtures are `orb-working`,
`orb-thinking`, and `orb-tools`. Unit/GPUI lifecycle tests cover exact morph
endpoints, interrupted reversals, dot budgets, static idle scheduling, state
selection, and retaining the same sidebar entity through completion.

The native harness waits for real presentation rather than accepting a blank
image just because a layout-state file exists. Inspect the full images and
enlarged crops as well as automated pixel checks. Eight distinct samples
establish visible motion, not measured 30fps delivery. `fixture_cpu_percent`
measures the entire debug application under software rendering, not isolated
spinner cost or hardware-GPU performance.
