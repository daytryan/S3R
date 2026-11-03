import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import re, sys
sys.path[:0] = [REPO_ROOT, env('SKATE_TABLES', 'the skate2port tables folder')]
from s2big import Big
b = Big(env('SKATE1_DISC', 'the extracted Skate 1 disc folder') + r'\data\fe\screens.big')
want = sys.argv[1:]
for e in b.entries:
    if any(w in e.path for w in want) and e.path.endswith(('.apt', '.const')):
        d = b.read(e)
        ss = [m.group().decode() for m in re.finditer(rb'[ -~]{3,}', d)]
        print('==', e.path, len(d))
        print(' | '.join(ss))
