#!/usr/bin/env python3
"""Render the real app on a private Xvfb display using offline fixture data."""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time


def isolated_env(root):
    """Allowlist, never inherit desktop sockets, credentials, or app settings."""
    return {
        "PATH": "/usr/bin:/bin",
        "HOME": str(root / "home"),
        "XDG_RUNTIME_DIR": str(root / "runtime"),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "data"),
        "XDG_STATE_HOME": str(root / "logs"),
        "JCODE_HOME": str(root / "jcode"),
        "JCODE_DESKTOP_SCREENSHOT": "1",
        "JCODE_DESKTOP_STATE": str(root / "state"),
        "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(root / "no-dbus"),
        "LIBGL_ALWAYS_SOFTWARE": "1",
        "LANG": "C.UTF-8",
    }


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/jcode-desktop")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--swarm", action="store_true", help="show nested swarm agents in the sidebar")
    parser.add_argument("--notification", action="store_true", help="show the shortcut notification design fixture")
    parser.add_argument("--fresh-interact", action="store_true",
                        help="measure fresh composer pixels and verify native typing and submission")
    parser.add_argument("--html-interact", action="store_true",
                        help="exercise native input and controls on the HTML fixture")
    parser.add_argument("--image-interact", action="store_true",
                        help="click an image, verify enlargement, and dismiss by Escape and click")
    parser.add_argument("--image-cache-interact", action="store_true",
                        help="verify distinct pasted/transcript images and repeated stable image frames (GTK3 required)")
    parser.add_argument("--mermaid-interact", action="store_true",
                        help="click a Mermaid diagram, verify enlargement, and dismiss by Escape and Close")
    parser.add_argument("--responsive-interact", action="store_true",
                        help="verify compact navigation, sidebar drawer, panel focus and native resizing")
    parser.add_argument("--sidebar-interact", action="store_true",
                        help="verify hover-only close and native safe left-drag dismissal across workspaces")
    parser.add_argument("--history-interact", action="store_true",
                        help="verify native history clicks open and focus the intended composer")
    parser.add_argument("--fps-header-interact", action="store_true",
                        help="verify the integrated FPS header while navigating four native workspaces")
    parser.add_argument("--workspace-interact", action="store_true",
                        help="measure four workspace identities and verify numbered map navigation")
    parser.add_argument("--close-interact", action="store_true",
                        help="hold Super+Q from the right edge of six panels and verify focus through retirement and reopen")
    parser.add_argument("--login-interact", action="store_true",
                        help="verify native account/provider clicks, masked clipboard paste, and draft restoration offline")
    parser.add_argument("--model-interact", action="store_true",
                        help="verify native model search, scrolling, dismissal, aliases, and selection with offline routes")
    parser.add_argument("--default-directory-interact", action="store_true",
                        help="verify native default-directory selection, TOML persistence, validation, cancellation, and new drafts")
    parser.add_argument("--transcript", choices=("all", "empty", "reasoning", "streaming", "html", "image", "mermaid", "tokens", "diff", "diff-rich"), default="all",
                        help="choose the isolated transcript fixture")
    parser.add_argument("--preview-state", choices=("empty", "streaming", "login-error", "model-access-error", "rate-limit", "disconnected", "login-dialog-error"),
                        help="render a named self-dev panel state using the real, offline UI")
    parser.add_argument("--preview-interact", action="store_true",
                        help="verify the self-dev control API and native recovery actions offline")
    parser.add_argument("--mermaid-source", type=Path,
                        help="custom Mermaid source file for the mermaid transcript fixture")
    parser.add_argument("--size", default="1440x1000")
    parser.add_argument("--learn-stage", type=int, choices=(1, 2, 3),
                        help="show the staged tutorial in the top-left Learn tab")
    parser.add_argument("--panels", type=int, choices=range(1, 7), default=1,
                        help="show a connected folder group with its middle panel focused")
    parser.add_argument("--focus-panel", type=int,
                        help="click this zero-based panel through X11 before capture")
    parser.add_argument("--layout-mode", choices=("normal", "folder_tabs"), default="folder_tabs")
    parser.add_argument("--theme", default="warm-neutral", choices=(
        "warm-neutral", "warm-studio", "neutral-dark", "neutral-light",
        "midnight", "ocean", "forest", "plum", "rose-dawn", "parchment",
    ), help="render a built-in palette with isolated settings")
    parser.add_argument("--ai-font", help="assistant-only font family for the isolated fixture")
    args = parser.parse_args()
    if args.preview_state is not None:
        if (args.panels != 1 or args.transcript != "all" or args.swarm
                or args.notification or args.learn_stage is not None
                or args.focus_panel is not None
                or any(value for key, value in vars(args).items()
                       if key.endswith("_interact") and key != "preview_interact")):
            parser.error("preview-state requires one panel and no transcript, notification, tutorial, swarm, or interaction options")
    if args.preview_interact and args.preview_state is None:
        parser.error("preview-interact requires --preview-state")
    if args.responsive_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "responsive_interact")
        if others or args.panels != 2 or args.size != "1440x1000" or args.layout_mode != "folder_tabs":
            parser.error("responsive-interact requires two panels, default size/layout, and no other interactions")
        if not shutil.which("xdotool"):
            parser.error("responsive-interact requires xdotool")
    if args.login_interact:
        other = any(value for key, value in vars(args).items()
                    if key.endswith("_interact") and key != "login_interact")
        if (other or args.panels != 1 or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
                or args.transcript != "empty" or args.learn_stage is not None
                or args.focus_panel is not None):
            parser.error("login-interact requires --transcript empty, one panel, default size/theme/layout, and no other interactions")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("login-interact requires xdotool and tesseract")
    if args.mermaid_source is not None and args.transcript != "mermaid":
        parser.error("mermaid-source requires --transcript mermaid")
    if args.sidebar_interact:
        if (args.panels != 2 or args.size != "1440x1000" or args.theme != "warm-neutral"
                or args.layout_mode != "folder_tabs" or args.learn_stage is not None
                or args.focus_panel is not None
                or any((args.fresh_interact, args.html_interact, args.image_interact,
                        args.image_cache_interact, args.mermaid_interact, args.history_interact,
                        args.workspace_interact, args.close_interact, args.model_interact,
                        args.default_directory_interact))):
            parser.error("sidebar-interact requires two panels, default size/theme/layout, and no other interaction mode")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("sidebar-interact requires xdotool and tesseract")
    if args.close_interact:
        if (args.panels != 6 or args.learn_stage is not None or args.focus_panel is not None
                or any((args.fresh_interact, args.html_interact, args.image_interact,
                        args.image_cache_interact, args.mermaid_interact, args.history_interact,
                        args.workspace_interact, args.model_interact, args.default_directory_interact))):
            parser.error("close-interact requires six panels and no other interaction mode")
        if not shutil.which("xdotool") or not shutil.which("xset"):
            parser.error("close-interact requires xdotool and xset")
    if args.default_directory_interact:
        if (args.panels != 1 or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
                or args.transcript not in ("all", "empty")
                or args.learn_stage is not None or args.focus_panel is not None
                or any((args.fresh_interact, args.html_interact, args.image_interact,
                        args.image_cache_interact, args.mermaid_interact,
                        args.history_interact, args.workspace_interact, args.model_interact))):
            parser.error("default-directory-interact requires one panel, default size/theme/layout, all or empty transcript, and no other interaction mode")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("default-directory-interact requires xdotool and tesseract")
    if args.model_interact:
        if (args.panels != 1 or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
                or args.transcript not in ("all", "empty")
                or args.learn_stage is not None or args.focus_panel is not None
                or any((args.fresh_interact, args.html_interact, args.image_interact,
                        args.image_cache_interact, args.mermaid_interact,
                        args.history_interact, args.workspace_interact))):
            parser.error("model-interact requires one panel, default size/theme/layout, all or empty transcript, and no other interaction mode")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("model-interact requires xdotool and tesseract")
    if args.fps_header_interact and (args.panels != 4 or args.layout_mode != "folder_tabs"):
        parser.error("fps-header-interact requires four panels and folder_tabs layout")
    if args.workspace_interact:
        if (args.panels != 4 or args.size != "1440x1000"
                or args.theme not in ("warm-neutral", "neutral-light")
                or args.layout_mode != "folder_tabs" or args.transcript != "all"
                or args.learn_stage is not None or args.focus_panel is not None
                or any((args.fresh_interact, args.html_interact, args.image_interact,
                        args.image_cache_interact, args.mermaid_interact, args.history_interact))):
            parser.error("workspace-interact requires four panels, default size/layout/transcript, warm-neutral or neutral-light, and no other interactions")
        if not shutil.which("xdotool"):
            parser.error("workspace-interact requires xdotool")
    if args.image_cache_interact:
        if (args.transcript != "image" or args.panels not in (1, 2) or args.size != "1440x1000"
                or args.layout_mode != "folder_tabs" or args.theme != "warm-neutral"
                or args.fresh_interact or args.html_interact or args.image_interact
                or args.mermaid_interact or args.history_interact
                or args.learn_stage is not None or args.focus_panel is not None):
            parser.error("image-cache-interact requires the image transcript, default size/theme/layout, one or two panels, and no other interaction mode")
        if not shutil.which("xdotool"):
            parser.error("image-cache-interact requires xdotool")
    if args.mermaid_interact:
        if (args.transcript != "mermaid" or args.panels != 1
                or args.fresh_interact or args.html_interact or args.image_interact
                or args.history_interact or args.learn_stage is not None
                or args.focus_panel is not None):
            parser.error("mermaid-interact requires the mermaid transcript, one panel, and no other interaction mode")
        if not shutil.which("xdotool"):
            parser.error("mermaid-interact requires xdotool")
    if args.fresh_interact:
        incompatible = (
            args.transcript != "empty" or args.panels != 1
            or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
            or args.learn_stage is not None or args.focus_panel is not None
            or args.html_interact or getattr(args, "image_interact", False)
            or args.history_interact
        )
        if incompatible:
            parser.error("fresh-interact requires an empty transcript, one panel, warm-neutral folder tabs, and no other interaction mode")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("fresh-interact requires xdotool and tesseract")
    if args.image_interact and (args.transcript != "image" or args.panels != 1 or args.html_interact or args.history_interact or args.learn_stage is not None or args.focus_panel is not None):
        parser.error("image-interact requires the image transcript, one panel, and no other interaction mode")
    if args.image_interact and not shutil.which("xdotool"):
        parser.error("image-interact requires xdotool")
    if args.history_interact and (args.panels != 1 or args.size != "1440x1000" or args.learn_stage is not None or args.focus_panel is not None or args.html_interact):
        parser.error("history-interact requires default size, one panel, and no other interaction mode")
    if args.history_interact and (not shutil.which("xdotool") or not shutil.which("tesseract")):
        parser.error("history-interact requires xdotool and tesseract")
    if args.html_interact and (args.transcript != "html" or args.size != "1440x1000" or args.theme != "warm-neutral" or args.panels != 1):
        parser.error("html-interact requires the html transcript, default size/theme, and one panel")
    if args.html_interact and not shutil.which("xdotool"):
        parser.error("html-interact requires xdotool")
    if args.focus_panel is not None and not 0 <= args.focus_panel < args.panels:
        parser.error("focus-panel must identify one of the displayed panels")
    if args.focus_panel is not None and not shutil.which("xdotool"):
        parser.error("native focus verification requires xdotool")
    width, height = (int(n) for n in args.size.split("x"))
    if not (640 <= width <= 7680 and 480 <= height <= 4320):
        parser.error("size must be between 640x480 and 7680x4320")
    canvas_left = 276 if args.layout_mode == "folder_tabs" else 264
    canvas_insets = 288 if args.layout_mode == "folder_tabs" else 264
    if args.focus_panel is not None and (width - canvas_insets) / args.panels < 320:
        parser.error("native focus verification needs at least 320px per panel beside the sidebar")
    for tool in ("Xvfb", "import", "openbox"):
        if not shutil.which(tool):
            parser.error(f"missing {tool}: install Xvfb, ImageMagick, and Openbox")
    drivers = sorted(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
    if not drivers:
        parser.error("missing Mesa lavapipe: install vulkan-swrast (Arch) or mesa-vulkan-drivers (Debian/Ubuntu)")
    if not args.no_build:
        subprocess.run(["cargo", "build", "-p", "jcode-desktop"], cwd=repo, check=True)
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        parser.error(f"refusing to overwrite {output}")
    scratch = Path(os.environ.get("JCODE_SCRATCH_DIR", repo / "target"))
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="screenshot-", dir=scratch) as temporary:
        root = Path(temporary)
        env = isolated_env(root)
        if args.swarm:
            env["JCODE_DESKTOP_SCREENSHOT_SWARM"] = "1"
        config = root / "desktop.toml"
        config.write_text(f'[appearance]\nlayout_mode = "{args.layout_mode}"\ntheme = "{args.theme}"\n'
                          + (f"ai_font = {json.dumps(args.ai_font)}\n" if args.ai_font else ""))
        if args.notification:
            env["JCODE_DESKTOP_SCREENSHOT_NOTIFICATION"] = "1"
        env["JCODE_DESKTOP_CONFIG"] = str(config)
        env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = args.transcript
        if args.preview_state is not None:
            env["JCODE_DESKTOP_SCREENSHOT_PREVIEW_STATE"] = args.preview_state
        if args.preview_interact:
            env["JCODE_DESKTOP_SELF_DEV"] = "1"
        if args.mermaid_source is not None:
            env["JCODE_DESKTOP_SCREENSHOT_MERMAID_SOURCE"] = args.mermaid_source.read_text()
        if args.learn_stage is not None:
            env["JCODE_DESKTOP_SCREENSHOT_LEARN_STAGE"] = str(args.learn_stage)
        env["JCODE_DESKTOP_SCREENSHOT_PANELS"] = str(args.panels)
        if args.model_interact:
            env["JCODE_DESKTOP_SCREENSHOT_MODELS"] = "1"
        if args.history_interact:
            env["JCODE_DESKTOP_SCREENSHOT_HISTORY"] = "1"
        env["VK_DRIVER_FILES"] = str(drivers[0])
        wm_config = root / "openbox.xml"
        wm_config.write_text('''<openbox_config xmlns="http://openbox.org/3.4/rc">
<applications><application class="*"><decor>no</decor>
<maximized>yes</maximized></application></applications></openbox_config>'''.replace(
            "<maximized>yes</maximized>",
            "<maximized>no</maximized>" if args.responsive_interact else "<maximized>yes</maximized>"))
        for name in ("home", "runtime", "config", "cache", "data", "jcode"):
            (root / name).mkdir(mode=0o700)
        if args.ai_font:
            fonts = Path.home() / ".local/share/fonts"
            if fonts.is_dir():
                shutil.copytree(fonts, root / "data/fonts", dirs_exist_ok=True)
        processes = []
        read_fd, write_fd = os.pipe()
        try:
            with (root / "xvfb.log").open("w+") as xvfb_log, (root / "app.log").open("w+") as app_log:
                xvfb = subprocess.Popen(
                    ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", f"{width}x{height}x24", "-nolisten", "tcp"],
                    pass_fds=(write_fd,), env=env, cwd=root, stdout=xvfb_log, stderr=xvfb_log,
                )
                processes.append(xvfb)
                os.close(write_fd)
                write_fd = None
                if not select.select([read_fd], [], [], 15)[0]:
                    raise RuntimeError("Xvfb did not become ready")
                display = os.read(read_fd, 64).decode().strip()
                if not display.isdigit():
                    raise RuntimeError("Xvfb failed to allocate a display")
                env["DISPLAY"] = ":" + display
                wm = subprocess.Popen(["openbox", "--sm-disable", "--config-file", str(wm_config)], env=env, cwd=root,
                                      stdout=xvfb_log, stderr=xvfb_log)
                processes.append(wm)
                time.sleep(0.5)
                if wm.poll() is not None:
                    raise RuntimeError("Private Openbox failed to start")
                app = subprocess.Popen([str(binary)], env=env, cwd=root, stdout=app_log, stderr=app_log)
                processes.append(app)
                deadline = time.monotonic() + 45
                state = root / "state"
                expected_widths = "widths=" + ",".join([f"{1 / args.panels:.2f}"] * args.panels)
                while not state.exists() or expected_widths not in state.read_text():
                    if app.poll() is not None or time.monotonic() > deadline:
                        app_log.seek(0)
                        diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
                        detail = diagnostics.read_text() if diagnostics.exists() else ""
                        raise RuntimeError("App failed to render:\n" + app_log.read() + detail)
                    time.sleep(0.1)
                # Allow opening animation and font rasterization to settle.
                time.sleep(2)
                if args.history_interact:
                    # Locate rendered titles so header rows (such as Default
                    # directory) can grow without silently clicking another session.
                    from default_directory_acceptance import click_sidebar_text
                    for step, (title, session_id) in enumerate([
                            ("History session 06", "screenshot-history-06"),
                            ("Review markdown", "screenshot-fixture"),
                            ("History session 06", "screenshot-history-06")]):
                        click_sidebar_text(output, env, root, title, f"history-click-{step}")
                        deadline = time.monotonic() + 10
                        while True:
                            text = state.read_text()
                            lines = [line for line in text.splitlines() if line.startswith("navigation=")]
                            navigation = json.loads(lines[0].split("=", 1)[1]) if lines else {}
                            focused = [panel for row in navigation.get("rows", []) for panel in row["panels"] if panel["focused"]]
                            if focused and focused[0]["session"] == session_id and navigation.get("keyboard_panel") == focused[0]["slot"]:
                                break
                            if app.poll() is not None or time.monotonic() > deadline:
                                raise RuntimeError("History click lost session or keyboard focus: " + text)
                            time.sleep(.05)
                    subprocess.run(["xdotool", "type", "history click typing works"],
                                   env=env, cwd=root, check=True, timeout=10)
                    time.sleep(.5)
                if args.focus_panel is not None:
                    # The fixture shows a 264px sidebar and 12px page connector.
                    # Native X11
                    # input crosses the same platform -> GPUI -> workspace path
                    # as a user click, on this private display only.
                    x = round(canvas_left + (width - canvas_insets) * (args.focus_panel + 0.5) / args.panels)
                    subprocess.run(["xdotool", "mousemove", str(x), str(height // 2), "click", "1"],
                                   env=env, cwd=root, check=True, timeout=10)
                    deadline = time.monotonic() + 10
                    while f"focus={args.focus_panel} " not in state.read_text():
                        if app.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("Native panel click did not update public focus state: " + state.read_text())
                        time.sleep(0.05)
                    time.sleep(0.5)
                if args.fps_header_interact:
                    from fps_header_acceptance import verify
                    verify(output, env, root)
                if args.workspace_interact:
                    from workspace_identity_acceptance import verify
                    verify(output, env, root, theme=args.theme)
                if app.poll() is not None:
                    raise RuntimeError("App exited before capture")
                subprocess.run(["import", "-window", "root", "png:" + str(output)], env=env, cwd=root, check=True, timeout=15)
                print(f"Screenshot: {output}\nFixture state: {state.read_text().strip()}")
                if args.preview_interact:
                    from preview_acceptance import verify
                    verify(output, env, root)
                if args.responsive_interact:
                    from responsive_acceptance import verify
                    verify(output, env, root)
                if args.sidebar_interact:
                    from sidebar_gesture_acceptance import verify
                    verify(output, env, root)
                if args.close_interact:
                    from close_panel_acceptance import verify
                    verify(output, env, root, layout_mode=args.layout_mode)
                if args.default_directory_interact:
                    from default_directory_acceptance import verify
                    verify(output, env, root)
                if args.login_interact:
                    from login_acceptance import verify
                    verify(output, env, root)
                if args.model_interact:
                    from model_picker_acceptance import verify
                    verify(output, env, root)
                if args.fresh_interact:
                    from fresh_session_acceptance import verify
                    verify(output, env, root)
                if args.image_interact:
                    from image_preview_acceptance import verify
                    verify(output, env, root)
                if args.image_cache_interact:
                    from image_cache_acceptance import verify
                    verify(output, env, root, panels=args.panels)
                if args.mermaid_interact:
                    from mermaid_preview_acceptance import verify
                    verify(output, env, root)
                if args.html_interact:
                    from html_preview_acceptance import verify
                    try:
                        verify(output, env, root)
                    except Exception:
                        diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
                        if diagnostics.exists():
                            print(diagnostics.read_text())
                        raise
        finally:
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


if __name__ == "__main__":
    main()
