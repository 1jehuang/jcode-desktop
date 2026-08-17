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
DMG="$OUT/Jcode-macOS-universal.dmg"
ZIP="$OUT/Jcode-macOS-universal.zip"
DMG_ROOT="$OUT/dmg-root"
ENTITLEMENTS="$ROOT/packaging/macos/Jcode.entitlements"
SPARKLE_ROOT="${SPARKLE_ROOT:-$OUT/sparkle}"
UPDATE_FEED_URL="${UPDATE_FEED_URL:-https://github.com/1jehuang/jcode-desktop/releases/download/desktop-updates/appcast.xml}"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
BINS=(jcode-desktop jcode jcode-harness-api-bridge)

# Release tags and beta labels are valid artifact versions, but Apple's bundle
# short version must be exactly three numeric components.
VERSION="${VERSION#desktop-v}"
VERSION="${VERSION#v}"
VERSION="${VERSION%%-*}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "error: VERSION must contain a three-part numeric macOS version" >&2
  exit 1
}
[[ "$BUILD_NUMBER" =~ ^[1-9][0-9]*$ ]] || {
  echo "error: BUILD_NUMBER must be a positive integer" >&2
  exit 1
}

for command in cargo codesign ditto hdiutil iconutil lipo python3 rustup sips xcrun; do
  command -v "$command" >/dev/null || { echo "error: missing required command: $command" >&2; exit 1; }
done
[[ -f "$JCODE_REPO/Cargo.toml" ]] || { echo "error: set JCODE_REPO to a Jcode checkout" >&2; exit 1; }
[[ -f "$ENTITLEMENTS" ]] || { echo "error: missing entitlements: $ENTITLEMENTS" >&2; exit 1; }
if [[ -n "${APPLE_NOTARY_PROFILE:-}" && "${APPLE_SIGNING_IDENTITY:--}" == "-" ]]; then
  echo "error: notarization requires APPLE_SIGNING_IDENTITY" >&2
  exit 1
fi

export MACOSX_DEPLOYMENT_TARGET=13.0
for target in "${TARGETS[@]}"; do rustup target add "$target"; done
"$ROOT/scripts/fetch-sparkle.sh" "$SPARKLE_ROOT"

for target in "${TARGETS[@]}"; do
  cargo build --manifest-path "$ROOT/Cargo.toml" --release --target "$target"
  cargo build --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$target" --bin jcode
  cargo build --manifest-path "$JCODE_REPO/Cargo.toml" --release --target "$target" \
    --package jcode-harness-api-server --bin jcode-harness-api-bridge
done

rm -rf "$APP" "$DMG_ROOT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks" "$OUT/icon.iconset"
PLIST_ARGS=(
  --template "$ROOT/packaging/macos/Info.plist.in"
  --output "$APP/Contents/Info.plist"
  --version "$VERSION"
  --build "$BUILD_NUMBER"
)
if [[ -n "${SPARKLE_PUBLIC_KEY:-}" ]]; then
  PLIST_ARGS+=(--public-key "$SPARKLE_PUBLIC_KEY" --feed-url "$UPDATE_FEED_URL")
fi
if [[ "${REQUIRE_SECURE_UPDATES:-0}" == 1 ]]; then
  PLIST_ARGS+=(--require-updates)
fi
python3 "$ROOT/scripts/render-macos-plist.py" "${PLIST_ARGS[@]}"
ditto "$SPARKLE_ROOT/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
printf 'APPL????' > "$APP/Contents/PkgInfo"

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

# Use the hardened runtime without broad exceptions. The explicit empty
# entitlement set makes accidental privilege additions visible in review and in
# verify-macos-package.sh. Ad-hoc signing remains available for local builds.
IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
SIGN_ARGS=(--force --options runtime --entitlements "$ENTITLEMENTS" --sign "$IDENTITY")
if [[ "$IDENTITY" != "-" ]]; then SIGN_ARGS+=(--timestamp); fi
# Sparkle's helper services need their own entitlements. Preserve those while
# re-signing every nested bundle with the host application's identity, then
# seal the framework. Avoid codesign --deep for signing because it can hide a
# malformed or incompletely signed nested bundle.
NESTED_SIGN_ARGS=(--force --options runtime --preserve-metadata=identifier,entitlements --sign "$IDENTITY")
if [[ "$IDENTITY" != "-" ]]; then NESTED_SIGN_ARGS+=(--timestamp); fi
for nested in \
  "$APP/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices/Downloader.xpc" \
  "$APP/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices/Installer.xpc" \
  "$APP/Contents/Frameworks/Sparkle.framework/Versions/B/Updater.app"; do
  codesign "${NESTED_SIGN_ARGS[@]}" "$nested"
done
codesign "${NESTED_SIGN_ARGS[@]}" "$APP/Contents/Frameworks/Sparkle.framework"
for bin in jcode jcode-harness-api-bridge jcode-desktop; do
  codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/$bin"
done
codesign "${SIGN_ARGS[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# Notarize and staple the application before creating the final archives. The
# temporary ZIP is only the transport accepted by notarytool.
if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  rm -f "$ZIP"
  ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
fi

# A top-level Applications link gives Finder users the expected drag-to-install
# target. Copy the app so the source bundle remains available for local testing.
mkdir -p "$DMG_ROOT"
ditto "$APP" "$DMG_ROOT/Jcode.app"
ln -s /Applications "$DMG_ROOT/Applications"

rm -f "$ZIP" "$DMG"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
hdiutil create -volname "Jcode" -srcfolder "$DMG_ROOT" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$DMG_ROOT"

if [[ "$IDENTITY" != "-" ]]; then
  codesign --force --timestamp --sign "$IDENTITY" "$DMG"
fi
if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  xcrun notarytool submit "$DMG" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
fi

EXPECT_NOTARIZED="$([[ -n "${APPLE_NOTARY_PROFILE:-}" ]] && echo 1 || echo 0)" \
  "$ROOT/scripts/verify-macos-package.sh" "$APP" "$DMG"
(cd "$OUT" && shasum -a 256 "$(basename "$ZIP")" "$(basename "$DMG")" > SHA256SUMS)
echo "macOS beta artifacts: $OUT"
