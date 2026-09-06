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


def assert_state(state, row, position, sessions=None, overview=False):
    assert state['version'] == 1
    assert state['active_row'] == row, state
    panels = state['rows'][row]['panels']
    slot = panels[position]['slot'] if position is not None else None
    assert state['focused_slot'] == slot, state
    assert state['overview'] == overview, 'Unexpected overview state'
    assert state['keyboard_panel'] == (None if overview else slot), 'Keyboard focus diverged from visible input'
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
    parser.add_argument('--jcode-binary', type=Path,
                        help='specific daemon executable for cross-repository runtime acceptance')
    parser.add_argument('--reloads', type=int, default=2)
    parser.add_argument('--extended-shortcuts', action='store_true',
                        help='also verify arrow/move/width aliases, home creation, and native Quit')
    parser.add_argument('--default-launch', action='store_true',
                        help='verify development hot reload without the --hot-reload flag')
    parser.add_argument('--compact-tabs', action='store_true',
                        help='use 12 real sessions at 800px and click every top tab without scrolling')
    parser.add_argument('--linked-ui', action='store_true',
                        help='exercise the linked UI instead of a hot-reload plugin (requires --reloads 0)')
    parser.add_argument('--build-timeout', type=int, default=600,
                        help='seconds allowed for startup/reload builds, including Cargo lock waits')
    args = parser.parse_args()
    if args.jcode_binary:
        args.jcode_binary = args.jcode_binary.expanduser().absolute()
        if not args.jcode_binary.is_file() or not os.access(args.jcode_binary, os.X_OK):
            parser.error('--jcode-binary must name an executable file')
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
        if name == 'jcode' and args.jcode_binary:
            continue
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
    # Extended service-panel checks must stay on the unconfigured direct
    # backend. isolated_env never inherits tokens or the user's credential home.
    env['JCODE_GMAIL_BACKEND'] = 'direct'
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
    jcode = str(args.jcode_binary.resolve(strict=True)) if args.jcode_binary else shutil.which('jcode')
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
            exited = [(p.pid, p.args, p.poll()) for p in processes if p.poll() is not None]
            assert not exited, f'Child process exited during {label}: {exited}'
            if diagnostics.exists():
                assert_reload_healthy(diagnostics.read_text())
            result = predicate()
            if result:
                return result
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

    def check(label, row, position, sessions=None, overview=False):
        def ready():
            state = navigation_state(state_path)
            if state is None:
                return False
            try:
                assert_state(state, row, position, sessions, overview)
                return state
            except AssertionError:
                return False
        # Use the exact valid snapshot. A second read can catch the next
        # diagnostic frame while the state file is being rewritten.
        state = wait_until(ready, label, timeout=5)
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
        app = launch('app', [str(args.binary.absolute() if args.binary else repo / 'target/debug/jcode-desktop')]
               + (['--no-hot-reload'] if args.linked_ui else
                  [] if args.default_launch else ['--hot-reload']))
        wait_until(lambda: (s := navigation_state(state_path)) is not None and len(s['rows'][0]['panels']) == 1,
                   'first real session (including startup build)', args.build_timeout)
        if not args.linked_ui:
            wait_until(lambda: diagnostics.exists() and 'activated UI generation' in diagnostics.read_text(),
                       'initial hot reload', args.build_timeout)
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
                key('super+o')
                check(f'g{generation}-overview-before-reload', 0, 0, sessions, overview=True)
                before = diagnostics.read_text().count('activated UI generation')
                key('ctrl+r')
                wait_until(lambda: diagnostics.read_text().count('activated UI generation') > before,
                           'Ctrl+R hot reload', args.build_timeout)
                time.sleep(.5)
                check(f'g{generation}-overview-after-reload', 0, 0, sessions, overview=True)
                key('super+o')
                check(f'g{generation}-input-after-reload', 0, 0, sessions)
            # Activation must identify the window, including after hot reload.
            # This queries only our private X11 display, not the compositor.
            identified = subprocess.check_output(
                ['xdotool', 'search', '--onlyvisible', '--class', '^jcode-desktop$'],
                env=env, text=True, timeout=10).splitlines()
            assert len(identified) == 1, identified
            # Every position, both edges and reversals. One chord means one hop.
            key('super+u')
            check(f'g{generation}-first', 0, 0, sessions)
            # People commonly keep Super held while moving across several
            # panels. Exercise that native modifier lifetime, not only chords
            # that release every modifier between presses. Do not clear the
            # modifiers here, which would hide a stuck/lost Super regression.
            subprocess.run(['xdotool', 'keydown', 'Super_L'], env=env, check=True, timeout=10)
            try:
                for direction, positions in [
                    ('l', [*range(1, panel_count), panel_count - 1]),
                    ('h', [*range(panel_count - 2, -1, -1), 0]),
                ]:
                    for position in positions:
                        subprocess.run(['xdotool', 'key', direction], env=env, check=True, timeout=10)
                        check(f'g{generation}-held-super-{direction}-{position}', 0, position, sessions)
            finally:
                subprocess.run(['xdotool', 'keyup', 'Super_L'], env=env, check=True, timeout=10)
            for chord, positions in [('super+l', [*range(1, panel_count), panel_count - 1]),
                                     ('super+h', [*range(panel_count - 2, -1, -1), 0]),
                                     ('ctrl+Tab', range(1, panel_count)),
                                     ('ctrl+shift+Tab', range(panel_count - 2, -1, -1)),
                                     ('ctrl+Page_Down', range(1, panel_count)),
                                     ('ctrl+Page_Up', range(panel_count - 2, -1, -1))]:
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
            key('super+o')
            # Wait until the panel inputs have actually unmounted. Dispatching
            # during the opening animation can hide a detached focus handle.
            time.sleep(1)
            check(f'g{generation}-overview-open', 0, 0, sessions, overview=True)
            key('super+l')
            check(f'g{generation}-overview-right', 0, 1, sessions, overview=True)
            key('super+h')
            check(f'g{generation}-overview-left', 0, 0, sessions, overview=True)
            key('super+l')
            check(f'g{generation}-overview-right-again', 0, 1, sessions, overview=True)
            key('super+o')
            check(f'g{generation}-overview-close', 0, 1, sessions)
            key('super+h')
            check(f'g{generation}-after-overview-left', 0, 0, sessions)
            if args.compact_tabs:
                geometry = subprocess.check_output(
                    ['xdotool', 'getactivewindow', 'getwindowgeometry', '--shell'],
                    env=env, text=True, timeout=10)
                (root / f'geometry-g{generation}.txt').write_text(geometry)
                subprocess.run(['import', '-window', 'root',
                                str(root / f'compact-tabs-before-g{generation}.png')],
                               env=env, check=True, timeout=15)
                # Click the exposed edge on the appropriate side of the
                # expanded selection. Native hit-testing must not reach a tab
                # hidden underneath another folder.
                positions = [*range(panel_count), *range(panel_count - 2, -1, -1), panel_count - 1, panel_count // 2, 0]
                for position in positions:
                    state = wait_until(lambda: (s := navigation_state(state_path))
                                       and not s['tab_motion'] and not s['camera_motion'] and s,
                                       'settled tab geometry', 10)
                    # Read the actual rendered exposed-edge target rather than
                    # duplicating layout math or assuming a fully expanded tab.
                    targets = dict(state['tab_targets'])
                    assert len(targets) == panel_count
                    x = round(276 + targets[position])
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
            if args.extended_shortcuts:
                from shortcut_behavior_acceptance import verify
                verify(key, check, wait_until, lambda: navigation_state(state_path),
                       root, generation, sessions)
        # No per-key settling delay: navigation must skip the dismissed panel
        # while its close animation is still in flight, in either direction.
        key('super+l')
        check('before-close-left', 0, 1, sessions)
        subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '1',
                        'super+q', 'super+h'], env=env, check=True, timeout=10)
        sessions.pop(1)
        check('after-close-left', 0, 0, sessions)
        key('super+l')
        check('before-close-right', 0, 1, sessions)
        subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '1',
                        'super+q', 'super+Home', 'super+l'], env=env, check=True, timeout=10)
        sessions.pop(1)
        check('after-close-right', 0, 1, sessions)
        subprocess.run(['import', '-window', 'root', str(root / 'navigation.png')], env=env, check=True, timeout=15)
        if args.extended_shortcuts:
            key('super+shift+q')
            app.wait(timeout=10)
            assert app.returncode == 0, app.returncode
            (root / 'quit.json').write_text(json.dumps({'chord': 'super+shift+q', 'exit_code': app.returncode}) + '\n')
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
        # Each reload copy can be hundreds of MB. Keep evidence, not unloaded
        # plugin binaries from this private test process.
        for plugin_dir in (root / 'tmp').glob('jcode-desktop-ui-*'):
            if plugin_dir.is_dir():
                shutil.rmtree(plugin_dir)


if __name__ == '__main__':
    main()
