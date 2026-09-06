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
is rebuilding the same immutable release source and pinned runtime.

Windows job `101390299810` in run `33997432311` passed compilation, package
verification, native launch smoke testing, and artifact upload at 00:49 UTC on
September 6. Its downloaded ZIP also passed the local package-content verifier
and SHA-256 checksum. The parent workflow's Linux failure does not invalidate
these observed Windows acceptance results.

Beta23 remains the current public release until Linux recovery passes and the
complete beta24 release is published and anonymously verified.

The clean-source UI test limitations are recorded separately in
[`release-preflight-beta24.md`](release-preflight-beta24.md). The full UI suite is
not claimed to be green.
