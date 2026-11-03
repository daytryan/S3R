"""The 18 required source items: find them in any folder layout, identify them by what is inside, verify them.

Discs:    the SHA-256 of default.xex identifies the disc; deep verify checks every disc file's size and SHA-256.
Packages: STFS header title ID (0x360) and header SHA-1 (0x32C) identify the package; it is then checked for
          size only (catches truncation). Body blocks are not re-hashed.
Mods:     the mod's largest file (its marker) finds the folder; deep verify hashes every file."""
import hashlib, json, os, re
from pathlib import Path
from tools.pipeline.xdvdfs import Iso

_REFERENCE_TEXT = (Path(__file__).with_name('reference.json')).read_text(encoding='utf-8')
REFERENCE = json.loads(_REFERENCE_TEXT)
ITEMS = [  # (id, Skate Source Files folder, label, kind, STFS title ID)
    ('s3-disc', '01', 'Skate 3 (USA, Europe)', 'iso', None),
    ('s3-tu3', '02', 'Skate 3 Title Update 3', 'stfs', '454108E6'),
    ('s3-dlc-sanvan', '03', 'San Van Party Pack', 'stfs', '454108E6'),
    ('s3-dlc-hawaii', '03', "Danny Way's Hawaiian Dream", 'stfs', '454108E6'),
    ('s3-dlc-maloof', '03', 'Maloof Money Cup 2010 NYC', 'stfs', '454108E6'),
    ('s3-dlc-afterdark', '03', 'After Dark', 'stfs', '454108E6'),
    ('s3-dlc-create', '03', 'Skate.Create Upgrade Pack', 'stfs', '454108E6'),
    ('s3-dlc-blackbox', '03', 'Black Box Distribution Skate Park', 'stfs', '454108E6'),
    ('s3-dlc-filmer', '03', 'Filmer Pack', 'stfs', '454108E6'),
    ('s3-dlc-share', '03', 'Skate Share Pack', 'stfs', '454108E6'),
    ('s3-dlc-money', '03', 'Time is Money', 'stfs', '454108E6'),
    ('s2-disc', '04', 'Skate 2 (Europe)', 'iso', None),
    ('s2-dlc-maloof', '05', 'Maloof Money Cup Pack', 'stfs', '4541087F'),
    ('s2-dlc-filmer', '05', 'Skate 2 Filmer Pack', 'stfs', '4541087F'),
    ('s2-dlc-throwback', '05', 'Throwback Pack', 'stfs', '4541087F'),
    ('s1-disc', '06', 'Skate. (Europe)', 'iso', None),
    ('mod-newsanvan', '07', 'RPCS3 New San Van mod', 'mod', None),
    ('mod-skate1map', '08', 'Skate1Map mod', 'mod', None),
]
PREFERRED = {'s3-dlc-sanvan': '8576AE23'}  # canonical San Van Party Pack (copy B) when both copies are present


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        while block := f.read(8 << 20):
            h.update(block)
    return h.hexdigest()


def stfs_header(path):
    with open(path, 'rb') as f:
        h = f.read(0x364)
    if len(h) < 0x364 or h[:4] not in (b'LIVE', b'PIRS', b'CON '):
        return None
    return h[0x360:0x364].hex().upper(), h[0x32C:0x340].hex()


# Hints for items still missing after a scan (#18, public release review): a pseudo id from identify_file names what
# was seen, and every missing item it fits gets the text as its detail.
UNREADABLE_ISO = '*iso'  # an .iso that can't be read as an Xbox 360 disc
OTHER_PACKAGE = '*stfs:'  # + title ID: a package of that game that isn't a known release
CLOUD_FILES = '*cloud'
HINTS = {
    UNREADABLE_ISO: "An .iso in your folders can't be read as an Xbox 360 disc image ({name}): it may be damaged or "
                    "incomplete. Make the image again from the disc.",
    OTHER_PACKAGE: 'A package for this game was found that is a different release ({name}): only the release the '
                   'guide lists works.',
    CLOUD_FILES: "Some files in your folders are online-only (OneDrive or Dropbox) and aren't checked: right-click "
                 'the folder, choose "Always keep on this device", then check again.',
}
EXTRACTED = 'an extracted disc folder: only .iso disc images work, so make an .iso image of this disc'
GAMES_ON_DEMAND = 'a Games on Demand copy: only .iso disc images work, so make an .iso image of the disc'
DISC_TITLES = {'454108E6': 's3-disc', '4541087F': 's2-disc'}  # the STFS title IDs a Games on Demand copy carries
INSTALLER, GOD = 0x000B0000, 0x00007000  # STFS content types (header 0x344): a title update, a Games on Demand copy


def identify_file(path):
    """(id, identity, problem) for one file, or None when it isn't any known item. The id can be a pseudo id from
    HINTS: not an item, but something seen that explains why one is missing."""
    if path.name.lower() == 'default.xex':
        try:
            xex = sha256(path)
        except OSError:
            return None
        return next(((item_id, xex, EXTRACTED) for item_id, ref in REFERENCE['items'].items()
                     if ref['kind'] == 'iso' and ref['xex'] == xex), None)
    if path.suffix.lower() == '.iso':
        try:
            with Iso(path) as iso:
                xex = hashlib.sha256(iso.read('default.xex')).hexdigest()
                disc_files = {k.lower() for k in iso.files().keys()}
        except (ValueError, KeyError, OSError):
            return UNREADABLE_ISO, path.name, None
        for item_id, ref in REFERENCE['items'].items():
            if ref['kind'] == 'iso' and ref['xex'] == xex:
                return item_id, xex, None
        # Wrong-region/revision disc: find the item with most overlapping file names
        best_item, best_overlap = None, 0
        for item_id, ref in REFERENCE['items'].items():
            if ref['kind'] == 'iso':
                ref_names_lower = {k.lower() for k in ref['files'].keys()}
                overlap = len(disc_files & ref_names_lower)
                if overlap >= len(ref_names_lower) / 2 and overlap > best_overlap:
                    best_item, best_overlap = item_id, overlap
        if best_item:
            return best_item, xex, 'a different region or revision (default.xex differs)'
        return None
    try:
        head = stfs_header(path)
        if not head:
            return None
        title, digest = head
        for item_id, ref in REFERENCE['items'].items():
            if ref['kind'] == 'stfs' and ref['title'] == title and digest in ref['headers']:
                want = ref['headers'][digest]
                size = path.stat().st_size
                problem = None if size == want else f'incomplete ({size} of {want} bytes)'
                return item_id, digest, problem
        if not any(ref['kind'] == 'stfs' and ref['title'] == title for ref in REFERENCE['items'].values()):
            return None
        with open(path, 'rb') as f:
            f.seek(0x344)
            content = int.from_bytes(f.read(4), 'big')
        if content == GOD and title in DISC_TITLES:
            return DISC_TITLES[title], digest, GAMES_ON_DEMAND
        if content == INSTALLER and title == '454108E6':
            return 's3-tu3', digest, 'a different title update: Title Update 3 is needed'
        return OTHER_PACKAGE + title, path.name, None
    except OSError:
        return None
    return None


def identify_folder(folder):
    for item_id, ref in REFERENCE['items'].items():
        if ref['kind'] == 'mod':
            marker = folder/ref['marker']
            if marker.is_file() and marker.stat().st_size == ref['files'][ref['marker']][0]:
                return item_id, ref['marker'], None
    return None


# B7: folders a picked drive or user folder holds that never hold source files (matched case-insensitively), and how
# deep below a picked folder the walk goes (Skate Source Files\03\<pack>\Content\0000000000000000\<title>\00000002 is 7).
SKIP = ('windows', 'program files', 'program files (x86)', 'programdata', '$recycle.bin', 'system volume information',
        'node_modules', '.git', 'steamapps', 'appdata')
DEPTH = 10
# OneDrive/Dropbox "online-only" files: opening one downloads it. RECALL_ON_DATA_ACCESS, RECALL_ON_OPEN, OFFLINE.
CLOUD = 0x400000 | 0x40000 | 0x1000


def scan(folders, cache=None):
    """`cache` (A3: data/sources-identified.json): each file's identify_file() answer with its size and modified time,
    reused while both match, so a repeat scan is a directory walk and a stat per candidate, not a read of every ISO's
    default.xex. Rewritten with only this scan's files."""
    found = {item[0]: {'status': 'missing', 'path': None, 'identity': None, 'detail': ''} for item in ITEMS}
    candidates = []
    # a new reference.json identifies afresh; so does a new identify_file ('hints 1': the HINTS pseudo ids)
    stamp = hashlib.sha256((_REFERENCE_TEXT + 'hints 1').encode()).hexdigest()
    try:
        saved = json.loads(Path(cache).read_text(encoding='utf-8')) if cache else {}
        known = saved['files'] if saved.get('reference') == stamp and isinstance(saved['files'], dict) else {}
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        known = {}
    seen = {}
    for top in folders:
        depth = str(top).rstrip('\\/').count(os.sep)
        for dirpath, dirnames, filenames in os.walk(top):
            dirnames[:] = sorted(d for d in dirnames if d.lower() not in SKIP) if dirpath.count(os.sep) - depth < DEPTH else []
            hit = identify_folder(Path(dirpath))
            if hit:
                candidates.append((Path(dirpath), *hit))
                dirnames[:] = []  # don't look inside a mod for other items
                continue
            for name in sorted(filenames):
                path = Path(dirpath, name)
                # Check if file might be a game item: ISO, Title Update, no dots, or hex-code (8+ hex digits)
                if path.suffix.lower() == '.iso' or '.' not in name or name.upper().startswith('TU_') or re.search(r'[0-9A-Fa-f]{8}', name) \
                        or name.lower() == 'default.xex':
                    try:
                        stat = os.stat(path, follow_symlinks=False)
                    except OSError:
                        continue
                    if getattr(stat, 'st_file_attributes', 0) & CLOUD:
                        candidates.append((path, CLOUD_FILES, name, None))
                        continue  # not on this PC: identifying it would download it
                    entry = known.get(str(path))
                    if isinstance(entry, list) and len(entry) == 3 and entry[:2] == [stat.st_size, stat.st_mtime_ns]:
                        hit = tuple(entry[2]) if entry[2] else None
                    else:
                        hit = identify_file(path)
                    seen[str(path)] = [stat.st_size, stat.st_mtime_ns, list(hit) if hit else None]
                    if hit:
                        candidates.append((path, *hit))
    if cache and seen != known:
        from tools.asset_pipeline.setup_state import atomic_json
        try:
            Path(cache).parent.mkdir(parents=True, exist_ok=True)
            atomic_json(Path(cache), {'reference': stamp, 'files': seen})
        except OSError:
            pass  # only a slower next scan
    hints = {}
    for path, item_id, identity, problem in candidates:
        if item_id not in found:
            if item_id in HINTS or item_id.startswith(OTHER_PACKAGE):
                hints.setdefault(item_id, identity)
            continue
        slot = found[item_id]
        better = slot['status'] != 'ok' or (not problem and identity and identity.upper().startswith(PREFERRED.get(item_id, '\0')))
        if better:
            found[item_id] = {'status': 'mismatch' if problem else 'ok', 'path': str(path),
                              'identity': identity, 'detail': problem or ''}
    for item_id, _, _, kind, title in ITEMS:
        if found[item_id]['status'] != 'missing':
            continue
        fits = [UNREADABLE_ISO] if kind == 'iso' else [OTHER_PACKAGE + title] if kind == 'stfs' else []
        why = next((HINTS[OTHER_PACKAGE if h.startswith(OTHER_PACKAGE) else h].format(name=hints[h])
                    for h in fits + [CLOUD_FILES] if h in hints), '')
        found[item_id]['detail'] = why
    return found


def stamp(item_id, path):
    """What a verified item looked like on disk: path, size and modified time of each file the check read.
    Copying, editing or replacing a file changes it; the content is only re-hashed when it does."""
    ref = REFERENCE['items'][item_id]
    files = [path] if ref['kind'] == 'iso' else [path/name for name in sorted(ref['files'])] if ref['kind'] == 'mod' else []
    return [[str(f), s.st_size, s.st_mtime_ns] for f in files for s in [f.stat()]]


def verify(found, cache=None, report=None, threads=4):
    """Deep check before a build: every disc file, every mod file. Returns problems as readable lines.
    With `cache` (a JSON file in the install), an item whose stamp() matches its last clean check is not re-read:
    an update's build then starts in seconds instead of re-hashing ~29 GB. Four items are checked at once (hashing
    lets go of the GIL, and an SSD reads several files faster than one); report(done, total) follows each."""
    import threading
    from concurrent.futures import ThreadPoolExecutor
    try:
        passed = json.loads(Path(cache).read_text(encoding='utf-8')) if cache else {}
    except (OSError, ValueError):
        passed = {}
    clean, todo = {}, []
    for item_id, slot in found.items():
        now = None
        if slot['status'] == 'ok':
            try:
                now = stamp(item_id, Path(slot['path']))
            except OSError:
                now = None
            if now and passed.get(item_id) == now:
                clean[item_id] = now
                continue
        todo.append((item_id, slot, now))
    lock, done = threading.Lock(), []

    def check(job):
        item_id, slot, now = job
        mine = []
        verify_one(item_id, slot, mine)
        with lock:
            if not mine and slot['status'] == 'ok' and now:
                clean[item_id] = now
            done.append(item_id)
            if report:
                report(len(done), len(todo))
        return mine
    with ThreadPoolExecutor(threads) as pool:  # 1 for a hard drive (runner #10)
        problems = [line for mine in pool.map(check, todo) for line in mine]  # in found's order
    if cache:
        Path(cache).write_text(json.dumps({k: clean[k] for k in found if k in clean}), encoding='utf-8')
    return problems


def verify_one(item_id, slot, problems):
    """One item's deep check; its problems are appended to `problems`."""
    label = next(i[2] for i in ITEMS if i[0] == item_id)
    if slot['status'] != 'ok':
        problems.append(f'{label}: missing' if slot['status'] == 'missing' else f'{label}: {slot["detail"]}')
        return
    ref = REFERENCE['items'][item_id]
    path = Path(slot['path'])
    try:
        if ref['kind'] == 'iso':
            with Iso(path) as iso:
                files_lower = {k.lower(): k for k in iso.files().keys()}
                bad = []
                for name, (size, digest) in ref['files'].items():
                    if name.lower() not in files_lower:
                        bad.append(name)
                        continue
                    actual_name = files_lower[name.lower()]
                    actual_size = iso.files()[actual_name][1]
                    if actual_size != size:
                        bad.append(name)
                        continue
                    h = hashlib.sha256()
                    for block in iso.chunks(actual_name):
                        h.update(block)
                    if h.hexdigest() != digest:
                        bad.append(name)
        elif ref['kind'] == 'mod':
            bad = [name for name, (size, digest) in ref['files'].items()
                   if not (path/name).is_file() or (path/name).stat().st_size != size or sha256(path/name) != digest]
        else:
            return  # packages identified by header title + digest, size checked; body blocks not re-hashed
        if bad:
            fix = 'make the .iso image again from the disc' if ref['kind'] == 'iso' else \
                'download this mod again: only the release the guide lists works'  # #18: Try again alone repeats this
            problems.append(f'{label}: {len(bad)} files differ, for example {", ".join(bad[:5])}. The copy is damaged or '
                            f'changed: {fix}')
    except (ValueError, KeyError, OSError) as e:
        problems.append(f'{label}: {e}')
