# Tab animation texture identity

## Failure

A tab's two native emoji poses, decoded attachments, and Mermaid previews share
GPUI's image atlas. `ImageIds` used to be a Global declared inside the UI crate.
The linked UI and the cdylib are different compilations of that crate, so their
Global TypeIds differ. The first plugin activation could therefore restart the
counter in the same App while the atlas still held the linked UI's textures.
The next animated pose could display an attachment instead of its emoji.

The live log reproduced this on September 12: before generation 1, texture
`9223372036854775811` belonged to source `3a1c66ad5e09ab25`. After activation,
the counter restarted and assigned that texture to source `37885ce69ee611d4`.
These are opaque cache identifiers, not user content.

## Fix

The counter and its Global implementation now live in `jcode-desktop-api`, a
shared dependency of both UI builds. UI wrappers retrieve that same app-owned
counter. The new range starts at `0xc000000000000000` on 64-bit systems, away
from both GPUI's low IDs and the old UI-local `0x8000000000000000` range. This
allows an already-running host to receive the fix without aliasing old textures.
IDs remain monotonic and exhaustion panics instead of wrapping.

Emoji frame diagnostics record each pose's texture ID once when rasterized, not
on animation ticks. No session identifiers, titles, paths, or image data are logged.

## Checks

```sh
cargo test -p jcode-desktop-api --lib
cargo test -p jcode-desktop-ui --lib image_cache -- --test-threads=1
cargo test -p jcode-desktop-ui --lib tab_emoji -- --test-threads=1
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/screenshot.py target/ui-review-tab-animation.png --no-build --transcript streaming --panels 3
python3 scripts/tab_image_reload_acceptance.py target/tab-image-reload-proof
```

The native acceptance probe runs on private Xvfb with offline fixtures. It lets
the linked UI paint first, then activates three actual cdylib generations and
captures each one. It requires both native emoji poses in every generation and
unique, increasing texture allocations across the linked-to-plugin boundary.
Only the build command inside the isolated child is substituted with a gated
no-op, so the probe tests real loading and rendering of prebuilt binaries, not
compilation. The running user's Ctrl+R path separately verifies rebuild delivery.

## Observed result (2026-09-12)

- Five shared-API tests, ten image-cache tests, and two animation tests passed.
  The font-dependent unit test remains ignored by default. The native probe
  exercised the installed color font and both actual rasterized poses instead.
- `target/tab-image-reload-proof-1789210264289003872/result.json` records eight
  distinct, increasing texture IDs across the linked UI and three loaded UI
  generations. Both poses were allocated in every generation.
- Inspected `target/ui-review-tab-animation.png` and the probe's
  `generation-3.png`: the tab retains the expected emoji after repeated reloads.
- The running release host accepted the Ctrl+R-equivalent `--reload-ui` request
  and activated generation 2 with the new texture range at 10:48 UTC. Existing
  attachments and animated icons received distinct IDs in that same process.
