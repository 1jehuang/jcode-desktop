"""Exercise real Alt+9 and onboarding controls on screenshot.py's private Xvfb."""
import json

from default_directory_acceptance import NativeUI
from model_picker_acceptance import phrase_bounds


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    config = (root / "desktop.toml").read_bytes()

    def words(label, expected):
        image = ui.capture(label)
        current = ui.words(image, (0, 0, image.width, image.height), label, psm=11)
        phrase_bounds(current, expected)
        return current

    ui.native("key", "alt+9")
    current = words("onboarding-welcome", "Welcome to Jcode Desktop")
    ui.click(phrase_bounds(current, "Get started"))
    current = words("onboarding-account", "Connect your")
    ui.click(phrase_bounds(current, "Simulate sign-in error"))
    current = words("onboarding-error", "Demo sign-in failed")
    ui.click(phrase_bounds(current, "Retry demo connection"))
    current = words("onboarding-connected", "Demo account connected")
    ui.click(phrase_bounds(current, "Continue"))
    current = words("onboarding-folder", "Choose a project")
    ui.click(phrase_bounds(current, "Use demo project"))
    current = words("onboarding-selected", "Selected")
    ui.click(phrase_bounds(current, "Continue"))
    current = words("onboarding-ready", "You're ready")
    ui.click(phrase_bounds(current, "Return to workspace"))
    ui.native("key", "alt+9")
    current = words("onboarding-reopened", "Welcome to Jcode Desktop")
    ui.native("key", "alt+9")
    ui.capture("onboarding-toggled-off")
    ui.native("key", "alt+9")
    words("onboarding-toggle-back", "Welcome to Jcode Desktop")
    ui.native("key", "Escape")
    ui.capture("onboarding-escaped")
    ui.native("key", "alt+9")
    words("onboarding-final", "Welcome to Jcode Desktop")
    assert (root / "desktop.toml").read_bytes() == config
    assert not (root / "home/Projects/hello-jcode").exists()
    ui.artifact("onboarding-result.json").write_text(json.dumps({
        "native_alt_9": True,
        "four_steps": True,
        "mock_connection_error_and_retry": True,
        "mock_project_selection": True,
        "finish_toggle_and_escape": True,
        "configuration_unchanged": True,
        "no_project_created": True,
    }, indent=2) + "\n")
