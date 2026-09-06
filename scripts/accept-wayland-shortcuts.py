#!/usr/bin/env python3
"""Accept real global shortcuts and wtype forwarding on private headless Sway.

Never starts or queries the user's compositor. Only the helper's focused-window
query is translated from its existing CLI shape into real private Sway IPC.
The original Super chords, global grabs, helper, wtype, Wayland keyboard events,
Desktop, and SDK session creation all execute, including a real UI hot reload.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import time

from screenshot import isolated_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--helper', type=Path, required=True)
    parser.add_argument('--baseline-helper', type=Path)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--pinned-launch-command', type=Path)
    parser.add_argument('--home-launch-command', type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    root = args.output.resolve()
    helper = args.helper.resolve(strict=True)
    baseline = args.baseline_helper.resolve(strict=True) if args.baseline_helper else None
    directory = args.directory.resolve(strict=True)
    if bool(args.pinned_launch_command) != bool(args.home_launch_command):
        parser.error('both launcher command paths are required together')
    launch_commands = [(name, path.resolve(strict=True)) for name, path in [
        ('semicolon', args.pinned_launch_command), ('apostrophe', args.home_launch_command),
    ] if path]
    assert directory.is_dir()
    assert len(str(root / 'runtime/daemon.sock').encode()) < 104
    for tool in ['sway', 'swaymsg', 'wtype', 'jcode', 'cargo', 'jq']:
        assert shutil.which(tool), tool
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ['home', 'runtime', 'config', 'cache', 'data', 'logs', 'jcode', 'shims', 'tmp']:
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    jcode = Path(shutil.which('jcode')).resolve()
    env.update({
        'PATH': str(root / 'shims') + ':' + str(cargo_home / 'bin') + ':/usr/bin:/bin',
        'WLR_BACKENDS': 'headless', 'WLR_RENDERER': 'pixman',
        'WLR_LIBINPUT_NO_DEVICES': '1', 'XDG_SESSION_TYPE': 'wayland',
        'JCODE_NO_TELEMETRY': '1', 'JCODE_RUNTIME_DIR': str(root / 'runtime'),
        'JCODE_API_SOCKET': str(root / 'runtime/api.sock'),
        'JCODE_SOCKET': str(root / 'runtime/daemon.sock'),
        'JCODE_TEMP_SERVER': '1', 'JCODE_SERVER_OWNER_PID': str(os.getpid()),
        'JCODE_TEMP_SERVER_IDLE_SECS': '600',
        'JCODE_DESKTOP_CONFIG': str(root / 'desktop.toml'),
        'CARGO': str(cargo_home / 'bin/cargo'), 'CARGO_HOME': str(cargo_home),
        'RUSTUP_HOME': os.environ.get('RUSTUP_HOME', str(Path.home() / '.rustup')),
        'CARGO_NET_OFFLINE': 'true', 'TMPDIR': str(root / 'tmp'),
        'VK_DRIVER_FILES': str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))),
        'FOCUS_QUERY_LOG': str(root / 'focus-queries.jsonl'),
    })
    (root / 'desktop.toml').write_text('[workspace]\ncoaching_hints = false\npinned_working_dir = '
                                      + json.dumps(str(directory)) + '\n')
    # No real `niri` executable is ever invoked. Resolve focus from actual
    # native Wayland app identity on our private compositor instead.
    adapter = root / 'shims/niri'
    adapter.write_text('''#!/usr/bin/python3
import json, os, subprocess, sys
assert sys.argv[1:] == ['msg', '-j', 'focused-window']
tree = json.loads(subprocess.check_output(['swaymsg', '-t', 'get_tree', '-r'], text=True))
def focused(node):
    if node.get('focused') and node.get('type') == 'con':
        return node
    for child in node.get('nodes', []) + node.get('floating_nodes', []):
        found = focused(child)
        if found:
            return found
node = focused(tree)
result = {'app_id': node.get('app_id'), 'title': node.get('name')} if node else None
with open(os.environ['FOCUS_QUERY_LOG'], 'a') as log:
    log.write(json.dumps(result) + '\\n')
print(json.dumps(result))
''')
    adapter.chmod(0o755)
    processes, logs, evidence = [], [], []

    def launch(name, command):
        log = (root / f'{name}.log').open('w')
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log,
                                   stderr=log, start_new_session=True)
        processes.append(process)
        return process

    def wait(predicate, label, timeout=60):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
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
        return [p for row in state().get('rows', []) for p in row['panels']]

    def key(keysym, *mods, hold_ms=0):
        # Let wl_keyboard.enter from this newly attached virtual keyboard
        # reach the native window before sending its first shortcut.
        command = ['wtype', '-s', '100']
        for mod in mods:
            command += ['-M', mod]
        command += ['-s', '50', '-k', keysym]
        if hold_ms:
            command += ['-s', str(hold_ms)]
        command += ['-s', '100']
        for mod in reversed(mods):
            command += ['-m', mod]
        subprocess.run(command, env=env, check=True, timeout=10)

    def check(label, slot):
        current = wait(lambda: (s := state()) and s.get('focused_slot') == slot
                       and s.get('keyboard_panel') == slot and s, label)
        evidence.append({'checkpoint': label, 'state': current})
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print(label, 'focused_slot=', slot, flush=True)
        return current

    def configure(selected_helper):
        config = root / 'sway.config'
        config.write_text('xwayland disable\noutput HEADLESS-1 mode 1600x1000\n'
                          'seat seat0 fallback true\ndefault_border none\n'
                          'focus_follows_mouse no\n'
                          + ''.join(f'bindsym Mod4+{chord} exec bash {shlex.quote(str(selected_helper))} {action}\n'
                                    for chord, action in [('h', 'previous'), ('l', 'next'), ('Return', 'new'), ('q', 'close')])
                          + ''.join(f'bindsym Mod4+{chord} exec sh {shlex.quote(str(command))}\n'
                                    for chord, command in launch_commands))
        return config

    def creation(session_id):
        for log in (root / 'jcode/logs').glob('jcode-*.log'):
            for line in log.read_text().splitlines():
                if 'ENV_SNAPSHOT ' in line:
                    record = json.loads(line.split('ENV_SNAPSHOT ', 1)[1])
                    if record.get('session_id') == session_id and record.get('reason') == 'create':
                        return record
        return None

    try:
        launch('sway', ['sway', '--config', str(configure(baseline or helper))])
        env['SWAYSOCK'] = str(wait(lambda: next((root / 'runtime').glob('sway-ipc.*.sock'), None), 'private Sway IPC'))
        env['WAYLAND_DISPLAY'] = wait(lambda: next((p.name for p in (root / 'runtime').glob('wayland-*')
                                                  if p.is_socket()), None), 'private Wayland socket')
        launch('daemon', [str(jcode), '--no-update', '--no-selfdev', '--provider', 'jcode', 'serve'])
        wait(lambda: Path(env['JCODE_SOCKET']).exists(), 'private daemon')
        launch('bridge', [str(repo / 'target/debug/jcode-harness-api-bridge')])
        wait(lambda: Path(env['JCODE_API_SOCKET']).exists(), 'private bridge')
        launch('app', [str(repo / 'target/debug/jcode-desktop'), '--hot-reload'])
        diagnostics = root / 'logs/jcode-desktop/jcode-desktop.log'
        wait(lambda: diagnostics.exists() and 'activated UI generation' in diagnostics.read_text(),
             'initial UI activation', 180)
        wait(lambda: len(panels()) == 1 and panels()[0]['session'].startswith('session_'), 'first session')
        # Plugin activation logs before the new root's first frame is mounted.
        # Do not send setup input into that deliberately suspended interval.
        time.sleep(.5)
        check('initial-composer-ready', 0)
        for count in [2, 3]:
            key('t', 'ctrl')
            wait(lambda: len(panels()) == count and all(p['session'].startswith('session_') for p in panels()), 'setup panel')
        key('Page_Up', 'ctrl')
        check('setup-middle', 1)
        if baseline:
            identities = [p['session'] for p in panels()]
            for chord in ['h', 'l', 'Return', 'q']:
                key(chord, 'logo', hold_ms=300)
                time.sleep(.3)
                check('baseline-dropped-' + chord, 1)
                assert [p['session'] for p in panels()] == identities
            configure(helper)
            subprocess.run(['swaymsg', 'reload'], env=env, check=True, timeout=10)
        for generation in range(2):
            if generation:
                before = diagnostics.read_text().count('activated UI generation')
                key('r', 'ctrl')
                wait(lambda: diagnostics.read_text().count('activated UI generation') > before,
                     'Ctrl+R UI hot reload', 180)
                time.sleep(.5)
            for chord, expected in [('h', 0), ('h', 0), ('l', 1), ('l', 2), ('l', 2), ('h', 1)]:
                identities = [p['session'] for p in panels()]
                key(chord, 'logo', hold_ms=400)
                check(f'g{generation}-global-{chord}-{expected}', expected)
                assert [p['session'] for p in panels()] == identities
        before = {p['session'] for p in panels()}
        key('Return', 'logo', hold_ms=400)
        wait(lambda: len(panels()) == len(before) + 1 and all(p['session'].startswith('session_') for p in panels()), 'global Enter panel')
        added = [p for p in panels() if p['session'] not in before]
        assert len(added) == 1
        check('global-enter-focused', added[0]['slot'])
        record = wait(lambda: creation(added[0]['session']), 'global Enter daemon cwd')
        assert record['working_dir'] == str(directory), record
        (root / 'creation.json').write_text(json.dumps(record, indent=2) + '\n')
        before_close = [p['session'] for p in panels()]
        dismissed = added[0]['session']
        key('q', 'logo', hold_ms=400)
        wait(lambda: len(panels()) == len(before_close) - 1, 'global Q dismisses one panel')
        assert [p['session'] for p in panels()] == [s for s in before_close if s != dismissed]
        check('global-q-restores-focus', state()['focused_slot'])
        # Immediately navigating after closing must still reach a live input.
        key('h', 'logo', hold_ms=400)
        check('global-h-after-close', 1)
        for chord, _ in launch_commands:
            previous = {p['session'] for p in panels()}
            key(chord, 'logo', hold_ms=400)
            wait(lambda: len(panels()) == len(previous) + 1
                 and all(p['session'].startswith('session_') for p in panels()), 'managed ' + chord)
            created = [p for p in panels() if p['session'] not in previous]
            assert len(created) == 1
            check('managed-' + chord, created[0]['slot'])
            record = wait(lambda: creation(created[0]['session']), 'managed launcher cwd')
            expected = directory if chord == 'semicolon' else root / 'home'
            assert record['working_dir'] == str(expected), record
            (root / (chord + '-creation.json')).write_text(json.dumps(record, indent=2) + '\n')
        close_count = len(panels())
        for remaining in range(close_count - 1, -1, -1):
            key('q', 'logo', hold_ms=400)
            wait(lambda: len(panels()) == remaining, 'close down to ' + str(remaining))
        check('last-q-keeps-empty-workspace-alive', None)
        key('Return', 'logo', hold_ms=400)
        wait(lambda: len(panels()) == 1 and panels()[0]['session'].startswith('session_'), 'Enter from empty workspace')
        check('enter-after-last-q', 0)
        record = wait(lambda: creation(panels()[0]['session']), 'empty workspace Enter cwd')
        assert record['working_dir'] == str(directory), record
        (root / 'empty-reopen-creation.json').write_text(json.dumps(record, indent=2) + '\n')
        queries = [json.loads(line) for line in (root / 'focus-queries.jsonl').read_text().splitlines()]
        assert len(queries) == (19 if baseline else 15) + len(launch_commands) + close_count + 1, queries
        assert all(q['app_id'] == 'jcode-desktop' for q in queries), queries
        assert not any(message in diagnostics.read_text() for message in ['UI rebuild failed', 'UI reload failed'])
        print('PASS: real global Super keys, real wtype forwarding, native Wayland focus, reload and configured creation', flush=True)
    finally:
        for process in reversed(processes):
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
