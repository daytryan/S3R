"""Validate prepared groups without treating user files as extractor output."""
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from . import setup_state
from .setup_state import valid_receipt

ROOTS = {
    'core': ('assets/private/stock/data/anim', 'assets/private/stock/data/state',
             'assets/private/stock/data/config', 'assets/private/stock/data/cacrecipes',
             'assets/private/stock/skater-collections.json', 'assets/private/stock/physics-skeletons.json'),
    'hud': ('assets/private/hud', 'assets/private/session-marker', 'assets/private/core-menu',
            'assets/private/create-skater', 'assets/private/challenge-map',
            'assets/private/tip-loader', 'assets/private/music-ui', 'assets/private/hud-meters',
            'assets/private/hud-availability.json', 'assets/private/session-marker-availability.json',
            'assets/private/core-menu-availability.json', 'assets/private/create-skater-availability.json',
            'assets/private/challenge-map-availability.json',
            'assets/private/tip-loader-availability.json', 'assets/private/music-ui-availability.json',
            'assets/private/hud-meters-availability.json'),
    'character': ('assets/private/game.json', 'assets/private/skater.glb', 'assets/private/default_skater',
                  'assets/private/native-character', 'assets/private/character-lighting.json'),
    'environment': ('assets/private/native-skies', 'assets/private/native-backdrops',
                    'assets/private/render-parameters.json', 'assets/private/exposure.json',
                    'assets/private/exposure-profiles.json', 'assets/private/teleports.json', 'assets/private/environment-status'),
    'maps': ('assets/private/native-props', 'maps.json', 'assets/private/map-status'),
}


def files(root, group):
    result = []
    for name in ROOTS[group]:
        path = root/name
        result.extend(path.rglob('*') if path.is_dir() else [path])
    if group == 'core':
        stock = root/'assets/private/stock'
        result.extend(p for p in stock.rglob('*')
                      if not p.is_relative_to(stock/'data/content/createacharacter'))
    if group == 'maps':
        for item in json.loads((root/'maps.json').read_text()):
            path = (root/item['path']).resolve()
            if not path.is_relative_to((root/'maps').resolve()):
                raise ValueError('Invalid prepared map path')
            result.append(path)
    return result


def record(root, group):
    """setup_state.receipt of the group's files, hashed on 8 threads (hashlib lets go of the GIL; a group is GBs)."""
    root = root.resolve()  # once: resolve() per file opens each one, seconds per 10,000 files
    paths = [p for p in sorted({Path(os.path.normpath(p)) for p in files(root, group)}) if p.is_file()]
    with ThreadPoolExecutor(8) as pool:
        digests = list(pool.map(lambda p: setup_state.sha(p), paths))  # looked up per call: the build's runner caches it
    return {p.relative_to(root).as_posix(): {'size': p.stat().st_size, 'sha256': d} for p, d in zip(paths, digests)}


def damaged(root, marker, exclude=(), sizes=None):
    saved = marker.get('outputs', {})
    result = set()
    for group, roots in ROOTS.items():
        if group in exclude:continue
        if group in saved:
            if not valid_receipt(root, saved[group], sizes):result.add(group)
        else:
            # Legacy packages have no receipts: require their declared roots
            # and verify the map checksums they already recorded. The runtime
            # validator also checks legacy core inputs before publication.
            if any(not (root/name).exists() for name in roots if not name.endswith(('-availability.json', '-status'))):result.add(group)
            if group == 'hud' and group not in result:
                tools = str(Path(__file__).resolve().parents[1])  # it imports vendor.* from tools/ (never frozen)
                if tools not in sys.path:
                    sys.path.append(tools)
                from tools.install_prepared_hud import runtime_files
                try:
                    runtime_files(root/'assets/private/hud')
                    runtime_files(root/'assets/private/session-marker', marker=True)
                except (OSError, ValueError, KeyError, TypeError):result.add(group)
            if group == 'maps' and group not in result:
                try:
                    maps = json.loads((root/'maps.json').read_text())
                    checks = {m['path']: {'size': (root/m['path']).stat().st_size,
                                         'sha256': m['sha256']} for m in maps}
                    if not valid_receipt(root, checks, sizes):result.add(group)
                except (OSError, ValueError, KeyError, TypeError):result.add(group)
    if 'core' in result:result.update(ROOTS)
    return result
