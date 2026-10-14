# Hosted Linux release compilation validation

## Scope and safety

`LINUX_BUILD_BACKEND=hosted scripts/package-linux.sh` opts into justrust hosted
compilation of `jcode-desktop`, `jcode`, and `jcode-harness-api-bridge`. It currently
accepts only native x86_64 Linux targeting `x86_64-unknown-linux-gnu`. Without the
opt-in, the existing local build and `SKIP_BUILD=1` packaging route is unchanged.
Hosted compilation rejects `SKIP_BUILD=1`, dirty source checkouts, missing
lockfiles, missing artifacts, compiler failure, and evidence of local fallback.

Only compilation moves to hosted. Tar/deb assembly, binary architecture checks,
package verification, checksums, X11 launch, and native Wayland launch remain on
the Ubuntu 24.04 runner. No automatic release or public publication workflow is
changed. The new `linux-hosted-validation.yml` is manual, main-only, read-only,
and uploads validation packages only after both native smoke gates pass.

## Required client and compiler

Use a reviewed immutable justrust commit supporting native explicit GNU Linux
targets, release artifact retrieval (including warm-cache outputs), the Desktop
and CLI build metadata allowlist, `JUSTRUST_REMOTE_REQUIRED=1`, and
`JUSTRUST_TOOLCHAIN_REQUIRED=1`. The workflow requires the full justrust revision
as an input and builds that client with `cargo install --locked`. Its reviewed
default is `c3e8cdb43cee1896e0f50966ef0e7bb811de314b`, including strict compiler
fallback guards for ordinary, nested, and unrecorded execution. It never
installs a floating latest client. All third-party actions are pinned by SHA.

`RUSTUP_TOOLCHAIN` must be an exact stable Rust release, not `stable`, `nightly`,
or the default justrust fork. The helper sets `JUSTRUST_TOOLCHAIN` to that exact
version and requires the justrust official compiler installer to succeed. This
is important because the hosted machine may have standalone Cargo without
rustup. Both remote routing and compiler installation must fail closed, never
silently use local/system Cargo. Each recorded build must report
`remote.machine == "hosted"`, `toolchain == requested_version`, success, a current
start time, and matching locked release target/binary arguments before packaging.
Reports are saved under `target/hosted-release-evidence`.

## Credentials and manual workflow

A maintainer must configure the separate `JCODE_API_KEY` Actions secret using an
account authorized for hosted builds with available credits. The secret is
currently absent from the Desktop repository, so CI validation deliberately
fails with a clear message before checkout/bootstrap. Never reuse a public
release token, print the API key, add credentials to source files, or expose the
secret to pull requests. Dispatch only reviewed `main` code with reviewed client
and runtime SHA inputs. Do not dispatch public release workflows as a test.

The default runtime revision is
`af0dff74f2342245df43d2aa2ba6620a75669414`, which adds metadata-file rerun tracking.
The exact Rust version input must be supported by both projects and available
through the justrust official installer. Missing compiler components must fail
validation, not select another compiler.

For a local trial with clean, reviewed sibling checkouts and an already
configured hosted login (or an API key supplied privately through the environment):

```bash
RUSTUP_TOOLCHAIN=<exact-major.minor.patch> \
LINUX_BUILD_BACKEND=hosted \
TARGET=x86_64-unknown-linux-gnu \
VERSION=<package-version> \
JCODE_REPO=/path/to/jcode \
scripts/package-linux.sh
```

The helper requires `justrust`, `git`, Python 3.11+, `zstd`, `rsync`, and `ssh`.
The package script retains its local tar/dpkg/install/checksum dependencies.
A local client can use stored hosted authentication. CI independently requires
its explicit secret. Hosted builds consume the account's ordinary build credits.

## Credential-free provenance

Hosted source sync excludes `.git`, which can contain credentials. Before
uploading, the helper requires clean Desktop and runtime repositories and writes
nonignored `.jcode-release-metadata` source snapshots, cleaning up only the
snapshots it created on exit. An existing snapshot is an error, not overwritten.

Desktop's snapshot contains the full source hash, clean status, eight recent
subjects, and the existing structured first-parent/version-tag update history.
`JCODE_DESKTOP_BUILD_METADATA=1` makes the build script validate this snapshot,
preserve exact version/hash/history, and watch its files for rebuilds. Ordinary
builds without the opt-in retain their existing Git-derived development metadata.
`SOURCE_DATE_EPOCH` defaults to the Desktop commit time.

The CLI's existing metadata parser reads its hash/date/tag/status/changelog from
`.jcode-release-metadata/build.txt`. The environment path is deliberately
`../../.jcode-release-metadata/build.txt`, relative to Cargo's build-script cwd
`crates/jcode-build-meta`, not the workspace root. CLI package version is supplied
as `JCODE_BUILD_SEMVER` and `JCODE_RELEASE_BUILD=1`. Neither snapshot includes Git
configuration, remotes, credentials, or the `.git` directory.

## Acceptance before enabling production

Focused Python tests use synthetic ELF compiler fixtures but run genuine local
archive/deb verification on Ubuntu. They prove orchestration and rejection gates,
not that the actual Desktop compiles or launches. The dependency-free metadata
tests cover ordinary Git builds, archive fallback, and valid/invalid snapshots.

The hosted fleet and Actions runner both use Ubuntu 24.04, with a glibc 2.39
baseline. Nevertheless, require actual packaged artifact dependency checks and
native X11/Wayland launch evidence on that runner, not a distro assumption.
Before considering automatic hosted release compilation, record a real
three-binary hosted build with the exact compiler, validate embedded Desktop and
CLI version/hash/history, inspect artifact dependencies, verify both archives and
checksums, and pass both native launch gates. A successful tiny compiler fixture
or merely declaring remote mode is insufficient. Missing credentials or any
unproven gate keeps production release compilation on the existing local path.
