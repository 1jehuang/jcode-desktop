#!/usr/bin/env python3
"""Accept immediate startup input while runtime and UI rebuild are blocked.

Runs the existing desktop host on a private Xvfb display. PATH shims hold both
Jcode companion startup and the default development Cargo rebuild until the
script explicitly releases them. The run saves screenshots, logs, and a JSONL
timing/state trace in OUTPUT. It never builds before launching or touches the
user's display, home, sockets, credentials, or desktop settings.
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


STARTUP_SESSION = 'startup://draft'
DRAFT = 'startup draft survives attachment and reload'


def navigation_state(path):
    try:
        line = next(line for line in path.read_text().splitlines()
                    if line.startswith('navigation='))
        return json.loads(line.partition('=')[2])
    except (FileNotFoundError, StopIteration, json.JSONDecodeError):
        return None


def focused_panel(state):
    panels = [panel for row in state.get('rows', []) for panel in row.get('panels', [])
              if panel.get('focused')]
    assert len(panels) == 1, state
    panel = panels[0]
    assert state.get('keyboard_panel') == panel.get('slot'), state
    return panel


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='new directory for evidence')
    parser.add_argument('--binary', type=Path,
                        help='existing desktop host (default: target/debug/jcode-desktop)')
    parser.add_argument('--timeout', type=int, default=600,
                        help='seconds allowed after releasing each blocked process')
    parser.add_argument('--runtime-delay', type=float, default=2,
                        help='extra delay after releasing the isolated Jcode companion')
    parser.add_argument('--startup-only', action='store_true',
                        help='stop after proving input works with runtime and Cargo blocked')
    parser.add_argument('--trace-startup', action='store_true',
                        help='record strace syscall timings for startup attribution')
    args = parser.parse_args()
    if args.timeout <= 0 or args.runtime_delay < 0:
        parser.error('timeout must be positive and runtime-delay nonnegative')

    repo = Path(__file__).resolve().parents[1]
    root = args.output.resolve()
    if len(str(root / 'runtime/daemon.sock').encode()) >= 104:
        parser.error('output path is too long for a private Unix socket')
    required = ('jcode', 'cargo', 'Xvfb', 'openbox', 'xdotool', 'import')
    for name in required:
        if not shutil.which(name):
            parser.error('missing executable: ' + name)
    if args.trace_startup and not shutil.which('strace'):
        parser.error('--trace-startup requires strace')
    xclip = shutil.which('xclip')
    if not xclip and not (shutil.which('tesseract') and shutil.which('magick')):
        parser.error('native text verification requires xclip or tesseract plus ImageMagick')
    drivers = sorted(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if not drivers:
        parser.error('missing Mesa lavapipe Vulkan driver')
    binary = (args.binary or repo / 'target/debug/jcode-desktop').resolve(strict=True)
    real_jcode = Path(shutil.which('jcode')).resolve()
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    real_cargo = cargo_home / 'bin/cargo'
    if not real_cargo.is_file():
        real_cargo = Path(shutil.which('cargo')).resolve()

    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode', 'tmp', 'shims'):
        (root / name).mkdir(mode=0o700)
    (root / 'desktop.toml').write_text('[workspace]\ncoaching_hints = false\n')
    wm_config = root / 'openbox.xml'
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')

    # Each marker proves the host attempted the operation while its release file
    # was absent. The shim then becomes transparent and execs the real program.
    shim = '''#!/bin/sh
set -eu
name=$(basename "$0")
printf '%s\\n' "$(date +%s.%N) $*" >> "{root}/$name-invocations.log"
touch "{root}/$name-blocked"
while [ ! -e "{root}/release-$name" ]; do sleep .05; done
if [ "$name" = jcode ]; then
  sleep "{delay}"
  # A private home intentionally has no credentials. The desktop's companion
  # `serve` still needs a provider selection, but acceptance performs no inference.
  if [ "${{1:-}}" = serve ]; then
    shift
    exec "{jcode}" --no-update --no-selfdev --provider jcode serve "$@"
  fi
  exec "{jcode}" "$@"
fi
exec "{cargo}" "$@"
'''.format(root=root, delay=args.runtime_delay, jcode=real_jcode, cargo=real_cargo)
    for name in ('jcode', 'cargo'):
        path = root / 'shims' / name
        path.write_text(shim)
        path.chmod(0o755)

    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    env.update({
        # Keep the shim first, then preserve Cargo's toolchain-proxy directory
        # so compiler identity matches the normal development launch.
        'PATH': ':'.join((str(root / 'shims'), str(real_cargo.parent),
                         '/usr/bin', '/bin')),
        'JCODE_NO_TELEMETRY': '1',
        'JCODE_RUNTIME_DIR': str(root / 'runtime'),
        'JCODE_API_SOCKET': str(root / 'runtime/api.sock'),
        'JCODE_SOCKET': str(root / 'runtime/daemon.sock'),
        'JCODE_TEMP_SERVER': '1',
        'JCODE_SERVER_OWNER_PID': str(os.getpid()),
        'JCODE_TEMP_SERVER_IDLE_SECS': str(args.timeout + 60),
        'JCODE_DESKTOP_CONFIG': str(root / 'desktop.toml'),
        'VK_DRIVER_FILES': str(drivers[0]),
        'CARGO': str(root / 'shims/cargo'),
        'CARGO_HOME': str(cargo_home),
        'RUSTUP_HOME': os.environ.get('RUSTUP_HOME', str(Path.home() / '.rustup')),
        'CARGO_NET_OFFLINE': 'true',
        'TMPDIR': str(root / 'tmp'),
    })
    state_path = root / 'state'
    diagnostics = root / 'logs/jcode-desktop/jcode-desktop.log'
    started = time.monotonic()
    processes, logs = [], []

    def record(checkpoint, state=None, **extra):
        item = {'checkpoint': checkpoint, 'elapsed_seconds': time.monotonic() - started,
                **extra}
        if state is not None:
            item['navigation'] = state
        with (root / 'timing.jsonl').open('a') as output:
            output.write(json.dumps(item) + '\n')
        print(checkpoint + f": {item['elapsed_seconds']:.3f}s", flush=True)

    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(command, cwd=root, env=env, stdout=log,
                                   stderr=log, start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait_until(predicate, label, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert all(process.poll() is None for process in processes), 'child exited while waiting for ' + label
            if predicate():
                return
            time.sleep(.05)
        raise AssertionError('Timed out: ' + label + '\n' +
                             (state_path.read_text() if state_path.exists() else 'No state'))

    def capture(name):
        subprocess.run(['import', '-window', 'root', str(root / name)], env=env,
                       check=True, timeout=15)

    def text_verified(stage):
        if xclip:
            subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+a'], env=env, check=True)
            subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+c'], env=env, check=True)
            time.sleep(.15)
            copied = subprocess.check_output([xclip, '-selection', 'clipboard', '-o'],
                                             env=env, text=True, timeout=10)
            assert copied == DRAFT, stage + ': native clipboard text differs'
            return 'clipboard'
        image = root / (stage + '-ocr.png')
        capture(image.name)
        # This runner fixes the window at 1440x1000. Crop to the composer so
        # an echoed message in the transcript cannot satisfy this check.
        crop = root / (stage + '-composer.png')
        subprocess.run(['magick', str(image), '-crop', '1440x64+0+930',
                        '+repage', str(crop)], env=env, check=True, timeout=15)
        text = subprocess.check_output(['tesseract', str(crop), 'stdout', '--psm', '7'],
                                       env=env, text=True, stderr=subprocess.DEVNULL, timeout=30)
        assert DRAFT.lower() in ' '.join(text.lower().split()), \
            stage + ': screenshot OCR did not find the native typed draft: ' + text
        return 'screenshot-ocr'

    read_fd, write_fd = os.pipe()
    try:
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                        '1440x1000x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0], 'Xvfb startup timeout'
        display = os.read(read_fd, 64).decode().strip()
        assert display.isdigit(), display
        env['DISPLAY'] = ':' + display
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm_config)])
        record('app-launch')
        command = [str(binary), '--hot-reload']
        if args.trace_startup:
            command = ['strace', '-f', '-ttt', '-T', '-o', str(root / 'startup.strace'), *command]
        launch('app', command)

        wait_until(lambda: (s := navigation_state(state_path)) is not None and
                   focused_panel(s).get('session') == STARTUP_SESSION,
                   'immediate startup panel', 30)
        record('first-panel-ready', navigation_state(state_path))
        wait_until(lambda: (root / 'cargo-blocked').exists() and
                   (root / 'jcode-blocked').exists(), 'both startup gates', 30)
        subprocess.run(['xdotool', 'type', '--delay', '15', DRAFT], env=env,
                       check=True, timeout=15)
        proof = text_verified('pending-typed')
        subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Return'], env=env,
                       check=True, timeout=10)
        time.sleep(.25)
        pending = navigation_state(state_path)
        assert focused_panel(pending).get('session') == STARTUP_SESSION, \
            'Enter must not submit while startup is pending'
        assert text_verified('pending-enter') == proof, 'text proof method changed'
        capture('01-blocked-typed.png')
        record('blocked-typed-enter-disabled', pending, text_verified=proof)
        if args.startup_only:
            print('PASS: startup panel is editable while runtime and Cargo remain blocked.', flush=True)
            return

        (root / 'release-jcode').touch()
        wait_until(lambda: (s := navigation_state(state_path)) is not None and
                   focused_panel(s).get('session', '').startswith('session_'),
                   'connected session attachment', args.timeout)
        attached = navigation_state(state_path)
        assert text_verified('attached') == proof, 'text proof method changed'
        assert len(attached['rows'][attached['active_row']]['panels']) == 1, attached
        capture('02-attached.png')
        record('attached', attached, text_verified=proof)

        before = diagnostics.read_text().count('activated UI generation') if diagnostics.exists() else 0
        (root / 'release-cargo').touch()
        wait_until(lambda: diagnostics.exists() and
                   diagnostics.read_text().count('activated UI generation') > before,
                   'automatic startup hot reload', args.timeout)
        wait_until(lambda: (s := navigation_state(state_path)) is not None and
                   focused_panel(s).get('session') == focused_panel(attached).get('session'),
                   'focus after hot reload', 15)
        reloaded = navigation_state(state_path)
        assert text_verified('reloaded') == proof, 'text proof method changed'
        capture('03-reloaded.png')
        record('reloaded', reloaded, text_verified=proof,
               activated_generations=diagnostics.read_text().count('activated UI generation'))
        print('PASS: startup panel accepted native typing while runtime and Cargo were blocked; '
              f'attachment and automatic hot reload retained text/focus. Evidence: {root}', flush=True)
    finally:
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)
        # Release gates so shell children can terminate promptly.
        (root / 'release-jcode').touch(exist_ok=True)
        (root / 'release-cargo').touch(exist_ok=True)
        for process in reversed(processes):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        for log in logs:
            log.close()


if __name__ == '__main__':
    main()
