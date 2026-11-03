import unittest
import prepare_online_ui as ui

class OnlineUiTest(unittest.TestCase):
    def test_movies_and_folder(self):
        self.assertEqual(ui.FOLDER, "online-ui")
        self.assertEqual(sorted(ui.MOVIES), ["friends", "hud_online_eventlog2", "leaderboardhud"])
        self.assertTrue(all(p.startswith("data/fe/source/screens/") for p in ui.MOVIES.values()))
        self.assertEqual(ui.manifest("friends"), "runtime/friends.json")
        self.assertEqual(ui.POPUP_MANIFEST, "runtime/sk8popup.json")

    def test_shadow_only_movie_gets_futuraheavy(self):
        mappings = {"Futura Shadow": {"file_name": "futurashadow"}, "Futura Std Medium": {"file_name": "futuraheavy"}}
        shadow = {"id": 1, "type_name": "font", "font": {"name": "Futura Shadow"}}
        result = {"characters": [{"id": 0, "type_name": "movie"}, shadow], "fonts": {"Futura Shadow": {}},
                  "font_mappings": mappings}
        ui.shadow_foreground(result, lambda family: {"family": family})
        self.assertEqual(result["characters"][-1]["id"], 2)
        self.assertEqual(result["characters"][-1]["font"]["name"], "Futura Std Medium")
        self.assertEqual(result["fonts"]["Futura Std Medium"], {"family": "Futura Std Medium"})
        ui.shadow_foreground(result, lambda family: self.fail("already has futuraheavy"))
        self.assertEqual(len(result["characters"]), 3)

if __name__ == "__main__":
    unittest.main()
