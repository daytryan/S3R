"""Checks exported GLBs keep Skate 2's face rig (faces.md section 6, Python checks 1-2).

usage: py -3 check_cast_faces.py GLB [GLB ...]   (exit 1 on the first failure)
"""
import json, struct, sys
from pathlib import Path

# The head rx2's face: Face, the jaw chain, and the 27 leaves parented to Face (faces.md section 2.1).
FACE_JOINTS = [
    'FACE', 'OFFSET_JAW', 'JAW', 'OFFSET_CHIN', 'CHIN', 'TONGUE', 'TONGUETIP',
    'LEFTEYE', 'RIGHTEYE', 'LEFTUPEYELID', 'RIGHTUPEYELID', 'LEFTLOWEYELID', 'RIGHTLOWEYELID',
    'LEFTINNEREYEBROW', 'RIGHTINNEREYEBROW', 'LEFTOUTEREYEBROW', 'RIGHTOUTEREYEBROW',
    'LEFTCHEEK', 'RIGHTCHEEK', 'LEFTUPCHEEK', 'RIGHTUPCHEEK', 'LEFTNOSE', 'RIGHTNOSE',
    'LEFTCREASE', 'RIGHTCREASE', 'LEFTMOUTH', 'RIGHTMOUTH',
    'UPPERLIP', 'LOWERLIP', 'LEFTUPPERLIP', 'RIGHTUPPERLIP', 'LEFTLOWERLIP', 'RIGHTLOWERLIP',
]


def glb(path):
    data = Path(path).read_bytes()
    length = struct.unpack_from('<I', data, 12)[0]
    doc = json.loads(data[20:20 + length])
    rest = data[20 + length:]
    blob = rest[8:8 + struct.unpack_from('<I', rest, 0)[0]] if rest else b''
    return doc, blob


def accessor(doc, blob, index):
    a = doc['accessors'][index]; v = doc['bufferViews'][a['bufferView']]
    width = {'SCALAR': 1, 'VEC2': 2, 'VEC3': 3, 'VEC4': 4}[a['type']]
    fmt = {5121: 'B', 5123: 'H', 5125: 'I', 5126: 'f'}[a['componentType']]
    size = struct.calcsize(fmt); stride = v.get('byteStride') or size * width
    base = v.get('byteOffset', 0) + a.get('byteOffset', 0)
    return [struct.unpack_from('<' + fmt * width, blob, base + i * stride) for i in range(a['count'])]


def check(path, joints=FACE_JOINTS):
    doc, blob = glb(path)
    names = [doc['nodes'][j].get('name', '').upper() for j in doc['skins'][0]['joints']]
    missing = [n for n in joints if n not in names]
    assert not missing, f'{path}: no face joints {missing}'
    # The eye mesh moves with the eyes only, never TONGUE or JAW (faces.md 4A). The export has one unnamed mesh; the
    # eyes are the primitive whose material is Retail_Organ.
    for mesh in doc['meshes']:
        for prim in mesh['primitives']:
            if doc['materials'][prim['material']].get('name') != 'Retail_Organ':
                continue
            used = {names[j] for js, ws in zip(accessor(doc, blob, prim['attributes']['JOINTS_0']),
                                               accessor(doc, blob, prim['attributes']['WEIGHTS_0']))
                    for j, w in zip(js, ws) if w > 0}
            assert used <= {'LEFTEYE', 'RIGHTEYE'}, f'{path}: eye mesh weighted to {sorted(used)}'
    return len(names)


if __name__ == '__main__':
    for p in sys.argv[1:]:
        print(p, check(p), 'joints')
