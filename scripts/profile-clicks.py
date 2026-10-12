#!/usr/bin/env python3
"""Measure UI-thread work caused by mouse clicks on an isolated Xvfb display.

Mirrors the issue #5 Windows repro (synthetic button down/up over the window
while the cursor moves) on Linux so click-path stalls can be attributed with
the passive live profiler. Reports input-to-frame and draw latency for an idle
baseline, a moves-only run, and a clicks run at the same spot.
No input reaches the user's display.
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import select
import shutil
import subprocess
import time

from screenshot import isolated_env

spec = importlib.util.spec_from_file_location('profile_live', Path(__file__).with_name('profile-live.py'))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


def thread_ticks(pid):
    ticks = {}
    for task in Path(f'/proc/{pid}/task').iterdir():
        try:
            stat = (task / 'stat').read_text()
        except OSError:
            continue
        name = stat[stat.index('(') + 1:stat.rindex(')')]
        fields = stat.rsplit(')', 1)[1].split()
        key = f'{name}#{task.name}'
        ticks[key] = int(fields[11]) + int(fields[12])
    return ticks


def busiest_threads(before, after, limit=8):
    hz = os.sysconf('SC_CLK_TCK')
    grouped = {}
    for key, value in after.items():
        name = key.split('#')[0]
        grouped[name] = grouped.get(name, 0) + value - before.get(key, 0)
    ranked = sorted(((n, t) for n, t in grouped.items() if not n.startswith("llvmpipe")), key=lambda item: -item[1])[:limit]
    return [(name, round(ticks / hz, 2)) for name, ticks in ranked if ticks]


def summarize(frames):
    def values(field):
        return [f[field] for f in frames if f.get(field) is not None]
    def top(field):
        found = values(field)
        return max(found) if found else None
    views = {}
    for frame in frames:
        for name, renders, total_ms, _ in frame.get('views', []):
            entry = views.setdefault(name, [0, 0.0])
            entry[0] += renders
            entry[1] += total_ms
    busiest = sorted(views.items(), key=lambda item: -item[1][1])[:8]
    return dict(
        draws=sum(f['draw_count'] for f in frames),
        draw_max_ms=top('draw_max_ms'),
        draw_p95_max_ms=top('draw_p95_ms'),
        input_frames=sum(f['input_frame_count'] for f in frames),
        input_max_ms=top('input_max_ms'),
        input_p95_max_ms=top('input_p95_ms'),
        wake_max_ms=top('ui_wake_lag_ms'),
        views=[(name, renders, round(ms, 1)) for name, (renders, ms) in busiest],
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=Path('target/release/jcode-desktop'))
    parser.add_argument('--seconds', type=float, default=10)
    parser.add_argument('--transcript', default='all')
    parser.add_argument('--target', default='center',
                        help="click spot: center, composer, sidebar, header, or X,Y in window pixels")
    parser.add_argument('--scenarios', default='idle,moves,clicks',
                        help='comma-separated: idle, moves, clicks, single')
    parser.add_argument('--stacks', action='store_true',
                        help='sample main-thread stacks with eu-stack during the clicks scenario')
    parser.add_argument('--samply', action='store_true',
                        help='record the app with samply (clicks scenario dominates the profile)')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    icds = sorted(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if icds:
        env['VK_DRIVER_FILES'] = str(icds[0])
    env['JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT'] = args.transcript
    env['JCODE_DESKTOP_SCREENSHOT_PANELS'] = '1'
    env['JCODE_DESKTOP_CONFIG'] = str(root / 'desktop.toml')
    (root / 'desktop.toml').write_text('[appearance]\nlayout_mode = "folder_tabs"\n[workspace]\ncoaching_hints = false\n')
    with binary.open('rb') as file:
        digest = hashlib.file_digest(file, 'sha256').hexdigest()
    processes, logs = [], []
    read_fd, write_fd = os.pipe()

    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        if name == 'app' and args.stacks:
            def allow_ptrace():
                import ctypes
                ctypes.CDLL(None).prctl(0x59616d61, ctypes.c_ulong(-1 & 0xffffffffffffffff), 0, 0, 0)
            kwargs['preexec_fn'] = allow_ptrace
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process

    def xdo(*command):
        subprocess.run(['xdotool', *command], env=env, check=True, timeout=10)

    try:
        width, height = 1440, 1000
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', f'{width}x{height}x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        if not select.select([read_fd], [], [], 15)[0]:
            raise RuntimeError('Xvfb did not start')
        env['DISPLAY'] = ':' + os.read(read_fd, 64).decode().strip()
        config = root / 'openbox.xml'
        config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(config)])
        time.sleep(.5)
        command = [str(binary), '--no-hot-reload']
        if args.samply:
            command = [shutil.which('samply') or os.path.expanduser('~/.cargo/bin/samply'), 'record', '--save-only', '--no-open', '-o', str(root / 'profile.json.gz'), *command]
        app = launch('app', command)
        time.sleep(5)
        if args.samply:
            children = Path(f'/proc/{app.pid}/task/{app.pid}/children').read_text().split()
            app_pid = int(children[0])
        else:
            app_pid = app.pid
        xdo('key', 'Escape')
        time.sleep(1)
        spots = dict(center=(width // 2, height // 2), composer=(width // 2, height - 70),
                     sidebar=(140, height // 2), header=(width // 2, 16))
        cx, cy = spots.get(args.target) or tuple(int(n) for n in args.target.split(','))
        results = {}
        for scenario in args.scenarios.split(','):
            control = root / 'runtime/jcode-desktop-profile.json'
            control.write_text(json.dumps(dict(capture_id=scenario, until_unix_ms=int((time.time() + args.seconds + 3) * 1000))))
            time.sleep(1.2)
            before = profile.process_sample(app_pid)
            threads_before = thread_ticks(app_pid)
            started = time.monotonic()
            step = 0
            sampler = None
            if args.stacks and scenario == 'clicks':
                sampler = subprocess.Popen(['bash', '-c', f'while true; do eu-stack -1 -p {app_pid} -n 60 2>/dev/null; echo ===; sleep 0.003; done'],
                                           stdout=(root / 'stacks.txt').open('w'), stderr=subprocess.DEVNULL)
            while time.monotonic() - started < args.seconds:
                if app.poll() is not None:
                    raise RuntimeError('App exited, see app.log')
                if scenario == 'single':
                    # One still click, then quiet, so every frame it caused is attributable.
                    xdo('mousemove', str(cx), str(cy))
                    xdo('click', '1')
                    step += 1
                    time.sleep(2.0)
                    continue
                if scenario != 'idle':
                    angle = step * .15
                    radius = 1 if scenario == 'clicks' else 120
                    xdo('mousemove', str(int(cx + radius * math.cos(angle))), str(int(cy + radius * math.sin(angle))))
                    if scenario == 'clicks' and step % 40 == 0:
                        xdo('mousedown', '1')
                    elif scenario == 'clicks' and step % 40 == 30:
                        xdo('mouseup', '1')
                step += 1
                time.sleep(.008)
            if sampler:
                sampler.kill()
            after = profile.process_sample(app_pid)
            threads = busiest_threads(threads_before, thread_ticks(app_pid))
            control.unlink()
            source = root / f'runtime/jcode-desktop-profile-{app_pid}-{scenario}.jsonl'
            frames = [json.loads(line) for line in source.read_text().splitlines() if line.endswith('}')] if source.exists() else []
            frames = [f for f in frames if before['unix_ms'] <= f['unix_ms'] <= after['unix_ms']]
            cpu = (after['main_cpu_ticks'] - before['main_cpu_ticks']) / os.sysconf('SC_CLK_TCK')
            results[scenario] = dict(summarize(frames), cpu_seconds=round(cpu, 2), steps=step, threads=threads)
            (root / f'{scenario}-frames.json').write_text(json.dumps(frames, indent=2) + '\n')
            print(scenario, json.dumps(results[scenario]), flush=True)
            xdo('mouseup', '1')
            time.sleep(1.2)
        results['binary'] = dict(path=str(binary), sha256=digest, target=[cx, cy])
        (root / 'summary.json').write_text(json.dumps(results, indent=2) + '\n')
    finally:
        if args.samply and 'app_pid' in locals():
            subprocess.run(['kill', '-INT', str(app_pid)])
            try:
                processes[-1].wait(timeout=60)
            except subprocess.TimeoutExpired:
                pass
        for process in reversed(processes):
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for log in logs:
            log.close()
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)


if __name__ == '__main__':
    main()
