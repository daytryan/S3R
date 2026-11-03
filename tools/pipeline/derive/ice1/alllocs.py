import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import sys, json, re
from rx import M, E
sys.path.insert(0, REPO_ROOT)
from tools.online_challenge_map import global_locators
out = {}
for n, e in E.items():
    if n.endswith('.rx2') and not n.endswith('_textures.rx2'):
        try:
            for k, v in global_locators(M.read(e)).items():
                out.setdefault(k, dict(v, stream=n))
        except Exception as ex:
            print('fail', n, ex)
S1 = env('SKATE1_DISC', 'the extracted Skate 1 disc folder')
text = open(S1 + r'\data\content\world\stream\BAM.loc', encoding='latin-1').read()
bam = {}
for m in re.finditer(r'<Name>([^<]*)</Name>.*?<Transform>([^<]*)</Transform>', text, re.S):
    f = [float(x) for x in m.group(2).split(',')]
    bam[m.group(1).lower()] = [f[0:4], f[4:8], f[8:12], f[12:16]]
json.dump({'missions': out, 'bam': bam}, open('locs.json', 'w'), indent=0)
print(len(out), 'mission locators;', len(bam), 'BAM')
