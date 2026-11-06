#!/usr/bin/env bash
# Sourced by macOS packaging scripts. Only recognized transport failures retry.
# Five total attempts, with 5/10/20/40 second backoff. Unknown errors fail closed.
apple_network_retry() {
  local attempt=1 delay=5 status decision output errors
  output="$(mktemp "${TMPDIR:-/tmp}/jcode-apple-output.XXXXXX")" || return $?
  errors="$(mktemp "${TMPDIR:-/tmp}/jcode-apple-errors.XXXXXX")" || {
    status=$?
    rm -f "$output"
    return "$status"
  }
  while :; do
    if "$@" >"$output" 2>"$errors"; then status=0; else status=$?; fi
    # Keep only the final attempt on stdout so notarytool JSON stays parseable.
    cat "$errors" >&2
    if decision="$(python3 - "$output" "$errors" <<'PY'
import json
import re
import sys

out, err = (open(p, encoding="utf-8", errors="replace").read() for p in sys.argv[1:])
text = out + "\n" + err
try:
    result = json.loads(out)
except (ValueError, TypeError):
    result = {}
# Security failures take precedence even if a transport error is also printed.
rejected = isinstance(result, dict) and str(result.get("status", "")).lower() in {"invalid", "rejected"}
permanent = rejected or re.search(
    r"\b(rejected|invalid signature|signature is invalid|not signed|invalid credentials|"
    r"authentication failed|unauthorized|forbidden)\b", text, re.I)
# URL-loading transport errors only, never a bare stapler exit code (e.g. 65).
network = re.search(
    r"(?:NSURLErrorDomain[^\n]*?(?:Code\s*=\s*|error\s+)|"
    r"(?:NSURLError|URLSession)[^\n]*?)"
    r"-(?:1001|1003|1004|1005|1006|1009)\b", text, re.I)
cloudkit = re.search(
    r"\b(?:CKErrorDomain|CloudKit)\b[^\n]*?\b(?:NetworkUnavailable|NetworkFailure|"
    r"network (?:unavailable|failure)|(?:Code\s*=\s*)(?:3|4)\b)", text, re.I)
print("permanent" if permanent else "transient" if network or cloudkit else "unknown")
PY
)"; then :; else
      status=$?
      rm -f "$output" "$errors"
      return "$status"
    fi
    if [[ "$decision" == permanent ]]; then
      [[ "$status" != 0 ]] || status=1
      break
    fi
    [[ "$status" != 0 ]] || break
    # Interrupted commands are not retried, regardless of their output.
    if [[ "$decision" != transient || "$attempt" -ge 5 || "$status" -ge 128 ]]; then break; fi
    cat "$output" >&2
    echo "Apple network failure (exit $status), retry $((attempt + 1))/5 in ${delay}s: $*" >&2
    if sleep "$delay"; then :; else
      status=$?
      break
    fi
    attempt=$((attempt + 1))
    delay=$((delay * 2))
  done
  cat "$output"
  rm -f "$output" "$errors"
  return "$status"
}
