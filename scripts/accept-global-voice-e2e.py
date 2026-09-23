#!/usr/bin/env python3
"""End-to-end unfocused hold-to-talk on a private Wayland compositor.

Unlike accept-global-voice-overlay.py this runs the production path, with no
fixture. The pieces are:

* Private headless Sway with a focused foreign app (foot). Jcode is never focused.
* A virtual Copilot key from scripts/virtual_copilot_key.py. udev tags it as a
  joystick so the live compositor ignores it. The script checks that before pressing.
* A private virtual source fed by a null sink. Jcode's capture stream is pinned
  to it with PIPEWIRE_PROPS target.object and fallback disabled. The script
  asserts the live PipeWire link targets only that source and aborts before any
  speech is played otherwise.
* Real Nari transcription with the user's existing key. This costs a few cents.

It passes when the native layer-shell pill shows while the foreign app keeps
focus, the chat draft grows by the transcript, and the pill is then removed.
Only lengths are logged, never transcript or draft text.

Usage: accept-global-voice-e2e.py OUT --binary target/debug/jcode-desktop \
          --sway-prefix target/headless-sway-tools --speech phrase.wav
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

from screenshot import isolated_env

REPO = Path(__file__).resolve().parents[1]
SINK = 'jcode_voice_e2e'
MIC = 'jcode_voice_e2e_mic'


def walk(node):
    yield node
    for child in node.get('nodes', []) + node.get('floating_nodes', []):
        yield from walk(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--sway-prefix', type=Path)
    parser.add_argument('--speech', type=Path, required=True, help='WAV phrase to speak into the virtual mic')
    parser.add_argument('--nari-env', type=Path, default=Path.home() / '.config/jcode/nari.env')
    parser.add_argument('--timeout', type=float, default=45)
    parser.add_argument('--min-chars', type=int, default=40,
                        help='minimum transcript length, to catch clipped recordings')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    prefix = args.sway_prefix.resolve(strict=True) if args.sway_prefix else None
    path = (str(prefix / 'usr/bin') + ':' if prefix else '') + '/usr/bin:/bin'
    for tool in ('sway', 'swaymsg', 'foot', 'grim', 'pactl', 'paplay', 'wtype'):
        if not shutil.which(tool, path=path):
            parser.error('missing dependency: ' + tool)
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'logs', 'jcode', 'tmp', 'jcode/config/jcode'):
        (root / name).mkdir(mode=0o700, parents=True)
    shutil.copy(args.nari_env, root / 'jcode/config/jcode/nari.env')
    host_env = dict(os.environ)
    env = isolated_env(root)
    # Production path: no screenshot fixture, but still private dirs and sockets.
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    state_file = root / 'state'
    env.update(PATH=path, WLR_BACKENDS='headless', WLR_RENDERER='pixman',
               WLR_LIBINPUT_NO_DEVICES='1', WLR_HEADLESS_OUTPUTS='1',
               XDG_SESSION_TYPE='wayland', JCODE_NO_TELEMETRY='1',
               TMPDIR=str(root / 'tmp'), JCODE_DESKTOP_STATE=str(state_file),
               # logind is consulted over the system bus, like production.
               XDG_SESSION_ID=host_env.get('XDG_SESSION_ID', ''))
    drivers = list(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if drivers:
        env['VK_DRIVER_FILES'] = str(drivers[0])
    sway_env = dict(env)
    if prefix:
        sway_env['LD_LIBRARY_PATH'] = str(prefix / 'usr/lib')
    (root / 'sway.config').write_text('xwayland disable\noutput HEADLESS-1 mode 1280x800\n'
                                      'output * bg #18202a solid_color\nseat seat0 fallback true\n'
                                      'default_border none\nfocus_follows_mouse no\n')
    processes, logs, evidence = [], [], {'checks': []}
    modules = []
    key = None

    def check(name, ok, detail=None):
        evidence['checks'].append({'check': name, 'ok': bool(ok), 'detail': detail})
        print(('PASS ' if ok else 'FAIL ') + name + (f': {detail}' if detail is not None else ''), flush=True)
        if not ok:
            raise AssertionError(name)

    def launch(name, command, extra=None, stdin=None, stdout=None):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        proc = subprocess.Popen(command, env=extra or env, cwd=root, stdin=stdin,
                                stdout=stdout or log, stderr=log, start_new_session=True, text=True)
        processes.append(proc)
        return proc

    def wait(predicate, label, timeout=None):
        deadline = time.monotonic() + (timeout or args.timeout)
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(.1)
        raise AssertionError('timeout: ' + label)

    def ipc(*command):
        return json.loads(subprocess.check_output(['swaymsg', '-r', *command], env=sway_env, text=True, timeout=5))

    def app_log():
        text = ''
        for file in (root / 'app.log', root / 'logs/jcode-desktop/jcode-desktop.log'):
            if file.exists():
                text += file.read_text(errors='replace')
        return text

    def panel():
        try:
            nav = json.loads(state_file.read_text().split('navigation=', 1)[1])
        except (FileNotFoundError, IndexError, json.JSONDecodeError):
            return None
        panels = [p for row in nav.get('rows', []) for p in row.get('panels', [])]
        return panels[0] if panels else None

    def capture(label):
        file = root / (label + '.png')
        subprocess.run(['grim', '-o', 'HEADLESS-1', str(file)], env=sway_env, check=True, timeout=10)
        return file

    def os_pills():
        # Layer surfaces never appear in the Sway window tree. Count app events.
        return app_log().count('global voice: OS pill shown')

    def events():
        return [l for l in app_log().splitlines() if l.startswith('global voice:')]

    def capture_targets(pid):
        """Source node names that the app's PipeWire capture streams are linked to."""
        dump = json.loads(subprocess.check_output(['pw-dump'], text=True, env=host_env))
        nodes = {o['id']: o for o in dump if o.get('type') == 'PipeWire:Interface:Node'}
        # ALSA stream nodes carry only client.id. The Client object carries the PID.
        clients = {o['id'] for o in dump if o.get('type') == 'PipeWire:Interface:Client'
                   and str(o.get('info', {}).get('props', {}).get('application.process.id')) == str(pid)}
        mine = {i for i, o in nodes.items()
                if o.get('info', {}).get('props', {}).get('client.id') in clients}
        targets = set()
        for o in dump:
            if o.get('type') != 'PipeWire:Interface:Link':
                continue
            info = o.get('info', {})
            if info.get('input-node-id') in mine and info.get('output-node-id') in nodes:
                props = nodes[info['output-node-id']].get('info', {}).get('props', {})
                targets.add(props.get('node.name', '?'))
        return targets

    def foreign_focused():
        tree = ipc('-t', 'get_tree')
        target = [n for n in walk(tree) if n.get('app_id') == 'e2e-foreign-app']
        return bool(target) and target[0].get('focused')

    try:
        # 1. Isolated virtual microphone.
        modules.append(subprocess.check_output(['pactl', 'load-module', 'module-null-sink', f'sink_name={SINK}',
                                                'rate=16000', 'channels=1'], text=True).strip())
        modules.append(subprocess.check_output(['pactl', 'load-module', 'module-remap-source', f'master={SINK}.monitor',
                                                f'source_name={MIC}', 'rate=16000', 'channels=1'], text=True).strip())
        # 2. Virtual Copilot key, verified invisible to the live compositor.
        key = launch('key', ['python3', str(REPO / 'scripts/virtual_copilot_key.py')], extra=host_env,
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        node = key.stdout.readline().strip()
        check('virtual key created', node.startswith('/dev/input/event'), node)
        udev = subprocess.check_output(['udevadm', 'info', node], text=True)
        check('virtual key tagged as joystick', 'ID_INPUT_JOYSTICK=1' in udev)
        time.sleep(.5)
        live = [pid for pid in subprocess.run(['pgrep', '-x', 'niri|sway|kwin_wayland|gnome-shell|Hyprland|mutter'],
                                              capture_output=True, text=True).stdout.split()]
        for pid in live:
            fds = [os.readlink(f) for f in Path(f'/proc/{pid}/fd').iterdir() if f.is_symlink()]
            check(f'live compositor {pid} did not open the virtual key', node not in fds)
        (root / 'jcode/config.toml').write_text(
            f'[desktop.workspace]\naccount_sign_in_handled = true\n\n'
            f'[desktop.voice]\nglobal_hold = true\nglobal_devices = ["{node}"]\n')
        # 3. Private compositor with a focused foreign app.
        launch('sway', ['sway', '--config', str(root / 'sway.config')], extra=sway_env)
        sway_env['SWAYSOCK'] = env['SWAYSOCK'] = str(wait(lambda: next((root / 'runtime').glob('sway-ipc.*.sock'), None), 'sway ipc'))
        sway_env['WAYLAND_DISPLAY'] = env['WAYLAND_DISPLAY'] = wait(
            lambda: next((p.name for p in (root / 'runtime').glob('wayland-*') if p.is_socket()), None), 'wayland socket')
        # 4. Jcode Desktop with only the virtual microphone reachable.
        # Pin the capture stream to the virtual mic with fallback disabled. If the
        # target vanished, PipeWire fails the stream instead of using the real mic.
        # The live link is asserted during recording.
        app_env = dict(env, WAYLAND_DEBUG='client', PIPEWIRE_RUNTIME_DIR=host_env['XDG_RUNTIME_DIR'],
                       PIPEWIRE_PROPS=f'{{ target.object = {MIC} node.dont-fallback = true node.dont-reconnect = true }}')
        # A persistent virtual keyboard gives the private seat keyboard focus,
        # so the chat can become the last-focused Jcode window like real use.
        launch('vkbd', ['wtype', '-s', '100000000', 'x'], extra=sway_env)
        wait(lambda: 'keyboard' in json.dumps(ipc('-t', 'get_inputs')), 'virtual keyboard')
        app = launch('app', [str(binary), '--single-panel', '--no-hot-reload'], extra=app_env)
        wait(lambda: panel() is not None, 'jcode chat panel', timeout=60)
        wait(lambda: 'global voice: listening on 1 configured device(s)' in app_log(), 'global listener armed')
        check('global listener armed', True)
        wait(lambda: 'wl_keyboard' in app_log() and '.enter(' in app_log(), 'jcode received keyboard focus')
        check('jcode was focused once', True)
        launch('foreign', ['foot', '--app-id=e2e-foreign-app', 'sh', '-c', 'echo FOREIGN APP HAS FOCUS; exec sleep 600'], extra=sway_env)
        wait(lambda: any(n.get('app_id') == 'e2e-foreign-app' for n in walk(ipc('-t', 'get_tree'))), 'foreign app')
        ipc('[app_id="e2e-foreign-app"] focus')
        ipc('[app_id="e2e-foreign-app"] fullscreen enable')
        wait(foreign_focused, 'foreign app focused')
        time.sleep(3.5)  # let Jcode's inactive heartbeat publish; last-focused still wins
        before = panel()['draft_chars']
        check('foreign app focused before press', foreign_focused())
        # 5. Hold Copilot, speak, release.
        key.stdin.write('press\n'); key.stdin.flush()
        wait(lambda: 'global voice: press' in app_log(), 'press delivered to Jcode', timeout=5)
        check('press reached Jcode while unfocused', True)
        wait(lambda: os_pills() == 1, 'native OS pill created', timeout=10)
        check('OS pill shown while another app is focused', True)
        # The microphone opens only after Nari acknowledges the session.
        wait(lambda: 'global voice: recording started' in app_log(), 'microphone recording', timeout=15)
        check('recording started while unfocused', True)
        targets = wait(lambda: capture_targets(app.pid), 'capture stream linked', timeout=5)
        check('app captures only the private test source', targets == {MIC}, sorted(targets))
        time.sleep(.3)
        speech = subprocess.Popen(['paplay', f'--device={SINK}', str(args.speech)])
        time.sleep(1.5)
        capture('listening')  # mid-phrase, so the meter shows live levels
        check('foreign app kept focus while listening', foreign_focused())
        speech.wait(timeout=30)
        time.sleep(.6)
        key.stdin.write('release\n'); key.stdin.flush()
        wait(lambda: 'global voice: release' in app_log(), 'release delivered', timeout=5)
        capture('after-release')
        result = wait(lambda: ('inserted' if 'global voice: inserted' in app_log() else
                               'failed' if ('finished without text' in app_log() or 'failed to start' in app_log()
                                            or 'denied by logind' in app_log()) else None),
                      'transcript outcome', timeout=40)
        lines = [l for l in app_log().splitlines() if l.startswith('global voice:')]
        (root / 'voice-events.txt').write_text('\n'.join(lines) + '\n')
        check('transcript inserted', result == 'inserted', lines[-3:])
        check('foreign app still focused at insertion', foreign_focused())
        # An occluded GPUI window gets no frame callbacks, so its state dump
        # is stale. Unfullscreen the foreign app, keeping its focus, to let Jcode paint.
        ipc('[app_id="e2e-foreign-app"] fullscreen disable')
        def grown():
            value = (panel() or {}).get('draft_chars', 0)
            return value if value > before else None
        after = wait(grown, 'draft grew')
        check('chat draft grew by the full transcript', after - before >= args.min_chars, f'{before} -> {after} chars')
        time.sleep(6.5)
        capture('closed')
        check('app still running', app.poll() is None)

        # 6. Focused chat: the in-panel pill is the only live indicator.
        ipc('[app_id="jcode-desktop"] focus') if any(n.get('app_id') == 'jcode-desktop' for n in walk(ipc('-t', 'get_tree'))) \
            else ipc('[app_id="e2e-foreign-app"] kill')
        wait(lambda: not foreign_focused(), 'jcode focused')
        time.sleep(1)
        pills_before, done_before = os_pills(), app_log().count('global voice: inserted')
        start = len(events())
        key.stdin.write('press\n'); key.stdin.flush()
        wait(lambda: 'global voice: recording started' in events()[start:], 'focused recording', timeout=15)
        speech = subprocess.Popen(['paplay', f'--device={SINK}', str(args.speech)])
        time.sleep(1.5)
        capture('focused-listening')
        check('no OS pill while the chat is focused', os_pills() == pills_before)
        speech.wait(timeout=30)
        time.sleep(.6)
        key.stdin.write('release\n'); key.stdin.flush()
        wait(lambda: app_log().count('global voice: inserted') > done_before, 'focused transcript', timeout=40)
        check('focused hold still inserted a transcript', True)
        # The final outcome (Jev's decision) is always shown globally, even when
        # focused, so at most one OS pill appears and only after capture ended.
        check('focused hold shows at most the final decision pill', os_pills() - pills_before <= 1)
        evidence['result'] = 'passed'
        print('PASS: unfocused hold shows the OS pill, focused hold shows the chat pill while live, both insert transcripts')
    finally:
        (root / 'acceptance.json').write_text(json.dumps(evidence, indent=2, default=str) + '\n')
        if key and key.poll() is None:
            try:
                key.stdin.write('release\nquit\n'); key.stdin.flush()
            except BrokenPipeError:
                pass
        for proc in reversed(processes):
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
        for log in logs:
            log.close()
        for module in reversed(modules):
            subprocess.run(['pactl', 'unload-module', module], check=False)
        (root / 'jcode/config/jcode/nari.env').unlink(missing_ok=True)


if __name__ == '__main__':
    main()
