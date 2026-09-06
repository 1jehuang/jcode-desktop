# Public download verification, 2026-09-05

## Restored distribution

The original anonymous request to the private repository's beta23 DMG returned
HTTP 404. The desktop source repository remains private. The new public
`1jehuang/jcode-desktop-releases` repository's git tree contains only `README.md`.
No private source commits or source archives were copied to it.

The website deployment passed in run
[`33997406649`](https://github.com/1jehuang/jcode-website/actions/runs/33997406649).
A real browser loaded `https://jcode.sh/desktop`, resolved the beta23 release
metadata, and displayed direct download buttons for macOS, Linux, and Windows.

The following beta23 files were downloaded in full without credentials from both
the public GitHub origin and their stable `jcode.sh/desktop/releases/` URLs. Every
size and SHA-256 matched the published manifest:

| Artifact | Bytes |
| --- | ---: |
| Linux amd64 DEB | 50,213,520 |
| Linux x86_64 tar.gz | 75,634,781 |
| macOS universal update ZIP | 143,971,807 |
| Windows x86_64 ZIP | 63,514,214 |
| macOS universal DMG | 159,287,825 |
| SHA256SUMS | 198 |
| SHA256SUMS-linux | 208 |
| SHA256SUMS-windows | 106 |
| appcast.xml | 811 |

`/desktop/latest.json` returned HTTP 200, JSON, and `Cache-Control: no-store`.
Its contents matched the local validated nine-asset manifest. `/desktop/appcast.xml`
resolved to the feed containing public website archive URLs.

The migrated beta23 ZIP's Ed25519 signature was independently verified using
OpenSSL and the Sparkle public key extracted from its own `Info.plist`:
`Signature Verified Successfully`. Only the feed's enclosure URL changed.
The archive bytes and signature were preserved. Historical Linux checksum paths
were normalized from build-machine absolute paths to portable basenames.

## Automated checks

The new public publisher tests cover explicit artifact allowlisting, corrupted or
missing packages, private updater URLs, missing signature metadata, incorrect
archive lengths, path injection, symlinks, published-version immutability, failed
anonymous download promotion, numeric release ordering, and credential-free HTTP
requests. The local release test suite passed 26 tests after the verifier-header
fix. Website route tests passed five checks, including beta and stable tags.

Cloudflare rejects its blocked generic `Python-urllib` user-agent with error 1010.
The release verifier now identifies itself as `JcodeReleaseVerifier/1.0`, without
cookies or authorization headers. This client, curl, the real browser, and a
Sparkle user-agent all reached the public endpoints. No site security policy was
weakened.

## New release status

`desktop-v0.1.0-beta.24` was tagged at `face07e`. The macOS workflow
[`33997432277`](https://github.com/1jehuang/jcode-desktop/actions/runs/33997432277)
passed universal compilation, Developer ID signing, notarization, bundle/DMG
installation checks, and signed update generation. The downloaded Mac ZIP and
DMG both passed their SHA-256 checks. Independent verification confirmed build
43 embeds `https://jcode.sh/desktop/appcast.xml`, retains the existing Sparkle
public key, and has a valid Ed25519 archive signature under that key.

The original Linux job compiled and packaged successfully and passed the X11
launch check, then failed the headless Weston Wayland smoke test with exit 101.
The fixture failure was traced to the headless compositor exposing no `wl_seat`,
which GPUI requires. The same packaged binary passed the corrected native
Wayland fixture, nested Weston on a private Xvfb/Openbox display with `DISPLAY`
removed from the app. Recovery run
[`34000612721`](https://github.com/1jehuang/jcode-desktop/actions/runs/34000612721)
passed the recovered build, package verification, X11 and native Wayland smoke
tests, and upload to the private prerelease. Both downloaded Linux packages
passed their SHA-256 checks and the tarball passed the package-content verifier.

Windows job `101390299810` in run `33997432311` passed compilation, package
verification, native launch smoke testing, and artifact upload at 00:49 UTC on
September 6. Its downloaded ZIP also passed the local package-content verifier
and SHA-256 checksum. The parent workflow's Linux failure does not invalidate
these observed Windows acceptance results.

The complete beta24 release was published by run
[`34005643062`](https://github.com/1jehuang/jcode-desktop/actions/runs/34005643062)
at 02:07 UTC on September 6. All nine assets were downloaded in full without
credentials from both the public GitHub origin and stable website URLs, with
matching SHA-256 hashes and lengths:

| Artifact | Bytes |
| --- | ---: |
| Linux amd64 DEB | 58,165,336 |
| Linux x86_64 tar.gz | 82,100,793 |
| macOS universal update ZIP | 146,733,629 |
| Windows x86_64 ZIP | 64,857,341 |
| macOS universal DMG | 162,314,138 |
| SHA256SUMS | 198 |
| SHA256SUMS-linux | 208 |
| SHA256SUMS-windows | 106 |
| appcast.xml | 811 |

An independent request confirmed `/desktop/latest.json` equals the complete
locally validated beta24 manifest and retains `Cache-Control: no-store`. The
current `/desktop/appcast.xml` matches the published feed hash and points to the
exact beta24 public update ZIP. The website now advertises beta24.

Public-artifact macOS installation acceptance passed at 02:08 UTC on September 6
in run [`34005671711`](https://github.com/1jehuang/jcode-desktop/actions/runs/34005671711).
The job downloaded the exact public artifacts anonymously, verified checksums,
validated DMG notarization and Gatekeeper acceptance, installed the app, and
passed installed-bundle verification and launch checks.
Existing Mac installations with the old private-GitHub feed require one manual
installation from the new public DMG to receive future updates through the
public website feed.

The clean-source UI test limitations are recorded separately in
[`release-preflight-beta24.md`](release-preflight-beta24.md). The full UI suite is
not claimed to be green.

## Whole-result acceptance sweep

After publication and public macOS acceptance had completed, a fresh sweep at
02:11 UTC on September 6 reran the checks against the delivered result:

| Requirement or public output | Observed result |
| --- | --- |
| Release tooling regression checks | All 26 Python tests passed again. |
| Current website release metadata | Anonymous `latest.json` exactly matched the locally validated beta24 manifest, with `Cache-Control: no-store`. |
| Downloadable platform packages and supporting files | All nine website URLs were fetched again in full without credentials. Every byte count and SHA-256 matched the manifest. |
| Current macOS updater output | Anonymous appcast hash matched beta24 and its signed enclosure referenced the exact public beta24 ZIP. |
| Public landing page | Anonymous `/desktop` returned HTTP 200. Earlier real-browser button checks remain recorded above. |
| Private source preserved | GitHub reported the source repository private and the distribution repository public. The distribution git tree still contained only `README.md`. |
| Public macOS installation | Rechecked completed run `34005671711`: success, including anonymous download, Gatekeeper/notarization, installation, and launch. This acceptance ran after publication against the delivered artifacts. |

The sweep did not rebuild or alter immutable package bytes. Linux and Windows
native launch acceptance applies to those same hash-verified packages, as
recorded above. The separate pre-existing UI test limitations remain unchanged.
