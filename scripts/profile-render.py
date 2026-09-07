#!/usr/bin/env python3
"""Measure genuine native panel animation on an isolated Xvfb software renderer.

Uses the screenshot fixture and passive live-profile instrumentation. This is
CPU/software-renderer evidence, not a claim about the user's GPU presentation.
No daemon, credentials, compositor commands, or input to the user's display.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import statistics
import subprocess
import time

from screenshot import isolated_env

spec = importlib.util.spec_from_file_location('profile_live', Path(__file__).with_name('profile-live.py'))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


def summarize(frames, before, after):
    seconds = sum(frame['interval_ms'] for frame in frames) / 1000
    draws = sum(frame['draw_count'] for frame in frames)
    cpu_seconds = (after['main_cpu_ticks'] - before['main_cpu_ticks']) / os.sysconf('SC_CLK_TCK')
    elapsed = (after['unix_ms'] - before['unix_ms']) / 1000
    def maximum(field):
        values = [f[field] for f in frames if f[field] is not None]
        return max(values) if values else None
    return dict(sample_windows=len(frames), sampled_seconds=seconds, draws=draws,
                draw_rate=draws / seconds if seconds else None,
                process_cpu_percent=cpu_seconds / elapsed * 100 if elapsed else None,
                draw_max_ms=maximum('draw_max_ms'),
                draw_window_p95_median_ms=statistics.median(
                    [f['draw_p95_ms'] for f in frames if f['draw_p95_ms'] is not None]) if draws else None,
                animation_present_count=sum(f['animation_present_count'] for f in frames),
                animation_present_max_window_p95_ms=maximum('animation_present_p95_ms'),
                input_frames=sum(f['input_frame_count'] for f in frames),
                input_max_ms=maximum('input_max_ms'),
                wake_max_ms=maximum('ui_wake_lag_ms'))


def settled_draw_summary(frames, started_unix_ms):
    # Exclude intended animation and a full sampling interval at its boundary.
    settled = [f for f in frames if f['unix_ms'] >= started_unix_ms + 1500]
    if not settled:
        raise RuntimeError('No post-transition samples')
    return dict(settled_draws=sum(f['draw_count'] for f in settled),
                settled_sample_windows=len(settled))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/jcode-desktop'))
    parser.add_argument('--seconds', type=float, default=8)
    parser.add_argument('--uncached-panels', action='store_true', help='same-binary offline control without the panel cache')
    parser.add_argument('--scalar-inline', action='store_true', help='same-binary offline control without bulk inline prose scanning')
    parser.add_argument('--scenario', choices=('overview', 'focus-switch', 'overview-after-resize'), default='overview')
    parser.add_argument('--stale-hidden-animations', action='store_true', help='same-binary offline control retaining hidden animation flags')
    parser.add_argument('--panels', type=int, choices=range(2, 7), default=4)
    parser.add_argument('--transcript', choices=('all', 'empty', 'reasoning'), default='all')
    args = parser.parse_args()
    if args.seconds < 3:
        parser.error('seconds must be at least 3')
    binary = args.binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env['VK_DRIVER_FILES'] = str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json')))
    env['JCODE_DESKTOP_SCREENSHOT_PANELS'] = str(args.panels)
    if args.uncached_panels:
        env['JCODE_DESKTOP_SCREENSHOT_UNCACHED_PANELS'] = '1'
    if args.scalar_inline:
        env['JCODE_DESKTOP_SCREENSHOT_SCALAR_INLINE'] = '1'
    if args.stale_hidden_animations:
        env['JCODE_DESKTOP_SCREENSHOT_STALE_HIDDEN_ANIMATIONS'] = '1'
    env['JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT'] = args.transcript
    env['JCODE_DESKTOP_CONFIG'] = str(root / 'desktop.toml')
    (root / 'desktop.toml').write_text('[appearance]\nlayout_mode = "folder_tabs"\n[workspace]\ncoaching_hints = false\n')
    with binary.open('rb') as file:
        binary_hash = hashlib.file_digest(file, 'sha256').hexdigest()
    (root / 'capture.json').write_text(json.dumps(dict(
        binary=str(binary), sha256=binary_hash, uncached_panels=args.uncached_panels,
        scalar_inline=args.scalar_inline,
        stale_hidden_animations=args.stale_hidden_animations,
        scenario=args.scenario, seconds=args.seconds, panels=args.panels,
        transcript=args.transcript, renderer=env['VK_DRIVER_FILES']), indent=2) + '\n')
    processes, logs = [], []
    read_fd, write_fd = os.pipe()
    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process
    try:
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1440x1000x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        if not select.select([read_fd], [], [], 15)[0]:
            raise RuntimeError('Xvfb did not start')
        env['DISPLAY'] = ':' + os.read(read_fd, 64).decode().strip()
        config = root / 'openbox.xml'
        config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(config)])
        time.sleep(.5)
        app = launch('app', [str(binary), '--no-hot-reload'])
        time.sleep(4)
        results = {}
        for scenario in ('idle', args.scenario):
            capture = scenario.replace('-', '')
            control = root / 'runtime/jcode-desktop-profile.json'
            control.write_text(json.dumps(dict(capture_id=capture, until_unix_ms=int((time.time() + args.seconds + 3) * 1000))))
            time.sleep(1.2)
            before = profile.process_sample(app.pid)
            started = time.monotonic()
            actions = 0
            observed_states = set()
            observed_widths = set()
            def observe_state():
                try:
                    state = json.loads(next(line[11:] for line in (root / 'state').read_text().splitlines() if line.startswith('navigation=')))
                    observed_states.add((state['overview'], state['focused_slot']))
                    observed_widths.add(tuple(panel['width'] for row in state['rows'] for panel in row['panels']))
                except (OSError, StopIteration, json.JSONDecodeError):
                    pass
            observe_state()
            while time.monotonic() - started < args.seconds:
                if app.poll() is not None:
                    raise RuntimeError('App exited, see app.log')
                if scenario == 'overview-after-resize' and actions == 0:
                    # Hide the strip before its width transition finishes, then
                    # stop all input. Expired hidden transitions must go idle.
                    subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '0', 'super+r', 'super+o'], env=env, check=True, timeout=10)
                    actions += 2
                elif scenario not in ('idle', 'overview-after-resize'):
                    # Alternating overview transitions exercise real layout,
                    # painting and text without manufacturing redraw events.
                    chord = 'super+o' if scenario == 'overview' else ('super+h' if actions % 2 == 0 else 'super+l')
                    subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '0', chord], env=env, check=True, timeout=10)
                    actions += 1
                time.sleep(.3)
                observe_state()
            after = profile.process_sample(app.pid)
            control.unlink()
            source = root / f'runtime/jcode-desktop-profile-{app.pid}-{capture}.jsonl'
            if not source.exists():
                raise RuntimeError('No profile samples, rebuild the binary with live profiling')
            frames = [json.loads(line) for line in source.read_text().splitlines() if line.endswith('}')]
            frames = [f for f in frames if before['unix_ms'] <= f['unix_ms'] <= after['unix_ms']]
            (root / (scenario + '-frames.json')).write_text(json.dumps(frames, indent=2) + '\n')
            results[scenario] = dict(summarize(frames, before, after), native_actions=actions, observed_states=sorted(observed_states))
            if scenario == 'overview-after-resize':
                results[scenario].update(settled_draw_summary(frames, before['unix_ms']))
                results[scenario]['observed_widths'] = sorted(observed_widths)
                if len(observed_widths) < 2:
                    raise RuntimeError('Native resize did not change panel width')
            if scenario != 'idle' and not results[scenario]['input_frames']:
                raise RuntimeError('No input-bearing frames, animation evidence is inconclusive')
            if scenario != 'idle' and len(observed_states) < 2:
                raise RuntimeError('Native actions did not change navigation state')
            if scenario == 'overview' and results[scenario]['animation_present_count'] <= actions:
                raise RuntimeError('No sustained active animation evidence')
            print(scenario, json.dumps(results[scenario]), flush=True)
            time.sleep(1.2)
        (root / 'summary.json').write_text(json.dumps(results, indent=2) + '\n')
        subprocess.run(['import', '-window', 'root', str(root / 'final.png')], env=env, check=True, timeout=20)
    finally:
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
