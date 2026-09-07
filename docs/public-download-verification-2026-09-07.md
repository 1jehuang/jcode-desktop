# Public desktop download verification, 2026-09-07

## Availability before beta 25

At 00:55 UTC, `https://jcode.sh/desktop/latest.json` advertised
`desktop-v0.1.0-beta.24`. The release was already publicly downloadable.

- All nine manifest assets downloaded anonymously through their stable
  `jcode.sh/desktop/releases/` URLs with HTTP 200.
- The downloads totaled 514,172,560 bytes. Every byte count and SHA-256 matched
  the public manifest.
- The three checksum files independently matched all five application packages:
  Linux DEB and tarball, Windows ZIP, macOS universal DMG and update ZIP.
- Fresh-profile headless Chrome executed the live page JavaScript with Linux,
  macOS, and Windows user agents. All five package links matched verified
  versioned URLs, and the hero selected the appropriate DEB, DMG, or Windows ZIP.
- No visible browser, compositor automation, or credentials were needed.

Raw verification evidence, complete downloaded assets, response headers, and
rendered page DOMs are stored locally under
`/home/jeremy/.jcode/scratch/desktop-beta24-verify-IdUjRd/`.

The prerendered page uses fragment placeholders before JavaScript resolves the
manifest. The tested JavaScript-enabled download flow works. No website change
was required to restore availability.
