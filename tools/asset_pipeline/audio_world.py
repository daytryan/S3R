"""World-audio data for the engine's audio runtime: `private/stock/data/audio/world-audio.json`.

Two things the runtime cannot read cheaply from the staged archives:

* the audio vault classes (reverb presets, audio volumes, painter ambiences, district configs)
  with each collection's raw layout block. The title's own loaders (`sub_824DE140` for reverb
  presets) are run as original code and read a layout byte for byte, so the layout is exported
  verbatim beside the decoded fields;
* each district's audio painter layers (`audio_reverb`, `audio_ambience`, `audio_emitters`) out of
  its Sim stream. A layer tile is an RW4 arena object: a quadtree (type `0xEB0010`), a palette of
  vault keys (`0xEB0011`) and a slot naming the layer (`0xEB000F` = {layer index, palette index,
  u64 layer key}). Sampling (the game's `sub_82C0F0B0`): the tile whose centre is within its half
  size of (x, z); children (-x,-z), (-x,+z), (+x,-z), (+x,+z); leaf value = palette index,
  0xFFFF = unpainted.

    py -3.12 -m tools.asset_pipeline.audio_world <disc root> <assets dir>
"""
import json
import struct
import sys
import tempfile
from pathlib import Path

from . import install as engine
from .vlt import hash64, vault, convert, unpack

CLASSES = {
    0x204CAC1FD77088B8: 'aud_reverb',
    0xF0CEF367088EFFF8: 'audio_volume',  # every .ems box resolves to one of these
    0x656AAE7BB40F6C7F: 'aud_wp_ambiences',
    0x5FD3F641DB66DF9A: 'ambience_pair',
    0xF4917ACACAFAF913: 'district_audio',
    0x39A2DE0232912CE5: 'emitter_set',  # audio_emitters palette: weighted emitter volumes
    0x4CDD7CDC1A955D5C: 'output_wobble',  # the final outputs' Del0/PI20 modulators (sub_824DDF58)
}
LAYERS = {
    0x6879AFCFA737F03A: 'audio_reverb',
    0xEA754449D4731193: 'audio_ambience',
    0xD2DB3C8744839633: 'audio_emitters',
}
SLOT, QUADTREE, PALETTE = 0xEB000F, 0xEB0010, 0xEB0011
# ponytail: the parks whose district_audio key is not hash64('dist_<snake name>'); paired by
# the zone files they list (sfx_mega_skatepark.ems, ...). The title's own lookup is not traced.
CONFIGS = {'DIST_MegaPark': 0x3016BE7C6B34E27A, 'DIST_StartPark': 0x5ACC9E68DA549C74,
           'DIST_IndustrialSkatePark': 0xDA948BAB65025C15, 'DIST_MaloofMoneyCup': 0x83E284E31F8EED57,
           'DIST_DownTownSkatePark': hash64('dist_downtown_skatepark')}


def key_of(name):
    return int(name[5:], 16) if name.startswith('Hash_') else hash64(name)


def vault_classes(db_root, names):
    """{class: [{key, parent, layout, fields}]} for CLASSES, layouts verbatim from the .bin, in
    the order Attrib's class iteration (`sub_82B6DED0` / `sub_82B6DF30`) visits them: the slot
    order of the class's hash table, `reserve` slots, home slot = low 32 bits of the key mod
    `reserve`, linear probing, filled in vault order. Checked against the running game's tables
    (Xenia RAM) for all seven classes."""
    stem = db_root / 'data/db/skatercollections'
    cv, cb, ce = vault(stem)
    sv, _, se = vault(db_root / 'data/db/skaterschema')
    span, reserve = {}, {}
    for _, kind, _, at in se:
        if kind == 0x2A7895AC4A876152:
            key, slots, _, _, _, _, layout = unpack('QIIIIII', sv, at)
            span[key], reserve[key] = layout, slots
    raw = {}
    for _, kind, _, at in ce:
        if kind != 0xAD303B8F42B3307E:
            continue
        key, cls, parent, _, _, _, _, _, layout, _ = unpack('QQQIIIHHII', cv, at)
        if cls in CLASSES:
            size = span.get(cls, 0)
            raw[(cls, key)] = (parent, bytes(cb[layout:layout + size]) if layout else b'')
    decoded = convert(db_root / 'data/db/skaterschema', stem, names)['collections']
    out = {name: [] for name in CLASSES.values()}
    for row in decoded:
        cls = key_of(row['class'])
        if cls not in CLASSES:
            continue
        key = key_of(row['key'])
        parent, layout = raw[(cls, key)]
        out[CLASSES[cls]].append({
            'key': '%016X' % key,
            'parent': '%016X' % parent if parent else '',
            'layout': layout.hex().upper(),
            'fields': row['fields'],
        })
    for cls, name in CLASSES.items():
        table = [None] * max(reserve.get(cls, 0), len(out[name]))
        for row in out[name]:
            slot = (int(row['key'], 16) & 0xFFFFFFFF) % len(table)
            while table[slot] is not None:
                slot = (slot + 1) % len(table)
            table[slot] = row
        out[name] = [row for row in table if row is not None]
    return out


def arena_layers(data):
    """The audio painter layer tiles in one RW4 arena."""
    if data[:4] != b'\x89RW4':
        return []
    count, = struct.unpack_from('>I', data, 0x20)
    table, = struct.unpack_from('>I', data, 0x30)
    objects = [struct.unpack_from('>6I', data, table + 24 * i) for i in range(count)]
    tiles = []
    for at, _, size, _, _, kind in objects:
        if kind != SLOT or size < 16:
            continue
        layer, palette, hi, lo = struct.unpack_from('>IIII', data, at)
        name = LAYERS.get((hi << 32) | lo)
        if name is None:
            continue
        q = objects[layer][0]
        cx, cz = struct.unpack_from('>ff', data, q)
        hx, hz = struct.unpack_from('>ff', data, q + 0x10)
        nodes, first = struct.unpack_from('>II', data, q + 0x20)
        p = objects[palette][0]
        n, = struct.unpack_from('>I', data, p + 4)
        keys = []
        for i in range(n):
            off, _ = struct.unpack_from('>II', data, p + 0x14 + 8 * i)
            w0, w1 = struct.unpack_from('>II', data, p + off)
            keys.append('%016X' % ((w1 << 32) | w0))
        tiles.append((name, {
            'cx': cx, 'cz': cz, 'hx': hx, 'hz': hz,
            'nodes': data[q + first:q + first + 10 * nodes].hex().upper(),
            'palette': keys,
        }))
    return tiles


def district_layers(game_root, district, work):
    """{layer name: [tile]} from DIST_<name>'s Sim stream. High-detail cells mark `high`."""
    sys.path.insert(0, str(engine.TOOLS / 'vendor/university/tools/vanilla_map_extraction/tools'))
    from skate3_streams import load_district_stream
    archive = game_root / f'data/content/world{district}.big'
    prefix = f'data/content/world/stream/{district}/'.lower()
    engine.extract(archive, work, lambda e: e.path.lower().startswith(prefix)
                   and ('_sim.' in e.path.lower() or '/csim_' in e.path.lower()))
    stream = work / f'data/content/world/stream/{district}'
    out = {name: [] for name in LAYERS.values()}
    for asset in load_district_stream(stream, 'Sim', district):
        for name, tile in arena_layers(asset.data):
            tile['high'] = asset.source_path.name.lower().endswith('_high.xsf')
            out[name].append(tile)
    return out


def district_config(district, configs):
    """The district_audio collection for DIST_<Name>: dist_<name>, dist_<snake_name>, or CONFIGS."""
    name = district.removeprefix('DIST_')
    snake = ''.join('_' + c.lower() if c.isupper() and i else c.lower() for i, c in enumerate(name))
    for key in (CONFIGS.get(district), hash64('dist_' + name.lower()), hash64('dist_' + snake)):
        if key and '%016X' % key in configs:
            return '%016X' % key
    return None


def zones(audiofiles, member):
    """An .ems member: u32 count, then 72-byte boxes {u32 index, flags; f32 pos[3], half extent[3],
    inner fraction, cos, 0, sin; u64 audio volume key; f32 gains[4]}."""
    entry = next((e for e in audiofiles.entries if e.path.lower() == 'data/audio/' + member.lower()), None)
    if entry is None:
        return []
    with open(audiofiles.path, 'rb') as f:
        f.seek(entry.offset)
        data = f.read(entry.stored_size)
    count, = struct.unpack_from('>I', data, 0)
    out = []
    for i in range(count):
        r = data[4 + 72 * i:4 + 72 * (i + 1)]
        index, flags = struct.unpack_from('>II', r, 0)
        out.append({'index': index, 'flags': flags, 'pos': struct.unpack_from('>3f', r, 8),
                    'ext': struct.unpack_from('>3f', r, 20), 'shape': struct.unpack_from('>4f', r, 32),
                    'key': r[48:56].hex().upper(), 'gains': struct.unpack_from('>4f', r, 56)})
    return out


VAULT_FILES = ('skaterschema.bin', 'skaterschema.vlt', 'skatercollections.bin', 'skatercollections.vlt')


def stage_vaults(game_root, assets, work):
    """The disc's gameplay vault, verbatim, at `private/stock/data/db/`: the audio runtime loads it
    into guest memory so the title's own code reads the same bytes it reads on the console (the
    customisation sets keep their own, possibly merged, copies)."""
    database = Path(work) / 'database'
    engine.extract(Path(game_root) / 'data/big/db.big', database,
                   lambda e: Path(e.path).name.lower() in VAULT_FILES)
    target = Path(assets) / 'private/stock/data/db'
    target.mkdir(parents=True, exist_ok=True)
    for name in VAULT_FILES:
        (target / name).write_bytes((database / 'data/db' / name).read_bytes())
    return database


def export(game_root, assets, work, names):
    game_root, assets, work = Path(game_root), Path(assets), Path(work)
    database = stage_vaults(game_root, assets, work)
    classes = vault_classes(database, names)
    result = {'version': 1, 'classes': classes, 'districts': {}}
    from tools.owned_game.big import BigArchive
    audiofiles = BigArchive(game_root / 'data/audio/audiofiles.big')
    for archive in sorted((game_root / 'data/content').glob('worldDIST_*.big')):
        district = archive.stem.removeprefix('world')
        entry = {'layers': district_layers(game_root, district, work / district), 'zones': {}}
        configs = {c['key']: c for c in classes['district_audio']}
        config = district_config(district, configs)
        if config:
            fields = configs[config]['fields']
            entry['config'] = config
            entry['crossfade_bank'] = fields.get('Hash_33526BC9D1C36B4A', {}).get('data', '')
            ems = fields.get('Hash_65FA976EF23A314E', {}).get('array', {}).get('text_items', [])
            entry['zones'] = {name: zones(audiofiles, name) for name in ems}
        result['districts'][district] = entry
    target = assets / 'private/stock/data/audio/world-audio.json'
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(result, separators=(',', ':')), encoding='utf-8')
    return target


if __name__ == '__main__':
    names = (engine.TOOLS / 'asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
    with tempfile.TemporaryDirectory() as work:
        print(export(sys.argv[1], sys.argv[2], work, names))
