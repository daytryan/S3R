"""Prepare the original Challenge Map screen (map/challengemap) and its world map.

No executable/game launch. Output contains copyrighted assets and stays private.
The screen and its imported controls are merged exactly as the Start menu is
(prepare_core_menu). The map image comes from fedynamic.big
(data/fe/source/images/map/<name>.rx2); the runtime resolves the file name the
movie asks ImageRenderer_Load for. The location marker is the native map
renderer's icon (data/fe/source/images/icons/mini_teleport.rx2).
"""
from pathlib import Path
import argparse
import json

from vendor.skate3_ui.project import extract_project
import prepare_core_menu as screen
from prepare_create_skater import _glyphs

MAIN = "data/fe/source/screens/map/challengemap"
MANIFEST = "runtime/challengemap.json"
KIND = "skate3-challenge-map"
FOLDER = "challenge-map"
IMAGES = "data/fe/source/images/map/"
# The Port Carverton overview, the skate.School map, and the skate.Park previews
# for the parks this engine can load.
MAPS = ("map1024", "map_skateschool", "map_skatepark_downtown_full", "map_skatepark_basic",
        "map_skatepark_industrial_full", "map_skatepark_stadium_full")
ICONS = "data/fe/source/images/icons/"
# Location photos for the info panel (challenge_map.rs DETAILS).
LOCATIONS = "data/fe/source/images/locations/"
MARKER = "mini_teleport"


def prepare(game: Path, output: Path, collections: Path) -> dict:
    result = screen.prepare(game, output, collections, MAIN, MANIFEST, KIND)
    work = output / "images"
    prefixes = tuple(IMAGES + name for name in MAPS) + (ICONS + MARKER, LOCATIONS)
    extract_project(game, work, include_dynamic=True, prefixes=prefixes, update=True)
    thumbnails = {}
    photos = [(f"{d.name.lower()}.rx2", LOCATIONS + d.name) for d in (work / "assets" / LOCATIONS).iterdir() if d.is_dir()]
    for key, folder in [(f"{name}.rx2", IMAGES + name) for name in MAPS] + [(MARKER, ICONS + MARKER)] + photos:
        folder = work / "assets" / folder
        data = json.loads(screen.read_text(folder / "manifest.json"))
        t = next(t for t in data["textures"] if t.get("rgba_file"))
        thumbnails[key] = {"rgba": (folder / t["rgba_file"]).relative_to(output).as_posix(),
                           "width": t["width"], "height": t["height"]}
    result["thumbnails"] = thumbnails
    # The button bar (A Select, B Back, Y Legend) draws the native glyphs.
    result["buttons"] = _glyphs(game, output)
    screen.write_text(output / MANIFEST, json.dumps(result, separators=(",", ":")) + "\n")
    print(f"Prepared the Challenge Map screen with {len(MAPS)} map images and its marker")
    return result


def runtime_files(root: Path) -> dict:
    return screen.runtime_files(root, MANIFEST, KIND)


def install(assets: Path, cache: Path) -> dict:
    return screen.install(assets, cache, FOLDER, MANIFEST, KIND)


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
        print(f"Installed {len(files)} verified files into {args.install / 'private' / FOLDER}")
