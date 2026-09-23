# Hot-reload element-arena lifetime

## Cause

The host executable and each UI cdylib statically link GPUI. Consequently, GPUI's
`CURRENT_ELEMENT_ARENA` and fallback `ELEMENT_ARENA` thread locals are distinct in
each library. A host draw selected its per-App arena, but element construction
inside a loaded UI could not see that selection and allocated into the plugin's
fallback arena. End-of-frame cleanup cleared only the host's per-App arena.
Temporary element graphs, owned resources, and destructor records accumulated
on every redraw, even without new transcript content or repeated reloads.

The original release-pair reproduction used a fixed offline streaming fixture
on private Xvfb. Between seconds 2 and 32, the linked UI grew from 238.30 to
239.40 MiB RSS, while the loaded UI grew from 271.09 to 435.12 MiB. Interrupting
the response stopped the growth. Live heaps reached approximately 13 GiB in a
single host. Shared library pages were not the dominant memory consumer.

## Fix

GPUI exposes `ElementArenaContext`, an explicit callback table for reading and
replacing the current thread's arena and accessing its fallback. The host
supplies its canonical table through `HostApi`. Each UI generation installs it
before snapshot restoration or root construction. This includes statically
linked activation, new loaded generations, and rollback to retained generations.

The callbacks access the host's TLS directly, rather than forwarding through
another installed table. Sharing a table again cannot introduce recursion.
There is no long-lived pointer to an individual App. Nested draws, multiple Apps,
and scope unwind restoration continue to use GPUI's original scope-depth and
post-frame cleanup rules. Each scope captures its context for restoration.
Installation applies to the calling thread. Desktop activates and renders GPUI
on its UI thread. Any future additional GPUI thread must install the context too.

Do not clear a plugin arena at the end of `Render::render`: layout and paint can
still reference those elements. Do not unload retained libraries while callbacks
or element destructors can still refer to their code. This fix shares allocation
ownership instead of changing either of those lifetime rules.

## Compatibility and deployment

The host ABI is version 4. The `PluginApi` return layout stays unchanged so old
hosts can reject the version before activation. `HostHandle::new` validates the
stable version/size prefix before reading the extended host table, rejecting old
shorter tables without an out-of-bounds reference. Host and UI must use the same
pinned GPUI source, Rust toolchain, and layout-affecting build configuration.
Both upstream-Git and crates.io GPUI dependencies are patched to the same fork
revision, including the activity-orb dependency.

This is a one-time host upgrade, not a UI-only update. Do not force this plugin
into an already-running ABI-3 host. Preserve drafts and workspace recovery state
and coordinate a normal restart. Host-owned PTYs cannot be transferred through a
UI snapshot, so active terminal resources must be accounted for before restart.
Once the matched host/UI pair is running, Ctrl+R continues to perform normal
state-preserving hot reload. Existing leaked allocations in an old host are not
reclaimed merely by building the new binaries.

## Regression checks

- GPUI unit tests cover joining an already-active foreign host draw, shared
  fallback identity, Drop probes at frame cleanup, same-arena and different-arena
  nested draws, panic unwind, multiple Apps, bounded capacity over repeated
  frames, and thread-local installation/isolation.
- Desktop API tests reject old plugin versions, null/short host tables and
  reinstall the canonical context without recursion.
- `scripts/hot_reload_memory_acceptance.py` copies a prebuilt matched host/UI pair
  into an isolated fixture and exercises real cdylib loading. It measures
  within-generation memory growth after warmup, checks repeated reloads and
  rollback, preserves a typed draft, and saves screenshots and process samples.
  Its `CARGO=/usr/bin/true` is confined to the private fixture so the normal
  host reload transaction uses the copied, prebuilt plugin without rebuilding or
  touching the user's live windows.

Run the release acceptance with:

```sh
python3 scripts/hot_reload_memory_acceptance.py --no-build \
  --host target/release/jcode-desktop \
  --plugin target/release/libjcode_desktop_ui.so \
  --output target/hot-reload-memory
```

Retained library mappings consume some memory per generation by design. The
acceptance test separates this activation cost from continuously growing element
allocations within a generation. It is not a promise of zero memory cost for an
unlimited number of hot reloads.
