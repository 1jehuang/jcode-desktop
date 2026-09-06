#!/usr/bin/env bash
set -euo pipefail

# Companion for global Super+H/L Firefox bindings. A compositor binding
# consumes the original key even when this helper chooses to do nothing.
# Install at the path already referenced by those bindings, not as a second
# set of bindings. See docs/global-focus-shortcuts.md.
case "${1:-}" in
    previous) key=Page_Up ;;
    next) key=Page_Down ;;
    new) key=t ;;
    close) key=w ;;
    *) echo "usage: $0 <previous|next|new|close>" >&2; exit 2 ;;
esac

app_id="$(niri msg -j focused-window | jq -r '.app_id // ""')"
case "$app_id" in
    firefox|org.mozilla.firefox) ;;
    jcode-desktop)
        # Forward only navigation. Keep the existing Firefox-only new/close
        # behavior unchanged, and never re-emit Super+H/L (which would loop).
        case "$1" in previous|next) ;; *) exit 0 ;; esac
        ;;
    *) exit 0 ;;
esac

# Both apps use Ctrl+PageUp/Down to follow visible tab/panel order. The
# existing Firefox Ctrl+T/Ctrl+W behavior also preserves private browsing.
exec wtype -M ctrl -k "$key" -m ctrl
