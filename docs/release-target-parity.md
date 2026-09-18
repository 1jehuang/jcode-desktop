# Desktop release target parity

The release configuration matches the CLI's seven targets:

| OS | Rust target | Desktop package |
| --- | --- | --- |
| Linux | `x86_64-unknown-linux-gnu` | x86_64 tar.gz, amd64 deb |
| Linux | `aarch64-unknown-linux-gnu` | aarch64 tar.gz, arm64 deb |
| Windows | `x86_64-pc-windows-msvc` | x86_64 zip |
| Windows | `aarch64-pc-windows-msvc` | aarch64 zip |
| macOS | `x86_64-apple-darwin` | universal DMG and ZIP |
| macOS | `aarch64-apple-darwin` | same universal DMG and ZIP |
| FreeBSD | `x86_64-unknown-freebsd` | x86_64 tar.gz, X11 |

All packages bundle Desktop, the CLI, and the harness bridge. The Linux,
Windows, and FreeBSD packagers verify every executable's ELF/PE architecture.
Debian packages also verify the control-file architecture. Separate checksum
and Actions artifact names prevent ARM64/x86_64 overwrites during merging.
New public releases starting with `0.1.0-beta.29` require every package and
checksum. Existing published betas are not retroactively changed.

## Validation on 2026-09-18

- 58 Python release tests passed, including actual tar/deb packaging with
  fixture executables, wrong-architecture rejection, checksums, publication
  gates, historical-release compatibility, and CLI target parity.
- All 16 Linux managed-updater tests passed in the real Desktop crate.
- `actionlint` passed for both modified release workflows. Shell syntax and
  the embedded FreeBSD Python smoke program were checked.
- Cargo dependency checks confirmed the FreeBSD X11 backend is enabled and
  unsupported embedding/tract defaults are disabled for Windows ARM64 only.
- The running desktop rebuilt and activated a new UI generation after the
  updater changes.

These checks do **not** establish native ARM64 or FreeBSD launch support.
[Native build run 35300714274](https://github.com/1jehuang/jcode-desktop/actions/runs/35300714274)
was rejected before any job executed because GitHub reported failed account
payments or an insufficient spending limit. No new native packages were built
or published by this run. Billing settings were not changed.

After the account owner resolves the GitHub Actions billing restriction, run
validation at the latest commit:

```sh
gh workflow run cross-platform-release.yml --ref main -f version=0.1.0-beta.29
```

A dispatch on `main` only builds validation artifacts. It does not publish a
release. Require all native architecture checks and packaged-app smoke jobs to
pass before tagging a release. The existing macOS workflow still builds both
universal slices and enforces signing/notarization for release tags.
