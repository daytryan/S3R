"""Versioned, checksummed character stages, scoped to one portable install."""
import hashlib
import json
from pathlib import Path
import shutil
import stat
import time
from concurrent.futures import ThreadPoolExecutor
from . import setup_state
from .setup_state import atomic_json, valid_receipt

SHARED = ('owned_game/**/*.py', 'vendor/utt/**/*.py', 'vendor/utt/*_native.rs', 'vendor/utt/**/*.json',
          'asset_pipeline/fast_refpack.py', 'asset_pipeline/refpack_native.rs',
          'requirements-setup.txt', 'asset_pipeline/optional_content.py')
GEOMETRY = ('asset_pipeline/character_glb.py', 'asset_pipeline/retail_character.py',
            'vendor/skate3_anim/abin_importer.py', 'vendor/skate3_anim/rx2_skeleton.py',
            'extract_default_skater.py', 'default_skater_retail_manifest.json')
SOURCES = {
    'catalog': ('asset_pipeline/customisation_catalog.py', 'asset_pipeline/customisation_native.py',
                'asset_pipeline/vlt.py', 'asset_pipeline/names.txt'),
    'library': GEOMETRY + ('asset_pipeline/customisation_library.py', 'asset_pipeline/customisation_worker.py'),
    'menu': ('asset_pipeline/customisation_profiles.py',),
    'lighting': ('asset_pipeline/customiser_lighting.py', 'asset_pipeline/environment.py',
                 'asset_pipeline/native_roster.py', 'asset_pipeline/marquee_assets.py', 'extract_default_skater.py'),
    'roster': GEOMETRY + ('asset_pipeline/native_roster.py', 'asset_pipeline/marquee_assets.py', 'mixamo_to_skate/*.py',
               'asset_pipeline/customisation_library.py'),  # decode_png
}
OUTPUTS = {
    'catalog': ('catalog.json', 'native.json', 'database', 'source/data/content/recipe'),
    'library': ('library', 'decoded', 'source/data/content/createacharacter', 'library-base.json'),
    'menu': ('extra-menu.json',),
    'lighting': ('specular', 'native-lighting.json', 'library-v3.json', 'lighting-availability.json'),
    'roster': ('native-roster',),
}
REQUIRED = {
    'catalog': ('catalog.json', 'native.json', 'database/collections.json'),
    'library': ('library-base.json',),
    'menu': ('extra-menu.json',),
    'lighting': ('library-v3.json', 'native-lighting.json'),
    'roster': ('native-roster/complete.json',),
}


def versions(tools=None):
    tools = tools or Path(__file__).resolve().parents[1]
    result = {}
    for stage, patterns in SOURCES.items():
        digest = hashlib.sha256()
        for path in sorted({p for pat in SHARED+patterns for p in tools.glob(pat)
                            if p.is_file() and not p.name.startswith('test_')}):
            digest.update(path.relative_to(tools).as_posix().encode()+b'\0')
            digest.update(hashlib.sha256(path.read_bytes().replace(b'\r\n', b'\n')).digest())
        if stage != 'catalog':digest.update(result['catalog'].encode())
        if stage == 'lighting':digest.update(result['library'].encode())
        result[stage] = digest.hexdigest()
    return result


def receipt(root, paths, marker=None):
    """setup_state.receipt (the same entries, in the same order) for many files: the root is resolved once, not every
    file (a resolve opens it), and the SHA-256s run on 8 threads (hashlib lets go of the GIL; a file written moments ago
    waits on the virus scanner at its first open). Paths not spelled under root take setup_state's own route.
    marker: the stage's last *-complete.json; a file it lists that is older than it and still the same size keeps the
    SHA-256 it records (the wardrobe stages rewrite part of the set and receipt all of it, ~4 GB).
    ponytail: a same-size rewrite that also sets its modified time back would keep the old hash, as runner._sha's memo;
    the build's own check then hashes it, calls the stage damaged and redoes it."""
    root, paths = Path(root), list(paths)
    resolved = root.resolve()
    try:
        files = sorted({resolved/Path(p).relative_to(root) for p in paths})
    except ValueError:
        return setup_state.receipt(root, paths)
    known, since = {}, 0
    if marker is not None and Path(marker).is_file():
        since = Path(marker).stat().st_mtime_ns
        known = read(Path(marker)).get('files') or {}

    def entry(path):
        try:
            info = path.stat()
        except OSError:
            return None
        if not stat.S_ISREG(info.st_mode):
            return None
        old = known.get(path.relative_to(resolved).as_posix())
        if isinstance(old, dict) and old.get('size') == info.st_size and info.st_mtime_ns < since and isinstance(old.get('sha256'), str):
            return {'size': info.st_size, 'sha256': old['sha256']}
        return {'size': info.st_size, 'sha256': setup_state.sha(path)}
    with ThreadPoolExecutor(8) as pool:
        found = list(pool.map(entry, files))
    return {p.relative_to(resolved).as_posix(): e for p, e in zip(files, found) if e is not None}


def read(path):
    try:
        data = json.loads(path.read_text())
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):return {}


def stage_valid(directory, name, saved):
    files = saved.get('files')
    return (isinstance(files, dict) and set(REQUIRED[name]).issubset(files)
            and valid_receipt(directory, files))


def stage(directory, previous, name, version, source, action, assets, report):
    start = time.perf_counter()
    marker = directory/(name+'-complete.json')
    saved = read(marker)
    if (saved.get('version') == version and saved.get('source') == source
            and stage_valid(directory, name, saved)):
        report(f'Character {name}: resumed complete stage ({time.perf_counter()-start:.2f}s)')
        return
    # Incomplete stage output must never reach an exists()-based converter.
    for relative in OUTPUTS[name]:
        path = directory/relative
        if path.is_dir():shutil.rmtree(path)
        else:path.unlink(missing_ok=True)
    cached = read(previous/(name+'-complete.json')) if previous else {}
    reused = (previous and cached.get('version') == version and cached.get('source') == source
              and stage_valid(previous, name, cached))
    if reused:
        old_prefix = previous.relative_to(assets).as_posix()+'/'
        new_prefix = directory.relative_to(assets).as_posix()+'/'
        for relative in cached['files']:
            src = previous/relative; dst = directory/relative
            dst.parent.mkdir(parents=True, exist_ok=True)
            if src.suffix == '.json':
                # Runtime references are relative to assets, including generation.
                dst.write_text(src.read_text().replace(old_prefix,new_prefix))
            else:
                try:dst.hardlink_to(src)
                except OSError:shutil.copy2(src,dst)
    else:
        action()
    if any(not (directory/path).is_file() for path in REQUIRED[name]):
        raise RuntimeError('Incomplete character stage: '+name)
    paths = []
    for relative in OUTPUTS[name]:
        path = directory/relative
        paths.extend(path.rglob('*') if path.is_dir() else [path])
    files = receipt(directory, paths)
    if not files:raise RuntimeError('Character stage produced no output: '+name)
    atomic_json(marker, dict(version=version, source=source, files=files))
    elapsed = time.perf_counter()-start
    timing = read(directory/'timings.json')
    timing[name] = {'seconds': round(elapsed,3), 'reused': bool(reused)}
    atomic_json(directory/'timings.json', timing)
    report(f'Character {name}: {"reused" if reused else "prepared"} in {elapsed:.2f}s')


def complete(directory):
    return all(stage_valid(directory, name, read(directory/(name+'-complete.json')))
               for name in SOURCES)
