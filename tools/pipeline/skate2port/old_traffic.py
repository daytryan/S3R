"""Skate 1's, 2's and 3's traffic for their maps (single player only; crates/skate-game/src/traffic.rs).

Writes, for each game, assets/private/traffic/skate2/ and assets/private/traffic/skate1/ (Skate 3: see the end):
- roads.json: the road network, every RW object of kind 0xEB0013 (PEGASUS::RWOBJECTTYPE_RAINDATA) in the game's world
  stream, merged by id. Skate 2: data/content/worldbam.big cSim_<x>_<z>_high.xsf cells; Skate 1: BAM.xsf.
  TRACED (Skate 2 exe): the type table 0x82D80A10 names 0xEB0013; the asset dispatch 0x826EE2D0 hands type 25 to the
  RAIN loader 0x825C7DC0; the pointer fixup 0x827E64C0 (roads 0x827E6680, intersections 0x827E6858) gives the layout.
  Every pointer is relative to the start of the struct that holds it.
    header: +0 bbox min, +0x10 bbox max, +0x20 n intersections, +0x24 n roads, +0x28 n superroads,
            +0x2C intersections (608 B), +0x30 roads (160 B), +0x34 superroads (64 B)
    superroad (ctor 0x82BF2DA0): +0 u64 id, +8 u64 node A, +16 u32, +24 u64 node B, +32 u32, +36 length, +40/+44 f32,
            +48 speed (m/s), +52 lanes A->B, +56 lanes B->A
    road (ctor 0x82BF28C8): +0 u64 id, +8 end 0, +64 end 1 (56 B each), +120 n pieces, +124 pieces (224 B), +128..+148 six
            words (kept raw), +152 u64 superroad id
    end: +0 u64 id, +8 u32 a, +12 u32 b, +16 n lanes (<= 4), +20 u32[4] connection counts, +36 ptr[4] -> u32 lists
    piece (ctor 0x82BF2C50): +0 p0, +16 p1, +32 t0, +48 t1 (centre Hermite), +64 length, +68 n arc (16), +72 arc table
            (cumulative length), +80..+128 four edge points, +144..+192 their tangents, +208 edge length
    intersection: +0 four inner corners, +64 four outer corners (the Intersection Points KDTree, 0x825C8E88), +128 four
            entry ends, +352 four exit ends (56 B each), +576 speed, +580 u32, +584 u64 id, +592 lanes (112 B), +596 n
    intersection lane: +0 p0, +16 p1, +32 t0, +48 t1, +64 length, +68 n arc, +72 arc table, +80 speed, +84 from slot,
            +88 u32, +92 to slot, +96 u32
  Numbers are the f32 values exactly (JSON doubles that round-trip to the same f32).
- census.json: which vehicles spawn where.
    painter: the world stream's WorldPainter layer 5 tiles (RW 0xEB000F {quadtree, dictionary, layer, tag}, the format
      skate_data::audio::world reads), each leaf's value as its census row key. TRACED: the census lookup 0x823AE878
      asks the painter 0x82387388 for slot 3; slot table 0x82D803F8 (16 B each: flag, name table, layer, tag) gives
      slot 3 layer 5; the name table (Skate 2 0x820882B0 {name, 0, value, tag}; Skate 1 0x820C4640, 24 B
      {name, 0, value, tag, ptr, 0}) names each value's row, keyed by hash64 of the lowercased name (0x8231E130).
      Values with no name (Skate 2's 0x15, 0x1D, 0x1F, 0x21) spawn nothing (0x823AE878 leaves the count 0), nor do
      names with no vault row (Skate 2's district_park01_vehicles_census): 0x82331AF0 then hands back the zero layout
      0x82E57998, count 0. Skate 1 (0x824DC788, slot table 0x830B0FE8): slot 5 = layer 5 for the census row, plus
      slot 4 = layer 4 for the group (group_painter below).
    census: row key -> {name, count (tLWCensusEntry +24), limit (Int32 Hash_FFE5E258BD468196 or null), group}.
      The entry is Skate 2's Hash_98F6489E23ED2661 / Skate 1's Hash_3107ABED83835EBF (both layout +0). group: the
      entry's group row key (Skate 2), or null (Skate 1: group_painter's value at the spawn point names it).
    groups: group row key (class Hash_D4DD1D82B9981BC5, array of tLWGroupEntry {RefSpec, f32 weight at +24}) ->
      [{weight, entities}], each entity list from class Hash_ADCB4D6BD3B4F85E (RefSpec array). A key missing here
      has no row: retail's pick fails, nothing spawns.
    group_painter: Skate 1 only (else null), layer 4 tiles in the painter format, palette = group row keys.
    entities: livingworld_entities key -> {name, model, appearance, profile, behaviour}: the resolved rows of
      Hash_7DFFA6187E2DE132 (appearance, class Hash_4E740A6670F241A7, model = Hash_62F7C58...), Hash_92A043B4A11F1A2A
      (driver profile, class Hash_62B1C1B37ED2E2CA) and Hash_023EF929823A3C50 (behaviour, class Hash_02AA3F538D6C188A).
      audio: the Skate 3 vault's audio row (class Hash_259095163B974174) the car's sounds read (audio_row).
- audio/<bank>.abk: the game's traffic sound banks under the names of the Skate 3 banks whose slots they take (sounds).
    circle: class Hash_C779D2E21A4F4F35 row 'vehicles' (spawn ring and despawn radius); livingworld: every
      livingworld row. Each row is {field hash (16 hex digits): value}: Float -> number, Int/Bool -> integer,
      Text -> string, Vector -> list, RefSpec -> 'CLASS:KEY', anything else its hex.
- vehicles/<recipe>.glb: each data/content/recipe/vehicle/<recipe>.xml (livingworld.big) a census vehicle uses, at LOD 0, one node per bone
  its vertices are skinned to (rigid, weight 1.0): Chassis at the origin, each wheel at its bind position with its
  vertices relative to it, so the engine can spin it about its axle. Positions are FLOAT16_4 (0x1A2360), which
  rx2_skeleton does not decode; this script does. Diffuse textures, plus the normal maps of Skate 3 (i_normal) and
  Skate 1 (vehiclematerial.psh / vehiclelightmaterial.psh: normalize(2 * rgb - 1) in the mesh's own TANGENT, BINORMAL,
  NORMAL frame, so its GLBs carry TANGENT). Skate 1's shaders (render.big shader source, debug .updb) also sample
  specular (r: the Blinn mask, g: the environment mask), eccentric (no recipe channel binds it) and the shared
  environment cube, scaled by engine constants (spec_spread, spec_power, fresnel_exponent, brightness_modulator) whose
  values are UNTRACED, so those maps are left out; detail, lookup, diffuse2 and transparency are never sampled.

Skate 3 (tu3.pe; its own data, the same RAIN agent): assets/private/traffic/skate3/<Map>/{roads.json,census.json} for each
district whose world stream has roads (DownTown, Industrial, University; SkateSchool is painted but has no roads), and
skate3/vehicles/<recipe>.glb shared by them. Differences from Skate 2, all data: the roads are worldDIST_<Map>.big's Sim
cells (each district in its own frame, the engine map's frame); the census painter layer is named by a 64-bit key,
hash64('livingworld_vehicle_census') = 7FC53B5B51129A55 (layer-name string 0x820C4954), whose palette holds the census
row keys themselves (the audio_world.py layout); the vault is data/big/db.big's skaterschema/skatercollections.

usage: py old_traffic.py ROOT SKATE2_ROOT SKATE1_ROOT SKATE3_ROOT ASSETS
"""
import ast, hashlib, io, json, struct, sys, tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

repo, s2game, s1game, s3game, assets = map(Path, sys.argv[1:6])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools'), str(repo/'tools/vendor/skate3_anim'), str(here)]
from s2big import Big
from tools.asset_pipeline.vlt import convert, hash64
from tools.asset_pipeline.environment import Collections, key_hash
from tools.owned_game.refpack import decompress
import rx2_skeleton as R
import s1mesh

RAIN, LAYER = 0xEB0013, 5
REFPACK = (b'\x10\xfb', b'\x90\xfb')
# The painter name tables' vehicle census rows (value -> row name), from each exe (see the docstring).
SKATE2_CENSUS = {0xA8: 'district_artdeco01_vehicles_census', 0xA9: 'district_svmountain01_vehicles_census',
                 0xCD: 'district_soho01_vehicles_census', 0xCE: 'district_projects01_vehicles_census',
                 0xCF: 'district_park01_vehicles_census', 0xD0: 'district_waterfront01_vehicles_census',
                 0xF5: 'district_oldtown02_vehicles_census', 0xF7: 'district_urbanres01_vehicles_census'}
SKATE1_CENSUS = {0x15: 'district_financial01_vehicles_census', 0x1B: 'district_outskirts01_vehicles_census',
                 0x1D: 'district_oldtown01_vehicles_census', 0x1F: 'district_suburbs01_vehicles_census',
                 0x21: 'district_urbanres01_vehicles_census'}
# Skate 1 paints the vehicle group too (slot 4 = layer 4; same name table).
SKATE1_GROUPS = {0x0E: 'district_financial01_vehicles', 0x11: 'district_outskirts01_vehicles',
                 0x12: 'district_oldtown01_vehicles', 0x13: 'district_suburbs01_vehicles',
                 0x14: 'district_urbanres01_vehicles', 0x8E: 'area_financial_mainroad_vehicles',
                 0x95: 'area_suburbs_shingo_route'}
CENSUS, GROUP, LIST, CIRCLE = 0x1B7BA2772F011BCD, 0xD4DD1D82B9981BC5, 0xADCB4D6BD3B4F85E, 0xC779D2E21A4F4F35
ENTRY, ENTRY1, ITEMS, LIMIT = 0x98F6489E23ED2661, 0x3107ABED83835EBF, 0xD5E1267E2D715124, 0xFFE5E258BD468196
APPEARANCE, PROFILE, BEHAVIOUR, NAME = 0x7DFFA6187E2DE132, 0x92A043B4A11F1A2A, 0x023EF929823A3C50, 0xBD8F02BA1986215A
APPEARANCE_CLASS, SPAWNABLE, MODEL = 0x4E740A6670F241A7, 0xA75AF7F3CFABE6E1, 0x62F7C585686F3371
HOM_REF, HOM_TYPE = 0xE356ED00ABF1D7F0, 0x8B670C504A363A7C  # Score::HoM vehicle type (hom_type)
AUDIO_CLASS, AUDIO_ROW, ENGINE_TYPE = 0x259095163B974174, 0xBA2DDD830C731EE4, 0xC436B6BC22BC023C  # audio_row
# The engine banks by engine type: Skate 3's table 0x8302D408 (Skate 2's 0x82D72E18 is the same eight), Skate 1's
# 0x830A60A4 (nine; no engine type reaches the ninth, C08_family03, through Skate 3's code).
ENGINES3 = ['C00_heavy01', 'C01_family01', 'C03_sports01', 'C04_taxi01', 'C05_truck01', 'C06_sports02', 'C07_family02',
            'C08_family03']
ENGINES1 = ['C00_heavy01', 'C01_family01', 'C02_minivan01', 'C03_sports01', 'C04_taxi01', 'C05_truck01', 'C06_sports02',
            'C07_family02']

# old_ambience.py's world-stream reader and painter tiles, reused as-is (that script runs its export at import).
painter = {'hashlib': hashlib, 'struct': struct, 'decompress': decompress, 'hash64': hash64, 'REFPACK': REFPACK,
           'LAYER': LAYER, 'SLOT': 0xEB000F, 'QUADTREE': 0xEB0010, 'DICTIONARY': 0xEB0011}
for node in ast.parse((here/'old_ambience.py').read_text(encoding='utf-8')).body:
    if isinstance(node, ast.FunctionDef) and node.name in ('records', 'tiles'):
        exec(compile(ast.Module([node], []), 'old_ambience.py', 'exec'), painter)
records = painter['records']


def tiles(payloads, names, layer=LAYER):
    painter['LAYER'] = layer  # old_ambience's tiles() reads its layer from module globals
    return painter['tiles'](payloads, names)


# ---- roads ----------------------------------------------------------------------------------------------------
def _u32(d, o): return struct.unpack_from('>I', d, o)[0]
def _u64(d, o): return '%016X' % struct.unpack_from('>Q', d, o)[0]
def _f(d, o): return struct.unpack_from('>f', d, o)[0]
def _v3(d, o): return list(struct.unpack_from('>3f', d, o))


def _end(d, o):
    n = _u32(d, o + 16)
    assert n <= 4, n
    counts = [_u32(d, o + 20 + 4*i) for i in range(n)]
    lists = [[_u32(d, o + _u32(d, o + 36 + 4*i) + 4*j) for j in range(counts[i])] if _u32(d, o + 36 + 4*i) else []
             for i in range(n)]
    return {'id': _u64(d, o), 'a': _u32(d, o + 8), 'b': _u32(d, o + 12), 'lanes': lists}


def _curve(d, o):
    n, arc = _u32(d, o + 68), _u32(d, o + 72)
    return {'p0': _v3(d, o), 'p1': _v3(d, o + 16), 't0': _v3(d, o + 32), 't1': _v3(d, o + 48), 'length': _f(d, o + 64),
            'arc': [_f(d, o + arc + 4*i) for i in range(n)] if arc else []}


def rain(d):
    ni, nr, ns = struct.unpack_from('>III', d, 0x20)
    pi, pr, ps = struct.unpack_from('>III', d, 0x2C)
    supers = [{'id': _u64(d, o), 'a': _u64(d, o + 8), 'a_n': _u32(d, o + 16), 'b': _u64(d, o + 24), 'b_n': _u32(d, o + 32),
               'length': _f(d, o + 36), 'w40': _f(d, o + 40), 'w44': _f(d, o + 44), 'speed': _f(d, o + 48),
               'lanes_ab': _u32(d, o + 52), 'lanes_ba': _u32(d, o + 56)} for o in (ps + 64*i for i in range(ns))]
    roads = []
    for o in (pr + 160*i for i in range(nr)):
        pieces = []
        for q in (o + _u32(d, o + 124) + 224*k for k in range(_u32(d, o + 120))):
            piece = _curve(d, q)
            piece.update(edges=[_v3(d, q + 80 + 16*j) for j in range(4)], edge_t=[_v3(d, q + 144 + 16*j) for j in range(4)],
                         edge_length=_f(d, q + 208))
            pieces.append(piece)
        roads.append({'id': _u64(d, o), 'ends': [_end(d, o + 8), _end(d, o + 64)], 'super': _u64(d, o + 152),
                      'w128': [_u32(d, o + 128 + 4*j) for j in range(6)], 'pieces': pieces})
    inters = []
    for o in (pi + 608*i for i in range(ni)):
        lanes = []
        for q in (o + _u32(d, o + 592) + 112*k for k in range(_u32(d, o + 596))):
            lane = _curve(d, q)
            lane.update(speed=_f(d, q + 80), entry=_u32(d, q + 84), w88=_u32(d, q + 88), exit=_u32(d, q + 92), w96=_u32(d, q + 96))
            lanes.append(lane)
        inters.append({'id': _u64(d, o + 584), 'speed': _f(d, o + 576), 'w580': _u32(d, o + 580),
                       'inner': [_v3(d, o + 16*j) for j in range(4)], 'outer': [_v3(d, o + 64 + 16*j) for j in range(4)],
                       'entries': [_end(d, o + 128 + 56*j) for j in range(4)], 'exits': [_end(d, o + 352 + 56*j) for j in range(4)],
                       'lanes': lanes})
    return supers, roads, inters


def rw_objects(data, kind):
    if data[:4] != b'\x89RW4':
        return
    count, table = _u32(data, 0x20), _u32(data, 0x30)
    for i in range(count):
        offset, _, length, _, _, k = struct.unpack_from('>6I', data, table + 24*i)
        if k == kind:
            yield data[offset:offset + length]


def network(payloads):
    """Every RAIN object's records, merged by id; a record found twice must be identical."""
    out, seen = {'superroads': {}, 'roads': {}, 'intersections': {}}, set()
    for data in payloads:
        for obj in rw_objects(data, RAIN):
            digest = hashlib.blake2b(obj, digest_size=16).digest()
            if digest in seen:
                continue
            seen.add(digest)
            for table, rows in zip(out.values(), rain(obj)):
                for row in rows:
                    assert table.setdefault(row['id'], row) == row, f'RAIN record {row["id"]} differs between cells'
    return {k: list(v.values()) for k, v in out.items()}, len(seen)


# ---- census -------------------------------------------------------------------------------------------------------
def vault(vlt, bin_):
    with tempfile.TemporaryDirectory(prefix='old-traffic-db-') as work:
        (Path(work)/'skater.vlt').write_bytes(vlt)
        (Path(work)/'skater.bin').write_bytes(bin_)
        return Collections(convert(Path(work)/'skater', Path(work)/'skater',
                                   (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()))


def value(field):
    kind = field.get('type', '').split('::')[-1]

    def one(raw):
        if kind == 'Text' or not isinstance(raw, str):
            return raw
        b = bytes.fromhex(raw)
        if kind == 'Float':
            return struct.unpack('>f', b)[0]
        if kind in ('Vector2', 'Vector3', 'Vector4', 'FloatColour') and len(b) % 4 == 0:
            return list(struct.unpack('>%df' % (len(b)//4), b))
        if kind in ('Int32', 'UInt32'):
            return struct.unpack('>i' if kind == 'Int32' else '>I', b[:4])[0]
        if kind in ('Int8', 'UInt8', 'Bool'):
            return struct.unpack('>b' if kind == 'Int8' else '>B', b[:1])[0]
        if kind == 'RefSpec':
            return '%s:%s' % (raw[:16], raw[16:32])
        return raw
    return [one(x) for x in field['array']['items']] if 'array' in field else one(field['data'])


def row(db, cls, key):
    fields, _ = db.resolve(cls, key)
    return {'%016X' % h: value(f) for h, f in sorted(fields.items())}


def ref(text):
    cls, key = text.split(':')
    return int(cls, 16), int(key, 16)


def variants(db, key, recipes):
    """The appearances a vehicle entity spawns as. TRACED: 0x825C6530 maps every appearance row (class
    Hash_4E740A6670F241A7) under the root 'vehicles' whose Bool Hash_A75AF7F3CFABE6E1 is set to its parent, or to
    itself when its parent is the root; 0x825C6918 picks uniformly (0x829F5C30) among the rows mapped to the entity's
    appearance whose model asset is resident (0x825C63B8: state 3). Resident here: the recipe ships in
    livingworld.big (Skate 2's '' and its leftover Skate 1 'trafsuva_kit00_lod' never load)."""
    root = hash64('vehicles')
    parent = {k: key_hash(r['parent']) for (c, k), r in db.rows.items() if c == APPEARANCE_CLASS}
    rows = [k for k, p in parent.items() if (p == key and p != root) or (k == key and p == root)]
    rows = [k for k in rows if row(db, APPEARANCE_CLASS, k).get('%016X' % SPAWNABLE)]
    resident = [k for k in sorted(rows) if row(db, APPEARANCE_CLASS, k).get('%016X' % MODEL) in recipes]
    # No row maps to the key, or none of them is resident: 0x825C6918 hands back the key itself.
    out = [{'key': '%016X' % k, 'model': row(db, APPEARANCE_CLASS, k).get('%016X' % MODEL), 'appearance': row(db, APPEARANCE_CLASS, k)}
           for k in (resident or [key])]
    assert all(v['model'] in recipes for v in out), f'appearance {key:016X}: no model'
    return out


def hom_type(db, e):
    """Hall of Meat vehicle type (Skate 2's Score::HoM vehicle scorer 0x82bb8a90): the entity's RefSpec
    Hash_E356ED00ABF1D7F0 (0x82bb1978) -> that row's u32 Hash_8B670C504A363A7C (0x82bb1a60); none = -1. hom.json's
    `vehicles` table prices a type (Skate 2: 12 and 13, the two trucks, 2000), any other scores vehicle_points."""
    spec = e.get('%016X' % HOM_REF)
    if not spec or ref(spec) not in db.rows:
        return -1
    t = row(db, *ref(spec)).get('%016X' % HOM_TYPE)  # untyped in the vault schema: the raw big-endian hex
    return -1 if t is None else struct.unpack('>i', bytes.fromhex(t) if isinstance(t, str) else struct.pack('>I', t))[0]


def audio_row(db, profile):
    """The Skate 3 audio row the car's sounds read: the collector 0x827A3790 hands the audio side the driver profile's
    RefSpec Hash_BA2DDD830C731EE4 (class Hash_259095163B974174), which SFXObj_TrafficEngine 0x824D6110 looks up in
    Skate 3's vault for its engine type Hash_C436B6BC22BC023C (every other field it reads is the 'default' row's). The
    row itself where Skate 3's vault has it (all of Skate 2's), else Skate 3's row of the same engine type (Skate 1's
    463FC0C75042C154 type 2 -> 'default', 02DDAB06A0F224BE type 1 -> AD9F4E178868DAD6)."""
    spec = profile.get('%016X' % AUDIO_ROW)
    if not spec:
        return None
    raw = spec.replace(':', '')
    cls, key = int(raw[:16], 16), int(raw[16:32], 16)
    if (cls, key) in db3.rows:
        return '%016X' % key

    def engine(d, k):
        t = row(d, AUDIO_CLASS, k).get('%016X' % ENGINE_TYPE)
        return t if isinstance(t, int) else int(t[:4], 16)  # untyped: the raw big-endian u16 (Skate 1 pads it to 4 bytes)
    same = [k for c, k in db3.rows if c == AUDIO_CLASS and engine(db3, k) == engine(db, key)]
    assert len(same) == 1, f'audio row {key:016X}: {len(same)} Skate 3 rows of its engine type'
    return '%016X' % same[0]


def exports(bank):
    """An .abk's exports: bank +0x38 -> u32 count, then 12-byte entries {+0 code offset, +4 record offset, +8 kind};
    each record (u16 project, u16 id, name), which the sound player resolves against the project's .csi."""
    table = _u32(bank, 0x38)
    out = []
    for i in range(_u32(bank, table)):
        at = _u32(bank, table + 4 + 12 * i + 4)
        out.append((at, bank[at + 4:bank.index(b'\0', at + 4)].decode()))
    return out


def readdress(bank, s3bank):
    """`bank` with every export under the (project, id) Skate 3's bank of that slot gives the same name (Skate 3's
    AEMS_TRAFFIC.csi is project 0x2DED with TRAFFIC_CAR 14577, TRAFFIC_HORN 16218, TRAFFIC_SKID 29024; Skate 1's and 2's
    0x7E72 with 27725, 13279, 6660; the Grab_* ids are the same in all three; car_alarms: Sk8_Emitters 10188)."""
    ids = {name: s3bank[at:at + 4] for at, name in exports(s3bank)}
    out = bytearray(bank)
    for at, name in exports(bank):
        out[at:at + 4] = ids[name]
    return bytes(out)


# Skate 1's and 2's SFXObj_TrafficEngine payload (Skate 2 0x82450710, update 0x82334B68) is a word longer than Skate 3's
# (0x824D5B20, update 0x824D6478): its +60 (a dot product x 2000 Skate 3 has no field for) puts the engine type at +64 and
# every later field a word on. A patch copies payload +N from src N + 0x10, so their TRAFFIC_CAR reads Skate 3's fields
# from src 0x50 on (its own +60, src 0x4C, it never reads).
CAR_INPUTS = {0x50: 0x4C, 0x54: 0x50, 0x58: 0x54, 0x5C: 0x58}


def car_inputs(bank, s3bank):
    """`bank` with its TRAFFIC_CAR patch's input copy re-pointed at the words Skate 3's code writes (CAR_INPUTS); it then
    reads exactly the words Skate 3's own patch does. The copy is the patch's first evaluator record {u8 opcode 1, u8
    pairs, u16, pairs x {i32 src, i32 dst}, ...} (skate_audio_core::eval::interp) whose every src is -1 or a payload word
    (under 0x80), found by scanning: the table ahead of it varies in length (nine entries in Skate 1's C02_minivan01).
    Traffic_Skid's payload differs too (Skate 2 0x82450810 has three words Skate 3's 0x824D5C30 lacks): UNTRACED."""
    def copy(b):
        table = _u32(b, 0x38)
        code = next(_u32(b, e) for e in (table + 4 + 12 * i for i in range(_u32(b, table)))
                    if b[_u32(b, e + 4) + 4:].startswith(b'TRAFFIC_CAR\0'))
        for at in range(code, len(b) - 8, 4):
            n = b[at + 1]
            pairs = [struct.unpack_from('>ii', b, at + 4 + 8 * k) for k in range(n)] if b[at] == 1 and n >= 8 else []
            if pairs and all(src == -1 or 0 <= src < 0x80 for src, _ in pairs):
                return at, pairs
        raise ValueError('no TRAFFIC_CAR input copy')
    at, pairs = copy(bank)
    out = bytearray(bank)
    for k, (src, _) in enumerate(pairs):
        if src in CAR_INPUTS:
            struct.pack_into('>i', out, at + 4 + 8 * k, CAR_INPUTS[src])

    def reads(b):
        return sorted({src for src, _ in copy(b)[1] if src != -1})
    assert reads(out) == reads(s3bank), f'TRAFFIC_CAR reads {[hex(x) for x in reads(out)]}'
    return bytes(out)


def sounds(game, folder, vehicle, emitters, engines):
    """audio/<bank>.abk: the banks Skate 3's 0x82487168 loads (its eight engine slots, Traffic_Horn, Traffic_Skid,
    car_alarms) from the game's own vehicle.big (and emitters2.big's car_alarms), each re-addressed (readdress) and named
    after the Skate 3 bank it stands in for, the engines' inputs where Skate 3's code writes them (car_inputs); the engine
    serves them in Skate 3's slots (skate_data::audio::title::with_traffic)."""
    out = assets/'private/traffic'/folder/'audio'
    out.mkdir(parents=True, exist_ok=True)

    def member(big, name):
        return big.read(next(e for e in big.entries if Path(e.path).name.lower() == name.lower()))
    others = ['Traffic_Horn', 'Traffic_Skid']
    pairs = [(vehicle, src, dst) for src, dst in zip(engines + others, ENGINES3 + others)]
    pairs.append((emitters, 'car_alarms', 'car_alarms'))
    for big, src, dst in pairs:
        s3bank = member(audio3, f'{dst}.abk')
        bank = readdress(member(big, f'{src}.abk'), s3bank)
        (out/f'{dst}.abk').write_bytes(car_inputs(bank, s3bank) if dst in ENGINES3 else bank)
    print(f'{game}: {len(pairs)} traffic sound banks written to {out}')


def census(db, names, payloads, recipes, entry_field=ENTRY, group_names=None, painted=None):
    """entry_field: the census row's vehicles tLWCensusEntry (Skate 2 Hash_98F6489E23ED2661, layout +0, read by
    0x823AE878; Skate 1 Hash_3107ABED83835EBF, layout +0, read by 0x824CA0D0 for 0x824DC788's type 2).
    painted: Skate 3's tiles (palette = row keys, `names` = {key: key}); else the layer-5 tiles named through `names`.
    group_names: None = the entry's RefSpec names the group (Skate 2, 0x82333308); else Skate 1's painted groups
    (0x824DC788 asks the painter for slot 4 = layer 4 and 0x824DC8F8 uses that row; the entry's RefSpec is unused)."""
    out = {'layer': LAYER, 'painter': painted if painted is not None else tiles(payloads, names), 'census': {}, 'groups': {}, 'entities': {},
           'group_painter': tiles(payloads, group_names, 4) if group_names else None,
           'circle': row(db, CIRCLE, 'vehicles') if (CIRCLE, hash64('vehicles')) in db.rows else None,
           'livingworld': {r['key']: row(db, 'livingworld', r['key']) for r in db.rows.values() if key_hash(r['class']) == key_hash('livingworld')}}
    assert out['painter'], 'no vehicle census tiles'
    assert group_names is None or out['group_painter'], 'no vehicle group tiles'

    def group(cls, key):
        """D4DD row -> [{weight, entities}]: tLWGroupEntry {RefSpec ADCB list, f32 weight at +24}, each list a RefSpec
        array of livingworld_entities. No row -> no entry: the pick (S2 0x82333308, S1 0x824E8AC0) then fails."""
        if (cls, key) not in db.rows or '%016X' % key in out['groups']:
            return
        groups = out['groups']['%016X' % key] = []
        for item in row(db, cls, key)['%016X' % ITEMS]:
            raw = bytes.fromhex(item)
            entities = [ref(e) for e in row(db, *struct.unpack_from('>QQ', raw, 0))['%016X' % ITEMS]]
            groups.append({'weight': struct.unpack_from('>f', raw, 24)[0], 'entities': ['%016X' % k for _, k in entities]})
            for ecls, ekey in entities:
                if '%016X' % ekey not in out['entities']:
                    e = row(db, ecls, ekey)
                    profile = row(db, *ref(e['%016X' % PROFILE]))
                    out['entities']['%016X' % ekey] = {
                        'name': e.get('%016X' % NAME, ''), 'variants': variants(db, ref(e['%016X' % APPEARANCE])[1], recipes),
                        'profile': profile, 'behaviour': row(db, *ref(e['%016X' % BEHAVIOUR])),
                        'hom': hom_type(db, e), 'audio': audio_row(db, profile)}

    for name in names.values():
        rk = name if isinstance(name, int) else hash64(name)
        if (CENSUS, rk) not in db.rows:  # 0x82331AF0: no row -> the zero layout 0x82E57998, count 0
            print(f'  {name}: no vault row, no traffic there')
            continue
        fields = row(db, CENSUS, rk)
        entry = bytes.fromhex(fields['%016X' % entry_field])
        cls, key = struct.unpack_from('>QQ', entry, 0)
        if group_names is None:
            group(cls, key)
        out['census']['%016X' % rk] = {'name': name if isinstance(name, str) else db.rows[(CENSUS, rk)]['key'], 'count': struct.unpack_from('>i', entry, 24)[0],
                                                 'limit': fields.get('%016X' % LIMIT),
                                                 'group': '%016X' % key if group_names is None else None}
    for name in (group_names or {}).values():
        group(GROUP, hash64(name))
    return out


# ---- vehicle models -----------------------------------------------------------------------------------------------
_decode = R._decode_vertices


def _vertices(d, vb, count, vdesc):
    """rx2_skeleton's decoder plus FLOAT16_4 (0x1A2360) positions, the vehicles' format."""
    out = _decode(d, vb, count, vdesc)
    for e in vdesc['elements']:
        if e['usage_name'] == 'POSITION' and e['format'] == 0x1A2360 and 'positions' not in out:
            out['positions'] = [struct.unpack_from('>3e', d, vb + v*vdesc['stride'] + e['offset']) for v in range(count)]
    return out


R._decode_vertices = _vertices
s1mesh.install(R)


def parse(blob):
    with tempfile.TemporaryDirectory(prefix='old-traffic-rx2-') as work:
        (Path(work)/'m.rx2').write_bytes(blob)
        return R.parse_rx2(str(Path(work)/'m.rx2'))


def _tangent(n, t, b):
    """glTF's TANGENT for one vertex: Bevy rebuilds the binormal as w * cross(N, T) (calculate_tbn_mikktspace), Skate 1's
    pixel shader reads the mesh's own BINORMAL, so w is the side of the N-T plane it lies on (one on the plane: +1).
    ponytail: a binormal off the N x T axis (|cos| < 0.9 on 13% of Honda_Fit's chassis vertices, where the map's y tilt
    averages 0.06) still bends along +-N x T; only a car material that reads BINORMAL itself would match retail there."""
    c = (n[1]*t[2] - n[2]*t[1], n[2]*t[0] - n[0]*t[2], n[0]*t[1] - n[1]*t[0])
    return [*t, 1.0 if c[0]*b[0] + c[1]*b[1] + c[2]*b[2] >= 0 else -1.0]


def vehicle(big, files, recipe, parser, out, normals=None):
    """normals: the normal map's packing, 'xy' (Skate 3's i_normal) or 'rgb' (Skate 1's); None drops it (Skate 2: unchecked)."""
    from tools.asset_pipeline.character_glb import Glb
    from tools.asset_pipeline.customisation_library import decode_png, _derive
    root = ET.fromstring(big.read(files[f'data/content/recipe/vehicle/{recipe}.xml']))
    mats = {m.get('id'): m for m in root.iter('mat')}
    glb, textures, nodes = Glb(), {}, {}

    def material(mat_id):
        mat = mats[mat_id]
        channels = {sp.get('chn'): sp.get('id') for sp in mat.iter('sp')}
        glass = mat.get('type') == 'vehicle_glass' or 'transparency' in channels
        pbr = {'metallicFactor': 0.0, 'roughnessFactor': 1.0}
        tex = channels.get('diffuse')
        if tex:
            if tex not in textures:
                with tempfile.TemporaryDirectory(prefix='old-traffic-tex-') as work:
                    png = Path(work)/'t.png'
                    decode_png(parser, big.read(files[f'data/content/vehicle/texture/{tex}.rx2']), png, tex + '.rx2')
                    textures[tex] = glb.texture(png.read_bytes())
            pbr['baseColorTexture'] = textures[tex]
        extra = {}
        # Skate 3's i_normal (bug 10-17 #7 / #42: the cars shaded flat): x in alpha, y in grey, as the CAS normal maps,
        # so the same conversion (checked on hatchback_01). Skate 1's 'normal' is a plain DXT1 RGB map (Honda_Fit's
        # 0x0000c3c703e3874b: mean (127, 128, 253), alpha 255), used as it is. Skate 2's packing is unchecked.
        # ponytail: i_environment (one shared cube map) is left out: StandardMaterial has no per-material cube map.
        normal = channels.get('normal') if normals else None
        if normal and f'data/content/vehicle/texture/{normal}.rx2' in files:
            key = ('normal', normal)
            if key not in textures:
                with tempfile.TemporaryDirectory(prefix='old-traffic-tex-') as work:
                    raw, png = Path(work)/'n_raw.png', Path(work)/'n.png'
                    image = decode_png(parser, big.read(files[f'data/content/vehicle/texture/{normal}.rx2']), raw, normal + '.rx2')
                    if normals == 'xy':
                        _derive(raw, png, 'normal', image)
                    textures[key] = glb.texture((png if normals == 'xy' else raw).read_bytes())
            extra['normalTexture'] = textures[key]
        glb.doc['materials'].append({'name': mat.get('type'), 'pbrMetallicRoughness': pbr, 'doubleSided': False,
                                     'alphaMode': 'BLEND' if glass else 'OPAQUE', **extra})
        return len(glb.doc['materials']) - 1

    material_index = {}
    for comp in root.iter('comp'):
        for mod in comp.iter('mod'):
            lod = next((l for l in mod.iter('lod') if l.get('idx') == '0'), None)
            if lod is None:
                continue
            variants = [v.get('id') for v in lod.iter('matvar')]
            parsed = parse(big.read(files[f'data/content/vehicle/model/{recipe}/{comp.get("n")}/{lod.get("arenaid")}.rx2']))
            bones = parsed['bones']
            meshes = [m for m in parsed['meshes'] if m.get('positions') and m.get('indices')]
            assert meshes and len(variants) in (1, len(meshes)), (recipe, len(variants), len(meshes))
            for i, mesh in enumerate(meshes):
                mat_id = variants[min(i, len(variants) - 1)]
                if mat_id not in material_index:
                    material_index[mat_id] = material(mat_id)
                skin = [b[0] for b in mesh['skin_bones']]
                tris = [mesh['indices'][t:t + 3] for t in range(0, len(mesh['indices']) - 2, 3)]
                by_bone = {}
                for tri in tris:
                    by_bone.setdefault(skin[tri[0]], []).append(tri)
                for bone, part in by_bone.items():
                    used = sorted({v for tri in part for v in tri})
                    remap = {v: j for j, v in enumerate(used)}
                    name = bones[bone]['name']
                    pivot = [0.0, 0.0, 0.0] if 'wheel' not in name.lower() else list(bones[bone]['bind_pos'])
                    attributes = {
                        'POSITION': glb.accessor([[p - c for p, c in zip(mesh['positions'][v], pivot)] for v in used], 'VEC3', bounds=True),
                        'NORMAL': glb.accessor([mesh['normals'][v] for v in used], 'VEC3'),
                        'TEXCOORD_0': glb.accessor([mesh['uvs'][v] for v in used], 'VEC2')}
                    if normals == 'rgb' and mesh.get('tangents') and mesh.get('binormals'):
                        attributes['TANGENT'] = glb.accessor(
                            [_tangent(mesh['normals'][v], mesh['tangents'][v], mesh['binormals'][v]) for v in used], 'VEC4')
                    nodes.setdefault(name, (pivot, []))[1].append({
                        'attributes': attributes,
                        'indices': glb.accessor([remap[v] for tri in part for v in tri], 'SCALAR', 5125),
                        'material': material_index[mat_id]})
    glb.doc['meshes'] = [{'name': name, 'primitives': prims} for name, (_, prims) in nodes.items()]
    glb.doc['nodes'] = [{'name': 'Vehicle_Root', 'children': list(range(1, len(nodes) + 1))}] + [
        {'name': name, 'mesh': i, **({'translation': pivot} if any(pivot) else {})} for i, (name, (pivot, _)) in enumerate(nodes.items())]
    glb.doc['scenes'] = [{'nodes': [0]}]
    glb.doc['scene'] = 0
    glb.save(out)
    return sorted(nodes)


def export(game, folder, big, db, names, payloads, normals=None, **schema):
    out = assets/'private/traffic'/folder
    (out/'vehicles').mkdir(parents=True, exist_ok=True)
    payloads = list(payloads)
    roads, objects = network(payloads)
    (out/'roads.json').write_text(json.dumps(roads), encoding='utf-8')
    files = {e.path: e for e in big.entries}
    recipes = sorted(Path(p).stem for p in files if p.startswith('data/content/recipe/vehicle/') and p.endswith('.xml'))
    data = census(db, names, payloads, set(recipes), **schema)
    (out/'census.json').write_text(json.dumps(data), encoding='utf-8')
    from tools.extract_default_skater import import_rx2_parser
    parser = import_rx2_parser(repo/'tools/vendor/utt')
    models = sorted({v['model'] for e in data['entities'].values() for v in e['variants']})
    for recipe in models:  # only what the census can spawn (livingworld.big also holds unused recipes, e.g. reda_car)
        vehicle(big, files, recipe, parser, out/'vehicles'/f'{recipe}.glb', normals)
    print(f'{game}: {objects} RAIN objects -> {len(roads["roads"])} roads, {len(roads["superroads"])} superroads, '
          f'{len(roads["intersections"])} intersections; {len(data["painter"])} census tiles, {len(data["census"])} census rows, '
          f'{len(data["entities"])} vehicles; {len(models)} models written to {out}')


# Skate 3's vault (audio_row) and audio archive (sounds) first; its traffic at the end.
from tools.owned_game.big import BigArchive
with tempfile.TemporaryDirectory(prefix='old-traffic-s3db-') as work:
    db_big = BigArchive(s3game/'data/big/db.big')
    for e in db_big.entries:
        if Path(e.path).name in ('skaterschema.vlt', 'skaterschema.bin', 'skatercollections.vlt', 'skatercollections.bin'):
            (Path(work)/Path(e.path).name).write_bytes(db_big.read(e))
    db3 = Collections(convert(Path(work)/'skaterschema', Path(work)/'skatercollections',
                              (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()))
audio3 = BigArchive(s3game/'data/audio/audiofiles.big')

# Skate 2
db_big = Big(s2game/'data/db/db.big')
skater = {Path(e.path).name: db_big.read(e) for e in db_big.entries if Path(e.path).name in ('skater.vlt', 'skater.bin')}
world = Big(s2game/'data/content/worldbam.big')


def cells():
    for entry in world.entries:
        if '/cSim_' in entry.path:
            blob = world.read(entry)
            yield from records(lambda at, n: blob[at:at + n], len(blob))


export('Skate 2', 'skate2', Big(s2game/'data/content/livingworld.big'), vault(skater['skater.vlt'], skater['skater.bin']),
       SKATE2_CENSUS, cells())
sounds('Skate 2', 'skate2', Big(s2game/'data/audio/vehicle.big'), Big(s2game/'data/audio/emitters2.big'), ENGINES3)

# Skate 1
with open(s1game/'data/content/world/stream/BAM.xsf', 'rb') as xsf:
    stream = list(records(lambda at, n: (xsf.seek(at), xsf.read(n))[1], xsf.seek(0, 2)))
export('Skate 1', 'skate1', Big(s1game/'data/content/livingworld.big'),
       vault((s1game/'data/db/skater.vlt').read_bytes(), (s1game/'data/db/skater.bin').read_bytes()), SKATE1_CENSUS, stream,
       normals='rgb', entry_field=ENTRY1, group_names=SKATE1_GROUPS)
sounds('Skate 1', 'skate1', Big(s1game/'data/audio/vehicle.big'), Big(s1game/'data/audio/emitters2.big'), ENGINES1)


# Skate 3
from tools.asset_pipeline import audio_world
audio_world.LAYERS = {hash64('livingworld_vehicle_census'): 'census'}  # arena_layers() keeps only the layers named here
s3 = assets/'private/traffic/skate3'
lw3 = BigArchive(s3game/'data/content/livingworld.big')
files3 = {e.path: e for e in lw3.entries}
recipes3 = {Path(p).stem for p in files3 if p.startswith('data/content/recipe/vehicle/') and p.endswith('.xml')}
models3 = set()
for district in sorted((s3game/'data/content').glob('worldDIST_*.big')):
    world3 = BigArchive(district)
    payloads, painted = [], []
    for entry in world3.entries:  # the Sim stream, as audio_world.district_layers reads it
        path = entry.path.lower()
        if not path.endswith('.xsf') or not ('_sim.' in path or '/csim_' in path):
            continue
        blob = world3.read(entry)
        for data in records(lambda at, n: blob[at:at + n], len(blob)):
            payloads.append(data)
            painted += [dict(tile, high=path.endswith('_high.xsf')) for _, tile in audio_world.arena_layers(data)]
    roads, objects = network(payloads)
    if not roads['roads']:
        continue  # no road network (SkateSchool is painted, but nothing can drive there)
    keys = {int(k, 16): int(k, 16) for t in painted for k in t['palette'] if int(k, 16)}
    data = census(db3, keys, payloads, recipes3, painted=painted)
    out = s3/district.stem.removeprefix('worldDIST_')
    out.mkdir(parents=True, exist_ok=True)
    (out/'roads.json').write_text(json.dumps(roads), encoding='utf-8')
    (out/'census.json').write_text(json.dumps(data), encoding='utf-8')
    models3 |= {v['model'] for e in data['entities'].values() for v in e['variants']}
    print(f'Skate 3 {out.name}: {objects} RAIN objects -> {len(roads["roads"])} roads, {len(roads["superroads"])} superroads, '
          f'{len(roads["intersections"])} intersections; {len(painted)} census tiles, {len(data["census"])} census rows, '
          f'{len(data["entities"])} vehicles')
assert {'DownTown', 'Industrial', 'University'} <= {d.name for d in s3.iterdir() if d.is_dir()}, 'Skate 3 roads missing'
(s3/'vehicles').mkdir(parents=True, exist_ok=True)
from tools.extract_default_skater import import_rx2_parser
parser3 = import_rx2_parser(repo/'tools/vendor/utt')
for recipe in sorted(models3):
    vehicle(lw3, files3, recipe, parser3, s3/'vehicles'/f'{recipe}.glb', normals='xy')
print(f'Skate 3: {len(models3)} models written to {s3/"vehicles"}')
