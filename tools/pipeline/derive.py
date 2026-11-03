"""Tables the Skate 1/2 ports read from the game executables, rebuilt from the player's own discs at build time.
Never shipped: they exist only in the player's work/tables folder. Which script makes which table: ORIGINS.md Notes."""
import json, os, shutil, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
TASK = [sys.executable, '--task'] if getattr(sys, 'frozen', False) else [sys.executable]  # as runner's ctx.script
GENERATORS = [  # (script, disc image it reads, file it writes in its working folder or None for stdout, table)
    ('derive/ice2/params2.py', 's2', 'ice_params.json', 'ice_params.json'),
    ('derive/ice1/params1.py', 's1', 'ice_params1.json', 'ice_params1.json'),
    ('derive/ice1/enums.py', 's1', None, 'online1_enums.json'),
    ('derive/names-5372/names.py', 's2', 'extra_names.txt', 's2_names.txt'),
]
DB_TABLES = ('ChallengeCore.bin', 'progressioncore.bin')  # names.py's input, from Skate 2's data/db/db.big


def derive_tables(s2_disc, s1_disc, out_dir, env=None):
    """Write the four tables into out_dir. env (e.g. SKATE_JAVA/SKATE_UNLUAC from fetch_tools) goes to the scripts."""
    from tools.build_skate2_image import build
    from tools.pipeline.skate2port.s2big import Big
    env = {**os.environ, **(env or {})}
    if not Path(env.get('SKATE_UNLUAC', '')).is_file() or not shutil.which(env.get('SKATE_JAVA', 'java')):
        raise RuntimeError('the Skate 2 names table needs Java and unluac (SKATE_JAVA / SKATE_UNLUAC)')
    out_dir = Path(out_dir)
    scratch = out_dir/'scratch'  # memory images and db dumps are game content: always removed
    shutil.rmtree(scratch, ignore_errors=True)
    scratch.mkdir(parents=True)
    try:
        for which, disc in (('s2', s2_disc), ('s1', s1_disc)):
            (scratch/f'{which}.img').write_bytes(build(Path(disc)/'default.xex'))
        db = Big(Path(s2_disc)/'data/db/db.big')
        for entry in db.entries:
            if Path(entry.path).name in DB_TABLES:
                (scratch/Path(entry.path).name).write_bytes(db.read(entry))
        made = []
        for script, which, wrote, table in GENERATORS:
            run = subprocess.run([*TASK, str(HERE/script)], cwd=scratch, capture_output=True, encoding='utf-8',
                                 errors='replace', creationflags=0x08000000 if os.name == 'nt' else 0,
                                 env={**env, 'SKATE_XEX_IMAGE': str(scratch/f'{which}.img'), 'SKATE_S2DB': str(scratch)})
            if run.returncode:
                raise RuntimeError(f'{script} exited with code {run.returncode}: {run.stderr.strip()[-800:]}')
            if not wrote:
                wrote = table
                (scratch/table).write_text(run.stdout, encoding='utf-8')
            text = (scratch/wrote).read_text(encoding='utf-8')
            if not (json.loads(text) if table.endswith('.json') else text.strip()):
                raise RuntimeError(f'{script} wrote an empty {table}')
            made.append((wrote, table))
        for wrote, table in made:  # publish only a complete set
            (scratch/wrote).replace(out_dir/table)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
