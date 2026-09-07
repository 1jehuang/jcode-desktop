#!/usr/bin/env python3
"""Hold close on a private Xvfb display with production UI and offline panels.

No compositor commands, real sessions, credentials, or active-desktop input.
Use --picker to insert the default-directory utility before holding close.
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import time

from screenshot import isolated_env
from default_directory_acceptance import NativeUI
from model_picker_acceptance import phrase_bounds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug/jcode-desktop')
    parser.add_argument('--picker', action='store_true')
    parser.add_argument('--start', choices=('first', 'middle', 'last'), default='middle')
    parser.add_argument('--rate', type=int, default=25)
    parser.add_argument('--alias', action='store_true', help='use Ctrl+Shift+W instead of Super+Q')
    args = parser.parse_args()
    if not 1 <= args.rate <= 100:
        parser.error('--rate must be between 1 and 100 Hz')
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.update({
        'JCODE_DESKTOP_SCREENSHOT_PANELS': '6',
        'JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT': 'empty',
        'VK_DRIVER_FILES': str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))),
        'JCODE_DESKTOP_CONFIG': str(root / 'desktop.toml'),
    })
    (root / 'desktop.toml').write_text('[workspace]\ncoaching_hints = false\n')
    wm = root / 'openbox.xml'
    wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
    processes, logs, trace = [], [], []

    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log, **kwargs)
        processes.append(process)
        return process

    def native(*command):
        subprocess.run(['xdotool', *command], env=env, check=True, timeout=10)

    def state():
        try:
            line = next(line for line in (root / 'state').read_text().splitlines() if line.startswith('navigation='))
            return json.loads(line.split('=', 1)[1])
        except (OSError, StopIteration, ValueError):
            return None

    def live(current):
        return [p for r in current['rows'] for p in r['panels'] if not p.get('closing')]

    def wait(predicate, label):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            assert all(p.poll() is None for p in processes), label + ': child exited'
            current = state()
            if current and predicate(current):
                trace.append({'checkpoint': label, 'state': current})
                return current
            time.sleep(.05)
        raise AssertionError(label + ': ' + json.dumps(state()))

    def capture(label):
        subprocess.run(['import', '-window', 'root', str(root / (label + '.png'))], env=env, check=True, timeout=10)

    read_fd, write_fd = os.pipe()
    try:
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', '1440x1000x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0]
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit()
        env['DISPLAY'] = ':' + display
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm)])
        time.sleep(.5)
        launch('app', [str(args.binary.resolve()), '--no-hot-reload'])
        wait(lambda s: len(live(s)) == 6, 'six offline panes')
        time.sleep(1)
        if args.picker:
            ui = NativeUI(root / 'picker.png', env, root)
            bounds = ui.wait_frame('open-picker', lambda image: phrase_bounds(
                ui.words(image, (0, 48, 264, 110), 'header'), 'Default directory'))
            ui.click(bounds)
            wait(lambda s: any(p['session'] == 'settings://default-directory' for p in live(s)), 'picker opened')
            # Close from the right so picker removal encounters a fading neighbor.
            native('key', '--clearmodifiers', 'super+End')
            time.sleep(.4)
        elif args.start != 'middle':
            native('key', '--clearmodifiers', 'super+Home' if args.start == 'first' else 'super+End')
            time.sleep(.4)
        capture('before-hold')
        trace.append({'checkpoint': 'before-hold', 'state': state()})
        # Configure only our private X server. Xlib is already required by GPUI.
        import ctypes
        x11 = ctypes.CDLL('libX11.so.6')
        x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        x11.XOpenDisplay.restype = ctypes.c_void_p
        x11.XkbSetAutoRepeatRate.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_uint, ctypes.c_uint]
        x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        display_handle = x11.XOpenDisplay(env['DISPLAY'].encode())
        assert display_handle
        try:
            assert x11.XkbSetAutoRepeatRate(display_handle, 0x100, 250, max(1, 1000 // args.rate))
        finally:
            x11.XCloseDisplay(display_handle)
        modifiers, key = (['Control_L', 'Shift_L'], 'w') if args.alias else (['Super_L'], 'q')
        for modifier in modifiers:
            native('keydown', modifier)
        native('keydown', key)
        try:
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                current = state()
                if current:
                    trace.append({'checkpoint': 'held', 'state': current})
                time.sleep(.04)
        finally:
            native('keyup', key)
            for modifier in reversed(modifiers):
                native('keyup', modifier)
        time.sleep(.4)
        capture('after-hold')
        current = state()
        trace.append({'checkpoint': 'released', 'state': current})
        assert current and not live(current), 'Held close stalled with live panes: ' + json.dumps(current)
        native('key', '--clearmodifiers', 'super+n')
        reopened = wait(lambda s: len(live(s)) == 1, 'empty workspace reopens')
        time.sleep(.5)
        assert len(live(state())) == 1, 'Close continued after release'
        assert reopened['keyboard_panel'] == reopened['focused_slot'], reopened
        print('PASS: held close empties workspace and release leaves reopened pane alive', flush=True)
    finally:
        (root / 'trace.json').write_text(json.dumps(trace, indent=2) + '\n')
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
                    process.wait()
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
