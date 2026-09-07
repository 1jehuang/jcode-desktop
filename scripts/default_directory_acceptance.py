"""Native default-directory acceptance inside screenshot.py's private Xvfb.

Run: python3 scripts/screenshot.py target/default-directory.png \
    --default-directory-interact --no-build

Evidence is real mouse/keyboard input, rendered OCR, and the isolated TOML file.
The offline fixture has no real historical directories, so a private home child
exercises the same clickable frequent-folder list without touching host folders.
No daemon or provider is contacted. Per-step PNG/OCR, TOML, navigation, logs, and
an acceptance JSON report survive the harness's temporary-directory cleanup.
"""
import json
import shutil
import subprocess
import time
import tomllib

from PIL import Image

from model_picker_acceptance import normalized, parse_words, phrase_bounds


DEFAULT_DIRECTORY_SESSION = "settings://default-directory"


def panels(state):
    return [panel for row in state["rows"] for panel in row["panels"]]


def default_panel(state):
    matches = [panel for panel in panels(state)
               if panel["session"] == DEFAULT_DIRECTORY_SESSION]
    assert len(matches) == 1, "Expected exactly one default-directory slot: " + json.dumps(state)
    assert not matches[0].get("closing"), "Default-directory slot is closing"
    return matches[0]


def panel_bounds(state, panel, image_size):
    """Settled folder-tab geometry, located by identity rather than slot number.

    screenshot.py fixes the sidebar/canvas insets for this acceptance mode. Row
    order, panel widths, camera scrolling and image height come from live output.
    OCR below locates the controls inside this crop, not fixed modal coordinates.
    """
    width, height = image_size
    canvas_left, canvas_right = 276, width - 12
    viewport = canvas_right - canvas_left
    row = next(row for row in state["rows"] if row["row"] == state["active_row"])
    assert not state.get("camera_motion"), "Wait for workspace camera to settle"
    left = canvas_left - row["camera"]
    for candidate in row["panels"]:
        panel_width = max(320, (viewport - 2 * .58) * candidate["width"])
        if candidate["id"] == panel["id"]:
            bounds = (round(max(canvas_left, left)), 48,
                      round(min(canvas_right, left + panel_width)), height - 16)
            assert bounds[2] - bounds[0] > 250, "Panel is clipped or offscreen"
            return bounds
        left += panel_width
    raise AssertionError("Panel is not in the active row")


def assert_focused(state, panel_id):
    focused = [panel for panel in panels(state) if panel["focused"]]
    assert len(focused) == 1 and focused[0]["id"] == panel_id, state
    assert state["keyboard_panel"] == focused[0]["slot"], "Input focus differs from workspace focus: " + json.dumps(state)


class NativeUI:
    def __init__(self, output, env, root):
        assert env.get("JCODE_DESKTOP_SCREENSHOT") == "1", "Offline fixture required"
        assert env.get("XDG_RUNTIME_DIR") == str(root / "runtime"), "Private runtime required"
        assert env.get("HOME") == str(root / "home"), "Private home required"
        assert env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"), "Private X11 required"
        self.output, self.env, self.root = output, env, root

    def artifact(self, suffix):
        return self.output.with_name(self.output.stem + "-" + suffix)

    def native(self, *args):
        subprocess.run(["xdotool", *map(str, args)], env=self.env, cwd=self.root,
                       check=True, timeout=15)

    def click(self, bounds):
        x1, y1, x2, y2 = bounds
        self.native("mousemove", round((x1 + x2) / 2), round((y1 + y2) / 2), "click", "1")

    def capture(self, label):
        time.sleep(.35)
        path = self.artifact(label + ".png")
        subprocess.run(["import", "-window", "root", "png:" + str(path)],
                       env=self.env, cwd=self.root, check=True, timeout=15)
        if (self.root / "state").exists():
            shutil.copyfile(self.root / "state", self.artifact(label + "-state.txt"))
        with Image.open(path) as image:
            return image.convert("RGB")

    def words(self, image, bounds, label, psm=6):
        path = self.artifact(label + "-ocr.png")
        crop = image.crop(bounds)
        crop.resize((crop.width * 3, crop.height * 3)).save(path)
        tsv = subprocess.check_output(
            ["tesseract", str(path), "stdout", "--psm", str(psm), "tsv"],
            env=self.env, cwd=self.root, timeout=20, stderr=subprocess.DEVNULL,
        ).decode()
        self.artifact(label + "-ocr.tsv").write_text(tsv)
        return parse_words(tsv, bounds)

    def wait_frame(self, label, check):
        deadline = time.monotonic() + 12
        while True:
            image = self.capture(label)
            try:
                return check(image)
            except AssertionError:
                if time.monotonic() >= deadline:
                    raise


def click_sidebar_text(output, env, root, phrase, label):
    """Find visible session text instead of assuming a fixed sidebar row height."""
    ui = NativeUI(output, env, root)

    def locate(image):
        words = ui.words(image, (8, 140, 256, image.height - 20), label)
        try:
            return phrase_bounds(words, phrase)
        except AssertionError:
            # Slashed zero can become 8 in block OCR. Re-read individual title
            # lines, never fuzzy-match digits that could select another session.
            for index, word in enumerate(words):
                if normalized(word["text"]) != normalized(phrase.split()[0]):
                    continue
                bounds = (round(word["x"] - 5), round(word["y"] - 5), 250,
                          round(word["y"] + word["height"] + 5))
                row = ui.words(image, bounds, f"{label}-row-{index}", psm=7)
                try:
                    return phrase_bounds(row, phrase)
                except AssertionError:
                    pass
            raise
    bounds = ui.wait_frame(label, locate)
    ui.click(bounds)


def verify(output, env, root):
    ui = NativeUI(output, env, root)
    config = root / "desktop.toml"
    assert env.get("JCODE_DESKTOP_CONFIG") == str(config), "Isolated TOML required"
    frequent = root / "home" / "frequent"
    custom = root / "home" / "Custom Directory"
    frequent.mkdir()
    custom.mkdir()
    invalid_file = root / "home" / "not-a-directory"
    invalid_file.write_text("This is a regular file, not a directory.\n")
    missing = root / "home" / "missing-directory"
    original_config = config.read_bytes()
    original_settings = tomllib.loads(original_config.decode())
    report = {"checks": {}, "failures": [], "scope": "native offline UI, persisted preference, and local draft",
              "frequent_list_fixture": str(frequent), "custom_directory": str(custom)}
    stage = "initial"

    def pinned():
        return tomllib.loads(config.read_text()).get("workspace", {}).get("pinned_working_dir")

    def picker_words(image, label):
        state = navigation()
        panel = default_panel(state)
        bounds = panel_bounds(state, panel, image.size)
        words = ui.words(image, bounds, label)
        title = phrase_bounds(words, "Default directory")
        assert title[1] < image.height * .25, "Picker must start at the workspace top, not in a modal"
        # Block OCR sometimes drops the isolated bottom button after a large
        # empty list. Read that rendered control independently, not heuristically.
        footer_top = bounds[3] - 65
        footer = ui.words(image, (bounds[0], footer_top, bounds[2], bounds[3]),
                          label + "-footer", psm=6)
        button = phrase_bounds(footer, "Set as default")
        assert button[1] > image.height * .8, "Picker footer must be at the workspace bottom"
        return [word for word in words if word["y"] < footer_top] + footer

    def closed(image):
        assert not any(panel["session"] == DEFAULT_DIRECTORY_SESSION
                       for panel in panels(navigation())), "Utility slot stayed open"
        # The launcher remains visible after the utility slot is removed.
        phrase_bounds(ui.words(image, (0, 48, 264, 110), stage + "-sidebar"), "Default directory")
        words = ui.words(image, (276, 48, image.width - 12, image.height - 16), stage + "-closed")
        assert "defaultdirectory" not in normalized(" ".join(w["text"] for w in words)), "Picker stayed rendered"

    def open_picker(label):
        bounds = ui.wait_frame(label + "-button", lambda image: phrase_bounds(
            ui.words(image, (0, 48, 264, 110), label + "-button"), "Default directory"))
        ui.click(bounds)
        def opened(image):
            state = navigation()
            panel = default_panel(state)
            assert_focused(state, panel["id"])
            assert abs(panel["width"] - .5) < .001, "Default utility width must be one half"
            return picker_words(image, label)
        return ui.wait_frame(label, opened)

    def type_path(text):
        # Opening focuses the input. Use actual native editing, not app state.
        ui.native("key", "--clearmodifiers", "ctrl+a")
        ui.native("type", "--clearmodifiers", "--delay", "15", text)

    def save_typed(label):
        words = ui.wait_frame(label + "-typed", lambda image: picker_words(image, label + "-typed"))
        # Footer is the last occurrence, since unfiltered rows have the same CTA.
        last_y = max(word["y"] for word in words)
        footer = [word for word in words if word["y"] > last_y - 18]
        ui.click(phrase_bounds(footer, "Set as default"))

    def assert_saved(directory, label):
        ui.wait_frame(label, closed)
        assert pinned() == str(directory), (pinned(), str(directory))
        settings = tomllib.loads(config.read_text())
        for section, value in original_settings.items():
            if section != "workspace":
                assert settings.get(section) == value, "Saving default clobbered other settings"
        shutil.copyfile(config, ui.artifact(label + ".toml"))
        report["checks"][stage] = str(directory)
        print(f"Default-directory check passed: {stage}", flush=True)

    def navigation():
        deadline = time.monotonic() + 5
        while True:
            try:
                lines = (root / "state").read_text().splitlines()
                return json.loads(next(line.split("=", 1)[1] for line in lines if line.startswith("navigation=")))
            except (FileNotFoundError, StopIteration, json.JSONDecodeError):
                # The opt-in state file is rewritten during painting.
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.05)

    def record(label, evidence=True):
        report["checks"][label] = evidence
        print(f"Default-directory check passed: {label}", flush=True)

    def focus_key(key, panel_id, label):
        ui.native("key", "--clearmodifiers", key)
        ui.wait_frame(label, lambda image: assert_focused(navigation(), panel_id))

    def assert_query(query, label):
        def visible(image):
            words = picker_words(image, label)
            # Only search below the actual home/computer controls and above the
            # list. A matching directory row must not impersonate the input.
            computer = phrase_bounds(words, "computer")
            state = navigation()
            bounds = panel_bounds(state, default_panel(state), image.size)
            search = ui.words(image, (bounds[0], round(computer[3] + 5),
                                     bounds[2], round(computer[3] + 75)),
                              label + "-search")
            phrase_bounds(search, query)
        ui.wait_frame(label, visible)

    try:
        stage = "inline-slot-and-native-focus"
        before = navigation()
        chat = next(panel for panel in panels(before) if panel["focused"])
        original_ids = {panel["id"] for panel in panels(before)}
        # Keep chat and the new half-width utility visible together. This is a
        # real workspace width shortcut, not fixture state mutation.
        ui.native("key", "--clearmodifiers", "super+2")
        open_picker("inline-open")
        picker = default_panel(navigation())
        assert {panel["id"] for panel in panels(navigation())} == original_ids | {picker["id"]}
        # A trailing space separates the native caret from the last word for OCR.
        type_path("picker editing retained ")
        assert_query("picker editing retained", "picker-editing")
        focus_key("super+Left", chat["id"], "chat-focused-left")
        # Do not submit any prompt. Actual composer editing proves that focus
        # moved beyond the navigation diagnostic to a usable chat input.
        ui.native("type", "--clearmodifiers", "chat editing retained ")

        def chat_editing(image):
            state = navigation()
            assert_focused(state, chat["id"])
            bounds = panel_bounds(state, chat, image.size)
            words = ui.words(image, (bounds[0], round(image.height * .6),
                                    bounds[2], bounds[3]), "chat-editing")
            phrase_bounds(words, "chat editing retained")
        ui.wait_frame("chat-editing", chat_editing)
        assert_query("picker editing retained", "picker-retained-while-away")
        focus_key("super+Right", picker["id"], "picker-focused-right")
        ui.native("key", "--clearmodifiers", "ctrl+End")
        ui.native("type", "--clearmodifiers", "again ")
        assert_query("picker editing retained again", "picker-editing-restored")
        record(stage, {"chat_id": chat["id"], "picker_id": picker["id"], "width": picker["width"]})

        stage = "repeated-launch-focuses-same-panel-preserves-search"
        focus_key("super+h", chat["id"], "chat-focused-before-reopen")
        ui.wait_frame("chat-retained", chat_editing)
        for attempt in range(2):
            open_picker(f"reopen-{attempt}")
            state = navigation()
            assert default_panel(state)["id"] == picker["id"], "Reopening replaced the utility"
            assert {panel["id"] for panel in panels(state)} == original_ids | {picker["id"]}, "Reopening added a duplicate panel"
            assert_query("picker editing retained again", f"reopen-{attempt}-retained")
        record(stage)

        stage = "close-shortcut-removes-utility-not-chat"
        ui.native("key", "--clearmodifiers", "super+q")
        ui.wait_frame("shortcut-closed", closed)
        state = navigation()
        assert {panel["id"] for panel in panels(state)} == original_ids, "Close shortcut removed a chat"
        assert_focused(state, chat["id"])
        assert config.read_bytes() == original_config, "Close shortcut persisted an unsaved search"
        ui.wait_frame("chat-after-picker-close", chat_editing)
        ui.native("key", "--clearmodifiers", "ctrl+a", "BackSpace")
        record(stage)

        stage = "ordinary-open-folder-remains-overlay"
        open_picker("before-open-folder")
        ui.native("key", "--clearmodifiers", "ctrl+o")

        def ordinary_overlay(image):
            state = navigation()
            assert {panel["id"] for panel in panels(state)} == original_ids, "OpenFolder must remove the default utility, not add another slot"
            bounds = ((image.width - 680) // 2, (image.height - 560) // 2,
                      (image.width + 680) // 2, (image.height + 560) // 2)
            words = ui.words(image, bounds, "ordinary-overlay")
            title = phrase_bounds(words, "open folder")
            phrase_bounds(words, "cancel")
            assert image.height * .15 < title[1] < image.height * .5, "Ordinary OpenFolder is no longer centered"
        ui.wait_frame("ordinary-overlay", ordinary_overlay)
        ui.native("key", "Escape")
        ui.wait_frame("ordinary-overlay-closed", closed)
        assert config.read_bytes() == original_config
        record(stage)

        stage = "cancel-button-removes-utility"
        words = open_picker("cancel-button-open")
        type_path("unsaved editing")
        ui.click(phrase_bounds(words, "cancel"))
        ui.wait_frame("cancel-button-closed", closed)
        assert config.read_bytes() == original_config, "Cancel persisted an unsaved search"
        record(stage)

        stage = "open-and-escape-cancels"
        words = open_picker("picker-open")
        phrase_bounds(words, "Choose a frequent folder")
        shutil.copyfile(ui.artifact("picker-open.png"), output)
        type_path("~/Custom Directory")
        ui.native("key", "Escape")
        ui.wait_frame("escape-cancelled", closed)
        assert config.read_bytes() == original_config, "Escape persisted an unsaved path"
        report["checks"][stage] = True
        print(f"Default-directory check passed: {stage}", flush=True)

        stage = "frequent-row-saves-immediately"
        words = open_picker("frequent-open")
        # The path label is in the list, below the search field, not the header.
        home = phrase_bounds(words, "computer")
        row_words = [word for word in words if word["y"] > home[3] + 55]
        matches = [word for word in row_words
                   if normalized(word["text"]) in ("frequent", normalized("~/frequent"), normalized(str(frequent)))]
        assert len(matches) == 1, "Frequent directory row missing: " + " ".join(w["text"] for w in row_words)
        word = matches[0]
        ui.click((word["x"], word["y"], word["x"] + word["width"], word["y"] + word["height"]))
        assert_saved(frequent, "frequent-saved")

        stage = "typed-custom-path-persists"
        open_picker("custom-open")
        type_path("~/Custom Directory")
        save_typed("custom")
        assert_saved(custom, "custom-saved")
        saved_config = config.read_bytes()

        for label, value in (("regular-file", invalid_file), ("missing-path", missing)):
            stage = label + "-rejected"
            open_picker(label + "-open")
            type_path(str(value))
            save_typed(label)

            def rejected(image):
                words = picker_words(image, label + "-error")
                phrase_bounds(words, "Not an existing directory")
            ui.wait_frame(label + "-error", rejected)
            assert config.read_bytes() == saved_config, "Invalid path altered the saved preference"
            report["checks"][stage] = True
            print(f"Default-directory check passed: {stage}", flush=True)
            stage = label + "-escape-cancels"
            ui.native("key", "Escape")
            try:
                ui.wait_frame(label + "-cancelled", closed)
                report["checks"][stage] = True
            except AssertionError as error:
                # Keep the regression as a hard failure, but use the visible
                # cancel control to collect evidence for the remaining cases.
                report["checks"][stage] = False
                report["failures"].append({"step": stage, "error": str(error)})
                words = picker_words(ui.capture(label + "-escape-failed"), label + "-escape-failed")
                ui.click(phrase_bounds(words, "cancel"))
                ui.wait_frame(label + "-recovery-cancel", closed)
            assert config.read_bytes() == saved_config, "Escape changed the saved preference"

        stage = "super-enter-local-draft-uses-saved-path"
        before = navigation()
        ui.artifact("before-draft-navigation.json").write_text(json.dumps(before, indent=2) + "\n")
        before_ids = {panel["id"] for row in before["rows"] for panel in row["panels"]}
        ui.native("key", "--clearmodifiers", "super+Return")
        deadline = time.monotonic() + 12
        while True:
            state = navigation()
            added = [panel for row in state["rows"] for panel in row["panels"]
                     if panel["id"] not in before_ids]
            if (len(added) == 1 and added[0]["focused"]
                    and state["keyboard_panel"] == added[0]["slot"]):
                break
            assert time.monotonic() < deadline, "Super+Enter did not focus one new draft: " + json.dumps(state)
            time.sleep(.1)
        assert added[0]["session"].startswith("startup://draft/"), added
        assert before_ids.issubset({panel["id"] for row in state["rows"] for panel in row["panels"]}), "New draft replaced an existing panel"
        ui.artifact("draft-navigation.json").write_text(json.dumps(state, indent=2) + "\n")
        # Maximize the focused new panel so its complete path is visible for OCR.
        if added[0]["width"] < .99:
            ui.native("key", "--clearmodifiers", "super+f")

        def draft_path(image):
            words = ui.words(image, (276, 48, image.width - 12, 80), "draft-path", psm=7)
            # Panel headers compact the isolated HOME to ~/. Require the path
            # prefix so the adjacent "Custom Directory" title cannot pass.
            starts = [index for index, word in enumerate(words) if word["text"].startswith("~/")]
            assert len(starts) == 1, "Draft header has no unambiguous home-relative path"
            phrase_bounds(words[starts[0]:], "~/Custom Directory")
        ui.wait_frame("draft-created", draft_path)
        assert pinned() == str(custom), "Draft creation changed the preference"
        report["checks"][stage] = {"panel": added[0], "path": str(custom),
                                  "rendered_path": "~/Custom Directory", "isolated_home": env["HOME"],
                                  "path_evidence": "rendered header OCR with explicit ~/ prefix"}
        stage = "all-native-checks"
        assert not report["failures"], json.dumps(report["failures"])
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, failed_step=stage, error=str(error))
        try:
            ui.capture("failure")
        except Exception as capture_error:
            report["capture_error"] = str(capture_error)
        raise
    finally:
        for source, name in ((config, "final.toml"), (root / "state", "final-state.txt"),
                             (root / "app.log", "app.log"),
                             (root / "logs/jcode-desktop/jcode-desktop.log", "desktop.log")):
            if source.exists():
                shutil.copyfile(source, ui.artifact(name))
        output.with_suffix(".acceptance.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Default-directory native acceptance: " + json.dumps(report))
