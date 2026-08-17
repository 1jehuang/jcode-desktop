# GPUI native-window-preserving reload prototype

This prototype keeps GPUI's application, event loop, and native window in a
small executable while replacing only the window's root entity with code from a
reloadable `cdylib`.

## Try it

From this directory:

```sh
cargo build -p hot-reload-ui
cargo run -p hot-reload-host
```

Edit `ui/src/lib.rs`, rebuild with `cargo build -p hot-reload-ui`, then press
**F5** in the running host. The host copies each build to a unique temporary
filename before loading it, so rebuilding never overwrites the mapped library.

An explicit plugin path can be passed as the first host argument or through
`HOT_RELOAD_UI`.

## Safety and failure behavior

- `Window::replace_root` creates the new entity before assigning the root. The
  plugin catches construction panics, so an unsuccessful activation leaves the
  current UI installed.
- Copy, load, symbol, and ABI-version failures happen before root replacement.
- Every ABI-compatible library handle is intentionally leaked until process
  exit, including a library whose activation reports failure. Old entities and
  GPUI callbacks may contain function pointers into old generations, and GPUI's
  internal teardown order is not an API we can rely on for safe unloading.
- The dynamic boundary uses a versioned `repr(C)` function table and opaque
  pointers. The pointed-to GPUI types still use Rust's unstable ABI. Host and UI
  must therefore be built with the same toolchain, feature set, and pinned GPUI
  revision. This is a development-time prototype, not a stable third-party
  plugin ABI.

The counter button intentionally installs an entity listener from the cdylib.
It demonstrates why retaining old library generations is required even after a
new root has been activated.
