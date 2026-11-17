# Public desktop release distribution

The desktop source/build repository and the binaries-only distribution repository
serve different purposes. The source repository is now public, but draft releases
and their staged assets remain unpublished. Never send website visitors or Sparkle
directly to the build repository. Only the validated distribution channel is the
supported download origin.

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
distribution README, never application source. Each release uses the
same version tag as the build repository. The `desktop-latest` release holds the
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
in the build repository, used only in the publication step. It needs release
write access to the distribution repository. Prefer a repository-scoped token
when rotating this credential. Build asset downloads use the job's separate
read-only repository token.

## Release gates

1. Pin the same published Jcode runtime commit in all three native build workflows.
2. Push a new `desktop-v*` tag. Existing tags must not be moved.
3. The macOS workflow builds universal binaries, signs with Developer ID,
   notarizes, tests installation, and generates the signed Sparkle archive.
4. Linux, Windows, and FreeBSD packaging and native launch smoke checks must pass.
   Tagged builds upload verified packages to an unpublished draft release rather
   than depending on Actions artifact storage quota. The cross-platform workflow
   requires the successful macOS workflow for the same tag and commit, validates
   the complete package set, then publishes the build release. Manual branch
   validation still uses Actions artifacts.
5. `scripts/prepare-public-release.py ASSET_DIRECTORY TAG --manifest latest.json`
   validates that all seven architecture/OS targets are represented, checks SHA-256 hashes, and
   rejects appcasts targeting private GitHub URLs. It emits a public asset
   allowlist suitable for publication. Do not mirror the whole repository.
6. `scripts/publish-public-release.py ASSET_DIRECTORY TAG` uploads the explicit
   allowlist to a draft public release, publishes it, then downloads every asset
   anonymously and verifies its hash before changing the manifest or appcast.
   Published versions cannot be overwritten. Older reruns cannot roll the
   current channel back. A later beta cannot replace a promoted stable channel.
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

## Discord release announcements

After public website verification, the publisher dispatches `discord-release.yml`
on trusted `main`. The announcement workflow runs offline guard tests, verifies
public release identity and every versioned download's size and SHA-256 hash,
then posts release notes through the Desktop-specific Discord webhook.

The stable download feed intentionally remains stable when a newer beta ships.
A beta newer than the live stable version is verified against its immutable
public `latest.json` and versioned website downloads, without promoting the
stable channel. Beta announcements link directly to that beta's public binaries
and label the stable download separately. A stable version at least as new as
the requested beta supersedes it, so stale beta announcements are skipped.
Channel changes during verification fail closed. Transient public download
transport errors receive at most three attempts, restarting the size and hash
check each time. Missing files and integrity failures are not retried, and
Discord posts are never automatically retried.

To recover a missing announcement, dispatch `discord-release.yml` with the exact
published tag. A hidden receipt in the source release body prevents duplicate
posts. If a post succeeds but receipt persistence fails, reconcile the receipt
manually before rerunning. Verification progress identifies the failing stage
without logging webhook credentials or transport exception text.

## Local validation

```sh
python3 -m unittest discover -s tests -v
bash -n scripts/package-macos.sh scripts/verify-macos-package.sh
```

Release tags use committed source. Uncommitted development work is not silently
included or committed as part of cutting a release.
