# Motion review audit

From the repository root, run with Python 3.10+ and no third-party dependencies
(the script itself resolves its default source root independently of your cwd):

```sh
python3 scripts/motion_audit.py                 # prioritized known gaps + review errors
python3 scripts/motion_audit.py --check         # CI gate
python3 scripts/motion_audit.py --discover      # include advisory handlers/conditionals
python3 scripts/motion_audit.py --json          # full stable machine-readable report
python3 -m unittest discover -s scripts -p test_motion_audit.py -v
```

This is a **review queue and freshness check**, not an animation detector or proof
that all UI transitions are covered. It does not execute Rust tests, render frames,
inspect the running application, or infer that a policy enum means a surface animates.
The inventory's test evidence records reviewed test source, not a passing test run.

## Discovery and gate boundary

The scanner visits sorted `crates/jcode-desktop-ui/src/**/*.rs` files. The default
`named-ui-methods-v1` gate automatically discovers `toggle_*`, `open_*`, and
`close_*` methods with mutable `self` and a `Context<...>` parameter. New methods
cannot silently inherit a blanket approval. Non-UI service methods without a GPUI
context, conventional test files, and `#[cfg(test)] mod ...` bodies are excluded.

A second, advisory scan finds `.on_*`, `.subscribe*`, `.when`, `.when_some`, and
`if self.*` / `if let ... = self.*` occurrences. These often include nonvisual
conditions and event handlers with no state change. They are **not gated** and are
not each counted as reviewed. `additional_sites` in the inventory promotes an
explicit `{path, symbol}` enclosing function into the gate. The initial inventory
includes compact-sidebar overlay rendering and input slash/model suggestion
rendering this way. This bounded approach avoids approving hundreds of giant
closures without meaningful review. Use `--discover` periodically to choose more
surfaces for promotion.

The lightweight lexical scanner handles comments, nested block comments, strings,
raw strings, character literals, and ordinary lifetimes. It is not a Rust parser.
Macros, unusual signatures, state changes named `set_*`/`select_*`, indirect
callbacks, `match` surfaces, and conditions on local variables can be missed.
Duplicate path/function identities fail closed rather than claiming both reviewed.
An AST-based successor could improve discovery, but should retain this explicit
human decision gate.

## Inventory contract

`docs/motion-inventory.json` is versioned and reviewed with the implementation.
Each decision contains:

- `id`: repository-relative path plus `::function_name`, stable across line movement.
- `sha256`: exact source from `fn` through its matching closing brace. Edits within
  that span, including comments or formatting, require re-review. Unrelated file
  edits and line-number movement do not invalidate the entry.
- `decision`: `animated`, `instant`, or `gap`. Animated can describe a delegated
  path or one branch, but the reason must state that boundary. An intentional
  instant response is not a missing animation.
- `reason`: specific behavior, UX intent, and important exceptions. `gap` entries
  also require priority `1` (frequent/high-impact), `2` (secondary surface), or `3`
  (lower-frequency/investigate first). Priorities are human judgments.
- `evidence`: source and optionally test references, each with `kind`, `path`,
  `sha256`, and an explanatory `note`. A Rust `symbol` hashes only that function.
  Omitting `symbol` hashes the entire file, useful for Python acceptance scripts.
  Missing files/functions or changed fingerprints fail the check. Paths cannot
  escape the repository. At least one source reference is required.
- `test_gap`: mandatory if no test reference exists. It explicitly records missing
  animation-specific verification instead of substituting generic policy tests.

The baseline is conservative. For example, constructing an `AnimatedValue` at its
final width is not evidence of newcomer entrance. Panel-close delegates are marked
animated only for ordinary panels, with immediate default-directory removal
tracked separately. Known gaps do not fail CI: **new unreviewed candidates and
stale/invalid reviews or evidence do**. Exit status is 0 for a clean check, 1 for
review failures under `--check`, and 2 for unreadable/malformed input. Report-only
mode prints review failures but exits 0. JSON is deterministic for identical
sources and inventory, with no timestamps or machine-specific root paths.

## Review workflow

1. Run `--check` and inspect the changed/new source. Read its render consumer,
   lifecycle, focus handling, and delegated transition implementation.
2. Choose animated, intentional instant, or a prioritized gap. Do not simply
   refresh hashes to make CI green. New sites should normally begin as gaps until
   their behavior is understood.
3. For implemented motion, test intermediate frames, reversal during opening,
   retained exit before unmount, reduced motion, usable input/focus, and no ongoing
   frame work after settlement. Run those tests separately and report results.
4. Add source/test evidence and refresh only the reviewed entries. Obtain current
   candidate hashes from `--json`. For an additional evidence function/file:

   ```sh
   python3 - <<'PY'
   import sys
   from pathlib import Path
   sys.path.insert(0, 'scripts')
   from motion_audit import evidence_hash
   print(evidence_hash(Path('.'), {
       'path': 'crates/jcode-desktop-ui/src/transition.rs',
       'symbol': 'retargeting_preserves_the_in_flight_value',
   }))
   PY
   ```

5. Re-run audit tests and `--check`. Commit the reviewed inventory with its source
   changes. In a dirty/shared worktree, ensure every referenced new file actually
   belongs in the change. No blanket auto-baseline/approve command is provided.

The remaining P1 queue includes persistent sidebar layout toggling, image lightbox
entrance/exit, and panel model picker. Compact drawer and input suggestions now
record entrance motion with intentional immediate dismissal and test evidence.
The live report, not this prose snapshot, is the authoritative prioritized queue.
CI should run both commands above without a Rust build or display server. A passing
check means the bounded queue is current, **not** that every gap is fixed or all
UI motion has been tested.
