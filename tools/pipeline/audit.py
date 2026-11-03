"""Release audit: fail when anything shipped is game content or carries a personal path.
Looks inside the zips and inside the frozen skate3setup.exe (its PyInstaller CArchive and the PYZ in it).

    python -m tools.pipeline.audit KNOWN_CACHE.json SOURCES_DIR ZIP [ZIP ...]
"""
import hashlib, json, re, sys, tempfile, zipfile
from pathlib import Path

BANNED = {'.big', '.psg', '.psf', '.apt', '.xex', '.edat', '.mus', '.rx2', '.iso', '.xsf'}
PERSONAL = re.compile(rb'(?i)[a-z]:(\\\\|\\|/)users(\\\\|\\|/)')


def members(name, data):
    """The file itself, and for the frozen setup every entry of its archive and of the module archive inside it."""
    yield name, data
    if not name.endswith('skate3setup.exe'):
        return
    from PyInstaller.archive.readers import CArchiveReader
    with tempfile.TemporaryDirectory() as t:
        exe = Path(t, 'setup.exe')
        exe.write_bytes(data)
        archive = CArchiveReader(str(exe))
        for entry, info in archive.toc.items():
            if info[4] == 'z':  # PKG_ITEM_PYZ: the compiled modules
                pyz = archive.open_embedded_archive(entry)
                for module in pyz.toc:
                    yield f'{name}!{entry}!{module}', pyz.extract(module, raw=True) or b''
            else:
                yield f'{name}!{entry}', archive.extract(entry)


def audit(zips, known):
    problems = []
    for path in zips:
        if hashlib.sha256(path.read_bytes()).hexdigest() in known:
            problems.append(f'{path.name}: the whole package is a game file')
        with zipfile.ZipFile(path) as z:
            for info in z.infolist():
                if info.is_dir():
                    continue
                for name, data in members(info.filename, z.read(info)):
                    parts = name.split('!')
                    # A PYZ entry is a dotted module name (tools.owned_game.big), not a file with a suffix.
                    if len(parts) < 3 and Path(parts[-1]).suffix.lower() in BANNED:
                        problems.append(f'{path.name}:{name}: banned file type')
                    if hashlib.sha256(data).hexdigest() in known:
                        problems.append(f'{path.name}:{name}: identical to a game file')
                    # Inside the frozen setup only our own files count: Python, numpy and cryptography carry
                    # their CI machines' build paths (C:\Users\runneradmin\...), which say nothing about us.
                    # The --onedir setup folder (B1) holds the same files loose: only its tools/ are ours.
                    _, bundled, inside = name.partition('/skate3setup/_internal/')
                    ours = inside.startswith('tools/') if bundled else len(parts) == 1 or parts[-1].startswith('tools')
                    if ours and PERSONAL.search(data):
                        problems.append(f'{path.name}:{name}: contains a personal path (C:\\Users\\...)')
    return problems


def known_hashes(found, cache):
    """SHA-256 of every file the user's source items contain: the disc and mod files from reference.json, plus
    every file inside each STFS package. Cached in `cache` (a scratch JSON, never the repo)."""
    cache = Path(cache)
    if cache.is_file():
        return set(json.loads(cache.read_text()))
    from tools.pipeline.sources import REFERENCE
    hashes = set()
    for item in REFERENCE['items'].values():
        hashes |= {v[1] for v in item.get('files', {}).values()}
    sys.path.insert(0, str(Path(__file__).with_name('dlc')))
    from stfs_extract import extract
    for item_id, slot in found.items():
        if REFERENCE['items'].get(item_id, {}).get('kind') == 'stfs':
            hashes.add(hashlib.sha256(Path(slot['path']).read_bytes()).hexdigest())
            with tempfile.TemporaryDirectory() as t:
                with open(Path(t, 'log'), 'w') as log:  # the extractor prints a line per file
                    stdout, sys.stdout = sys.stdout, log
                    try:
                        extract(slot['path'], t + '/out')
                    finally:
                        sys.stdout = stdout
                hashes |= {hashlib.sha256(p.read_bytes()).hexdigest() for p in Path(t, 'out').rglob('*') if p.is_file()}
    cache.write_text(json.dumps(sorted(hashes)))
    return hashes


if __name__ == '__main__':
    # SOURCES_DIR '-' (a build machine without the player's files, e.g. GitHub's): the disc and mod files from
    # reference.json only; the DLC packages' contents need the packages themselves.
    from tools.pipeline.sources import scan
    full = sys.argv[2] != '-'
    known = known_hashes(scan([Path(sys.argv[2])]) if full else {}, sys.argv[1] if full else Path(sys.argv[1]).with_suffix('.partial'))
    found = audit([Path(p) for p in sys.argv[3:]], known)
    scope = 'discs, DLC and mods' if full else 'discs and mods only (no source folder on this machine)'
    print('\n'.join(found) or f'Release audit passed: {len(known)} game files checked ({scope})')
    sys.exit(1 if found else 0)
