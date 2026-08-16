# Jcode Desktop

A native, high-performance spatial desktop client built on the Jcode SDK.

See [PRODUCT.md](PRODUCT.md) for the product vision and requirements.

## macOS beta

Jcode Desktop supports Apple Silicon and Intel Macs running macOS 13 or newer.
The release bundle includes the Jcode CLI and harness bridge, so Finder launches
do not depend on Homebrew, shell startup files, or a separately installed CLI.

Download the latest DMG from the [beta releases][releases], open it, and drag
Jcode to Applications. The beta is distributed as a universal app.

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

Push a `desktop-v*` tag to run `.github/workflows/macos-beta.yml`. Fully trusted
Gatekeeper distribution requires these repository secrets:

- `APPLE_CERTIFICATE_P12` and `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`, and `APPLE_API_ISSUER_ID`

The workflow still produces an ad-hoc signed test artifact when the Apple
Developer credentials are absent, but that artifact is not notarized.
