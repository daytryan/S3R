"""List and extract Skate 3's skate park piece catalogue (data/content/parkassets.big) from the player's own disc.

py -3 tools/skate3_parkassets.py --disc DISC --list
py -3 tools/skate3_parkassets.py --disc DISC --out DIR

DISC is the extracted Skate 3 disc folder (data/content/parkassets.big; the exe names it at 0x82178bac, used at
0x828a1bf0, loaded by "Park::AssetManager" 0x821819b4; stage2_park.md Q2). DIR must be outside the repo: retail data
never goes into it.

--out writes
- DIR/parkassets/<archive path>: every entry as the archive stores it, decompressed. Piece models are
  data/content/{static,dynamic}/model/<...>/<arenaid>.rx2 (render model, ClusteredMesh collision, grind splines),
  textures data/content/{static,dynamic}/texture/<id>.rx2, the recipe databases under data/content/recipe/.
- DIR/recipe.json {"mods": {mod id: {"name", "db" (static | dynamic), "comp" (the recipe category), "caps" {cap: value}
  (only caps with a value), "lods": [{"idx", "id", "arenaid", "model" (archive path), "materials": [{"matvar", "name",
  "type", "textures": {channel: archive path}}]}]}}}.
  The mod id is the type-03 piece id a .park file stores (tools/skate3_park.py); the lod arenaid is the model RX2's
  file name. A lod's matvar ids are the recipe <mat> ids; each <sp> of that <mat> names a texture RX2 by id and channel.
  Ids are "0x" + 16 lower hex digits, as the recipe writes them.

Refusals (Refused, exit 2): a recipe that is not "Skate Park Recipe v1.0" XML, a mod id twice, a lod whose model, matvar
or texture the archive does not hold, an unsafe archive path, or --out inside the repo.
"""
import argparse
import json
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path[:0] = [str(Path(__file__).resolve().parent.parent), str(Path(__file__).resolve().parent)]  # as skate3_online.py:76
from owned_game.big import BigArchive  # noqa: E402

ARCHIVE = "data/content/parkassets.big"
RECIPES = {"static": "data/content/recipe/static/static_db_xbfdba595_000.xml",
           "dynamic": "data/content/recipe/dynamic/dynamic_db_x946ad45d_000.xml"}
REPO = Path(__file__).resolve().parent.parent


class Refused(Exception):
    """The export stops: `reason` names what the archive did not hold or did not match."""

    def __init__(self, reason, detail):
        super().__init__(f"{reason}: {detail}")
        self.reason = reason


def index(entries):
    """Model and texture archive paths by id. The same texture id can sit under static/ and dynamic/: kept per db."""
    models, textures = {}, {}
    for e in entries:
        parts = e.path.split("/")
        if not e.path.endswith(".rx2") or len(parts) < 4:
            continue
        key = parts[-1][:-4]
        if parts[3] == "model":
            if key in models:
                raise Refused("duplicate", f"model {key} at {models[key]} and {e.path}")
            models[key] = e.path
        elif parts[3] == "texture":
            textures[(parts[2], key)] = e.path
    return models, textures


def recipe(xml, db, models, textures):
    """One recipe database as {mod id: mod}; every model, matvar and texture must resolve."""
    if b"Skate Park Recipe v1.0" not in xml[:200]:
        raise Refused("recipe", f"{db} database is not a Skate Park Recipe v1.0")
    root = ET.fromstring(xml)
    mats = {m.get("id"): m for m in root.iter("mat")}
    mods = {}
    for comp in root.iter("comp"):
        for mod in comp.iter("mod"):
            mid = mod.get("id")
            lods = []
            for lod in mod.iter("lod"):
                arenaid = lod.get("arenaid")
                if arenaid not in models:
                    raise Refused("missing", f"{mid} lod {lod.get('idx')}: no model {arenaid}")
                materials = []
                for var in lod.iter("matvar"):
                    mat = mats.get(var.get("id"))
                    if mat is None:
                        raise Refused("missing", f"{mid}: no <mat> {var.get('id')}")
                    tex = {}
                    for sp in mat.iter("sp"):
                        # the same id under the other db's folder is a copy of the same texture (600 ids, 619 files)
                        path = textures.get((db, sp.get("id"))) or textures.get(("dynamic" if db == "static" else "static", sp.get("id")))
                        if path is None:
                            raise Refused("missing", f"{mid}: no texture {sp.get('id')} ({sp.get('chn')})")
                        tex[sp.get("chn")] = path
                    materials.append({"matvar": var.get("id"), "name": var.get("n"), "type": mat.get("type"), "textures": tex})
                lods.append({"idx": int(lod.get("idx")), "id": lod.get("id"), "arenaid": arenaid, "model": models[arenaid],
                             "materials": materials})
            if mid in mods:
                raise Refused("duplicate", f"mod {mid} twice in the {db} database")
            caps = {c.tag[4:]: c.get("value") for c in mod.iter() if c.tag.startswith("cap.") and c.get("value")}
            mods[mid] = {"name": mod.get("n"), "db": db, "comp": comp.get("n"), "caps": caps, "lods": lods}
    return mods


def catalogue(archive):
    """Both recipe databases joined: {mod id: mod}."""
    models, textures = index(archive.entries)
    by_path = {e.path: e for e in archive.entries}
    out = {}
    for db, path in RECIPES.items():
        if path not in by_path:
            raise Refused("recipe", f"no {path}")
        for mid, mod in recipe(archive.read(by_path[path]), db, models, textures).items():
            if mid in out:
                raise Refused("duplicate", f"mod {mid} in both databases")
            out[mid] = mod
    return out


def export(archive, out):
    out = Path(out).resolve()
    if out == REPO or REPO in out.parents:
        raise Refused("out", f"{out} is inside the repo; retail data stays outside it")
    mods = catalogue(archive)
    for e in archive.entries:
        BigArchive.safe_relative(e.path)  # refuse before writing anything
    archive.extract_entries(archive.entries, out / "parkassets", overwrite=True)
    out.mkdir(parents=True, exist_ok=True)
    (out / "recipe.json").write_text(json.dumps({"mods": mods}, indent=1), encoding="utf-8", newline="\n")
    return {"entries": len(archive.entries), "mods": len(mods)}


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--disc", type=Path, required=True, help="extracted Skate 3 disc folder (read only)")
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--list", action="store_true", help="print index, unpacked size, compression and path of each entry")
    g.add_argument("--out", type=Path, help="folder outside the repo that receives parkassets/ and recipe.json")
    a = p.parse_args(argv)
    archive = BigArchive(a.disc / ARCHIVE)
    if a.list:
        for e in archive.entries:
            print(e.index, e.unpacked_size, e.compression, e.path)
        return 0
    try:
        s = export(archive, a.out)
    except Refused as e:
        print(f"refused ({e})", file=sys.stderr)
        return 2
    print(f"wrote {s['entries']} entries under {a.out / 'parkassets'} and {s['mods']} mods to {a.out / 'recipe.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
