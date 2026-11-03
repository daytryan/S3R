import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import sys, re, json
from pathlib import Path
repo = Path(REPO_ROOT)
sys.path[:0] = [str(repo)]
from tools.asset_pipeline.vlt import hash64
img = Path(env('SKATE_TABLES', 'the skate2port tables folder'), 'work/s2_default.img').read_bytes()
strs = set(s.decode() for s in re.findall(rb'[A-Za-z_][A-Za-z0-9_:./ -]{2,80}', img))
want = set(sys.argv[1:])
found = {}
for s in strs:
    h = 'Hash_%016X' % hash64(s)
    if h in want: found[h] = s
for w in sys.argv[1:]: print(w, found.get(w))
print(len(strs), [s for s in strs if 'HoM' in s or 'HOM' in s][:80])
