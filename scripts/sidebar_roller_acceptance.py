"""Native sidebar roller acceptance on screenshot.py's private offline Xvfb.

Run: python3 scripts/screenshot.py target/sidebar-roller.png --roller-interact
Use --no-build only with a current binary. PNG/OCR/navigation evidence and a
.sidebar-roller.json report are retained, including on failure.
"""
import json
import shutil
import time

from default_directory_acceptance import NativeUI, panels, phrase_bounds


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    report = {"passed": False, "observations": [], "scroll_events": 0}
    stage = "initial"

    def navigation():
        deadline = time.monotonic() + 5
        while True:
            try:
                return json.loads(next(line.partition("=")[2]
                                       for line in (root / "state").read_text().splitlines()
                                       if line.startswith("navigation=")))
            except (OSError, StopIteration, json.JSONDecodeError):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.05)

    def identity(state):
        return {"panels": sorted((p["id"], p["session"], p["slot"], p.get("closing", False))
                                  for p in panels(state)),
                "count": len(panels(state)),
                "focused_slot": state["focused_slot"],
                "keyboard_panel": state["keyboard_panel"],
                "active_row": state["active_row"]}

    def unchanged(label):
        state = navigation()
        report["observations"].append({"stage": label, "navigation": state})
        assert identity(state) == baseline, ("Roller changed panels or keyboard focus", label, state)

    def words(image, label, header=False):
        return ui.words(image, (0, 0, min(264, image.width),
                                min(300, image.height) if header else image.height),
                        label, psm=11)

    def visible(image, phrase, label, header=False):
        return phrase_bounds(words(image, label, header), phrase)

    def popup(image):
        # Read the title/hint separately so two-column page OCR ordering cannot
        # merge the hint into an adjacent row. Click targets use actual OCR boxes.
        header = ui.words(image, (0, 0, 264, 70), stage + "-header", psm=6)
        phrase_bounds(header, "Sidebar")
        phrase_bounds(header, "Scroll to switch")
        page_words = words(image, stage + "-pages", header=True)
        for page in ("chat", "learn", "files", "accounts", "theme", "settings"):
            phrase_bounds(page_words, page)
        unchanged(stage)
        return page_words

    def collapsed(image):
        header = ui.words(image, (0, 0, 264, 70), stage + "-closed-header", psm=6)
        try:
            phrase_bounds(header, "Scroll to switch")
        except AssertionError:
            unchanged(stage)
            return
        raise AssertionError("Hover popout remained visible after pointer exit")

    def hover():
        ui.native("mousemove", 132, 30)
        return ui.wait_frame(stage, popup)

    def outside():
        ui.native("mousemove", 700, 600)
        return ui.wait_frame(stage, collapsed)

    def scroll():
        ui.native("mousemove", 132, 30, "click", 5)
        time.sleep(.35)
        report["scroll_events"] += 1
        unchanged(stage)

    try:
        ui.native("mousemove", 700, 600)
        time.sleep(.5)
        baseline = identity(navigation())
        assert baseline["count"] > 0 and baseline["keyboard_panel"] is not None, baseline
        unchanged(stage)
        stage = "hover-open"
        hover()
        shutil.copyfile(ui.artifact(stage + ".png"), output)

        stage = "scroll-four-notches"
        for _ in range(4):
            scroll()
        def live_theme(image):
            popup(image)
            palette_words = words(image, stage + "-palettes")
            # These exact ThemePreset labels are below the popout. Unlike the
            # occluded title or its "theme" option, they prove the live page
            # changed while the pointer was still over the roller header.
            labels = ("Midnight", "Ocean", "Forest")
            bounds = {label: phrase_bounds(palette_words, label) for label in labels}
            unchanged("theme-live-after-four-notches")
            return bounds

        report["live_palette_bounds"] = ui.wait_frame(stage, live_theme)
        report["theme_visible_behind_popup"] = True
        report["theme_selected_after_notches"] = report["scroll_events"]
        assert report["scroll_events"] == 4, report

        stage = "theme-collapsed"
        outside()
        # No more wheel events: persistence must hold for the page selected by
        # exactly four notches, rather than finding Theme again after dismissal.
        ui.wait_frame(stage, lambda image: visible(image, "Choose a palette", stage))
        unchanged("theme-remains-after-collapse")
        report["theme_remains_after_collapse"] = True

        stage = "return-hover"
        page_words = hover()
        ui.click(phrase_bounds(page_words, "chat"))
        time.sleep(.35)
        unchanged("chat-click")
        stage = "chat-collapsed"
        outside()
        # The default offline fixture supplies this visible session row. A
        # selected Theme label alone must not count as a successful page change.
        ui.wait_frame(stage, lambda image: visible(image, "Review markdown", stage))
        unchanged(stage)
        report["chat_restored"] = True
        report["passed"] = True
    except Exception as error:
        report.update(failed_step=stage, error=str(error))
        try:
            ui.capture("roller-failure")
        except Exception as capture_error:
            report["capture_error"] = str(capture_error)
        raise
    finally:
        for source, suffix in ((root / "state", "roller-final-state.txt"),
                               (root / "app.log", "roller-app.log")):
            if source.exists():
                shutil.copyfile(source, ui.artifact(suffix))
        path = output.with_suffix(".sidebar-roller.json")
        path.write_text(json.dumps(report, indent=2) + "\n")
        print("Sidebar roller acceptance: " + str(path), flush=True)
