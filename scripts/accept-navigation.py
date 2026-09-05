#!/usr/bin/env python3
"""Check native keyboard navigation and state across real UI hot reloads.

Runs real SDK sessions without inference on a private Xvfb display. Saves a
JSONL state trace and screenshot. Never sends input to the user's display.
Requires current desktop/UI binaries, jcode, cargo, Xvfb, Openbox, xdotool,
ImageMagick, and Mesa lavapipe. Build first with cargo build --workspace.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import time

from screenshot import isolated_env


def navigation_state(path):
    """The writer can be between truncate and write, so retry partial snapshots."""
    try:
        line = next(line for line in path.read_text().splitlines()
                    if line.startswith('navigation='))
        return json.loads(line.partition('=')[2])
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


def assert_state(state, row, position, sessions=None):
    assert state['version'] == 1
    assert state['active_row'] == row, state
    panels = state['rows'][row]['panels']
    slot = panels[position]['slot'] if position is not None else None
    assert state['focused_slot'] == slot, state
    assert state['keyboard_panel'] == slot, 'Keyboard focus diverged from map focus'
    focused = [p['slot'] for r in state['rows'] for p in r['panels'] if p['focused']]
    assert focused == ([] if slot is None else [slot]), 'Map must have exactly one focus, or none on an empty row'
    if slot is not None:
        assert state['rows'][row]['remembered'] == slot
    if sessions is not None:
        assert [p['session'] for p in panels] == sessions, 'Navigation changed the panel order'


def assert_reload_healthy(log):
    assert not any(error in log for error in (
        'UI rebuild failed', 'UI reload failed', 'initial UI plugin activation failed',
    )), log


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path,
                        help='desktop host executable, including an existing live host for compatibility checks')
    parser.add_argument('--reloads', type=int, default=2)
    parser.add_argument('--compact-tabs', action='store_true',
                        help='use 12 real sessions at 800px and click every top tab without scrolling')
    parser.add_argument('--linked-ui', action='store_true',
                        help='exercise the linked UI instead of a hot-reload plugin (requires --reloads 0)')
    parser.add_argument('--build-timeout', type=int, default=600,
                        help='seconds allowed for startup/reload builds, including Cargo lock waits')
    args = parser.parse_args()
    panel_count = 12 if args.compact_tabs else 4
    screen = '800x700x24' if args.compact_tabs else '1800x1000x24'
    if not 0 <= args.reloads <= 10:
        parser.error('--reloads must be between 0 and 10')
    if args.linked_ui and args.reloads:
        parser.error('--linked-ui requires --reloads 0')
    if args.build_timeout <= 0:
        parser.error('--build-timeout must be positive')
    repo = Path(__file__).resolve().parents[1]
    root = args.output.resolve()
    if len(str(root / 'runtime/daemon.sock').encode()) >= 104:
        parser.error('output path is too long for a private Unix socket')
    for name in ('jcode', 'cargo', 'Xvfb', 'openbox', 'xdotool', 'import'):
        if not shutil.which(name):
            parser.error('missing executable: ' + name)
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    # A PATH shim can resolve its real Cargo relative to HOME. HOME is private
    # here, so prefer the explicit rustup proxy without resolving its symlink.
    cargo = cargo_home / 'bin/cargo'
    if not cargo.is_file():
        cargo = Path(shutil.which('cargo'))
    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    env.update({
        'JCODE_NO_TELEMETRY': '1',
        'JCODE_RUNTIME_DIR': str(root / 'runtime'),
        'JCODE_API_SOCKET': str(root / 'runtime/api.sock'),
        'JCODE_SOCKET': str(root / 'runtime/daemon.sock'),
        'JCODE_TEMP_SERVER': '1',
        'JCODE_SERVER_OWNER_PID': str(os.getpid()),
        'JCODE_TEMP_SERVER_IDLE_SECS': str(args.build_timeout + 60),
        'JCODE_DESKTOP_CONFIG': str(root / 'desktop.toml'),
        'VK_DRIVER_FILES': str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))),
        # The host's public Ctrl+R action builds the UI. Reuse only build-tool
        # homes, not Jcode credentials, desktop sockets or user app settings.
        'CARGO': str(cargo),
        'CARGO_HOME': str(cargo_home),
        'RUSTUP_HOME': os.environ.get('RUSTUP_HOME', str(Path.home() / '.rustup')),
        'CARGO_NET_OFFLINE': 'true',
        # Reload generations and linker temporaries are large. Keep them with
        # this run's disk-backed artifacts, not the user's quota-limited /tmp.
        'TMPDIR': str(root / 'tmp'),
    })
    jcode = shutil.which('jcode')
    env['PATH'] = ':'.join([str(cargo.parent), str(Path(jcode).parent), '/usr/bin', '/bin'])
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode', 'tmp'):
        (root / name).mkdir(mode=0o700)
    (root / 'desktop.toml').write_text('[workspace]\ncoaching_hints = false\n')
    processes, logs = [], []
    state_path = root / 'state'
    diagnostics = root / 'logs/jcode-desktop/jcode-desktop.log'

    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(command, cwd=root, env=env, stdout=log,
                                   stderr=log, start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait_until(predicate, label, timeout=60):
        deadline = time.monotonic() + timeout
        report_at = time.monotonic() + 20
        while True:
            assert all(p.poll() is None for p in processes), 'Child process exited: ' + label
            if diagnostics.exists():
                assert_reload_healthy(diagnostics.read_text())
            if predicate():
                return
            if time.monotonic() >= deadline:
                raise AssertionError('Timed out: ' + label + '\n' + (state_path.read_text() if state_path.exists() else 'No state'))
            if time.monotonic() >= report_at:
                print('Waiting for ' + label, flush=True)
                report_at = time.monotonic() + 20
            time.sleep(.1)

    def socket_ready(path):
        try:
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(.2)
                client.connect(path)
            return True
        except OSError:
            return False

    def key(chord):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', chord], env=env, check=True, timeout=10)
        time.sleep(.25)

    def check(label, row, position, sessions=None):
        def ready():
            state = navigation_state(state_path)
            if state is None:
                return False
            try:
                assert_state(state, row, position, sessions)
                return True
            except AssertionError:
                return False
        wait_until(ready, label, timeout=5)
        state = navigation_state(state_path)
        assert state is not None, 'Missing structured navigation state'
        assert_state(state, row, position, sessions)
        with (root / 'navigation.jsonl').open('a') as trace:
            trace.write(json.dumps({'checkpoint': label, 'state': state}) + '\n')
        print(f'{label}: row={row} position={position}', flush=True)
        return state

    read_fd, write_fd = os.pipe()
    try:
        launch('daemon', [jcode, '--no-update', '--no-selfdev', '--provider', 'jcode', 'serve'])
        wait_until(lambda: socket_ready(env['JCODE_SOCKET']), 'daemon')
        launch('bridge', [jcode, '--no-update', '--no-selfdev', 'api-bridge'])
        wait_until(lambda: socket_ready(env['JCODE_API_SOCKET']), 'API bridge')
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', screen, '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0], 'Xvfb startup timeout'
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit()
        env['DISPLAY'] = ':' + display
        wm = root / 'openbox.xml'
        wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm)])
        launch('app', [str(args.binary.absolute() if args.binary else repo / 'target/debug/jcode-desktop')]
               + (['--no-hot-reload'] if args.linked_ui else ['--hot-reload']))
        wait_until(lambda: (s := navigation_state(state_path)) is not None and len(s['rows'][0]['panels']) == 1,
                   'first real session (including startup build)', args.build_timeout)
        time.sleep(1)
        key('super+1')
        for count in range(2, panel_count + 1):
            key('super+n')
            wait_until(lambda: (s := navigation_state(state_path)) is not None and len(s['rows'][0]['panels']) == count, f'{count} sessions')
            key('super+1')
        initial = check('created', 0, panel_count - 1)
        sessions = [p['session'] for p in initial['rows'][0]['panels']]
        assert len(set(sessions)) == panel_count and all(s.startswith('session_') for s in sessions)

        for generation in range(args.reloads + 1):
            if generation:
                before = diagnostics.read_text().count('activated UI generation')
                key('ctrl+r')
                wait_until(lambda: diagnostics.read_text().count('activated UI generation') > before,
                           'Ctrl+R hot reload', args.build_timeout)
                time.sleep(.5)
            # Every position, both edges and reversals. One chord means one hop.
            key('super+u')
            check(f'g{generation}-first', 0, 0, sessions)
            for chord, positions in [('super+l', [*range(1, panel_count), panel_count - 1]),
                                     ('super+h', [*range(panel_count - 2, -1, -1), 0]),
                                     ('ctrl+Tab', range(1, panel_count)),
                                     ('ctrl+shift+Tab', range(panel_count - 2, -1, -1))]:
                for position in positions:
                    key(chord)
                    check(f'g{generation}-{chord}-{position}', 0, position, sessions)
            key('super+p')
            check(f'g{generation}-last', 0, panel_count - 1, sessions)
            key('super+Tab')
            check(f'g{generation}-previous', 0, 0, sessions)
            key('super+j')
            check(f'g{generation}-empty', 1, None, [])
            key('super+l')
            check(f'g{generation}-empty-right', 1, None, [])
            key('super+k')
            check(f'g{generation}-return', 0, 0, sessions)
            if args.compact_tabs:
                geometry = subprocess.check_output(
                    ['xdotool', 'getactivewindow', 'getwindowgeometry', '--shell'],
                    env=env, text=True, timeout=10)
                (root / f'geometry-g{generation}.txt').write_text(geometry)
                subprocess.run(['import', '-window', 'root',
                                str(root / f'compact-tabs-before-g{generation}.png')],
                               env=env, check=True, timeout=15)
                # Public native hit-testing, not fixture data or injected app
                # actions. Twelve targets share the 512px row between the
                # sidebar/gutter and right margin. No wheel events are sent.
                for position in [*range(panel_count), 0]:
                    x = round(276 + (position + .5) * 512 / panel_count)
                    subprocess.run(['xdotool', 'mousemove', str(x), '40'],
                                   env=env, check=True, timeout=10)
                    time.sleep(.1)
                    subprocess.run(['xdotool', 'click', '1'],
                                   env=env, check=True, timeout=10)
                    time.sleep(.25)
                    check(f'g{generation}-tab-click-{position}', 0, position, sessions)
                subprocess.run(['import', '-window', 'root',
                                str(root / f'compact-tabs-g{generation}.png')],
                               env=env, check=True, timeout=15)
        subprocess.run(['import', '-window', 'root', str(root / 'navigation.png')], env=env, check=True, timeout=15)
        print(f'PASS: {panel_count} real sessions, {args.reloads} hot reloads, native keys'
              f'{" and all compact tab clicks" if args.compact_tabs else ""}, consistent map/focus state. {root}', flush=True)
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
