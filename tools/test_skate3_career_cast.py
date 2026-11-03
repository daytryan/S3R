"""Tests for tools/skate3_career_cast.py (Task 9 of the Skate 3 story-mode plan): the stage-1 cast as scene-rig GLBs.

The disc tests need SKATE3_DISC (the extracted Skate 3 disc) and SKATE3_ASSETS (the install's assets folder, read only: its
private/stock/data/anim/OnBoard.abin is the skater rig every character is converted on). They convert all eleven models, one at a time.
The other tests are synthetic and need nothing.
"""
import json
import os
import random
import struct
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

import numpy as np

import skate3_career as career
import skate3_career_cast as cast
from test_skate3_career import CareerFake, Scripted, bank, minimal_world

DISC = os.environ.get("SKATE3_DISC")
ASSETS = os.environ.get("SKATE3_ASSETS")
needs_disc = unittest.skipUnless(DISC and ASSETS, "set SKATE3_DISC (the extracted disc) and SKATE3_ASSETS (the install's assets folder, read only)")

# spec 15.2: 133 bones for the scene actors with the two prop bones, 131 for the skater banks' rig, 50 for the pedestrians.
JOINTS = {
    "benny_fairfax": 133, "giovanni_reda": 133, "giovanni_reda_prop": 133, "shingo": 133, "shingo_prop": 133,
    "ai_skater_03": 133, "ai_skater_04": 133,
    "player_skater": 131,
    "female_adult_2": 50, "male_adult_1": 50, "male_adult_2": 50,
}


def glb(joints, positions=((0, 0, 0), (1, 0, 0), (0, 1, 0))):
    """A minimal GLB: one skin with the named joints and one mesh primitive of the given positions."""
    pos = np.asarray(positions, dtype="<f4").tobytes()
    doc = {"asset": {"version": "2.0"}, "buffers": [{"byteLength": len(pos)}],
           "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": len(pos)}],
           "accessors": [{"bufferView": 0, "componentType": 5126, "count": len(positions), "type": "VEC3"}],
           "nodes": [{"name": n} for n in joints], "skins": [{"joints": list(range(len(joints)))}],
           "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}]}
    text = json.dumps(doc).encode()
    text += b" " * (-len(text) % 4)
    return (struct.pack("<III", 0x46546C67, 2, 28 + len(text) + len(pos)) + struct.pack("<I4s", len(text), b"JSON") + text
            + struct.pack("<I4s", len(pos), b"BIN\0") + pos)


class GlbReadersTest(unittest.TestCase):
    def test_joint_names_are_the_skin_s_node_names_in_order(self):
        self.assertEqual(cast.joint_names(glb(["HIPS", "SPINE", "HEAD"])), ["HIPS", "SPINE", "HEAD"])

    def test_the_mesh_digest_follows_the_vertices_not_the_rig(self):
        a, b = glb(["A"]), glb(["B"])
        self.assertEqual(cast.mesh_digest(a), cast.mesh_digest(b))
        self.assertNotEqual(cast.mesh_digest(a), cast.mesh_digest(glb(["A"], positions=((0, 0, 0), (1, 0, 0), (0, 2, 0)))))

    def test_a_file_that_is_not_a_glb_is_refused(self):
        with self.assertRaises(cast.CastFailed):
            cast.joint_names(b"not a glb at all, but long enough to read a header from")


class QuaternionTest(unittest.TestCase):
    def test_a_matrix_makes_its_quaternion_back(self):
        from tools.asset_pipeline.retail_character import quat_matrix
        rng = random.Random(7)
        for _ in range(50):
            q = np.array([rng.uniform(-1, 1) for _ in range(4)])
            q /= np.linalg.norm(q)
            m = quat_matrix(tuple(q))
            got = np.array(cast.matrix_quat(m))
            self.assertTrue(np.allclose(quat_matrix(tuple(got)), m, atol=1e-9), (q, got))


def rig(names, parents):
    return cast.Rig(names=list(names), parents=list(parents))


def translated(x, y, z):
    m = np.identity(4)
    m[3, :3] = (x, y, z)
    return m


class SynthesisTest(unittest.TestCase):
    """A bank bone no model file binds (FACE, OFFSET_*, TRAJECTORY, the props) is bound where its parent is."""

    def test_a_missing_bone_binds_at_its_parent(self):
        r = rig(["ROOT", "HIPS", "FACE", "OFFSET_JAW"], [-1, 0, 1, 2])
        bind = {"HIPS": translated(5, 0, 0), "OFFSET_JAW": translated(5, 1, 2)}
        cast.synthesise(bind, r)
        self.assertTrue(np.allclose(bind["FACE"], translated(5, 0, 0)))
        self.assertTrue(np.allclose(bind["ROOT"], np.identity(4)))          # a root stands at the origin

    def test_a_bone_the_model_binds_is_left_alone(self):
        r = rig(["ROOT", "HIPS"], [-1, 0])
        bind = {"HIPS": translated(5, 0, 0)}
        cast.synthesise(bind, r)
        self.assertTrue(np.allclose(bind["HIPS"], translated(5, 0, 0)))

    def test_a_synthesised_bind_does_not_alias_its_parent_s(self):
        bind = {"HIPS": translated(5, 0, 0)}
        cast.synthesise(bind, rig(["HIPS", "FACE"], [-1, 0]))
        bind["FACE"][3, 0] = 9
        self.assertEqual(bind["HIPS"][3, 0], 5)

    def test_a_bank_that_lists_a_child_before_its_parent_is_refused(self):
        with self.assertRaises(cast.CastFailed):
            cast.synthesise({}, rig(["CHILD", "PARENT"], [1, -1]))


def rigged(names, parents, worlds):
    """A GLB the way character_glb writes one: joint nodes with local matrices (row-vector locals: world = local @ parent's world),
    inverse binds, a mesh node and a root node over the parentless joints. `worlds` is {name: 4x4}."""
    nodes, ibm = [], []
    for name, parent in zip(names, parents):
        world = worlds[name]
        local = world @ np.linalg.inv(worlds[names[parent]]) if parent >= 0 else world
        nodes.append({"name": name, "matrix": local.ravel().tolist()})
        ibm.append(np.linalg.inv(world).ravel())
    for index, parent in enumerate(parents):
        if parent >= 0:
            nodes[parent].setdefault("children", []).append(index)
    mesh_node = len(nodes)
    nodes.append({"name": "mesh", "mesh": 0, "skin": 0})
    nodes.append({"name": "root", "children": [i for i, p in enumerate(parents) if p < 0] + [mesh_node]})
    binary = np.asarray(ibm, dtype="<f4").tobytes() + np.asarray(((0, 0, 0), (1, 0, 0), (0, 1, 0)), dtype="<f4").tobytes()
    doc = {"asset": {"version": "2.0"}, "buffers": [{"byteLength": len(binary)}],
           "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 64 * len(names)},
                           {"buffer": 0, "byteOffset": 64 * len(names), "byteLength": 36}],
           "accessors": [{"bufferView": 0, "componentType": 5126, "count": len(names), "type": "MAT4"},
                         {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}],
           "nodes": nodes, "skins": [{"joints": list(range(len(names))), "inverseBindMatrices": 0}],
           "meshes": [{"primitives": [{"attributes": {"POSITION": 1}}]}], "scenes": [{"nodes": [mesh_node + 1]}], "scene": 0}
    return repack(doc, binary)


def repack(doc, binary):
    text = json.dumps(doc).encode()
    text += b" " * (-len(text) % 4)
    return (struct.pack("<III", 0x46546C67, 2, 28 + len(text) + len(binary)) + struct.pack("<I4s", len(text), b"JSON") + text
            + struct.pack("<I4s", len(binary), b"BIN\0") + binary)


class RehangTest(unittest.TestCase):
    """The converter hangs a joint where the skater's rig or the model's file puts it; the scene bank's clips are authored against the
    bank's own hierarchy, so the GLB takes that, the bind pose unchanged."""
    NAMES = ["HIPS", "SPINE", "HEAD", "JAW"]
    WORLDS = {"HIPS": translated(0, 1, 0), "SPINE": translated(0, 2, 0), "HEAD": translated(0, 3, 1), "JAW": translated(1, 3, 2)}

    def test_a_joint_takes_the_bank_s_parent_and_keeps_its_world(self):
        data = rigged(self.NAMES, [-1, 0, 1, 2], self.WORLDS)
        bank = rig(["HIPS", "SPINE", "HEAD", "JAW"], [-1, 0, 0, 1])      # JAW under SPINE, HEAD under HIPS
        out = cast.rehang(data, bank)
        self.assertEqual(cast.joint_parents(out), {"HIPS": None, "SPINE": "HIPS", "HEAD": "HIPS", "JAW": "SPINE"})
        self.assertLess(cast.rest_error(out), 1e-4)
        self.assertEqual(cast.joint_names(out), self.NAMES)

    def test_a_bank_root_hangs_from_the_scene_root_beside_the_mesh(self):
        data = rigged(self.NAMES, [-1, 0, 1, 2], self.WORLDS)
        out = cast.rehang(data, rig(self.NAMES, [-1, -1, 0, 1]))
        doc, _ = cast.glb_json(out)
        root = next(n for n in doc["nodes"] if n["name"] == "root")
        self.assertEqual(sorted(doc["nodes"][c]["name"] for c in root["children"]), ["HIPS", "SPINE", "mesh"])
        self.assertLess(cast.rest_error(out), 1e-4)

    def test_a_joint_the_bank_lacks_is_refused(self):
        with self.assertRaises(cast.CastFailed):
            cast.rehang(rigged(self.NAMES, [-1, 0, 1, 2], self.WORLDS), rig(["HIPS", "SPINE"], [-1, 0]))


class RestErrorTest(unittest.TestCase):
    def test_a_rig_whose_nodes_give_the_inverse_binds_has_no_error(self):
        self.assertLess(cast.rest_error(rigged(RehangTest.NAMES, [-1, 0, 1, 2], RehangTest.WORLDS)), 1e-4)

    def test_a_node_that_moved_off_its_bind_is_an_error(self):
        doc, binary = cast.glb_json(rigged(RehangTest.NAMES, [-1, 0, 1, 2], RehangTest.WORLDS))
        doc["nodes"][2]["matrix"][13] += 0.5
        self.assertGreater(cast.rest_error(repack(doc, binary)), 0.4)


class PatchTest(unittest.TestCase):
    def test_the_converters_globals_are_put_back_even_when_the_body_fails(self):
        from tools.asset_pipeline import character_glb, native_roster
        before = (native_roster.BigArchive, native_roster.Resources, native_roster.convert, character_glb.SkeletonSet,
                  character_glb.ABIN.BONE_NAMES)
        with self.assertRaises(RuntimeError):
            with cast.patched(archive=object(), resources=object, skeleton=object, convert={}, bone_names=["X"]):
                self.assertIs(native_roster.Resources, object)
                self.assertEqual(character_glb.ABIN.BONE_NAMES, ["X"])
                raise RuntimeError("boom")
        self.assertEqual(before, (native_roster.BigArchive, native_roster.Resources, native_roster.convert, character_glb.SkeletonSet,
                                  character_glb.ABIN.BONE_NAMES))


class LivingResourcesTest(unittest.TestCase):
    """Pedestrians are livingworld.big's: the recipe and model folders are the same layout as marquee's under another name."""

    def archive(self, *paths):
        entries = [SimpleNamespace(path=p) for p in paths]
        return SimpleNamespace(entries=entries, read=lambda e: e.path.encode())

    def test_marquee_paths_resolve_to_livingworld_ones(self):
        res = cast.living_resources()(self.archive("data/content/recipe/livingworld/male_adult_1.xml",
                                                     "data/content/livingworld/model/male_adult_1/rostral/0x000013d703e38811.rx2",
                                                     "data/content/livingworld/texture/0x00001f8103e38818.rx2"))
        self.assertEqual(res.read("data/content/recipe/marquee/male_adult_1.xml"), b"data/content/recipe/livingworld/male_adult_1.xml")
        self.assertEqual(res.read("data/content/marquee/model/male_adult_1/Rostral/0x000013d703e38811.rx2"),
                         b"data/content/livingworld/model/male_adult_1/rostral/0x000013d703e38811.rx2")
        self.assertEqual(res.read("data/content/marquee/texture/0x00001f8103e38818.rx2"),
                         b"data/content/livingworld/texture/0x00001f8103e38818.rx2")


class SpareSlotsTest(unittest.TestCase):
    """character_glb.convert renames its base joints 32..35 and binds them at the reference pose; the pedestrian rig puts four joints that skin
    nothing there, and the converter's names for them are renamed back."""
    SPARE = ["L0", "L1", "L2", "L3"]

    def bank(self):
        """A chain B0..B39 and four leaves (L0..L3) hung from B3, B4, B5 and B6, listed after it."""
        names = [f"B{i}" for i in range(40)] + self.SPARE
        parents = [-1] + list(range(39)) + [3, 4, 5, 6]
        return rig(names, parents)

    def test_the_spare_bones_take_slots_32_to_35_and_the_rest_keep_their_order(self):
        old = self.bank()
        got = cast.in_spare_slots(old, self.SPARE)
        self.assertEqual(got.names[32:36], self.SPARE)
        rest = [n for n in old.names if n not in self.SPARE]
        self.assertEqual(got.names[:32], rest[:32])
        self.assertEqual(got.names[36:], rest[32:])
        for i, name in enumerate(got.names):          # a bone keeps its parent, and its parent comes first
            was = old.names.index(name)
            self.assertEqual(got.names[got.parents[i]] if got.parents[i] >= 0 else None,
                             old.names[old.parents[was]] if old.parents[was] >= 0 else None)
            self.assertLess(got.parents[i], i)

    def test_a_spare_bone_with_a_child_or_a_late_parent_or_not_in_the_rig_is_refused(self):
        old = self.bank()
        with self.assertRaises(cast.CastFailed):
            cast.in_spare_slots(old, ["B5", "L1", "L2", "L3"])            # B5 has a child
        with self.assertRaises(cast.CastFailed):
            cast.in_spare_slots(old, ["L0", "L1", "L2", "NOPE"])         # not a bone of the rig
        late = rig(old.names, old.parents[:-1] + [38])        # L3 hangs from B38, which is listed after slot 35
        with self.assertRaises(cast.CastFailed):
            cast.in_spare_slots(late, self.SPARE)


class SkinnedJointsTest(unittest.TestCase):
    def skinned(self, joints, weights, names=("A", "B", "C")):
        j = np.asarray(joints, dtype="<u2").tobytes()
        w = np.asarray(weights, dtype="<f4").tobytes()
        doc = {"asset": {"version": "2.0"}, "nodes": [{"name": n} for n in names], "skins": [{"joints": list(range(len(names)))}],
               "buffers": [{"byteLength": len(j) + len(w)}],
               "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": len(j)}, {"buffer": 0, "byteOffset": len(j), "byteLength": len(w)}],
               "accessors": [{"bufferView": 0, "componentType": 5123, "count": len(joints), "type": "VEC4"},
                             {"bufferView": 1, "componentType": 5126, "count": len(joints), "type": "VEC4"}],
               "meshes": [{"primitives": [{"attributes": {"JOINTS_0": 0, "WEIGHTS_0": 1}}]}]}
        return repack(doc, j + w)

    def test_a_joint_is_skinned_where_a_vertex_gives_it_weight(self):
        data = self.skinned([[0, 1, 0, 0], [1, 2, 0, 0]], [[0.5, 0.5, 0, 0], [1, 0, 0, 0]])
        self.assertEqual(cast.skinned_joints(data), {"A", "B"})

    def test_a_joint_with_only_zero_weights_is_not(self):
        data = self.skinned([[0, 1, 2, 0]], [[0.7, 0.3, 0, 0]])
        self.assertEqual(cast.skinned_joints(data), {"A", "B"})


class CutMapTest(unittest.TestCase):
    """A pedestrian's hair is cut by an alpha map of twice its diffuse map's size; native_roster puts the one in the other's alpha channel."""
    XML = (b'<c><mat id="0x1" type="marquee_hair"><sp id="0x00001f3a03e38818" chn="diffuse"/><sp id="0x00001f3b03e38818" chn="normal"/>'
           b'<sp id="0x00001f3c03e38818" chn="alpha"/></mat><mat id="0x2"><sp id="0x00001f3d03e38818" chn="diffuse"/></mat></c>')

    def test_the_recipe_names_each_cut_map_and_its_diffuse(self):
        self.assertEqual(cast.cut_maps(self.XML), {"00001f3c03e38818": "00001f3a03e38818"})

    def test_a_cut_map_is_resized_to_its_diffuse_and_other_maps_are_left_alone(self):
        from PIL import Image
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            Image.new("RGBA", (128, 128)).save(d / "00001f3a03e38818.png")

            def decode(parser, src, dest):
                Image.new("RGBA", (256, 256), (0, 0, 0, 255)).save(dest)

            wrapped = cast.cut_map_decoder(decode, cast.cut_maps(self.XML))
            wrapped(None, "src", d / "00001f3c03e38818.png")
            wrapped(None, "src", d / "00001f3b03e38818.png")
            for name, size in (("00001f3c03e38818.png", (128, 128)), ("00001f3b03e38818.png", (256, 256))):
                with Image.open(d / name) as picture:
                    self.assertEqual(picture.size, size, name)


class ConvertOneTest(unittest.TestCase):
    """A conversion that fails or loses a bone is a CastFailed, never a short GLB written."""

    def test_a_failed_conversion_is_a_failure_naming_the_character(self):
        with mock.patch.object(cast, "build_model", side_effect=ValueError("no mesh")):
            with tempfile.TemporaryDirectory() as d, self.assertRaises(cast.CastFailed) as ctx:
                cast.cast_export(Path(d), Path(d), Path(d) / "w", keys=["benny_fairfax"], rigs={"benny_fairfax": rig(["A"], [-1])})
        self.assertIn("benny_fairfax", str(ctx.exception))

    def test_a_model_with_a_joint_the_bank_lacks_or_without_one_it_has_is_a_failure(self):
        r = rig(["HIPS", "SPINE"], [-1, 0])
        for names in (["HIPS"], ["HIPS", "SPINE", "EXTRA"]):
            with self.subTest(names=names), mock.patch.object(cast, "build_model", return_value=glb(names)):
                with tempfile.TemporaryDirectory() as d, self.assertRaises(cast.CastFailed):
                    cast.cast_export(Path(d), Path(d), Path(d) / "w", keys=["benny_fairfax"], rigs={"benny_fairfax": r})

    def test_a_good_model_lands_under_cast_and_is_counted(self):
        r = rig(["HIPS", "SPINE"], [-1, 0])
        with mock.patch.object(cast, "build_model", return_value=glb(["HIPS", "SPINE"])):
            with tempfile.TemporaryDirectory() as d:
                files, summary = cast.cast_export(Path(d), Path(d), Path(d) / "w", keys=["benny_fairfax"], rigs={"benny_fairfax": r})
                self.assertEqual(list(files), ["cast/benny_fairfax.glb"])
                self.assertEqual(cast.joint_names(Path(files["cast/benny_fairfax.glb"]).read_bytes()), ["HIPS", "SPINE"])
                self.assertEqual(summary["joints"], {"benny_fairfax": 2})
                self.assertEqual(summary["models"], 1)

    def test_an_unknown_key_is_refused(self):
        with tempfile.TemporaryDirectory() as d, self.assertRaises(cast.CastFailed):
            cast.cast_export(Path(d), Path(d), Path(d) / "w", keys=["nobody"], rigs={})


class TableTest(unittest.TestCase):
    def test_the_table_names_the_spec_15_2_cast(self):
        self.assertEqual(set(cast.CAST), set(JOINTS))

    def test_every_member_is_a_marquee_or_livingworld_recipe_with_a_scene_bank(self):
        for key, member in cast.CAST.items():
            self.assertIn(member.package, ("marquee", "livingworld"), key)
            self.assertTrue(member.bank, key)

    def test_the_props_and_the_plain_models_are_different_recipes(self):
        self.assertNotEqual(cast.CAST["shingo"].recipe, cast.CAST["shingo_prop"].recipe)
        self.assertNotEqual(cast.CAST["giovanni_reda"].recipe, cast.CAST["giovanni_reda_prop"].recipe)


class ExportCastTest(Scripted):
    """The cast part joins skate3_career.export's atomic write, and a track with no model refuses."""

    def world(self):
        return CareerFake(minimal_world(), bank())

    def part(self, files, summary=None):
        return lambda work: (files, summary or {"models": len(files)})

    def test_export_writes_the_cast_beside_the_career_and_reports_it(self):
        with tempfile.TemporaryDirectory() as d:
            model = Path(d) / "a.glb"
            model.write_bytes(glb(["HIPS"]))
            out = Path(d) / "out"
            summary = career.export(self.world(), out, cast=self.part({"cast/benny_fairfax.glb": model}, {"models": 1}))
            self.assertEqual(summary["cast"], {"models": 1})
            self.assertEqual((out / "cast/benny_fairfax.glb").read_bytes(), model.read_bytes())
            self.assertTrue((out / "career.json").exists())

    def test_a_failed_cast_part_leaves_the_folder_as_it_was(self):
        def failing(work):
            raise career.Refused("conversion", "benny_fairfax", "no mesh")
        with tempfile.TemporaryDirectory() as d:
            career.write(Path(d), {"a": 1})
            with self.assertRaises(career.Refused):
                career.export(self.world(), Path(d), cast=failing)
            self.assertEqual([p.name for p in Path(d).iterdir()], ["career.json"])

    def test_without_a_cast_part_no_cast_folder_is_written(self):
        with tempfile.TemporaryDirectory() as d:
            summary = career.export(self.world(), Path(d))
            self.assertFalse((Path(d) / "cast").exists())
            self.assertIsNone(summary["cast"])

    def test_cast_failures_become_refusals(self):
        with mock.patch.object(cast, "cast_export", side_effect=cast.CastFailed("benny_fairfax", "no mesh")):
            with self.assertRaises(career.Refused) as ctx:
                career.cast_part(Path("d"), Path("a"), Path("w"))
        self.assertEqual(ctx.exception.reason, "conversion")
        self.assertIn("benny_fairfax", str(ctx.exception))

    def test_a_track_with_no_model_refuses(self):
        tutorial = {"scenes": {"s": {"tracks": [{"character": "current_player"}, {"character": "shingo"}, {"character": "ghost"}]}}}
        with self.assertRaises(career.Refused) as ctx:
            career.check_cast(tutorial, {"cast/shingo.glb": Path("x")})
        self.assertEqual(ctx.exception.reason, "missing")
        self.assertIn("ghost", str(ctx.exception))
        career.check_cast(tutorial, {"cast/shingo.glb": Path("x"), "cast/ghost.glb": Path("y")})

    def test_export_checks_the_scenes_tracks_against_the_cast(self):
        tutorial = {"scenes": {"s": {"tracks": [{"character": "ghost"}]}}}
        with tempfile.TemporaryDirectory() as d:
            model = Path(d) / "a.glb"
            model.write_bytes(glb(["HIPS"]))
            with self.assertRaises(career.Refused):
                career.export(self.world(), Path(d) / "out", cast=self.part({"cast/benny_fairfax.glb": model}),
                              scenes=lambda work: ({"scenes/tutorial.json": tutorial}, {}))
            self.assertFalse((Path(d) / "out/cast").exists())


@needs_disc
class CastDiscTest(unittest.TestCase):
    """Task 9 on the real disc (spec 15.2): every model, converted one at a time, with the scene rig of the bank that animates it."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-cast-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.disc = Path(DISC)
        cls.files, cls.summary = cast.cast_export(cls.disc, Path(ASSETS), Path(cls.tmp.name))
        cls.models = {Path(rel).stem: Path(p).read_bytes() for rel, p in cls.files.items()}

    def test_scene_rigs_have_their_bone_counts(self):
        """133 / 131 / 50 as spec 15.2 lists them; the joints are the bank's bones exactly."""
        self.assertEqual(sorted(self.models), sorted(JOINTS))
        for key, count in JOINTS.items():
            with self.subTest(key=key):
                names = cast.joint_names(self.models[key])
                self.assertEqual(len(names), count)
                self.assertEqual(len(set(names)), count)
                self.assertEqual(set(names), set(cast.bank_rig(self.disc, cast.CAST[key].bank).names))

    def test_shingo_and_shingo_prop_differ(self):
        """Two recipes, two models: the cutscene player must not merge them (spec 15.6)."""
        self.assertNotEqual(cast.mesh_digest(self.models["shingo"]), cast.mesh_digest(self.models["shingo_prop"]))

    def test_reda_s_prop_model_carries_the_video_camera(self):
        self.assertNotEqual(cast.mesh_digest(self.models["giovanni_reda"]), cast.mesh_digest(self.models["giovanni_reda_prop"]))

    def test_every_track_of_the_stage_1_scenes_has_a_model_and_its_bank_s_rig(self):
        """Each non-player track's bank has the same bones as the model the track names (stage1_nis 3.1)."""
        boot = career.big(self.disc, "data/big/miscboot.big")
        records = career.parse_scene_records(career.disc_entry(boot, "data/scene/scene_000.bin"), career.SCENES)
        seen = set()
        for scene in records.values():
            for track in scene["tracks"]:
                if track["character"] == "current_player":
                    continue
                key = track["character"]
                seen.add(key)
                with self.subTest(scene=scene, track=track["anim"]):
                    self.assertIn(key, self.models)
                    bank = Path(track["anim"]).stem
                    self.assertEqual(set(cast.bank_rig(self.disc, bank).names), set(cast.joint_names(self.models[key])))
        self.assertEqual(seen, set(JOINTS) - {"giovanni_reda", "player_skater"})

    def test_the_hierarchy_is_the_banks_and_the_bind_pose_skins_to_itself(self):
        """The joints hang as the bank hangs them (its clips are authored so), and the nodes' rest worlds are the inverse binds."""
        for key, data in self.models.items():
            with self.subTest(key=key):
                bank = cast.bank_rig(self.disc, cast.CAST[key].bank)
                self.assertEqual(cast.joint_parents(data), {n: (bank.names[p] if p >= 0 else None) for n, p in zip(bank.names, bank.parents)})
                self.assertLess(cast.rest_error(data), 1e-3)

    def test_the_player_rig_is_the_male1_and_female1_banks_rig(self):
        names = set(cast.joint_names(self.models["player_skater"]))
        for bank in ("02_PassesBaton_Male1", "02_PassesBaton_Female1", "03_RedaLeaves_Male1"):
            self.assertEqual(set(cast.bank_rig(self.disc, bank).names), names, bank)

    def test_the_models_are_skinned_and_not_empty(self):
        for key, data in self.models.items():
            with self.subTest(key=key):
                self.assertGreater(len(data), 100_000)
                length = struct.unpack_from("<I", data, 12)[0]
                doc = json.loads(data[20:20 + length])
                self.assertGreaterEqual(len(doc["meshes"][0]["primitives"]), 1)
                for primitive in doc["meshes"][0]["primitives"]:
                    self.assertIn("JOINTS_0", primitive["attributes"])


if __name__ == "__main__":
    unittest.main()
