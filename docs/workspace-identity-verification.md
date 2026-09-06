# Workspace identity outcome verification

Verified on 2026-09-06 against the real desktop renderer on private Xvfb, with
four sessions moved into four workspaces using native keyboard input.

## Measured outcomes

| Requirement | Warm neutral | Neutral light |
|---|---:|---:|
| Correct workspace identities recognized from tab pixels across four selections | 16/16 | 16/16 |
| Correct selected workspace recognized independently from filled-badge pixels | 4/4 | 4/4 |
| Numbered map clicks activate the intended workspace and its keyboard target | 4/4 | 4/4 |
| Minimum pairwise tab-rail RGB distance | 38.22 | 43.74 |
| Minimum selected badge accent coverage | 82.9% | 82.9% |
| Maximum unselected badge accent coverage | 1.6% | 1.6% |

The recognizer reads the rendered colors and filled-badge area, then compares
its result to the requested workspace and actual native navigation state.
It does not use navigation state to decide which badge looks selected. Four
workspace identities remain recognizable in every selected state, and selection
has a large filled-area distinction rather than relying only on hue.

A uniform-chrome negative control is rejected, as are zero or two selected
badges. These controls validate the checker, not a historical app benchmark.
RGB separation is a deterministic regression threshold, not a perceptual or
accessibility standard. This verifies visible distinguishability and correct
navigation, not a claim about measured human task speed or user preference.

The Rust theme test additionally verifies workspace-number color contrast of
at least 4.5:1 on every built-in palette. GPUI interaction tests cover empty
workspaces, moved panels, and identities with the minimap hidden. Existing tab
and minimap regression suites pass.

## Reproduce

```sh
python3 scripts/screenshot.py target/workspace-identity-dark.png --panels 4 --workspace-interact
python3 scripts/screenshot.py target/workspace-identity-light.png --no-build --panels 4 --workspace-interact --theme neutral-light
python3 -m unittest discover -s scripts -p test_workspace_identity_acceptance.py
python3 -m unittest discover -s scripts -p test_screenshot.py
cargo test -p jcode-desktop-ui workspace_ident
cargo test -p jcode-desktop-ui live_tabs
cargo test -p jcode-desktop-ui navigation_map_tests
```

Choose unused output names. Each acceptance run saves four selected-workspace
screenshots and a `.workspace-identity.json` report alongside the final PNG.
The measured run artifacts are `target/workspace-identity-measured-{dark,light}`.
All native input targets the script's private display. No live desktop input,
compositor control, network calls, or model requests are involved.
