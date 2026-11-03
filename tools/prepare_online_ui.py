"""Prepare the retail online session screens (event log, roster HUD, Player List, confirm popup).

No executable/game launch. Output contains copyrighted assets and stays private.
All four movies share one output/install folder (and so one canvas root).
"""
from pathlib import Path
import argparse
import json

import prepare_core_menu as screen
from prepare_create_skater import _glyphs

FOLDER = "online-ui"
KIND = "skate3-online-ui"
POPUP_KIND = "skate3-popup"
MOVIES = {
    "hud_online_eventlog2": "data/fe/source/screens/hud2/hud_online_eventlog2",
    "leaderboardhud": "data/fe/source/screens/hud2/leaderboardhud",
    "friends": "data/fe/source/screens/online/friends",
}
POPUP = "data/fe/source/screens/popup/sk8popup"
POPUP_MANIFEST = "runtime/sk8popup.json"


def manifest(name: str) -> str:
    return f"runtime/{name}.json"


def _screens():
    """(manifest path, movie path, kind) for all four screens."""
    return [(manifest(n), p, KIND) for n, p in MOVIES.items()] + [(POPUP_MANIFEST, POPUP, POPUP_KIND)]


def shadow_foreground(result: dict, font_asset) -> None:
    """The game draws Futura Shadow as a black pass plus futuraheavy (apt_text needs a futuraheavy font
    character in the manifest). hud_online_eventlog2 only imports Futura Shadow: add the futuraheavy family."""
    fonts = [c for c in result["characters"] if c["type_name"] == "font"]
    if not any(c["font"]["name"] == "Futura Shadow" for c in fonts):
        return
    heavy = next((name for name, m in result["font_mappings"].items() if m.get("file_name") == "futuraheavy"), None)
    if heavy is None:
        raise ValueError("No font family maps to futuraheavy")
    if any(c["font"]["name"] == heavy for c in fonts):
        return
    asset = font_asset(heavy)
    if asset is None:
        raise ValueError(f"Missing original {heavy} font")
    result["characters"].append({"id": max(c["id"] for c in result["characters"]) + 1, "type": 3,
                                 "type_name": "font", "font": {"name": heavy, "glyph_character_ids": []}})
    result["fonts"][heavy] = asset


def prepare(game: Path, output: Path, collections: Path) -> dict:
    results = {}
    for rel, path, kind in _screens():
        result = screen.prepare(game, output, collections, path, rel, kind)
        shadow_foreground(result, screen.AssetCache(output).font_asset)
        result["buttons"] = _glyphs(game, output)
        screen.write_text(output / rel, json.dumps(result, separators=(",", ":")) + "\n")
        results[rel] = result
    return results


def runtime_files(root: Path) -> dict:
    files = {}
    for rel, _, kind in _screens():
        files.update(screen.runtime_files(root, rel, kind))
    return files


def install(assets: Path, cache: Path) -> dict:
    files = {}
    for rel, _, kind in _screens():
        files.update(screen.install(assets, cache, FOLDER, rel, kind))
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
