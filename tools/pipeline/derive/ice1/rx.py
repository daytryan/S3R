import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import sys, struct, re
sys.path[:0] = [REPO_ROOT, env('SKATE_TABLES', 'the skate2port tables folder')]
from s2big import Big
M = Big(env('SKATE1_DISC', 'the extracted Skate 1 disc folder') + r'\data\big\missions.big')
E = {e.path.split('/')[-1].lower(): e for e in M.entries}
def sections(d):
    count, table_at = struct.unpack_from('>I', d, 0x20)[0], struct.unpack_from('>I', d, 0x30)[0]
    return [struct.unpack_from('>6I', d, table_at + 24 * i) for i in range(count)]
if __name__ == '__main__':
    for n in sys.argv[1:]:
        d = M.read(E[n.lower()])
        print('==', n, len(d), d[:8])
        for s in sections(d):
            print('  at %x  %x size %x  %x %x kind %x' % s)
