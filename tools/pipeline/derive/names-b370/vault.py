import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import sys, json
from pathlib import Path
repo = Path(REPO_ROOT)
sys.path[:0] = [str(repo), str(repo/'tools'), env('SKATE_TABLES', 'the skate2port tables folder')]
from s2big import Big
from tools.asset_pipeline.vlt import convert
out = Path(sys.argv[1])
db = Big(env('SKATE2_DISC', 'the extracted Skate 2 disc folder') + '/data/db/db.big')
for e in db.entries: (out/Path(e.path).name).write_bytes(db.read(e))
names = (repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
extra = Path(env('SKATE_TABLES', 'the skate2port tables folder'), 's2_names.txt')
names += extra.read_text(encoding='utf-8', errors='replace').splitlines()
for stem in ['skater', 'ChallengeCore', 'progressioncore']:
    data = convert(out/"skater", out/stem, names)
    (out/f'{stem}.json').write_text(json.dumps(data, indent=1, default=str), encoding='utf-8')
    print(stem, len(json.dumps(data, default=str)))
