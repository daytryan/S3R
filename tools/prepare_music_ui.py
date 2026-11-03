"""Prepare the retail music screens: the now-playing chyron, the Game Settings options screen (Audio Settings)
and the Music Player.

No executable/game launch. Output contains copyrighted assets and stays private.
The three movies share one output/install folder (one canvas root), as prepare_online_ui's do.
The music itself is not extracted: the game streams data/audio/music/Ipod_Stream.mus from the owned disc.
"""
from pathlib import Path
import argparse
import json

import prepare_core_menu as screen
from prepare_create_skater import _glyphs
from prepare_online_ui import shadow_foreground

FOLDER = "music-ui"
KIND = "skate3-music-ui"
MOVIES = {
    "chyron": "data/fe/source/screens/main/chyron",
    "options": "data/fe/source/screens/options/options",
    "musicplayertest": "data/fe/source/screens/musicplayer/musicplayertest",
}
# prepare_runtime_huds.prepare_screen drops this one when the folder can't be refreshed.
MANIFEST = "runtime/chyron.json"


def _screens():
    return [(f"runtime/{name}.json", path) for name, path in MOVIES.items()]


def prepare(game: Path, output: Path, collections: Path) -> dict:
    results = {}
    for rel, path in _screens():
        result = screen.prepare(game, output, collections, path, rel, KIND)
        shadow_foreground(result, screen.AssetCache(output).font_asset)
        result["buttons"] = _glyphs(game, output)
        screen.write_text(output / rel, json.dumps(result, separators=(",", ":")) + "\n")
        results[rel] = result
    return results


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
