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

The initial origin is the public, binaries-only
`1jehuang/jcode-desktop-releases` repository. Its Git history contains only a
distribution README, never private application source. Each release uses the
same version tag as the private build. The `desktop-latest` release holds the
current manifest and appcast. Website routes can later move to an owned server
without changing the URLs embedded in apps.

The website serves installer bytes, checksums, and the appcast directly through
Cloudflare Pages Functions, rather than redirecting download clients to GitHub.
GitHub remains the backend storage origin. This is not yet a separate R2 copy:
an origin-storage migration requires additional Cloudflare storage permissions.
Versioned assets stream without buffering, support HEAD and range requests, and
use immutable caching. The mutable manifest/appcast and failures remain uncached.

`.github/workflows/publish-public-release.yml` runs after a successful tagged
cross-platform build. `JCODE_PUBLIC_RELEASE_TOKEN` is an encrypted Actions secret
in the private repository, used only in the publication step. It needs release
write access to the distribution repository. Prefer a repository-scoped token
when rotating this credential. Private asset downloads use the job's separate
read-only repository token.

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
6. `scripts/publish-public-release.py ASSET_DIRECTORY TAG` uploads the explicit
   allowlist to a draft public release, publishes it, then downloads every asset
   anonymously and verifies its hash before changing the manifest or appcast.
   Published versions cannot be overwritten. Older reruns cannot roll the
   current channel back.
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
