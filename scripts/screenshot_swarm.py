"""Native swarm acceptance, called only by screenshot.py's private Xvfb harness.

Run: python3 scripts/screenshot.py target/swarm-native.png --swarm-interact --no-build

Requires the current two-line swarm UI and the existing three-agent fixture.
OCR locates rendered controls, native X11 clicks exercise actual event routing,
independent navigation state proves session identity, focus and slot retirement.
In particular, Test runner is initially hidden: choosing it tests that collapse
retains the viewed child, rather than merely retaining a default preview row.
No GPUI selectors, control API, live display, network, or fixture mutation.
Per-step PNG/OCR/navigation and a JSON report survive temporary-root cleanup.
"""
import json
import shutil
import time

from default_directory_acceptance import NativeUI, assert_focused, panels
from model_picker_acceptance import phrase_bounds


LEAD = "screenshot-fixture"
CHILD = "swarm-fixture-2"
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


def close_ink(image, title):
    y = round((title[1] + title[3]) / 2)
    return [(x, yy) for x in range(210, 245) for yy in range(y - 8, y + 9)
            if max(image.getpixel((x, yy))[:3]) > 120]


def close_bounds(image, title):
    """Locate the painted ×, not its GPUI selector or a guessed row index.

    Default 264px sidebar: outer 8px margin, parent/card 8px padding.
    The short Test runner label leaves this right-hand icon gutter empty when
    closed. Find the glyph's ink inside that gutter and click its center.
    This remains independent of row y, header height and expanded row count.
    """
    points = close_ink(image, title)
    assert 3 <= len(points) <= 100, f"Expected one visible close glyph, found {len(points)} ink pixels"
    xs, ys = zip(*points)
    assert max(xs) - min(xs) <= 12 and max(ys) - min(ys) <= 12, "Close gutter contains unexpected text"
    return min(xs), min(ys), max(xs) + 1, max(ys) + 1


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    assert env.get("JCODE_DESKTOP_SCREENSHOT_SWARM") == "1", "Swarm fixture required"
    report = {"passed": False, "checks": [], "scope": "native X11 + rendered OCR + independent navigation"}
    stage = "initial"

    def frame(label, visible, hidden, toggle, expected, focused):
        # Move away before every capture so tooltips cannot satisfy OCR checks.
        ui.native("mousemove", 1100, 700)

        def check(image):
            assert image.size == (1440, 1000), "Default screenshot dimensions required"
            words = ui.words(image, (28, 280, 252, 850), label, psm=6)
            phrase_bounds(words, "Swarm 3")
            phrase_bounds(words, "Open lead")
            phrase_bounds(words, "2 working")
            phrase_bounds(words, "1 done")
            for phrase in visible:
                phrase_bounds(words, phrase)
            if focused == CHILD:
                title = phrase_bounds(words, "Test runner")
                detail = ui.words(image, (36, round(title[3]), 205, round(title[3] + 24)),
                                  label + "-viewing", psm=7)
                phrase_bounds(detail, "viewing")
            for phrase in hidden:
                assert not has_phrase(words, phrase), f"{phrase!r} unexpectedly visible"
            toggle_bounds = phrase_bounds(words, toggle)
            assert_sessions(navigation(root), expected, focused)
            return image, words, toggle_bounds

        return ui.wait_frame(label, check)

    def passed(check):
        report["checks"].append(check)
        print("Swarm native check: " + check, flush=True)

    try:
        image, words, toggle = frame(stage, LABELS[:2], LABELS[2:], "Show all 3 agents", [LEAD], LEAD)
        lead_id = assert_sessions(navigation(root), [LEAD], LEAD)
        passed("collapsed preview hides Test runner")

        stage = "expanded"
        ui.click(toggle)
        image, words, toggle = frame(stage, LABELS, (), "Show fewer", [LEAD], LEAD)
        passed("native expand shows all three agent cards without navigation")

        stage = "child-open"
        ui.click(phrase_bounds(words, "Test runner"))
        image, words, toggle = frame(stage, LABELS, (), "Show fewer", [LEAD, CHILD], CHILD)
        child_id = assert_sessions(navigation(root), [LEAD, CHILD], CHILD)
        close_bounds(image, phrase_bounds(words, "Test runner"))
        passed("native child click opens exactly the intended session with keyboard focus and close affordance")

        stage = "collapsed-viewed-child"
        ui.click(toggle)
        image, words, toggle = frame(stage, (LABELS[0], LABELS[2]), (LABELS[1],),
                                    "Show all 3 agents", [LEAD, CHILD], CHILD)
        assert assert_sessions(navigation(root), [LEAD, CHILD], CHILD) == child_id, "Collapse replaced the viewed child"
        passed("collapse keeps the viewed non-preview child visible and focused")

        stage = "lead-return"
        ui.click(phrase_bounds(words, "Open lead"))
        image, words, toggle = frame(stage, LABELS[:2], LABELS[2:], "Show all 3 agents", [LEAD, CHILD], LEAD)
        assert assert_sessions(navigation(root), [LEAD, CHILD], LEAD) == lead_id, "Open lead duplicated/replaced lead"
        passed("Open lead returns to the original lead and preserves the child view")

        # Close while lead is focused: a bubbling row click would reopen/focus
        # the child. This provides a stronger oracle than closing the active row.
        stage = "close-ready"
        ui.click(toggle)
        image, words, toggle = frame(stage, LABELS, (), "Show fewer", [LEAD, CHILD], LEAD)
        ui.click(close_bounds(image, phrase_bounds(words, "Test runner")))
        stage = "child-closed"
        image, words, toggle = frame(stage, LABELS, (), "Show fewer", [LEAD], LEAD)
        title = phrase_bounds(words, "Test runner")
        assert not close_ink(image, title), "Closed child still paints ink in its close gutter"
        # Wait for physical slot retirement, not merely closing=true. Continue
        # observing after retirement to catch a delayed bubbling reopen.
        deadline = time.monotonic() + 12
        stable_since = None
        while True:
            state = navigation(root)
            assert assert_sessions(state, [LEAD], LEAD) == lead_id
            retired = all(panel["session"] != CHILD for panel in panels(state))
            now = time.monotonic()
            stable_since = (stable_since or now) if retired else None
            if stable_since is not None and now - stable_since >= 1.5:
                break
            assert now < deadline, "Child view never retired"
            time.sleep(.1)
        frame(stage, LABELS, (), "Show fewer", [LEAD], LEAD)
        passed("close retires only the child view without reopening or removing its agent card")
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
