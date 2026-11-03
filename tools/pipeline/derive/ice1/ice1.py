import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import sys, struct, json
from pathlib import Path
HERE = Path(env('SKATE_TABLES', 'the skate2port tables folder'))
REPO = Path(REPO_ROOT)
S2 = Path(env('SKATE2_DISC', 'the extracted Skate 2 disc folder'))
sys.path[:0] = [str(REPO), str(REPO/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(HERE)]
src = (HERE/'old_tutorial.py').read_text(encoding='utf-8')
T = {'__name__': 'old_tutorial_lib', '__file__': str(HERE/'old_tutorial.py')}
argv, sys.argv = sys.argv, ['old_tutorial.py', str(REPO), str(S2), str(Path('.').resolve()/'t2')]
exec(compile(src.split('\nvaults = db_vaults()')[0], str(HERE/'old_tutorial.py'), 'exec'), T)
sys.argv = argv
e = open('ice.bin', 'rb').read()
out = {}
o, end = 8, 8 + struct.unpack_from('<I', e, 4)[0]
while o < end:
    chunk_end = o + 8 + struct.unpack_from('<I', e, o + 4)[0]
    r = o + 8
    while r + 8 <= chunk_end:
        _, size, h, n = struct.unpack_from('<4I', e, r)
        takes, at = [], r + 0x10
        for _ in range(n):
            takes.append(T['ice_take'](e[at:r + 8 + size]))
            at = (at + takes[-1]['size'] + 3) & ~3
        out[h] = takes
        r += 8 + size
    o = chunk_end
print(len(out), 'scenes with takes')
d = open('scene.bin', 'rb').read()
n = struct.unpack_from('>I', d, 8)[0]
names = {}
for i in range(n):
    q = 0x10 + i * 0x68
    names[struct.unpack_from('>I', d, q)[0]] = d[q + 0x28:q + 0x68].split(b'\0')[0].decode()
print('takes without scene record', len(set(out) - set(names)), 'scenes without takes', len(set(names) - set(out)))
for want in sys.argv[1:]:
    h = next(k for k, v in names.items() if v == want)
    cam = T['scene_camera'](out[h])
    print(want, 'space', cam['space'], len(cam['samples']), cam['samples'][0][:7], cam['samples'][-1][:7])
