"""Prepare retail Skate 3's front-end screens the crossbar opens (crates/skate-game/src/retail_ui.rs) from the owned disc:
Learn > Trick Guide (screens/tricks/trickguide), Options > Extras (screens/main/extras, extras/credits, nucleus/nucleus_tos), Game Settings >
Video Calibration (options/videocal, popup/sk8popup) and the movie subtitles (main/subtitle), with the data their natives
answer from (`data`: subtitles, credits, licence).

No executable/game launch. Output contains copyrighted assets and stays private (zero assets in releases); the launcher's
`retail-screens` stage runs this with --install. Every screen is merged exactly as the Start menu is (prepare_core_menu);
some only exist in fedynamic.big, so the extraction includes it.
"""
from pathlib import Path
import argparse
import functools
import hashlib
import json
import shutil
import struct

import prepare_core_menu as screen
from prepare_create_skater import _glyphs
from prepare_online_ui import shadow_foreground
from vendor.skate3_ui import project
from vendor.skate3_ui.big import BigArchive
from vendor.skate3_ui.project import extract_project

FOLDER = "retail-screens"
KIND = "skate3-retail-screen"
SOURCE = "data/fe/source/"
SCREENS = {
    "trickguide": "screens/tricks/trickguide",
    "extras": "screens/main/extras",
    "credits": "screens/extras/credits",
    "videocal": "screens/options/videocal",
    "subtitle": "screens/main/subtitle",
    # Video Calibration's "Video calibration complete." question.
    "sk8popup": "screens/popup/sk8popup",
    # Extras > Licenses: the TermsOfServiceManager's text window (0x8263A930).
    "nucleus_tos": "screens/nucleus/nucleus_tos",
}
# Data the screens' natives answer from, beside the manifests.
SUBTITLES = "runtime/subtitles.json"
CREDITS = "runtime/credit-lines.json"
LICENSE = "runtime/license.txt"
DATA = (SUBTITLES, CREDITS, LICENSE)
# The .sub cue that blanks the line, and the terminator's time.
CLEAR, END = 0x1A20BA, 0xFFFF


def manifest(name: str) -> str:
    return f"runtime/{name}.json"


def _keyed(data: bytes) -> dict:
    """A language table as key -> string (u32 count @8, pool @16, (key, offset) pairs from 28)."""
    count, _, pool = struct.unpack_from("<III", data, 8)
    rows = {}
    for i in range(count):
        key, offset = struct.unpack_from("<II", data, 28 + i * 8)
        start = pool + 8 + offset
        rows[key] = data[start:data.index(b"\0", start)].decode("latin-1")
    return rows


def _text(value: str) -> str:
    # '^' breaks the line. ponytail: the tables' bytes 0x81 / 0x82 stand where the lines read an apostrophe and an
    # ellipsis; the fonts carry U+2019 / U+2026 and nothing at 0x81 / 0x82. The game's own byte -> glyph table is
    # not traced.
    return value.replace("^", "\n").replace("\x81", "’").replace("\x82", "…")


def data(game: Path, output: Path) -> None:
    """subtitles.json: every English .sub (data/fe/subtitles/subtitles.big, read by 0x8263E368 and stepped by
    0x8263E6B0) as {lowercase name: [[seconds, text], ...]}: 8-byte records (u16 LE time in 1/10 s, pad, u32 LE string
    key) each showing its line from its time, "" for the blanking key, up to the 0xFFFF terminator.
    credits.json: data/credits/credits.txt (FECredits 0x825AAF68..), one [offset, font size, spacing, text] per
    line; "#ID_..." lines are localised as GetLine's "#ID" strip lets the movie do, other "#" lines are literal.
    license.txt: the English WebKit licence Extras > Licenses shows (0x82655478)."""
    big = project.find_big_directory(game)
    boot = BigArchive(big / "miscboot.big")
    read = {e.path.lower(): e for e in boot.entries}
    labels = _keyed(boot.read(read["data/fe/languages/labels/language_labels_global_skate3ng.bin"]))
    values = _keyed(boot.read(read["data/fe/languages/english/language_english_global_skate3ng.bin"]))
    by_label = {labels[k].strip(): v for k, v in values.items() if k in labels}
    subs = BigArchive(game / "data/fe/subtitles/subtitles.big")
    cues = {}
    for entry in subs.entries:
        if "/subtitles_english/" not in entry.path.lower():
            continue
        raw, lines = subs.read(entry), []
        for at in range(0, len(raw) - 7, 8):
            time, _, key = struct.unpack_from("<HHI", raw, at)
            if time == END:
                break
            lines.append([time / 10, "" if key == CLEAR else _text(values.get(key, ""))])
        cues[Path(entry.path).stem.lower()] = lines
    misc = BigArchive(big / "miscload.big")
    files = {e.path.lower(): e for e in misc.entries}
    credits = []
    for line in misc.read(files["data/credits/credits.txt"]).decode("latin-1").splitlines():
        head, _, text = line.partition('"')
        if not text:
            continue
        offset, size, spacing = head.split()
        text = text.rsplit('"', 1)[0]
        text = by_label.get(text[1:], text[1:]) if text.startswith("#ID") else text.removeprefix("#")
        credits.append([int(offset), int(size), float(spacing), _text(text)])
    for rel, payload in ((SUBTITLES, json.dumps(cues, separators=(",", ":"))),
                         (CREDITS, json.dumps(credits, separators=(",", ":")))):
        (output / rel).write_text(payload + "\n", encoding="utf-8")
    (output / LICENSE).write_bytes(misc.read(files["data/license/webkit_license_en.txt"]))


def _blank_unresolved(result: dict) -> None:
    """trickguide's mTrickname ("Laser Flip", HelveticaPresto) is an authoring label no script or native writes. The
    game draws a family it has no record for with its "debug" record (FontManager lookup 0x82809208), which is no
    front-end bank, so nothing shows: the field keeps its place with no text, over a font the movie has."""
    missing = set(result["unresolved_fonts"])
    if not missing:
        return
    stand_in = next(name for name, asset in result["fonts"].items() if asset is not None)
    ids = {c["id"] for c in result["characters"] if c["type_name"] == "font" and c["font"]["name"] in missing}
    for c in result["characters"]:
        if c.get("text") and c["text"].get("font_id") in ids:
            c["text"]["initial_text"] = ""
    for name in missing:
        result["fonts"][name] = result["fonts"][stand_in]
        result["font_mappings"][name] = result["font_mappings"].get(stand_in)
    result["unresolved_fonts"] = []


def _debug_fallback(result: dict) -> None:
    """A family with no font record draws with the "debug" record (FontManager 0x82809208's second pass looks it up by
    name): credits' embedded Arial (the debug bank itself) and the Helvetica LT Std its setTextFormat asks for."""
    debug = next((m for m in result["font_mappings"].values() if m and m.get("file_name") == "debug"), None)
    for name in result["fonts"]:
        if debug and not result["font_mappings"].get(name):
            result["font_mappings"][name] = debug


def prepare(game: Path, output: Path, collections: Path) -> dict:
    original = screen.extract_project
    screen.extract_project = functools.partial(extract_project, include_dynamic=True)
    results = {}
    try:
        for name, path in SCREENS.items():
            rel = manifest(name)
            result = screen.prepare(game, output, collections, SOURCE + path, rel, KIND)
            _blank_unresolved(result)
            _debug_fallback(result)
            shadow_foreground(result, screen.AssetCache(output).font_asset)
            # The trick guide's gesture_item / button_item2 and every panel's button help draw these.
            result["buttons"] = _glyphs(game, output)
            (output / rel).write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8")
            results[rel] = result
    finally:
        screen.extract_project = original
    data(game, output)
    return results


def runtime_files(root: Path) -> dict:
    files = {}
    for name in SCREENS:
        files.update(screen.runtime_files(root, manifest(name), KIND))
    for rel in DATA:
        files[rel] = hashlib.sha256((root / rel).read_bytes()).hexdigest()
    return files


def install(assets: Path, cache: Path) -> dict:
    files = {}
    for name in SCREENS:
        files.update(screen.install(assets, cache, FOLDER, manifest(name), KIND))
    destination = assets / "private" / FOLDER
    for rel in DATA:
        target = destination / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(cache / rel, target)
        files[rel] = hashlib.sha256(target.read_bytes()).hexdigest()
        if files[rel] != hashlib.sha256((cache / rel).read_bytes()).hexdigest():
            raise ValueError(f"Installed retail-screens hash mismatch: {target}")
    return files


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--game", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True, help="Private extraction workspace")
    p.add_argument("--collections", type=Path, required=True, help="Owned private/stock/skater-collections.json")
    p.add_argument("--install", type=Path, help="Installed asset root to copy runtime files into")
    args = p.parse_args()
    prepare(args.game.resolve(), args.output.resolve(), args.collections.resolve())
    if args.install:
        files = install(args.install.resolve(), args.output.resolve())
        print(f"Installed {len(files)} verified files into {args.install / 'private' / FOLDER}")
