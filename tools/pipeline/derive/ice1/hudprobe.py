import os, sys
from pathlib import Path
def env(name, what):  # dev-only: the folders come from the environment, never from this file
    return os.environ.get(name) or sys.exit(f'set {name} to {what}')
REPO_ROOT = str(Path(__file__).resolve().parents[4])
import sys, json, re, collections
from pathlib import Path
REPO = REPO_ROOT
S1 = env('SKATE1_DISC', 'the extracted Skate 1 disc folder')
INST = env('SKATE_ASSETS', "an installation's assets folder")
sys.path.insert(0, env('SKATE_TABLES', 'the skate2port tables folder'))
argv, sys.argv = sys.argv, ['old_hud1.py', REPO, S1, INST]
import old_hud1 as H
sys.argv = argv
H.WORK = Path(__file__).parent/'hud/cache'
H.OUT = Path(__file__).parent/'hud/out'
P = 'data/fe/source/screens/'
M = {m.split('/')[-1]: P + m for m in [
    'hud/hud_online_eventlog', 'hud/hud_online_freeskate_overlay', 'hud/hud_online_players', 'hud/hud_online_timer',
    'hudComponents/onlinehud', 'hudComponents/timerhud', 'hudComponents/leaderboard', 'hudComponents/racehud',
    'hudComponents/skatehud', 'hudComponents/spectator_mode', 'hud/hud_besttrick_challenge_overlay',
    'hud/hud_deathrace/hud_deathrace_challenge_overlay', 'hud/hud_jam_challenge_overlay',
    'hud/hud_skate/hud_skate_challenge_overlay', 'hud/hud_spot_challenge_overlay', 'hud/hud_spotrace_challenge_overlay',
    'main/chyron', 'hudComponents/endchallengeoptions', 'challenge/challenge_signup', 'challenge/ownthespot', 'hudComponents/objectivehud']}
c = H.cache(tuple(M.values()) + H.EXTRA)
s3 = H.skate3_hud()
for stem, name in M.items():
    try:
        b = c.load_bundle(name)
        chars = list(b['characters'].values())
        kinds = collections.Counter(ch['type_name'] for ch in chars)
        fonts = sorted({ch['font']['name'] for ch in chars if ch['type_name'] == 'font'})
        texts = [ch.get('initial_text') or ch.get('value') or ch.get('text') for ch in chars if ch['type_name'] == 'text']
        root = next((ch for ch in chars if ch['id'] == 0), {})
        print(f'{stem:34s} {dict(kinds)} frames={len(root.get("frames", []))} fonts={fonts}')
        print('     texts', [t for t in texts if t][:12])
        try:
            H.movie(c, name, s3); print('     movie OK')
        except Exception as e:
            print('     movie FAIL', type(e).__name__, e)
    except Exception as e:
        print(stem, 'LOAD FAIL', type(e).__name__, e)
