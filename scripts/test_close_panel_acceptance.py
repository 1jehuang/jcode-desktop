import copy
import unittest

from close_panel_acceptance import assert_live_focus, panels


def navigation():
    return {"active_row": 0, "focused_slot": 0, "keyboard_panel": 0,
            "rows": [{"panels": [
                {"slot": 0, "id": 10, "closing": False, "focused": True},
                {"slot": 1, "id": 11, "closing": True, "focused": False},
            ]}]}


class ClosePanelAcceptanceTests(unittest.TestCase):
    def test_live_focus_with_fading_neighbor_is_valid(self):
        self.assertEqual([p["id"] for p in assert_live_focus(navigation())], [10])

    def test_fading_selection_or_keyboard_focus_is_rejected(self):
        for field in ("focused_slot", "keyboard_panel"):
            with self.subTest(field=field):
                nav = navigation()
                nav[field] = 1
                with self.assertRaises(AssertionError):
                    assert_live_focus(nav)

    def test_final_close_requires_composer_focus_released(self):
        nav = navigation()
        nav["rows"][0]["panels"][0]["closing"] = True
        with self.assertRaises(AssertionError):
            assert_live_focus(nav)
        nav["keyboard_panel"] = None
        self.assertEqual(assert_live_focus(nav), [])

    def test_retired_slots_can_renumber_but_identity_survives(self):
        nav = navigation()
        survivor = copy.deepcopy(nav["rows"][0]["panels"][0])
        nav["rows"][0]["panels"] = [survivor]
        self.assertEqual([p["id"] for p in panels(nav)], [10])
        self.assertEqual(assert_live_focus(nav), [survivor])

    def test_empty_workspace_has_no_composer(self):
        nav = navigation()
        nav.update(focused_slot=None, keyboard_panel=None)
        nav["rows"][0]["panels"] = []
        self.assertEqual(assert_live_focus(nav), [])


if __name__ == "__main__":
    unittest.main()
