#!/usr/bin/env python3
"""Accept real global shortcuts and wtype forwarding on private headless Sway.

Never starts or queries the user's compositor. Only the helper's focused-window
query is translated from its existing CLI shape into real private Sway IPC.
The original Super chords, global grabs, helper, wtype, Wayland keyboard events,
Desktop, and SDK session creation all execute, including a real UI hot reload.
With --held-keys, use true key-down/up instead of taps to verify repeat and
release on private Sway (300ms delay, 4Hz), without requesting a UI reload.
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
    parser.add_argument('--sway-prefix', type=Path,
                        help='private extracted Sway prefix containing usr/bin and usr/lib')
    parser.add_argument('--linked-ui', action='store_true',
                        help='test the built linked UI without hot reload (requires --held-keys)')
    parser.add_argument('--held-keys', action='store_true',
                        help='test actual key repeats, release, and empty reopen without UI reload')
    parser.add_argument('--build-timeout', type=int, default=180,
                        help='seconds allowed for initial/reload builds, including Cargo lock waits')
    args = parser.parse_args()
    if args.linked_ui and not args.held_keys:
        parser.error('--linked-ui requires --held-keys')
    sway_prefix = args.sway_prefix.resolve(strict=True) if args.sway_prefix else None
    tool_path = (str(sway_prefix / 'usr/bin') + ':' if sway_prefix else '') + os.environ.get('PATH', '')
    if args.build_timeout <= 0:
        parser.error('--build-timeout must be positive')
    if args.held_keys and (args.baseline_helper or args.pinned_launch_command or args.home_launch_command):
        parser.error('--held-keys cannot be combined with baseline or launcher tests')
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
        assert shutil.which(tool, path=tool_path), tool
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ['home', 'runtime', 'config', 'cache', 'data', 'logs', 'jcode', 'shims', 'tmp']:
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    jcode = Path(shutil.which('jcode')).resolve()
    env.update({
        'PATH': str(root / 'shims') + ':' + (str(sway_prefix / 'usr/bin') + ':' if sway_prefix else '') + str(cargo_home / 'bin') + ':/usr/bin:/bin',
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
    if sway_prefix:
        env['LD_LIBRARY_PATH'] = str(sway_prefix / 'usr/lib')
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

    def key(keysym, *mods, modifier_hold_ms=0):
        # Legacy tap tests deliberately keep only modifiers down afterward.
        # Actual press-and-hold coverage uses held_key below, never -k.
        # Let wl_keyboard.enter from this newly attached virtual keyboard
        # reach the native window before sending its first shortcut.
        command = ['wtype', '-s', '100']
        for mod in mods:
            command += ['-M', mod]
        command += ['-s', '50', '-k', keysym]
        if modifier_hold_ms:
            command += ['-s', str(modifier_hold_ms)]
        command += ['-s', '100']
        for mod in reversed(mods):
            command += ['-m', mod]
        subprocess.run(command, env=env, check=True, timeout=10)

    def focus_query_count():
        try:
            return len((root / 'focus-queries.jsonl').read_text().splitlines())
        except FileNotFoundError:
            return 0

    def held_key(keysym, *mods, duration_ms):
        queries_before = focus_query_count()
        command = ['wtype', '-s', '100']
        for mod in mods:
            command += ['-M', mod]
        command += ['-s', '50', '-P', keysym, '-s', str(duration_ms), '-p', keysym]
        # Keep the modifier held after key-up to specifically check key release.
        command += ['-s', '500']
        for mod in reversed(mods):
            command += ['-m', mod]
        process = launch('held-' + keysym, command)
        samples = []
        started = time.monotonic()
        while process.poll() is None:
            current = state()
            # The diagnostic file is rewritten, so a partial read is not an
            # empty workspace observation. Keep only complete snapshots.
            if current:
                samples.append({'elapsed_ms': round((time.monotonic() - started) * 1000),
                                'state': current})
            if time.monotonic() - started > duration_ms / 1000 + 10:
                raise AssertionError('held key injection timed out')
            time.sleep(.025)
        assert process.returncode == 0, command
        evidence.append({'checkpoint': 'held-' + keysym, 'command': command,
                         'duration_ms': duration_ms, 'samples': samples,
                         'helper_invocations': focus_query_count() - queries_before})
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        return focus_query_count() - queries_before

    def stable_after_release(label):
        def signature(current):
            # Tab target geometry and session metadata can settle after a new
            # panel opens. Release must stop navigation/closing, not layout.
            return (current['active_row'], current['focused_slot'], current['keyboard_panel'],
                    [[(p['slot'], p['id'], p['session'], p['closing']) for p in row['panels']]
                     for row in current['rows']])

        before = wait(state, label + ' initial snapshot')
        expected = signature(before)
        queries_before = focus_query_count()
        observations = 0
        deadline = time.monotonic() + 1.2
        while time.monotonic() < deadline:
            current = state()
            if current:
                assert signature(current) == expected, (label, before, current)
                observations += 1
            assert focus_query_count() == queries_before, (label, 'helper invoked after key release')
            time.sleep(.05)
        assert observations >= 10, (label, 'insufficient complete state snapshots', observations)
        evidence.append({'checkpoint': label, 'stable_ms': 1200,
                         'observations': observations, 'helper_invocations': 0, 'state': before})
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')

    def check(label, slot):
        current = wait(lambda: (s := state()) and s.get('focused_slot') == slot
                       and s.get('keyboard_panel') == slot and s, label)
        evidence.append({'checkpoint': label, 'state': current})
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print(label, 'focused_slot=', slot, flush=True)
        return current

    def configure(selected_helper, *, enter_repeat=True):
        config = root / 'sway.config'
        config.write_text('xwayland disable\noutput HEADLESS-1 mode 1600x1000\n'
                          'seat seat0 fallback true\ndefault_border none\n'
                          'focus_follows_mouse no\n'
                          + ('input * {\n repeat_delay 300\n repeat_rate 4\n}\n' if args.held_keys else '')
                          + ''.join(f'bindsym {"--no-repeat " if chord == "Return" and not enter_repeat else ""}Mod4+{chord} exec bash {shlex.quote(str(selected_helper))} {action}\n'
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
        bridge = repo / 'target/debug/jcode-harness-api-bridge'
        launch('bridge', [str(bridge)] if bridge.exists() else [str(jcode), '--no-update', '--no-selfdev', 'api-bridge'])
        wait(lambda: Path(env['JCODE_API_SOCKET']).exists(), 'private bridge')
        launch('app', [str(repo / 'target/debug/jcode-desktop'), '--no-hot-reload' if args.linked_ui else '--hot-reload'])
        diagnostics = root / 'logs/jcode-desktop/jcode-desktop.log'
        if not args.linked_ui:
            wait(lambda: diagnostics.exists() and 'activated UI generation' in diagnostics.read_text(),
                 'initial UI activation', args.build_timeout)
        wait(lambda: len(panels()) == 1 and panels()[0]['session'].startswith('session_'), 'first session')
        # Plugin activation logs before the new root's first frame is mounted.
        # Do not send setup input into that deliberately suspended interval.
        time.sleep(.5)
        check('initial-composer-ready', 0)
        if args.held_keys:
            for count in range(2, 9):
                key('t', 'ctrl')
                wait(lambda: len(panels()) == count and all(p['session'].startswith('session_') for p in panels()),
                     'held-key setup panel ' + str(count))
            check('held-key-setup-eight-panels', 7)
            identities = [p['session'] for p in panels()]
            held_key('Page_Up', 'ctrl', duration_ms=850)
            slot = state()['focused_slot']
            assert 0 < slot < 6, ('direct held navigation must repeat without hitting boundary', slot)
            check('direct-held-previous-repeated', slot)
            assert [p['session'] for p in panels()] == identities
            stable_after_release('direct-navigation-key-release-stops')
            held_key('Page_Down', 'ctrl', duration_ms=1800)
            check('direct-held-next-clamps-last', 7)
            assert [p['session'] for p in panels()] == identities
            stable_after_release('direct-next-key-release-stops')
            # Exercise the global grab + helper path too. Direct aliases alone
            # cannot detect a compositor swallowing repeats of Super chords.
            invocations = held_key('h', 'logo', duration_ms=850)
            slot = check('global-held-previous-repeated', 7 - invocations)['focused_slot']
            assert 0 < slot < 6, ('global held previous must repeat', slot)
            assert slot == 7 - invocations, ('one navigation per helper invocation', slot, invocations)
            assert [p['session'] for p in panels()] == identities
            stable_after_release('global-previous-key-release-stops')
            held_key('l', 'logo', duration_ms=1800)
            check('global-held-next-clamps-last', 7)
            assert [p['session'] for p in panels()] == identities
            stable_after_release('global-next-key-release-stops')
            invocations = held_key('q', 'logo', duration_ms=850)
            wait(lambda: len(panels()) == 8 - invocations, 'one close per helper invocation')
            remaining = len(panels())
            assert 0 < remaining < 7, ('held Super+Q must close multiple panels, leaving some for release check', remaining)
            check('held-super-q-closes-multiple', state()['focused_slot'])
            assert [p['session'] for p in panels()] == identities[:remaining]
            stable_after_release('super-q-key-release-stops-with-panels-left')
            held_key('q', 'logo', duration_ms=3000)
            wait(lambda: not panels(), 'held Super+Q empties workspace')
            check('held-super-q-empty-workspace-alive', None)
            stable_after_release('empty-workspace-release-stable')
            key('Return', 'logo')
            wait(lambda: len(panels()) == 1 and panels()[0]['session'].startswith('session_'),
                 'global Enter reopens after held close')
            check('reopen-after-held-super-q', 0)
            record = wait(lambda: creation(panels()[0]['session']), 'reopened session daemon cwd')
            assert record['working_dir'] == str(directory), record
            (root / 'empty-reopen-creation.json').write_text(json.dumps(record, indent=2) + '\n')
            stable_after_release('reopened-panel-not-closed-by-stale-repeat')
            # Negative control: reproduce the host's former repeat=false
            # policy on our private compositor before enabling repeats.
            configure(helper, enter_repeat=False)
            subprocess.run(['swaymsg', 'reload'], env=env, check=True, timeout=10)
            invocations = held_key('Return', 'logo', duration_ms=850)
            assert invocations == 1, ('single-shot Enter must invoke helper once', invocations)
            wait(lambda: len(panels()) == 2 and all(p['session'].startswith('session_') for p in panels()),
                 'single-shot Enter creates exactly one panel')
            check('held-super-enter-single-shot-baseline', 1)
            stable_after_release('single-shot-enter-release-stable')
            configure(helper)
            subprocess.run(['swaymsg', 'reload'], env=env, check=True, timeout=10)
            invocations = held_key('Return', 'logo', duration_ms=850)
            assert invocations > 1, ('held Enter must invoke helper multiple times', invocations)
            count = 2 + invocations
            wait(lambda: len(panels()) == count and all(p['session'].startswith('session_') for p in panels()),
                 'held Enter sessions attach')
            check('held-super-enter-creates-multiple', count - 1)
            assert len({p['session'] for p in panels()}) == count
            for panel in panels():
                record = wait(lambda: creation(panel['session']), 'held Enter session daemon cwd')
                assert record['working_dir'] == str(directory), record
            stable_after_release('super-enter-key-release-stops')
            queries = [json.loads(line) for line in (root / 'focus-queries.jsonl').read_text().splitlines()]
            assert len(queries) >= 9, queries
            assert all(q and q['app_id'] == 'jcode-desktop' for q in queries), queries
            print('PASS: held direct/global navigation, Super+Q, and Super+Enter repeat and stop on release; empty workspace reopens', flush=True)
            return
        for count in [2, 3]:
            key('t', 'ctrl')
            wait(lambda: len(panels()) == count and all(p['session'].startswith('session_') for p in panels()), 'setup panel')
        key('Page_Up', 'ctrl')
        check('setup-middle', 1)
        if baseline:
            identities = [p['session'] for p in panels()]
            for chord in ['h', 'l', 'Return', 'q']:
                key(chord, 'logo', modifier_hold_ms=300)
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
                     'Ctrl+R UI hot reload', args.build_timeout)
                time.sleep(.5)
            for chord, expected in [('h', 0), ('h', 0), ('l', 1), ('l', 2), ('l', 2), ('h', 1)]:
                identities = [p['session'] for p in panels()]
                key(chord, 'logo', modifier_hold_ms=400)
                check(f'g{generation}-global-{chord}-{expected}', expected)
                assert [p['session'] for p in panels()] == identities
        before = {p['session'] for p in panels()}
        key('Return', 'logo', modifier_hold_ms=400)
        wait(lambda: len(panels()) == len(before) + 1 and all(p['session'].startswith('session_') for p in panels()), 'global Enter panel')
        added = [p for p in panels() if p['session'] not in before]
        assert len(added) == 1
        check('global-enter-focused', added[0]['slot'])
        record = wait(lambda: creation(added[0]['session']), 'global Enter daemon cwd')
        assert record['working_dir'] == str(directory), record
        (root / 'creation.json').write_text(json.dumps(record, indent=2) + '\n')
        before_close = [p['session'] for p in panels()]
        dismissed = added[0]['session']
        key('q', 'logo', modifier_hold_ms=400)
        wait(lambda: len(panels()) == len(before_close) - 1, 'global Q dismisses one panel')
        assert [p['session'] for p in panels()] == [s for s in before_close if s != dismissed]
        check('global-q-restores-focus', state()['focused_slot'])
        # Immediately navigating after closing must still reach a live input.
        key('h', 'logo', modifier_hold_ms=400)
        check('global-h-after-close', 1)
        for chord, _ in launch_commands:
            previous = {p['session'] for p in panels()}
            key(chord, 'logo', modifier_hold_ms=400)
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
            key('q', 'logo', modifier_hold_ms=400)
            wait(lambda: len(panels()) == remaining, 'close down to ' + str(remaining))
        check('last-q-keeps-empty-workspace-alive', None)
        key('Return', 'logo', modifier_hold_ms=400)
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
