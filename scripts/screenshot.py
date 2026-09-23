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
    parser.add_argument("--release-status", choices=("current", "newer", "checking", "error", "source"),
                        help="render an offline Desktop release status in the workspace top bar")
    parser.add_argument("--account-sign-in", action="store_true",
                        help="show optional first-launch account sign-in without network access")
    parser.add_argument("--account-sign-in-docked", action="store_true",
                        help="with --account-sign-in, show the email tab docked over the demo composer")
    parser.add_argument("--account-sign-in-interact", action="store_true",
                        help="verify account welcome, waiting, skip and Settings re-entry offline")
    parser.add_argument("--beta-notice", action="store_true",
                        help="capture the initial beta overlay instead of dismissing it")
    parser.add_argument("--beta-notice-interact", action="store_true",
                        help="verify startup beta overlay dismissal and native typing")
    parser.add_argument("--pinned-todo-interact", action="store_true",
                        help="verify pinned todo header styling survives expansion and collapse")
    parser.add_argument("--queue-interact", action="store_true",
                        help="verify Ctrl+Enter queues prompts on the private display")
    parser.add_argument("--pending-interact", action="store_true",
                        help="verify Enter, retry and local fallback for an offline pending startup")
    parser.add_argument("--cloud-startup", choices=("connecting", "failed"),
                        help="render managed Jcode Cloud unavailability without cloud operations (both modes currently unavailable)")
    parser.add_argument("--onboarding-interact", action="store_true",
                        help="exercise real production first-run Welcome, Skip, accounts, and cleanup on private Xvfb")
    parser.add_argument("--sounds-interact", action="store_true",
                        help="verify sound opt-in, preview, and saved mute on the private display")
    parser.add_argument("--rename-interact", action="store_true",
                        help="verify native rename button, F2, validation, dismissal, and preserved composer draft")
    parser.add_argument("--tab-actions-interact", action="store_true",
                        help="verify hover-only tab actions and shortcut tooltips")
    parser.add_argument("--worktrees", action="store_true", help="show isolated Git worktrees in the sidebar")
    parser.add_argument("--swarm", action="store_true", help="load nested swarm fixtures (children remain hidden in the sidebar)")
    parser.add_argument("--swarm-interact", action="store_true",
                        help="verify swarm showcase stays absent and lead selection remains stable (implies --swarm)")
    parser.add_argument("--notification", action="store_true", help="show the shortcut notification design fixture")
    parser.add_argument("--changelog", action="store_true", help="show the read-only Desktop changelog panel")
    parser.add_argument("--fresh-interact", action="store_true",
                        help="measure fresh composer pixels and verify native typing and submission")
    parser.add_argument("--html-interact", action="store_true",
                        help="exercise native input and controls on the HTML fixture")
    parser.add_argument("--image-pane-interact", action="store_true",
                        help="verify session image pane, inline toggle, lightbox and draft preservation")
    parser.add_argument("--image-interact", action="store_true",
                        help="click an image, verify enlargement, and dismiss by Escape and click")
    parser.add_argument("--image-cache-interact", action="store_true",
                        help="verify distinct pasted/transcript images and repeated stable image frames (GTK3 required)")
    parser.add_argument("--mermaid-interact", action="store_true",
                        help="click a Mermaid diagram, verify enlargement, and dismiss by Escape and Close")
    parser.add_argument("--responsive-interact", action="store_true",
                        help="verify compact navigation, sidebar drawer, panel focus and native resizing")
    parser.add_argument("--roller-interact", action="store_true",
                        help="verify native sidebar hover popout, scroll selection, dismissal, and preserved focus")
    parser.add_argument("--accounts-sidebar-interact", action="store_true",
                        help="verify connected, signed-out, unconfigured and expired sidebar accounts offline")
    parser.add_argument("--sidebar-interact", action="store_true",
                        help="verify hover-only close and native safe left-drag dismissal across workspaces")
    parser.add_argument("--history-interact", action="store_true",
                        help="verify native history clicks open and focus the intended composer")
    parser.add_argument("--fps-header-interact", action="store_true",
                        help="verify the integrated FPS header while navigating four native workspaces")
    parser.add_argument("--map-motion-interact", action="store_true",
                        help="verify native tab clicks travel continuously across the 2D map")
    parser.add_argument("--workspace-interact", action="store_true",
                        help="measure four workspace identities and verify numbered map navigation")
    parser.add_argument("--sidebar-workspaces-interact", action="store_true",
                        help="verify pinned workspace navigation beneath a long chat history")
    parser.add_argument("--close-interact", action="store_true",
                        help="hold Super+Q from the right edge of six panels and verify focus through retirement and reopen")
    parser.add_argument("--login-interact", action="store_true",
                        help="verify native account/provider clicks, masked clipboard paste, and draft restoration offline")
    parser.add_argument("--model-interact", action="store_true",
                        help="verify native model search, scrolling, dismissal, aliases, and selection with offline routes")
    parser.add_argument("--slash-interact", action="store_true",
                        help="verify visible slash menu selection and overflow scrolling on the private display")
    parser.add_argument("--default-directory-interact", action="store_true",
                        help="verify native default-directory selection, TOML persistence, validation, cancellation, and new drafts")
    parser.add_argument("--transcript", choices=("all", "empty", "background-tasks", "reasoning", "streaming", "orb-working", "orb-thinking", "orb-tools", "tool-streaming", "tool-icons", "prompts", "html", "image", "mermaid", "tokens", "diff", "diff-rich", "todos", "todos-completed", "gmail-draft", "publish", "orchestration"), default="all",
                        help="choose the isolated transcript fixture")
    parser.add_argument("--preview-state", choices=("empty", "streaming", "interrupted", "crashed", "voice-connecting", "voice-listening", "voice-routing", "voice-coding-agent", "voice-quick-action", "login-error", "model-access-error", "rate-limit", "disconnected", "login-dialog-error"),
                        help="render a named self-dev panel state using the real, offline UI")
    parser.add_argument("--preview-interact", action="store_true",
                        help="verify the self-dev control API and native recovery actions offline")
    parser.add_argument("--mermaid-source", type=Path,
                        help="custom Mermaid source file for the mermaid transcript fixture")
    parser.add_argument("--size", default="1440x1000")
    parser.add_argument("--scroll-up", type=int, default=0, metavar="STEPS",
                        help="scroll the transcript upward on the private display before capture")
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
        "graphite", "slate", "paper", "silver", "light-neutral", "chatgpt-light", "dark-neutral", "pure-black",
    ), help="render a built-in palette with isolated settings")
    parser.add_argument("--ai-font", help="assistant-only font family for the isolated fixture")
    args = parser.parse_args()
    if args.onboarding_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "onboarding_interact")
        if (others or args.preview_state or args.account_sign_in or args.beta_notice
                or args.panels != 1 or args.learn_stage is not None or args.focus_panel is not None
                or args.changelog or args.notification or args.swarm or args.worktrees
                or args.cloud_startup or args.release_status or args.mermaid_source
                or args.transcript != "all" or args.theme != "warm-neutral"
                or args.layout_mode != "folder_tabs" or args.ai_font or args.scroll_up):
            parser.error("onboarding-interact runs production first-run alone, without fixtures or other interactions")
        from onboarding_real_acceptance import run
        run(args.output, args.binary, args.no_build, args.size)
        return
    if args.accounts_sidebar_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "accounts_sidebar_interact")
        if (others or args.panels != 1 or args.size != "1440x1000"
                or args.layout_mode != "folder_tabs" or args.preview_state
                or args.account_sign_in or args.beta_notice):
            parser.error("accounts-sidebar-interact requires one panel, default size, folder tabs, and no other interactions or overlays")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("accounts-sidebar-interact requires xdotool and tesseract")
    if args.image_pane_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "image_pane_interact")
        if (others or args.panels != 1 or args.transcript != "image" or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
                or args.focus_panel is not None or args.learn_stage is not None
                or args.preview_state is not None or args.changelog or args.notification
                or args.account_sign_in or args.beta_notice):
            parser.error("image-pane-interact requires --transcript image, default size/theme/layout, one panel, and no other interactions or overlays")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("image-pane-interact requires xdotool and tesseract")
    if args.swarm_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "swarm_interact")
        if (others or args.panels != 1 or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
                or args.transcript != "all" or args.focus_panel is not None
                or args.learn_stage is not None or args.preview_state is not None
                or args.changelog or args.notification or args.worktrees
                or args.account_sign_in or args.beta_notice or args.ai_font):
            parser.error("swarm-interact requires the default single-panel size/theme/layout/transcript and no other interactions or overlays")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("swarm-interact requires xdotool and tesseract")
        args.swarm = True
    if args.cloud_startup and (args.transcript != "empty" or args.panels != 1
            or args.preview_state or args.changelog or args.account_sign_in
            or any(value for key, value in vars(args).items() if key.endswith("_interact"))):
        parser.error("cloud-startup requires --transcript empty, one panel, and no other interactive fixture")
    if args.beta_notice_interact and not shutil.which("tesseract"):
        parser.error("beta-notice-interact requires tesseract")
    if (args.beta_notice or args.beta_notice_interact) and any(
            value for key, value in vars(args).items()
            if key.endswith("_interact") and key != "beta_notice_interact"):
        parser.error("beta-notice modes cannot be combined with other interactions")
    if args.pending_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "pending_interact")
        if (others or args.transcript != "empty" or args.panels != 1
                or args.size != "1440x1000" or args.theme != "warm-neutral"
                or args.layout_mode != "folder_tabs" or args.focus_panel is not None
                or args.learn_stage is not None or args.preview_state is not None
                or args.changelog or args.notification or args.swarm or args.worktrees
                or args.account_sign_in):
            parser.error("pending-interact requires --transcript empty and the default single-panel fixture without overlays")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("pending-interact requires xdotool and tesseract")
    if args.queue_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "queue_interact")
        if others or args.transcript != "streaming" or args.panels != 1:
            parser.error("queue-interact requires --transcript streaming, one panel, and no other interactions")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("queue-interact requires xdotool and tesseract")
    if args.rename_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "rename_interact")
        if (others or args.panels != 1 or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
                or args.transcript != "empty" or args.focus_panel is not None
                or args.learn_stage is not None or args.preview_state is not None
                or args.changelog or args.notification or args.swarm):
            parser.error("rename-interact requires --transcript empty, one panel, default size/theme/layout, and no other interactions or overlays")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("rename-interact requires xdotool and tesseract")
    if args.tab_actions_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "tab_actions_interact")
        if (others or args.panels != 1 or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.layout_mode != "folder_tabs"
                or args.transcript != "empty" or args.focus_panel is not None
                or args.learn_stage is not None or args.preview_state is not None
                or args.changelog or args.notification or args.swarm):
            parser.error("tab-actions-interact requires --transcript empty and the default single-tab fixture")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("tab-actions-interact requires xdotool and tesseract")
    if args.account_sign_in or args.account_sign_in_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "account_sign_in_interact")
        if others or args.panels != 1 or args.learn_stage is not None or args.preview_state or args.beta_notice:
            parser.error("account-sign-in requires one panel and no other interaction or overlay modes")
        if args.account_sign_in_interact and (args.size != "1440x1000" or not shutil.which("tesseract")):
            parser.error("account-sign-in-interact requires the default size and tesseract")
    if args.slash_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "slash_interact")
        if (others or args.panels != 1 or args.size != "1440x1000"
                or args.theme != "warm-neutral" or args.transcript != "empty"
                or args.layout_mode != "folder_tabs" or args.focus_panel is not None
                or args.learn_stage is not None or getattr(args, "changelog", False) or args.notification or args.swarm):
            parser.error("slash-interact requires --transcript empty, one panel, default size/theme/layout, and no other interactions")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("slash-interact requires xdotool and tesseract")
    if args.sounds_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "sounds_interact")
        if others or args.panels != 1 or args.size != "1440x1000" or getattr(args, "changelog", False) or args.preview_state:
            parser.error("sounds-interact requires one panel, default size, and no other interactions")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("sounds-interact requires xdotool and tesseract")
    if args.preview_state is not None:
        if (args.panels != 1 or args.transcript != "all" or args.swarm
                or args.notification or args.changelog or args.learn_stage is not None
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
    if args.roller_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "roller_interact")
        if (others or args.size != "1440x1000" or args.layout_mode != "folder_tabs"
                or args.learn_stage is not None or args.focus_panel is not None
                or args.swarm or args.notification or args.transcript != "all"):
            parser.error("roller-interact requires default size/folder layout/transcript and no other interactions or overlays")
        if not shutil.which("xdotool") or not shutil.which("tesseract"):
            parser.error("roller-interact requires xdotool and tesseract")
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
    if args.pinned_todo_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "pinned_todo_interact")
        if (others or args.panels != 1 or args.size != "1440x1000"
                or args.transcript != "all" or args.theme != "warm-neutral"
                or args.layout_mode != "folder_tabs" or args.preview_state
                or args.learn_stage is not None or args.beta_notice):
            parser.error("pinned-todo-interact requires the default one-panel chat fixture")
    if args.fps_header_interact and (args.panels != 4 or args.layout_mode != "folder_tabs"):
        parser.error("fps-header-interact requires four panels and folder_tabs layout")
    if args.sidebar_workspaces_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "sidebar_workspaces_interact")
        if others or args.panels != 4 or args.size != "1440x1000" or args.layout_mode != "folder_tabs":
            parser.error("sidebar-workspaces-interact requires four panels, default size/folder layout, and no other interactions")
    if args.map_motion_interact:
        others = any(value for key, value in vars(args).items()
                     if key.endswith("_interact") and key != "map_motion_interact")
        if (others or args.panels != 4 or args.size != "1440x1000"
                or args.layout_mode != "folder_tabs" or args.learn_stage is not None
                or args.focus_panel is not None):
            parser.error("map-motion-interact requires four panels, default size/layout, and no other interactions")
        if not shutil.which("xdotool"):
            parser.error("map-motion-interact requires xdotool")
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
    for tool in ("Xvfb", "import", "openbox", "xdotool"):
        if not shutil.which(tool):
            parser.error(f"missing {tool}: install Xvfb, ImageMagick, Openbox, and xdotool")
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
        if args.release_status:
            env["JCODE_DESKTOP_SCREENSHOT_RELEASE_STATUS"] = args.release_status
        if args.worktrees:
            env["JCODE_DESKTOP_SCREENSHOT_WORKTREES"] = "1"
        if args.swarm:
            env["JCODE_DESKTOP_SCREENSHOT_SWARM"] = "1"
        if args.changelog:
            env["JCODE_DESKTOP_SCREENSHOT_CHANGELOG"] = "1"
        config = root / "desktop.toml"
        config.write_text(f'[appearance]\nlayout_mode = "{args.layout_mode}"\ntheme = "{args.theme}"\n'
                          + (f"ai_font = {json.dumps(args.ai_font)}\n" if args.ai_font else ""))
        if args.notification:
            env["JCODE_DESKTOP_SCREENSHOT_NOTIFICATION"] = "1"
        env["JCODE_DESKTOP_CONFIG"] = str(config)
        env["JCODE_DESKTOP_SCREENSHOT_TRANSCRIPT"] = args.transcript
        if args.pending_interact:
            env["JCODE_DESKTOP_SCREENSHOT_PENDING"] = "1"
        if args.cloud_startup:
            env["JCODE_DESKTOP_SCREENSHOT_CLOUD_STARTUP"] = args.cloud_startup
        if args.account_sign_in or args.account_sign_in_interact:
            env["JCODE_DESKTOP_SCREENSHOT_ACCOUNT_SIGN_IN"] = "1"
        if args.account_sign_in_docked:
            env["JCODE_DESKTOP_SCREENSHOT_ACCOUNT_DOCKED"] = "1"
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
        if args.history_interact or args.sidebar_workspaces_interact:
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
                if args.changelog:
                    expected_widths = "widths=" + ",".join(
                        ["0.50", "0.50"] + [f"{1 / args.panels:.2f}"] * (args.panels - 1)
                    )
                if args.transcript == "publish":
                    # The fixture adds the publish tracker beside the selfdev panel.
                    expected_widths = "widths="
                while not state.exists() or expected_widths not in state.read_text():
                    if app.poll() is not None or time.monotonic() > deadline:
                        app_log.seek(0)
                        diagnostics = root / "logs/jcode-desktop/jcode-desktop.log"
                        detail = diagnostics.read_text() if diagnostics.exists() else ""
                        raise RuntimeError("App failed to render:\n" + app_log.read() + detail)
                    time.sleep(0.1)
                # Capture the short-lived beta toast early enough to test typing
                # while it is still visible. Other fixtures can fully settle.
                time.sleep(0.8 if (args.beta_notice or args.beta_notice_interact) else 2)
                # Exercise the real launch overlay, then leave other fixtures unobscured.
                if not (args.beta_notice or args.beta_notice_interact or args.account_sign_in or args.account_sign_in_interact):
                    subprocess.run(["xdotool", "key", "--clearmodifiers", "Escape"],
                                   env=env, cwd=root, check=True, timeout=10)
                    time.sleep(0.3)
                if args.pinned_todo_interact:
                    from pinned_todo_acceptance import verify
                    verify(output, env)
                if args.queue_interact:
                    # The streaming fixture has an active turn but no network.
                    subprocess.run(["xdotool", "mousemove", str((canvas_left + width) // 2),
                                    str(height - 50), "click", "1"], env=env, check=True)
                    for prompt in ("Review the final changes", "Then run the tests"):
                        subprocess.run(["xdotool", "type", "--clearmodifiers", prompt], env=env, check=True)
                        subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+Return"], env=env, check=True)
                    time.sleep(0.5)
                    proof = root / "queued.png"
                    subprocess.run(["import", "-window", "root", str(proof)], env=env, check=True)
                    shutil.copyfile(proof, output)
                    text = subprocess.check_output(["tesseract", str(proof), "stdout"], env=env, stderr=subprocess.DEVNULL).decode()
                    for expected in ("sends after this response", "Review the final changes", "Then run the tests"):
                        if expected not in text:
                            raise RuntimeError("Ctrl+Enter did not paint the expected queue: " + text)
                    print("Native Ctrl+Enter queue verified on the private display")
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
                if args.map_motion_interact:
                    from map_motion_acceptance import verify
                    verify(output, env, root)
                if args.workspace_interact:
                    from workspace_identity_acceptance import verify
                    verify(output, env, root, theme=args.theme)
                if args.sidebar_workspaces_interact:
                    from sidebar_workspaces_acceptance import verify
                    verify(output, env, root)
                if app.poll() is not None:
                    raise RuntimeError("App exited before capture")
                if args.sounds_interact:
                    from sounds_acceptance import verify
                    verify(output, env, root)
                if args.account_sign_in_interact:
                    from account_sign_in_acceptance import verify
                    verify(output, env, root)
                if args.scroll_up > 0:
                    subprocess.run(["xdotool", "mousemove", str((canvas_left + width) // 2),
                                    str(height // 2), "click", "--repeat", str(args.scroll_up),
                                    "--delay", "40", "4"], env=env, cwd=root, check=True, timeout=30)
                    time.sleep(1)
                subprocess.run(["import", "-window", "root", "png:" + str(output)], env=env, cwd=root, check=True, timeout=15)
                print(f"Screenshot: {output}\nFixture state: {state.read_text().strip()}")
                if args.beta_notice_interact:
                    from beta_notice_acceptance import verify
                    verify(output, env, root)
                if args.pending_interact:
                    from pending_submission_acceptance import verify
                    verify(output, env, root)
                if args.swarm_interact:
                    from screenshot_swarm import verify
                    verify(output, env, root)
                if args.tab_actions_interact:
                    from tab_actions_acceptance import verify
                    verify(output, env, root)
                if args.rename_interact:
                    from rename_acceptance import verify
                    verify(output, env, root)
                if args.slash_interact:
                    from slash_menu_acceptance import verify
                    verify(output, env, root)
                if args.preview_interact:
                    from preview_acceptance import verify
                    verify(output, env, root)
                if args.responsive_interact:
                    from responsive_acceptance import verify
                    verify(output, env, root)
                if args.roller_interact:
                    from sidebar_roller_acceptance import verify
                    verify(output, env, root)
                if args.accounts_sidebar_interact:
                    from accounts_sidebar_acceptance import verify
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
                if args.image_pane_interact:
                    from image_pane_acceptance import verify
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
