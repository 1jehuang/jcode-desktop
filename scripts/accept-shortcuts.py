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
    parser.add_argument('--check-attachments', action='store_true',
                        help='also verify the real companion API cwd and error contracts')
    parser.add_argument('--pinned-directory', type=Path,
                        help='explicit pinned directory to verify (sessions remain isolated)')
    parser.add_argument('--global-helper', type=Path,
                        help='exercise this installed global shortcut helper with private X11 adapters')
    parser.add_argument('--baseline-helper', type=Path,
                        help='also assert an older helper drops Enter before checking the fixed helper')
    args = parser.parse_args()
    if args.baseline_helper and not args.global_helper:
        parser.error('--baseline-helper requires --global-helper')
    for name in ('global_helper', 'baseline_helper', 'pinned_directory'):
        if value := getattr(args, name):
            setattr(args, name, value.resolve(strict=True))
    if args.pinned_directory and not args.pinned_directory.is_dir():
        parser.error('--pinned-directory must be a directory')
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
    pinned_directory = args.pinned_directory or favorite
    if args.pinned_directory:
        with config.open('a') as output:
            output.write('pinned_working_dir = ' + json.dumps(str(pinned_directory)) + '\n')
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
    if args.global_helper:
        # Exercise the actual helper without querying the user's compositor or
        # sending Wayland input. Window identity comes from the real private
        # X11 window. The virtual-keyboard adapter sends actual native keys.
        adapters = {
            'niri': '''#!/usr/bin/python3
import json, subprocess, sys
assert sys.argv[1:] == ['msg', '-j', 'focused-window']
active = subprocess.check_output(['xdotool', 'getactivewindow'], text=True).strip()
desktop = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--class', '^jcode-desktop$'], text=True).splitlines()
assert active in desktop, (active, desktop)
print(json.dumps({'app_id': 'jcode-desktop'}))
''',
            'wtype': '''#!/usr/bin/python3
import subprocess, sys
args = sys.argv[1:]
assert len(args) % 2 == 0
modifiers, chords = [], []
for flag, value in zip(args[::2], args[1::2]):
    if flag == '-M':
        modifiers.append(value)
    elif flag == '-m':
        modifiers.remove(value)
    elif flag == '-k':
        chords.append('+'.join([*modifiers, value]))
    else:
        raise AssertionError((flag, value))
assert not modifiers and len(chords) == 1, (modifiers, chords)
subprocess.run(['xdotool', 'key', '--clearmodifiers', chords[0]], check=True, timeout=10)
''',
        }
        for name, source in adapters.items():
            adapter = root / 'shims' / name
            adapter.write_text(source)
            adapter.chmod(0o755)
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

    def verify(chord, directory, stage, helper=None):
        before = panels()
        before_ids = {p['session'] for p in before}
        if helper:
            subprocess.run(['bash', str(helper), 'new'], env=env, check=True, timeout=10)
        else:
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
        wait(lambda: pinned() == str(pinned_directory), 'configured or most-used directory selection')
        if args.baseline_helper:
            before = panels()
            subprocess.run(['bash', str(args.baseline_helper), 'new'], env=env, check=True, timeout=10)
            time.sleep(.5)
            assert panels() == before, 'baseline helper unexpectedly created or selected a panel'
            evidence.append({'stage': 'baseline-helper', 'panels_before': len(before),
                             'panels_after': len(panels()), 'created': 0})
            print(json.dumps(evidence[-1]), flush=True)
        if args.global_helper:
            verify('global-helper:new', pinned_directory, 'installed-helper', args.global_helper)
        verify('super+Return', pinned_directory, 'enter')
        verify('ctrl+alt+Return', pinned_directory, 'forwarded-enter')
        verify('super+semicolon', pinned_directory, 'favorite')
        verify('super+apostrophe', root / 'home', 'home')
        # Make a different directory overwhelmingly more common, then restart
        # Desktop to exercise disk persistence, not just in-memory stability.
        seed('new-leader', other, 8)
        app.terminate()
        app.wait(timeout=15)
        (root / 'state').unlink(missing_ok=True)
        launch('restarted-app', [str(binary), '--no-hot-reload'])
        wait(connected, 'restarted runtime attachment', 90)
        assert pinned() == str(pinned_directory)
        verify('super+semicolon', pinned_directory, 'favorite-after-restart')
        verify('super+Return', pinned_directory, 'enter-after-restart')
        verify('ctrl+alt+Return', pinned_directory, 'forwarded-enter-after-restart')
        if args.global_helper:
            verify('global-helper:new', pinned_directory, 'installed-helper-after-restart', args.global_helper)
            # Real helper -> native alias -> workspace -> live session bridge.
            # Start on the right, where a fading successor used to strand focus.
            before_close = len(panels())
            for remaining in range(before_close - 1, -1, -1):
                subprocess.run(['bash', str(args.global_helper), 'close'], env=env,
                               check=True, timeout=10)
                def closed_one():
                    current = state()
                    if not current:
                        return False
                    live = [p for row in current['rows'] for p in row['panels']
                            if not p.get('closing')]
                    return (len(live) == remaining and
                            (not live or any(p['slot'] == current['keyboard_panel']
                                             == current['focused_slot'] for p in live)))
                wait(closed_one, 'helper closes to ' + str(remaining))
                evidence.append({'stage': 'installed-helper-close-step',
                                 'remaining': remaining, 'state': state()})
            wait(lambda: state() and not panels(), 'all closed slots retire')
            evidence.append({'stage': 'installed-helper-close-all',
                             'panels_before': before_close, 'panels_after': 0,
                             'keyboard_panel': state()['keyboard_panel']})
            print(json.dumps(evidence[-1]), flush=True)
            verify('global-helper:new', pinned_directory, 'installed-helper-reopen-after-close', args.global_helper)
            time.sleep(.5)
            assert len(panels()) == 1, 'new session closed after helper stopped'
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print('PASS: native keys created real sessions in the expected directories; pin survived changed history and restart.', flush=True)
        if args.check_attachments:
            from api_attachment_acceptance import verify as verify_attachments
            verify_attachments(root, env)
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
