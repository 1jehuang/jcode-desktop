#!/usr/bin/env bash
# Compile only. Packaging and native smoke gates remain on the local runner.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JCODE_REPO="$(cd "${1:-$ROOT/../jcode}" && pwd)"
TARGET="${TARGET:-x86_64-unknown-linux-gnu}"
[[ "$TARGET" == x86_64-unknown-linux-gnu && "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
  echo 'error: hosted release compilation requires native x86_64 Linux' >&2; exit 1;
}
[[ "${RUSTUP_TOOLCHAIN:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo 'error: set RUSTUP_TOOLCHAIN to an exact stable release (major.minor.patch)' >&2; exit 1;
}
[[ "${JCODE_DESKTOP_VERSION:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || {
  echo 'error: set JCODE_DESKTOP_VERSION to the package version' >&2; exit 1;
}
for command in justrust git python3 zstd rsync ssh; do
  command -v "$command" >/dev/null || { echo "error: missing $command" >&2; exit 1; }
done
# These sources are uploaded, so require clean, reviewed checkouts. Never sync
# .git: it can contain credentials. Snapshot only the public build inputs.
for repo in "$ROOT" "$JCODE_REPO"; do
  [[ -f "$repo/Cargo.lock" ]] || { echo "error: missing lockfile in $repo" >&2; exit 1; }
  [[ ! -e "$repo/.jcode-release-metadata" ]] || { echo "error: existing metadata snapshot in $repo" >&2; exit 1; }
  [[ -z "$(git -C "$repo" status --porcelain --untracked-files=normal)" ]] || {
    echo "error: hosted release validation requires a clean checkout: $repo" >&2; exit 1;
  }
done
created=()
cleanup() { for directory in "${created[@]}"; do rm -rf -- "$directory"; done; }
trap cleanup EXIT
mkdir "$ROOT/.jcode-release-metadata"
created+=("$ROOT/.jcode-release-metadata")
git -C "$ROOT" rev-parse HEAD > "$ROOT/.jcode-release-metadata/hash.txt"
: > "$ROOT/.jcode-release-metadata/status.txt"
git -C "$ROOT" log -8 --format=%s > "$ROOT/.jcode-release-metadata/subjects.txt"
TZ=UTC git -C "$ROOT" log --first-parent --decorate=short --decorate-refs='refs/tags/*' -120 \
  '--date=format-local:%Y-%m-%d %H:%M UTC' '--format=%H%x1e%D%x1e%cd%x1e%s%x1f' \
  > "$ROOT/.jcode-release-metadata/history.txt"
mkdir "$JCODE_REPO/.jcode-release-metadata"
created+=("$JCODE_REPO/.jcode-release-metadata")
{
  printf 'git_hash=%s\n' "$(git -C "$JCODE_REPO" rev-parse --short HEAD)"
  printf 'git_date=%s\n' "$(git -C "$JCODE_REPO" log -1 --format=%ci)"
  printf 'git_dirty=false\ngit_tag=%s\n' "$(git -C "$JCODE_REPO" describe --tags --always)"
  printf 'changelog_raw<<JCODE_RELEASE_HISTORY_END\n'
  git -C "$JCODE_REPO" log -700 '--format=%h|%ct|%D|%s'
  printf 'JCODE_RELEASE_HISTORY_END\n'
} > "$JCODE_REPO/.jcode-release-metadata/build.txt"
export JUSTRUST_REMOTE_REQUIRED=1 JUSTRUST_TOOLCHAIN_REQUIRED=1
export JUSTRUST_TOOLCHAIN="$RUSTUP_TOOLCHAIN"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct)}"
export JCODE_DESKTOP_BUILD_METADATA=1
EVIDENCE="${HOSTED_EVIDENCE_DIR:-$ROOT/target/hosted-release-evidence}"
mkdir -p "$EVIDENCE"
EVIDENCE="$(cd "$EVIDENCE" && pwd)"
justrust remote use hosted
build_hosted() {
  local repo="$1" binary="$2"
  shift 2
  (
    cd "$repo"
    local started_after
    started_after="$(date +%s)"
    justrust build --remote --release --locked --target "$TARGET" --bin "$binary" "$@"
    justrust show --json > "$EVIDENCE/$binary.json"
    python3 "$ROOT/scripts/verify-hosted-build.py" "$EVIDENCE/$binary.json" --binary "$binary" --target "$TARGET" --started-after "$started_after" --rust-version "$RUSTUP_TOOLCHAIN"
    [[ -s "target/$TARGET/release/$binary" && -x "target/$TARGET/release/$binary" ]] || {
      echo "error: hosted artifact missing: $binary" >&2; exit 1;
    }
  )
}
build_hosted "$ROOT" jcode-desktop
# Cargo runs the metadata build script from crates/jcode-build-meta on the host.
export JCODE_BUILD_METADATA_FILE=../../.jcode-release-metadata/build.txt
JCODE_BUILD_SEMVER="$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["package"]["version"])' "$JCODE_REPO/Cargo.toml")"
export JCODE_BUILD_SEMVER
export JCODE_RELEASE_BUILD=1
build_hosted "$JCODE_REPO" jcode
build_hosted "$JCODE_REPO" jcode-harness-api-bridge --package jcode-harness-api-server
