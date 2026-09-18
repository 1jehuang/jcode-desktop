#!/usr/bin/env python3
"""Capture the real CLI picker inside Desktop's real PTY on private Xvfb.

Example (run once per old/new CLI, each invocation captures light AND dark):
  python3 scripts/accept-cli-light-theme.py target/cli-old --binary /path/to/old/jcode
  python3 scripts/accept-cli-light-theme.py target/cli-new --binary /home/jeremy/jcode/target/selfdev/jcode

No build, shared daemon, credentials, model requests, or user-display input.
The CLI runs in a network/PID/filesystem bubblewrap namespace with only system
libraries, the chosen executable, and fresh fixture data. No session is opened.
Screenshots, OCR, logs, PTY receipts and reports persist even after failure.
This captures evidence, not a blanket WCAG assertion for antialiased pixels.
"""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import select
import shlex
import shutil
import subprocess
import sys
import time

from screenshot import isolated_env
from terminal_acceptance import Harness


def worker(root):
    proof = {'stdin_tty': os.isatty(0), 'stdout_tty': os.isatty(1),
             'tty': os.ttyname(0), 'pid': os.getpid()}
    temporary = root / 'pty-proof.tmp'
    temporary.write_text(json.dumps(proof))
    temporary.replace(root / 'pty-proof.json')
    if not proof['stdin_tty'] or not proof['stdout_tty']:
        raise RuntimeError('CLI must run in the actual embedded terminal PTY')
    command = json.loads((root / 'command.json').read_text())
    env = isolated_env(root)
    env.update({'TERM': os.environ.get('TERM', 'xterm-256color'),
                'COLORTERM': 'truecolor', 'JCODE_NO_TELEMETRY': '1', 'DO_NOT_TRACK': '1'})
    os.execvpe(command[0], command, env)


def metadata_contrast(path, box=(331, 314, 395, 328), label="metadata"):
    """Report a fixed metadata-label crop, not inferred ANSI/WCAG compliance.

    This 1440x1000 fixture places the third row's 'created:' label here. Use
    repeated near-neutral glyph pixels to avoid colored subpixel fringes.
    Antialiasing means these are observed raster contrast, not source colors.
    """
    from PIL import Image
    with Image.open(path) as image:
        crop = image.convert('RGB').crop(box)
        crop.resize((640, 140)).save(path.with_name(f'{path.stem}-{label}-crop.png'))
        pixels = getattr(crop, 'get_flattened_data', crop.getdata)
        colors = Counter(pixels())
    background = colors.most_common(1)[0][0]
    def luminance(rgb):
        values = [v / 255 for v in rgb]
        linear = [v / 12.92 if v <= .04045 else ((v + .055) / 1.055) ** 2.4
                  for v in values]
        return sum(v * weight for v, weight in zip(linear, (.2126, .7152, .0722)))
    def contrast(rgb):
        a, b = sorted((luminance(rgb), luminance(background)))
        return (b + .05) / (a + .05)
    candidates = [rgb for rgb, count in colors.items()
                  if count >= 2 and max(rgb) - min(rgb) <= 2 and rgb != background]
    if not candidates:
        raise AssertionError('No repeated metadata glyph pixels in expected crop')
    foreground = max(candidates, key=contrast)
    return {'crop': list(box), 'background_rgb': background,
            'strongest_repeated_neutral_glyph_rgb': foreground,
            'observed_raster_contrast': round(contrast(foreground), 3),
            'note': 'Antialiased raster sample, not source-color WCAG certification'}


def verify(h):
    h.wait(lambda: h.navigation().get('rows'), 'Desktop did not render', timeout=45)
    time.sleep(1)
    h.native('key', '--clearmodifiers', 'Escape')
    h.native('key', '--clearmodifiers', 'super+t')
    def terminal():
        return next((p for r in h.navigation().get('rows', []) for p in r['panels']
                     if p.get('session') == 'terminal' and p.get('focused')), None)
    h.wait(terminal, 'Terminal did not open')
    h.native('key', '--clearmodifiers', 'super+f')
    time.sleep(.5)
    command = 'python3 ' + shlex.quote(str(h.root / 'accept-cli-light-theme.py'))
    command += ' --worker ' + shlex.quote(str(h.root))
    h.native('type', '--clearmodifiers', '--delay', '1', '--', command)
    h.native('key', '--clearmodifiers', 'Return')
    h.wait(lambda: h.proof(), 'CLI worker did not reach PTY')
    time.sleep(4)  # Let Desktop's shortcut tooltip expire before evidence capture.
    # Poll actual screenshots: do not accept echoed command text as CLI proof.
    deadline = time.monotonic() + 30
    while True:
        path = h.capture('session-picker')
        text = h.text(path)
        if 'CONTRAST REVIEW' in text and 'SESSION' in text:
            break
        if time.monotonic() > deadline:
            raise AssertionError('Real session picker missing: ' + text)
    h.report['checks']['session_picker'] = {'screenshot': path.name, 'ocr': text}
    h.report['metadata_contrast'] = metadata_contrast(path)
    h.report['footer_contrast'] = metadata_contrast(path, (301, 944, 720, 959), 'footer')
    h.report['pty'] = h.proof()
    # Arrow navigation changes the preview without resuming or opening a session.
    h.native('key', '--clearmodifiers', 'Down', 'Down')
    time.sleep(.6)
    selected = h.capture('session-picker-selected')
    selected_text = h.text(selected)
    if 'CONTRAST REVIEW' not in selected_text:
        raise AssertionError('Selected preview did not render: ' + selected_text)
    h.report['checks']['selected'] = {'screenshot': selected.name, 'ocr': selected_text}
    h.report['selected_metadata_contrast'] = metadata_contrast(selected)
    h.native('key', '--clearmodifiers', 'Escape')


def capture(root, binary, desktop, theme, min_light_contrast=None):
    root.mkdir(parents=True, exist_ok=False)
    env = isolated_env(root)
    for directory in ('home', 'runtime', 'config', 'cache', 'data', 'logs', 'jcode'):
        (root / directory).mkdir(mode=0o700)
    drivers = sorted(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
    if not drivers:
        raise RuntimeError('Mesa lavapipe required')
    env.update({'VK_DRIVER_FILES': str(drivers[0]), 'SHELL': '/bin/bash',
                'JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT': 'empty',
                'JCODE_NO_TELEMETRY': '1', 'DO_NOT_TRACK': '1'})
    config = root / 'desktop.toml'
    config.write_text(f'[appearance]\ntheme = "{theme}"\nlayout_mode = "folder_tabs"\n')
    env['JCODE_DESKTOP_CONFIG'] = str(config)
    sessions = root / 'jcode/sessions'
    sessions.mkdir()
    for index, title in enumerate(('Contrast review', 'Terminal palette verification', 'Offline acceptance notes')):
        session_id = f'session_contrast_{index}'
        fixture = {'id': session_id, 'parent_id': None, 'title': title,
                   'created_at': '2026-09-17T20:00:00Z', 'updated_at': '2026-09-17T20:00:00Z',
                   'messages': [{'id': f'message_{index}', 'role': 'user',
                                 'content': [{'type': 'text', 'text': title}]}],
                   'working_dir': str(root), 'short_name': f'contrast{index}'}
        (sessions / f'{session_id}.json').write_text(json.dumps(fixture))
    command = ['bwrap', '--unshare-all', '--die-with-parent',
               '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin',
               '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib64', '/lib64',
               '--dev', '/dev', '--proc', '/proc', '--tmpfs', '/tmp',
               '--bind', str(root), str(root), '--ro-bind', str(binary), '/jcode-cli',
               '--chdir', str(root), '/jcode-cli', '--no-update', '--no-selfdev',
               '--socket', str(root / 'jcode/private.sock'), '--resume']
    (root / 'command.json').write_text(json.dumps(command))
    for name in ('accept-cli-light-theme.py', 'terminal_acceptance.py', 'screenshot.py'):
        shutil.copyfile(Path(__file__).with_name(name), root / name)
    wm_config = root / 'openbox.xml'
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                         '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                         '</application></applications></openbox_config>')
    h = Harness(root, env)
    with binary.open('rb') as executable:
        digest = hashlib.file_digest(executable, 'sha256').hexdigest()
    h.report.update({'cli_binary': str(binary), 'cli_sha256': digest, 'desktop_binary': str(desktop), 'theme': theme,
                     'isolation': 'fresh HOME/JCODE_HOME; bubblewrap network/PID/filesystem namespaces',
                     'command': command})
    processes = []
    try:
        with (root / 'xvfb.log').open('w') as xlog, (root / 'app.log').open('w') as log:
            read_fd, write_fd = os.pipe()
            try:
                xvfb = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-screen', '0',
                                         '1440x1000x24', '-nolisten', 'tcp'],
                                        pass_fds=(write_fd,), env=env, stdout=xlog, stderr=xlog)
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise TimeoutError('Private Xvfb failed to start')
                display = os.read(read_fd, 64).decode().strip()
                if not display.isdigit():
                    raise RuntimeError('Invalid private display')
                env['DISPLAY'] = ':' + display
            finally:
                os.close(read_fd)
                if write_fd is not None:
                    os.close(write_fd)
            wm = subprocess.Popen(['openbox', '--sm-disable', '--config-file', str(wm_config)],
                                  env=env, cwd=root, stdout=xlog, stderr=xlog)
            processes.append(wm)
            time.sleep(.5)
            h.app = subprocess.Popen([str(desktop)], env=env, cwd=root, stdout=log, stderr=log)
            processes.append(h.app)
            verify(h)
            if theme == 'neutral-light' and min_light_contrast is not None:
                h.report['min_light_contrast'] = min_light_contrast
                for sample in ('metadata_contrast', 'selected_metadata_contrast', 'footer_contrast'):
                    observed = h.report[sample]['observed_raster_contrast']
                    if observed < min_light_contrast:
                        raise AssertionError(f'{sample}: raster contrast {observed} < {min_light_contrast}')
        h.report['ok'] = True
    except Exception as error:
        h.report.update({'ok': False, 'error': str(error)})
        raise
    finally:
        (root / 'report.json').write_text(json.dumps(h.report, indent=2) + '\n')
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        print(f'CLI acceptance artifacts: {root}', flush=True)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == '--worker':
        worker(Path(sys.argv[2]))
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='new persistent artifact directory')
    parser.add_argument('--binary', type=Path, required=True, help='Jcode CLI executable under test')
    parser.add_argument('--desktop-binary', type=Path,
                        default=Path(__file__).resolve().parents[1] / 'target/debug/jcode-desktop')
    parser.add_argument('--theme', choices=('both', 'light', 'dark'), default='both')
    parser.add_argument('--min-light-contrast', type=float,
                        help='fail if measured light metadata/selected/footer raster contrast is below this ratio')
    args = parser.parse_args()
    if args.min_light_contrast is not None and not 1 <= args.min_light_contrast <= 21:
        parser.error('--min-light-contrast must be between 1 and 21')
    for command in ('Xvfb', 'openbox', 'xdotool', 'import', 'tesseract', 'python3', 'bwrap'):
        if not shutil.which(command):
            parser.error('missing prerequisite: ' + command)
    import PIL.Image  # noqa: F401
    binary = args.binary.resolve(strict=True)
    desktop = args.desktop_binary.resolve(strict=True)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    for name, theme in (('light', 'neutral-light'), ('dark', 'warm-neutral')):
        if args.theme in ('both', name):
            capture(root / name, binary, desktop, theme, args.min_light_contrast)


if __name__ == '__main__':
    main()
