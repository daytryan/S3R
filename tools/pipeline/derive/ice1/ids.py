import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import re, sys, json
sys.path[:0] = [REPO_ROOT, env('SKATE_TABLES', 'the skate2port tables folder')]
from s2big import Big
from lk import s
S1 = env('SKATE1_DISC', 'the extracted Skate 1 disc folder')
ids = {}
b = Big(S1 + r'\data\fe\screens.big')
for e in b.entries:
    if e.path.endswith(('.apt', '.const')):
        for m in re.findall(rb'ID_[A-Z0-9_]+', b.read(e)):
            ids.setdefault(m.decode(), set()).add(e.path.split('/')[-1])
for m in re.findall(rb'ID_[A-Z0-9_]+', open(S1 + r'\data\db\skater.bin', 'rb').read()):
    ids.setdefault(m.decode(), set()).add('skater.bin')
json.dump({k: [s(k), sorted(v)] for k, v in ids.items()}, open('ids.json', 'w', encoding='utf-8'), ensure_ascii=False, indent=0)
pat = re.compile(sys.argv[1]) if len(sys.argv) > 1 else None
for k in sorted(ids):
    if pat and pat.search(k):
        print(k, '=', repr(s(k)), sorted(ids[k])[:4])
