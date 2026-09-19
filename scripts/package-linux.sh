#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JCODE_REPO="${JCODE_REPO:-$ROOT/../jcode}"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}"
VERSION="${VERSION#desktop-v}"; VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || { echo "error: invalid VERSION: $VERSION" >&2; exit 1; }
TARGET="${TARGET:-$(uname -m)-unknown-linux-gnu}"
case "$TARGET" in
  x86_64-unknown-linux-gnu) ARCH=x86_64; DEB_ARCH=amd64; CHECKSUMS=SHA256SUMS-linux ;;
  aarch64-unknown-linux-gnu) ARCH=aarch64; DEB_ARCH=arm64; CHECKSUMS=SHA256SUMS-linux-aarch64 ;;
  *) echo "error: unsupported Linux TARGET: $TARGET" >&2; exit 1 ;;
esac
OUT="${OUT_DIR:-$ROOT/dist/linux}"
STAGE="$OUT/Jcode-$VERSION-linux-$ARCH"
DEBROOT="$OUT/deb-root"
for command in cargo tar dpkg-deb install python3 sha256sum; do command -v "$command" >/dev/null || { echo "error: missing $command" >&2; exit 1; }; done
[[ -f "$JCODE_REPO/Cargo.toml" ]] || { echo "error: set JCODE_REPO to a Jcode checkout" >&2; exit 1; }

export JCODE_DESKTOP_VERSION="$VERSION"
if [[ "${SKIP_BUILD:-0}" != 1 ]]; then
  cargo build --manifest-path "$ROOT/Cargo.toml" --release --target "$TARGET" --bin jcode-desktop
  cargo build --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$TARGET" --bin jcode
  cargo build --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$TARGET" --package jcode-harness-api-server --bin jcode-harness-api-bridge
fi

rm -rf "$STAGE" "$DEBROOT"; mkdir -p "$STAGE"
install -m755 "$ROOT/target/$TARGET/release/jcode-desktop" "$STAGE/"
install -m755 "$JCODE_REPO/target/$TARGET/release/jcode" "$STAGE/"
install -m755 "$JCODE_REPO/target/$TARGET/release/jcode-harness-api-bridge" "$STAGE/"
install -m644 "$ROOT/packaging/linux/jcode.desktop" "$STAGE/"
install -m644 "$ROOT/assets/app-icon/icon-1024.png" "$STAGE/jcode.png"
tar -C "$OUT" -czf "$OUT/Jcode-$VERSION-linux-$ARCH.tar.gz" "$(basename "$STAGE")"

mkdir -p "$DEBROOT/DEBIAN" "$DEBROOT/usr/bin" "$DEBROOT/usr/share/applications" "$DEBROOT/usr/share/icons/hicolor/1024x1024/apps"
install -m755 "$STAGE"/{jcode-desktop,jcode,jcode-harness-api-bridge} "$DEBROOT/usr/bin/"
install -m644 "$ROOT/packaging/linux/jcode.desktop" "$DEBROOT/usr/share/applications/jcode.desktop"
install -m644 "$ROOT/assets/app-icon/icon-1024.png" "$DEBROOT/usr/share/icons/hicolor/1024x1024/apps/jcode.png"
cat > "$DEBROOT/DEBIAN/control" <<EOF
Package: jcode-desktop
Version: ${VERSION//-/.}
Architecture: $DEB_ARCH
Maintainer: Jcode <support@jcode.sh>
Depends: libxkbcommon0, libfontconfig1, poppler-utils
Description: Native desktop client for Jcode
 Includes the Jcode CLI and harness bridge.
EOF
dpkg-deb --root-owner-group --build "$DEBROOT" "$OUT/Jcode-$VERSION-linux-$DEB_ARCH.deb"
python3 "$ROOT/scripts/verify-release-package.py" --target "$TARGET" "$OUT/Jcode-$VERSION-linux-$ARCH.tar.gz"
python3 "$ROOT/scripts/verify-release-package.py" --target "$TARGET" "$OUT/Jcode-$VERSION-linux-$DEB_ARCH.deb"
(
  cd "$OUT"
  sha256sum "Jcode-$VERSION-linux-$ARCH.tar.gz" "Jcode-$VERSION-linux-$DEB_ARCH.deb" > "$CHECKSUMS"
)
echo "Packaged Linux artifacts in $OUT"
