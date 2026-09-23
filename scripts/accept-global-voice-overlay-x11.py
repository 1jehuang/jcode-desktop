#!/usr/bin/env python3
"""Accept the real X11 voice pill (WindowKind::PopUp) on a private Xvfb.

Never builds. Requires a current --binary. Uses the same preview-only fixture
as accept-global-voice-overlay.py (JCODE_DESKTOP_GLOBAL_VOICE_OVERLAY_FIXTURE
with JCODE_DESKTOP_SCREENSHOT=1): no microphone, voice network, global input
capture or daemon. WAYLAND_DISPLAY is never inherited, so GPUI selects X11.

Asserts, against a foreign focused X11 window under a private Openbox:
* listening/transcribing paint pixels only in a bottom-centered pill region,
* the pill is an override-redirect _NET_WM_WINDOW_TYPE_NOTIFICATION window,
* `xdotool getactivewindow` stays on the foreign window throughout,
* complete and owner shutdown remove every pill pixel.
Screenshots test rendering/lifetime with synthetic state, not real recording.
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

from screenshot import isolated_env

WIDTH, HEIGHT = 1280, 800
FOREIGN_TITLE = 'voice-overlay-x11-accept-target'


def changed_bounds(before, after):
    from PIL import ImageChops
    assert before.size == after.size
    return ImageChops.difference(before.convert('RGB'), after.convert('RGB')).getbbox()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='new artifact directory, must not exist')
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=45)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error('--timeout must be positive')
    binary = args.binary.resolve(strict=True)
    for tool in ('Xvfb', 'openbox', 'xdotool', 'xprop', 'import', 'display'):
        if not shutil.which(tool, path='/usr/bin:/bin'):
            parser.error('missing dependency: ' + tool)
    from PIL import Image
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'logs', 'jcode', 'tmp'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.pop('WAYLAND_DISPLAY', None)
    control = root / 'overlay.json'
    env.update(XDG_SESSION_TYPE='x11', JCODE_NO_TELEMETRY='1', TMPDIR=str(root / 'tmp'),
               PULSE_SERVER='unix:' + str(root / 'no-pulse'), PIPEWIRE_REMOTE='no-pipewire',
               ALSA_CONFIG_PATH=str(root / 'alsa.conf'))
    (root / 'alsa.conf').write_text('pcm.!default { type null }\nctl.!default { type null }\n')
    drivers = list(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if drivers:
        env['VK_DRIVER_FILES'] = str(drivers[0])
    wm_config = root / 'openbox.xml'
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc">'
                         '<focus><followMouse>no</followMouse></focus>'
                         '<applications><application class="*"><decor>no</decor>'
                         '</application></applications></openbox_config>')
    processes, logs, evidence = [], [], []

    def launch(name, command, extra=None, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        proc = subprocess.Popen(command, env={**env, **(extra or {})}, cwd=root,
                                stdout=log, stderr=log, start_new_session=True, **kwargs)
        processes.append(proc)
        return proc

    def wait(predicate, label):
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(.05)
        raise AssertionError('timeout: ' + label)

    def x(*command, check=True):
        result = subprocess.run(list(command), env=env, cwd=root, capture_output=True,
                                text=True, timeout=10)
        if check and result.returncode != 0:
            raise AssertionError((command, result.stderr))
        return result.stdout.strip()

    def capture(label):
        file = root / (label + '.png')
        x('import', '-window', 'root', str(file))
        with Image.open(file) as image:
            return image.convert('RGB')

    def active_window():
        return x('xdotool', 'getactivewindow', check=False)

    def check_focus(label):
        active = active_window()
        assert active == foreign, ('overlay or app stole focus', label, active, foreign)
        evidence.append({'checkpoint': label, 'active_window': active})

    def overlay_windows():
        """Mapped override-redirect notification windows owned by the app."""
        found = []
        ids = x('xdotool', 'search', '--onlyvisible', '--class', 'jcode-voice-overlay', check=False)
        for wid in filter(None, ids.split()):
            props = x('xprop', '-id', wid, '_NET_WM_WINDOW_TYPE', check=False)
            found.append({'id': wid, 'type': props})
        return found

    sequence = 0

    def command(state):
        nonlocal sequence
        sequence += 1
        temp = control.with_suffix('.tmp')
        temp.write_text(json.dumps({'sequence': sequence, 'state': state}))
        temp.replace(control)

        def acknowledged():
            try:
                ack = json.loads(Path(str(control) + '.ack').read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                return False
            if ack.get('sequence') != sequence:
                return False
            assert 'error' not in ack, ('fixture failed', ack)
            return ack.get('state') == state
        wait(acknowledged, 'preview fixture acknowledgement: ' + state)
        assert app.poll() is None, 'owner exited during fixture'

    def visible(image):
        box = changed_bounds(baseline, image)
        if box is None:
            return False
        assert box[0] >= WIDTH / 2 - 110 and box[2] <= WIDTH / 2 + 110, ('pixels outside pill', box)
        assert box[1] >= HEIGHT - 80, ('pill not bottom anchored', box)
        assert (box[2] - box[0]) * (box[3] - box[1]) > 500, ('pill too small', box)
        evidence.append({'pill_bounds': box})
        return True

    def wait_frame(label, predicate):
        def sample():
            check_focus(label)
            image = capture(label)
            return image if predicate(image) else None
        image = wait(sample, label + ' X11 pixels')
        time.sleep(.3)
        check_focus(label)
        image = capture(label)
        assert predicate(image), ('unstable pixels', label)
        return image

    read_fd, write_fd = os.pipe()
    try:
        xvfb = launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                               f'{WIDTH}x{HEIGHT}x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0], 'Xvfb did not become ready'
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit(), 'Xvfb failed to allocate a display'
        env['DISPLAY'] = ':' + display
        launch('openbox', ['openbox', '--sm-disable', '--config-file', str(wm_config)])
        time.sleep(.5)
        # A foreign, full-screen, focusable X11 client with a flat background.
        target_image = root / 'target.png'
        Image.new('RGB', (WIDTH, HEIGHT), (24, 32, 42)).save(target_image)
        launch('target', ['display', '-title', FOREIGN_TITLE, '-geometry', f'{WIDTH}x{HEIGHT}+0+0',
                          str(target_image)])
        foreign = wait(lambda: x('xdotool', 'search', '--onlyvisible', '--name', FOREIGN_TITLE,
                                 check=False).split()[:1], 'foreign target')[0]
        app = launch('app', [str(binary), '--no-hot-reload'],
                     {'JCODE_DESKTOP_GLOBAL_VOICE_OVERLAY_FIXTURE': str(control)})
        command('complete')
        # The app window may have taken focus at launch. Put the user back in
        # the foreign app, full screen and on top, then never touch focus again.
        def refocus():
            x('xdotool', 'windowactivate', foreign, check=False)
            time.sleep(.2)
            return active_window() == foreign
        wait(refocus, 'foreign window focused')
        x('xdotool', 'windowraise', foreign)
        time.sleep(.5)
        check_focus('baseline')
        baseline = capture('baseline')
        assert not overlay_windows(), 'pill visible before fixture'
        command('listening')
        listening = wait_frame('listening', visible)
        windows = overlay_windows()
        assert windows, 'no mapped jcode-voice-overlay X11 window'
        for window in windows:
            assert '_NET_WM_WINDOW_TYPE_NOTIFICATION' in window['type'], ('wrong window type', window)
            attrs = x('xprop', '-id', window['id'], 'WM_STATE', check=False)
            # Override-redirect windows are never managed, so the WM sets no WM_STATE.
            assert 'Normal' not in attrs, ('pill is a WM-managed window', attrs)
        evidence.append({'overlay_windows': windows})
        command('transcribing')
        wait_frame('transcribing', lambda image: visible(image) and changed_bounds(listening, image) is not None)
        command('complete')
        wait_frame('complete', lambda image: changed_bounds(baseline, image) is None)
        command('complete')  # repeated completion is harmless
        command('listening')
        wait_frame('reopened', visible)
        app.terminate()
        app.wait(timeout=args.timeout)
        wait_frame('owner-shutdown', lambda image: changed_bounds(baseline, image) is None)
        evidence.append({'result': 'passed',
                         'scope': 'synthetic preview lifecycle, real X11 PopUp rendering on Xvfb+Openbox'})
        print('PASS: X11 pill visible bottom-center above focused foreign window, focus unchanged, removed on completion and owner shutdown')
    finally:
        if write_fd is not None:
            os.close(write_fd)
        os.close(read_fd)
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2) + '\n')
        for proc in reversed(processes):
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait(timeout=5)
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
