"""Skate 2 and Skate 1 gameplay cameras for the engine's Camera Style setting.

Writes assets/private/camera-styles/<skate2|skate1>/{collections.json, Default_cameragraph.stategraph}.
The engine overlays collections.json's camera classes on the stock Skate 3 collections and runs
the style's camera graph, so each style is that game's own shots, rig settings and shot choices.

Skate 2: its vault's camera classes and its shipped Default_cameragraph.stategraph (Low = type 0,
High = type 1, as in Skate 3). Skate 1: its Lua shot list (data/script/camera/Default_definitions.lua,
angles in radians -> degrees), its vault's rig settings and Default_cameragraph.xml compiled by
stategraph.py. Skate 1 had one camera, so its graph has no Low/High split.

Fields a game predates keep Skate 3's value, except the tenth shot reference weight
(Hash_D70C95EDC16A7DFC): weights are normalised, so a reference that did not exist weighs 0.

usage: py old_cameras.py REPO SKATE2_ROOT SKATE1_ROOT ASSETS
"""
import json, math, os, re, struct, sys, tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

repo, s2game, s1game, assets = map(Path, sys.argv[1:5])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(here)]
from s2big import Big
from stategraph import compile_graph, decompile
from tools.asset_pipeline.vlt import convert, hash64
from tools.asset_pipeline.environment import NAMES

CLASSES = ('camera', 'camera_compass', 'camera_droppredictor', 'camera_lag_profile', 'camera_positioner',
           'camera_shake', 'camera_shots', 'camera_tracker')
SHOTS = 'camera_shots'
SHOT_CLASS = '%016X' % hash64(SHOTS)
ABSENT_WEIGHT = 'Hash_D70C95EDC16A7DFC'
stock = json.loads((assets/'private/stock/skater-collections.json').read_text())['collections']


def vault_rows(stem):
    names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
    return [r for r in convert(stem, stem, [*names, *NAMES])['collections'] if r['class'] in CLASSES]


def flatten(rows):
    """Every row with its inherited fields made explicit (parents resolved within `rows`)."""
    by_key = {(r['class'], r['key']): r for r in rows}
    out = []
    for r in rows:
        fields, seen, cur = {}, set(), r
        while cur and (cur['class'], cur['key']) not in seen:
            seen.add((cur['class'], cur['key']))
            for name, value in cur['fields'].items():
                fields.setdefault(name, value)
            cur = by_key.get((cur['class'], cur['parent'])) if cur['parent'] else None
        out.append({**r, 'parent': '', 'fields': fields})
    return out


STOCK = {(r['class'], r['key']): r for r in flatten([r for r in stock if r['class'] in CLASSES])}


def fill(rows, source):
    """Add the fields (and non-shot rows) a game predates from Skate 3; report what was borrowed."""
    have = {(r['class'], r['key']) for r in rows}
    borrowed = {}
    for r in rows:
        template = STOCK.get((r['class'], r['key'])) or next(v for k, v in STOCK.items() if k[0] == r['class'])
        for name, value in template['fields'].items():
            if name not in r['fields']:
                r['fields'][name] = {'type': 'EA::Reflection::Float', 'data': '00000000'} if name == ABSENT_WEIGHT else value
                borrowed.setdefault(r['class'], set()).add(name)
        r['source'], r['sha256'] = source, ''
    for key, row in STOCK.items():
        if key not in have and key[0] != SHOTS:
            rows.append({**row, 'source': 'skate3 (not in ' + source + ')'})
            borrowed.setdefault(key[0], set()).add('row ' + key[1])
    return {k: sorted(v) for k, v in borrowed.items()}


def write(style, rows, graph, source):
    borrowed = fill(rows, source)
    out = assets/'private/camera-styles'/style
    out.mkdir(parents=True, exist_ok=True)
    (out/'collections.json').write_text(json.dumps({'version': 1, 'collections': rows}, separators=(',', ':')))
    (out/'Default_cameragraph.stategraph').write_bytes(graph)
    shots = [r for r in rows if r['class'] == SHOTS]
    print(style, len(shots), 'shots,', len(rows) - len(shots), 'rig rows; from Skate 3:',
          {k: (v if len(v) < 6 else f'{len(v)} fields') for k, v in borrowed.items()})


def skate2():
    db = Big(s2game/'data/db/db.big')
    boot = Big(s2game/'data/big/miscload.big')
    graph = next(boot.read(e) for e in boot.entries if e.path.endswith('camera/Default_cameragraph.stategraph'))
    with tempfile.TemporaryDirectory(prefix='skate2-db-') as work:
        for e in db.entries:
            if Path(e.path).stem == 'skater':
                (Path(work)/Path(e.path).name).write_bytes(db.read(e))
        rows = flatten(vault_rows(Path(work)/'skater'))
    assert not any(r['key'].startswith('Hash_') for r in rows if r['class'] == SHOTS)
    write('skate2', rows, graph, 'skate2 skater.vlt')


# Lua Shot{} field -> (camera_shots field, conversion). Angles are radians in the Lua file.
F, DEG = 'EA::Reflection::Float', math.degrees
LUA = {
    'ArmLength': ('PositionDistance', F, float), 'LensLength': ('FramingLensLength', F, float),
    'DirectionSmoothing': ('SmoothingDirection', F, float), 'ElevationSmoothing': ('SmoothingElevation', F, float),
    'PanSmoothing': ('SmoothingYaw', F, float), 'TiltSmoothing': ('SmoothingPitch', F, float),
    'HeadWeight': ('ReferenceWeightHead', F, float), 'HipsWeight': ('ReferenceWeightHips', F, float),
    'GrindWeight': ('ReferenceWeightGrind', F, float), 'COMWeight': ('ReferenceWeightCOM', F, float),
    'BoardWeight': ('ReferenceWeightBoard', F, float), 'TrajectoryWeight': ('ReferenceWeightTrajectory', F, float),
    'AnchorWeight': ('ReferenceWeightAnchor', F, float),
    'Heading': ('PositionHeading', F, lambda v: DEG(float(v))), 'Elevation': ('PositionElevation', F, lambda v: DEG(float(v))),
    'FixedDutch': ('FramingRoll', F, lambda v: DEG(float(v))), 'FramingX': ('FramingYaw', F, lambda v: DEG(float(v))),
    'FramingY': ('FramingPitch', F, lambda v: DEG(float(v))), 'WorldHeading': ('WorldHeading', F, lambda v: DEG(float(v))),
    'ShotBlur': ('FXBlurShot', F, float), 'TransitionBlur': ('FXBlurTransition', F, float),
    'Opacity': ('FXSubjectOpacity', F, float), 'TransitionTime': ('TransitionTime', F, float),
    'BlendPoint1': ('BlendPoint1', F, float), 'BlendPoint2': ('BlendPoint2', F, float),
    'BlendPoint3': ('BlendPoint3', F, float), 'BlendValue': ('BlendValue', F, float),
    'BlendSmoothing': ('SmoothingBlendValue', F, float),
}
FLAGS = {'FollowSubjectInAir': 'OptionFollowSubjectInAir', 'MirrorForStance': 'OptionMirrorForStance',
         'SnapToReferencePoint': 'OptionSnapToReferencePoint', 'UsePreviousShot': 'OptionUsePreviousShot'}
ENUMS = {'ShotType': 'ShotType', 'Collision': 'OptionCollisionType', 'Anchor': 'OptionAnchor',
         'CompassNorth': 'OptionCompassNorth', 'BlendType': 'BlendType'}
# fileFormat06112700.lua defaults, for fields a shot leaves out.
DEFAULTS = {'ShotType': '0', 'TransitionTime': '1', 'ElevationSmoothing': '0', 'DirectionSmoothing': '0',
            'Heading': str(math.pi), 'Elevation': '0', 'ArmLength': '1', 'FixedDutch': '0', 'LensLength': '20',
            'MirrorForStance': 'false', 'Anchor': '0', 'UsePreviousShot': 'false', 'Collision': '0',
            'FramingX': '0', 'FramingY': '0', 'PanSmoothing': '0.95', 'TiltSmoothing': '0.45',
            'BlendValue': '0.5', 'BlendType': '0', 'BlendSmoothing': '0', 'CompassNorth': '0',
            'FollowSubjectInAir': 'false', 'ShotBlur': '1', 'TransitionBlur': '1', 'Opacity': '1',
            'SnapToReferencePoint': 'false', 'WorldHeading': '0'}
# The file's own 180 default is degrees from an older format; every authored heading is radians.


def lua_shots(text):
    shots = []
    for body in re.findall(r'Shot\{(.*?)\}', text):
        s = {k: v.strip().strip('"') for k, v in re.findall(r'(\w+)\s*=\s*("[^"]*"|[^,]+)', body)}
        if s.get('name') and s['name'] != 'NewShot':
            shots.append({**DEFAULTS, **s})
    return shots


def skate1_rows():
    shots = lua_shots((WORK_ROOT/'cam/s1/Default_definitions.lua').read_text()
                      if (WORK_ROOT/'cam/s1/Default_definitions.lua').is_file() else _s1_file('Default_definitions.lua').decode())
    names = {s['name'] for s in shots}
    by_lower = {n.lower(): n for n in names}
    types = {k: v['type'] for k, v in STOCK[(SHOTS, 'bl_chase')]['fields'].items()}
    rows = []
    for s in shots:
        fields = {'CollectionName': {'type': 'EA::Reflection::Text', 'data': s['name']}}
        for lua, (name, kind, conv) in LUA.items():
            fields[name] = {'type': kind, 'data': struct.pack('>f', conv(s.get(lua, '0'))).hex().upper()}
        for lua, name in FLAGS.items():
            fields[name] = {'type': types[name], 'data': '01' if s[lua].lower() == 'true' else '00'}
        for lua, name in ENUMS.items():
            fields[name] = {'type': types[name], 'data': '%08X' % int(float(s[lua]))}
        for i in (1, 2, 3):
            child = s.get(f'BlendShot{i}', '')
            if child and child.lower() not in by_lower:
                # BL_MegaParkRunIn names two shots the file never defines; the graph never picks it.
                print(f'skate1: {s["name"]} blend shot {child} is not defined; left empty')
            child = by_lower.get(child.lower(), '')  # the file's references ignore case
            fields[f'BlendShot{i}'] = {'type': 'Attrib::RefSpec',
                                       'data': SHOT_CLASS + ('%016X' % hash64(child) if child else '0'*16) + '0'*16}
        extra = [k for k in ('BlendShot4', 'BlendShot5') if s.get(k)] + \
                [k for k in ('BlendPoint4', 'BlendPoint5') if float(s.get(k, 0))]
        assert not extra, (s['name'], extra)  # the engine's shots blend at most three children
        rows.append({'class': SHOTS, 'key': s['name'], 'parent': '', 'fields': fields})
    # ponytail: the graph's GrindExitAirMed picks "Low_Air", which the file never defines; its
    # sibling states pick "LowAir". The engine needs every chosen shot, so alias the typo.
    low_air = next(r for r in rows if r['key'] == 'LowAir')
    rows.append({**low_air, 'key': 'Low_Air', 'fields': {**low_air['fields'],
                 'CollectionName': {'type': 'EA::Reflection::Text', 'data': 'Low_Air'}}})
    return rows


def _s1_file(name):
    big = Big(s1game/'data/big/miscload.big')
    return next(big.read(e) for e in big.entries if e.path.endswith('script/camera/' + name))


def skate3_offboard():
    """Skate 3's OffBoard camera state and the shots it picks. Skate 1 has no off-board skating, so its graph
    has nothing for walking; with this the Skate 1 camera hands over to Skate 3's until the skater is back on."""
    stock_graph = decompile((assets/'private/stock/data/script/camera/Default_cameragraph.stategraph').read_bytes())
    # CameraHigh's and CameraLow's OffBoard states differ only in one state's name.
    high = next(s for s in stock_graph if s.get('name') == 'CameraHigh')
    offboard = next(s for s in high if s.get('name') == 'OffBoard')
    # Skate 3 ranks WipeOut above OffBoard; Skate 1's WipeOut sits under its OnBoard, so say it here.
    expression = offboard.find('expression')
    assert [c.get('name') for c in expression] == ['IsOffboard'], ET.tostring(expression)
    expression.set('op', 'and')
    ET.SubElement(ET.SubElement(expression, 'expression', op='not'), 'condition', name='IsWipingOut', mask='always')
    names = {v for b in offboard.iter('behaviour') for k, v in b.attrib.items() if k.startswith('shot')}
    for name in names:  # blended shots would need their children carried too
        fields = STOCK[(SHOTS, name)]['fields']
        assert all(fields.get(f'BlendShot{i}', {}).get('data', '0'*48)[16:32] == '0'*16 for i in (1, 2, 3)), name
    return offboard, [{**STOCK[(SHOTS, n)], 'source': 'skate3 off-board'} for n in sorted(names)]


def skate1():
    rig = [r for r in flatten(vault_rows(s1game/'data/db/skater')) if r['class'] != SHOTS]
    graph = Path(tempfile.mkdtemp(prefix='skate1-cam-'))/'Default_cameragraph.xml'
    xml = _s1_file('Default_cameragraph.xml')
    # The disc file ends without the root </state>; Skate 1's reader closed it at end of file.
    unclosed = len(re.findall(rb'<state[ >]', xml)) - xml.count(b'</state>')
    assert unclosed in (0, 1), unclosed
    root = ET.fromstring(xml + b'\n</state>\n' * unclosed)
    offboard, offboard_shots = skate3_offboard()
    root.insert(0, offboard)  # the root is a Priority list and Skate 1's OnBoard has no condition
    ET.ElementTree(root).write(graph, encoding='utf-8')
    write('skate1', skate1_rows() + offboard_shots + rig, compile_graph(graph),
          'skate1 skater.vlt + Default_definitions.lua + skate3 off-board')


if __name__ == '__main__':
    assert '%016X' % hash64('camera_shots') == 'F27DD93E059EF6CB'
    skate2()
    skate1()
