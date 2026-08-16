#!/usr/bin/env bash
set -euo pipefail

# Keep this version and digest together. Sparkle updates execute privileged
# installation code, so a moving download URL is not acceptable here.
SPARKLE_VERSION=2.9.5
SPARKLE_SHA256=015336b601493e05c237964954bff6191370003d94edefe663724c88840d73cc
SPARKLE_URL="https://github.com/sparkle-project/Sparkle/releases/download/${SPARKLE_VERSION}/Sparkle-${SPARKLE_VERSION}.tar.xz"

if [[ $# -ne 1 ]]; then
  echo "usage: $0 DESTINATION" >&2
  exit 2
fi

DEST="$1"
if [[ -e "$DEST/Sparkle.framework" && -x "$DEST/bin/generate_appcast" ]]; then
  exit 0
fi
if [[ -d "$DEST" && -n "$(find "$DEST" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "error: refusing to replace incomplete Sparkle destination: $DEST" >&2
  exit 1
fi

command -v curl >/dev/null
command -v shasum >/dev/null
command -v tar >/dev/null

mkdir -p "$DEST"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/jcode-sparkle.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
ARCHIVE="$WORK/Sparkle.tar.xz"

curl --fail --location --proto '=https' --tlsv1.2 --retry 3 \
  --output "$ARCHIVE" "$SPARKLE_URL"
ACTUAL_SHA256="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
if [[ "$ACTUAL_SHA256" != "$SPARKLE_SHA256" ]]; then
  echo "error: Sparkle archive checksum mismatch" >&2
  echo "expected: $SPARKLE_SHA256" >&2
  echo "actual:   $ACTUAL_SHA256" >&2
  exit 1
fi

tar -xJf "$ARCHIVE" -C "$WORK"
cp -R "$WORK/Sparkle.framework" "$DEST/Sparkle.framework"
mkdir -p "$DEST/bin"
cp "$WORK/bin/generate_appcast" "$WORK/bin/generate_keys" "$WORK/bin/sign_update" "$DEST/bin/"
chmod 755 "$DEST/bin/"*

printf '%s\n' "$SPARKLE_VERSION" > "$DEST/VERSION"
echo "Sparkle $SPARKLE_VERSION verified and staged at $DEST"
