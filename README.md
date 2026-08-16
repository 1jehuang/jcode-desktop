# Jcode Desktop

A native, high-performance spatial desktop client built on the Jcode SDK.

See [PRODUCT.md](PRODUCT.md) for the product vision and requirements.

## macOS beta

Jcode Desktop supports Apple Silicon and Intel Macs running macOS 13 or newer.
The release bundle includes the Jcode CLI and harness bridge, so Finder launches
do not depend on Homebrew, shell startup files, or a separately installed CLI.

Download the latest DMG from the [beta releases][releases], open it, and drag
Jcode onto the Applications link. The beta is distributed as a universal,
Developer ID signed, and notarized app, so its first launch follows the normal
macOS Gatekeeper flow without a shell workaround.

[releases]: https://github.com/1jehuang/jcode-desktop/releases/latest

### Build the app locally

Requirements: Xcode command-line tools, stable Rust, and a sibling checkout of
[`1jehuang/jcode`](https://github.com/1jehuang/jcode). Then run:

```sh
./scripts/package-macos.sh
open dist/macos/Jcode.app
```

Set `JCODE_REPO=/path/to/jcode` when the runtime repository is elsewhere. The
script builds a universal application, signs it ad hoc for local testing, and
creates ZIP and DMG artifacts under `dist/macos/`.

### Publish a beta

Push a `desktop-v*` tag to run `.github/workflows/macos-beta.yml`. Tagged
releases fail closed unless all of these signing and notarization secrets exist:

- `APPLE_CERTIFICATE_P12` and `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`, and `APPLE_API_ISSUER_ID`

Manual workflow runs may still produce an ad-hoc signed artifact when every
Apple credential is absent. That artifact is for testing only and is never
published as a tagged release. The package script verifies bundle metadata,
universal executables, hardened-runtime signatures, entitlements, the bundled
CLI, notarization tickets when present, and the DMG's Applications link.
