#!/usr/bin/env python3
"""Exercise release highlights and dense history on a private offline X11 display.

Run after cargo build -p jcode-desktop:
    python3 scripts/changelog_acceptance.py target/changelog-review
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time

from default_directory_acceptance import NativeUI
from model_picker_acceptance import normalized, phrase_bounds
from screenshot import isolated_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--theme", default="graphite")
    parser.add_argument("--width", type=int, default=1440)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"checks": {}, "theme": args.theme, "width": args.width}
    with tempfile.TemporaryDirectory(prefix="changelog-", dir=repo / "target") as temporary:
        root = Path(temporary)
        env = isolated_env(root)
        for name in ("home", "runtime", "config", "cache", "data", "jcode"):
            (root / name).mkdir(mode=0o700)
        config = root / "desktop.toml"
        config.write_text(f'[appearance]\nlayout_mode = "folder_tabs"\ntheme = "{args.theme}"\n')
        env.update(JCODE_DESKTOP_CONFIG=str(config), JCODE_DESKTOP_SCREENSHOT_CHANGELOG="1")
        drivers = sorted(Path('/usr/share/vulkan/icd.d').glob('lvp_icd*.json'))
        assert drivers, "Mesa lavapipe required"
        env['VK_ICD_FILENAMES'] = str(drivers[0])
        wm_config = root / "openbox.xml"
        wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><applications>'
                             '<application class="*"><decor>no</decor><maximized>yes</maximized>'
                             '</application></applications></openbox_config>')
        processes = []
        read_fd, write_fd = os.pipe()
        try:
            with (output / 'app.log').open('w') as log:
                processes.append(subprocess.Popen(
                    ['Xvfb', '-displayfd', str(write_fd), '-screen', '0', f'{args.width}x1000x24', '-nolisten', 'tcp'],
                    pass_fds=(write_fd,), env=env, stdout=log, stderr=log))
                os.close(write_fd)
                write_fd = None
                assert select.select([read_fd], [], [], 15)[0], 'Xvfb startup timeout'
                display = os.read(read_fd, 64).decode().strip()
                assert display.isdigit(), 'Invalid private display'
                env['DISPLAY'] = ':' + display
                processes.append(subprocess.Popen(['openbox', '--sm-disable', '--config-file', str(wm_config)],
                                                   env=env, stdout=log, stderr=log))
                time.sleep(.5)
                processes.append(subprocess.Popen([str(repo / 'target/debug/jcode-desktop')],
                                                   env=env, cwd=root, stdout=log, stderr=log))
                deadline = time.monotonic() + 45
                while not (root / 'state').exists():
                    assert processes[-1].poll() is None and time.monotonic() < deadline, 'App startup failed'
                    time.sleep(.1)
                # Let the real beta countdown dismiss itself, without Escape closing the notes.
                time.sleep(4)
                ui = NativeUI(output / 'notes.png', env, root)
                # At narrow widths the active folder tab fills the canvas.
                # Locate text across it instead of assuming a half-width pane.
                crop = (240, 40, args.width - 15, 980)

                def locate(label, phrase):
                    return ui.wait_frame(label, lambda image: phrase_bounds(ui.words(image, crop, label), phrase))

                locate('highlights', 'Highlights')
                locate('highlights', 'Improvements')
                report['checks']['editorial-sections-visible'] = True
                ui.click(locate('history-control', 'Full changelog'))
                locate('history', 'Newest first')
                report['checks']['history-click'] = True
                before = ui.capture('history-top')
                ui.native('mousemove', crop[0] + 200, 600, 'click', '--repeat', '8', '--delay', '70', '5')
                after = ui.capture('history-scrolled')
                assert before.crop((crop[0], 170, crop[2], 900)).tobytes() != after.crop((crop[0], 170, crop[2], 900)).tobytes(), 'History did not scroll'
                report['checks']['history-scroll'] = True
                ui.native('key', '--clearmodifiers', 'Left')
                locate('highlights-return', 'Highlights')
                locate('highlights-return', 'Improvements')
                report['checks']['keyboard-return-and-independent-scroll'] = True
                ui.native('key', '--clearmodifiers', 'Escape')

                def closed(image):
                    words = ui.words(image, (240, 40, args.width - 15, 180), 'closed')
                    text = normalized(' '.join(word['text'] for word in words))
                    assert 'releasenotesupdates' not in text, 'Changelog did not close'

                ui.wait_frame('closed', closed)
                report['checks']['escape-closes'] = True
                report['passed'] = True
        except Exception as error:
            report.update(passed=False, error=repr(error))
            raise
        finally:
            os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
            for process in reversed(processes):
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            (output / 'acceptance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
