"""The stage-1 cast of the Skate 3 career as character GLBs on the cutscene banks' full scene rigs (spec 15.2).

py -3 tools/skate3_career.py --disc DISC --assets INST/assets --out DIR     (this is one part of that export)

Output: DIR/cast/<key>.glb for the eleven models of CAST, each built from the disc's own recipe (marquee.big, or livingworld.big for
the pedestrians) the way tools/pipeline/skate2port/old_tutorial_cast.py builds the Skate 2 tutorial's cast, with the joints of the
scene bank that animates it (stage1_nis 3.1):
- 133 joints: benny_fairfax, giovanni_reda, giovanni_reda_prop (giovanni_reda with the video camera), shingo, shingo_prop (shingo with
  the video camera), ai_skater_03 and ai_skater_04: the skater, finger, twist, helper and face joints, and LEFT_PROP / RIGHT_PROP.
- 131 joints: player_skater, the rig of the player's Male1 and Female1 banks (the same without the two prop bones).
- 50 joints: female_adult_2, male_adult_1 and male_adult_2, the pedestrian rig (TRAJECTORY .. LEFTUPLEGTWIST).
The installed native-roster GLBs have 35 joints and the Skate 1 / Skate 2 Reda and Shingo are other games' models; none is used.

How a joint exists. The converter keeps a joint for each bank bone the model's own rx2 files bind. The face rig (FACE, OFFSET_*), the
trajectory root and the prop bones skin nothing, so no model file binds them: such a bone is bound where its parent is (no offset; the
bank's clips place it), the way old_challenges_cast.py gives Skate 1's face bones a bind. The joints hang as the bank hangs them (rehang),
each keeping its rest world. The model's skin is the converter's own, unchanged. Retail models: --out only, never the repo.

Every model is converted on its own, in a work folder that is removed again, so the memory the conversion needs is one character's.
Nothing here is tested in play.
"""
import contextlib
import functools
import hashlib
import io
import json
import shutil
import struct
import sys
import tempfile
import xml.etree.ElementTree as ET
from contextlib import redirect_stdout
from dataclasses import dataclass
from pathlib import Path
from typing import NamedTuple

import numpy as np

TOOLS = Path(__file__).resolve().parent
for _path in (TOOLS.parent, TOOLS):         # tools.asset_pipeline (the converter's own import form) and the bare tools modules
    if str(_path) not in sys.path:
        sys.path.insert(0, str(_path))

RECIPE_FIELD = "Hash_6C9F05D8DBE7A492"       # characters_marquee's recipe name (old_tutorial_cast.RECIPE_FIELD; native_roster.roster reads it)
SCENE_BIG = "data/big/scene.big"
BANK_DIR = "data/scene/anim/"
STOCK = "private/stock"                       # <assets>/private/stock/data/anim/OnBoard.abin: the skater rig the 36 base joints come from
REST_TOLERANCE = 1e-3                         # rest_error: the inverse binds are float32
REPARENTED = {32: "RIGHTTOEBASE_REPARENTED", 33: "LEFTTOEBASE_REPARENTED", 34: "LEFTHAND_REPARENTED", 35: "RIGHTHAND_REPARENTED"}
# character_glb.convert renames these four base joints (the skater's reparented toes and hands) and binds them at the rig's reference pose.
# The pedestrian rig has four bones no model skins to put there instead (HEADEND, the hand props and the trajectory root); they are named back.
PED_SPARE = ("LEFTHANDPROP", "RIGHTHANDPROP", "HEADEND", "TRAJECTORY")


class Member(NamedTuple):
    package: str      # "marquee" or "livingworld": the .big the recipe is in
    recipe: str       # recipe/<package>/<recipe>.xml
    bank: str         # the scene bank (scene.big data/scene/anim/<bank>.abin) whose bones are the joints


# spec 15.2 / stage1_nis 3.1 (scene_000.bin tracks): the model each track names and the bank that animates it. giovanni_reda is the
# plain recipe (Reda's FTL leader uses it, spec 15.2) on the prop bank's rig; player_skater is the retail tutorial skater recipe
# (tutorial_male) on the player's Male1 bank. The cutscene player poses the player's own skater for `current_player`, so that model is
# the stand-in that gives the 131-bone rig a GLB (spec 15.2 lists the rig): INFERRED (the recipe name; spec 15.2 names no recipe for it).
CAST = {
    "benny_fairfax": Member("marquee", "benny_fairfax", "tut_sksc_011"),
    "giovanni_reda": Member("marquee", "giovanni_reda", "02_PassesBaton_Reda1"),
    "giovanni_reda_prop": Member("marquee", "giovanni_reda_prop", "02_PassesBaton_Reda1"),
    "shingo": Member("marquee", "shingo", "03_RedaLeaves_Shingo1"),
    "shingo_prop": Member("marquee", "shingo_prop", "02_PassesBaton_Shingo1"),
    "ai_skater_03": Member("marquee", "ai_skater_03", "02_PassesBaton_Skater031"),
    "ai_skater_04": Member("marquee", "ai_skater_04", "02_PassesBaton_Skater041"),
    "player_skater": Member("marquee", "tutorial_male", "02_PassesBaton_Male1"),
    "female_adult_2": Member("livingworld", "female_adult_2", "02_PassesBaton_Ped011"),
    "male_adult_1": Member("livingworld", "male_adult_1", "02_PassesBaton_Ped021"),
    "male_adult_2": Member("livingworld", "male_adult_2", "02_PassesBaton_Ped031"),
}


class CastFailed(Exception):
    """A model could not be built or does not have its bank's rig: the export stops (nothing is written)."""

    def __init__(self, key, detail=""):
        super().__init__(f"{key}" + (f" ({detail})" if detail else ""))
        self.key, self.detail = key, detail


# ---------------------------------------------------------------- GLB readers
def glb_json(data):
    """(document, binary chunk) of a GLB; anything else is a failure."""
    if len(data) < 28 or struct.unpack_from("<II", data, 0) != (0x46546C67, 2):
        raise CastFailed("glb", "not a glTF 2.0 binary")
    length = struct.unpack_from("<I", data, 12)[0]
    if data[16:20] != b"JSON" or 20 + length + 8 > len(data):
        raise CastFailed("glb", "no JSON chunk")
    return json.loads(data[20:20 + length]), data[28 + length:]


def joint_names(data):
    """The names of the first skin's joints, in joint order."""
    doc, _ = glb_json(data)
    skins = doc.get("skins") or []
    if not skins:
        raise CastFailed("glb", "no skin")
    return [doc["nodes"][j].get("name", "") for j in skins[0]["joints"]]


def mesh_digest(data):
    """sha256 over the POSITION data of every primitive: two recipes with one model have one digest."""
    doc, binary = glb_json(data)
    digest = hashlib.sha256()
    for mesh in doc.get("meshes", []):
        for primitive in mesh["primitives"]:
            accessor = doc["accessors"][primitive["attributes"]["POSITION"]]
            view = doc["bufferViews"][accessor["bufferView"]]
            start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
            digest.update(binary[start:start + accessor["count"] * 12])
    return digest.hexdigest()


def pack_glb(doc, binary):
    text = json.dumps(doc, separators=(",", ":"), allow_nan=False).encode()
    text += b" " * (-len(text) % 4)
    return (struct.pack("<III", 0x46546C67, 2, 28 + len(text) + len(binary)) + struct.pack("<I4s", len(text), b"JSON") + text
            + struct.pack("<I4s", len(binary), b"BIN\0") + binary)


def skinned_joints(data):
    """The names of the first skin's joints that some vertex gives a weight above zero."""
    doc, binary = glb_json(data)
    joints = doc["skins"][0]["joints"]

    def read(accessor_index, dtype):
        accessor = doc["accessors"][accessor_index]
        view = doc["bufferViews"][accessor["bufferView"]]
        return np.frombuffer(binary, dtype=dtype, count=4 * accessor["count"],
                             offset=view.get("byteOffset", 0) + accessor.get("byteOffset", 0)).reshape(-1, 4)

    used = set()
    for mesh in doc["meshes"]:
        for primitive in mesh["primitives"]:
            attributes = primitive["attributes"]
            index, weight = read(attributes["JOINTS_0"], "<u2"), read(attributes["WEIGHTS_0"], "<f4")
            used |= {int(j) for j in np.unique(index[weight > 0])}
    return {doc["nodes"][joints[j]]["name"] for j in used}


def rename_nodes(data, names):
    """The GLB with each node named by a key of `names` renamed to its value; the binary chunk is untouched."""
    doc, binary = glb_json(data)
    for node in doc["nodes"]:
        node["name"] = names.get(node.get("name"), node.get("name"))
    return pack_glb(doc, binary)


def node_worlds(doc):
    """{node index: 4x4 world} of every node, rest pose: row-vector matrices as character_glb writes them (world = local @ parent's)."""
    parent = {child: index for index, node in enumerate(doc["nodes"]) for child in node.get("children", [])}
    worlds = {}

    def world(index):
        if index not in worlds:
            local = np.array(doc["nodes"][index].get("matrix", np.identity(4).ravel().tolist()), dtype=np.float64).reshape(4, 4)
            worlds[index] = local @ world(parent[index]) if index in parent else local
        return worlds[index]

    for index in range(len(doc["nodes"])):
        world(index)
    return worlds


def joint_parents(data):
    """{joint name: parent joint's name or None} of the first skin."""
    doc, _ = glb_json(data)
    joints = doc["skins"][0]["joints"]
    parent = {child: index for index, node in enumerate(doc["nodes"]) for child in node.get("children", [])}
    return {doc["nodes"][j]["name"]: (doc["nodes"][parent[j]]["name"] if parent.get(j) in joints else None) for j in joints}


def rest_error(data):
    """The largest difference between a joint's rest world (its nodes' matrices multiplied down from the scene root) and the inverse of its
    inverse bind matrix: 0 where the skin leaves the mesh as it was modelled."""
    doc, binary = glb_json(data)
    skin = doc["skins"][0]
    accessor = doc["accessors"][skin["inverseBindMatrices"]]
    view = doc["bufferViews"][accessor["bufferView"]]
    start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
    binds = np.frombuffer(binary, dtype="<f4", count=16 * accessor["count"], offset=start).reshape(-1, 4, 4).astype(np.float64)
    worlds = node_worlds(doc)
    return max(float(np.abs(worlds[j] - np.linalg.inv(binds[i])).max()) for i, j in enumerate(skin["joints"]))


def rehang(data, rig):
    """The GLB with every joint hung under the parent `rig` (the scene bank) gives it, a bank root under the scene root beside the mesh. Each
    joint keeps its rest world, so the bind pose and the skin do not change; only the nodes' local matrices and children do."""
    doc, binary = glb_json(data)
    nodes, joints = doc["nodes"], doc["skins"][0]["joints"]
    by_name = {nodes[j]["name"]: j for j in joints}
    if sorted(by_name) != sorted(rig.names):
        raise CastFailed("rehang", f"the joints are not the bank's bones: missing {sorted(set(rig.names) - set(by_name))[:6]}, "
                                   f"extra {sorted(set(by_name) - set(rig.names))[:6]}")
    scene = doc["scenes"][doc.get("scene", 0)]["nodes"]
    if len(scene) != 1:
        raise CastFailed("rehang", "the scene has no single root node")
    worlds = node_worlds(doc)
    root = scene[0]
    kept = [child for child in nodes[root].get("children", []) if child not in by_name.values()]
    roots = []
    for node in joints:
        nodes[node].pop("children", None)
    for name, parent in zip(rig.names, rig.parents):
        node = by_name[name]
        if parent < 0:
            roots.append(node)
            nodes[node]["matrix"] = worlds[node].ravel().tolist()
        else:
            above = by_name[rig.names[parent]]
            nodes[above].setdefault("children", []).append(node)
            nodes[node]["matrix"] = (worlds[node] @ np.linalg.inv(worlds[above])).ravel().tolist()
    nodes[root]["children"] = roots + kept
    return pack_glb(doc, binary)


# ---------------------------------------------------------------- the bank's rig
@dataclass
class Rig:
    names: list                                  # the bank's bone names, parents before children
    parents: list                                # the parent's index, -1 for a root


def matrix_sqt(m):
    """((sx, sy, sz), (x, y, z, w), (tx, ty, tz)) of a 4x4 row-vector matrix (character_glb's convention: retail_character.quat_matrix)."""
    m = np.asarray(m, dtype=np.float64)
    scale = np.linalg.norm(m[:3, :3], axis=1)
    r = m[:3, :3] / np.where(scale > 1e-12, scale, 1.0)[:, None]
    trace = r[0, 0] + r[1, 1] + r[2, 2]
    if trace > 0:
        s = 2 * np.sqrt(trace + 1)
        q = ((r[1, 2] - r[2, 1]) / s, (r[2, 0] - r[0, 2]) / s, (r[0, 1] - r[1, 0]) / s, s / 4)
    elif r[0, 0] >= r[1, 1] and r[0, 0] >= r[2, 2]:
        s = 2 * np.sqrt(1 + r[0, 0] - r[1, 1] - r[2, 2])
        q = (s / 4, (r[0, 1] + r[1, 0]) / s, (r[2, 0] + r[0, 2]) / s, (r[1, 2] - r[2, 1]) / s)
    elif r[1, 1] >= r[2, 2]:
        s = 2 * np.sqrt(1 + r[1, 1] - r[0, 0] - r[2, 2])
        q = ((r[0, 1] + r[1, 0]) / s, s / 4, (r[1, 2] + r[2, 1]) / s, (r[2, 0] - r[0, 2]) / s)
    else:
        s = 2 * np.sqrt(1 + r[2, 2] - r[0, 0] - r[1, 1])
        q = ((r[2, 0] + r[0, 2]) / s, (r[1, 2] + r[2, 1]) / s, s / 4, (r[0, 1] - r[1, 0]) / s)
    return tuple(float(v) for v in scale), tuple(float(v) for v in q), tuple(float(v) for v in m[3, :3])


def matrix_quat(m):
    return matrix_sqt(m)[1]


def bank_rig(disc, bank, archive=None):
    """The Rig of scene.big's data/scene/anim/<bank>.abin: its hierarchy's bone names and parents. One entry is read; `archive` is an open
    scene.big."""
    from tools.asset_pipeline.retail_character import ABIN
    from tools.owned_game.big import BigArchive
    archive = archive or BigArchive(Path(disc) / SCENE_BIG)
    wanted = f"{BANK_DIR}{bank}.abin".lower()
    entry = next((e for e in archive.entries if e.path.lower() == wanted), None)
    if entry is None:
        raise CastFailed(bank, "scene.big has no such bank")
    data = archive.read(entry)
    abin = ABIN.AbinFile(data)
    h = abin.hierarchy
    if h is None:
        raise CastFailed(bank, "the bank has no hierarchy")
    base = h.skel_base + 8 + 8 * h.num_bones + 36 * h.num_parts       # the FastString names follow the part records (5 words each)
    names = ["".join(ABIN._decode6(struct.unpack_from(">I", data, base + 20 * i + 4 * w)[0]) for w in range(5)) for i in range(h.num_bones)]
    if len(set(names)) != len(names):
        raise CastFailed(bank, "a bone name twice")
    return Rig(names=names, parents=list(h.parents))


def in_spare_slots(rig, spare):
    """`rig` with the bones `spare` in slots 32..35 (REPARENTED) and the others in their order around them, parents carried over.
    A spare bone must have no child and its parent must come before the slot; anything else is refused."""
    if len(spare) != len(REPARENTED) or any(name not in rig.names for name in spare):
        raise CastFailed("spare slots", f"{list(spare)} are not four bones of the rig")
    parents = {rig.names[p] for p in rig.parents if p >= 0}
    if parents & set(spare):
        raise CastFailed("spare slots", f"{sorted(parents & set(spare))} have children")
    rest = [i for i, name in enumerate(rig.names) if name not in spare]
    first = min(REPARENTED)
    order = rest[:first] + [rig.names.index(name) for name in spare] + rest[first:]
    new = {old: index for index, old in enumerate(order)}
    result = Rig(names=[rig.names[o] for o in order], parents=[new[rig.parents[o]] if rig.parents[o] >= 0 else -1 for o in order])
    for index, parent in enumerate(result.parents):
        if parent >= index:
            raise CastFailed(result.names[index], "its parent comes after it once the spare bones are in their slots")
    return result


def synthesise(bind, rig):
    """Give every bank bone `bind` lacks its parent's bind (identity for a root): it stands where its parent does and the clips move it.
    `bind` is {NAME: 4x4} and gains the entries; the bank lists a parent before its children, or it is refused.
    INFERRED (no model file binds these bones and the disc has no reference pose for the bank; the bank's clips place them)."""
    for index, name in enumerate(rig.names):
        parent = rig.parents[index]
        if parent >= index:
            raise CastFailed(name, "the bank lists a bone before its parent")
        if name in bind:
            continue
        bind[name] = np.array(bind[rig.names[parent]], dtype=np.float64) if parent >= 0 else np.identity(4)


def rig_skeleton(rig, ref_pose=None):
    """retail_character.SkeletonSet whose rig is the bank's: the bones no model binds are synthesised, and a bone a model file leaves
    parentless takes the bank's parent. `ref_pose` ({index: SQT}), where given, is filled with the models' own bind pose, local to local:
    character_glb.convert binds four base joints at the rig's reference pose instead (REPARENTED), and the pedestrian rig has no
    reference pose of its own on the disc."""
    from tools.asset_pipeline import retail_character
    from tools.asset_pipeline.retail_character import ABIN, rigid_inverse

    class RigSkeleton(retail_character.SkeletonSet):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            synthesise(self.bind, rig)
            self.attach = {n: (rig.names[p] if p >= 0 else None) for n, p in zip(rig.names, rig.parents)}
            if ref_pose is not None:
                for index, name in enumerate(rig.names):
                    parent = rig.parents[index]
                    local = self.bind[name] @ rigid_inverse(self.bind[rig.names[parent]]) if parent >= 0 else self.bind[name]
                    scale, quat, trans = matrix_sqt(local)
                    ref_pose[index] = ABIN.SQT(scale, quat, trans)

    return RigSkeleton


# ---------------------------------------------------------------- resources of the pedestrians
def living_path(path):
    return path.replace("/marquee/", "/livingworld/")


def living_resources():
    """native_roster.Resources reading livingworld.big, whose recipe/<name>.xml and model/texture folders are marquee's layout under another
    name (old_challenges_cast.py reads Seb the same way)."""
    from tools.asset_pipeline import native_roster

    class LivingResources(native_roster.Resources):
        def resolve(self, path):
            return super().resolve(living_path(path))

    return LivingResources


def cut_maps(xml):
    """{alpha map id: diffuse map id} (ids without 0x) of the recipe's materials that have both."""
    maps = {}
    for mat in ET.fromstring(xml).findall("mat"):
        channels = {sp.get("chn"): sp.get("id", "").removeprefix("0x") for sp in mat.findall("sp")}
        if "alpha" in channels and "diffuse" in channels:
            maps[channels["alpha"]] = channels["diffuse"]
    return maps


def cut_map_decoder(decode, maps):
    """`decode` (native_roster.decode_png: parser, rx2, png) that gives a cut map the size of the diffuse map it cuts. A pedestrian's hair
    has a 128 x 128 diffuse and a 256 x 256 alpha, and native_roster puts the alpha in the diffuse's alpha channel, which needs one size.
    INFERRED (native_roster.py: the map is sampled by UV, so its resolution is free); the diffuse is decoded first."""
    def wrapped(parser, src, dest):
        decode(parser, src, dest)
        dest = Path(dest)
        diffuse = maps.get(dest.stem)
        if diffuse and (dest.parent / f"{diffuse}.png").exists():
            from PIL import Image
            with Image.open(dest.parent / f"{diffuse}.png") as picture:
                size = picture.size
            with Image.open(dest) as picture:
                resized = picture.resize(size, Image.LANCZOS) if picture.size != size else None
            if resized is not None:
                resized.save(dest)
    return wrapped


@contextlib.contextmanager
def patched(archive, resources, skeleton, convert, bone_names=None, cut=None):
    """native_roster.prepare, which reads marquee.big and converts on the skater rig, pointed at one model: `archive` is the .big it opens,
    `resources` its Resources class, `skeleton` the SkeletonSet character_glb builds, `convert` extra keyword arguments for
    character_glb.convert, `bone_names` (where given) ABIN.BONE_NAMES, the base joints' names, and `cut` (where given) the cut_maps
    table. Everything is put back on exit."""
    from tools.asset_pipeline import character_glb, native_roster
    abin = character_glb.ABIN
    saved = (native_roster.BigArchive, native_roster.Resources, native_roster.convert, character_glb.SkeletonSet, abin.BONE_NAMES,
             native_roster.decode_png)
    try:
        native_roster.BigArchive = lambda path: archive
        native_roster.Resources = resources
        native_roster.convert = functools.partial(saved[2], **convert)        # a swapped convert also keeps prepare off its worker processes
        character_glb.SkeletonSet = skeleton
        if bone_names is not None:
            abin.BONE_NAMES = bone_names
        if cut:
            native_roster.decode_png = cut_map_decoder(saved[5], cut)
        yield
    finally:
        (native_roster.BigArchive, native_roster.Resources, native_roster.convert, character_glb.SkeletonSet, abin.BONE_NAMES,
         native_roster.decode_png) = saved


# ---------------------------------------------------------------- building a model
def build_model(disc, assets, work, key, rig):
    """The GLB bytes of CAST[key]: native_roster.prepare on the one recipe, in `work` (removed by the caller), on `rig`'s joints."""
    from tools.asset_pipeline import native_roster
    from tools.owned_game.big import BigArchive
    member = CAST[key]
    disc, assets, work = Path(disc), Path(assets), Path(work)
    archive = BigArchive(disc / f"data/content/{member.package}.big")
    pedestrian = member.package == "livingworld"
    ref_pose = {} if pedestrian else None
    convert = {"extra_bones": set(rig.names), "stock": assets / STOCK}
    shim = rig
    if pedestrian:
        # The pedestrian rig is not the skater's: its 50 bones are the bank's own, with four that skin nothing in slots 32..35.
        shim = in_spare_slots(rig, PED_SPARE)
        convert["extra_bones"] = set(rig.names) - set(PED_SPARE)
        convert["source"] = type("PedestrianRig", (), {"parents": shim.parents, "ref_pose": ref_pose})()
    rows = [{"class": "characters_marquee", "key": "ip", "parent": "", "fields": {}},
            {"class": "characters_marquee", "key": key, "parent": "ip", "fields": {RECIPE_FIELD: {"data": member.recipe}}}]
    (work / "collections.json").write_text(json.dumps({"collections": rows}), encoding="utf-8")
    log = io.StringIO()
    resources = living_resources() if pedestrian else native_roster.Resources
    cut = cut_maps(resources(archive).read(f"data/content/recipe/marquee/{member.recipe}.xml"))
    with patched(archive, resources, rig_skeleton(shim, ref_pose), convert, bone_names=shim.names if pedestrian else None, cut=cut), \
            redirect_stdout(log):
        report = native_roster.prepare(disc, assets, work / "library", work / "collections.json", work / "build", only=[key])
    if len(report) != 1 or report[0]["status"] != "ready":
        raise CastFailed(key, (report[0].get("error") if report else "native_roster built nothing") or log.getvalue().strip()[-300:])
    data = (work / "library" / "entries" / report[0]["id"] / "character.glb").read_bytes()
    if pedestrian:
        # character_glb.convert named the base joints 32..35 *_REPARENTED (the skater's); here they are the spare bones, and nothing may skin them.
        data = rename_nodes(data, {REPARENTED[slot]: shim.names[slot] for slot in REPARENTED})
        skinned = skinned_joints(data) & set(PED_SPARE)
        if skinned:
            raise CastFailed(key, f"the model skins {sorted(skinned)}, which stand in the converter's reparented slots")
    data = rehang(data, rig)
    error = rest_error(data)
    if error > REST_TOLERANCE:
        raise CastFailed(key, f"the rest pose no longer matches the inverse binds (off by {error:.4g})")
    return data


def cast_export(disc, assets, work, keys=None, rigs=None):
    """(files, summary): every model of `keys` (default all of CAST) built one at a time. `files` maps cast/<key>.glb to a Path in `work`;
    summary {"models", "joints": {key: n}, "bytes": {key: n}, "total_bytes"}. Each model must have exactly its bank's joints; `rigs`
    ({key: Rig}) replaces reading the banks (tests). Retail models: --out only."""
    keys = list(CAST) if keys is None else list(keys)
    for key in keys:
        if key not in CAST:
            raise CastFailed(key, "not in the cast table")
    work = Path(work)
    out = work / "cast"
    out.mkdir(parents=True, exist_ok=True)
    scene_big = None
    files, joints, sizes = {}, {}, {}
    for key in keys:
        try:
            if rigs and key in rigs:
                rig = rigs[key]
            else:
                if scene_big is None:
                    from tools.owned_game.big import BigArchive
                    scene_big = BigArchive(Path(disc) / SCENE_BIG)
                rig = bank_rig(disc, CAST[key].bank, scene_big)
            build = Path(tempfile.mkdtemp(prefix=f"cast-{key}-", dir=work))
            try:
                data = build_model(disc, assets, build, key, rig)
            finally:
                shutil.rmtree(build, ignore_errors=True)
            names = joint_names(data)
        except CastFailed:
            raise
        except Exception as error:
            raise CastFailed(key, f"{type(error).__name__}: {error}") from error
        if sorted(names) != sorted(rig.names):
            raise CastFailed(key, f"joints differ from the bank's {len(rig.names)} bones: missing {sorted(set(rig.names) - set(names))[:6]}, "
                                  f"extra {sorted(set(names) - set(rig.names))[:6]}, {len(names)} in all")
        path = out / f"{key}.glb"
        path.write_bytes(data)
        files[f"cast/{key}.glb"] = path
        joints[key], sizes[key] = len(names), len(data)
    return files, {"models": len(files), "joints": joints, "bytes": sizes, "total_bytes": sum(sizes.values())}
