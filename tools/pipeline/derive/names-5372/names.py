import json,os,re,struct,subprocess,sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[4]))  # the repo (derive.py runs this from its scratch folder)
from tools.unluac_batch import run,text
DB=Path(os.environ['SKATE_S2DB'])  # derive.py dumps the two tables from Skate 2's data/db/db.big here
ids=set(); decompiled=0; chunks=[]
for stem in ('ChallengeCore','progressioncore'):
    B=(DB/f'{stem}.bin').read_bytes()
    # all source-text strings
    for m in re.finditer(rb'[\x09\x0a\x0d\x20-\x7e]{6,}',B): ids.update(re.findall(r'[A-Za-z_][A-Za-z0-9_]{2,}',m.group().decode()))
    # compiled chunks, decompiled below in one unluac batch (one may fail on its own; none decompiling means java/unluac is broken)
    chunks+=[B[m.start():m.start()+200000] for m in re.finditer(rb'\x1bLuaQ',B)]
results=run(chunks)
for c in chunks:
    stdout,code=results[c]; stdout=text(stdout,'latin-1')
    decompiled+=code==0 and bool(stdout.strip())
    ids.update(re.findall(r'[A-Za-z_][A-Za-z0-9_]{2,}',stdout))
if not decompiled: raise SystemExit('unluac decompiled none of the Lua chunks: check SKATE_JAVA / SKATE_UNLUAC')
img=Path(os.environ['SKATE_XEX_IMAGE']).read_bytes()
ids.update(m.group().decode() for m in re.finditer(rb'[A-Za-z_][A-Za-z0-9_:]{2,80}(?=\x00)',img))
Path('extra_names.txt').write_text('\n'.join(sorted(ids)))
print(len(ids))
