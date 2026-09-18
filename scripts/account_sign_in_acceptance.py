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
            if phrase:
                phrase_bounds(current, phrase)
            return current
        return ui.wait_frame(label, check) if phrase else check(ui.capture(label))

    current = words("account-welcome", "Welcome to Jcode Desktop")
    phrase_bounds(current, "Skip for now")
    phrase_bounds(current, "magic link")
    ui.click(phrase_bounds(current, "Sign in with email"))
    current = words("account-waiting", "Finish signing")
    phrase_bounds(current, "Waiting for approval")
    ui.click(phrase_bounds(current, "Open browser again"))
    current = words("account-reopen-browser", "Finish signing")
    ui.click(phrase_bounds(current, "Cancel and go back"))
    current = words("account-back", "Welcome to Jcode Desktop")
    assert tomllib.loads(config.read_text()) == original
    # Keyboard-only choice is as accessible as the visible Skip button.
    ui.native("key", "--clearmodifiers", "Tab")
    ui.capture("account-keyboard-primary")
    ui.native("key", "--clearmodifiers", "Tab")
    ui.capture("account-keyboard-skip")
    ui.native("key", "--clearmodifiers", "Return")
    words("account-skipped", "Jcode Desktop is in beta testing")
    saved = tomllib.loads(config.read_text())
    assert saved["workspace"]["account_sign_in_handled"] is True
    saved["workspace"].pop("account_sign_in_handled")
    if not saved["workspace"] and "workspace" not in original:
        saved.pop("workspace")
    assert saved == original, (saved, original)
    ui.native("key", "Escape")
    current = words("account-workspace", "chat", sidebar=True)
    chat = phrase_bounds(current, "chat")
    nav_y = round((chat[1] + chat[3]) / 2)
    for step in range(15):
        current = words(f"account-tabs-{step}", sidebar=True)
        try:
            ui.click(phrase_bounds(current, "settings"))
            break
        except AssertionError:
            ui.native("mousemove", 180, nav_y, "click", "5")
    else:
        raise AssertionError("Settings tab not reachable")
    current = words("account-settings", "Jcode account", sidebar=True)
    ui.click(phrase_bounds(current, "Sign in with email"))
    current = words("account-reentry", "Welcome to Jcode Desktop")
    ui.click(phrase_bounds(current, "Sign in with email"))
    words("account-reentry-waiting", "Finish signing")
    ui.native("key", "Escape")
    words("account-escaped", "Jcode account", sidebar=True)
    assert tomllib.loads(config.read_text())["workspace"]["account_sign_in_handled"]
    assert not list((root / "jcode").glob("**/*credentials*")), "Offline sign-in wrote credentials"
    # Leave the welcome screen visible in the final artifact.
    current = words("account-settings-final", "Jcode account", sidebar=True)
    ui.click(phrase_bounds(current, "Sign in with email"))
    words("account-final", "Welcome to Jcode Desktop")
    ui.artifact("account-result.json").write_text(json.dumps({
        "welcome_and_magic_link_copy": True,
        "native_sign_in_waiting_reopen_and_cancel": True,
        "keyboard_skip_persisted": True,
        "other_configuration_unchanged": True,
        "settings_reentry_and_escape": True,
        "network_and_credential_writes": "disabled in offline fixture",
    }, indent=2) + "\n")
