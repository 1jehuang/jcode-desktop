# Real CLI light-theme acceptance

Verified 2026-09-18 using `scripts/accept-cli-light-theme.py`, the prebuilt
`target/debug/jcode-desktop`, and real Jcode CLI binaries. The CLI's actual
session picker runs on Desktop's embedded terminal PTY, with three synthetic
saved sessions. Arrow keys change the preview without resuming a session.

## Reproduce

Requires Xvfb, Openbox, xdotool, ImageMagick (`import`), Tesseract, Pillow,
Mesa lavapipe, and bubblewrap with unprivileged namespaces enabled.
Use a new output directory each time. No build is performed.

```sh
python3 scripts/accept-cli-light-theme.py target/cli-old \
  --binary /path/to/old/jcode
python3 scripts/accept-cli-light-theme.py target/cli-new \
  --binary /home/jeremy/jcode/target/selfdev/jcode --min-light-contrast 4.0
```

Both light (`neutral-light`) and dark (`warm-neutral`) are captured by default.
`--theme light` or `--theme dark` selects one. `--binary` always means the CLI.
`--desktop-binary` independently overrides the Desktop host.

The optional threshold rejects low observed raster contrast in all three
light samples: unselected metadata, selected metadata, and footer. It does not
apply to dark mode. Omitting it allows characterization of known-bad baselines.

## Observed results

| Actual screenshot sample | Old light | New light | Old dark | New dark |
| --- | ---: | ---: | ---: | ---: |
| Unselected `created:` metadata | 1.831 | 5.608 | 1.676 | 1.676 |
| Selected `created:` metadata | 1.256 | 4.555 | 1.483 | 1.483 |
| Keyboard-help footer | 2.047 | 5.608 | 1.962 | 1.962 |

Ratios are against the actual sampled background. The unselected light
background stayed RGB(248,247,244). Its strongest repeated near-neutral glyph
pixel changed from RGB(185,185,185) to RGB(99,99,99). Visual inspection
confirmed that metadata, paths, and help text became clearly readable while
the selected-row background remained unchanged. Dark sample crops were pixel-identical,
including their existing low-contrast dim text, which this light-only fix does
not claim to address.

Persistent before/after evidence from this run:

- `target/cli-contrast-old/{light,dark}/session-picker.png`
- `target/cli-contrast-new/{light,dark}/session-picker.png`
- The same directories contain `session-picker-selected.png`, enlarged crops,
  OCR text, `report.json`, app/Xvfb logs, and PTY receipts.
- `target/cli-contrast-comparison.json` contains all three sampled regions for
  both versions and themes.
- `target/cli-contrast-gate-new/{light,dark}/report.json` records the threshold run.
- `target/cli-contrast-gate-old-final/light/report.json` records the expected rejection
  of the old CLI at a threshold of 4.0, with screenshots preserved.

- `target/cli-contrast-final-verified/{light,dark}/report.json` records the final
  combined palette/contrast implementation passing the same gate. Both reports
  identify SHA-256 `0b9a133084d9663e4ea5ab8918a2c0036238276ba47cdd5670b38b84b0b90eb3`,
  which also matches the installed current-channel CLI.

One concurrent-load run in `target/cli-contrast-final/light` hit the existing
120 ms OSC 11 detection timeout and fell back to dark. A late color reply then
appeared in the picker filter. That failed run is retained separately, not
counted as a contrast pass. The fresh isolated final run above detected light
correctly. This contrast change does not fix that intermittent startup-query
race.

Reports record the resolved executable path and SHA-256, so evidence is tied
to the tested binary rather than a potentially moving `current` symlink.
Artifacts are local generated files under `target`, not committed screenshots.

## Stronger contrast follow-up

The initial repair still looked too faint to the user. CLI commit `b7b4cdd47`
raises built-in light text to a 7:1 contrast target, leaving backgrounds, explicit
user colors, and dark mode unchanged. On intermediate surfaces where neither
black nor white reaches 7:1, it uses the best available endpoint.

| Light screenshot sample | Initial repair | Stronger repair |
| --- | ---: | ---: |
| Unselected metadata | 5.608 | 8.673 |
| Selected metadata | 4.555 | 7.063 |
| Keyboard-help footer | 5.608 | 8.673 |

`target/cli-contrast-stronger/{light,dark}/report.json` passed the stricter
`--min-light-contrast 6.5` gate with SHA-256
`4bd775bc11bf701b32b9b4383925f849d9a22c565e1c97cfbc31705d7e5e84a6`.
That binary was installed into the current channel. Metadata/footer glyphs are
now RGB(71,71,71), and selected metadata is RGB(60,60,60). All three dark samples
retain their previous ratios. Full screenshots and the enlarged
`first-fix-left-stronger-right.png` comparison were visually inspected.

## Isolation and limits

Desktop uses the existing screenshot harness's environment allowlist and a
private Xvfb display. CLI environment isolation is enforced again in its PTY
worker. Bubblewrap supplies fresh network, PID, and filesystem namespaces,
mounting only system files under `/usr`, the selected binary, and the fresh
artifact sandbox. HOME, JCODE_HOME, XDG paths, and the explicit daemon socket
are private. Telemetry and updates are disabled. No credentials, shared daemon,
user display, model request, or external network are used.

These are actual antialiased raster measurements, not source-color WCAG
certification. The sampler uses repeated near-neutral glyph pixels rather
than colored subpixel fringes. Fixed crops assume this harness's 1440x1000
layout and bundled/default terminal font. Review the saved crop when changing
font, DPI, geometry, or picker layout. The 4.0 threshold is a regression gate
with room for rasterization variation, not a claim that all text meets 4.5:1.
The fixture covers the real picker and preview, not every transcript/tool UI.
