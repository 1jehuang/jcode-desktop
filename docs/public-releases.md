# Public desktop release distribution

The desktop source repository is private. GitHub release assets and its Releases
API are therefore not public download endpoints, even for published prereleases.
Never send website visitors or Sparkle directly to that repository.

## Public URL contract

- Download page: `https://jcode.sh/desktop`
- Release metadata: `https://jcode.sh/desktop/latest.json`
- Versioned assets: `https://jcode.sh/desktop/releases/<desktop-vVERSION>/<ASSET>`
- Signed macOS update feed: `https://jcode.sh/desktop/appcast.xml`

These website-owned routes keep installer and update URLs stable independently
of the underlying file server. Only packaged binaries, checksums, the appcast,
and release metadata are published. Source archives are not mirrored.

## Release gates

1. Pin the same published Jcode runtime commit in both build workflows.
2. Push a new `desktop-v*` tag. Existing tags must not be moved.
3. The macOS workflow builds universal binaries, signs with Developer ID,
   notarizes, tests installation, and generates the signed Sparkle archive.
4. Linux and Windows packaging and launch smoke checks must pass. The
   cross-platform workflow waits for macOS before uploading the remaining assets.
5. `scripts/prepare-public-release.py ASSET_DIRECTORY TAG --manifest latest.json`
   validates that all three platforms are complete, checks SHA-256 hashes, and
   rejects appcasts targeting private GitHub URLs. It emits a public asset
   allowlist suitable for publication. Do not mirror the whole repository.
6. Publish versioned assets before changing the current manifest or appcast.
7. Download public artifacts without credentials, verify their checksums, and run
   the `macOS public release acceptance` workflow against the new tag.

The publication validator checks the signature's structure and preserves archive
bytes. Sparkle verifies the Ed25519 signature against the application's embedded
public key before installing an update. Developer ID and notarization remain
required independently of public hosting.

## Existing installations

Apps built before the hosting migration have the private GitHub feed URL embedded
in their signed bundle. Hosting a new feed cannot rewrite that URL in an already
installed app. Those users must download and install the new DMG once from
`https://jcode.sh/desktop`. Subsequent releases use the public update feed.

## Local validation

```sh
python3 -m unittest discover -s tests -v
bash -n scripts/package-macos.sh scripts/verify-macos-package.sh
```

Release tags use committed source. Uncommitted development work is not silently
included or committed as part of cutting a release.
