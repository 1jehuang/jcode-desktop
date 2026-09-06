#!/usr/bin/env python3
"""Exercise folder focus through native input and real isolated SDK sessions.

No screenshot fixtures, inherited credentials, model requests, or user desktop
sockets are used. Requires current desktop and jcode binaries plus Xvfb tools.
"""
import argparse
from itertools import groupby
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import time

from PIL import Image, ImageDraw
from screenshot import isolated_env


def check_strip(image, focused, count=3):
    """Assert visible public output, not a copied layout implementation."""
    sheet, backing = (37, 34, 31), (48, 43, 39)
    fraction = 0.5 if count == 2 else 0.25
    panel_width = (image.width - 288) * fraction
    for index in range(count):
        x = round(296 + panel_width * index)
        surface = sheet if index == focused else backing
        assert image.getpixel((x, 300)) == surface, 'Pane color must follow focus'
        assert image.getpixel((x, 983)) == surface, 'Focus color extends to the bottom edge'
        assert all(image.getpixel((x, y)) == backing for y in range(984, 1000))
        assert image.getpixel((x, 50)) == surface, 'Focus color starts below the tabs'
        assert image.getpixel((x, 8)) == backing, 'Keep background above the tabs'
    # The full-height gutter is deliberately independent of the sidebar selection.
    assert all(image.getpixel((270, y)) == backing for y in range(16, 984)), 'No sidebar-to-panel connector'
    assert image.getpixel((image.width - 6, 500)) == backing, 'Keep the outer right edge visible'
    assert image.getpixel((image.width - 30, 24)) == backing, 'No wide shoulder behind the live tabs'
    selected_rows = []
    for color, rows in groupby(range(60, 450), lambda y: image.getpixel((250, y))):
        rows = list(rows)
        if color == sheet and len(rows) >= 20:
            selected_rows.append(rows)
    assert len(selected_rows) == 1, 'The real sidebar shows exactly one selected session'
    selected = (250, selected_rows[0][len(selected_rows[0]) // 2])
    page = image.copy()
    marker = (255, 0, 255)
    ImageDraw.floodfill(page, (296, 300), marker)
    assert page.getpixel(selected) != marker, 'Sidebar selection must remain independent of the session sheet'
    print(f'Pixel acceptance passed: focused={focused}, level body, live-tab space, independent sidebar', flush=True)



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--panels', type=int, choices=(2, 3), default=3)
    args = parser.parse_args()
    count = args.panels
    fraction = 0.5 if count == 2 else 0.25
    width_key = 'super+2' if count == 2 else 'super+1'
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
    env['JCODE_DESKTOP_CONFIG'] = str(root / 'desktop.toml')
    (root / 'desktop.toml').write_text('[workspace]\nsession_refresh_seconds = 5\ncoaching_hints = false\n')
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

    def name_sessions(ids):
        # Exercise the same public API used by SDK rename_session. Naming a
        # fresh session persists its metadata without sending a model prompt.
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(15)
            client.connect(env['JCODE_API_SOCKET'])
            with client.makefile('rwb') as stream:
                requests = [dict(req='hello', min_version=1, max_version=1, client='folder-acceptance/1')]
                for i, session in enumerate(ids):
                    requests.extend([
                        dict(req='attach_session', session_id=session),
                        dict(req='rename_session', session_id=session, title=f'Folder acceptance {i + 1}'),
                        dict(req='detach_session', session_id=session),
                    ])
                requests.append(dict(req='list_sessions'))
                for request_id, request in enumerate(requests, 1):
                    stream.write((json.dumps(dict(v=1, id=request_id, **request)) + '\n').encode())
                    stream.flush()
                    while True:
                        raw = stream.readline()
                        assert raw, 'API closed before metadata reply'
                        reply = json.loads(raw)
                        if reply.get('reply_to') == request_id:
                            assert reply['ev'] != 'error', reply
                            break
                assert set(ids) <= {s['session_id'] for s in reply['sessions']}, 'Renamed sessions must enter the real catalog'

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
            check_strip(image, int(re.search(r'focus=(\d+)', state())[1]), count)
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
        key(width_key)
        for created in range(2, count + 1):
            key('super+n')
            wait_until(lambda: len(state().split('widths=')[-1].splitlines()[0].split(',')) == created,
                       f'{created} SDK-created panels rendered')
            key(width_key)
        expected_widths = 'widths=' + ','.join([f'{fraction:.2f}'] * count)
        wait_until(lambda: expected_widths in state(), 'Native width changes accepted')
        unnamed = capture('unnamed-folders', strip=False)
        for index in range(count):
            left = round(276 + (1800 - 288) * fraction * index)
            top = 48
            ink = sum(
                unnamed.getpixel((x, y)) not in ((37, 34, 31), (48, 43, 39))
                for x in range(left + 12, left + 160)
                for y in range(top + 8, top + 22)
            )
            assert ink > 30, 'An unnamed real session must still paint its folder label'
        diagnostics = root / 'logs/jcode-desktop/jcode-desktop.log'
        ids = list(dict.fromkeys(re.findall(r'adopting created session (session_\S+)', diagnostics.read_text())))
        assert len(ids) == count
        refreshes = diagnostics.read_text().count('session list completed')
        name_sessions(ids)
        # Use the supported five-second refresh setting and wait for its actual
        # bridge response. A bounded runtime catalog can omit open sessions;
        # Workspace must retain their real SDK-backed panel identities anyway.
        wait_until(lambda: diagnostics.read_text().count('session list completed') > refreshes,
                   'Real catalog refresh completed; open-session sidebar preserved')
        key('super+u')
        key('super+l')
        wait_until(lambda: 'focus=1 ' in state(), 'Native keyboard focuses middle folder')
        capture('middle-focused')
        for index in (0, count - 1):
            x = round(276 + (1800 - 288) * fraction * (index + 0.5))
            subprocess.run(['xdotool', 'mousemove', str(x), '500', 'click', '1'], env=env, check=True, timeout=10)
            wait_until(lambda: f'focus={index} ' in state(), f'Native click focuses folder {index}')
            capture(f'folder-{index}-focused')
        # The first labeled live tab is always reachable at the left edge.
        # This is native pointer input, not direct workspace state mutation.
        subprocess.run(['xdotool', 'mousemove', '310', '35', 'click', '1'], env=env, check=True, timeout=10)
        wait_until(lambda: 'focus=0 ' in state(), 'Native live-session folder tab focuses first session')
        capture('live-tab-selected')
        subprocess.run(['xdotool', 'mousemove', '5', '995'], env=env, check=True, timeout=10)
        key('super+o')
        overview = capture('overview', strip=False)
        cards = []
        for header_y in range(200, 600):
            bands = []
            for color, pixels in groupby(range(276, 1800), lambda x: overview.getpixel((x, header_y))):
                pixels = list(pixels)
                if color == (48, 43, 39) and 280 <= len(pixels) <= 320:
                    bands.append((pixels[0], pixels[-1]))
            if len(bands) == count:
                cards = bands
                break
        assert len(cards) == count, 'Overview must show all real sessions'
        body_y = header_y + 70
        assert overview.getpixel((cards[0][0] + 4, body_y)) == (37, 34, 31)
        left, right = cards[0]
        subprocess.run(['xdotool', 'mousemove', str((left + right) // 2), str(body_y)], env=env, check=True, timeout=10)
        hovered = capture('overview-hover', strip=False)
        assert hovered.getpixel((left, body_y)) == hovered.getpixel((left + 4, body_y)) == (41, 37, 33)
        subprocess.run(['xdotool', 'click', '1'], env=env, check=True, timeout=10)
        wait_until(lambda: 'focus=0 ' in state(), 'Native overview selection focuses left folder')
        capture('overview-selected')
        # Use the real navigation scrollbar and Settings tab, not direct app
        # state mutation. The two modes must be selectable and persistent.
        def click(x, y):
            subprocess.run(['xdotool', 'mousemove', str(x), str(y), 'click', '1'], env=env, check=True, timeout=10)
        # Settings sits midway through the overflow tabs, before the action tabs.
        # Native wheel scrolling must move the visible tab outlines, with no
        # detached scrollbar rail in the old top gutter.
        before_scroll = capture('outline-scroll-start', strip=False)
        assert all(before_scroll.getpixel((x, 5)) == (48, 43, 39) for x in range(12, 250))
        subprocess.run(['xdotool', 'mousemove', '140', '36', 'click', '--repeat', '3', '--delay', '100', '5'], env=env, check=True, timeout=10)
        time.sleep(0.3)
        after_scroll = capture('outline-scroll-wheel', strip=False)
        assert before_scroll.crop((12, 16, 250, 50)).tobytes() != after_scroll.crop((12, 16, 250, 50)).tobytes(), 'Native wheel input must move the navigation tabs'
        click(132, 20)
        time.sleep(0.3)
        click(142, 36)
        capture('settings-navigation', strip=False)
        wait_until(lambda: 'sidebar=Settings' in state(), 'Native Settings tab opened', timeout=10)
        settings = capture('settings-folder-mode', strip=False)
        controls = []
        for color, rows in groupby(range(100, 400), lambda y: settings.getpixel((16, y))):
            rows = list(rows)
            if color == (37, 34, 31) and 24 <= len(rows) <= 60:
                controls.append(rows)
        assert len(controls) == 3, 'Find the three actual native Settings controls'
        mode_y = controls[-1][len(controls[-1]) // 2]
        click(130, mode_y)
        wait_until(lambda: 'layout=Normal' in state(), 'Native Settings switched to Normal mode')
        assert 'layout_mode = "normal"' in (root / 'desktop.toml').read_text()
        normal = capture('normal-mode', strip=False)
        normal_width = (1800 - 264) * fraction
        # Separate cards have a real background gap, not a connected sheet.
        assert normal.getpixel((round(264 + normal_width), 300)) == (28, 26, 24)
        for i in range(count):
            assert normal.getpixel((round(284 + normal_width * i), 300)) == (37, 34, 31)
        click(130, mode_y)
        wait_until(lambda: 'layout=FolderTabs' in state(), 'Native Settings restored Folder tabs mode')
        assert 'layout_mode = "folder_tabs"' in (root / 'desktop.toml').read_text()
        click(14, 20)
        time.sleep(0.3)
        click(35, 36)
        wait_until(lambda: 'sidebar=Sessions' in state(), 'Native chat navigation restored')
        capture('folder-mode-restored')
        log = (root / 'logs/jcode-desktop/jcode-desktop.log').read_text()
        sessions = set(re.findall(r'adopting created session (session_\S+)', log))
        assert len(sessions) == count, f'Expected {count} real runtime session IDs, got {sessions}'
        assert 'screenshot-fixture' not in log
        print(f'PASS: {count} real SDK sessions, keyboard/pointer focus, overview hover/selection, and rendered geometry/colors. Artifacts: {root}', flush=True)
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
