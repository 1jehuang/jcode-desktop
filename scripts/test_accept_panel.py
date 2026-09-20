"""Regression checks for the real panel acceptance oracle."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("accept_panel", Path(__file__).with_name("accept-panel.py"))
accept = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accept)


class PanelAcceptanceTests(unittest.TestCase):
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


class PanelToolContractTests(unittest.TestCase):
    def result(self, output="panel_id: unique\nidentity: side-panel://owner/unique\nStatus"):
        return {"output": output, "metadata": {"focused_page_id": "unique", "pages": [{"id": "unique"}]}}

    def test_actual_debug_envelope(self):
        payload = accept.tool_payload(self.result())
        self.assertEqual(accept.spawn_identity(payload, "owner"), ("unique", "side-panel://owner/unique"))
        accept.check_listing(payload, ["unique"])

    def test_rejects_wrong_owner_and_missing_identity(self):
        payload = accept.tool_payload(self.result())
        with self.assertRaises(AssertionError):
            accept.spawn_identity(payload, "other")
        del payload["identity"]
        with self.assertRaises(AssertionError):
            accept.spawn_identity(payload, "owner")

    def test_rejects_reused_spawn_identity(self):
        payload = accept.tool_payload(self.result())
        with self.assertRaisesRegex(AssertionError, "reused"):
            accept.spawn_identity(payload, "owner", ["side-panel://owner/unique"])

    def test_ignores_identity_embedded_in_user_text(self):
        payload = accept.tool_payload(self.result("Status\nDocument:\npanel_id: fake\nidentity: fake"))
        self.assertNotIn("panel_id", payload)

    def test_rejects_error_and_missing_snapshot(self):
        for result in ({"is_error": True}, {"output": "hello"}, {"content": "{}"}):
            with self.subTest(result=result), self.assertRaises(AssertionError):
                accept.tool_payload(result)

    def test_list_empty_after_last_close(self):
        payload = accept.tool_payload({"output": "No panels", "metadata": {"pages": []}})
        accept.check_listing(payload, [])
        with self.assertRaises(AssertionError):
            accept.check_listing(payload, ["unique"])

    def test_empty_snapshot_omits_pages_under_serde(self):
        payload = accept.tool_payload({"output": "Side panel: empty", "metadata": {
            "focused_page_id": None, "focus_revision": 0}})
        accept.check_listing(payload, [])

    def test_rejects_duplicate_and_stale_persisted_panels(self):
        for pages in ([{"id": "unique"}, {"id": "unique"}], [{"id": "stale"}]):
            with self.subTest(pages=pages), self.assertRaises(AssertionError):
                accept.check_listing({"panels": pages}, ["unique"])

    def test_update_keeps_entity(self):
        state = {"rows": [{"panels": [{"session": "doc", "id": 42}]}]}
        accept.same_entities(state, copy.deepcopy(state))
        after = copy.deepcopy(state)
        after["rows"][0]["panels"][0]["id"] = 43
        with self.assertRaises(AssertionError):
            accept.same_entities(state, after)
        after = copy.deepcopy(state)
        after["rows"][0]["panels"] *= 2
        with self.assertRaisesRegex(AssertionError, "Duplicate"):
            accept.same_entities(state, after)


class PdfPixelOracleTests(unittest.TestCase):
    def tsv(self, words):
        header = "left\ttop\twidth\theight\tconf\ttext\n"
        return header + "".join(f"100\t200\t40\t20\t{confidence}\t{word}\n" for word, confidence in words)

    def test_click_center_from_unique_visible_button(self):
        self.assertEqual(accept.ocr_button_center(self.tsv([("Next", 95)]), "Next"), (120, 210))

    def test_ocr_spurious_trailing_period_on_button(self):
        self.assertEqual(accept.ocr_button_center(self.tsv([("Next.", 38.9)]), "Next"), (120, 210))

    def test_no_guessing_missing_ambiguous_or_uncertain_buttons(self):
        for words in ([], [("Next", 95), ("Next", 95)], [("Next", 20)], [("Nextish", 99)]):
            with self.subTest(words=words), self.assertRaises(AssertionError):
                accept.ocr_button_center(self.tsv(words), "Next")

    def test_zoom_symbols_scoped_to_pdf_toolbar(self):
        tsv = ("left\ttop\twidth\theight\tconf\ttext\n"
               "1700\t20\t8\t8\t90\t+\n"
               "1300\t142\t20\t10\t90\tFit\n"
               "1390\t144\t6\t6\t90\t+\n")
        self.assertEqual(accept.ocr_button_center(tsv, "+", anchor="Fit"), (1393, 147))
        with self.assertRaises(AssertionError):
            accept.ocr_button_center(tsv, "+")

    def test_ocr_literal_quote_does_not_consume_following_rows(self):
        self.assertEqual(accept.ocr_button_center(self.tsv([('"', 90), ("Fit", 90)]), "Fit"), (120, 210))

    def test_unicode_minus_normalization(self):
        self.assertEqual(accept.ocr_button_center(self.tsv([("−", 95)]), "-"), (120, 210))

    def test_error_pixels_require_message_and_no_stale_page_text(self):
        expected = ("Cannot display this PDF",)
        stale = ("PDF panel acceptance", "Second PDF page")
        self.assertTrue(accept.rendered_text_matches("Cannot display\nthis PDF", expected, stale))
        self.assertFalse(accept.rendered_text_matches("Loading PDF", expected, stale))
        for old in stale:
            with self.subTest(old=old):
                self.assertFalse(accept.rendered_text_matches(
                    "Cannot display this PDF\n" + old, expected, stale))

    def test_zoom_requires_single_visible_status(self):
        self.assertEqual(accept.zoom_percent("Page 1 of 2\n100%"), 100)
        self.assertEqual(accept.zoom_percent("125 %"), 125)
        for text in ("no zoom", "100% 125%"):
            with self.subTest(text=text), self.assertRaises(AssertionError):
                accept.zoom_percent(text)


if __name__ == "__main__":
    unittest.main()
