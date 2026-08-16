#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  echo "error: macOS packaging must run on macOS" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JCODE_REPO="${JCODE_REPO:-$ROOT/../jcode}"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}"
BUILD_NUMBER="${BUILD_NUMBER:-${GITHUB_RUN_NUMBER:-1}}"
OUT="${OUT_DIR:-$ROOT/dist/macos}"
APP="$OUT/Jcode.app"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
BINS=(jcode-desktop jcode jcode-harness-api-bridge)

command -v cargo >/dev/null
command -v lipo >/dev/null
command -v iconutil >/dev/null
[[ -f "$JCODE_REPO/Cargo.toml" ]] || { echo "error: set JCODE_REPO to a Jcode checkout" >&2; exit 1; }
for target in "${TARGETS[@]}"; do rustup target add "$target"; done

for target in "${TARGETS[@]}"; do
  cargo build --manifest-path "$ROOT/Cargo.toml" --release --target "$target"
  cargo build --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$target" --bin jcode
  cargo build --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$target" \
    --package jcode-harness-api-server --bin jcode-harness-api-bridge
 done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$OUT/icon.iconset"
sed -e "s/__VERSION__/$VERSION/g" -e "s/__BUILD__/$BUILD_NUMBER/g" \
  "$ROOT/packaging/macos/Info.plist.in" > "$APP/Contents/Info.plist"

for bin in "${BINS[@]}"; do
  if [[ "$bin" == jcode-desktop ]]; then base="$ROOT/target"; else base="$JCODE_REPO/target"; fi
  lipo -create \
    "$base/aarch64-apple-darwin/release/$bin" \
    "$base/x86_64-apple-darwin/release/$bin" \
    -output "$APP/Contents/MacOS/$bin"
  chmod 755 "$APP/Contents/MacOS/$bin"
done

for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$ROOT/assets/app-icon/icon-1024.png" --out "$OUT/icon.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$ROOT/assets/app-icon/icon-1024.png" --out "$OUT/icon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$OUT/icon.iconset" -o "$APP/Contents/Resources/Jcode.icns"
rm -rf "$OUT/icon.iconset"

# Sign inner executables before the bundle. Without release credentials this
# creates an ad-hoc signed artifact suitable for local beta testing.
IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
SIGN_ARGS=(--force --options runtime --sign "$IDENTITY")
if [[ "$IDENTITY" != "-" ]]; then SIGN_ARGS+=(--timestamp); fi
for bin in jcode jcode-harness-api-bridge jcode-desktop; do
  codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/$bin"
done
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

rm -f "$OUT/Jcode-macOS-universal.zip" "$OUT/Jcode-macOS-universal.dmg"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/Jcode-macOS-universal.zip"
hdiutil create -volname "Jcode" -srcfolder "$APP" -ov -format UDZO "$OUT/Jcode-macOS-universal.dmg" >/dev/null

if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  xcrun notarytool submit "$OUT/Jcode-macOS-universal.zip" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/Jcode-macOS-universal.zip"
  hdiutil create -volname "Jcode" -srcfolder "$APP" -ov -format UDZO "$OUT/Jcode-macOS-universal.dmg" >/dev/null
fi

shasum -a 256 "$OUT/Jcode-macOS-universal.zip" "$OUT/Jcode-macOS-universal.dmg" > "$OUT/SHA256SUMS"
echo "macOS beta artifacts: $OUT"
