"""Prepare retail Skate 3's Replay Editor screens (Skate.Create replay_editor3 + replay_help2, skatereel_tags, sk8popup),
the native timeline's marker/key textures, the export watermark logos and the replay strings.

No executable/game launch. Output contains copyrighted assets and stays private (zero assets in releases).
RE3 and replay_help2 ship in the Skate.Create Upgrade Pack (creator_00000000.big; research Q1), the rest on the disc.
The DLC archive is read as one more UI archive beside fedata/fetexture/miscboot (later archives win).
"""
from pathlib import Path
import argparse
import json
import os
import shutil
import struct

import prepare_core_menu as screen
from prepare_create_skater import _glyphs
from prepare_online_ui import shadow_foreground
from prepare_retail_screens import _blank_unresolved
from vendor.skate3_ui import project
from vendor.skate3_ui.big import BigArchive
from vendor.skate3_ui.rw4 import read_textures
from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant, write_png

FOLDER = "replay-ui"
KIND = "skate3-replay-ui"
DLC_BIG = "creator_00000000.big"
MOVIES = {
    "replay_editor3": "data/fe/source/screens/replay2/replay_editor3",
    "replay_help2": "data/fe/source/screens/replay2/replay_help2",
    "skatereel_tags": "data/fe/source/screens/skatereel/skatereel_tags",
    # Create > skate.Reel > My Replays (Stage 0 §7, crossbar table 0x830279a0): the exe names
    # skatereel_myfootage.swf (0x8220f24c) but no archive carries it; the footage list is CommonBrowser's movie.
    "browser": "data/fe/source/screens/browser/browser",
    "sk8popup": "data/fe/source/screens/popup/sk8popup",
}
# EXE 0x82509298 (timeline) and 0x82507ee8 (watermarks) name these; the files are fedynamic.big's icons and
# miscboot.big's globaltextures.rx2.
ICONS = ("marker_glow", "marker_default", "key_glow", "key_default")
WATERMARKS = ("logo_watermark_easy", "logo_watermark_pro", "logo_watermark")
PREFIXES = ("ID_REPLAY", "ID_SKATE_REEL", "ID_SKATEREEL", "ID_CROSSBAR_SKATEREEL", "ID_CROSSBAR_REPLAY",
            "ID_START_HUB_SKATE_REPLAY")
LANGUAGES = ("English", "French", "German", "Italian", "Spanish", "Dutch", "Japanese")
# Engine strings with no retail id (replay/hud.rs ENGINE_TEXT keeps the same English as its fallback). RULING
# (review M12): the engine's front end is English (prepare_core_menu reads english_global), so the movies carry the
# English table; strings.json keeps every language for a later localised front end.
ENGINE_STRINGS = {
    "ID_S3R_MEDIA_FEATURE_PACK": "Exporting footage needs Windows Media Foundation. Install the Media Feature Pack "
                                 "(Settings > Apps > Optional features), then try again.",
    "ID_S3R_REPLAY_MAP_UNAVAILABLE": "The map this replay was recorded on could not be loaded.",
    "ID_S3R_REPLAY_OFFLINE_ONLY": "Saved replays open in single player only.",
}


def manifest(name: str) -> str:
    return f"runtime/{name}.json"


def _game(disc: Path, dlc: Path, work: Path) -> Path:
    """A game root whose data/big holds the disc's UI archives and the Skate.Create archive (hardlinks)."""
    big = work / "data" / "big"
    shutil.rmtree(work, ignore_errors=True)
    # A read-only hardlink outlives rmtree (deleting it would clear the shared file's read-only flag): its folders stay.
    big.mkdir(parents=True, exist_ok=True)
    source = project.find_big_directory(disc)
    for name in (*project.CORE_ARCHIVES, project.DYNAMIC_ARCHIVE):
        _link(source / name, big / name)
    _link(dlc, big / DLC_BIG)
    return work


def _link(source: Path, target: Path) -> None:
    if target.exists() and os.path.samefile(source, target):  # the same file, left from the last run
        return
    try:
        os.link(source, target)
    except OSError:  # another drive
        shutil.copy2(source, target)


def _language(archive: BigArchive, language: str, table: str) -> dict:
    """label -> value of one language's `table` (Global on miscboot, creator on the DLC)."""
    entries = {e.path.lower(): e for e in archive.entries}
    tail = "skate3ng.bin" if table == "Global" else "skate3ng-dlc_creator.bin"
    labels = entries.get(f"data/fe/languages/labels/language_labels_{table.lower()}_{tail}")
    values = entries.get(f"data/fe/languages/{language.lower()}/language_{language.lower()}_{table.lower()}_{tail}")
    if labels is None or values is None:
        return {}
    pair = [_pool(archive.read(entry)) for entry in (labels, values)]
    return {k.strip(): v for k, v in zip(*pair) if k and v}


def _pool(data: bytes) -> list:
    """The declared strings of a language table (header at 8: count, size 28, pool offset; strings after the pool's
    8-byte descriptor). The Skate.Create tables end with a varying number of empty strings, which the shared
    parse_language_table rejects; only the declared ones are read here."""
    count, size, pool = struct.unpack_from("<3I", data, 8)
    if size != 28:
        raise ValueError(f"language header size {size}")
    return [s.decode("latin-1") for s in data[pool + 8:].split(b"\0")[:count]]


def strings(disc: Path, dlc: Path) -> dict:
    """Every replay key in every language the disc and Skate.Create carry: {language: {key: text}}."""
    boot = BigArchive(project.find_big_directory(disc) / "miscboot.big")
    creator = BigArchive(dlc)
    out = {}
    for language in LANGUAGES:
        table = {**_language(boot, language, "Global"), **_language(creator, language, "creator")}
        rows = {k: v for k, v in table.items() if k.startswith(PREFIXES) or ("MEMCARD" in k and "REPLAY" in k)}
        if rows:
            out[language] = dict(sorted(rows.items()))
    if "English" not in out:
        raise ValueError("No English replay strings")
    return out


def _png(rx2: bytes, name: str, target: Path, scratch: Path) -> None:
    scratch.write_bytes(rx2)
    _, textures = read_textures(scratch)
    texture = next((t for t in textures if t.name.split(".")[0] == name), None)
    if texture is None:
        raise ValueError(f"{name} missing")
    fetch = parse_fetch_constant(bytes.fromhex(texture.metadata["fetch_constant_hex"]))
    write_png(target, fetch.width, fetch.height, decode_rgba(texture.data, fetch))


def textures(disc: Path, output: Path) -> list:
    big = project.find_big_directory(disc)
    dynamic, boot = BigArchive(big / project.DYNAMIC_ARCHIVE), BigArchive(big / "miscboot.big")
    folder, scratch = output / "textures", output / "texture.rx2"
    folder.mkdir(parents=True, exist_ok=True)
    made = []
    for name in ICONS:
        entry = next(e for e in dynamic.entries if e.path.lower() == f"data/fe/source/images/icons/{name}.rx2")
        _png(dynamic.read(entry), name, folder / f"{name}.png", scratch)
        made.append(f"textures/{name}.png")
    globals_ = next(e for e in boot.entries if e.path.lower().endswith("textures/globaltextures.rx2"))
    for name in WATERMARKS:
        _png(boot.read(globals_), name, folder / f"{name}.png", scratch)
        made.append(f"textures/{name}.png")
    scratch.unlink(missing_ok=True)
    return made


def prepare(disc: Path, dlc: Path, output: Path, collections: Path) -> dict:
    game = _game(disc, dlc, output.parent / (output.name + "-game"))
    # The DLC archive is one more UI archive; the extractor reads exactly CORE_ARCHIVES.
    core = project.CORE_ARCHIVES
    project.CORE_ARCHIVES = (*core, DLC_BIG)
    try:
        table = strings(disc, dlc)
        english = table["English"]
        results = {}
        for name, path in MOVIES.items():
            rel = manifest(name)
            result = screen.prepare(game, output, collections, path, rel, KIND)
            shadow_foreground(result, screen.AssetCache(output).font_asset)
            result["buttons"] = _glyphs(game, output)
            # browser's HelveticaPresto / Marker_streak labels: the game draws an unknown family with its "debug"
            # record, so nothing shows (prepare_retail_screens' ruling).
            _blank_unresolved(result)
            # RE3's Skate.Create keys (ID_REPLAYEDITOR_EXIT, the channel names, ...) are not in the global table.
            result["language"] = {**english, **ENGINE_STRINGS, **result["language"]}
            (output / rel).write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8")
            results[rel] = result
    finally:
        project.CORE_ARCHIVES = core
        shutil.rmtree(game, ignore_errors=True)
    (output / "strings.json").write_text(json.dumps(table, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    results["textures"] = textures(disc, output)
    return results


def install(assets: Path, cache: Path) -> dict:
    files = {}
    for name in MOVIES:
        files.update(screen.install(assets, cache, FOLDER, manifest(name), KIND))
    destination = assets / "private" / FOLDER
    for rel in ["strings.json", *(f"textures/{n}.png" for n in (*ICONS, *WATERMARKS))]:
        (destination / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(cache / rel, destination / rel)
        files[rel] = rel
    return files


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--game", type=Path, required=True, help="Extracted Skate 3 disc")
    p.add_argument("--dlc", type=Path, required=True, help="Skate.Create Upgrade Pack creator_00000000.big")
    p.add_argument("--output", type=Path, required=True, help="Private extraction workspace")
    p.add_argument("--collections", type=Path, required=True,
                   help="Owned private/stock/skater-collections.json")
    p.add_argument("--install", type=Path, help="Installed asset root to copy runtime files into")
    args = p.parse_args()
    prepare(args.game.resolve(), args.dlc.resolve(), args.output.resolve(), args.collections.resolve())
    if args.install:
        files = install(args.install.resolve(), args.output.resolve())
        print(f"Installed {len(files)} verified files into {args.install / 'private' / FOLDER}")
