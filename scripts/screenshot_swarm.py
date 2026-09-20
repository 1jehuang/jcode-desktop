"""Native swarm acceptance, called only by screenshot.py's private Xvfb harness.

Run: python3 scripts/screenshot.py target/swarm-native.png --swarm-interact --no-build

The three-agent fixture must not expose a sidebar swarm showcase. OCR checks
absence across the entire sidebar, and native lead selection must preserve the
single focused panel. GPUI tests separately verify exact compact row, header,
and list geometry before and after adding swarm metadata.
No GPUI selectors, control API, live display, network, or fixture mutation.
Per-step PNG/OCR/navigation and a JSON report survive temporary-root cleanup.
"""
import json
import shutil

from default_directory_acceptance import NativeUI, assert_focused, panels
from model_picker_acceptance import phrase_bounds


LEAD = "screenshot-fixture"
LABELS = ("API reviewer", "Sidebar implementation", "Test runner")


def navigation(root):
    text = (root / "state").read_text()
    lines = [line[len("navigation="):] for line in text.splitlines()
             if line.startswith("navigation=")]
    assert len(lines) == 1, "Missing independent navigation fixture evidence"
    return json.loads(lines[0])


def assert_sessions(state, expected, focused):
    live = [panel for panel in panels(state) if not panel.get("closing")]
    actual = [panel["session"] for panel in live]
    assert sorted(actual) == sorted(expected), (actual, expected)
    matches = [panel for panel in live if panel["session"] == focused]
    assert len(matches) == 1, (focused, state)
    assert_focused(state, matches[0]["id"])
    assert not any(state.get(key) for key in ("camera_motion", "tab_motion", "row_motion")), "Navigation still moving"
    return matches[0]["id"]


def has_phrase(words, phrase):
    try:
        phrase_bounds(words, phrase)
        return True
    except AssertionError:
        return False


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    assert env.get("JCODE_DESKTOP_SCREENSHOT_SWARM") == "1", "Swarm fixture required"
    report = {"passed": False, "checks": [], "scope": "native X11 + rendered OCR + independent navigation"}
    stage = "initial"

    def frame(label):
        # Move away so tooltips cannot satisfy absence or title checks.
        ui.native("mousemove", 1100, 700)

        def check(image):
            assert image.size == (1440, 1000), "Default screenshot dimensions required"
            words = ui.words(image, (0, 0, 224, 1000), label, psm=6)
            title = phrase_bounds(words, "Review markdown")
            for phrase in (*LABELS, "Swarm", "Open lead", "Show all", "Show fewer",
                           "working", "need attention"):
                assert not has_phrase(words, phrase), f"{phrase!r} unexpectedly visible"
            if label == "lead-click":
                phrase_bounds(words, "1 selected")
                phrase_bounds(words, "Close selected")
            assert_sessions(navigation(root), [LEAD], LEAD)
            return image, title

        return ui.wait_frame(label, check)

    def passed(check):
        report["checks"].append(check)
        print("Swarm native check: " + check, flush=True)

    try:
        image, title = frame(stage)
        lead_id = assert_sessions(navigation(root), [LEAD], LEAD)
        passed("swarm fixture renders no showcase labels, children, summary, or expansion controls")

        stage = "lead-click"
        ui.click(title)
        image, after_title = frame(stage)
        # Selecting a row adds the normal bulk-selection toolbar above it.
        # Its vertical offset is intentional, not a swarm expansion.
        assert after_title[0] == title[0] and after_title[2] == title[2], "Lead title width changed"
        assert after_title[3] - after_title[1] == title[3] - title[1], "Lead title height changed"
        assert 0 < after_title[1] - title[1] <= 48, "Unexpected selection toolbar offset"
        assert assert_sessions(navigation(root), [LEAD], LEAD) == lead_id, "Lead click replaced the panel"
        passed("native lead click preserves one focused session and compact single-line sidebar title")
        shutil.copyfile(ui.artifact(stage + ".png"), output)
        report["passed"] = True
    except Exception as error:
        report.update(failed_stage=stage, error=str(error))
        raise
    finally:
        report["last_stage"] = stage
        for source, suffix in ((root / "state", "swarm-final-state.txt"),
                               (root / "app.log", "swarm-app.log"),
                               (root / "logs/jcode-desktop/jcode-desktop.log", "swarm-desktop.log")):
            if source.exists():
                shutil.copyfile(source, ui.artifact(suffix))
        output.with_suffix(".swarm.json").write_text(json.dumps(report, indent=2) + "\n")
