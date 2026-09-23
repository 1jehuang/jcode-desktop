# Desktop memory diagnostics

## Staged plugin copies on tmpfs

Profiling on 2026-09-22 found a second, larger consumer outside process RSS.
Each hot reload copies the UI cdylib (100-500 MiB per release build) into a
staging directory so that `dlopen` sees a unique path. Staging used to be
`$TMPDIR/jcode-desktop-ui-*`. On systems where `/tmp` is tmpfs, every retained
copy occupies physical memory. Directories from crashed, SIGKILLed, or `exit()`ed
hosts skipped `TempDir` cleanup and were never reclaimed. One machine had 69
such directories totalling 14 GiB of RAM, 55 of them owned by exited hosts.

Staging now lives in `ui-staging/` beside the plugin in `target/<profile>/`,
on the build filesystem. Directory names embed the owning host PID
(`jcode-desktop-ui-<pid>-XXXX`). Each hot-reload launch removes directories
whose owner is no longer alive and never touches a live host's directory.

## Memory telemetry

The host diagnostics thread (`~/.local/state/jcode-desktop/jcode-desktop.log`)
records a `memory startup:` line, then a `memory:` sample every 10 minutes with
RSS split into anonymous (heap/GPUI), file (mapped code, including retained
generations), shmem, and swap. It logs `memory warning:` when RSS first
crosses 1 GiB and each doubling after that, and when RSS grows by 256 MiB or
more between samples. Reloads log each staged generation's size. Growth that
tracks `anon` points at allocation leaks such as the element-arena issue in `hot-reload-memory.md`, while
growth that tracks `file` points at retained generations.
