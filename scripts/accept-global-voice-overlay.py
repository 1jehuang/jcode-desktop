#!/usr/bin/env python3
"""Accept the real native voice layer surface on a private headless Sway.

Never builds. Requires a current --binary and the preview-only fixture described
below. No input injection or audio capture.

Fixture contract (main UI owner must wire this, never in production startup):
* Only honor JCODE_DESKTOP_GLOBAL_VOICE_OVERLAY_FIXTURE with JCODE_DESKTOP_SCREENSHOT=1.
* Poll the named JSON file: {"sequence":N,"state":"listening"|"transcribing"|"complete"}.
* listening: open the real global_voice_overlay::open(Snapshot), fixed RMS bars.
* transcribing: VoiceOverlay::set_snapshot with different text and no bars.
* complete: remove/drop the native window, including repeated complete calls.
* Atomically write <control-path>.ack with the applied sequence and state.
* Never start microphone, voice network, global input capture or daemon.
This uses actual layer-shell rendering with synthetic state, not real recording.
Screenshots and protocol evidence test rendering/lifetime, not real recording.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import time

from screenshot import isolated_env


def walk(node):
    yield node
    for child in node.get('nodes', []) + node.get('floating_nodes', []):
        yield from walk(child)


def target_state(tree):
    targets = [n for n in walk(tree) if n.get('app_id') == 'voice-overlay-accept-target']
    assert len(targets) == 1, ('expected one native target', targets)
    target = targets[0]
    assert target.get('focused'), ('overlay stole focus', target)
    return {k: target.get(k) for k in ('id', 'rect', 'window_rect', 'deco_rect', 'fullscreen_mode')}


def assert_native_protocol(text):
    """Tie assertions to the overlay object, not unrelated app layer surfaces."""
    matches = re.findall(r'get_layer_surface\(new id zwlr_layer_surface_v1[@#](\d+),[^\n]*, 3, "jcode-voice-overlay"\)', text)
    assert matches, 'no native overlay-layer request in WAYLAND_DEBUG log'
    for object_id in set(matches):
        prefix = r'zwlr_layer_surface_v1[@#]' + object_id + r'\.'
        assert re.search(prefix + r'set_keyboard_interactivity\(0\)', text), 'overlay must never accept keyboard focus'
        assert re.search(prefix + r'set_exclusive_zone\(0\)', text), 'overlay must not reserve workspace space'
        assert not re.search(prefix + r'set_keyboard_interactivity\([1-9]', text), 'interactive overlay'
        assert not re.search(prefix + r'set_exclusive_zone\((?!0\))', text), 'nonzero exclusive zone'
    return sorted(set(matches))


def changed_bounds(before, after):
    from PIL import ImageChops
    assert before.size == after.size
    return ImageChops.difference(before.convert('RGB'), after.convert('RGB')).getbbox()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='new artifact directory, must not exist')
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--sway-prefix', type=Path)
    parser.add_argument('--timeout', type=float, default=30)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error('--timeout must be positive')
    binary = args.binary.resolve(strict=True)
    prefix = args.sway_prefix.resolve(strict=True) if args.sway_prefix else None
    path = (str(prefix / 'usr/bin') + ':' if prefix else '') + '/usr/bin:/bin'
    for tool in ('sway', 'swaymsg', 'foot', 'grim'):
        if not shutil.which(tool, path=path):
            parser.error('missing dependency: ' + tool)
    from PIL import Image
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'logs', 'jcode', 'tmp'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    control = root / 'overlay.json'
    env.update(PATH=path, WLR_BACKENDS='headless', WLR_RENDERER='pixman',
               WLR_LIBINPUT_NO_DEVICES='1', WLR_HEADLESS_OUTPUTS='1',
               XDG_SESSION_TYPE='wayland', JCODE_NO_TELEMETRY='1',
               TMPDIR=str(root / 'tmp'),
               PULSE_SERVER='unix:' + str(root / 'no-pulse'),
               PIPEWIRE_REMOTE='no-pipewire',
               ALSA_CONFIG_PATH=str(root / 'alsa.conf'))
    (root / 'alsa.conf').write_text('pcm.!default { type null }\nctl.!default { type null }\n')
    drivers = list(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if drivers:
        env['VK_DRIVER_FILES'] = str(drivers[0])
    if prefix:
        env['LD_LIBRARY_PATH'] = str(prefix / 'usr/lib')
    config = root / 'sway.config'
    config.write_text('xwayland disable\noutput HEADLESS-1 mode 1280x800\n'
                      'output * bg #18202a solid_color\nseat seat0 fallback true\n'
                      'default_border none\nfocus_follows_mouse no\n')
    processes, logs, evidence = [], [], []

    def launch(name, command, extra=None):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        proc = subprocess.Popen(command, env={**env, **(extra or {})}, cwd=root,
                                stdout=log, stderr=log, start_new_session=True)
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

    def ipc(*command):
        return json.loads(subprocess.check_output(['swaymsg', '-r', *command], env=env, text=True, timeout=5))

    def capture(label):
        file = root / (label + '.png')
        subprocess.run(['grim', '-o', 'HEADLESS-1', str(file)], env=env, check=True, timeout=10)
        with Image.open(file) as image:
            return image.convert('RGB')

    def check_target(label):
        tree = ipc('-t', 'get_tree')
        assert target_state(tree) == baseline_state, ('target geometry changed', label)
        assert not any(n.get('app_id') == 'jcode-voice-overlay' for n in walk(tree)), 'overlay is a normal workspace window'
        evidence.append({'checkpoint': label, 'target': target_state(tree)})
        (root / (label + '-tree.json')).write_text(json.dumps(tree, indent=2))

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
                return ack.get('sequence') == sequence and ack.get('state') == state
            except (FileNotFoundError, json.JSONDecodeError):
                return False
        wait(acknowledged, 'preview fixture acknowledgement: ' + state)
        assert app.poll() is None, 'owner exited during fixture'

    def wait_frame(label, predicate):
        # Multiple frames also catch a focus/geometry change after fixture ack.
        def sample():
            check_target(label)
            image = capture(label)
            return image if predicate(image) else None
        image = wait(sample, label + ' compositor pixels')
        time.sleep(.2)
        check_target(label)
        image = capture(label)
        assert predicate(image), ('unstable compositor pixels', label)
        return image

    try:
        launch('sway', ['sway', '--config', str(config)])
        env['SWAYSOCK'] = str(wait(lambda: next((root / 'runtime').glob('sway-ipc.*.sock'), None), 'private Sway IPC'))
        env['WAYLAND_DISPLAY'] = wait(lambda: next((p.name for p in (root / 'runtime').glob('wayland-*') if p.is_socket()), None), 'private Wayland socket')
        command_file = root / 'target.sh'
        command_file.write_text("#!/bin/sh\nprintf '\\033[?25l\\033[2J\\033[HVOICE OVERLAY ACCEPTANCE TARGET'\nexec sleep 600\n")
        launch('target', ['foot', '--app-id=voice-overlay-accept-target', '--', '/bin/sh', str(command_file)])
        wait(lambda: any(n.get('app_id') == 'voice-overlay-accept-target' for n in walk(ipc('-t', 'get_tree'))), 'native target')
        app = launch('app', [str(binary), '--no-hot-reload'], {
            'JCODE_DESKTOP_GLOBAL_VOICE_OVERLAY_FIXTURE': str(control), 'WAYLAND_DEBUG': 'client',
        })
        command('complete')
        ipc('[app_id="voice-overlay-accept-target"] focus')
        ipc('[app_id="voice-overlay-accept-target"] fullscreen enable')
        baseline_state = target_state(ipc('-t', 'get_tree'))
        time.sleep(.3)
        baseline = capture('baseline')
        command('listening')
        def visible(image):
            box = changed_bounds(baseline, image)
            if box is None:
                return False
            width, height = image.size
            assert box[0] >= width / 2 - 160 and box[2] <= width / 2 + 160, ('pixels outside overlay', box)
            assert box[1] >= height - 110, ('overlay not bottom anchored', box)
            assert (box[2] - box[0]) * (box[3] - box[1]) > 500, ('overlay too small', box)
            return True
        listening = wait_frame('listening', visible)
        command('transcribing')
        wait_frame('transcribing', lambda image: visible(image) and changed_bounds(listening, image) is not None)
        command('complete')
        wait_frame('complete', lambda image: changed_bounds(baseline, image) is None)
        command('listening')
        wait_frame('reopened', visible)
        app.terminate()
        app.wait(timeout=args.timeout)
        wait_frame('owner-shutdown', lambda image: changed_bounds(baseline, image) is None)
        # Production redirects stderr into XDG_STATE_HOME after startup.
        diagnostic = root / 'logs/jcode-desktop/jcode-desktop.log'
        protocol = (root / 'app.log').read_text(errors='replace')
        if diagnostic.exists():
            protocol += diagnostic.read_text(errors='replace')
        objects = assert_native_protocol(protocol)
        evidence.append({'native_layer_objects': objects, 'result': 'passed',
                         'scope': 'synthetic preview lifecycle, real native compositor rendering'})
        print('PASS: native overlay visible above focused target, no focus/layout reservation, removed on completion and owner shutdown')
    finally:
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
