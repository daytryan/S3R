"""py -3 -m unittest tools.test_skate3_parkassets (from the repo root). The archive tests need SKATE3_DISC, the extracted
Skate 3 disc folder (read only); the rest use a small recipe in memory."""
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
import skate3_parkassets as pa  # noqa: E402

XML = b"""<?xml version="1.0" standalone="yes" ?>
<!--Skate Park Recipe v1.0-->
<compositeasset n="static_db" type="Static" schemaversion="1.0">
    <mat id="0xm1" type="environmentParkDiffuse"><ud n="cap" /><sp id="0xt1" chn="diffuse" /></mat>
    <comp n="street">
        <mod id="0x0001dc3503e38703" n="aa_padStreet_sm_University">
            <ud n="cap"><cap.IsColorizable value="" /><cap.Unlock value="skatepark_unlock_startpark" /></ud>
            <lod idx="0" id="0xl0" arenaid="0x00021c6b03e38707"><matinst><matvar id="0xm1" n="sur" /></matinst></lod>
        </mod>
    </comp>
</compositeasset>
"""
MODELS = {"0x00021c6b03e38707": "data/content/static/model/static/street/0x00021c6b03e38707.rx2"}
TEXTURES = {("dynamic", "0xt1"): "data/content/dynamic/texture/0xt1.rx2"}


class Recipe(unittest.TestCase):
    def test_a_mod_resolves_model_material_and_texture(self):
        mod = pa.recipe(XML, "static", MODELS, TEXTURES)["0x0001dc3503e38703"]
        self.assertEqual((mod["comp"], mod["caps"]), ("street", {"Unlock": "skatepark_unlock_startpark"}))
        lod = mod["lods"][0]
        self.assertEqual(lod["model"], MODELS["0x00021c6b03e38707"])
        self.assertEqual(lod["materials"][0]["textures"], {"diffuse": "data/content/dynamic/texture/0xt1.rx2"})

    def test_missing_pieces_refuse(self):
        for models, textures, xml in (({}, TEXTURES, XML), (MODELS, {}, XML), (MODELS, TEXTURES, XML.replace(b'matvar id="0xm1"', b'matvar id="0xm2"'))):
            with self.assertRaises(pa.Refused) as e:
                pa.recipe(xml, "static", models, textures)
            self.assertEqual(e.exception.reason, "missing")

    def test_not_a_recipe_refuses(self):
        with self.assertRaises(pa.Refused):
            pa.recipe(XML.replace(b"Skate Park Recipe v1.0", b"something else"), "static", MODELS, TEXTURES)

    def test_out_inside_the_repo_refuses(self):
        archive = SimpleNamespace(entries=[])
        with self.assertRaises(pa.Refused) as e:
            pa.export(archive, pa.REPO / "tmp-parkassets")
        self.assertEqual(e.exception.reason, "out")


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class Disc(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.archive = pa.BigArchive(Path(os.environ["SKATE3_DISC"]) / pa.ARCHIVE)
        cls.mods = pa.catalogue(cls.archive)

    def test_catalogue(self):
        self.assertEqual(len(self.archive.entries), 1296)
        self.assertEqual(sum(m["db"] == "static" for m in self.mods.values()), 542)
        self.assertEqual(sum(m["db"] == "dynamic" for m in self.mods.values()), 119)

    def test_progression_pieces(self):
        # stage2_park.md Q2: piece id -> model arenaid for two of the 15 progression kinds
        self.assertEqual(self.mods["0x2c70170300030006"]["lods"][0]["model"],
                         "data/content/static/model/static/hubbas/0x2c70170700030006.rx2")
        self.assertEqual(self.mods["0x0001dc9803e38703"]["name"], "oi_quarterpipe_lrg_University")

    def test_export_writes_outside_the_repo(self):
        with tempfile.TemporaryDirectory() as tmp:
            s = pa.export(self.archive, tmp)
            self.assertEqual(s["mods"], 661)
            self.assertTrue((Path(tmp) / "recipe.json").exists())
            self.assertTrue((Path(tmp) / "parkassets/data/content/static/model/static/spawner/0x2c70170700030210.rx2").exists())


if __name__ == "__main__":
    unittest.main()
