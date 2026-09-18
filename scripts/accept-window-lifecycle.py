#!/usr/bin/env python3
"""Accept real host window close/Show lifecycle on private Linux Xvfb + Openbox.

Run only after the debug host (and optional matching UI library) has finished
building. This script never builds. Artifacts are retained in OUTPUT, which must
not exist. Linux WM_DELETE_WINDOW exercises shared host paths, but does NOT prove
macOS native red-button event delivery. The screenshot-only lifecycle flag
selects macOS-style Explicit quit policy on Linux, whose production default
otherwise quits when the last window closes.
"""
import argparse
import json
import os
from pathlib import Path
import select
import re
import shutil
import socket
import subprocess
import sys
import time

from screenshot import isolated_env


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=repo / 'target/debug/jcode-desktop')
    parser.add_argument('--hot-reload', action='store_true',
                        help='also send R with CARGO=/usr/bin/true and a matching prebuilt UI')
    parser.add_argument('--plugin', type=Path, default=repo / 'target/debug/libjcode_desktop_ui.so')
    parser.add_argument('--cycles', type=int, default=3)
    parser.add_argument('--timeout', type=float, default=45)
    parser.add_argument('--close-method', choices=('alt-f4', 'wmctrl'), default='alt-f4')
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('this acceptance test requires Linux X11')
    if not 2 <= args.cycles <= 20 or not 0 < args.timeout <= 300:
        parser.error('cycles must be 2..20 and timeout must be positive and <=300')
    for tool in ('Xvfb', 'openbox', 'xdotool', 'xprop', 'import') + (('wmctrl',) if args.close_method == 'wmctrl' else ()):
        if not shutil.which(tool, path='/usr/bin:/bin'):
            parser.error(f'missing prerequisite: {tool}')
    drivers = sorted(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if not drivers:
        parser.error('Mesa lavapipe Vulkan driver is required')
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error('build the debug host first, then rerun with a current --binary')
    if args.hot_reload and not args.plugin.is_file():
        parser.error('build a matching UI library first, then supply --plugin')
    root = args.output.resolve()
    instance = root / 'runtime/jcode-desktop.sock'
    if len(os.fsencode(instance)) >= 104:
        parser.error('output path is too long for a private Unix socket')
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode', 'tmp', 'logs'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.update({
        'TMPDIR': str(root / 'tmp'),
        'JCODE_NO_TELEMETRY': '1',
        'JCODE_DESKTOP_SCREENSHOT_PANELS': '2',
        'JCODE_DESKTOP_SCREENSHOT_MACOS_LIFECYCLE': '1',
        'JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT': 'empty',
        'JCODE_DESKTOP_CONFIG': str(root / 'desktop.toml'),
        'VK_DRIVER_FILES': str(drivers[0]),
    })
    (root / 'desktop.toml').write_text(
        '[appearance]\nlayout_mode = "normal"\ntheme = "warm-neutral"\n'
        '[workspace]\ncoaching_hints = false\n')
    # Explicitly retain Openbox's default Alt+F4 Close binding with private config.
    wm_config = root / 'openbox.xml'
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc">'
                         '<keyboard><keybind key="A-F4"><action name="Close"/></keybind></keyboard>'
                         '<applications><application class="*"><maximized>yes</maximized>'
                         '</application></applications></openbox_config>')
    binary = root / 'jcode-desktop'
    shutil.copy2(args.binary.resolve(), binary)
    command = [str(binary), '--no-hot-reload']
    if args.hot_reload:
        plugin = root / 'prebuilt-ui.so'
        shutil.copy2(args.plugin.resolve(), plugin)
        env['CARGO'] = '/usr/bin/true'
        command = [str(binary), '--hot-reload', str(plugin)]
    processes, logs, evidence = [], [], []
    state_path = root / 'state'
    host_log = root / 'logs/jcode-desktop/jcode-desktop.log'
    recovery = host_log.parent / 'crash-recovery.json'
    report = {'status': 'running', 'scope': 'Linux shared real host close/reopen paths only',
              'macos_native_event_proof': False, 'hot_reload': args.hot_reload,
              'quit_policy': 'screenshot-only emulated macOS Explicit policy on Linux',
              'close_method': args.close_method, 'checkpoints': evidence}

    def launch(name, argv, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(argv, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process

    def run(*argv, check=True):
        return subprocess.run(argv, env=env, cwd=root, capture_output=True,
                              text=True, check=check, timeout=10)

    def wait(label, predicate, timeout=None):
        deadline = time.monotonic() + (args.timeout if timeout is None else timeout)
        while time.monotonic() < deadline:
            if any(p.poll() is not None for p in processes):
                raise RuntimeError(label + ': private child exited: ' + repr(
                    [(p.args[0], p.poll()) for p in processes if p.poll() is not None]))
            result = predicate()
            if result:
                return result
            time.sleep(.1)
        raise AssertionError('timed out: ' + label)

    def navigation():
        try:
            return json.loads(next(line.split('=', 1)[1] for line in
                                   state_path.read_text().splitlines() if line.startswith('navigation=')))
        except (OSError, StopIteration, ValueError):
            return None

    def panels():
        current = navigation()
        return [p for row in current['rows'] for p in row['panels']] if current else []

    def identities():
        return [p['session'] for p in panels()]

    def windows():
        # EWMH client list, not title matching: excludes WM frames and helper windows.
        clients = re.findall(r'0x[0-9a-fA-F]+', run('xprop', '-root', '_NET_CLIENT_LIST').stdout)
        return [wid for wid in clients if
                run('xprop', '-id', wid, '_NET_WM_PID', check=False).stdout.strip().endswith('= ' + str(app.pid))]

    def exists(window):
        # A merely unmapped/hidden client still has a valid XID and fails this test.
        return run('xdotool', 'getwindowgeometry', window, check=False).returncode == 0

    def send(byte):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(3)
            client.connect(str(instance))
            client.sendall(byte)
            reply = b''
            while len(reply) < 3:
                chunk = client.recv(3 - len(reply))
                if not chunk:
                    break
                reply += chunk
            if reply != b'ok\n':
                raise AssertionError(f'instance command {byte!r}: {reply!r}')

    def draft():
        try:
            checkpoint = json.loads(recovery.read_text())
            return next(slot['panel']['draft']['content'] for slot in checkpoint['snapshot']['slots']
                        if slot['panel']['session_id'] == draft_session)
        except (OSError, ValueError, KeyError, StopIteration):
            return None

    def capture(label):
        run('import', '-window', 'root', str(root / (label + '.png')))
        if state_path.exists():
            shutil.copy2(state_path, root / (label + '.state'))
        if recovery.exists():
            shutil.copy2(recovery, root / (label + '.recovery.json'))
        evidence.append({'checkpoint': label, 'windows': windows(), 'navigation': navigation(),
                         'process_alive': app.poll() is None})

    def generations():
        return host_log.read_text().count('activated UI generation') if host_log.exists() else 0

    def check_restored(label):
        nonlocal text
        wait(label + ' fresh state and one window', lambda: len(windows()) == 1 and identities() == original)
        if draft_accessible:
            # Appending proves the restored live composer retained the draft,
            # rather than trusting an unchanged recovery file from before close.
            run('xdotool', 'windowactivate', '--sync', windows()[0])
            text += 'x'
            run('xdotool', 'key', '--clearmodifiers', 'End')
            run('xdotool', 'type', '--clearmodifiers', 'x')
            wait(label + ' live draft retained', lambda: draft() == text)
        capture(label)

    def cycle(label):
        old = wait('one window before close', lambda: windows() if len(windows()) == 1 else None)[0]
        if args.close_method == 'wmctrl':
            run('wmctrl', '-ic', old)
        else:
            run('xdotool', 'windowactivate', '--sync', old)
            run('xdotool', 'key', '--clearmodifiers', 'alt+F4')
        wait(label + ' actual X11 window destroyed', lambda: not windows() and not exists(old))
        # Require a stable no-window interval while the same host stays alive.
        for _ in range(5):
            time.sleep(.1)
            if app.poll() is not None or windows() or exists(old):
                raise AssertionError('close did not leave a live windowless host')
        capture(label + '-closed')
        state_path.unlink(missing_ok=True)
        send(b'S')
        check_restored(label + '-reopened')
        for _ in range(5):
            send(b'S')
        for _ in range(10):
            time.sleep(.1)
            if app.poll() is not None or len(windows()) != 1:
                raise AssertionError('repeated Show duplicated a window or stopped the host')
        wait(label + ' repeated Show retains panels', lambda: identities() == original)
        capture(label + '-repeated-show')

    read_fd, write_fd = os.pipe()
    try:
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1440x1000x24',
                        '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        if not select.select([read_fd], [], [], 15)[0]:
            raise RuntimeError('private Xvfb did not start')
        display = os.read(read_fd, 64).decode().strip()
        if not display.isdigit():
            raise RuntimeError('Xvfb returned an invalid display')
        env['DISPLAY'] = ':' + display
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm_config)])
        wait('private WM', lambda: bool(re.search(r'0x[1-9a-fA-F][0-9a-fA-F]*',
             run('xprop', '-root', '_NET_SUPPORTING_WM_CHECK').stdout)))
        app = launch('app', command)
        wait('two offline panels', lambda: len(panels()) == 2 and len(windows()) == 1 and instance.exists())
        if args.hot_reload:
            wait('startup prebuilt UI activation', lambda: generations() >= 1)
        original = identities()
        if len(set(original)) != 2:
            raise AssertionError('fixture session IDs must be distinct')
        draft_session = next(p['session'] for p in panels() if p['focused'])
        text = 'window lifecycle retained draft'
        run('xdotool', 'windowactivate', '--sync', windows()[0])
        run('xdotool', 'type', '--clearmodifiers', text)
        # Require current-host recovery snapshots to expose the edited live draft.
        wait('composer rendered', lambda: navigation() is not None)
        deadline = time.monotonic() + 5
        while draft() is None and time.monotonic() < deadline:
            time.sleep(.1)
        draft_accessible = draft() is not None
        if not draft_accessible:
            raise AssertionError('required recovery draft inaccessible: ' + str(recovery))
        report['draft_assertion'] = 'required live append after every restore'
        wait('initial typed draft', lambda: draft() == text)
        capture('initial')
        for index in range(args.cycles):
            cycle(f'cycle-{index + 1}')
        if args.hot_reload:
            before = generations()
            state_path.unlink(missing_ok=True)
            send(b'R')
            wait('R activates another prebuilt UI generation', lambda: generations() > before)
            check_restored('after-reload')
            cycle('post-reload')
        report['status'] = 'passed'
        print(f'PASS: Linux host window lifecycle. Artifacts: {root}')
        print('Not proof of macOS native red-close event delivery.')
    except BaseException as error:
        report['status'] = 'failed'
        report['error'] = str(error)
        report['children_at_failure'] = [{'command': p.args, 'returncode': p.poll()} for p in processes]
        raise
    finally:
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
