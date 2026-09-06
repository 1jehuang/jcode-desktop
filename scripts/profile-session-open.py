#!/usr/bin/env python3
"""Click persisted history in an isolated native desktop with a real daemon/bridge.

Never builds, inherits credentials, or submits a prompt. Each sample opens a
separate persisted session for the first time. Diagnostic timings end at render
state, not photons. OCR timings are conservative screenshot-observation upper
bounds, including capture/polling overhead but excluding recognition time.
"""
import argparse
import csv
from datetime import datetime, timezone, timedelta
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import statistics
import subprocess
import time
from screenshot import isolated_env

_spec = importlib.util.spec_from_file_location('panel_profile', Path(__file__).with_name('profile-panel-spawn.py'))
_panel = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_panel)
navigation, panels = _panel.navigation, _panel.panels
MARKER = 'Persisted history marker is visible'


def seed_sessions(root, count, messages):
    directory = root / 'jcode/sessions'
    directory.mkdir(parents=True)
    fixtures = []
    for index in range(count):
        session_id = f'session_profile_history_{index:03d}'
        title = 'HISTORY ' + ('ALPHA', 'BRAVO', 'CHARLIE', 'DELTA', 'ECHO', 'FOXTROT', 'GOLF', 'HOTEL')[index]
        stamp = (datetime.now(timezone.utc) - timedelta(minutes=index + 1)).isoformat()
        contents = [dict(id=f'message_{n}', role='user' if n % 2 == 0 else 'assistant',
                         content=[dict(type='text', text=(MARKER if n == messages - 1 else
                                  f'Persisted transcript entry {n + 1}. This is offline fixture text.'))],
                         timestamp=stamp) for n in range(messages)]
        data = dict(id=session_id, parent_id=None, title=title, custom_title=title,
                    created_at=stamp, updated_at=stamp, messages=contents,
                    working_dir=str(root), status='Closed')
        (directory / (session_id + '.json')).write_text(json.dumps(data) + '\n')
        fixtures.append(dict(session_id=session_id, title=title))
    return fixtures


def ocr_lines(tsv):
    """Keep OCR line coordinates, avoiding assumed sidebar row positions."""
    groups = {}
    for word in csv.DictReader(io.StringIO(tsv), delimiter='\t'):
        if not word.get('text', '').strip():
            continue
        key = tuple(word[k] for k in ('page_num', 'block_num', 'par_num', 'line_num'))
        groups.setdefault(key, []).append(word)
    return [dict(text=' '.join(w['text'] for w in words),
                 x=min(int(w['left']) for w in words),
                 y=min(int(w['top']) for w in words),
                 height=max(int(w['height']) for w in words)) for words in groups.values()]


def history_ready(panel, minimum_items):
    return panel.get('history_loaded') is True and panel.get('history_items', 0) >= minimum_items


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return dict(path=str(path), sha256=digest.hexdigest(), bytes=path.stat().st_size)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/jcode-desktop'))
    parser.add_argument('--jcode', type=Path)
    parser.add_argument('--samples', type=int, default=5)
    parser.add_argument('--messages', type=int, default=20)
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('--history-signal', choices=('auto', 'ocr', 'diagnostic'), default='auto')
    args = parser.parse_args()
    if not 1 <= args.samples <= 8 or args.messages < 2 or args.timeout <= 0:
        parser.error('samples must be 1..8, messages >= 2, timeout positive')
    for tool in ('Xvfb', 'openbox', 'xdotool', 'import', 'tesseract'):
        if not shutil.which(tool):
            parser.error(f'missing {tool}')
    drivers = sorted(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if not drivers:
        parser.error('Mesa lavapipe is required')
    binary = args.binary.resolve(strict=True)
    runtime = args.jcode or (Path(shutil.which('jcode')) if shutil.which('jcode') else None)
    if runtime is None:
        parser.error('pass --jcode or install jcode')
    runtime = runtime.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
        (root / name).mkdir(mode=0o700)
    fixtures = seed_sessions(root, args.samples, args.messages)
    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    env.update(JCODE_NO_TELEMETRY='1', JCODE_RUNTIME_DIR=str(root / 'runtime'),
               JCODE_API_SOCKET=str(root / 'runtime/api.sock'),
               JCODE_SOCKET=str(root / 'runtime/daemon.sock'),
               JCODE_DESKTOP_CONFIG=str(root / 'desktop.toml'),
               VK_DRIVER_FILES=str(drivers[0]), PATH=str(runtime.parent) + ':/usr/bin:/bin')
    (root / 'desktop.toml').write_text('[workspace]\ncoaching_hints = false\n')
    result = dict(binary=fingerprint(binary), daemon=fingerprint(runtime), messages=args.messages,
                  samples=[], history_signal=args.history_signal,
                  timing_note='Render-state times are not physical presentation. Pixel times are OCR polling upper bounds, not precise latency.',
                  isolation='private Xvfb, HOME, XDG, daemon and bridge')
    (root / 'setup.json').write_text(json.dumps(result, indent=2) + '\n')
    processes, logs = [], []
    read_fd, write_fd = os.pipe()

    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(command, cwd=root, env=env, stdout=log, stderr=log,
                                   start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait(predicate):
        deadline = time.perf_counter() + args.timeout
        while True:
            value = predicate()
            if value:
                return value
            if any(p.poll() is not None for p in processes):
                raise RuntimeError('Child exited. See logs in ' + str(root))
            if time.perf_counter() >= deadline:
                raise TimeoutError('See logs/state in ' + str(root))
            time.sleep(.002)

    def ready(path):
        try:
            with socket.socket(socket.AF_UNIX) as client:
                client.connect(path)
            return True
        except OSError:
            return False

    def capture(name):
        path = root / name
        subprocess.run(['import', '-window', 'root', str(path)], env=env, check=True, timeout=10)
        observed = time.perf_counter()
        tsv = subprocess.check_output(['tesseract', str(path), 'stdout', '--psm', '11', 'tsv'],
                                      env=env, timeout=15, stderr=subprocess.DEVNULL).decode()
        return ocr_lines(tsv), observed

    def key(value):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '0', value],
                       env=env, check=True, timeout=10)

    try:
        launch('daemon', [str(runtime), '--no-update', '--no-selfdev', '--provider', 'jcode', 'serve'])
        wait(lambda: ready(env['JCODE_SOCKET']))
        launch('bridge', [str(runtime), '--no-update', '--no-selfdev', 'api-bridge'])
        wait(lambda: ready(env['JCODE_API_SOCKET']))
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1440x1000x24', '-nolisten', 'tcp'],
               pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        if not select.select([read_fd], [], [], 15)[0]:
            raise TimeoutError('Xvfb did not allocate display')
        display = os.read(read_fd, 64).decode().strip()
        if not display.isdigit():
            raise RuntimeError('Invalid private display')
        env['DISPLAY'] = ':' + display
        wm = root / 'openbox.xml'
        wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm)])
        time.sleep(.5)
        launch('desktop', [str(binary), '--no-hot-reload'])
        state_path = root / 'state'
        wait(lambda: panels(navigation(state_path)))
        time.sleep(2)
        for index, fixture in enumerate(fixtures):
            lines, _ = capture(f'before-{index}.png')
            matches = [line for line in lines if line['x'] < 264 and line['text'].endswith(fixture['title'])]
            if len(matches) != 1:
                raise RuntimeError(f'Cannot locate history title {fixture["title"]!r}: {lines}')
            line = matches[0]
            before = panels(navigation(state_path))
            if any(p.get('session') == fixture['session_id'] for p in before):
                raise RuntimeError('Sample must begin with persisted session not open')
            subprocess.run(['xdotool', 'mousemove', '100', str(line['y'] + line['height'] // 2)], env=env, check=True)
            started = time.perf_counter()
            subprocess.run(['xdotool', 'click', '--clearmodifiers', '--delay', '0', '1'], env=env, check=True, timeout=10)
            def selected():
                return next((p for p in panels(navigation(state_path))
                             if p.get('session') == fixture['session_id'] and p.get('focused')), None)
            shown = wait(selected)
            panel_ms = (time.perf_counter() - started) * 1000
            signal_name = args.history_signal
            if signal_name == 'auto':
                signal_name = 'diagnostic' if 'history_loaded' in shown else 'ocr'
            diagnostic_ms = None
            if signal_name == 'diagnostic':
                if 'history_loaded' not in shown or 'history_items' not in shown:
                    raise RuntimeError('Binary lacks history diagnostics. Use --history-signal ocr or rebuild with diagnostics.')
                wait(lambda: (p if (p := selected()) and history_ready(p, args.messages) else None))
                diagnostic_ms = (time.perf_counter() - started) * 1000
            # Pixel evidence is always required, even when diagnostics are available.
            def marker_visible():
                lines, captured = capture(f'history-{index}.png')
                return captured if any(MARKER.lower() in line['text'].lower() and line['x'] > 264 for line in lines) else None
            visible = wait(marker_visible)
            state = navigation(state_path)
            current = selected()
            if not current or state.get('keyboard_panel') != current.get('slot'):
                raise RuntimeError('History open lost composer focus')
            sample = dict(sample=index, **fixture, panel_render_state_ms=panel_ms,
                          history_render_state_ms=diagnostic_ms,
                          history_after_panel_state_ms=(diagnostic_ms - panel_ms if diagnostic_ms is not None else None),
                          history_pixel_observation_ms=(visible - started) * 1000,
                          history_signal=signal_name, history_items=current.get('history_items'),
                          keyboard_focus_verified=True, marker_verified=True)
            result['samples'].append(sample)
            (root / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(sample), flush=True)
            key('ctrl+shift+w')
            wait(lambda: not any(p.get('session') == fixture['session_id'] for p in panels(navigation(state_path))))
            time.sleep(.3)
        for field in ('panel_render_state_ms', 'history_render_state_ms', 'history_pixel_observation_ms'):
            values = [s[field] for s in result['samples'] if s[field] is not None]
            if values:
                result[field.replace('_ms', '_median_ms')] = statistics.median(values)
        result['success'] = True
    except Exception as error:
        result.update(success=False, error=str(error))
        raise
    finally:
        (root / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        for log in logs:
            log.close()
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
