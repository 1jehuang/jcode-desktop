#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  echo "error: macOS package verification must run on macOS" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${1:-$ROOT/dist/macos/Jcode.app}"
DMG="${2:-$ROOT/dist/macos/Jcode-macOS-universal.dmg}"
EXPECTED_ENTITLEMENTS="$ROOT/packaging/macos/Jcode.entitlements"
BINS=(jcode-desktop jcode jcode-harness-api-bridge)

fail() {
  echo "error: $*" >&2
  exit 1
}

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1"
}

verify_bundle() {
  local app="$1"
  local plist="$app/Contents/Info.plist"

  [[ -d "$app" ]] || fail "app bundle is missing: $app"
  plutil -lint "$plist" >/dev/null
  [[ "$(plist_value "$plist" CFBundleIdentifier)" == "dev.solosystems.jcode.desktop" ]] || fail "unexpected bundle identifier"
  [[ "$(plist_value "$plist" CFBundleExecutable)" == "jcode-desktop" ]] || fail "unexpected bundle executable"
  [[ "$(plist_value "$plist" CFBundlePackageType)" == "APPL" ]] || fail "unexpected bundle package type"
  [[ "$(plist_value "$plist" LSMinimumSystemVersion)" == "13.0" ]] || fail "unexpected deployment target"
  [[ "$(plist_value "$plist" LSMultipleInstancesProhibited)" == "true" ]] || fail "multiple app instances are not prohibited"
  [[ "$(plist_value "$plist" CFBundleShortVersionString)" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "invalid short version"
  [[ "$(plist_value "$plist" CFBundleVersion)" =~ ^[1-9][0-9]*$ ]] || fail "invalid build number"
  [[ "$(cat "$app/Contents/PkgInfo")" == "APPL????" ]] || fail "invalid PkgInfo"
  [[ -s "$app/Contents/Resources/Jcode.icns" ]] || fail "app icon is missing"

  local sparkle="$app/Contents/Frameworks/Sparkle.framework"
  local sparkle_executable="$sparkle/Versions/B/Sparkle"
  [[ -x "$sparkle_executable" ]] || fail "Sparkle framework is missing"
  lipo "$sparkle_executable" -verify_arch arm64 x86_64 || fail "Sparkle is not universal"
  codesign --verify --deep --strict --verbose=2 "$sparkle"

  local update_key=""
  update_key="$(plist_value "$plist" SUPublicEDKey 2>/dev/null || true)"
  if [[ -n "$update_key" ]]; then
    [[ "$(plist_value "$plist" SUFeedURL)" == \
      "https://github.com/1jehuang/jcode-desktop/releases/download/desktop-updates/appcast.xml" ]] || \
      fail "unexpected automatic update feed"
    [[ "$(plist_value "$plist" SUEnableAutomaticChecks)" == "true" ]] || fail "automatic checks are disabled"
    [[ "$(plist_value "$plist" SUAutomaticallyUpdate)" == "true" ]] || fail "automatic updates are disabled"
    python3 - "$update_key" <<'PY'
import base64
import binascii
import sys

try:
    key = base64.b64decode(sys.argv[1], validate=True)
except binascii.Error as error:
    raise SystemExit(f"invalid Sparkle public key: {error}")
if len(key) != 32:
    raise SystemExit("Sparkle public key is not a 32-byte Ed25519 key")
PY
  elif [[ "${REQUIRE_SECURE_UPDATES:-0}" == 1 ]]; then
    fail "secure automatic update metadata is missing"
  fi

  for bin in "${BINS[@]}"; do
    local executable="$app/Contents/MacOS/$bin"
    [[ -x "$executable" ]] || fail "missing executable companion: $bin"
    lipo "$executable" -verify_arch arm64 x86_64 || fail "$bin is not universal"
    codesign --verify --strict --verbose=2 "$executable"

    local actual_entitlements
    actual_entitlements="$(mktemp)"
    codesign -d --entitlements :- "$executable" >"$actual_entitlements" 2>/dev/null || fail "could not read $bin entitlements"
    python3 - "$EXPECTED_ENTITLEMENTS" "$actual_entitlements" <<'PY'
import plistlib
import sys

with open(sys.argv[1], "rb") as expected_file:
    expected = plistlib.load(expected_file)
with open(sys.argv[2], "rb") as actual_file:
    actual = plistlib.load(actual_file)
if actual != expected:
    raise SystemExit(f"unexpected entitlements: {actual!r}")
PY
    rm -f "$actual_entitlements"
  done

  codesign --verify --deep --strict --verbose=2 "$app"
}

for command in codesign ditto hdiutil lipo open pgrep plutil python3; do
  command -v "$command" >/dev/null || fail "missing required command: $command"
done
[[ -f "$EXPECTED_ENTITLEMENTS" ]] || fail "expected entitlements are missing"
[[ -f "$DMG" ]] || fail "DMG is missing: $DMG"

verify_bundle "$APP"
hdiutil verify "$DMG" >/dev/null

MOUNT="$(mktemp -d)"
SCRATCH="$(mktemp -d)"
APP_PID=""
cleanup() {
  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
  fi
  hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1 || true
  rm -rf "$MOUNT" "$SCRATCH"
}
trap cleanup EXIT

hdiutil attach "$DMG" -nobrowse -readonly -mountpoint "$MOUNT" >/dev/null
[[ -L "$MOUNT/Applications" ]] || fail "DMG is missing its Applications link"
[[ "$(readlink "$MOUNT/Applications")" == "/Applications" ]] || fail "DMG Applications link has the wrong target"
verify_bundle "$MOUNT/Jcode.app"

# Finder launches do not inherit an interactive shell PATH. Running the bundled
# CLI in an empty environment proves that the app ships a usable companion and
# does not accidentally resolve a Homebrew or developer checkout executable.
env -i HOME="$SCRATCH" TMPDIR="$SCRATCH" PATH=/usr/bin:/bin \
  "$MOUNT/Jcode.app/Contents/MacOS/jcode" --version >/dev/null

if [[ "${FIRST_LAUNCH_CHECK:-0}" == 1 ]]; then
  # Exercise the actual Finder/LaunchServices path after a drag-style install,
  # not only the bundle's executable. A healthy first launch must create the
  # desktop process and keep it alive long enough to initialize its first
  # window and bundled runtime discovery.
  INSTALLED_APP="$SCRATCH/Applications/Jcode.app"
  mkdir -p "$(dirname "$INSTALLED_APP")"
  ditto "$MOUNT/Jcode.app" "$INSTALLED_APP"
  codesign --verify --deep --strict --verbose=2 "$INSTALLED_APP"

  open -n "$INSTALLED_APP"
  for _ in {1..40}; do
    APP_PID="$(pgrep -f "$INSTALLED_APP/Contents/MacOS/jcode-desktop" | head -1 || true)"
    [[ -n "$APP_PID" ]] && break
    sleep 0.25
  done
  [[ -n "$APP_PID" ]] || fail "LaunchServices did not start the installed app"
  sleep 3
  kill -0 "$APP_PID" 2>/dev/null || fail "the installed app exited during first launch"
  kill "$APP_PID"
  APP_PID=""
fi

if [[ "${EXPECT_NOTARIZED:-0}" == 1 ]]; then
  xcrun stapler validate "$APP"
  xcrun stapler validate "$DMG"
  spctl --assess --type execute --verbose=2 "$APP"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
fi

echo "verified macOS app bundle, bundled CLI bridge layout, and drag-to-Applications DMG"
