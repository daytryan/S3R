"""Prepare the original Create-a-Skater screen (cas/createskater) and its part thumbnails.

No executable/game launch. Output contains copyrighted assets and stays private.
The screen and its imported controls are merged exactly as the Start menu is
(prepare_core_menu). Part thumbnails come from fedynamic.big
(data/fe/cas/<gender>/<slot>/<model>_<material>.rx2); the runtime resolves a part
to its thumbnail by that authored name. Brand logos (data/fe/cas/logos) are the
info panel's sponsor logos.
"""
from pathlib import Path
import argparse
import json
import struct
import zlib

from vendor.skate3_ui.project import extract_project
from vendor.skate3_ui.binary import read_text, write_file, write_text
import prepare_core_menu as screen

MAIN = "data/fe/source/screens/cas/createskater"
MANIFEST = "runtime/createskater.json"
KIND = "skate3-create-skater"
FOLDER = "create-skater"
THUMBS = "data/fe/cas/"
# Confirmation popups (gender switch, tattoo overwrite): popup/sk8popup, its own
# merged manifest in a subfolder so both movies keep independent character ids.
POPUP = "data/fe/source/screens/popup/sk8popup"
POPUP_MANIFEST = "popup/runtime/sk8popup.json"
POPUP_KIND = "skate3-popup"
# The native button renderer's Xbox 360 glyphs; the disc ships them as PNGs.
GLYPHS = "data/webkit/Content/images/Buttons/XBL2/"


def png_rgba(data: bytes) -> tuple[int, int, bytes]:
    """8-bit RGBA, non-interlaced PNG (the only kind GLYPHS contains)."""
    if data[:8] != bytes.fromhex("89504e470d0a1a0a"):
        raise ValueError("Not a PNG")
    at, idat, header = 8, b"", None
    while at < len(data):
        length, kind = struct.unpack(">I4s", data[at:at + 8])
        body = data[at + 8:at + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        at += 12 + length
    width, height, depth, colour, _, _, interlace = header
    if (depth, colour, interlace) != (8, 6, 0):
        raise ValueError(f"Unsupported PNG layout {depth}/{colour}/{interlace}")
    raw, stride, out, previous = zlib.decompress(idat), width * 4, bytearray(), bytearray(width * 4)
    for y in range(height):
        kind, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for x in range(stride):
            a = line[x - 4] if x >= 4 else 0
            b = previous[x]
            c = previous[x - 4] if x >= 4 else 0
            if kind == 1:
                line[x] = (line[x] + a) & 255
            elif kind == 2:
                line[x] = (line[x] + b) & 255
            elif kind == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif kind == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        out += line
        previous = line
    return width, height, bytes(out)


def _glyphs(game: Path, output: Path) -> dict:
    from vendor.skate3_ui.big import BigArchive
    from vendor.skate3_ui.project import find_big_directory
    archive = BigArchive(find_big_directory(game) / "miscboot.big")
    glyphs = {}
    for entry in archive.entries:
        if entry.path.startswith(GLYPHS) and entry.path.endswith(".png"):
            name = entry.path[len(GLYPHS):-4].removeprefix("button_")
            width, height, rgba = png_rgba(archive.read(entry))
            relative = f"runtime/buttons/{name}.rgba"
            (output / relative).parent.mkdir(parents=True, exist_ok=True)
            write_file(output / relative, rgba)
            glyphs[name] = {"rgba": relative, "width": width, "height": height}
    if not glyphs:
        raise ValueError("Xbox 360 button glyphs missing")
    return glyphs


def prepare(game: Path, output: Path, collections: Path) -> dict:
    result = screen.prepare(game, output, collections, MAIN, MANIFEST, KIND)
    work = output / "thumbs"
    extract_project(game, work, include_dynamic=True, prefixes=(THUMBS,), update=True)
    thumbnails = {}
    for manifest in sorted((work / "assets" / THUMBS).rglob("manifest.json")):
        folder = manifest.parent
        data = json.loads(read_text(manifest))
        textures = [t for t in data["textures"] if t.get("rgba_file")]
        if textures:
            t = textures[0]
            key = folder.relative_to(work / "assets" / THUMBS).as_posix().lower()
            thumbnails[key] = {"rgba": (folder / t["rgba_file"]).relative_to(output).as_posix(),
                               "width": t["width"], "height": t["height"]}
    if not thumbnails:
        raise ValueError("No Create-a-Skater part thumbnails decoded")
    result["thumbnails"] = thumbnails
    result["buttons"] = _glyphs(game, output)
    popup = screen.prepare(game, output / "popup", collections, POPUP, "runtime/sk8popup.json", POPUP_KIND)
    popup["buttons"] = _glyphs(game, output / "popup")
    write_text(output / "popup" / "runtime/sk8popup.json", json.dumps(popup, separators=(",", ":")) + chr(10))
    write_text(output / MANIFEST, json.dumps(result, separators=(",", ":")) + "\n")
    print(f"Prepared {len(thumbnails)} Create-a-Skater part thumbnails")
    return result


def runtime_files(root: Path) -> dict:
    files = screen.runtime_files(root, MANIFEST, KIND)
    popup = screen.runtime_files(root / "popup", "runtime/sk8popup.json", POPUP_KIND)
    files.update({f"popup/{relative}": sha for relative, sha in popup.items()})
    return files


def install(assets: Path, cache: Path) -> dict:
    files = screen.install(assets, cache, FOLDER, MANIFEST, KIND)
    popup = screen.install(assets, cache / "popup", f"{FOLDER}/popup", "runtime/sk8popup.json", POPUP_KIND)
    files.update({f"popup/{relative}": sha for relative, sha in popup.items()})
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
