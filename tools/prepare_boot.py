"""Prepare Skate 3's power-on sequence (ESRB, EA logo, Press Start, autosave warning, the new-career welcome,
team name, first team member, Coach Frank) from the owned disc.

No executable/game launch. Output contains copyrighted assets and stays private.
Every screen is merged exactly as the Start menu is (prepare_core_menu), except that the bootflow movies only
exist in fedynamic.big, so the extraction includes it. The movies are copied as they are (.vp6, played by
ffmpeg) with their EA-XA audio decoded by vgmstream to a stereo 16-bit WAV, scaled by the fe_movie record's
Volume (re/vault_fe_movie.txt: ea_intro / attract_video / intro_movie 0.8, coach_frank_intro 0.6), since the
engine's movie player has no per-movie volume. The camera previews have no audio.
"""
from pathlib import Path
import argparse
import functools
import hashlib
import json
import os
import subprocess
import tempfile
import wave

import numpy as np

import prepare_core_menu as screen
from prepare_create_skater import _glyphs
from prepare_online_ui import shadow_foreground
from vendor.skate3_ui.project import extract_project, find_big_directory
# What this process writes is read back (hashed, installed) from memory: the same bytes, without Defender's scan of
# each fresh file (vendor binary.py).
from vendor.skate3_ui.binary import read_file, read_text, write_file, write_text

FOLDER = "boot"
KIND = "skate3-boot"
POPUP_KIND = "skate3-popup"
SOURCE = "data/fe/source/"
SCREENS = {
    "esrb": "screens/bootflow/esrb",
    "pressstart": "screens/bootflow/pressstart",
    "welcomescreen": "screens/bootflow/welcomescreen",
    "coachfrank": "screens/bootflow/coachfrank",
    "team_management_boot": "screens/TeamSkater_Selection/team_management_boot",
    "fmv_popup": "screens/main/fmv_popup",
    "autosave": "controls/autosave",
    # Edit Skaters (FE slot 63) and Import Skater (slot 68): team_select.rs, also the new career's logo picker.
    "team_management": "screens/TeamSkater_Selection/team_management",
    "import_skater": "screens/TeamSkater_Selection/import_skater",
}
# runtime/team.json (team_select.rs): the team logos, the preset skaters and the team photo setups.
TEAM = "runtime/team.json"
# skatercollections.vlt: team logos (leaves under LOGO_ROOT whose hidden bool is false, by key; 0x828da2c8), their
# texture name (data/content/createacharacter/texture/<name>.rx2, 0x82641840).
LOGO_CLASS, LOGO_ROOT = "Hash_6CBE732C8A2F0FF8", "Hash_DBF972899B38314B"
LOGO_HIDDEN, LOGO_TEXTURE = "Hash_261A8CE4321B29A6", "Hash_C25F26D48CEABD7C"
# Preset teammates (PresetCharacterLoader 0x827d57c8): name string id, SavedRecipe index.
PRESET_CLASS, PRESET_NAME, PRESET_INDEX = "Hash_12BCDBD69BA7D33D", "Hash_8063E6A0DB33E680", "Hash_3F1DD58F86CAAB1E"
# Photo setups (0x82640eb0): camera, target, clear colour, background, width, height. The render target
# (0x8278e960) takes EF42 as its width and 2A7D as its height: team_profile is 128 x 256, as its background art.
PHOTO_CLASS = "Hash_569B02CE038D028A"
PHOTO_FIELDS = {"camera": "Hash_4E6809A1F98D140E", "target": "Hash_C4C58FBAC3110A0A", "colour": "Hash_A9D0E661C487506C",
                "background": "Hash_486F0AA534688275", "width": "Hash_EF42F81B54671521", "height": "Hash_2A7DB174EAAAF951"}
# The photo camera (0x8278f788 -> 0x82802b98): vertical field of view in degrees, near and far.
PHOTO_LENS = {"fov": "Hash_7D4D6707B0A9DBE0", "near": "Hash_DC0918318BABCF5A", "far": "Hash_0D5D86EE00191ACC"}
# Its render location (0x8278fb90; default record: cas_photo, under cas): the sun, given in the camera's basis
# (0x8296efe0: right, up, forward), and the ambient SH rows the character shader takes.
PHOTO_LOCATION, LOCATION_SH = "Hash_7E23D10785C43717", "Hash_F45DFCAB6EC195CD"
# Slot 68's preview uses team_profile_<member> too ("%s_%02u", 0x82653858).
PHOTO_SETUPS = [f"team_profile_{i:02d}" for i in range(5)]
# The setup's two cac_body references (ClassRefSpec_cac_body; equal in every team setup) -> the cac_body record whose
# text field is the editor clip the skater holds: team_profile_00..04 = CAS_Single_Pose_5/2/3/7/8 (cac_edit_<gender>.abin).
PHOTO_POSE_FIELDS = ("Hash_EE2537852C08E316", "Hash_934501958A868D9C")
CAC_BODY_CLASS, CAC_BODY_CLIP = "Hash_95E142284549717F", "Hash_249D0D4624998D68"
# The customiser's face targets in library order (library-v3 `morphs` 2..18): the recipe's body_mods 2..18.
FACE = ["local_brows_depth", "local_brows_height", "local_brows_rotation", "local_chin_length", "local_eye_height",
        "local_eye_rotation", "local_eye_width", "local_jaw_chiseled", "local_jaw_depth", "local_mouth_corner",
        "local_mouth_height", "local_mouth_lipsize", "local_mouth_width", "local_nose_curve", "local_nose_height",
        "local_nose_length", "local_nose_width"]
POPUPS = {"save_warning": "screens/popup/save_warning", "sk8popup": "screens/popup/sk8popup"}
# (fe_movie record file stem, Volume as stored: 0x3F4CCCCD / 0x3F19999A).
MOVIES = {"EA_Blackbox": 0.8, "Attract": 0.8, "intro_movie": 0.8, "Coach_frank_intro": 0.6}
PREVIEWS = ("cameraLow", "cameraHigh")
VGMSTREAM = os.environ.get("SKATE_VGMSTREAM") or "vgmstream-cli"
CHUNK = 1 << 20


def _manifests():
    return [(f"runtime/{n}.json", SOURCE + p, KIND) for n, p in SCREENS.items()] + \
           [(f"runtime/{n}.json", SOURCE + p, POPUP_KIND) for n, p in POPUPS.items()]


# RwAudioCore's 5.1 -> stereo fold-down (exe string 0x820F0528, as player_audio.rs FOLD_DOWN): LFE dropped,
# centre and surrounds at -3 dB into the fronts, the whole mix a further -3 dB.
FOLD = 0.70710678


def _stereo(source: Path, target: Path, volume: float) -> float:
    """6-channel movie audio to stereo by the retail fold-down, scaled by the fe_movie `volume`; no normalisation.
    The SCHl channel order is L C R Ls Rs LFE, measured on all four movies' vgmstream output: ch5 is the LFE
    (centroid ~50 Hz, <2.5% energy above 200 Hz), ch1 carries Coach Frank's dialogue alone (centre), and the
    side pairs correlate ch0~ch3 and ch2~ch4 (Coach_frank_intro 0.55 / 0.49). Chunked: the intro is six minutes."""
    with wave.open(str(source)) as w, wave.open(str(target), "wb") as out:
        channels, rate, frames = w.getnchannels(), w.getframerate(), w.getnframes()
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(rate)
        while data := w.readframes(CHUNK):
            pcm = np.frombuffer(data, "<i2").reshape(-1, channels).astype(np.float32)
            if channels == 6:
                pcm = np.stack([pcm[:, 0] + FOLD * (pcm[:, 1] + pcm[:, 3]), pcm[:, 2] + FOLD * (pcm[:, 1] + pcm[:, 4])], 1) * FOLD
            else:
                pcm = pcm[:, :2] if channels >= 2 else np.repeat(pcm, 2, 1)
            out.writeframes(np.clip(np.round(pcm * volume), -32768, 32767).astype("<i2").tobytes())
    return frames / rate


def _movies(game: Path, output: Path) -> dict:
    movies = find_big_directory(game).parent / "movies"
    result = {}
    (output / "movies").mkdir(parents=True, exist_ok=True)
    for stem, volume in MOVIES.items():
        source = movies / f"{stem}_english_ntsc.vp6"
        write_file(output / f"movies/{stem}.vp6", source.read_bytes())
        with tempfile.TemporaryDirectory(prefix="skate3-boot-movie-") as work:
            decoded = Path(work) / "a.wav"
            subprocess.run([VGMSTREAM, "-s", "1", "-o", str(decoded), str(source)], check=True, capture_output=True)
            seconds = _stereo(decoded, output / f"movies/{stem}.wav", volume)
        result[stem] = {"video": f"movies/{stem}.vp6", "audio": f"movies/{stem}.wav", "seconds": seconds, "volume": volume}
    for stem in PREVIEWS:
        write_file(output / f"movies/{stem}.vp6", (movies / f"{stem}_english_ntsc.vp6").read_bytes())
        result[stem] = {"video": f"movies/{stem}.vp6"}
    return result


def _rows(collections: Path) -> list:
    return json.loads(collections.read_text(encoding="utf-8"))["collections"]


def _inherited(rows: list, cls: str) -> dict:
    """{key: (row, fields with the parents' filled in)} for a vault class."""
    by_key = {r["key"]: r for r in rows if r["class"] == cls}

    def fields(key, seen=()):
        row = by_key.get(key)
        if row is None or key in seen:
            return {}
        return {**fields(row["parent"], seen + (key,)), **row["fields"]}
    return {k: (r, fields(k)) for k, r in by_key.items()}


def _key(key: str) -> int:
    return int(key.removeprefix("Hash_"), 16) if key.startswith("Hash_") else 0


def team_logos(rows: list) -> list:
    """The logo grid (0x828da2c8 / 0x828da0d8): leaf records under LOGO_ROOT not flagged hidden, by 64-bit key."""
    records = _inherited(rows, LOGO_CLASS)
    parents = {r["parent"] for r, _ in records.values()}

    def under(key):
        while key:
            if key == LOGO_ROOT:
                return True
            key = records[key][0]["parent"] if key in records else ""
        return False
    logos = [(k, f) for k, (r, f) in records.items()
             if k != LOGO_ROOT and k not in parents and under(r["parent"])
             and f.get(LOGO_HIDDEN, {}).get("data", "00000000")[:2] == "00"]
    return [{"key": f"{_key(k):016x}", "texture": f[LOGO_TEXTURE]["data"]} for k, f in sorted(logos, key=lambda x: _key(x[0]))]


def _floats(field: dict, n: int) -> list:
    import struct
    return list(struct.unpack(f">{n}f", bytes.fromhex(field["data"])[: 4 * n]))


def photo_setups(rows: list) -> dict:
    import struct
    from asset_pipeline.environment import Collections, key_hash
    records = _inherited(rows, PHOTO_CLASS)
    bodies = _inherited(rows, CAC_BODY_CLASS)
    collections = Collections({"collections": rows})
    setups = {}
    for name in PHOTO_SETUPS:
        f = records[name][1]
        refs = {f"Hash_{f[k]['data'][:16].upper()}" for k in PHOTO_POSE_FIELDS if k in f}
        if len(refs) != 1:
            raise ValueError(f"{name}: pose references {sorted(refs)}")
        location, _ = collections.resolve(*struct.unpack(">2Q", bytes.fromhex(f[PHOTO_LOCATION]["data"])[:16]))
        sh = [_floats({"data": row}, 3) for row in location[key_hash(LOCATION_SH)]["array"]["items"]]
        if len(sh) != 9:
            raise ValueError(f"{name}: {len(sh)} SH rows")
        setups[name] = {
            "pose": bodies[refs.pop()][1][CAC_BODY_CLIP]["data"],
            **{k: _floats(f[v], 1)[0] for k, v in PHOTO_LENS.items()},
            "sun": _floats(location[key_hash("sun_position")], 3),
            "sh": sh,
            "camera": _floats(f[PHOTO_FIELDS["camera"]], 3),
            "target": _floats(f[PHOTO_FIELDS["target"]], 3),
            "colour": _floats(f[PHOTO_FIELDS["colour"]], 4),
            "background": f.get(PHOTO_FIELDS["background"], {}).get("data", ""),
            "width": int(f[PHOTO_FIELDS["width"]]["data"], 16),
            "height": int(f[PHOTO_FIELDS["height"]]["data"], 16),
        }
    return setups


def preset_profile(parsed: dict) -> dict:
    """A SavedRecipe as a customiser profile: the outfit (first asset, its first model's material), the body and face
    values, and the skin and hair tints from the colour table. Skin light/dark follows the head material."""
    selections = {slot: {"asset_id": items[0]["asset_id"], "material_id": items[0]["models"][0]["material_id"]}
                  for slot, items in parsed["asset_lists"].items() if items and items[0]["models"]}
    body = parsed["body_mods"]
    morphs = {"thin": body[0], "fat": body[1], **dict(zip(FACE, body[2:]))}
    profile = {"gender": "male" if parsed["gender"] == 1 else "female", "selections": selections,
               "morphs": {k: round(v, 6) for k, v in morphs.items()}}
    colours = {int(k): v for k, v in parsed.get("colours", {}).items()}
    if 1 in colours:
        profile["hair_tint"] = [round(v, 6) for v in colours[1][:3]]
    if 2 in colours:
        profile["skin_tint"] = [round(v, 6) for v in colours[2][:3]]
    return profile


def _rgba(data: bytes, work: Path, name: str) -> tuple:
    from vendor.skate3_ui.rw4 import parse_textures
    from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant
    texture = parse_textures(data, str(work / f"{name}.rx2"))[1][0]  # read_textures of the arena, without the file
    fetch = parse_fetch_constant(bytes.fromhex(texture.metadata["fetch_constant_hex"]))
    return fetch.width, fetch.height, decode_rgba(texture.data, fetch)


def _team(game: Path, output: Path, collections: Path) -> dict:
    """runtime/team.json plus its textures (runtime/team/*.rgba): logos, photo backgrounds and the preset skaters."""
    import sys
    tools = Path(__file__).resolve().parent
    sys.path[:0] = [str(tools.parent), str(tools)]
    from owned_game.big import BigArchive
    from extract_default_skater import parse_fallback_recipe
    rows = _rows(collections)
    big = find_big_directory(game)
    textures, folder = {}, output / "runtime/team"
    folder.mkdir(parents=True, exist_ok=True)

    def keep(name: str, data: bytes, work: Path):
        width, height, rgba = _rgba(data, work, name)
        write_file(folder / f"{name}.rgba", rgba)
        textures[name] = {"rgba": f"runtime/team/{name}.rgba", "width": width, "height": height}

    logos = team_logos(rows)
    setups = photo_setups(rows)
    presets = sorted(((int(f.get(PRESET_INDEX, {}).get("data", "0") or "0", 16), f[PRESET_NAME]["data"])
                      for k, (_, f) in _inherited(rows, PRESET_CLASS).items() if k != "default" and PRESET_NAME in f))
    with tempfile.TemporaryDirectory(prefix="skate3-team-") as temp:
        work = Path(temp)
        cas = BigArchive(game / "data/content/createacharacter.big")
        wanted = {f"data/content/createacharacter/texture/{l['texture']}.rx2".lower(): l["texture"] for l in logos}
        for entry in cas.entries:
            if entry.path.lower() in wanted:
                keep(wanted[entry.path.lower()], cas.read(entry), work)
        backgrounds = {f"data/{s['background']}.rx2".lower() for s in setups.values() if s["background"]}
        fe = BigArchive(big / "fedynamic.big")
        for entry in fe.entries:
            if entry.path.lower() in backgrounds:
                keep(Path(entry.path).stem.lower(), fe.read(entry), work)
        misc = BigArchive(big / "miscload.big")
        recipes = {e.path.lower(): e for e in misc.entries}
        profiles = []
        for index, name in presets:
            recipe = work / f"SavedRecipe{index}.bin"
            recipe.write_bytes(misc.read(recipes[f"data/cacrecipes/savedrecipe{index}.bin"]))
            profiles.append({"name": name, "recipe": index, "profile": preset_profile(parse_fallback_recipe(recipe, 16))})
    missing = [l["texture"] for l in logos if l["texture"] not in textures]
    if missing or len(textures) != len(logos) + len({s["background"] for s in setups.values() if s["background"]}):
        raise ValueError(f"Team textures missing: {missing}")
    for setup in setups.values():
        setup["background"] = Path(setup["background"]).name.lower()
    team = {"format": KIND, "version": 1, "logos": logos, "presets": profiles, "photos": setups, "thumbnails": textures}
    write_text(output / TEAM, json.dumps(team, indent=1) + "\n")
    return team


def prepare(game: Path, output: Path, collections: Path) -> dict:
    original = screen.extract_project
    screen.extract_project = functools.partial(extract_project, include_dynamic=True)
    try:
        for rel, path, kind in _manifests():
            result = screen.prepare(game, output, collections, path, rel, kind)
            # fmv_popup imports only Futura Shadow; the game draws it over futuraheavy.
            shadow_foreground(result, screen.AssetCache(output).font_asset)
            result["buttons"] = _glyphs(game, output)
            write_text(output / rel, json.dumps(result, separators=(",", ":")) + "\n")
    finally:
        screen.extract_project = original
    _team(game, output, collections)
    index = {"format": KIND, "version": 1, "movies": _movies(game, output)}
    write_text(output / "runtime/boot.json", json.dumps(index, indent=1) + "\n")
    return index


def runtime_files(root: Path) -> dict:
    files = {}
    for rel, _, kind in _manifests():
        files.update(screen.runtime_files(root, rel, kind))
    index = json.loads(read_text(root / "runtime/boot.json"))
    if index.get("format") != KIND or index.get("version") != 1:
        raise ValueError("Invalid boot index")
    files["runtime/boot.json"] = hashlib.sha256(read_file(root / "runtime/boot.json")).hexdigest()
    team = json.loads(read_text(root / TEAM))
    if team.get("format") != KIND or team.get("version") != 1:
        raise ValueError("Invalid team index")
    files[TEAM] = hashlib.sha256(read_file(root / TEAM)).hexdigest()
    for texture in team["thumbnails"].values():
        path = (root / texture["rgba"]).resolve()
        if not path.is_relative_to(root.resolve()) or path.stat().st_size != texture["width"] * texture["height"] * 4:
            raise ValueError(f"Invalid team texture {texture['rgba']}")
        files[texture["rgba"]] = hashlib.sha256(read_file(path)).hexdigest()
    for movie in index["movies"].values():
        for key in ("video", "audio"):
            if key in movie:
                files[movie[key]] = hashlib.sha256(read_file(root / movie[key])).hexdigest()
    return files


def install(assets: Path, cache: Path) -> dict:
    destination = (assets / "private" / FOLDER).resolve()
    files = runtime_files(cache)
    for relative, sha256 in files.items():
        target = (destination / relative).resolve()
        if not target.is_relative_to(destination):
            raise ValueError(f"Boot path escapes install: {relative}")
        if target.exists() and hashlib.sha256(target.read_bytes()).hexdigest() == sha256:
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        write_file(target, read_file(cache / relative))  # shutil.copyfile's bytes
        if hashlib.sha256(read_file(target)).hexdigest() != sha256:
            raise ValueError(f"Installed boot file hash mismatch: {target}")
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
