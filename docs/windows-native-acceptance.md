# Native Windows acceptance

`windows-runtime-acceptance.yml` validates the actual extracted package on
native Windows x64 and ARM64 runners. Source builds require an immutable
runtime commit. `artifact_run` retests the identical ZIP from a previous
source-build run without rebuilding it. Artifacts retain the ZIP checksum,
source-run metadata, protocol transcript, logs, and live screenshots.

The real-runtime test launches only Desktop. Desktop must start its bundled
daemon and API bridge automatically, create and attach a real session, persist
and retrieve a no-reply message, survive 40 connect/disconnect probes, recover
from forced bridge termination, and reuse the daemon and session after a
normal window close and warm relaunch. Normal relaunch uses the owned HWND's
`WM_CLOSE` and requires a successful process exit. Forced bridge termination
is a separate failure-injection phase, not a substitute for normal shutdown.

Fixture rendering is a separate phase. It checks native caption interaction,
window dragging, maximize/restore, single-instance restoration, version output,
and selected chat, diff and applet states. A fixture screenshot is never used
as proof that the real runtime works. Screenshot evidence requires that the
foreground window belongs to the actual tested process.

## Disposable CI desktop

Some hosted ARM64 Windows images open an OOBE privacy screen that prevents
foreground activation of the app. The workflows run the unchanged acceptance
scripts through `scripts/windows-ci-desktop.py` on a private, visible Win32
input desktop. This is native window/input testing, not an offscreen renderer.
The wrapper explicitly assigns `STARTUPINFO.lpDesktop`, preserves the child
exit status, restores the original input and thread desktops in `finally`,
and saves restoration evidence. Failure to restore is a failed gate.

This wrapper refuses to run unless the environment identifies a disposable
GitHub-hosted Windows runner. Do not use it on a user's desktop or a self-hosted
runner. It never dismisses OOBE, changes privacy options, terminates unrelated
processes, or changes global foreground-lock policy. On a developer machine,
run the acceptance or screenshot script directly without the CI wrapper.

Both native architectures and the mandatory release-package checks must pass
before publication. A blocked foreground, skipped UI phase, or old permissive
check is not a passing strict acceptance result.
