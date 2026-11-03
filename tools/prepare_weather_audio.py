"""Master the user's CC0 Freesound rain and thunder recordings into looped,
loudness-matched OGG Vorbis plus a manifest for the game's weather audio
(docs/superpowers/plans/2026-10-06-weather-audio.md, Task 1).

    py -3 tools/prepare_weather_audio.py --sources "<Rain Sounds>" --output <dir> [--install <assets>]

Writes <output>/weather-audio/{manifest.json,*.ogg}; --install also copies that folder to
<assets>/weather-audio (swapped in by rename). The source folder is only read.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import numpy as np

RATE = 48000
BED_EQ = "highpass=f=90,lowpass=f=12000,equalizer=f=4000:t=q:w=1.0:g=-3"
THUNDER_EQ = "highpass=f=25,silenceremove=start_periods=1:start_threshold=-50dB"
# BlueDelta 446753 is quad (IRT cross): fold front and back pairs.
QUAD_FOLD = "pan=stereo|c0=0.5*c0+0.5*c2|c1=0.5*c1+0.5*c3"
# ponytail: loops are cut from the first 8 minutes; 845501 runs 16:41 (385 MB as f32).
ANALYSIS_SECONDS = 480
LOOP_FADE = 2.0
THUNDER_FADE = 2.0

# role, sources (Freesound ids), treatment, seconds (loop length or one-shot cap), LUFS target
ROLES = [
    ("bed_light", [845501, 171980], "loop", 60, -30.0),
    ("bed_heavy", [778364, 171981], "loop", 60, -30.0),
    ("hard_light", [119500], "loop", 45, -32.0),
    ("hard_heavy", [483580, 202899, 637777], "loop", 45, -32.0),
    ("metal", [611424, 454142], "loop", 45, -32.0),
    ("wood", [611421], "loop", 45, -32.0),
    ("water", [202898], "loop", 45, -32.0),
    ("foliage", [870824, 584268, 584272, 451154], "loop", 45, -32.0),
    # ponytail: 611424's "close mix" is the same best window, mastered 4 dB hotter than as `metal`.
    ("roof_metal", [611424, 171979], "loop", 45, -28.0),
    ("roof_concrete", [171983, 398738], "loop", 45, -28.0),
    ("thunder_crack", [567945, 574412, 534023, 446753], "thunder", 30, -20.0),
    ("thunder_roll", [574416, 717888, 567746, 476738], "thunder", 35, -20.0),
    ("thunder_rumble", [577317, 577315, 778372, 169534], "thunder", 40, -20.0),
]

_FFMPEG = None


def ffmpeg():
    """SKATE_FFMPEG, then <install root>/data/tools/ffmpeg/**/ffmpeg.exe (set by --install), then PATH."""
    return _FFMPEG or os.environ.get("SKATE_FFMPEG") or "ffmpeg"


def _find_install_ffmpeg(assets):
    # <install>/data/installations/<id>/assets -> <install>/data/tools/ffmpeg
    root = Path(assets).resolve().parents[2] / "tools" / "ffmpeg"
    found = sorted(root.glob("**/ffmpeg.exe")) if root.is_dir() else []
    return str(found[0]) if found else None


def _run(args, **kw):
    return subprocess.run([ffmpeg(), "-hide_banner", "-nostdin", *args], capture_output=True, check=True, **kw)


def find_sources(folder):
    """{freesound id: path} for files named `<id>__...`, skipping partial downloads."""
    found = {}
    for entry in sorted(Path(folder).iterdir()):
        m = re.match(r"^(\d+)__", entry.name)
        if entry.is_file() and m and not entry.name.endswith(".crdownload"):
            found[int(m.group(1))] = str(entry)
    return found


def source_channels(path):
    """1 (mono), 4 (quad: needs the fold) or 2 for anything else (ffmpeg's -ac 2 downmixes it)."""
    probe = subprocess.run([ffmpeg(), "-hide_banner", "-i", str(path)], capture_output=True, text=True).stderr
    m = re.search(r"Audio:.*?Hz, ([^,]+)", probe)
    return {"mono": 1, "4.0": 4, "quad": 4, "4 channels": 4}.get(m.group(1).strip() if m else "", 2)


def decode(path, rate=RATE, channels=2, filters=None, limit=None):
    """Float PCM, shape (frames, channels). A quad source folds to stereo; mono stays mono for channels=1."""
    chain = [QUAD_FOLD] if channels == 2 and source_channels(path) == 4 else []
    chain += [filters] if filters else []
    args = ["-i", str(path)]
    if chain:
        args += ["-af", ",".join(chain)]
    if limit:
        args += ["-t", str(limit)]
    args += ["-ac", str(channels), "-ar", str(rate), "-f", "f32le", "-"]
    raw = _run(args).stdout
    return np.frombuffer(raw, dtype="<f4").reshape(-1, channels).copy()


def best_start(pcm, rate, frames):
    """Start (whole seconds) of the `frames`-long window whose 1 s block levels vary least: no one-off events."""
    blocks = len(pcm) // rate
    need = -(-frames // rate)
    if blocks <= need:
        return 0
    level = pcm[: blocks * rate].reshape(blocks, rate, -1)
    db = 10 * np.log10(np.mean(level.astype(np.float64) ** 2, axis=(1, 2)) + 1e-12)
    c1, c2 = np.concatenate([[0], np.cumsum(db)]), np.concatenate([[0], np.cumsum(db * db)])
    s1, s2 = c1[need:] - c1[:-need], c2[need:] - c2[:-need]
    variance = s2 / need - (s1 / need) ** 2
    return int(np.argmin(variance)) * rate


def make_loop(pcm, rate, seconds, fade):
    """`seconds` of the steadiest stretch, its following `fade` s crossfaded (equal power) into its start, so
    the last sample runs straight on into the first. A short source loops whole."""
    fade_n = int(fade * rate)
    length = min(int(seconds * rate), len(pcm) - fade_n)
    start = best_start(pcm, rate, length + fade_n)
    window = pcm[start : start + length + fade_n]
    out = window[:length].copy()
    t = (np.arange(fade_n) / fade_n * (np.pi / 2))[:, None]
    out[:fade_n] = window[:fade_n] * np.sin(t) + window[length : length + fade_n] * np.cos(t)
    return out


def _json(log):
    """loudnorm's print_format=json block (ffmpeg logs more after it)."""
    return json.loads(re.findall(r"\{[^{}]*\}", log)[-1])


def _loudnorm(raw, channels, lufs, out):
    """loudnorm's measuring pass, then its gain applied linearly: loudnorm's own second pass falls back to dynamic
    mode (a riding gain) whenever the input's LRA is wide, which would put a level step at a loop's seam and squash
    thunder. The gain stops at -1.5 dBTP, so a peaky thunder clip lands under its LUFS target."""
    base = ["-f", "f32le", "-ar", str(RATE), "-ac", str(channels), "-i", raw]
    first = _run(base + ["-af", f"loudnorm=I={lufs}:TP=-1.5:LRA=11:print_format=json", "-f", "null", "-"], text=True)
    m = _json(first.stderr)
    gain = min(lufs - float(m["input_i"]), -1.5 - float(m["input_tp"]))
    _run(base + ["-af", f"volume={gain:.2f}dB", "-c:a", "libvorbis", "-q:a", "5", "-y", out])
    return f"gain {gain:+.1f} dB" + (" (peak-capped)" if gain < lufs - float(m["input_i"]) - 0.05 else "")


def master(src, out, lufs, loop_seconds, eq, max_seconds=None):
    """One source to one OGG: EQ (eq="bed" or "thunder"), loop or onset-trim + fade, loudness, Vorbis q5.
    Returns (channels, normalization type)."""
    channels = 1 if source_channels(src) == 1 else 2
    if eq == "thunder":
        pcm = decode(src, RATE, channels, THUNDER_EQ, limit=max_seconds)
        fade = min(int(THUNDER_FADE * RATE), len(pcm))
        if fade:
            pcm[-fade:] *= np.linspace(1, 0, fade, dtype=np.float32)[:, None]
    else:
        pcm = decode(src, RATE, channels, BED_EQ, limit=ANALYSIS_SECONDS if loop_seconds else None)
        if loop_seconds:
            pcm = make_loop(pcm, RATE, loop_seconds, LOOP_FADE)
    with tempfile.TemporaryDirectory() as tmp:
        raw = os.path.join(tmp, "pcm.f32")
        np.ascontiguousarray(pcm, dtype="<f4").tofile(raw)
        kind = _loudnorm(raw, channels, lufs, out)
    return channels, kind


def measure_lufs(path):
    log = _run(["-nostats", "-i", str(path), "-af", "ebur128", "-f", "null", "-"], text=True).stderr
    return float(re.findall(r"I:\s+(-?[\d.]+) LUFS", log)[-1])


def _swap(src_dir, dest):
    """Copy `src_dir` to `dest` through a sibling temp folder and renames, so `dest` is never half written."""
    dest = Path(dest)
    partial, old = dest.with_name(dest.name + ".partial"), dest.with_name(dest.name + ".old")
    shutil.rmtree(partial, ignore_errors=True)
    shutil.copytree(src_dir, partial)
    shutil.rmtree(old, ignore_errors=True)
    if dest.exists():
        dest.rename(old)
    try:
        partial.rename(dest)
    except OSError:
        # The new copy could not take its place (a locked file): the old one goes back.
        if old.exists() and not dest.exists():
            old.rename(dest)
        raise
    shutil.rmtree(old, ignore_errors=True)


def run(sources, out, install=None):
    found = find_sources(sources)
    empty = [role for role, ids, *_ in ROLES if not any(i in found for i in ids)]
    if len(empty) == len(ROLES):
        raise ValueError(f"none of the recordings are in {sources}")
    if install and empty:
        # A short bank would silence those layers in the game: the installed one stays.
        raise ValueError(f"not installing: no recordings for {', '.join(empty)}")
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    roles = {}
    with tempfile.TemporaryDirectory(dir=out) as tmp:
        for role, ids, treatment, seconds, lufs in ROLES:
            roles[role] = []
            for source in (i for i in ids if i in found):
                name = f"{role}_{len(roles[role])}.ogg"
                path = os.path.join(tmp, name)
                looped = treatment == "loop"
                channels, kind = master(
                    found[source], path, lufs,
                    loop_seconds=seconds if looped else None,
                    eq="bed" if looped else "thunder",
                    max_seconds=None if looped else seconds,
                )
                measured = round(measure_lufs(path), 1)
                roles[role].append({"file": name, "channels": channels, "loop": looped, "lufs": measured, "source": source})
                print(f"{name}: {source} ch={channels} {kind} {measured} LUFS {os.path.getsize(path) // 1024} KB", flush=True)
        manifest = {"version": 1, "sample_rate": RATE, "roles": roles}
        with open(os.path.join(tmp, "manifest.json"), "w") as f:
            json.dump(manifest, f, indent=1)
        _swap(tmp, out / "weather-audio")
    if install:
        _swap(out / "weather-audio", Path(install) / "weather-audio")
    return manifest


def main():
    global _FFMPEG
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--sources", required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--install", help="install assets root (<install>/data/installations/<id>/assets)")
    a = p.parse_args()
    if a.install and not os.environ.get("SKATE_FFMPEG"):
        _FFMPEG = _find_install_ffmpeg(a.install)
    run(a.sources, a.output, a.install)


if __name__ == "__main__":
    main()
