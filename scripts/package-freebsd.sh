#!/usr/bin/env bash
# Build on FreeBSD, or stage existing FreeBSD binaries with SKIP_BUILD=1.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JCODE_REPO="${JCODE_REPO:-$ROOT/../jcode}"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}"
VERSION="${VERSION#desktop-v}"; VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || { echo "error: invalid VERSION: $VERSION" >&2; exit 1; }
TARGET="${TARGET:-x86_64-unknown-freebsd}"
[[ "$TARGET" == x86_64-unknown-freebsd ]] || { echo "error: unsupported FreeBSD TARGET: $TARGET" >&2; exit 1; }
OUT="${OUT_DIR:-$ROOT/dist/freebsd}"
NAME="Jcode-$VERSION-freebsd-x86_64"
for command in tar install python3; do command -v "$command" >/dev/null || { echo "error: missing $command" >&2; exit 1; }; done
[[ -f "$JCODE_REPO/Cargo.toml" ]] || { echo "error: set JCODE_REPO to a Jcode checkout" >&2; exit 1; }

export JCODE_DESKTOP_VERSION="$VERSION"
if [[ "${SKIP_BUILD:-0}" != 1 ]]; then
  [[ "$(uname -s)" == FreeBSD ]] || { echo "error: native FreeBSD build required (or set SKIP_BUILD=1)" >&2; exit 1; }
  cargo build --locked --manifest-path "$ROOT/Cargo.toml" --release --target "$TARGET" --bin jcode-desktop
  cargo build --locked --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$TARGET" --package jcode --bin jcode
  cargo build --locked --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$TARGET" --package jcode-harness-api-server --bin jcode-harness-api-bridge
fi

mkdir -p "$OUT"
WORK="$(mktemp -d "$OUT/.stage.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
STAGE="$WORK/$NAME"
mkdir -p "$STAGE"
install -m755 "$ROOT/target/$TARGET/release/jcode-desktop" "$STAGE/"
install -m755 "$JCODE_REPO/target/$TARGET/release/jcode" "$STAGE/"
install -m755 "$JCODE_REPO/target/$TARGET/release/jcode-harness-api-bridge" "$STAGE/"
install -m644 "$ROOT/packaging/linux/jcode.desktop" "$STAGE/"
install -m644 "$ROOT/assets/app-icon/icon-1024.png" "$STAGE/jcode.png"
tar -C "$WORK" -czf "$OUT/$NAME.tar.gz" "$NAME"
python3 "$ROOT/scripts/verify-release-package.py" --target "$TARGET" "$OUT/$NAME.tar.gz"
# hashlib works on FreeBSD and avoids requiring GNU sha256sum/coreutils.
python3 - "$OUT" "$NAME.tar.gz" <<'PY'
import hashlib
import pathlib
import sys
root, name = pathlib.Path(sys.argv[1]), sys.argv[2]
with (root / name).open("rb") as stream:
    digest = hashlib.file_digest(stream, "sha256").hexdigest()
(root / "SHA256SUMS-freebsd-x86_64").write_text(f"{digest}  {name}\n")
PY
echo "Packaged FreeBSD artifact in $OUT/$NAME.tar.gz"
