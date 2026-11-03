"""Prepare the original Start-button crossbar menu (main/core_menu) from the owned game.

No executable/game launch. Output contains copyrighted assets and stays private.
The movie imports shared controls (menu picker, highlight text, ...). Their bundles
are merged into one self-contained manifest: every bundle keeps its own character
ids offset by BUNDLE_STRIDE, imported placeholders resolve to the exporting
bundle's character, and action blocks are keyed by (bundle index << 20) | offset.
The runtime therefore reuses the single-movie loader unchanged.
"""
from pathlib import Path
import argparse
import hashlib
import json

from vendor.skate3_ui.project import extract_project
from vendor.skate3_ui.scene_graph import AssetCache, SceneFlattener
from vendor.skate3_ui.actions import Actions
# The cache this process writes is read back from memory (vendor binary.py: Defender scans each fresh file on its
# first open); the bytes on disk are the same as Path.read_bytes / write_text would give.
from vendor.skate3_ui.binary import read_file, read_text, write_file, write_text
from prepare_hud import font_mapping

MAIN = "data/fe/source/screens/main/core_menu"
# The pause backdrop screen: one full-screen shape using a 256x256 vignette.
DIMMER = "data/fe/source/screens/main/dimmer"
BUNDLE_STRIDE = 10000
ACTION_SHIFT = 20
MANIFEST = "runtime/core_menu.json"
WHITE = "runtime/white.rgba"


def _bundles(cache: AssetCache, main: str = MAIN) -> list[str]:
    """Main first, then imports breadth-first (normalized, de-duplicated)."""
    order = [cache.normalize_bundle(main)]
    for key in order:
        for item in cache.load_bundle(key)["imports"].values():
            name = cache.normalize_bundle(item["file"])
            if name not in order:
                order.append(name)
    return order


def _init_order(cache: AssetCache, bundles: list[str]) -> list[str]:
    """Dependencies before dependents, so `extends` sees defined superclasses."""
    done: list[str] = []

    def visit(key: str, stack: tuple[str, ...]):
        if key in done:
            return
        if key in stack:
            raise ValueError(f"Cyclic APT imports through {key}")
        for item in cache.load_bundle(key)["imports"].values():
            visit(cache.normalize_bundle(item["file"]), stack + (key,))
        done.append(key)

    for key in bundles:
        visit(key, ())
    return done


CONSTRUCT_EVENT = 0x40000


def _parameters(apt: bytes, acts: Actions, offset: int) -> dict | None:
    """{name: value} if every clip event is construct-time `name = "value"` pairs."""
    import struct
    count, pointer = struct.unpack(">II", apt[offset:offset + 8])
    result = {}
    for i in range(count):
        flags, _, actions = struct.unpack(">III", apt[pointer + 12 * i:pointer + 12 * i + 12])
        stream = [row for row in acts.stream(actions) if row["opcode"] != 0]
        if flags != CONSTRUCT_EVENT or len(stream) % 2 or any(
                (a["opcode"], b["opcode"]) != (0xA1, 0xA6) for a, b in zip(stream[::2], stream[1::2])):
            return None
        result.update({a["operand"]: b["operand"] for a, b in zip(stream[::2], stream[1::2])})
    return result


def _backdrop(cache: AssetCache) -> dict:
    """Texture of the dimmer screen's single full-screen shape."""
    bundle = cache.load_bundle(DIMMER)
    for local, c in bundle["characters"].items():
        if c["type_name"] == "shape":
            for primitive in SceneFlattener(cache, lambda *_: {}).flatten(DIMMER, local)["primitives"]:
                if primitive.get("texture"):
                    return primitive["texture"]
    raise ValueError("Dimmer backdrop texture missing")


def prepare(game: Path, output: Path, collections: Path, main: str = MAIN, manifest: str = MANIFEST,
            kind: str = "skate3-core-menu") -> dict:
    """Merge `main` and its imported control bundles into one runtime manifest."""
    # Extract the main screen plus every control bundle; imports are resolved below.
    extract_project(game, output, prefixes=(main, DIMMER, "data/fe/source/controls/"), update=True)
    cache = AssetCache(output)
    mappings = font_mapping(collections, cache)
    bundles = _bundles(cache, main)
    index = {key: i for i, key in enumerate(bundles)}

    def global_id(bundle_key: str, local: int) -> int:
        owner, character = cache.resolve_character(bundle_key, local)
        return index[owner["key"]] * BUNDLE_STRIDE + character["id"]

    def action_key(bundle_key: str, offset: int) -> int:
        return (index[bundle_key] << ACTION_SHIFT) | offset

    characters, shapes, fonts, actions = [], {}, {}, {}
    init_actions: dict[str, list[int]] = {}
    exports: dict[str, int] = {}
    scoped_exports: dict[str, dict[str, int]] = {}
    dropped_clip_actions = []
    for key in bundles:
        bundle = cache.load_bundle(key)
        for name, local in bundle["exports"].items():
            # Linkage names are per library (several controls export "Notch"): the
            # runtime resolves attachMovie in the calling clip's own bundle first.
            target = global_id(key, local)
            exports.setdefault(name, target)
            scoped_exports.setdefault(str(index[key]), {})[name] = target
        raw = output / "raw" / (key + ".apt")
        acts = Actions(read_file(raw), read_file(raw.with_suffix(".const")))
        base = index[key] * BUNDLE_STRIDE
        for local, c in bundle["characters"].items():
            c = json.loads(json.dumps(c))  # private copy
            c["id"] = base + local
            for frame in c.get("frames", []):
                for control in frame["controls"]:
                    # Only placements that set a character (flag 0x2) carry an id; moves hold -1.
                    if control["type_name"].startswith("place_object") and control.get("flags", 0) & 2:
                        control["character_id"] = global_id(key, control["character_id"])
                    # onClipEvent(construct) records that only assign component
                    # parameters (button_item2's mButtonImage = "DPad_Up") become
                    # placement init values. Any other clip event (the core menu's
                    # hidden DLC banner) is dropped and listed: no clip-event support.
                    if control["type_name"].startswith("place_object") and control.get("flags", 0) & 0x80:
                        init = _parameters(read_file(raw), acts, control["actions_offset"])
                        if init is None:
                            dropped_clip_actions.append({"bundle": key, "character": c["id"],
                                                         "depth": control.get("depth")})
                        else:
                            control["init"] = init
                        control["flags"] &= ~0x80
                        control["actions_offset"] = 0
                        continue
                    offset = control.get("actions_offset", 0)
                    if control["type_name"] in ("do_action", "do_init_action") and offset:
                        stream = acts.stream(offset)
                        control["actions_offset"] = action_key(key, offset)
                        actions[str(control["actions_offset"])] = stream
                        if control["type_name"] == "do_init_action":
                            init_actions.setdefault(key, []).append(control["actions_offset"])
            if c.get("text") and c["text"].get("font_id") is not None:
                c["text"]["font_id"] = global_id(key, c["text"]["font_id"])
            if c["type_name"] == "shape":
                scene = SceneFlattener(cache, lambda *_: {}).flatten(key, local)
                if scene["unresolved"]:
                    raise ValueError(f"Unresolved core-menu shape {key}:{local}: {scene['unresolved']}")
                # ponytail: untessellated strokes (the chyron's "line" unit) have nothing to draw; dropped.
                scene["primitives"] = [p for p in scene["primitives"] if p["triangles"]]
                for primitive in scene["primitives"]:
                    # Solid fills carry their colour in `color`; sample a white texel.
                    if primitive.get("texture") is None:
                        if primitive.get("render_type") != "solid":
                            raise ValueError(f"Untextured non-solid core-menu shape {key}:{local}")
                        primitive["texture"] = {"width": 1, "height": 1, "rgba": WHITE}
                        for triangle in primitive["triangles"]:
                            for vertex in triangle:
                                vertex.setdefault("uv", [0.0, 0.0])
                shapes[str(c["id"])] = scene["primitives"]
            elif c["type_name"] == "font":
                fonts[c["font"]["name"]] = cache.font_asset(c["font"]["name"])
            characters.append(c)

    order = [offset for key in _init_order(cache, bundles) for offset in init_actions.get(key, [])]
    sources = {}
    for key in bundles:
        raw = output / "raw" / (key + ".apt")
        sources[key] = {"apt_sha256": hashlib.sha256(read_file(raw)).hexdigest(),
                        "const_sha256": hashlib.sha256(read_file(raw.with_suffix(".const"))).hexdigest()}
    result = {
        "format": kind, "version": 1,
        "source": {"bundles": sources,
                   "collections_sha256": hashlib.sha256(collections.read_bytes()).hexdigest()},
        "bundles": bundles,
        "characters": characters, "shapes": shapes, "fonts": fonts, "actions": actions,
        "init_order": order, "exports": exports, "scoped_exports": scoped_exports,
        "dropped_clip_actions": dropped_clip_actions,
        "font_mappings": mappings,
        "language": {row["label"].strip(): row["value"] for row in
                     json.loads(read_text(output / "metadata/languages/english_global.json"))["entries"]},
        "unresolved_fonts": [family for family, asset in fonts.items() if asset is None],
        "backdrop": _backdrop(cache),
    }
    target = output / manifest
    target.parent.mkdir(parents=True, exist_ok=True)
    write_file(output / WHITE, b"\xff\xff\xff\xff")
    write_text(target, json.dumps(result, separators=(",", ":")) + "\n")
    print(f"Prepared {main}: {len(bundles)} bundles, {len(characters)} characters, "
          f"{len(shapes)} shapes, {len(actions)} action blocks: {target}")
    if result["unresolved_fonts"]:
        print("Unresolved authored font families: " + ", ".join(result["unresolved_fonts"]))
    return result


def runtime_files(root: Path, manifest: str = MANIFEST, kind: str = "skate3-core-menu") -> dict:
    """Manifest plus every referenced RGBA payload, validated by size."""
    data = json.loads(read_text(root / manifest))
    if data.get("format") != kind or data.get("version") != 1 or data.get("unresolved_fonts"):
        raise ValueError("Invalid core menu manifest or unresolved original fonts")
    textures = [(p["texture"]["rgba"], p["texture"]["width"], p["texture"]["height"])
                for shape in data["shapes"].values() for p in shape]
    if data.get("backdrop"):
        b = data["backdrop"]
        textures.append((b["rgba"], b["width"], b["height"]))
    textures += [(f["texture"], f["definition"]["textures"][0]["width"], f["definition"]["textures"][0]["height"])
                 for f in data["fonts"].values()]
    textures += [(t["rgba"], t["width"], t["height"])
                 for group in ("thumbnails", "buttons") for t in data.get(group, {}).values()]
    files = {manifest: hashlib.sha256(read_file(root / manifest)).hexdigest()}
    for relative, width, height in textures:
        path = (root / relative).resolve()
        if not path.is_relative_to(root.resolve()):
            raise ValueError(f"Core menu path escapes cache: {relative}")
        if width <= 0 or height <= 0 or path.stat().st_size != width * height * 4:
            raise ValueError(f"Invalid RGBA size: {path}")
        files[relative] = hashlib.sha256(read_file(path)).hexdigest()
    return files


def install(assets: Path, cache: Path, folder: str = "core-menu", manifest: str = MANIFEST,
            kind: str = "skate3-core-menu") -> dict:
    """Copy runtime files into <assets>/private/<folder>, verifying hashes."""
    destination = (assets / "private" / folder).resolve()
    files = runtime_files(cache, manifest, kind)
    for relative, sha256 in files.items():
        target = (destination / relative).resolve()
        if not target.is_relative_to(destination):
            raise ValueError(f"Core menu path escapes install: {relative}")
        if target.exists() and hashlib.sha256(target.read_bytes()).hexdigest() == sha256:
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        write_file(target, read_file(cache / relative))  # shutil.copyfile's bytes
        if hashlib.sha256(read_file(target)).hexdigest() != sha256:
            raise ValueError(f"Installed core menu hash mismatch: {target}")
    return files


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--game", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True, help="Private extraction workspace")
    p.add_argument("--collections", type=Path, required=True,
                   help="Owned private/stock/skater-collections.json")
    p.add_argument("--install", type=Path, help="Installed asset root to copy runtime files into")
    args = p.parse_args()
    prepare(args.game.resolve(), args.output.resolve(), args.collections.resolve())
    if args.install:
        files = install(args.install.resolve(), args.output.resolve())
        print(f"Installed {len(files)} verified core-menu files into {args.install / 'private/core-menu'}")
