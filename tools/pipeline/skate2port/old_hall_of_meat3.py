"""Skate 3's own Hall of Meat score panel (hud2/homscoring) for the engine's free-skate Hall Of Meat Mode (hall_of_meat.rs).

Writes OUT (a staging folder; --install copies it into ASSETS/private/online-ui/, the folder hall_of_meat.rs opens it from
on apt_screen's ONLINE layer; its fonts are the online-ui ones already installed there):
  runtime/homscoring.json   the movie as apt_screen runs it (old_screens.movie's manifest)
  assets/...                its RGBA payloads (shapes; the futura font pages it shares with online-ui)

Only new files are installed: a payload already there must be byte-identical (the receipted online-ui fonts), else
the script stops. New files need no installation.json receipt.

Evidence: HOM_FREESKATE_REPORT.md.
usage (numpy, PIL): py -3 old_hall_of_meat3.py REPO SKATE3_ROOT ASSETS OUT [--install]
  SKATE3_ROOT  the Skate 3 disc folder (REPO/Skate 3 (USA, Europe) (En,Fr,De,Es,It,Nl))
  ASSETS       the installation's assets folder (its private/stock/skater-collections.json maps the fonts)
"""
import filecmp, json, os, shutil, sys
from pathlib import Path

repo, s3game, assets, OUT = map(Path, sys.argv[1:5])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
# old_hud / old_screens parse argv as REPO SKATE2_ROOT ASSETS; their OUT/WORK are redirected below before any use.
sys.argv[1:4] = [str(repo), str(s3game), str(OUT)]
sys.path[:0] = [str(repo), str(here)]
import old_hud, old_screens

NAME = 'data/fe/source/screens/hud2/homscoring'
WORK = WORK_ROOT/'hom3-cache'
old_hud.WORK = old_screens.WORK = WORK
old_hud.OUT = old_screens.OUT = OUT
old_screens.buttons = lambda: {}  # homscoring has no button glyphs (and Skate 3's disc no Skate 2 button cache)

if __name__ == '__main__':
    # Skate 3's own BIG format: the vendored extractor as it is (no S2Archive patch).
    old_hud.project.extract_project(s3game, WORK, prefixes=(NAME, *old_screens.SHARED), force=True)
    c = old_hud.AssetCache(WORK)
    mappings = old_hud.font_mapping(assets/'private/stock/skater-collections.json', c)
    language = {r['label'].strip(): r['value']
                for r in json.loads((WORK/'metadata/languages/english_global.json').read_text(encoding='utf-8'))['entries']}
    m = old_screens.movie(c, NAME, mappings, language)
    # Same as Skate 2's (old_hall_of_meat.py): the typed {0:speed} / {0:distance} formatters are the exe's; the engine
    # hands HOM_GetBonusData's text already formatted, so the strings pass it through.
    m['language'].update(ID_HOM_SPEED_VALUE='%s', ID_COMMON_METRIC_METERS='%s')
    old_hud.install({'homscoring': m})
    (OUT/'natives.json').write_text(json.dumps({'homscoring': old_screens.natives(m)}, indent=1), encoding='utf-8')
    if '--install' in sys.argv:
        target = assets/'private/online-ui'
        files = [p for p in OUT.rglob('*') if p.is_file() and p.name != 'natives.json']
        for p in files:
            dest = target/p.relative_to(OUT)
            if dest.exists():
                assert filecmp.cmp(p, dest, shallow=False), f'{dest} exists and differs (a receipted file?)'
                continue
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(p, dest)
            print('installed', dest.relative_to(assets))
