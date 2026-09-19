"""Regression checks for the real side-panel acceptance oracle."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("accept_side_panel", Path(__file__).with_name("accept-side-panel.py"))
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


class SidePanelAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.identities = ["session_owner", "side-panel://session_owner/notes"]
        self.state = {
            "rows": [{"panels": [
                {"slot": 0, "session": self.identities[0], "focused": False, "closing": False},
                {"slot": 1, "session": self.identities[1], "focused": True, "closing": False},
            ]}],
            "keyboard_panel": 1, "focused_slot": 1,
            "camera_motion": False, "tab_motion": False,
        }

    def check(self, state):
        return accept.check_state(state, self.identities, self.identities[1])

    def test_accepts_adjacent_document_with_keyboard_focus(self):
        self.assertIs(self.check(self.state), self.state)

    def test_rejects_missing_document(self):
        self.state["rows"][0]["panels"].pop()
        with self.assertRaises(AssertionError):
            self.check(self.state)

    def test_rejects_document_in_other_row(self):
        self.state["rows"].append({"panels": [self.state["rows"][0]["panels"].pop()]})
        with self.assertRaisesRegex(AssertionError, "beside"):
            self.check(self.state)

    def test_rejects_duplicate_or_additional_chat(self):
        self.state["rows"][0]["panels"].append(copy.deepcopy(self.state["rows"][0]["panels"][1]))
        with self.assertRaises(AssertionError):
            self.check(self.state)

    def test_rejects_keyboard_focus_left_in_chat(self):
        self.state["keyboard_panel"] = 0
        with self.assertRaises(AssertionError):
            self.check(self.state)

    def test_rejects_unsettled_motion(self):
        self.state["camera_motion"] = True
        with self.assertRaisesRegex(AssertionError, "Unsettled"):
            self.check(self.state)

    def test_rejects_retiring_document(self):
        self.state["rows"][0]["panels"][1]["closing"] = True
        with self.assertRaises(AssertionError):
            self.check(self.state)

    def test_navigation_reader_tolerates_partial_and_missing_state(self):
        # Tiny test data only. The actual acceptance stores all evidence under
        # the caller's explicit output directory, not a temporary live display.
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "state"
            self.assertIsNone(accept.navigation_state(path))
            path.write_text('navigation={"rows":')
            self.assertIsNone(accept.navigation_state(path))
            path.write_text("strip=0\nnavigation=" + json.dumps(self.state) + "\n")
            self.assertEqual(accept.navigation_state(path), self.state)


if __name__ == "__main__":
    unittest.main()
