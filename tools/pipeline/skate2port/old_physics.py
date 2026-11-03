"""Skate 2's physics data: the cut-down copy of the user's own disc that the Skate 2 physics switch needs.

The Skate 2 Rust port inside the game (vendor/skate2-rust-port, the host library skate2_physics_host) reads a few hundred
entries out of nine of the disc's archives. This copies exactly those entries, byte for byte (RefPack stays packed), into
assets/private/skate2-physics/:
  data/<archive path as on the disc>   each archive holds only the listed entries, with its .bh when the disc has one
  manifest.json   {"version": 1, "disc": sha256 of the disc's default.xex, "entries": [[archive, key, stored_size, sha256], ...]}
  draw/           empty; the Skate 3 stage that draws the skater fills it (S3)
Nothing from the disc goes in the repo: old_physics_entries.tsv holds only names, keys and sizes.

Why per-archive files and not one merged table: the port finds an entry in three of the nine archives (anim, livingworld,
ropa2) by the case-sensitive djb2 hash of its path, in the .bh, and in the others by name, ignoring case; a path can also
repeat across archives, and the mount order then picks one. Writing each archive at its disc path leaves all of that to
the port's own mount code.

old_physics_entries.tsv (no header): archive \\t key \\t name \\t stored_size. The key is the entry name in a named table
and 0x%08x in a hashed one (what the port's read log records); the name is the entry's path. It holds:
- every entry the host's disc tests read (merge_reads.py: the read logs of the tests, minus the tests that no longer
  exist), plus all 284 cSim_<x>_<z>_high.xsf cells of worldbam.big: the cells the sim loads depend on where the skater rides;
- every entry of the four WHOLE archives (whole_entries: generated from their tables). The tests cannot say which city
  recipes and models a long, clock-seeded session loads, so these four small archives are copied entire (fix round 1).
The list stays the one statement of what is copied: the stage has no special case for a whole archive.

usage: py old_physics.py ROOT DISC ASSETS     DISC: an extracted disc folder or the .iso itself
"""
import hashlib, json, re, shutil, sys, time
from collections import namedtuple
from pathlib import Path

here = Path(__file__).resolve().parent
sys.path[:0] = [str(here)]
import physics_big  # noqa: E402

Entry = namedtuple('Entry', 'archive key name stored')
CELL = re.compile(r'cSim_.*_high\.xsf')
VERSION = 1
# Copied whole (every entry of the table is listed): the tests do not show which recipes and models free play loads.
WHOLE = ('data\\content\\livingworld.big', 'data\\big\\miscload.big', 'data\\big\\ropa2.big', 'data\\db\\db.big')
# The archives the port reads through their hashed .bh (ArchiveSpec.use_external_header): their keys are hashes.
HASHED = ('data\\big\\anim.big', 'data\\content\\livingworld.big', 'data\\big\\ropa2.big')


def read_entries(path):
    entries = []
    for number, line in enumerate(Path(path).read_text(encoding='utf-8').splitlines(), 1):
        if not line.strip():
            continue
        parts = line.split('\t')
        if len(parts) != 4 or not parts[3].isdigit():
            raise ValueError(f'{Path(path).name} line {number}: want archive, key, name, stored size')
        entries.append(Entry(parts[0], parts[1], parts[2], int(parts[3])))
    return entries


class FolderDisc:
    """An extracted disc: paths like data/big/anim.big."""

    def __init__(self, root):
        self.root = Path(root)

    def has(self, rel):
        return (self.root/rel).is_file()

    def read(self, rel, offset, size):
        with open(self.root/rel, 'rb') as f:
            f.seek(offset)
            data = f.read(size)
        if len(data) != size:
            raise ValueError(f'{rel} is truncated at {offset}+{size}')
        return data

    def size(self, rel):
        return (self.root/rel).stat().st_size

    def close(self):
        pass


class IsoDisc:
    """The disc image itself, read through xdvdfs one range at a time (never the whole image, never a whole archive)."""

    def __init__(self, root, path):
        sys.path[:0] = [str(root)]
        from tools.pipeline.xdvdfs import Iso
        self.iso = Iso(path)

    def has(self, rel):
        return rel in self.iso.files()

    def read(self, rel, offset, size):
        from tools.pipeline.xdvdfs import SECTOR
        sector, length = self.iso.files()[rel]
        if offset + size > length:
            raise ValueError(f'{rel} is truncated at {offset}+{size}')
        self.iso.file.seek(self.iso.base + sector*SECTOR + offset)
        data = self.iso.file.read(size)
        if len(data) != size:
            raise ValueError(f'{rel} is truncated at {offset}+{size}')
        return data

    def size(self, rel):
        return self.iso.files()[rel][1]

    def close(self):
        self.iso.file.close()


def table_of(disc, rel):
    """The table at the start of a .big or .bh (its header size is in the first 16 bytes)."""
    size = int.from_bytes(disc.read(rel, 12, 4), 'big')
    if size < 16 or size > disc.size(rel):
        raise ValueError(f'{rel}: header size {size}')
    return physics_big.read_table(disc.read(rel, 0, size))


def whole_entries(disc, archive, hashed=None):
    """Every entry of `archive`'s table as list entries, in table order. The key is the name, or for an archive the port
    reads through its .bh (`hashed`, by default whether `archive` is in HASHED) 0x%08x of the name's djb2, which must
    be one row of the .bh with the same offset and size. Raises ValueError for a name or hash that occurs twice."""
    hashed = archive in HASHED if hashed is None else hashed
    rel = archive.replace('\\', '/')
    big = table_of(disc, rel)
    bh_rel = rel[:-len('.big')] + '.bh'
    bh = {(r.key, r.offset, r.stored) for r in table_of(disc, bh_rel).rows} if hashed else None
    if hashed and not disc.has(bh_rel):
        raise ValueError(f'{archive}: no .bh')
    entries, seen = [], set()
    for row in big.rows:
        name = row.key
        key = f'0x{physics_big.djb2(name.encode("latin-1")):08x}' if hashed else name
        if (key.lower() if not hashed else key) in seen:
            raise ValueError(f'{archive}: {key} twice')
        seen.add(key.lower() if not hashed else key)
        if hashed and (int(key, 16), row.offset, row.stored) not in bh:
            raise ValueError(f'{archive}: the .bh has no row for {name}')
        entries.append(Entry(archive, key, name, row.stored))
    return entries


def build(disc, entries, out):
    """Copies `entries` from `disc` into the folder `out` (made new) and returns (entry count, stored bytes)."""
    out = Path(out)
    shutil.rmtree(out, ignore_errors=True)
    sha, archives = {}, {}
    for e in entries:
        archives.setdefault(e.archive, []).append(e)
    for archive, wanted in archives.items():
        rel = archive.replace('\\', '/')
        big = table_of(disc, rel)
        if big.hashed:
            raise ValueError(f'{archive}: the .big holds a hashed table')
        bh_rel = rel[:-len('.big')] + '.bh'
        bh = table_of(disc, bh_rel) if disc.has(bh_rel) else None
        by_name, by_hash = {}, {}
        for index, row in enumerate(big.rows):
            by_name.setdefault(row.key, []).append((index, row))
            by_hash.setdefault(physics_big.djb2(row.key.encode('latin-1')), []).append((index, row))
        hashed = {(r.offset, r.stored) for r in bh.rows} if bh else set()  # the .bh must agree with the names
        chosen = []
        for e in wanted:
            if e.key.startswith('0x'):
                found = by_hash.get(int(e.key, 16), [])
                if bh is None or len(found) != 1 or (found[0][1].offset, found[0][1].stored) not in hashed:
                    raise ValueError(f'the disc differs: {archive} has no hashed entry {e.key}')
            else:
                found = by_name.get(e.key, [])
            if len(found) != 1:
                raise ValueError(f'the disc differs: {archive} holds {len(found)} entries for {e.key}')
            index, row = found[0]
            if row.key != e.name or row.stored != e.stored:
                raise ValueError(f'the disc differs: {archive} {e.key} is {row.key} of {row.stored} bytes, the list says {e.name} of {e.stored}')
            chosen.append((index, row))
        chosen.sort(key=lambda item: item[0])  # the disc's own order, which decides between entries that share a name
        payloads = []
        for index, row in chosen:
            data = disc.read(rel, row.offset, row.stored)
            sha[(archive, row.key)] = hashlib.sha256(data).hexdigest()
            payloads.append((row.key, data))
        physics_big.write(out/rel, physics_big.Table(big.magic, big.width, ()), payloads, bh=bh)
    rows = [json.dumps([e.archive, e.key, e.stored, sha[(e.archive, e.name)]]) for e in entries]
    (out/'draw').mkdir(parents=True, exist_ok=True)  # S3's skater draw data goes here
    xex = hashlib.sha256(disc.read('default.xex', 0, disc.size('default.xex'))).hexdigest()
    text = f'{{"version": {VERSION}, "disc": "{xex}", "entries": [\n' + ',\n'.join(rows) + '\n]}\n'
    (out/'manifest.json').write_bytes(text.encode('utf-8'))  # last: a folder without it was not finished
    return len(entries), sum(e.stored for e in entries)


def verify(folder):
    """Checks `folder` against its manifest, as the host does at boot; returns the entry count.
    Raises ValueError('missing <archive> <key>') naming the first entry that is listed but not there."""
    folder = Path(folder)
    manifest = json.loads((folder/'manifest.json').read_text(encoding='utf-8'))
    if manifest.get('version') != VERSION:
        raise ValueError(f'manifest version {manifest.get("version")}')
    tables = {}

    def lookup(archive, hashed):
        if (archive, hashed) not in tables:
            rel = archive.replace('\\', '/')
            path = folder/(rel[:-len('.big')] + '.bh' if hashed else rel)
            rows = {}
            if path.is_file():
                try:
                    table = physics_big.read_table(path.read_bytes())
                except ValueError:
                    table = physics_big.Table(b'', 0, ())
                for r in table.rows:
                    rows.setdefault(f'0x{r.key:08x}' if hashed else r.key.lower(), r.stored)
            tables[(archive, hashed)] = rows
        return tables[(archive, hashed)]

    for archive, key, stored, _ in manifest['entries']:
        hashed = key.startswith('0x')
        if lookup(archive, hashed).get(key if hashed else key.lower()) != stored:
            raise ValueError(f'missing {archive} {key}')
    return len(manifest['entries'])


def main(root, disc, assets):
    started = time.time()
    root, disc, assets = Path(root), Path(disc), Path(assets)
    source = FolderDisc(disc) if disc.is_dir() else IsoDisc(root, disc)
    try:
        count, size = build(source, read_entries(here/'old_physics_entries.tsv'), assets/'private/skate2-physics')
    finally:
        source.close()
    verify(assets/'private/skate2-physics')
    print(f'skate2-physics: {count} entries, {size} bytes ({size/1e6:.1f} MB) in {time.time() - started:.1f} s')


if __name__ == '__main__':
    main(*sys.argv[1:4])
