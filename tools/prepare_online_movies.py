"""Prepare the Skate 3 online movies: online_chyron (OnlineChyronHUD, screen table 0x8302F1A8 row 57, item 30b),
endchallenge_teams (EndChallengeOptions_TeamChallenge, row 35) and challenge_signup (SignUp_Poster, row 23), item 6.

No executable/game launch. Output contains copyrighted assets and stays private (zero assets in releases).
The manifests are drawn on the online layer (apt_screen ONLINE, root private/online-ui), so their payload paths are
rewritten to resolve from there, as tools/skate3_online.py does for the mode HUDs.
"""
from pathlib import Path
import argparse
import json
import os
import shutil

import prepare_core_menu as screen
from prepare_online_ui import shadow_foreground

FOLDER = "online-movies"
KIND = "skate3-online-movie"
LAYER_ROOT = "private/online-ui"
MOVIES = {
    "online_chyron": "data/fe/source/screens/hud2/online_chyron",
    "endchallenge_teams": "data/fe/source/screens/hudComponents/endchallenge_teams",
    "challenge_signup": "data/fe/source/screens/challenge/challenge_signup",
}


def prepare(disc: Path, output: Path, collections: Path) -> dict:
    """{manifest rel: its text, payloads rewritten to resolve from LAYER_ROOT}; the payloads stay in `output`."""
    manifests = {}
    prefix = os.path.relpath(f"private/{FOLDER}", LAYER_ROOT).replace("\\", "/")
    for stem, path in MOVIES.items():
        rel = f"runtime/{stem}.json"
        result = screen.prepare(disc, output, collections, path, rel, KIND)
        if result["unresolved_fonts"]:
            raise ValueError(f"{stem}: fonts {result['unresolved_fonts']}")
        shadow_foreground(result, screen.AssetCache(output).font_asset)
        (output / rel).write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8", newline="\n")
        text = (output / rel).read_text(encoding="utf-8")
        for f in screen.runtime_files(output, rel, KIND):
            if f != rel:
                text = text.replace(json.dumps(f), json.dumps(f"{prefix}/{f}"))
        manifests[rel] = text
    return manifests


def install(assets: Path, output: Path, manifests: dict) -> list:
    destination = assets / "private" / FOLDER
    files = []
    for rel, text in manifests.items():
        for f in screen.runtime_files(output, rel, KIND):
            target = destination / f
            target.parent.mkdir(parents=True, exist_ok=True)
            if f == rel:
                target.write_text(text, encoding="utf-8", newline="\n")
            else:
                shutil.copyfile(output / f, target)
            files.append(f)
    return files


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--disc", type=Path, required=True, help="Extracted Skate 3 disc")
    p.add_argument("--output", type=Path, required=True, help="Private extraction workspace")
    p.add_argument("--collections", type=Path, required=True, help="Owned private/stock/skater-collections.json")
    p.add_argument("--install", type=Path, help="Installed asset root to copy the runtime files into")
    args = p.parse_args()
    made = prepare(args.disc.resolve(), args.output.resolve(), args.collections.resolve())
    if args.install:
        files = install(args.install.resolve(), args.output.resolve(), made)
        print(f"Installed {len(files)} files into {args.install / 'private' / FOLDER}")
