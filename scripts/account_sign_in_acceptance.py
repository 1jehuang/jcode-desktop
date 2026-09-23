"""Native optional account onboarding on screenshot.py's private Xvfb display.

No browser is opened, no email is sent, and no credential is read or written.
The production screen and native controls run with a waiting-state fixture.
"""
import json
import tomllib

from default_directory_acceptance import NativeUI
from model_picker_acceptance import phrase_bounds


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    config = root / "desktop.toml"
    original = tomllib.loads(config.read_text())

    def words(label, phrase=None, sidebar=False):
        def check(image):
            current = ui.words(image, (0, 0, 264 if sidebar else image.width, image.height),
                               label, psm=11)
            # The UI font's lowercase l can be recognized as the two glyphs 1l.
            for word in current:
                if word["text"] == "1link":
                    word["text"] = "link"
            if phrase:
                phrase_bounds(current, phrase)
            return current
        return ui.wait_frame(label, check) if phrase else check(ui.capture(label))

    current = words("account-welcome", "Welcome to Jcode")
    phrase_bounds(current, "Continue")
    phrase_bounds(current, "magic link")
    phrase_bounds(current, "Subscribe")
    phrase_bounds(current, "Import less")
    phrase_bounds(current, "Telemetry")
    ui.click(phrase_bounds(current, "Sign in with email"))
    current = words("account-waiting", "Finish signing")
    phrase_bounds(current, "Waiting for approval")
    ui.click(phrase_bounds(current, "Open browser again"))
    current = words("account-reopen-browser", "Finish signing")
    ui.click(phrase_bounds(current, "Copy link"))
    current = words("account-copy-link", "Link copied")
    ui.click(phrase_bounds(current, "Start over"))
    current = words("account-back", "Welcome to Jcode")
    assert tomllib.loads(config.read_text()) == original
    # Keyboard-only: Shift+Tab wraps straight to Continue in the right half.
    ui.native("key", "--clearmodifiers", "Tab")
    ui.capture("account-keyboard-primary")
    ui.native("key", "--clearmodifiers", "shift+Tab")
    ui.capture("account-keyboard-continue")
    ui.native("key", "--clearmodifiers", "Return")
    # The beta notice is independently optional (for example after restoring a
    # snapshot). Prove Skip persisted and reached the workspace, not that a
    # second onboarding screen happened to appear.
    def skipped(image):
        current = ui.words(image, (0, 0, image.width, image.height), "account-skipped", psm=11)
        try:
            phrase_bounds(current, "Jcode Desktop is in beta testing")
            return True
        except AssertionError:
            phrase_bounds(current, "chat")
            return False
    beta_visible = ui.wait_frame("account-skipped", skipped)
    saved = tomllib.loads(config.read_text())
    assert saved["workspace"]["account_sign_in_handled"] is True
    saved["workspace"].pop("account_sign_in_handled")
    if not saved["workspace"] and "workspace" not in original:
        saved.pop("workspace")
    assert saved == original, (saved, original)
    if beta_visible:
        ui.native("key", "Escape")
    current = words("account-workspace", "chat", sidebar=True)
    chat = phrase_bounds(current, "chat")
    nav_y = round((chat[1] + chat[3]) / 2)
    # Folder-tab layout: hovering the section title opens the navigation menu.
    ui.native("mousemove", round((chat[0] + chat[2]) / 2), nav_y)
    for step in range(15):
        current = words(f"account-tabs-{step}", sidebar=True)
        try:
            ui.click(phrase_bounds(current, "settings"))
            break
        except AssertionError:
            ui.native("mousemove", round((chat[0] + chat[2]) / 2), nav_y, "click", "5")
    else:
        raise AssertionError("Settings tab not reachable")
    ui.native("mousemove", 130, 600)
    current = words("account-settings", "Jcode account", sidebar=True)
    ui.click(phrase_bounds(current, "Sign in with email"))
    current = words("account-reentry", "Welcome to Jcode")
    ui.click(phrase_bounds(current, "Sign in with email"))
    words("account-reentry-waiting", "Finish signing")
    ui.native("key", "Escape")
    words("account-escaped", "Jcode account", sidebar=True)
    assert tomllib.loads(config.read_text())["workspace"]["account_sign_in_handled"]
    assert not list((root / "jcode").glob("**/*credentials*")), "Offline sign-in wrote credentials"
    # Leave the welcome screen visible in the final artifact.
    current = words("account-settings-final", "Jcode account", sidebar=True)
    ui.click(phrase_bounds(current, "Sign in with email"))
    words("account-final", "Welcome to Jcode")
    ui.artifact("account-result.json").write_text(json.dumps({
        "welcome_and_magic_link_copy": True,
        "native_sign_in_waiting_reopen_and_cancel": True,
        "copy_link_feedback": True,
        "keyboard_continue_persisted": True,
        "other_configuration_unchanged": True,
        "settings_reentry_and_escape": True,
        "network_and_credential_writes": "disabled in offline fixture",
    }, indent=2) + "\n")
