# Linux release recovery

## beta.24 incident

The Linux job in release run `33997432311`, job `101390299625`, built and
packaged successfully and passed its X11 launch smoke. The native Wayland smoke
then exited with status 101. Its redirected stdout and stderr were blank because
`src/diagnostics.rs` redirects stderr to
`$HOME/.local/state/jcode-desktop/jcode-desktop.log` before GPUI starts.

The release inputs are immutable and remain unchanged:

- Desktop tag: `desktop-v0.1.0-beta.24`
- Desktop commit: `face07e405eb11f209f39e78f01704fac2dff6f7`
- Jcode runtime commit: `f11adb5996c541592e28519018709eebebc9fce4`
- GPUI commit: `bc538def4545534201bbfcac4e95ac34ea6501b6`

The failed job did not reach `actions/upload-artifact`, so its beta.24 Linux
binary was not recoverable after the job. The local reproducer used the prior
packaged binary below. beta.23 and beta.24 lock the same GPUI commit and execute
the same failing Wayland initialization path.

- Package version: `0.1.0-beta.23`
- Binary path: `dist/linux/Jcode-0.1.0-beta.23-linux-x86_64/jcode-desktop`
- Binary size: `86997248` bytes
- SHA-256: `8bd36f393785313c2f3a9628cd99a28fb81a974f9f13d253057ed430caa4ce63`
- Binary diagnostic version: `0.1.0`

## Root cause

Weston 13's default headless backend creates an output but does not advertise a
`wl_seat`. Waiting longer after the socket appears cannot add that missing
global. GPUI at the pinned commit unconditionally unwraps the seat in
`gpui_linux/src/linux/wayland/client.rs`:

```rust
let seat = seat.unwrap();
```

Running the packaged reproducer against default headless Weston returned 101
and wrote this persistent diagnostic:

```text
thread 'main' panicked at .../gpui_linux/src/linux/wayland/client.rs:776:25:
called `Option::unwrap()` on a `None` value
```

This proves the incident was caused by a missing virtual input seat, not by GPU,
dmabuf, or compositor startup timing.

## Corrected Wayland smoke

Nest Weston in Xvfb with Openbox. The X11 backend creates a virtual seat. Weston
uses Pixman, while the application uses Mesa's software Vulkan implementation.
Probe the registry for `wl_seat` before launching GPUI, and remove `DISPLAY` from
the application environment so the tested application path is native Wayland.

This is the corrected smoke body used by both release workflows:

```bash
set -euo pipefail
app=$(find jcode-desktop/dist/linux -type f -name jcode-desktop | head -1)
smoke="$RUNNER_TEMP/jcode-linux-smoke/wayland"
runtime="$smoke/runtime"
home="$smoke/home"
mkdir -p "$runtime" "$home"
chmod 700 "$runtime"
timeout 30s xvfb-run -a -s '-screen 0 1280x800x24' bash -c '
  set -euo pipefail
  app=$1; runtime=$2; home=$3; smoke=$4
  openbox >"$smoke/openbox.log" 2>&1 &
  openbox_pid=$!
  XDG_RUNTIME_DIR="$runtime" LIBGL_ALWAYS_SOFTWARE=1 \
    weston --backend=x11-backend.so --use-pixman \
      --socket=jcode-ci-wayland --idle-time=0 >"$smoke/weston.log" 2>&1 &
  weston_pid=$!
  trap '\''kill "$weston_pid" "$openbox_pid" 2>/dev/null || true'\'' EXIT
  ready=0
  for _ in {1..100}; do
    if [[ -S "$runtime/jcode-ci-wayland" ]] && \
       XDG_RUNTIME_DIR="$runtime" WAYLAND_DISPLAY=jcode-ci-wayland \
         timeout 2s wayland-info >"$smoke/registry.log" 2>&1 && \
       grep -q wl_seat "$smoke/registry.log"; then
      ready=1
      break
    fi
    kill -0 "$weston_pid" 2>/dev/null || break
    sleep 0.1
  done
  [[ "$ready" == 1 ]]
  env -u DISPLAY XDG_RUNTIME_DIR="$runtime" HOME="$home" \
    WAYLAND_DISPLAY=jcode-ci-wayland LIBGL_ALWAYS_SOFTWARE=1 \
    WGPU_BACKEND=vulkan "$app" >"$smoke/app.log" 2>&1 &
  pid=$!
  sleep 8
  if ! kill -0 "$pid" 2>/dev/null; then
    wait "$pid" || status=$?
    echo "Wayland app exited before smoke completed (status ${status:-0})" >&2
    exit 1
  fi
  kill "$pid"
  wait "$pid" || true
' bash "$app" "$runtime" "$home" "$smoke"
```

Local validation used Weston 15.0.1, Xvfb, Openbox, Pixman, and Mesa software
rendering without niri. `wayland-info` reported:

```text
interface: 'wl_seat', version: 7, name: 16
```

The packaged app remained alive for the full 10-second local probe. Its
persistent log reached normal session initialization with no panic. The exact
revised X11 body also remained alive for its 8-second probe and completed after
the controlled termination.

## Recovery and future-release safeguards

`.github/workflows/linux-release-recovery.yml`:

1. Resolves the dispatched tag to its checked-out commit and specifically checks
   beta.24 against `face07e405eb11f209f39e78f01704fac2dff6f7`.
2. Checks out the sibling runtime at the pinned `f11adb5` commit.
3. Saves the Rust cache even when the job fails.
4. Uploads verified Linux packages as private workflow artifacts before either
   smoke test.
5. Uploads to the existing prerelease only after X11 and native Wayland pass.
6. Always prints and uploads fixture logs, including hidden persistent HOME
   diagnostics via `include-hidden-files: true`.

The canonical cross-platform workflow uses the same corrected fixture and
safeguards. Diagnostic artifact names start with `linux-`, not `desktop-`, so
the publish job's `desktop-*` download pattern cannot ingest logs as release
assets.

Recovery workflow commit: `767794c612739aecd1dd75d1395f63fef745bffc`

Recovery run: <https://github.com/1jehuang/jcode-desktop/actions/runs/34000612721>
