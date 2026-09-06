#!/usr/bin/env python3
"""Verify native session shortcuts against a real, isolated Jcode runtime.

No provider requests are sent. Home, sockets, display, settings, and session
history are private. Requires an up-to-date target/debug/jcode-desktop binary.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import time
import tomllib

from screenshot import isolated_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='new evidence directory')
    parser.add_argument('--bridge', type=Path, help='freshly built jcode-harness-api-bridge')
    args = parser.parse_args()
    root = args.output.resolve()
    assert len(str(root / 'runtime/daemon.sock').encode()) < 104
    repo = Path(__file__).resolve().parents[1]
    binary = repo / 'target/debug/jcode-desktop'
    jcode = Path(shutil.which('jcode')).resolve()
    root.mkdir(parents=True, exist_ok=False)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode', 'shims'):
        (root / name).mkdir(mode=0o700)
    favorite, other = root / 'home/favorite', root / 'home/other'
    favorite.mkdir()
    other.mkdir()
    history = root / 'jcode/sessions'
    history.mkdir()

    def seed(prefix, directory, count):
        for index in range(count):
            (history / f'{prefix}-{index}.json').write_text(json.dumps({
                'working_dir': str(directory), 'title': f'{prefix} {index}',
                'messages': [],
            }))

    seed('favorite', favorite, 3)
    seed('other', other, 1)
    config = root / 'desktop.toml'
    config.write_text('[workspace]\ncoaching_hints = false\nsession_refresh_seconds = 5\n')
    shim = root / 'shims/jcode'
    if args.bridge:
        args.bridge = args.bridge.resolve(strict=True)
    shim.write_text(f'''#!/bin/sh
if [ "$1" = serve ]; then
  shift
  exec "{jcode}" --no-update --no-selfdev --provider jcode serve "$@"
fi
exec "{jcode}" "$@"
''')
    shim.chmod(0o755)
    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    env.update({
        'PATH': str(root / 'shims') + ':/usr/bin:/bin',
        'JCODE_NO_TELEMETRY': '1',
        'JCODE_RUNTIME_DIR': str(root / 'runtime'),
        'JCODE_API_SOCKET': str(root / 'runtime/api.sock'),
        'JCODE_SOCKET': str(root / 'runtime/daemon.sock'),
        'JCODE_TEMP_SERVER': '1',
        'JCODE_SERVER_OWNER_PID': str(os.getpid()),
        'JCODE_TEMP_SERVER_IDLE_SECS': '180',
        'JCODE_DESKTOP_CONFIG': str(config),
        'VK_DRIVER_FILES': str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))),
    })
    wm_config = root / 'openbox.xml'
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
    processes, logs, evidence = [], [], []

    def launch(name, command, **kwargs):
        log = (root / f'{name}.log').open('w')
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log,
                                   stderr=log, start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait(predicate, label, timeout=45):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(.05)
        raise AssertionError('Timed out: ' + label)

    def state():
        try:
            line = next(line for line in (root / 'state').read_text().splitlines()
                        if line.startswith('navigation='))
            return json.loads(line.partition('=')[2])
        except (FileNotFoundError, StopIteration, json.JSONDecodeError):
            return {}

    def panels():
        return [panel for row in state().get('rows', []) for panel in row['panels']]

    def connected():
        return panels() and all(p['session'].startswith('session_') for p in panels())

    def pinned():
        return tomllib.loads(config.read_text()).get('workspace', {}).get('pinned_working_dir')

    def created_session(session_id):
        # Read the real daemon's creation snapshot rather than attaching an
        # observer, which can itself affect legacy session lifecycle state.
        for log in (root / 'jcode/logs').glob('jcode-*.log'):
            for line in log.read_text().splitlines():
                if 'ENV_SNAPSHOT ' not in line:
                    continue
                record = json.loads(line.split('ENV_SNAPSHOT ', 1)[1])
                if record.get('session_id') == session_id and record.get('reason') == 'create':
                    return record
        return None

    def key(chord):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', chord], env=env,
                       check=True, timeout=10)

    def verify(chord, directory, stage):
        before = panels()
        before_ids = {p['session'] for p in before}
        key(chord)
        wait(lambda: len(panels()) == len(before) + 1 and connected(), chord)
        time.sleep(.3)
        after = panels()
        assert len(after) == len(before) + 1, 'one keypress must create exactly one panel'
        created = [p for p in after if p['session'] not in before_ids]
        assert len(created) == 1 and created[0]['focused'], after
        assert state()['keyboard_panel'] == created[0]['slot'], state()
        session = wait(lambda: created_session(created[0]['session']), 'daemon creation evidence')
        assert session['working_dir'] == str(directory), session
        evidence.append({'stage': stage, 'chord': chord, 'panels_before': len(before),
                         'panels_after': len(after), 'keyboard_panel': state()['keyboard_panel'],
                         'session': session['session_id'], 'working_dir': session['working_dir']})
        print(json.dumps(evidence[-1]), flush=True)
        subprocess.run(['import', '-window', 'root', str(root / f'{stage}.png')],
                       env=env, check=True, timeout=15)

    read_fd, write_fd = os.pipe()
    try:
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                        '1440x1000x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0]
        env['DISPLAY'] = ':' + os.read(read_fd, 64).decode().strip()
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm_config)])
        time.sleep(.5)
        if args.bridge:
            launch('daemon', [str(shim), 'serve'])
            wait(lambda: Path(env['JCODE_SOCKET']).exists(), 'private daemon socket', 90)
            launch('bridge', [str(args.bridge.resolve())])
            wait(lambda: Path(env['JCODE_API_SOCKET']).exists(), 'private API socket', 90)
        app = launch('app', [str(binary), '--no-hot-reload'])
        wait(connected, 'real runtime attachment', 90)
        wait(lambda: pinned() == str(favorite), 'initial most-used directory selection')
        verify('super+Return', root / 'home', 'enter')
        verify('super+semicolon', favorite, 'favorite')
        verify('super+apostrophe', root / 'home', 'home')
        # Make a different directory overwhelmingly more common, then restart
        # Desktop to exercise disk persistence, not just in-memory stability.
        seed('new-leader', other, 8)
        app.terminate()
        app.wait(timeout=15)
        (root / 'state').unlink(missing_ok=True)
        launch('restarted-app', [str(binary), '--no-hot-reload'])
        wait(connected, 'restarted runtime attachment', 90)
        assert pinned() == str(favorite)
        verify('super+semicolon', favorite, 'favorite-after-restart')
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print('PASS: native keys created real sessions in the expected directories; pin survived changed history and restart.', flush=True)
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        for process in reversed(processes):
            # The app may already have exited but its private daemon children
            # can still belong to this process group.
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                continue
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
