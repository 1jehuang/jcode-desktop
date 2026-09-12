"""Real self-dev preview API and native UI acceptance on screenshot.py's private display."""
import json
from pathlib import Path
import subprocess
import sys
import time
from PIL import Image

from model_picker_acceptance import normalized, parse_words, phrase_bounds


EXPECTED = {
    "empty", "streaming", "login-error", "model-access-error",
    "rate-limit", "disconnected", "login-dialog-error",
}


def verify(output, env, root):
    assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1"
    assert env.get("JCODE_DESKTOP_SELF_DEV") == "1"
    assert env["XDG_RUNTIME_DIR"] == str(root / "runtime")
    cli = Path(__file__).with_name("preview-state.py")
    report = {"scope": "real offline app, private Xvfb, public preview CLI and native clicks", "checks": {}}

    def request(*args, success=True):
        result = subprocess.run([sys.executable, str(cli), *args], env=env,
                                text=True, capture_output=True, timeout=15)
        if success:
            assert result.returncode == 0, result.stderr + result.stdout
            value = json.loads(result.stdout)
            assert value["ok"], value
            return value
        assert result.returncode != 0, result.stdout
        return result

    def navigation():
        for line in (root / "state").read_text().splitlines():
            if line.startswith("navigation="):
                return json.loads(line.split("=", 1)[1])
        raise AssertionError("Missing navigation state")

    def panels():
        return [panel for row in navigation()["rows"] for panel in row["panels"]]

    def wait_for(predicate, message):
        deadline = time.monotonic() + 10
        while not predicate():
            if time.monotonic() >= deadline:
                raise AssertionError(message + ": " + json.dumps(navigation()))
            time.sleep(.1)

    def native(*args):
        subprocess.run(["xdotool", *map(str, args)], env=env, cwd=root,
                       check=True, timeout=10)

    def capture(label):
        path = output.with_name(output.stem + "-" + label + ".png")
        time.sleep(.4)
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=env, check=True, timeout=15)
        ocr_path = path.with_suffix(".ocr.png")
        with Image.open(path) as image:
            image.resize((image.width * 3, image.height * 3)).save(ocr_path)
        tsv = subprocess.check_output(
            ["tesseract", str(ocr_path), "stdout", "--psm", "11", "tsv"],
            env=env, stderr=subprocess.DEVNULL, timeout=20).decode()
        words = [w for w in parse_words(tsv, (0, 0, 10000, 10000)) if normalized(w["text"])]
        return words

    def click_phrase(words, phrase):
        bounds = phrase_bounds(words, phrase)
        native("mousemove", round((bounds[0] + bounds[2]) / 2),
               round((bounds[1] + bounds[3]) / 2), "click", "1")

    catalog = request("--list")
    ids = {state["id"] for state in catalog["states"]}
    assert ids == EXPECTED, ids
    report["checks"]["catalog"] = sorted(ids)
    initial = {panel["session"] for panel in panels()}
    assert len(initial) == 1
    before = len(panels())
    request("not-a-real-state", success=False)
    assert len(panels()) == before
    report["checks"]["unknown_state_rejected_without_mutation"] = True

    for state in sorted(ids):
        opened = request(state)
        session_id = opened["id"]
        wait_for(lambda: any(p["session"] == session_id and p["focused"] for p in panels()),
                 "New preview did not receive focus")
        assert initial <= {p["session"] for p in panels()}, "Existing panel was replaced"
        native("key", "super+f")
        wait_for(lambda: any(p["session"] == session_id and p["width"] == 1 for p in panels()),
                 "Preview did not maximize")
        words = capture(state)
        assert any("preview" in normalized(w["text"]) for w in words), (state, words)
        text = " ".join(normalized(w["text"]) for w in words)
        if state == "login-error":
            assert "authentication failed" in text, text
            click_phrase(words, "Choose model")
            choices = capture(state + "-models")
            phrase_bounds(choices, "Choose a model")
            request("--reset", state)
            words = capture(state + "-reset")
            click_phrase(words, "Log in")
            login = capture(state + "-login")
            phrase_bounds(login, "Connect account")
        elif state == "model-access-error":
            assert "model is unavailable" in text, text
        elif state == "rate-limit":
            assert "rate limit" in text, text
        reset = request("--reset", state)
        assert reset["id"] == session_id, reset
        assert len(panels()) == before + 1, "Reset spawned another panel"
        report["checks"][state] = {"opened": True, "rendered": True, "reset_same_panel": True}
        native("key", "ctrl+shift+w")
        wait_for(lambda: all(p["session"] != session_id for p in panels()),
                 "Native close did not retire preview")
        assert {p["session"] for p in panels()} == initial

    report["checks"]["existing_panel_preserved"] = True
    # Preview identities must not leak into reload/crash-recovery session lists.
    checkpoints = list(root.rglob("crash-recovery.json"))
    if checkpoints:
        time.sleep(1.2)
        for path in checkpoints:
            assert "preview://" not in path.read_text(), path
        report["checks"]["previews_excluded_from_checkpoints"] = True
    path = output.with_name(output.stem + "-acceptance.json")
    path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
