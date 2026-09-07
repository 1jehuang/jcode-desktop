"""Native repeated-close acceptance, only for screenshot.py's private Xvfb.

One XTest keydown starts each hold. The private X server supplies native repeats
until the expected number of panels closes, with no injected intervening keyup.
The public navigation dump supplies stable identities across slot retirement.
"""
import json
import subprocess
import time


def panels(nav):
    return [panel for row in nav["rows"] for panel in row["panels"]]


def assert_live_focus(nav):
    """A closing surface must never own composer focus, even before retirement."""
    row = nav["rows"][nav["active_row"]]
    live = [panel for panel in row["panels"] if not panel["closing"]]
    keyboard = nav["keyboard_panel"]
    if live:
        focused = [panel for panel in live if panel["slot"] == nav["focused_slot"]]
        assert len(focused) == 1, ("focused slot is not live", nav)
        assert keyboard == focused[0]["slot"], ("composer focus differs from live selection", nav)
        assert focused[0]["focused"], ("panel focus marker differs from selection", nav)
    else:
        assert keyboard is None, ("empty row still focuses a closing composer", nav)
    return live


def verify(output, env, root, *, layout_mode):
    state = root / "state"
    report_path = output.with_suffix(".close-panel.json")
    frames = {stage: output.with_name(f"{output.stem}-close-{stage}.png")
              for stage in ("survivors", "empty", "reopened")}
    if any(path.exists() for path in [report_path, *frames.values()]):
        raise FileExistsError("close evidence already exists; choose a new output name")
    assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1"
    assert env.get("JCODE_DESKTOP_STATE") == str(state)
    assert env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY")
    observations = []
    started = time.monotonic()

    def native(*args):
        subprocess.run(["xdotool", *args], env=env, cwd=root, check=True, timeout=10)

    def read_nav():
        # The app truncates and rewrites this diagnostic file each render.
        try:
            line = next(line for line in state.read_text().splitlines()
                        if line.startswith("navigation="))
            return json.loads(line.partition("=")[2])
        except (OSError, StopIteration, json.JSONDecodeError):
            return None

    def wait_for(label, predicate, *, validate=True):
        deadline = time.monotonic() + 8
        last = None
        while time.monotonic() < deadline:
            nav = read_nav()
            if nav is not None:
                last = nav
                if validate:
                    assert_live_focus(nav)
                if not observations or observations[-1]["navigation"] != nav:
                    observations.append({"stage": label, "seconds": time.monotonic() - started,
                                         "navigation": nav})
                if predicate(nav):
                    return nav
            time.sleep(.003)
        raise AssertionError(("timed out", label, last))

    def capture(stage):
        before = wait_for(f"{stage}-ready", lambda nav: True)
        # The navigation file is written during render, before X11 presents it.
        # Let the new draft's entrance settle before sampling rendered pixels.
        time.sleep(.4)
        after = wait_for(f"{stage}-presented", lambda nav: True)
        assert [(p["id"], p["closing"]) for p in panels(before)] == [
            (p["id"], p["closing"]) for p in panels(after)
        ], ("panel state changed after key release", stage, after)
        subprocess.run(["import", "-window", "root", "png:" + str(frames[stage])],
                       env=env, cwd=root, check=True, timeout=15)

    report = {"passed": False, "layout_mode": layout_mode,
              "input": "native XTest hold with X11 autorepeat and no injected intervening keyup",
              "repeat_delay_ms": 60, "repeat_rate_hz": 10,
              "observations": observations}
    try:
        initial = wait_for("initial", lambda nav: len(panels(nav)) == 6)
        ids = [panel["id"] for panel in panels(initial)]
        native("key", "--clearmodifiers", "super+End")
        wait_for("right-edge", lambda nav: nav["focused_slot"] == panels(nav)[-1]["slot"]
                 and not nav["camera_motion"])
        # Only the throwaway Xvfb server is changed, never the user's keyboard.
        subprocess.run(["xset", "r", "rate", "60", "10"], env=env, cwd=root, check=True, timeout=10)

        def close_burst(count, already_closed):
            native("keydown", "--delay", "0", "Super_L")
            try:
                native("keydown", "--delay", "0", "q")
                for repeat in range(count):
                    expected = 6 - already_closed - repeat - 1
                    nav = wait_for(f"close-{already_closed + repeat + 1}", lambda nav:
                                   len([p for p in panels(nav) if not p["closing"]]) == expected)
                    live = assert_live_focus(nav)
                    assert [p["id"] for p in live] == ids[:expected], ("wrong panels closed", nav)
                    if live:
                        assert live[-1]["slot"] == nav["focused_slot"], ("not the rightmost survivor", nav)
            finally:
                native("keyup", "--delay", "0", "q", "Super_L")

        close_burst(3, 0)
        assert any(sum(p["closing"] for p in panels(item["navigation"])) >= 2
                   for item in observations), "probe did not exercise overlapping close animations"
        survivors = wait_for("survivors-retired", lambda nav:
                             len(panels(nav)) == 3 and not any(p["closing"] for p in panels(nav))
                             and not nav["camera_motion"])
        assert [p["id"] for p in panels(survivors)] == ids[:3]
        capture("survivors")
        close_burst(3, 3)
        wait_for("all-retired", lambda nav: not panels(nav))
        capture("empty")
        native("key", "--clearmodifiers", "super+n")
        reopened = wait_for("reopened", lambda nav:
                            len(panels(nav)) == 1 and not panels(nav)[0]["closing"])
        assert panels(reopened)[0]["id"] not in ids, ("old panel resurrected", reopened)
        capture("reopened")
        report.update(passed=True, initial_ids=ids, survivor_ids=ids[:3],
                      reopened_id=panels(reopened)[0]["id"],
                      screenshots={stage: path.name for stage, path in frames.items()})
        print(f"Held close PASS ({layout_mode}): six native keydowns, overlapping fades, "
              "survivor focus after retirement, empty workspace, and Super+N reopen", flush=True)
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        report_path.write_text(json.dumps(report, indent=2) + "\n")
