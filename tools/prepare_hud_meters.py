"""Prepare the retail On Screen Help HUDs: the Trick Analyzer (hud2/trickanalyser) and the
Manual Meter (hud2/manualmeter).

No executable/game launch. Output contains copyrighted assets and stays private.
Both movies are merged with their imported controls (gesture_item) exactly as the Start menu
is (prepare_core_menu) and share one output/install folder.
"""
from pathlib import Path
import argparse

import prepare_core_menu as screen

FOLDER = "hud-meters"
KIND = "skate3-hud-meter"
MOVIES = {
    "trickanalyser": "data/fe/source/screens/hud2/trickanalyser",
    "manualmeter": "data/fe/source/screens/hud2/manualmeter",
}
# prepare_runtime_huds.prepare_screen drops this one when the folder can't be refreshed.
MANIFEST = "runtime/trickanalyser.json"


def _screens():
    return [(f"runtime/{name}.json", path) for name, path in MOVIES.items()]


def prepare(game: Path, output: Path, collections: Path) -> dict:
    return {rel: screen.prepare(game, output, collections, path, rel, KIND) for rel, path in _screens()}


def runtime_files(root: Path) -> dict:
    files = {}
    for rel, _ in _screens():
        files.update(screen.runtime_files(root, rel, KIND))
    return files


def install(assets: Path, cache: Path) -> dict:
    files = {}
    for rel, _ in _screens():
        files.update(screen.install(assets, cache, FOLDER, rel, KIND))
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
        print(f"Installed {len(files)} verified files into {args.install / 'private' / FOLDER}")
