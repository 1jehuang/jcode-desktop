#!/usr/bin/env python3
"""Exercise folder focus through native input and real isolated SDK sessions.

No screenshot fixtures, inherited credentials, model requests, or user desktop
sockets are used. Requires current desktop and jcode binaries plus Xvfb tools.
"""
import argparse
from itertools import groupby
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import time

from PIL import Image
from screenshot import isolated_env


def check_strip(image, focused):
    """Assert visible public output, not a copied layout implementation."""
    canvas, active, idle = (28, 26, 24), (37, 34, 31), (48, 43, 39)
    tops = []
    for index in range(3):
        x = 284 + 384 * index
        color = active if index == focused else idle
        assert image.getpixel((x, 300)) == color, 'Focus must change surface color'
        top = 300
        while top > 0 and image.getpixel((x, top - 1)) == color:
            top -= 1
        tops.append(top)
        assert image.getpixel((x, 983)) == color, 'Folders must share their bottom edge'
        assert all(image.getpixel((x, y)) == canvas for y in range(984, 1000))
    active_top = tops[focused]
    assert active_top >= 16, 'Canvas must remain above folders'
    for index, top in enumerate(tops):
        assert top - active_top == (0 if index == focused else 8), 'Active folder must rise 8px'
        x = 284 + 384 * index
        assert all(image.getpixel((x, y)) == canvas for y in range(active_top - 16, active_top))
    # A scan through empty real sessions must contain only the adjoining
    # surface colors. Any gap or focus-outline pixel fails this check.
    assert all(image.getpixel((x, 300)) in (active, idle) for x in range(264, 1414))
    print(f'Pixel acceptance passed: focused={focused}, raised=8px, bottom gap=16px, joined/no ring', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    root = args.output.resolve()
    if len(str(root / 'runtime/daemon.sock').encode()) >= 104:
        parser.error('output path is too long for a private Unix socket')
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    env['JCODE_NO_TELEMETRY'] = '1'
    env['JCODE_RUNTIME_DIR'] = str(root / 'runtime')
    env['JCODE_API_SOCKET'] = str(root / 'runtime/api.sock')
    env['JCODE_SOCKET'] = str(root / 'runtime/daemon.sock')
    env['VK_DRIVER_FILES'] = str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json')))
    jcode = shutil.which('jcode')
    assert jcode, 'jcode must be installed'
    env['PATH'] = str(Path(jcode).parent) + ':/usr/bin:/bin'
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
        (root / name).mkdir(mode=0o700)
    processes = []
    logs = []

    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        p = subprocess.Popen(command, cwd=root, env=env, stdout=log,
                             stderr=log, start_new_session=True, **kwargs)
        processes.append(p)
        return p

    def wait_until(predicate, label, timeout=45):
        deadline = time.monotonic() + timeout
        while not predicate():
            assert all(p.poll() is None for p in processes), 'Child process exited: ' + label
            if time.monotonic() > deadline:
                raise RuntimeError('Timed out: ' + label)
            time.sleep(0.1)
        print(label, flush=True)

    def socket_ready(path):
        try:
            with socket.socket(socket.AF_UNIX) as s:
                s.settimeout(0.2)
                s.connect(path)
            return True
        except OSError:
            return False

    state_path = root / 'state'
    def state():
        return state_path.read_text() if state_path.exists() else ''

    def key(value):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', value], env=env, check=True, timeout=10)
        time.sleep(0.3)

    def capture(name, strip=True):
        time.sleep(0.7)
        path = root / (name + '.png')
        subprocess.run(['import', '-window', 'root', str(path)], env=env, check=True, timeout=15)
        (root / (name + '.state')).write_text(state())
        print(name + ': ' + state().splitlines()[0], flush=True)
        image = Image.open(path).convert('RGB')
        if strip:
            check_strip(image, int(re.search(r'focus=(\d+)', state())[1]))
        return image

    read_fd, write_fd = os.pipe()
    try:
        # Auto-detection refuses an empty credential store. This supported
        # explicit provider initializes lazily, allowing real session lifecycle
        # commands without authenticating or sending an inference request.
        launch('daemon', [jcode, '--no-update', '--no-selfdev', '--provider', 'jcode', 'serve'])
        wait_until(lambda: socket_ready(env['JCODE_SOCKET']), 'Real daemon ready')
        launch('bridge', [jcode, '--no-update', '--no-selfdev', 'api-bridge'])
        wait_until(lambda: socket_ready(env['JCODE_API_SOCKET']), 'Real harness API ready')
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1800x1000x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0], 'Xvfb startup timeout'
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit()
        env['DISPLAY'] = ':' + display
        wm_config = root / 'openbox.xml'
        wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm_config)])
        time.sleep(0.5)
        launch('desktop', [str(repo / 'target/debug/jcode-desktop')])
        wait_until(lambda: re.search(r'widths=\d', state()), 'First SDK-created panel rendered')
        key('super+1')
        for count in (2, 3):
            key('super+n')
            wait_until(lambda: len(state().split('widths=')[-1].splitlines()[0].split(',')) == count,
                       f'{count} SDK-created panels rendered')
            key('super+1')
        wait_until(lambda: 'widths=0.25,0.25,0.25' in state(), 'Three native width changes accepted')
        key('super+u')
        key('super+l')
        wait_until(lambda: 'focus=1 ' in state(), 'Native keyboard focuses middle folder')
        capture('middle-focused')
        for index in (0, 2):
            x = round(264 + (1800 - 264) * 0.25 * (index + 0.5))
            subprocess.run(['xdotool', 'mousemove', str(x), '500', 'click', '1'], env=env, check=True, timeout=10)
            wait_until(lambda: f'focus={index} ' in state(), f'Native click focuses folder {index}')
            capture(f'folder-{index}-focused')
        subprocess.run(['xdotool', 'mousemove', '5', '995'], env=env, check=True, timeout=10)
        key('super+o')
        overview = capture('overview', strip=False)
        cards = []
        for color, pixels in groupby(range(264, 1800), lambda x: overview.getpixel((x, 500))):
            pixels = list(pixels)
            if color in ((48, 43, 39), (37, 34, 31)) and len(pixels) >= 250:
                cards.append((pixels[0], pixels[-1], color))
        assert len(cards) == 3, 'Overview must show all three real sessions'
        assert cards[2][2] == (37, 34, 31), 'Overview must identify the active session'
        left, right, _ = cards[0]
        subprocess.run(['xdotool', 'mousemove', str((left + right) // 2), '500'], env=env, check=True, timeout=10)
        hovered = capture('overview-hover', strip=False)
        assert hovered.getpixel((left, 500)) == hovered.getpixel((left + 4, 500)) == (41, 37, 33)
        subprocess.run(['xdotool', 'click', '1'], env=env, check=True, timeout=10)
        wait_until(lambda: 'focus=0 ' in state(), 'Native overview selection focuses left folder')
        capture('overview-selected')
        log = (root / 'logs/jcode-desktop/jcode-desktop.log').read_text()
        sessions = set(re.findall(r'adopting created session (session_\S+)', log))
        assert len(sessions) == 3, f'Expected three real runtime session IDs, got {sessions}'
        assert 'screenshot-fixture' not in log
        print(f'PASS: 3 real SDK sessions, keyboard/pointer focus, overview hover/selection, and rendered geometry/colors. Artifacts: {root}', flush=True)
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
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


if __name__ == '__main__':
    main()
