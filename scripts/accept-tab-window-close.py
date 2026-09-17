#!/usr/bin/env python3
"""Click the real tab-bar close control on private Linux Xvfb + Openbox.

Build the host first. No live desktop, daemon, credentials, or app settings are
used. Artifacts are retained in OUTPUT, which must not already exist. This checks
normal Linux last-window quit. Host unit tests cover retained-draft restoration
and refusing a close when the workspace snapshot fails.
"""
import argparse
import json
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import time

from screenshot import isolated_env


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=repo / 'target/debug/jcode-desktop')
    args = parser.parse_args()
    for tool in ('Xvfb', 'openbox', 'xdotool', 'xprop', 'import'):
        if not shutil.which(tool, path='/usr/bin:/bin'):
            parser.error(f'missing prerequisite: {tool}')
    drivers = sorted(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if not drivers or not args.binary.is_file():
        parser.error('requires Mesa lavapipe and a freshly built host')
    root = args.output.resolve()
    if len(os.fsencode(root / 'runtime/jcode-desktop.sock')) >= 104:
        parser.error('output path is too long for a private Unix socket')
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode', 'tmp', 'logs'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.update({
        'TMPDIR': str(root / 'tmp'),
        'JCODE_NO_TELEMETRY': '1',
        'JCODE_DESKTOP_SCREENSHOT_PANELS': '2',
        'JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT': 'empty',
        'JCODE_DESKTOP_CONFIG': str(root / 'desktop.toml'),
        'VK_DRIVER_FILES': str(drivers[0]),
    })
    (root / 'desktop.toml').write_text(
        '[appearance]\nlayout_mode = "normal"\ntheme = "warm-neutral"\n'
        '[workspace]\ncoaching_hints = false\n')
    wm_config = root / 'openbox.xml'
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc">'
                         '<applications><application class="*"><maximized>yes</maximized>'
                         '</application></applications></openbox_config>')
    binary = root / 'jcode-desktop'
    shutil.copy2(args.binary.resolve(), binary)
    processes, logs = [], []
    state_path = root / 'state'
    report = {'status': 'running', 'scope': 'native Linux tab-button close'}

    def launch(name, argv, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(argv, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process

    def run(*argv, check=True):
        return subprocess.run(argv, env=env, cwd=root, capture_output=True,
                              text=True, check=check, timeout=10)

    def wait(label, predicate):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            if any(p.poll() is not None for p in processes):
                raise RuntimeError(label + ': private child exited unexpectedly')
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

    def windows():
        clients = re.findall(r'0x[0-9a-fA-F]+', run('xprop', '-root', '_NET_CLIENT_LIST').stdout)
        return [wid for wid in clients if
                run('xprop', '-id', wid, '_NET_WM_PID', check=False).stdout.strip().endswith('= ' + str(app.pid))]

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
        app = launch('app', [str(binary), '--no-hot-reload'])
        wait('rendered window', lambda: len(windows()) == 1 and navigation() is not None)
        before = navigation()
        panels = [p for row in before['rows'] for p in row['panels']]
        assert len(panels) == 2, 'fixture must contain two live sessions'
        old = windows()[0]
        run('xdotool', 'windowactivate', '--sync', old)
        run('import', '-window', 'root', str(root / 'before.png'))
        # Normal layout, minimap off: a 32px control flush right at y=10.
        # Coordinates are relative to the native client, not the WM frame.
        width = before['viewport'][0]
        run('xdotool', 'mousemove', '--window', old, str(round(width - 16)), '26')
        run('xdotool', 'click', '1')
        app.wait(timeout=15)
        assert app.returncode == 0, 'close must quit cleanly'
        assert not windows(), 'close must destroy the window, not just a session'
        assert run('xdotool', 'getwindowgeometry', old, check=False).returncode != 0
        run('import', '-window', 'root', str(root / 'after.png'))
        report.update(status='passed', exit_code=app.returncode,
                      original_panels=len(panels), window_destroyed=True)
        print(f'PASS: native tab-bar window close. Artifacts: {root}')
    except BaseException as error:
        report.update(status='failed', error=str(error))
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
