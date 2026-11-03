"""Scratch: dump every Skate 1 challenge row (class 8876...) merged, to find the online ones."""
import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import json, sys, re
from pathlib import Path
HERE = Path(env('SKATE_TABLES', 'the skate2port tables folder'))
REPO = Path(REPO_ROOT)
S1 = Path(env('SKATE1_DISC', 'the extracted Skate 1 disc folder'))
OUT = Path(__file__).parent
sys.path[:0] = [str(REPO), str(HERE)]
src = (HERE/'old_challenges.py').read_text(encoding='utf-8').split('# ---------------------------------------------------------------- main')[0]
ns = {'__name__': 'lib', '__file__': str(HERE/'old_challenges.py')}
argv, sys.argv = sys.argv, ['x', str(REPO), str(S1), str(OUT/'assets')]
exec(compile(src, 'old_challenges', 'exec'), ns)
sys.argv = argv
v = ns['Vault']()
import pickle
classes = {}
for rows in v.by_key.values():
    for r in rows:
        classes[r['class']] = classes.get(r['class'], 0) + 1
print(sorted(classes.items(), key=lambda x: -x[1])[:40])
out = {}
for k in v.lessons:
    m = v.merged(k)
    me = {}
    for f, field in m.items():
        f = ns['ALIAS'].get(f, f)
        try:
            me[f] = v.value(field) if f != 'Steps' else '<steps>'
        except Exception as e:
            me[f] = f'ERR {e}'
    me['_parent'] = v.lessons[k]['parent']
    out[k] = me
(OUT/'lessons.json').write_text(json.dumps(out, indent=1, default=str), encoding='utf-8')
print(len(out))

# all rows of every class, key list per class
allc = {}
for rows in v.by_key.values():
    for r in rows:
        allc.setdefault(r['class'], []).append(r['key'])
(OUT/'classes.json').write_text(json.dumps(allc, indent=0), encoding='utf-8')
# strings
from s2big import Big
from s2lang import table
text = {}
eng = Big(S1/'data/big/english.big')
for e in eng.entries:
    if e.path.lower().endswith('.bin') and 'histogram' not in e.path.lower():
        text.update(table(eng.read(e)))
(OUT/'text.json').write_text(json.dumps({str(k): v_ for k, v_ in text.items()}, ensure_ascii=False), encoding='utf-8')
print([e.path for e in eng.entries])

# templates: the online rows' Steps owners, decompiled
import struct
AO, DO, STEPS = 'Hash_F9A9A1DDED84A643', 'Hash_FD40E4A31027C984', 'Hash_3CB7BA2AFC9BDE42'
lessons = v.lessons
parents = {r['parent'] for r in lessons.values()}
def tpl(k):
    while k and STEPS not in lessons[k]['fields']:
        k = lessons[k]['parent']
    return k
on = [k for k in lessons if out[k].get(AO) and not out[k].get(DO) and k not in parents]
temps = {}
for k in on:
    temps.setdefault(tpl(k), []).append(k)
(OUT/'lua').mkdir(exist_ok=True)
for t, rows in temps.items():
    lua = [f'-- {t}: rows {sorted(rows)}']
    for step in v.rows(lessons[t]['fields'][STEPS]):
        sname = step['fields']['Name']['data']
        for h in v.rows(step['fields'].get('Hash_810394ECEFE7E48B', {'data': ''}) or {'data': ''}):
            size, offset = struct.unpack('>II', bytes.fromhex(h['fields']['Hash_C3CD4B4183274819']['data']))
            fn, src_ = ns['decompile'](v.bin[offset:offset + size])
            lua.append(f'-- [{sname}] {h["fields"]["Name"]["data"]}\n{src_.strip()}\n')
    (OUT/'lua'/f'{t}.lua').write_text('\n'.join(lua), encoding='utf-8')
    print(t, len(rows), len(lua) - 1, 'handlers')
