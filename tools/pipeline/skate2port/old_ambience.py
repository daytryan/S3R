"""Skate 1's and 2's ambience for their converted maps.

Writes, for each game, assets/private/skate2-audio/ and assets/private/skate1-audio/:
- NN_<name>.snr / NN_<name>.sns: each bed's EAAC header (data/audio/ambienceresident.big) and block chain
  (data/audio/ambience.big), loose. The same stream type Skate 3's beds are (EA-XMA, 48 kHz, streamed loop); the
  engine (skate_data::audio::title::Beds) decodes bed N into Skate 3's slot N. Never overwrites a file already there.
- world-audio.json: the game's ambience painter and rows in skate_data::audio::world's form. Class aud_wp_ambiences
  holds its rows (skater.vlt: bed, volume, fades); district DIST_<map> layer audio_ambience holds its world stream's
  WorldPainter layer 15 tiles (RW object 0xEB000F {quadtree, dictionary, layer, tag}: 128 m quadtrees as Skate 3's),
  each leaf's dictionary value as its row, unknown values as none.

TRACED: the value -> row tables, each exe's painter name records {name, 0, value, ...}: Skate 1's at 0x820C4AA8 (its
27 rows sorted, from 0x52), Skate 2's at 0x82088CD0 (SKATE2_ROWS). Skate 2's layer 15 also paints 0x5A, 0x5B
(e_dt_main, an emitter) and 0x63, which name no ambience row: none plays there.
INFERRED: layer 15 is the ambience layer (its values are ambience values; eLayerID's names are in neither exe).

Skate 1's painter is in data/content/world/stream/BAM.xsf; Skate 2's in data/content/worldbam.big's
cSim_<x>_<z>_high.xsf cells, each record's payload behind a 0x50-byte chunk header.

usage: py old_ambience.py ROOT SKATE2_ROOT SKATE1_ROOT ASSETS
"""
import hashlib, json, struct, sys, tempfile
from pathlib import Path

repo, s2game, s1game, assets = map(Path, sys.argv[1:5])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(here)]
from s2big import Big
from tools.asset_pipeline.vlt import convert, hash64
from tools.asset_pipeline.environment import Collections
from tools.owned_game.refpack import decompress

SKATE1_ROWS = {0x52 + i: name for i, name in enumerate(sorted(
    'art_gallery finance_downtown finance_downtown_alt finance_riverbay finance_riverbay_alt finance_waterfront '
    'finance_waterfront_alt oldtown_industrial oldtown_industrial_alt oldtown_waterfront oldtown_waterfront_alt '
    'outskirts_1 outskirts_1_alt outskirts_2 outskirts_2_alt suburbs_1 suburbs_1_alt suburbs_2 suburbs_2_alt '
    'urbres_main urbres_main_alt urbres_skids urbres_skids_alt warehouse_clean warehouse_clean_alt '
    'warehouse_ratty warehouse_ratty_alt'.split()))}
SKATE2_ROWS = {0x52: 'back_alley_pete', 0x53: 'soho', 0x55: 'dt_courtyard', 0x56: 'soho_alley', 0x58: 'dt_interiors',
               0x59: 'spillway', 0x5C: 'svm_north', 0x5E: 'dt_matrix', 0x5F: 'svm_south', 0x61: 'dt_parks',
               0x62: 'urban_res', 0x64: 'old_town', 0x65: 'wf_main', 0x68: 'wf_shore', 0x6A: 'projects_main',
               0x6B: 'x_games', 0xDE: 'soho_classic', 0xDF: 'wf_main2', 0xE2: 'dt_main', 0xE3: 'projects_below',
               0xE6: 'ot_courtyard'}
LAYER, SLOT, QUADTREE, DICTIONARY = 15, 0xEB000F, 0xEB0010, 0xEB0011
REFPACK = (b'\x10\xfb', b'\x90\xfb')
# The row fields the engine reads, by the names Skate 3's world-audio.json uses: bed, volume, fade out, fade in.
FIELDS = {'Hash_4042D2F96C4E4B8C': 0x4042D2F96C4E4B8C, 'volume': hash64('volume'),
          'Hash_2E2ABEBF53F107EC': 0x2E2ABEBF53F107EC, 'Hash_17C3EB9C543E1210': 0x17C3EB9C543E1210}


def copy_beds(audio, out):
    written = 0
    for archive, ext in (('ambienceresident.big', '.snr'), ('ambience.big', '.sns')):
        big = Big(audio/archive)
        for entry in big.entries:
            name = Path(entry.path).name
            if name[:2].isdigit() and name.endswith(ext) and not (out/name).exists():
                (out/name).write_bytes(big.read(entry))
                written += 1
    return written


def rows(skater_vlt, skater_bin, names):
    with tempfile.TemporaryDirectory(prefix='old-ambience-db-') as work:
        (Path(work)/'skater.vlt').write_bytes(skater_vlt)
        (Path(work)/'skater.bin').write_bytes(skater_bin)
        vault = Collections(convert(Path(work)/'skater', Path(work)/'skater',
                                    (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()))
    out = []
    for name in names:
        fields, _ = vault.resolve('aud_wp_ambiences', name)
        assert FIELDS['Hash_4042D2F96C4E4B8C'] in fields, f'{name}: no bed'
        out.append({'key': '%016X' % hash64(name), 'parent': '', 'layout': '',
                    'fields': {n: {'type': fields[h].get('type', ''), 'data': fields[h]['data']} for n, h in FIELDS.items() if h in fields}})
    return out


def records(read, size):
    """A world stream's record payloads (from 0x80: {u64 asset, u32 stored, u32 header, u32 stride}), unpacked."""
    at = 0x80
    while at + 20 <= size:
        asset, stored, header, stride = struct.unpack('>QIII', read(at, 20))
        if not asset or not stride:
            break
        packed = read(at + header, stored)
        at += stride
        if packed[:2] not in REFPACK and packed[0x50:0x52] in REFPACK:  # Skate 2: a 0x50-byte chunk header first
            packed = packed[0x50:]
        yield decompress(packed) if packed[:2] in REFPACK else packed


def tiles(payloads, by_value):
    out, seen = [], set()
    for data in payloads:
        digest = hashlib.blake2b(data, digest_size=16).digest()
        if data[:4] != b'\x89RW4' or digest in seen:
            continue
        seen.add(digest)
        count, = struct.unpack_from('>I', data, 0x20)
        table, = struct.unpack_from('>I', data, 0x30)
        objects = [struct.unpack_from('>6I', data, table + 24*i) for i in range(count)]
        for offset, _, length, _, _, kind in objects:
            if kind != SLOT or length != 16:
                continue
            quad, dictionary, layer, _ = struct.unpack_from('>4I', data, offset)
            if layer != LAYER:
                continue
            (qo, _, qs, _, _, qk), (do, _, ds, _, _, dk) = objects[quad], objects[dictionary]
            assert (qk, dk) == (QUADTREE, DICTIONARY), (hex(qk), hex(dk))
            q, d = data[qo:qo + qs], data[do:do + ds]
            cx, cz = struct.unpack_from('>ff', q, 0)
            hx, hz = struct.unpack_from('>ff', q, 0x10)
            nodes, first = struct.unpack_from('>II', q, 0x20)
            n, = struct.unpack_from('>I', d, 0)
            values = [struct.unpack_from('>I', d, struct.unpack_from('>I', d, 0x14 + 8*j)[0])[0] for j in range(n)]
            out.append({'cx': cx, 'cz': cz, 'hx': hx, 'hz': hz, 'nodes': q[first:first + 10*nodes].hex(),
                        'palette': ['%016X' % hash64(by_value[v]) if v in by_value else '' for v in values], 'high': False})
    return out


def write(game, out, beds, row_list, map_name, painted):
    assert painted, f'{game}: no ambience tiles'
    world = {'classes': {'aud_wp_ambiences': row_list},
             'districts': {f'DIST_{map_name}': {'layers': {'audio_ambience': painted}}}}
    (out/'world-audio.json').write_text(json.dumps(world), encoding='utf-8')
    print(f'{game}: {beds} bed files, {len(row_list)} rows, {len(painted)} tiles written to {out}')


# Skate 2
out = assets/'private/skate2-audio'
out.mkdir(parents=True, exist_ok=True)
beds = copy_beds(s2game/'data/audio', out)
db = Big(s2game/'data/db/db.big')
skater = {Path(e.path).name: db.read(e) for e in db.entries if Path(e.path).name in ('skater.vlt', 'skater.bin')}
world = Big(s2game/'data/content/worldbam.big')


def cells():
    for entry in world.entries:
        if '/cSim_' in entry.path:
            blob = world.read(entry)
            yield from records(lambda at, n: blob[at:at + n], len(blob))


write('Skate 2', out, beds, rows(skater['skater.vlt'], skater['skater.bin'], SKATE2_ROWS.values()), 'New_San_Vanelona',
      tiles(cells(), SKATE2_ROWS))

# Skate 1
out = assets/'private/skate1-audio'
out.mkdir(parents=True, exist_ok=True)
beds = copy_beds(s1game/'data/audio', out)
row_list = rows((s1game/'data/db/skater.vlt').read_bytes(), (s1game/'data/db/skater.bin').read_bytes(), SKATE1_ROWS.values())
with open(s1game/'data/content/world/stream/BAM.xsf', 'rb') as xsf:
    painted = tiles(records(lambda at, n: (xsf.seek(at), xsf.read(n))[1], xsf.seek(0, 2)), SKATE1_ROWS)
write('Skate 1', out, beds, row_list, 'Skate_1_San_Vanelona', painted)
