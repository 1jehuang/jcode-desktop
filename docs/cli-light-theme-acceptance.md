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

Reports record the resolved executable path and SHA-256, so evidence is tied
to the tested binary rather than a potentially moving `current` symlink.
Artifacts are local generated files under `target`, not committed screenshots.

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
