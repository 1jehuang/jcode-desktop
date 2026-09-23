# One-command Desktop releases

From a clean, reviewed checkout of current `origin/main`, on Linux or macOS:

```sh
python3 scripts/release-desktop.py 0.3.1 --apply
```

Without `--apply`, the command only checks the source/pins and reports its plan.
Prerequisites: Python 3.11+, Git with release history, authenticated `gh` with
repository contents/actions write access, and configured signing, public-release,
and Discord secrets on GitHub. Fetch and review main before invoking the command.
A new version must resolve to the current remote main SHA. Existing versions resume
their original immutable commit, regardless of how main has subsequently advanced.

## What the command owns

1. Validate the explicit version against all four source manifests and consistent
   immutable runtime pins.
2. For a new version, reject other active release pipelines, dirty or stale checkouts,
   then run the complete Python release contract suite before creating an atomic tag.
3. Observe tag-triggered macOS and cross-platform builds. If using `GITHUB_TOKEN`,
   which cannot trigger tag-push workflows, dispatch missing workflows after a
   90-second discovery window. macOS, Linux, Windows and FreeBSD still build in parallel.
4. Wait for both complete build workflows, the public-download publisher, actual
   downloaded macOS installation/Gatekeeper acceptance, and the Discord announcement.
   A green build is not reported as a completed public release.
5. Fetch the actual `https://jcode.sh/desktop/latest.json` for stable releases and
   require the exact tag, non-draft/non-prerelease flags, complete unique asset
   set, valid sizes/hashes and exact versioned download URLs. A stale website
   channel is a failure even if all workflow runs are green. Betas instead verify
   their versioned manifest because they intentionally do not displace stable.

The existing workflows remain the sole owners of signing, native smoke gates,
checksums, staging, anonymous-download verification and promotion. No bypass,
tag move, unconditional retry, or manual asset upload is introduced. Main's
private-draft transport remains independent of Actions artifact storage quota.

Run names identify downstream work with its exact version. Build runs additionally
require the exact tag and SHA. The latest matching attempt wins, never a stale
successful attempt when a newer one failed. Skipped/cancelled/failed gates stop
with the run URL. The orchestrator never cancels remote work on interruption.

## Resume and failures

Re-run the same command after interruption. Dispatch intents persist under
`.git/desktop-releases/` and a POSIX advisory lock prevents simultaneous local
owners in the same checkout. Run discovery supports pagination. An ambiguous
failed dispatch is deliberately not automatically repeated after resume: inspect
GitHub, explicitly dispatch only if no run exists, and resume monitoring. Use one
release operator/checkout at a time, since GitHub does not offer an atomic
check-and-dispatch transaction across separate machines.

The default wait deadline is four hours. `--timeout-minutes` and `--poll-seconds`
change monitoring only, not workflow gates. If a gate fails, diagnose it and
explicitly rerun the failed job/workflow, then resume. If publisher/acceptance/
announcement dispatches are missing, inspect the responsible workflow rather
than bypassing it. Older releases without versioned downstream run names cannot
be automatically correlated by this command and require their existing recovery
procedures. A Discord failure is reported separately by its workflow and does not
unpublish already verified downloads, but the full orchestration exits nonzero.

## September 20, 2026 measurements

Read-only GitHub job measurements, not estimated local compile times:

| Workflow/run | Measured job time | Main contributor |
| --- | ---: | --- |
| macOS `35526094412` | 51.70 min | universal build/package 49.32 min |
| Windows x64 `35526095831` | 50.53 min | build 46.50 min, cache save 3.02 min |
| Windows ARM64, same run | 27.70 min | build 24.62 min |
| Linux x64, same run | 21.42 min | build 18.97 min |
| Linux ARM64, same run | 20.53 min | build 18.88 min |
| FreeBSD, same run | failed at 15.95 min | missing GPUI queue exports |
| Verified FreeBSD recovery `35508120526` | 40.87 min | VM build/package/smoke 40.28 min |
| Public publisher `35510019484` | 1.87 min | upload and anonymous download verification |
| Public macOS acceptance `35510111144` | 0.58 min | real public DMG install and acceptance |

Current native critical path is approximately 52 minutes before publication, not
including runner queue time. These are different runs, so about 54 minutes total
is a planning estimate, not an observed successful all-platform release duration.

The old automatic policy intentionally batches 30 minutes of quiet and enforces
a default six-hour interval, with an hourly scheduled catch-up. Explicit release
intent now bypasses those scheduling delays without skipping any tests. The
cross-platform publisher polls macOS every 20 seconds instead of 120, preserving
the same three-hour maximum while reducing final synchronization latency by up
to 100 seconds. Do not disable native tests to chase compilation time.

The highest-value reliability fix is preventing recovery cycles. Regular FreeBSD
releases now use the already hash-verified, dependency-only GPUI compatibility
preparation in a fresh Cargo home and verify it again after build. Source selection
uses the exact upstream revision, tolerating the independently pinned Linux fork
without changing it. The real native graphical gate uses short private Unix socket
paths, robust X11 title decoding, an exact Jcode window check, and full lifetime
and process-cleanup checks, as proven in the 0.2.1 recovery.

Potential next optimization: profile the macOS per-architecture compile/link split
and cache hit/miss logs before adding parallel architecture jobs. Windows cache
save consumes about three minutes, but disabling saves may increase the next
release's cold-build cost. Neither speculative change belongs in 0.3.0 without
representative runner measurements and full native acceptance.

## Offline validation

```sh
python3 -m unittest discover -s tests -v
python3 -m unittest discover -s scripts -p test_prepare_freebsd_gpui.py -v
```

`test_freebsd_regular_smoke.py` executes functions extracted from the actual normal
workflow, reusing the recovery regression cases. These tests do not substitute for
a complete native FreeBSD build and graphical smoke in the release workflow.
