"""Real sound settings clicks and persistence, always muted on private Xvfb."""
import json
import tomllib

from default_directory_acceptance import NativeUI
from model_picker_acceptance import phrase_bounds


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    config = root / "desktop.toml"
    assert env["JCODE_DESKTOP_CONFIG"] == str(config)
    original = tomllib.loads(config.read_text())
    assert not original.get("sounds", {}).get("enabled", False)

    def words(label):
        image = ui.capture(label)
        return ui.words(image, (0, 0, 264, image.height), label)

    initial = words("sounds-initial")
    chat = phrase_bounds(initial, "chat")
    nav_y = round((chat[1] + chat[3]) / 2)
    for step in range(15):
        current = words(f"sounds-tabs-{step}")
        try:
            tab = phrase_bounds(current, "settings")
            ui.click(tab)
            break
        except AssertionError:
            ui.native("mousemove", 180, nav_y, "click", "5")
    else:
        raise AssertionError("Settings tab not reachable through native tab scrolling")

    current = words("sounds-default-off")
    toggle = phrase_bounds(current, "Sound effects")
    assert not any(word["text"].lower() == "preview" for word in current)
    ui.click(toggle)
    current = words("sounds-enabled")
    saved = tomllib.loads(config.read_text())
    assert saved["sounds"]["enabled"] is True
    assert saved["appearance"] == original["appearance"]
    preview = phrase_bounds(current, "Play preview")
    ui.click(preview)
    assert tomllib.loads(config.read_text())["sounds"]["enabled"] is True
    ui.click(phrase_bounds(current, "Sound effects"))
    current = words("sounds-disabled")
    assert tomllib.loads(config.read_text())["sounds"]["enabled"] is False
    assert not any(word["text"].lower() == "preview" for word in current)
    # Final screenshot demonstrates the discoverable preview and enabled state.
    ui.click(phrase_bounds(current, "Sound effects"))
    current = words("sounds-final")
    phrase_bounds(current, "Play preview")
    ui.artifact("sounds-result.json").write_text(json.dumps({
        "default_off": True,
        "native_toggle_and_preview": True,
        "on_and_off_persisted": True,
        "other_config_preserved": True,
        "audio": "intentionally muted by offline fixture",
    }, indent=2) + "\n")
