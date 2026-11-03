"""Prepare the original loading screen (main/tiploader) from the owned game.

No executable/game launch. Output contains copyrighted assets and stays private.
The screen and its imported controls are merged exactly as the Start menu is
(prepare_core_menu). Its Coach Frank and "Did you know?" tips are the language
table's ID_TIPS_COACHFRANK_n and ID_HINTS_TITLE_n/ID_HINTS_TEXT_n strings, which
the manifest already carries. The corner loading wheel is the shared busy tag
(controls/autosave), prepared beside it as its own movie.
"""
from pathlib import Path
import argparse

import prepare_core_menu as screen

MAIN = "data/fe/source/screens/main/tiploader"
MANIFEST = "runtime/tiploader.json"
KIND = "skate3-tip-loader"
FOLDER = "tip-loader"
WHEEL = "data/fe/source/controls/autosave"
WHEEL_MANIFEST = "runtime/autosave.json"
WHEEL_KIND = "skate3-busy-tag"


def prepare(game: Path, output: Path, collections: Path) -> dict:
    import json
    from prepare_create_skater import _glyphs
    screen.prepare(game, output, collections, WHEEL, WHEEL_MANIFEST, WHEEL_KIND)
    result = screen.prepare(game, output, collections, MAIN, MANIFEST, KIND)
    # The controller glyphs the tips' <B:...> tags name (apt_scene draws them inline).
    result["buttons"] = _glyphs(game, output)
    screen.write_text(output / MANIFEST, json.dumps(result, separators=(",", ":")) + "\n")
    return result


def runtime_files(root: Path) -> dict:
    return screen.runtime_files(root, WHEEL_MANIFEST, WHEEL_KIND) | screen.runtime_files(root, MANIFEST, KIND)


def install(assets: Path, cache: Path) -> dict:
    return (screen.install(assets, cache, FOLDER, WHEEL_MANIFEST, WHEEL_KIND)
            | screen.install(assets, cache, FOLDER, MANIFEST, KIND))


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
