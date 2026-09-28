#!/usr/bin/env bash
# Install a bundled applet from applets/<id> into ~/.jcode/applets/<id>.
set -euo pipefail
id="${1:?usage: install-applet.sh <applet-id>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/applets/$id"
[ -f "$src/applet.json" ] || { echo "no applet at $src" >&2; exit 1; }
dest="${JCODE_HOME:-$HOME/.jcode}/applets/$id"
mkdir -p "$dest"
cp -R "$src"/. "$dest"/
echo "Installed $id to $dest"
