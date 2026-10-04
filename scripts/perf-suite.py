#!/usr/bin/env python3
"""Workspace performance suite: FPS, draw time and CPU under real workspace use.

Drives the real app in the multi-panel workspace on a private Xvfb software
renderer with offline fixture data, through native keyboard shortcuts only:
open panels, move focus left/right/up/down, move panels between strips, cycle
widths, toggle overview and close panels. Each scenario runs with GPUI view
retention on and off, repeated, and reports medians.

This is CPU/software-renderer evidence for comparing builds and settings on
one machine, not a claim about GPU presentation on the user's display. No
daemon, credentials, compositor commands or input to the user's display.

    python3 scripts/perf-suite.py target/perf/run1
    python3 scripts/perf-suite.py target/perf/run2 --baseline target/perf/run1/summary.json
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import random
import select
import signal
import statistics
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
from screenshot import isolated_env  # noqa: E402

_spec = importlib.util.spec_from_file_location('profile_live', Path(__file__).with_name('profile-live.py'))
profile = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(profile)

# Workspace shortcuts, as bound in crates/jcode-desktop-ui/src/lib.rs.
NEW = 'super+n'
CLOSE = 'super+q'
FOCUS = {'left': 'super+h', 'right': 'super+l', 'down': 'super+j', 'up': 'super+k'}
MOVE = {'left': 'super+shift+h', 'right': 'super+shift+l', 'down': 'super+shift+j', 'up': 'super+shift+k'}
WIDTH = 'super+r'
OVERVIEW = 'super+o'


def scenario_keys(name, rng):
    """The keystrokes of one pass through a scenario, as (chord, pause_seconds)."""
    fast, settle = 0.08, 0.35
    if name == 'idle':
        return []
    if name == 'spawn-close':
        return [(NEW, settle)] * 6 + [(CLOSE, settle)] * 6
    if name == 'navigate':
        keys = []
        for _ in range(3):
            keys += [(FOCUS['right'], fast)] * 4 + [(FOCUS['left'], fast)] * 4
            keys += [(FOCUS['down'], settle), (FOCUS['right'], fast), (FOCUS['up'], settle)]
        return keys
    if name == 'rearrange':
        keys = []
        for _ in range(3):
            keys += [(MOVE['right'], settle), (MOVE['down'], settle), (MOVE['left'], settle),
                     (MOVE['up'], settle), (WIDTH, settle)]
        return keys
    if name == 'overview':
        return [(OVERVIEW, settle)] * 8
    if name == 'stress':
        # Everything at once, fast, in a reproducible random order.
        pool = [NEW, CLOSE, WIDTH, OVERVIEW] + list(FOCUS.values()) * 3 + list(MOVE.values())
        keys, open_panels = [], 0
        for _ in range(60):
            chord = rng.choice(pool)
            if chord == NEW:
                if open_panels >= 8:
                    chord = CLOSE
                else:
                    open_panels += 1
            if chord == CLOSE:
                if open_panels == 0:
                    chord = FOCUS['right']
                else:
                    open_panels -= 1
            keys.append((chord, rng.choice((0.04, 0.08, 0.15))))
        return keys + [(CLOSE, 0.15)] * open_panels
    raise ValueError(name)


SCENARIOS = ('idle', 'spawn-close', 'navigate', 'rearrange', 'overview', 'stress')
# The workspace a scenario starts from: how many fixture panels, and whether
# they need a second strip to navigate between.
SETUP = {'navigate': 'rows', 'rearrange': 'rows', 'stress': 'rows'}


def summarize(frames, before, after):
    """FPS and frame cost over the frames sampled between `before` and `after`."""
    seconds = sum(f['interval_ms'] for f in frames) / 1000
    draws = sum(f['draw_count'] for f in frames)
    presents = sum(f['animation_present_count'] for f in frames)
    elapsed = (after['unix_ms'] - before['unix_ms']) / 1000
    cpu = (after['main_cpu_ticks'] - before['main_cpu_ticks']) / os.sysconf('SC_CLK_TCK')
    weighted = [(f['draw_mean_ms'], f['draw_count']) for f in frames if f['draw_count'] and f['draw_mean_ms'] is not None]
    present_weighted = [(f['animation_present_mean_ms'], f['animation_present_count']) for f in frames
                        if f['animation_present_count'] and f['animation_present_mean_ms'] is not None]
    def mean(pairs):
        total = sum(count for _, count in pairs)
        return sum(value * count for value, count in pairs) / total if total else None
    def maximum(field):
        values = [f[field] for f in frames if f.get(field) is not None]
        return max(values) if values else None
    present_mean = mean(present_weighted)
    views = profile.view_totals(frames)
    causes = profile.cause_totals(frames)
    return dict(
        seconds=round(seconds, 3),
        draws=draws,
        draws_per_second=draws / seconds if seconds else 0,
        # Frames per second while animating, from presentation intervals. Idle
        # time between animations is not counted as a slow frame.
        animation_fps=1000 / present_mean if present_mean else None,
        animation_frames=presents,
        draw_mean_ms=mean(weighted),
        draw_p95_max_ms=maximum('draw_p95_ms'),
        draw_max_ms=maximum('draw_max_ms'),
        input_p95_max_ms=maximum('input_p95_ms'),
        input_max_ms=maximum('input_max_ms'),
        wake_lag_max_ms=maximum('ui_wake_lag_ms'),
        cpu_percent=cpu / elapsed * 100 if elapsed else None,
        # The UI thread alone. Process CPU is dominated by the software
        # rasterizer on Xvfb, which runs on the GPU on a real display.
        main_thread_cpu_percent=(
            (after['main_thread_ticks'] - before['main_thread_ticks'])
            / os.sysconf('SC_CLK_TCK') / elapsed * 100
            if elapsed and 'main_thread_ticks' in after else None
        ),
        rss_mb=after['rss_kb'] / 1024,
        # Which views rebuilt their element trees, and what that cost.
        views=views,
        causes=causes,
    )


def process_sample(pid):
    """`profile-live`'s process sample, plus the UI (main) thread's own CPU."""
    sample = profile.process_sample(pid)
    stat = Path(f'/proc/{pid}/task/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    sample['main_thread_ticks'] = int(stat[11]) + int(stat[12])
    return sample


def median_of(runs, key):
    values = [run[key] for run in runs if run.get(key) is not None]
    return statistics.median(values) if values else None


class App:
    """The real app on a private Xvfb with an isolated home and fixture data."""

    def __init__(self, root, binary, retention, panels, rows, transcript):
        self.root = root
        for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
        env = isolated_env(root)
        env['VK_DRIVER_FILES'] = str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json')))
        env['JCODE_DESKTOP_SCREENSHOT_PANELS'] = str(panels)
        env['JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT'] = transcript
        env['GPUI_VIEW_RETENTION'] = '1' if retention else '0'
        # Pass through tracing switches for investigations.
        for name in ('GPUI_TRACE_NOTIFY', 'GPUI_TRACE_RENDERS', 'GPUI_TRACE_WRITES', 'GPUI_TRACE_UPDATES', 'RUST_BACKTRACE'):
            if name in os.environ:
                env[name] = os.environ[name]
        env['JCODE_DESKTOP_CONFIG'] = str(root / 'desktop.toml')
        (root / 'desktop.toml').write_text('[appearance]\nlayout_mode = "normal"\n'
                                           '[workspace]\ncoaching_hints = false\n')
        self.env = env
        self.rows = rows
        self.processes, self.logs = [], []
        read_fd, write_fd = os.pipe()
        try:
            self._launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1440x1000x24',
                                  '-nolisten', 'tcp'], pass_fds=(write_fd,))
            os.close(write_fd)
            write_fd = None
            if not select.select([read_fd], [], [], 15)[0]:
                raise RuntimeError('Xvfb did not start')
            env['DISPLAY'] = ':' + os.read(read_fd, 64).decode().strip()
        finally:
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
        config = root / 'openbox.xml'
        config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                          '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                          '</application></applications></openbox_config>')
        self._launch('wm', ['openbox', '--sm-disable', '--config-file', str(config)])
        time.sleep(0.5)
        self.app = self._launch('app', [str(binary), '--no-hot-reload'])
        self._wait_rendered(panels)
        # Dismiss the launch overlay, then give the first frames time to settle.
        self.keys('Escape')
        time.sleep(2)
        if rows:
            # A second strip to move up and down between.
            self.keys(MOVE['down'])
            time.sleep(0.6)
            self.keys(FOCUS['up'])
            time.sleep(1)

    def _launch(self, name, command, **kwargs):
        log = (self.root / (name + '.log')).open('w')
        self.logs.append(log)
        process = subprocess.Popen(command, env=self.env, cwd=self.root, stdout=log, stderr=log, **kwargs)
        self.processes.append(process)
        return process

    def _wait_rendered(self, panels):
        state = self.root / 'state'
        widths = 'widths=' + ','.join([f'{1 / panels:.2f}'] * panels)
        deadline = time.monotonic() + 180
        while not state.exists() or widths not in state.read_text():
            if self.app.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f'App failed to render, see {self.root / "app.log"}')
            time.sleep(0.1)

    def keys(self, *chords):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '0', *chords],
                       env=self.env, check=True, timeout=10)

    def navigation(self):
        try:
            text = (self.root / 'state').read_text()
            line = next(l for l in text.splitlines() if l.startswith('navigation='))
            return json.loads(line[len('navigation='):])
        except (OSError, StopIteration, json.JSONDecodeError):
            return None

    def measure(self, capture, keys, seconds, profile_to=None):
        """Plays `keys` in a loop for `seconds` while sampling frame timing."""
        control = self.root / 'runtime/jcode-desktop-profile.json'
        control.write_text(json.dumps(dict(capture_id=capture,
                                           until_unix_ms=int((time.time() + seconds + 4) * 1000))))
        time.sleep(1.2)
        sampler = None
        if profile_to:
            # A CPU profile of the app over exactly the measured window.
            sampler = subprocess.Popen(
                ['samply', 'record', '--save-only', '--no-open', '--rate', '2000',
                 '-o', str(profile_to), '-p', str(self.app.pid)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            time.sleep(0.3)
        before = process_sample(self.app.pid)
        started = time.monotonic()
        actions, states = 0, set()
        while time.monotonic() - started < seconds:
            if self.app.poll() is not None:
                raise RuntimeError(f'App exited, see {self.root / "app.log"}')
            if not keys:
                time.sleep(0.25)
                continue
            for chord, pause in keys:
                if time.monotonic() - started >= seconds:
                    break
                self.keys(chord)
                actions += 1
                time.sleep(pause)
                nav = self.navigation()
                if nav:
                    states.add((nav['active_row'], nav['focused_slot'], nav['overview'],
                                sum(len(row['panels']) for row in nav['rows'])))
        after = process_sample(self.app.pid)
        control.unlink(missing_ok=True)
        if sampler:
            # samply ignores --duration when attached; it stops and saves on SIGINT.
            sampler.send_signal(signal.SIGINT)
            sampler.wait(timeout=120)
        time.sleep(0.3)
        source = self.root / f'runtime/jcode-desktop-profile-{self.app.pid}-{capture}.jsonl'
        if not source.exists():
            raise RuntimeError('No profile samples; build with live profiling')
        frames = [json.loads(line) for line in source.read_text().splitlines() if line.endswith('}')]
        frames = [f for f in frames if before['unix_ms'] <= f['unix_ms'] <= after['unix_ms']]
        result = summarize(frames, before, after)
        result.update(actions=actions, distinct_states=len(states))
        if keys and len(states) < 2:
            raise RuntimeError(f'{capture}: shortcuts did not change the workspace; see {self.root}')
        return result

    def close(self):
        for process in reversed(self.processes):
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for log in self.logs:
            log.close()


def compare(current, baseline, tolerance):
    """Regressions of more than `tolerance` (a fraction) against a baseline."""
    worse = []
    for key, result in current.items():
        base = baseline.get(key)
        if not base:
            continue
        # Lower is better for these; FPS is higher-is-better.
        for metric in ('draw_mean_ms', 'main_thread_cpu_percent'):
            now, then = result.get(metric), base.get(metric)
            if now is not None and then and now > then * (1 + tolerance) and now - then > 0.5:
                worse.append(f'{key} {metric}: {then:.2f} -> {now:.2f}')
        now, then = result.get('animation_fps'), base.get('animation_fps')
        if now is not None and then and now < then * (1 - tolerance):
            worse.append(f'{key} animation_fps: {then:.1f} -> {now:.1f}')
    return worse


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=Path('target/release/jcode-desktop'))
    parser.add_argument('--scenario', choices=SCENARIOS, action='append',
                        help='repeatable; default runs every scenario')
    parser.add_argument('--retention', choices=('on', 'off', 'both'), default='both')
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--seconds', type=float, default=8)
    parser.add_argument('--panels', type=int, choices=range(2, 7), default=3)
    parser.add_argument('--transcript', choices=('all', 'long-history', 'streaming', 'empty'), default='all')
    parser.add_argument('--seed', type=int, default=7)
    parser.add_argument('--baseline', type=Path, help='summary.json of an earlier run to compare against')
    parser.add_argument('--tolerance', type=float, default=0.15,
                        help='fraction a metric may worsen against the baseline before failing')
    parser.add_argument('--profile', action='store_true',
                        help='record a samply CPU profile of each measured scenario (needs samply)')
    args = parser.parse_args()
    if args.seconds < 3 or args.repeats < 1:
        parser.error('need at least 3 seconds and one repeat')
    binary = args.binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    scenarios = args.scenario or list(SCENARIOS)
    retentions = {'on': [True], 'off': [False], 'both': [True, False]}[args.retention]
    with binary.open('rb') as file:
        digest = hashlib.file_digest(file, 'sha256').hexdigest()
    meta = dict(binary=str(binary), sha256=digest, scenarios=scenarios, repeats=args.repeats,
                seconds=args.seconds, panels=args.panels, transcript=args.transcript, seed=args.seed,
                loadavg=Path('/proc/loadavg').read_text().strip())
    (root / 'capture.json').write_text(json.dumps(meta, indent=2) + '\n')

    runs = {}
    for repeat in range(args.repeats):
        for retention in retentions:
            # A fresh app per repeat and setting: one app's history must not
            # carry into the next measurement.
            for rows in sorted({SETUP.get(s) is not None for s in scenarios}):
                group = [s for s in scenarios if (SETUP.get(s) is not None) == rows]
                if not group:
                    continue
                tag = f'{"on" if retention else "off"}-{"rows" if rows else "flat"}-{repeat}'
                app = App(root / tag, binary, retention, args.panels, rows, args.transcript)
                try:
                    for scenario in group:
                        keys = scenario_keys(scenario, random.Random(args.seed))
                        key = f'{scenario}/retention-{"on" if retention else "off"}'
                        profile_to = root / f'{key.replace("/", "-")}-{repeat}.json.gz' if args.profile else None
                        result = app.measure(scenario.replace('-', ''), keys, args.seconds, profile_to)
                        runs.setdefault(key, []).append(result)
                        print(f'{key:32s} run {repeat + 1}: '
                              f'fps={result["animation_fps"] or 0:6.1f} '
                              f'draw={result["draw_mean_ms"] or 0:6.2f}ms '
                              f'p95max={result["draw_p95_max_ms"] or 0:6.2f}ms '
                              f'ui={result["main_thread_cpu_percent"] or 0:5.1f}% all={result["cpu_percent"]:5.1f}% actions={result["actions"]}',
                              flush=True)
                        time.sleep(1)
                finally:
                    app.close()

    metrics = ('animation_fps', 'draws_per_second', 'draw_mean_ms', 'draw_p95_max_ms', 'draw_max_ms',
               'input_max_ms', 'main_thread_cpu_percent', 'cpu_percent', 'rss_mb')
    summary = {key: {m: median_of(results, m) for m in metrics} | {'runs': len(results)}
               for key, results in runs.items()}
    (root / 'runs.json').write_text(json.dumps(runs, indent=2) + '\n')
    (root / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')

    print(f'\nMedians of {args.repeats} run(s), {args.seconds:g}s each. Load at start: {meta["loadavg"]}')
    print(f'{"scenario":32s} {"fps":>7s} {"draw ms":>8s} {"p95 max":>8s} {"max ms":>8s} {"ui cpu%":>8s} {"all cpu%":>8s} {"rss MB":>7s}')
    def cell(value, width, digits):
        return f'{value:{width}.{digits}f}' if value is not None else ' ' * (width - 1) + '-'
    for key in sorted(summary):
        s = summary[key]
        print(f'{key:32s} {cell(s["animation_fps"], 7, 1)} {cell(s["draw_mean_ms"], 8, 2)} '
              f'{cell(s["draw_p95_max_ms"], 8, 2)} {cell(s["draw_max_ms"], 8, 2)} '
              f'{cell(s["main_thread_cpu_percent"], 8, 1)} {cell(s["cpu_percent"], 8, 1)} '
              f'{cell(s["rss_mb"], 7, 0)}')

    for key in sorted(runs):
        first = runs[key][0]
        print(f'\nViews rendered in {key} (first run, render() time only):')
        profile.print_view_totals(first.get('views') or {}, first['seconds'] or 1)
        profile.print_cause_totals(first.get('causes') or {}, first['seconds'] or 1)

    if args.baseline:
        worse = compare(summary, json.loads(args.baseline.read_text()), args.tolerance)
        if worse:
            print('\nRegressions against baseline:')
            for line in worse:
                print('  ' + line)
            return 1
        print(f'\nNo regressions beyond {args.tolerance:.0%} against {args.baseline}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
