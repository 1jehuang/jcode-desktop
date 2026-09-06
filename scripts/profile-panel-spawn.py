#!/usr/bin/env python3
"""Profile native new-panel input on private X11 with real isolated SDK sessions.

No credentials or model requests. Timings end at the app's rendered diagnostic
state (not physical input-to-photon). Keep before/after daemon binaries equal.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import statistics
import subprocess
import threading
import time
from screenshot import isolated_env


def navigation(path):
    try:
        return json.loads(next(line[11:] for line in path.read_text().splitlines()
                               if line.startswith('navigation=')))
    except (OSError, StopIteration, json.JSONDecodeError):
        return {}


def panels(state):
    return [panel for row in state.get('rows', []) for panel in row.get('panels', [])]


def spawn_timings(root):
    prefix = 'jcode desktop spawn: '
    path = root / 'logs/jcode-desktop/jcode-desktop.log'
    if not path.exists():
        path = root / 'desktop.log'
    return [json.loads(line.partition(prefix)[2]) for line in path.read_text().splitlines()
            if prefix in line]


class DelayedCreates:
    """Transparent private API proxy, delaying only CreateSession requests."""
    def __init__(self, path, upstream, delay):
        self.upstream, self.delay = upstream, delay
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(str(path))
        self.listener.listen()
        self.listener.settimeout(.1)
        self.closed = threading.Event()
        self.connections = []
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        while not self.closed.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            server = socket.socket(socket.AF_UNIX)
            server.connect(self.upstream)
            self.connections.extend([client, server])
            threading.Thread(target=self.pump, args=(client, server, True), daemon=True).start()
            threading.Thread(target=self.pump, args=(server, client, False), daemon=True).start()

    def pump(self, source, destination, requests):
        try:
            with source.makefile('rb') as stream:
                for line in stream:
                    if requests and json.loads(line).get('req') == 'create_session':
                        if self.closed.wait(self.delay):
                            return
                    destination.sendall(line)
        except (OSError, ValueError):
            pass
        finally:
            try:
                destination.shutdown(socket.SHUT_WR)
            except OSError:
                pass

    def close(self):
        self.closed.set()
        self.listener.close()
        for connection in self.connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/jcode-desktop'))
    parser.add_argument('--samples', type=int, default=8)
    parser.add_argument('--jcode', type=Path, help='runtime binary to measure')
    parser.add_argument('--create-delay', type=float, default=0,
                        help='delay each real API CreateSession through a private proxy')
    parser.add_argument('--verify-early-input', action='store_true',
                        help='require a focused draft before delayed attachment and verify native text survives')
    args = parser.parse_args()
    if args.samples < 1 or args.create_delay < 0:
        parser.error('samples must be positive and create-delay nonnegative')
    if args.verify_early_input and (args.create_delay < 1 or not (shutil.which('xclip') or shutil.which('tesseract'))):
        parser.error('early-input verification requires create-delay >= 1 and xclip or tesseract')
    binary = args.binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ('home', 'runtime', 'config', 'cache', 'data', 'jcode'):
        (root / name).mkdir(mode=0o700)
    env = isolated_env(root)
    env.pop('JCODE_DESKTOP_SCREENSHOT')
    env.update(JCODE_NO_TELEMETRY='1', JCODE_RUNTIME_DIR=str(root / 'runtime'),
               JCODE_API_SOCKET=str(root / 'runtime/api.sock'),
               JCODE_SOCKET=str(root / 'runtime/daemon.sock'),
               JCODE_DESKTOP_CONFIG=str(root / 'desktop.toml'))
    jcode = str((args.jcode or Path(shutil.which('jcode'))).resolve(strict=True))
    env['PATH'] = str(Path(jcode).parent) + ':/usr/bin:/bin'
    env['VK_DRIVER_FILES'] = str(next(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json')))
    (root / 'desktop.toml').write_text('[workspace]\ncoaching_hints = false\n')
    processes, logs = [], []

    def launch(name, command, **kwargs):
        log = (root / (name + '.log')).open('w')
        logs.append(log)
        process = subprocess.Popen(command, cwd=root, env=env, stdout=log,
                                   stderr=log, start_new_session=True, **kwargs)
        processes.append(process)
        return process

    def wait(predicate, timeout=60):
        deadline = time.monotonic() + timeout
        while True:
            result = predicate()
            if result:
                return result
            assert all(p.poll() is None for p in processes), 'Child exited'
            if time.monotonic() > deadline:
                raise TimeoutError('See logs in ' + str(root))
            time.sleep(.002)

    def ready(path):
        try:
            with socket.socket(socket.AF_UNIX) as client:
                client.connect(path)
            return True
        except OSError:
            return False

    def key(value):
        subprocess.run(['xdotool', 'key', '--clearmodifiers', '--delay', '0', value],
                       env=env, check=True, timeout=10)

    read_fd, write_fd = os.pipe()
    proxy = None
    try:
        launch('daemon', [jcode, '--no-update', '--no-selfdev', '--provider', 'jcode', 'serve'])
        wait(lambda: ready(env['JCODE_SOCKET']))
        launch('bridge', [jcode, '--no-update', '--no-selfdev', 'api-bridge'])
        wait(lambda: ready(env['JCODE_API_SOCKET']))
        if args.create_delay:
            proxy = DelayedCreates(root / 'runtime/delayed.sock', env['JCODE_API_SOCKET'], args.create_delay)
            env['JCODE_API_SOCKET'] = str(root / 'runtime/delayed.sock')
        launch('xvfb', ['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                        '1440x1000x24', '-nolisten', 'tcp'], pass_fds=(write_fd,))
        os.close(write_fd)
        write_fd = None
        assert select.select([read_fd], [], [], 15)[0]
        env['DISPLAY'] = ':' + os.read(read_fd, 64).decode().strip()
        wm = root / 'openbox.xml'
        wm.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications><application class="*"><decor>no</decor><maximized>yes</maximized></application></applications></openbox_config>')
        launch('wm', ['openbox', '--sm-disable', '--config-file', str(wm)])
        time.sleep(.5)
        launch('desktop', [str(binary), '--no-hot-reload'])
        state_path = root / 'state'
        wait(lambda: any(p.get('session', '').startswith('session_') for p in panels(navigation(state_path))))
        time.sleep(1)
        samples = []
        for index in range(args.samples):
            before = panels(navigation(state_path))
            started = time.perf_counter()
            key('super+n')
            shown = wait(lambda: (p if len(p := panels(navigation(state_path))) > len(before) else None))
            visible_ms = (time.perf_counter() - started) * 1000
            focused = next(p for p in shown if p.get('focused'))
            text = 'Typing before connection survives'
            if args.verify_early_input:
                assert focused['session'].startswith('startup://draft/'), focused
                assert visible_ms < args.create_delay * 500, 'Panel still waits for runtime'
                assert navigation(state_path)['keyboard_panel'] == focused['slot']
                subprocess.run(['xdotool', 'type', '--clearmodifiers', '--delay', '0', text],
                               env=env, check=True, timeout=10)
                if index == 0:
                    # The timing above ends at render-state construction. Give
                    # the normal 150ms layout animation time to present before
                    # taking independent pixel evidence, still before attach.
                    time.sleep(.25)
                    assert any(p.get('focused') and p['session'] == focused['session']
                               for p in panels(navigation(state_path))), 'Attached before pending capture'
                    subprocess.run(['import', '-window', 'root', str(root / 'pending-input.png')],
                                   env=env, check=True, timeout=10)
            attached = wait(lambda: next((p for p in panels(navigation(state_path))
                                          if p.get('focused') and p.get('session', '').startswith('session_')), None))
            attached_ms = (time.perf_counter() - started) * 1000
            if args.verify_early_input:
                assert attached['id'] == focused['id'], 'Attachment replaced the panel entity'
                if index == 0 and shutil.which('tesseract'):
                    pending_text = subprocess.check_output(
                        ['tesseract', str(root / 'pending-input.png'), 'stdout', '--psm', '11'],
                        env=env, timeout=15, stderr=subprocess.DEVNULL).decode()
                    assert text.lower() in ' '.join(pending_text.lower().split()), pending_text
                if shutil.which('xclip'):
                    key('ctrl+a')
                    key('ctrl+c')
                    copied = subprocess.check_output(['xclip', '-selection', 'clipboard', '-o'],
                                                     env=env, timeout=5).decode()
                    assert copied == text, (copied, text)
                else:
                    image = root / f'attached-input-{index}.png'
                    time.sleep(.2)  # Let the attachment frame reach X11 too.
                    subprocess.run(['import', '-window', 'root', str(image)], env=env, check=True, timeout=10)
                    rendered = subprocess.check_output(['tesseract', str(image), 'stdout', '--psm', '11'],
                                                       env=env, timeout=15, stderr=subprocess.DEVNULL).decode()
                    assert text.lower() in ' '.join(rendered.lower().split()), rendered
            sample = dict(sample=index, panel_render_state_ms=visible_ms, attached_ms=attached_ms,
                          first_session_id=focused.get('session'), session_id=attached['session'],
                          early_input_verified=args.verify_early_input)
            samples.append(sample)
            print(json.dumps(sample), flush=True)
            time.sleep(.4)
            key('ctrl+shift+w')
            wait(lambda: len(panels(navigation(state_path))) == len(before))
            time.sleep(.2)
        subprocess.run(['import', '-window', 'root', str(root / 'final.png')], env=env, check=True)
        result = dict(binary=str(binary), daemon=jcode, samples=samples,
                      create_delay_ms=args.create_delay * 1000,
                      panel_render_state_median_ms=statistics.median(s['panel_render_state_ms'] for s in samples),
                      attached_median_ms=statistics.median(s['attached_ms'] for s in samples))
        result['sdk_timings'] = spawn_timings(root)
        (root / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result, indent=2))
    finally:
        if proxy:
            proxy.close()
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
        os.close(read_fd)
        if write_fd is not None:
            os.close(write_fd)


if __name__ == '__main__':
    main()
