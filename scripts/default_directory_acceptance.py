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
    # Default size is enforced by screenshot.py. Crop excludes the sidebar label,
    # ensuring a visible modal title cannot be confused with the launcher.
    modal = (370, 210, 1070, 790)

    def pinned():
        return tomllib.loads(config.read_text()).get("workspace", {}).get("pinned_working_dir")

    def modal_words(image, label):
        words = ui.words(image, modal, label)
        phrase_bounds(words, "Default directory")
        # Block OCR sometimes drops the isolated bottom button after a large
        # empty list. Read that rendered control independently, not heuristically.
        footer = ui.words(image, (880, 720, 1055, 775), label + "-footer", psm=7)
        phrase_bounds(footer, "Set as default")
        return [word for word in words if word["y"] <= 720] + footer

    def closed(image):
        # The same title in the sidebar remains visible when the overlay closes.
        phrase_bounds(ui.words(image, (0, 48, 264, 110), stage + "-sidebar"), "Default directory")
        words = ui.words(image, modal, stage + "-closed")
        assert "defaultdirectory" not in normalized(" ".join(w["text"] for w in words)), "Modal stayed open"

    def open_modal(label):
        bounds = ui.wait_frame(label + "-button", lambda image: phrase_bounds(
            ui.words(image, (0, 48, 264, 110), label + "-button"), "Default directory"))
        ui.click(bounds)
        return ui.wait_frame(label, lambda image: modal_words(image, label))

    def type_path(text):
        # Opening focuses the input. Use actual native editing, not app state.
        ui.native("key", "--clearmodifiers", "ctrl+a")
        ui.native("type", "--clearmodifiers", "--delay", "15", text)

    def save_typed(label):
        words = ui.wait_frame(label + "-typed", lambda image: modal_words(image, label + "-typed"))
        # Footer is the last occurrence, since unfiltered rows have the same CTA.
        footer = [word for word in words if word["y"] > 720]
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
            except (StopIteration, json.JSONDecodeError):
                # The opt-in state file is rewritten during painting.
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.05)

    try:
        stage = "open-and-escape-cancels"
        words = open_modal("modal-open")
        phrase_bounds(words, "Choose a frequent folder")
        shutil.copyfile(ui.artifact("modal-open.png"), output)
        type_path("~/Custom Directory")
        ui.native("key", "Escape")
        ui.wait_frame("escape-cancelled", closed)
        assert config.read_bytes() == original_config, "Escape persisted an unsaved path"
        report["checks"][stage] = True
        print(f"Default-directory check passed: {stage}", flush=True)

        stage = "frequent-row-saves-immediately"
        words = open_modal("frequent-open")
        # The path label is in the list, below the search field, not the header.
        row_words = [word for word in words if 410 < word["y"] < 710]
        matches = [word for word in row_words
                   if normalized(word["text"]) in ("frequent", normalized(str(frequent)))]
        assert len(matches) == 1, "Frequent directory row missing: " + " ".join(w["text"] for w in row_words)
        word = matches[0]
        ui.click((word["x"], word["y"], word["x"] + word["width"], word["y"] + word["height"]))
        assert_saved(frequent, "frequent-saved")

        stage = "typed-custom-path-persists"
        open_modal("custom-open")
        type_path("~/Custom Directory")
        save_typed("custom")
        assert_saved(custom, "custom-saved")
        saved_config = config.read_bytes()

        for label, value in (("regular-file", invalid_file), ("missing-path", missing)):
            stage = label + "-rejected"
            open_modal(label + "-open")
            type_path(str(value))
            save_typed(label)

            def rejected(image):
                words = modal_words(image, label + "-error")
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
                words = modal_words(ui.capture(label + "-escape-failed"), label + "-escape-failed")
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
