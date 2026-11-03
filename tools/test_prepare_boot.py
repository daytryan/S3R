"""prepare_boot's movie audio: 6-channel downmix scaled by the fe_movie volume, and the manifest list."""
import sys
import tempfile
import unittest
import wave
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import prepare_boot  # noqa: E402


class Stereo(unittest.TestCase):
    def test_downmix_and_volume(self):
        with tempfile.TemporaryDirectory() as temp:
            source, target = Path(temp) / "six.wav", Path(temp) / "two.wav"
            # L C R Ls Rs LFE: 1000 on L, 100 on C, 10 on Rs, 5000 on the LFE.
            frame = np.array([1000, 100, 0, 0, 10, 5000], "<i2")
            with wave.open(str(source), "wb") as w:
                w.setnchannels(6)
                w.setsampwidth(2)
                w.setframerate(48000)
                w.writeframes(np.tile(frame, 480).tobytes())
            seconds = prepare_boot._stereo(source, target, 0.6)
            self.assertAlmostEqual(seconds, 0.01)
            with wave.open(str(target)) as w:
                self.assertEqual((w.getnchannels(), w.getframerate(), w.getnframes()), (2, 48000, 480))
                pcm = np.frombuffer(w.readframes(1), "<i2")
            # RwAudioCore fold-down: L = (L + .7071 C + .7071 Ls) * .7071, R likewise; the LFE is dropped; x volume.
            k = prepare_boot.FOLD
            self.assertEqual(list(pcm), [round((1000 + k * 100) * k * 0.6), round(k * 110 * k * 0.6)])

    def test_every_screen_the_game_reads(self):
        names = {Path(rel).stem for rel, _, _ in prepare_boot._manifests()}
        self.assertEqual(names, {"esrb", "pressstart", "welcomescreen", "coachfrank", "team_management_boot",
                                 "fmv_popup", "autosave", "save_warning", "sk8popup", "team_management", "import_skater"})


class Team(unittest.TestCase):
    def test_logos_are_visible_leaves_under_the_root_by_key(self):
        row = lambda key, parent, **fields: {"class": prepare_boot.LOGO_CLASS, "key": key, "parent": parent, "fields": {
            prepare_boot.LOGO_TEXTURE if k == "texture" else prepare_boot.LOGO_HIDDEN: {"data": v} for k, v in fields.items()}}
        rows = [row("default", "", texture="0xdef", hidden="00000000"), row(prepare_boot.LOGO_ROOT, "default"),
                row("Hash_F000000000000001", prepare_boot.LOGO_ROOT, texture="0xb"),
                row("Hash_0000000000000002", prepare_boot.LOGO_ROOT, texture="0xa"),
                row("Hash_0000000000000003", prepare_boot.LOGO_ROOT, texture="0xc", hidden="01000000"),
                row("Hash_0000000000000004", "Hash_0000000000000099", texture="0xd")]
        self.assertEqual(prepare_boot.team_logos(rows), [{"key": "0000000000000002", "texture": "0xa"},
                                                          {"key": "f000000000000001", "texture": "0xb"}])

    def test_preset_profile(self):
        parsed = {"gender": 0, "body_mods": [0.1, 0.2] + [0.25] * 17,
                  "asset_lists": {"Hair": [{"asset_id": "aa", "models": [{"material_id": "m1"}, {"material_id": "m2"}]}],
                                  "Hat": []},
                  "colours": {1: [0.02, 0.01, 0.01] + [0.0] * 7, 2: [0.72, 0.57, 0.49] + [0.0] * 7}}
        profile = prepare_boot.preset_profile(parsed)
        self.assertEqual(profile["gender"], "female")
        self.assertEqual(profile["selections"], {"Hair": {"asset_id": "aa", "material_id": "m1"}})
        self.assertEqual((profile["morphs"]["thin"], profile["morphs"]["fat"], profile["morphs"]["local_nose_width"]), (0.1, 0.2, 0.25))
        self.assertEqual(len(profile["morphs"]), 19)
        self.assertEqual((profile["hair_tint"], profile["skin_tint"]), ([0.02, 0.01, 0.01], [0.72, 0.57, 0.49]))

    def test_photo_setups_carry_their_editor_pose(self):
        f = prepare_boot.PHOTO_FIELDS
        vec = lambda *v: {"data": "".join(__import__("struct").pack(">f", x).hex() for x in v)}
        lens = prepare_boot.PHOTO_LENS
        base = {f["camera"]: vec(0, 2, 0.2), f["target"]: vec(0, 2, 0.1), f["colour"]: vec(1, 0, 0, 0),
                f["width"]: {"data": "00000080"}, f["height"]: {"data": "00000100"},
                lens["fov"]: vec(45), lens["near"]: vec(0.01), lens["far"]: vec(200),
                prepare_boot.PHOTO_LOCATION: {"data": "6A2B5946514B84906B240F01502FF99D"}}
        sh = [vec(0.1 * i, 0, 0)["data"] for i in range(9)]
        rows = [{"class": prepare_boot.CAC_BODY_CLASS, "key": "Hash_00000000000000AB", "parent": "",
                 "fields": {prepare_boot.CAC_BODY_CLIP: {"data": "CAS_Single_Pose_5"}}},
                {"class": prepare_boot.PHOTO_CLASS, "key": "team_profile", "parent": "", "fields": base},
                # The render location: cas_photo's own sun over its parent's SH rows.
                {"class": "Hash_6A2B5946514B8490", "key": "cas", "parent": "",
                 "fields": {"sun_position": vec(1, 1, 1), prepare_boot.LOCATION_SH: {"data": "", "array": {"items": sh}}}},
                {"class": "Hash_6A2B5946514B8490", "key": "cas_photo", "parent": "cas", "fields": {"sun_position": vec(-5, 1, 0)}}]
        rows += [{"class": prepare_boot.PHOTO_CLASS, "key": n, "parent": "team_profile",
                  "fields": {k: {"data": "00000000000000AB0000000000000000"} for k in prepare_boot.PHOTO_POSE_FIELDS}}
                 for n in prepare_boot.PHOTO_SETUPS]
        setups = prepare_boot.photo_setups(rows)
        self.assertEqual(sorted(setups), [f"team_profile_0{i}" for i in range(5)])
        self.assertEqual(setups["team_profile_03"]["pose"], "CAS_Single_Pose_5")
        # EF42 is the render target's width and 2A7D its height (0x8278e960): 128 x 256, portrait.
        self.assertEqual((setups["team_profile_03"]["width"], setups["team_profile_03"]["height"]), (128, 256))
        self.assertEqual((setups["team_profile_03"]["fov"], setups["team_profile_03"]["far"]), (45, 200))
        self.assertEqual(setups["team_profile_03"]["sun"], [-5, 1, 0])
        self.assertAlmostEqual(setups["team_profile_03"]["sh"][3][0], 0.3, places=6)


if __name__ == "__main__":
    unittest.main()
