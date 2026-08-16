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
macOS Gatekeeper flow without a shell workaround. Packaged releases check for
updates daily and use Sparkle's signed background download and install flow.

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
- `SPARKLE_PUBLIC_KEY` and `SPARKLE_PRIVATE_KEY`

Generate the update signing key once on a trusted Mac with Sparkle's
`generate_keys` tool (staged by `scripts/fetch-sparkle.sh`). Store the printed
public key as `SPARKLE_PUBLIC_KEY`, export the private key with
`generate_keys -x private-key-file`, and store that file's contents as
`SPARKLE_PRIVATE_KEY`. Keep an offline backup. Existing installations trust
that public key, so rotating or losing it requires following Sparkle's key
rotation procedure rather than simply generating a replacement.

Manual workflow runs may still produce an ad-hoc signed artifact when every
Apple credential is absent. That artifact is for testing only and is never
published as a tagged release. The package script verifies bundle metadata,
universal executables, hardened-runtime signatures, entitlements, the bundled
CLI, notarization tickets when present, and the DMG's Applications link.

For tagged releases, CI also signs the final notarized ZIP with Sparkle's
Ed25519 key and publishes `appcast.xml` at the stable `desktop-updates` release.
The app embeds only the public key and an HTTPS feed URL. Both the Sparkle
signature and the app's Developer ID signature must validate before an update
can be installed. The pinned Sparkle archive is checksum-verified during
packaging. Local ad-hoc packages omit update metadata and do not check the
production feed.
