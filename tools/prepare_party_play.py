"""Prepare Party Play from the San Van Party Pack (STFS 8576AE23, play_00000000.big): the setup screen (partyplay,
FE slot 31 / screen 67), the "Player N ready! Press A to begin." card (partyplay_start, FE slot 32 / screen 68), the
disc's confirm popup (sk8popup: Exit Party Play / Rematch) and results dialog (endchallengeoptions), on the same
canvas root, the
pack's English string table (ID_PARTY_PLAY_*, ID_PARTYPLAY_*, its DLC challenge titles) and its audio zone file
(data/audio/dlc_party.ems: 197 72-byte .ems zone boxes for the pack's districts, read by world audio, not by Party
Play; research.md 4.5 settled).

No executable/game launch. Output contains copyrighted assets and stays private. The two movies only exist in the
pack; the controls they import (panel, highlight_selector), the fonts, the dimmer and the button glyphs come from the
disc. Each movie is added to the disc's UI cache as one more bundle (exactly as extract_project would add it), then
merged by prepare_core_menu.prepare like every other screen.
"""
from pathlib import Path
import argparse
import hashlib
import json
import shutil
import struct

import prepare_core_menu as screen
from prepare_create_skater import _glyphs
from prepare_online_ui import shadow_foreground

FOLDER = "party-play"
KIND = "skate3-party-play"
SCREENS = "data/fe/source/screens/partyplay/"
MOVIES = {"partyplay": SCREENS + "partyplay", "partyplay_start": SCREENS + "partyplay_start"}
LABELS = "data/fe/languages/labels/LANGUAGE_Labels_play_skate3ng-dlc_play.BIN"
ENGLISH = "data/fe/languages/english/LANGUAGE_English_play_skate3ng-dlc_play.BIN"
AUDIO = "data/audio/dlc_party.ems"
AUDIO_OUT = "audio/dlc_party.ems"
# The disc's confirm popup (Exit Party Play 213, Rematch 217), on the same canvas root as the party screens.
POPUP = "data/fe/source/screens/popup/sk8popup"
POPUP_MANIFEST = "runtime/sk8popup.json"
POPUP_KIND = "skate3-popup"
# The disc's results dialog (OpenContinueRetryDialog, FE slot 4): its end options are the A / B / X legends, Party
# Play's Rematch / Quit / Change settings.
RESULTS = "data/fe/source/screens/hudcomponents/endchallengeoptions"
RESULTS_MANIFEST = "runtime/endchallengeoptions.json"
DISC_SCREENS = ((POPUP, POPUP_MANIFEST, POPUP_KIND), (RESULTS, RESULTS_MANIFEST, KIND))


def manifest(name: str) -> str:
    return f"runtime/{name}.json"


def _entries(archive) -> dict:
    return {e.path.replace("\\", "/").lower(): e for e in archive.entries}


def add_bundle(output: Path, archive, stem: str) -> None:
    """The pack's movie `stem` as one more bundle of the UI cache at `output` (extract_project's own steps)."""
    from vendor.skate3_ui.apt import inspect_apt
    from vendor.skate3_ui.geo import parse_geo
    from vendor.skate3_ui.rw4 import extract_textures

    entries = _entries(archive)
    parts = {}
    for ext in (".apt", ".const", ".geo", ".rx2"):
        entry = entries.get(stem + ext)
        if entry is not None:
            target = output / "raw" / (stem + ext)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(archive.read(entry))
            parts[ext] = target
    if ".apt" not in parts or ".const" not in parts:
        raise ValueError(f"the San Van Party Pack lacks {stem}.apt/.const")
    rel = lambda p: p.relative_to(output).as_posix()
    record = {"name": stem, "files": {ext[1:]: rel(p) for ext, p in sorted(parts.items())}}
    meta = output / "metadata" / stem
    meta.parent.mkdir(parents=True, exist_ok=True)
    apt = inspect_apt(parts[".apt"], parts[".const"])
    (meta.with_suffix(".apt.json")).write_text(json.dumps(apt, indent=2) + "\n", encoding="utf-8")
    movie = (apt.get("root") or {}).get("movie", {})
    record["timeline"] = {"metadata": rel(meta.with_suffix(".apt.json")), "frames": movie.get("frame_count", 0),
                          "milliseconds_per_frame": movie.get("milliseconds_per_frame", 0),
                          "characters": len(apt.get("characters", []))}
    if ".geo" in parts:
        geo = parse_geo(parts[".geo"])
        (meta.with_suffix(".geo.json")).write_text(json.dumps(geo, indent=2) + "\n", encoding="utf-8")
        record["geometry"] = {"metadata": rel(meta.with_suffix(".geo.json")), "shapes": len(geo.get("shapes", []))}
    if ".rx2" in parts:
        folder = output / "assets" / stem
        textures = extract_textures(parts[".rx2"], folder, force=True, decode_png=True)
        (folder / "manifest.json").write_text(json.dumps(textures, indent=2) + "\n", encoding="utf-8")
        record["textures"] = {"manifest": rel(folder / "manifest.json"), "platform": textures["platform"],
                              "count": len(textures["textures"])}
    path = output / "manifest.json"
    cache = json.loads(path.read_text(encoding="utf-8"))
    cache["bundles"] = [b for b in cache["bundles"] if b["name"].lower() != stem] + [record]
    path.write_text(json.dumps(cache, indent=2) + "\n", encoding="utf-8")


def _table(data: bytes) -> dict:
    """A language table, {string id: text}: the id/offset index after the 28-byte header. The pack's tables end in
    padding, which the disc tables' pool-split reader (vendor language.py) refuses."""
    count, _, pool = struct.unpack_from("<III", data, 8)
    rows = {}
    for i in range(count):
        key, offset = struct.unpack_from("<II", data, 28 + i * 8)
        start = pool + 8 + offset
        rows[key] = data[start:data.index(0, start)].decode("latin-1")
    return rows


def strings(archive) -> dict:
    """The pack's English table, {label: text}."""
    entries = _entries(archive)
    labels, values = (_table(archive.read(entries[name.lower()])) for name in (LABELS, ENGLISH))
    return {labels[k].strip(): values[k] for k in labels if k in values and labels[k].strip()}


def prepare(game: Path, dlc: Path, output: Path, collections: Path) -> dict:
    from vendor.skate3_ui.big import BigArchive
    from vendor.skate3_ui.project import extract_project
    archive = BigArchive(dlc)
    language = strings(archive)
    results = {}
    for name, path in MOVIES.items():
        # The disc's cache for this movie's prefixes, then the movie itself; screen.prepare reuses that cache.
        extract_project(game, output, prefixes=(path, screen.DIMMER, "data/fe/source/controls/"), update=True)
        add_bundle(output, archive, path)
        rel = manifest(name)
        result = screen.prepare(game, output, collections, path, rel, KIND)
        shadow_foreground(result, screen.AssetCache(output).font_asset)
        result["buttons"] = _glyphs(game, output)
        result["language"].update(language)
        (output / rel).write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8")
        results[rel] = result
    for path, rel, kind in DISC_SCREENS:
        extract_project(game, output, prefixes=(path, "data/fe/source/controls/"), update=True)
        result = screen.prepare(game, output, collections, path, rel, kind)
        shadow_foreground(result, screen.AssetCache(output).font_asset)
        result["buttons"] = _glyphs(game, output)
        result["language"].update(language)
        (output / rel).write_text(json.dumps(result, separators=(",", ":")) + "\n", encoding="utf-8")
        results[rel] = result
    (output / AUDIO_OUT).parent.mkdir(parents=True, exist_ok=True)
    (output / AUDIO_OUT).write_bytes(archive.read(_entries(archive)[AUDIO]))
    return results


def runtime_files(root: Path) -> dict:
    files = {}
    for name in MOVIES:
        files.update(screen.runtime_files(root, manifest(name), KIND))
    for _, rel, kind in DISC_SCREENS:
        files.update(screen.runtime_files(root, rel, kind))
    files[AUDIO_OUT] = hashlib.sha256((root / AUDIO_OUT).read_bytes()).hexdigest()
    return files


def install(assets: Path, cache: Path) -> dict:
    """Copy the runtime files into <assets>/private/party-play (the folder is replaced: it is this stage's root)."""
    destination = assets / "private" / FOLDER
    shutil.rmtree(destination, ignore_errors=True)
    files = runtime_files(cache)
    for relative, sha256 in files.items():
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(cache / relative, target)
        if hashlib.sha256(target.read_bytes()).hexdigest() != sha256:
            raise ValueError(f"Installed Party Play hash mismatch: {target}")
    return files


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--game", type=Path, required=True)
    p.add_argument("--dlc", type=Path, required=True, help="The San Van Party Pack's play_00000000.big")
    p.add_argument("--output", type=Path, required=True, help="Private extraction workspace")
    p.add_argument("--collections", type=Path, required=True, help="Owned private/stock/skater-collections.json")
    p.add_argument("--install", type=Path, help="Installed asset root to copy runtime files into")
    args = p.parse_args()
    prepare(args.game.resolve(), args.dlc.resolve(), args.output.resolve(), args.collections.resolve())
    if args.install:
        files = install(args.install.resolve(), args.output.resolve())
        print(f"Installed {len(files)} verified Party Play files into {args.install / 'private' / FOLDER}")
