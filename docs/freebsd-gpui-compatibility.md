# Pinned GPUI FreeBSD dependency compatibility

FreeBSD release job 106060890220 fails in `gpui_linux` because Zed revision
`bc538def4545534201bbfcac4e95ac34ea6501b6` omits FreeBSD from the `gpui::queue`
module and `PriorityQueueReceiver`/`PriorityQueueSender` re-export cfgs.
The queue implementation itself uses portable Rust synchronization primitives.

`scripts/prepare-freebsd-gpui.py` adds `target_os = "freebsd"` to exactly those
two cfgs. It does not enable test-support, impersonate Linux, alter Cargo's
dependency resolution, or touch the tagged Desktop source or lockfile.
It requires native FreeBSD, the exact locked and checked-out Zed revision,
one dependency checkout, and these full `crates/gpui/src/gpui.rs` SHA256 values:

- Before: `7da10d7a7896fabd1bf0a29a989c433b946f322a3355dbd6cd1866243b1ca680`
- After: `1b4154a26a2b84b978cf07903f459398d4c763901be7b7fd570423168d01b972`

## Recovery invocation

Use reviewed tooling from a separately pinned checkout, while retaining the
immutable release checkout and runtime pins. This follows the source/tag
verification approach in the beta28 recovery workflow. The coordinator must
retain private draft checks, package verification, CLI/ldd checks, and native
X11 graphical smoke before upload or publication.

```sh
# Inside the native FreeBSD VM. This path must NOT already exist.
# Keep the dependency cache outside the workspace synced back to the host.
export CARGO_HOME="$HOME/.cargo-freebsd-recovery"
python3 release-tools/scripts/prepare-freebsd-gpui.py \
  --desktop-root "$PWD/jcode-desktop" --cargo-home "$CARGO_HOME" \
  --target x86_64-unknown-freebsd
bash jcode-desktop/scripts/package-freebsd.sh
python3 release-tools/scripts/prepare-freebsd-gpui.py \
  --desktop-root "$PWD/jcode-desktop" --cargo-home "$CARGO_HOME" \
  --target x86_64-unknown-freebsd --verify-only
# Continue every existing native verification and smoke gate.
```

Use a new directory for each attempt. Never seed it with linked/shared Cargo
checkouts. Failed fetches or patch checks leave the isolated home for diagnostics
and abort before packaging. Keep the exported Cargo home for all build commands.
No existing Cargo home is accepted, even an empty one. No shared cache is changed.
The `--verify-only` mode instead requires the prepared home and verifies its exact
locked revision, checked-out revision, patched digest, and that the only tracked
Zed diff is `crates/gpui/src/gpui.rs`, without refetching or writing anything. Use it after
packaging to catch dependency source replacement during the build.

## Validation and long-term resolution

Run `python3 -m unittest discover -s scripts -p test_prepare_freebsd_gpui.py -v`.
Optionally set `GPUI_PINNED_SOURCE` to a pristine pinned `gpui.rs` to exercise
the real before/after hashes on a temporary copy. Other tests use a small cfg
fixture and explicitly substituted fixture hashes, not a fake production hash.
These offline tests do not establish native compile or graphical success.

Sustainable resolution is to upstream the two cfg additions, then move the Zed
pin to a reviewed revision containing them and remove this temporary helper.
Until then, keep recovery tooling separately reviewed/pinned and the dependency
patch auditable. Any source or pin change must fail closed and be re-reviewed,
not automatically refresh the digests or silently skip the patch.
