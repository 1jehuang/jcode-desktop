#!/usr/bin/env python3
"""Workspace performance suite: FPS, draw time and CPU under real workspace use.

Drives the real app on private Xvfb (software) or headless Sway (GPU)
with offline fixture data, through native keyboard shortcuts only:
open panels, move focus left/right/up/down, move panels between strips, cycle
widths, toggle overview and close panels. Each scenario runs with GPUI view
retention on and off, repeated, and reports medians.

Results compare builds and settings on one machine, not presentation on the
user's physical display. No daemon, credentials or input to the user's display.

    python3 scripts/perf-suite.py target/perf/run1
    python3 scripts/perf-suite.py target/perf/run2 --baseline target/perf/run1/summary.json
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import random
import select
import signal
import statistics
import subprocess
import sys
import tempfile
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

    def __init__(self, root, binary, retention, panels, rows, transcript, display='xvfb', sway_prefix=None,
                 panel_width=None, layout_mode='normal'):
        self.root = root
        self.display = display
        self.runtime_alias = None
        self.expect_camera = rows and panel_width is not None and panel_width * (panels - 1) > 1
        for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
        env = isolated_env(root)
        if display == 'xvfb':
            env['VK_DRIVER_FILES'] = str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json')))
        else:
            # The real GPU through its render node, on a private compositor:
            # the user's own compositor is never contacted.
            env.pop('LIBGL_ALWAYS_SOFTWARE', None)
            env.update(WLR_BACKENDS='headless', WLR_LIBINPUT_NO_DEVICES='1', XDG_SESSION_TYPE='wayland',
                       WLR_RENDERER=os.environ.get('WLR_RENDERER', 'gles2'))
            if sway_prefix:
                env['PATH'] = str(sway_prefix / 'usr/bin') + ':' + env['PATH']
                env['LD_LIBRARY_PATH'] = str(sway_prefix / 'usr/lib')
        env['JCODE_DESKTOP_SCREENSHOT_PANELS'] = str(panels)
        env['JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT'] = transcript
        env['GPUI_VIEW_RETENTION'] = '1' if retention else '0'
        # Pass through tracing switches for investigations.
        for name in ('GPUI_TRACE_NOTIFY', 'GPUI_TRACE_RENDERS', 'GPUI_TRACE_WRITES', 'GPUI_TRACE_UPDATES', 'RUST_BACKTRACE'):
            if name in os.environ:
                env[name] = os.environ[name]
        env['JCODE_DESKTOP_CONFIG'] = str(root / 'desktop.toml')
        (root / 'desktop.toml').write_text(f'[appearance]\nlayout_mode = "{layout_mode}"\n'
                                           '[workspace]\ncoaching_hints = false\n')
        self.env = env
        self.rows = rows
        self.processes, self.logs = [], []
        try:
            if display == 'wayland':
                # Unix socket names are limited to 108 bytes. Keep their names
                # short even when the requested capture directory is deeply nested.
                self.runtime_alias = tempfile.TemporaryDirectory(prefix='jdperf-')
                runtime_alias = Path(self.runtime_alias.name) / 'runtime'
                runtime_alias.symlink_to(root / 'runtime', target_is_directory=True)
                self.env['XDG_RUNTIME_DIR'] = str(runtime_alias)
                self._start_sway()
            else:
                self._start_xvfb()
            self.app = self._launch('app', [str(binary), '--no-hot-reload'])
            self._wait_rendered(panels)
            self._prepare(rows, panels, panel_width)
        except BaseException:
            self.close()
            raise

    def _prepare(self, rows, panels, panel_width):
        # Dismiss the launch overlay, then give the first frames time to settle.
        self.keys('Escape')
        time.sleep(2)
        if panel_width is not None:
            self.keys(*([FOCUS['left']] * panels))
            time.sleep(0.5)
            for panel_index in range(panels):
                for attempt in range(4):
                    time.sleep(0.1)
                    nav = self.navigation()
                    focused = next((p for row in nav['rows'] for p in row['panels'] if p['focused']), None) if nav else None
                    if focused and abs(focused['width'] - panel_width) < 0.001:
                        break
                    if attempt == 3:
                        raise RuntimeError('Could not set panel width through native shortcuts')
                    self.keys(WIDTH)
                    old_width = focused['width'] if focused else None
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        nav = self.navigation()
                        updated = next((p for row in nav['rows'] for p in row['panels'] if p['focused']), None) if nav else None
                        if updated and updated['width'] != old_width:
                            break
                        time.sleep(0.05)
                    else:
                        raise RuntimeError('Native width shortcut did not update fixture state')
                self.keys(FOCUS['right'])
                if panel_index < panels - 1:
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        nav = self.navigation()
                        if nav and nav['focused_slot'] != focused['slot']:
                            break
                        time.sleep(0.05)
                    else:
                        raise RuntimeError('Native focus shortcut did not advance fixture panel')
            self.keys(*([FOCUS['left']] * panels))
            time.sleep(0.5)
            nav = self.navigation()
            if not nav or any(abs(p['width'] - panel_width) > 0.001 for row in nav['rows'] for p in row['panels']):
                raise RuntimeError('Native width setup did not produce the requested fixture')
        if rows:
            # A second strip to move up and down between.
            self.keys(MOVE['down'])
            time.sleep(0.6)
            self.keys(FOCUS['up'])
            time.sleep(1)

    def _start_sway(self):
        config = self.root / 'sway.config'
        config.write_text('xwayland disable\noutput HEADLESS-1 mode ' + os.environ.get('PERF_SUITE_MODE', '2880x1800@120Hz')
                          + ' scale ' + os.environ.get('PERF_SUITE_SCALE', '1.6') + '\n'
                          'default_border none\nfocus_follows_mouse no\n')
        for key in ('DISPLAY', 'WAYLAND_DISPLAY', 'SWAYSOCK', 'I3SOCK'):
            self.env.pop(key, None)
        sway = self._launch('sway', ['sway', '--config', str(config)])
        runtime = self.root / 'runtime'
        deadline = time.monotonic() + 20
        while True:
            names = [p.name for p in runtime.glob('wayland-*') if p.is_socket()]
            sockets = [p for p in runtime.glob('sway-ipc.*.sock') if p.is_socket()]
            if names and sockets:
                break
            if sway.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(f'Sway did not start, see {self.root / "sway.log"}')
            time.sleep(0.05)
        self.env['WAYLAND_DISPLAY'] = names[0]
        self.env['SWAYSOCK'] = str(Path(self.env['XDG_RUNTIME_DIR']) / sockets[0].name)
        outputs = subprocess.run(['swaymsg', '-t', 'get_outputs', '-r'], env=self.env,
                                 capture_output=True, text=True, check=True, timeout=10)
        (self.root / 'outputs.json').write_text(outputs.stdout)

    def _start_xvfb(self):
        env = self.env
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
        config = self.root / 'openbox.xml'
        config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                          '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                          '</application></applications></openbox_config>')
        self._launch('wm', ['openbox', '--sm-disable', '--config-file', str(config)])
        time.sleep(0.5)

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

    @staticmethod
    def _wtype(keys):
        """One wtype command for `keys`, as (chord, pause_seconds).

        Each wtype process attaches its own virtual keyboard, and the window
        loses keyboard focus when that keyboard goes away. An inactive window
        is throttled to 30 fps, so a measured run must be a single process.
        """
        names = {'super': 'logo'}
        # Let wl_keyboard.enter reach the window before the first shortcut.
        command = ['wtype', '-s', '150']
        for chord, pause in keys:
            *mods, key = chord.split('+')
            mods = [names.get(m, m) for m in mods]
            for mod in mods:
                command += ['-M', mod]
            command += ['-k', key]
            for mod in reversed(mods):
                command += ['-m', mod]
            if pause:
                command += ['-s', str(round(pause * 1000))]
        return command

    def keys(self, *chords):
        if self.display == 'wayland':
            subprocess.run(self._wtype([(chord, 0) for chord in chords]), env=self.env, check=True, timeout=10)
            return
        subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '0', *chords],
                       env=self.env, check=True, timeout=10)

    def navigation(self):
        try:
            text = (self.root / 'state').read_text()
            line = next(l for l in text.splitlines() if l.startswith('navigation='))
            return json.loads(line[len('navigation='):])
        except (OSError, StopIteration, json.JSONDecodeError):
            return None

    @staticmethod
    def _schedule(keys, seconds):
        """Bounded input sequence, without an action on the end boundary."""
        if not keys:
            return []
        if any(pause <= 0 for _, pause in keys):
            raise ValueError('scenario pauses must be positive')
        result, elapsed = [], 0.0
        while elapsed < seconds - 0.05:
            chord, pause = keys[len(result) % len(keys)]
            pause = min(pause, seconds - elapsed)
            result.append((chord, pause))
            elapsed += pause
        return result

    @staticmethod
    def _stop(process, sig=signal.SIGTERM, timeout=10):
        if process is None:
            return
        if process.poll() is None:
            try:
                process.send_signal(sig)
            except ProcessLookupError:
                pass
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)

    def measure(self, capture, keys, seconds, profile_to=None):
        """Plays `keys` in a loop for `seconds` while sampling frame timing."""
        input_lead = 0.25 if self.display == 'wayland' else 0
        schedule = self._schedule(keys, max(0, seconds - input_lead))
        control = self.root / 'runtime/jcode-desktop-profile.json'
        control.write_text(json.dumps(dict(capture_id=capture,
                                           until_unix_ms=int((time.time() + seconds + 4) * 1000))))
        sampler = typing = sampler_log = None
        actions, states, cameras = 0, set(), set()
        try:
            time.sleep(1.2)
            if profile_to:
                sampler_log = (self.root / f'samply-{capture}.log').open('w')
                sampler = subprocess.Popen(
                    ['samply', 'record', '--save-only', '--unstable-presymbolicate', '--no-open', '--rate', '2000',
                     '-o', str(profile_to), '-p', str(self.app.pid)],
                    stdout=sampler_log, stderr=sampler_log)
                time.sleep(0.3)
                if sampler.poll() is not None:
                    raise RuntimeError(f'samply failed to attach, see {self.root / f"samply-{capture}.log"}')
            if self.display == 'wayland':
                # A single virtual keyboard spans warmup, idle and active runs,
                # and remains attached until AFTER the final process sample.
                command = self._wtype(schedule)
                command[1:3] = ['-s', '1000']
                command += ['-s', str(round((seconds - sum(p for _, p in schedule) + 2) * 1000))]
                typing = subprocess.Popen(command, env=self.env)
                time.sleep(1 - input_lead)
            before = process_sample(self.app.pid)
            started = time.monotonic()
            deadline = started + seconds
            next_key = 0
            next_at = started
            while time.monotonic() < deadline:
                if self.app.poll() is not None:
                    raise RuntimeError(f'App exited, see {self.root / "app.log"}')
                if typing is not None and typing.poll() is not None:
                    raise RuntimeError('wtype exited before the measurement boundary')
                if self.display != 'wayland' and schedule and next_key < len(schedule) and time.monotonic() >= next_at:
                    chord, pause = schedule[next_key]
                    self.keys(chord)
                    next_key += 1
                    actions += 1
                    next_at = time.monotonic() + pause
                nav = self.navigation()
                if nav:
                    states.add((nav['active_row'], nav['focused_slot'], nav['overview'],
                                sum(len(row['panels']) for row in nav['rows'])))
                    cameras.add(tuple(round(row.get('camera_target', row.get('camera', 0)), 2)
                                      for row in nav['rows']))
                time.sleep(min(0.03, max(0, deadline - time.monotonic())))
            after = process_sample(self.app.pid)
            elapsed = time.monotonic() - started
            if self.display == 'wayland':
                actions = len(schedule)
        finally:
            control.unlink(missing_ok=True)
            try:
                self._stop(typing)
            finally:
                # Attached samply saves on SIGINT, even on input/app failures.
                try:
                    self._stop(sampler, signal.SIGINT, timeout=120)
                finally:
                    if sampler_log:
                        sampler_log.close()
        if profile_to and (not profile_to.exists() or not profile_to.stat().st_size):
            raise RuntimeError(f'samply did not save a profile, see {self.root / f"samply-{capture}.log"}')
        time.sleep(0.3)
        source = self.root / f'runtime/jcode-desktop-profile-{self.app.pid}-{capture}.jsonl'
        if not source.exists():
            raise RuntimeError('No profile samples; build with live profiling')
        frames = [json.loads(line) for line in source.read_text().splitlines() if line.endswith('}')]
        frames = [f for f in frames if before['unix_ms'] <= f['unix_ms'] - f['interval_ms']
                  and f['unix_ms'] <= after['unix_ms']]
        if not frames:
            raise RuntimeError('No complete profile intervals inside the measurement window')
        result = summarize(frames, before, after)
        result.update(actions=actions, distinct_states=len(states), distinct_camera_targets=len(cameras),
                      camera_target_range=max((max(c) - min(c) for c in zip(*cameras)), default=0), requested_seconds=seconds,
                      elapsed_seconds=elapsed, measurement_start_unix_ms=before['unix_ms'],
                      measurement_end_unix_ms=after['unix_ms'], display=self.display,
                      action_count_kind='scheduled' if self.display == 'wayland' else 'dispatched',
                      input_start_delay_seconds=input_lead,
                      profile_includes_warmup=bool(profile_to))
        if keys and len(states) < 2:
            raise RuntimeError(f'{capture}: shortcuts did not change the workspace; see {self.root}')
        if capture == 'navigate' and getattr(self, 'expect_camera', False) and len(cameras) < 2:
            raise RuntimeError(f'{capture}: wide panels did not exercise camera pans; see {self.root}')
        return result

    def close(self):
        try:
            for process in reversed(self.processes):
                self._stop(process)
        finally:
            for log in self.logs:
                log.close()
            if self.runtime_alias:
                self.runtime_alias.cleanup()


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
    parser.add_argument('--panel-width', type=float, choices=(0.25, 0.5, 0.75),
                        help='set each panel to a real preset through shortcuts before measuring')
    parser.add_argument('--layout-mode', choices=('normal', 'folder_tabs'), default='normal')
    parser.add_argument('--transcript', choices=('all', 'long-history', 'streaming', 'empty'), default='all')
    parser.add_argument('--seed', type=int, default=7)
    parser.add_argument('--baseline', type=Path, help='summary.json of an earlier run to compare against')
    parser.add_argument('--tolerance', type=float, default=0.15,
                        help='fraction a metric may worsen against the baseline before failing')
    parser.add_argument('--display', choices=('xvfb', 'wayland'), default='xvfb',
                        help='xvfb: software rasterizer (CPU cost only). wayland: the real GPU on a '
                             'private headless Sway at 120 Hz, for true frame pacing')
    parser.add_argument('--sway-prefix', type=Path, default=Path('target/headless-sway-tools'),
                        help='extracted Sway prefix containing usr/bin and usr/lib, if Sway is not installed')
    parser.add_argument('--profile', action='store_true',
                        help='record a samply CPU profile of each measured scenario (needs samply)')
    args = parser.parse_args()
    if not math.isfinite(args.seconds) or args.seconds < 3 or args.repeats < 1:
        parser.error('need at least 3 seconds and one repeat')
    binary = args.binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    scenarios = args.scenario or list(SCENARIOS)
    retentions = {'on': [True], 'off': [False], 'both': [True, False]}[args.retention]
    with binary.open('rb') as file:
        digest = hashlib.file_digest(file, 'sha256').hexdigest()
    meta = dict(binary=str(binary), sha256=digest, scenarios=scenarios, repeats=args.repeats,
                seconds=args.seconds, panels=args.panels, panel_width=args.panel_width, layout_mode=args.layout_mode,
                transcript=args.transcript, seed=args.seed,
                display=args.display, private_compositor=True, measurement_version=2,
                mode=os.environ.get('PERF_SUITE_MODE', '2880x1800@120Hz') if args.display == 'wayland' else '1440x1000',
                scale=os.environ.get('PERF_SUITE_SCALE', '1.6') if args.display == 'wayland' else '1',
                renderer=os.environ.get('WLR_RENDERER', 'gles2') if args.display == 'wayland' else 'lavapipe',
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
                app = App(root / tag, binary, retention, args.panels, rows, args.transcript, args.display,
                          args.sway_prefix.resolve() if args.sway_prefix.exists() else None, args.panel_width, args.layout_mode)
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
        baseline_meta = args.baseline.parent / 'capture.json'
        if baseline_meta.exists():
            previous = json.loads(baseline_meta.read_text())
            mismatch = [name for name in ('panels', 'panel_width', 'layout_mode', 'transcript', 'display', 'mode', 'scale')
                        if name in previous and previous[name] != meta[name]]
            if mismatch:
                print('\nWarning: baseline workload differs in ' + ', '.join(mismatch)
                      + '. This comparison is exploratory, not like-for-like.')
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
