# Live profiler task retention investigation, 2026-09-07

## Status

**Unresolved in the live process. No runtime fix was applied.** The reported live
capture contained 920 samples in 20 seconds for one window, versus roughly 200
from one 100 ms sampler. However, fresh isolated processes successfully loaded
three actual cdylib generations without reproducing duplicate sampling. Do not
report the GPUI arena hypothesis below as a confirmed root cause or validated fix.

## Ownership trace

- `Workspace` owns `_live_profile_task`, `_bridge_task`, `_housekeeping_task`, and
  `_performance_task`. Dropping the workspace should cancel these tasks.
- `live_profile::spawn` takes an `AnyWindowHandle`, not an entity handle. Its loop
  survives root replacement as long as its owning task survives. Closing the
  window only terminates this loop when an active request reaches window update.
- GPUI `Context::spawn` supplies a **weak** entity. Workspace bridge and
  housekeeping tasks therefore do not directly keep their workspace alive.
- `lib.rs::activate` calls `Window::replace_root`. In pinned GPUI
  `bc538def4545534201bbfcac4e95ac34ea6501b6`, that creates the replacement and assigns
  it to `self.root`, dropping the old root reference, then requests refresh.
- `ReloadManager` intentionally retains libraries because old callbacks can still
  contain code pointers. It does **not** retain an `Entity<Workspace>` or old root.
  Failed activation restores a fresh root from the previous snapshot.
- Recovery's detached loop holds only a weak workspace. Its generation-tagged
  writer exits after a newer writer takes over. The detached quit callback holds
  a writer, not the workspace. Recovery does not explain persistent old roots.
- Render callbacks can hold strong entities. In particular the sidebar virtual
  list renderer captures `cx.entity()`, and the strip's mouse routing captures a
  workspace until its frame listener is released. A panel transcript list also
  captures its panel. Retaining render allocation payloads could retain these.

## Arena hypothesis and its limits

GPUI has `CURRENT_ELEMENT_ARENA` and fallback `ELEMENT_ARENA` thread locals.
`Window::draw` scopes the App-owned arena and its returned `ArenaClearNeeded`
clears that arena. `AnyElement::new` uses the current library's thread local.
`ArenaBox` does not drop its payload: `Arena::clear` drops allocations.

`nm -a` on the actual host and cdylib shows separate symbols for both TLS slots,
with local TLS symbols in the cdylib. This suggests a host draw could leave a
plugin callback allocating in the plugin fallback arena. All these APIs and
`App::element_arena` are crate-private; there is no existing public arena-binding
API for a desktop-only repair.

This is **structural evidence, not proof of the live retention chain**. The actual
cdylib tests below do not reproduce excess sampler tasks, even with sidebar
history enabled. Also GPUI consumes view handles during layout and moves some
callbacks into frame listeners, so an allocation being retained does not imply
its original entity reference is retained. RSS growth also includes intentionally
retained mapped libraries and cannot independently establish leaked workspaces.

If the hypothesis is confirmed with allocation/drop instrumentation, the proposed
runtime repair is to scope the shared App-owned arena at library callback entry:
`any_view::render<V>` and generic `Drawable<E>` ElementObject layout, prepaint,
paint, and root-layout entry points. Each callback must restore its library's
previous TLS value, without clearing live allocations. The outer `Window::draw`
must remain the frame cleanup owner, preserving nested draw and panic safety.
A same-arena fast path would avoid repeated TLS work for every element. This is a
proposal, not an implemented or compiled patch. Binding only Workspace::render,
weakening one capture, or suppressing profiler output would not repair general
allocation ownership.

## Repeatable actual-cdylib check

```sh
cargo build -p jcode-desktop -p jcode-desktop-ui
python3 scripts/reload_lifecycle_probe.py target/reload-lifecycle-check
python3 scripts/reload_lifecycle_probe.py target/reload-lifecycle-history-check --history --seconds 3
```

The output directory must not exist. The probe uses private Xvfb, offline fixture
data, isolated directories, and the real host/plugin. It explicitly substitutes
`CARGO=/usr/bin/true` **inside the isolated child only**, making Ctrl+R reuse the
already-built artifact. Thus this checks real library loading, root replacement,
and task behavior, but does **not** validate rebuilding or different-build TypeIds.
Every capture waits for a confirmed generation activation in persisted diagnostics.
Raw profile files, process diagnostics, and RSS/sample summaries remain in the
artifact directory. Duplicate output is never deduplicated. Missing samples,
multiple window IDs, or more than 11 samples/second fail the check.

Observed on this checkout's prebuilt artifacts:

| Fixture | Generation | Samples / capture | RSS KiB |
| --- | ---: | ---: | ---: |
| Empty transcript | 1 | 49 / 6 s | 228164 |
| Empty transcript | 2 | 50 / 6 s | 356760 |
| Empty transcript | 3 | 50 / 6 s | 399540 |
| Sidebar history | 1 | 20 / 3 s | 236600 |
| Sidebar history | 2 | 19 / 3 s | 364904 |
| Sidebar history | 3 | 20 / 3 s | 407596 |

Artifacts: `target/reload-lifecycle-regression` and
`target/reload-lifecycle-history`. Both tests passed the one-sampler bound and
confirmed all three activations. Python bytecode compilation also passed.
Initial exploratory probes that did not wait for activation were discarded as
reload evidence. No user's desktop was restarted, no host reload code was changed,
and no GPUI fork or dependency patch was installed.

## Next discriminating tests

1. Instrument Workspace creation/release and GPUI arena allocation/drop counts in
   a matched host/plugin test build. Assert weak old roots become un-upgradeable
   after replacement and a completed draw, not merely that samples are bounded.
2. Repeat with distinct rebuilt cdylibs, realistic session/history state, cached
   child views, and event traffic. The current test reuses identical plugin bytes.
3. Compare those counters in the live-process-equivalent scenario. Determine
   whether retained old roots, a historical plugin implementation, or another
   ownership path explains the extra active tasks.
4. Only after reproducing the ownership leak should an arena patch be adopted.
   Validate across actual libraries, nested draws, failed activation/rollback,
   window closure, and multiple windows. Monitor root/task drops and bounded
   allocation counts, not only aggregate RSS or log counts.
