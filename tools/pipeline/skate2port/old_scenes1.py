"""Skate 1's tutorial cutscenes (to the end of Filming 101): the four lesson demos, Shingo's ROP 2001 offer, his decline,
the he_u1_skate01 fly-through, ROP 2002 and the he_u2_photo01 win, in challenge::cutscene's scene format.

Writes ASSETS/private/challenges-skate1/nis/ (rmtrees only that folder; old_challenges.py, which rmtrees all of
challenges-skate1/ and writes the tutorial.json this one checks its scene list against, runs first):
  tutorial.json             {'scenes': {name: kind, origin, duration, camera, audio, subtitles, tracks}}, the name
                            Cutscene::start(<root>, scene) reads
  scene/camera/<name>.json  old_tutorial.scene_camera over the scene's Skate 1 ICE takes (s1ice); the lesson demos are
                            edits (CONTAINS_SUBTAKE on track 32) and carry `shots`
  nis/<stream>.wav          each scene's AUDIO_POST stream (nis.big 99_<id>_*, old_tutorial.post_audio; research.md 4.4)
  anim1/<clip>.bin          each track's clip on its bank's pose (s1abin.export_clip; research.md 4.3a); the track's
                            `clip1` is its meta

usage (numpy): py -3 old_scenes1.py REPO SKATE1_ROOT ASSETS
"""
import json, os, shutil, struct, sys
from pathlib import Path

repo, s1game, assets = map(Path, sys.argv[1:4])
here = Path(__file__).resolve().parent
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
sys.path[:0] = [str(repo), str(here)]
from s2big import Big
import s1abin, s1ice

MAIN = '# ' + '-' * 64 + ' main'  # the marker, built: this file is split on it too
OUT = assets/'private/challenges-skate1/nis'
# The scenes the window plays (spec "Retail sequence in scope", research.md 1.2): the lessons' NISFlyThroughScene;
# he_u1_skate01's NISIntroOfferScene (ROP 2001), NISIntroDeclineScene and NISIntroAcceptScene (its fly-through); ROP
# row 2 (old_challenges.ROP_SCENES); he_u2_photo01's NISOutroScene. Main checks the list against old_challenges' vault.
SCENES = ['Basic_Basics_Tutorial', 'Basic_Timing_Tutorial', 'Basic_Air_Tutorial', 'Basic_Grinds_Tutorial',
          'NIS_ROP_2001_shingo_S01_0', 'NIS_IP_Shingo_SkateDecline_S01_0', 'he_u1_skate01', 'NIS_ROP_2002_shingo_S01_0',
          'he_u2_photo01',
          # P6: the pro tutorials' NISIntroOfferScene and NISFlyThroughScene (research6 2).
          'NIS_PRod_ProIntro_S01_0', 'NIS_PRod_ProTutorial_S02_0', 'NIS_JasonDill_ProIntro_S01_0',
          'NIS_JasonDill_ProTutorial_S02_0',  # its bank is codec word 0 (s1abin.load, research 4.3a)
          'NIS_MCarrol_ProIntro_S01_0', 'NIS_MCarrol_ProTutorial_S01_0',
          'NIS_CHaslam_ProIntro2_S01_0', 'NIS_CHaslam_ProTutorial_S01_0',
          # User 10-15: Community Spirit's per-round NISFlyThroughScenes (setup_round plays round N's before its
          # countdown), then Danny Way's NISIntroOfferScene and NISFlyThroughScene.
          'he_u4_besttrick01_R1', 'he_u4_besttrick01_R2', 'he_u4_besttrick01_R3',
          'NIS_DWay_ProIntro_S01_0', 'NIS_DWay_ProTutorial_S01_0']


def lib(script):
    """A sibling script's library half (its main body rmtrees its own output)."""
    ns = {'__name__': script[:-3] + '_lib', '__file__': str(here/script)}
    argv, sys.argv = sys.argv, [script, str(repo), str(s1game), str(assets)]
    try:
        exec(compile((here/script).read_text(encoding='utf-8').split(MAIN)[0], str(here/script), 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


C = lib('old_challenges.py')  # label_key, subtitles1, english, ROP_SCENES
T = lib('old_tutorial.py')  # the ICE evaluator: scene_camera (edit_shots), post_ids, post_audio


def scene_records(scene_bin):
    """scene.big data/scene/scene.bin: 'scen', count @8, 0x68-byte records from 0x10 (hash, kind @4, origin and
    duration in the 7 floats @8, track count / first @0x24, name @0x28), then 148-byte tracks (Skate 2's: 180):
    character, package, anim (the bank's path under data/), clip (research.md 4.3, scratch/scenes1.py)."""
    assert scene_bin[:4] == b'scen', scene_bin[:4]
    n = struct.unpack_from('>I', scene_bin, 8)[0]
    tracks_at = 0x10 + n * 0x68
    text = lambda at, a, b: scene_bin[at + a:at + b].split(b'\0')[0].decode('latin-1')
    out = {}
    for i in range(n):
        o = 0x10 + i * 0x68
        f = struct.unpack_from('>7f', scene_bin, o + 8)
        count, first = struct.unpack_from('>HH', scene_bin, o + 0x24)
        out[text(o, 0x28, 0x68)] = {
            'hash': struct.unpack_from('>I', scene_bin, o)[0], 'kind': scene_bin[o + 4], 'origin': list(f[0:3]),
            'duration': f[6],
            'tracks': [{'character': text(r, 0, 32), 'package': text(r, 32, 48), 'anim': text(r, 48, 112),
                        'clip': text(r, 112, 148)} for r in (tracks_at + 148 * t for t in range(first, first + count))]}
    return out


def tutorial_scenes(names):
    """names -> {name: scene}, writing their cameras, clips and post streams under OUT."""
    big = Big(s1game/'data/big/scene.big')
    entries = {e.path.lower(): e for e in big.entries}
    read = lambda path: big.read(entries[path.lower()])
    s1ice.use(T, sorted(json.loads((TABLES/'ice_params1.json').read_text(encoding='utf-8')), key=lambda p: p['pos']))
    cameras = s1ice.ice_cameras(T, read('data/scene/ice.bin'))
    s1ice.use_shakes(T, cameras, C['label_key'])
    records = scene_records(read('data/scene/scene.bin'))
    boot = Big(s1game/'data/big/miscboot.big')
    subs = C['subtitles1'](boot.read(next(e for e in boot.entries if e.path.endswith('/subtitle.bin'))), names,
                           C['english']())
    (OUT/'scene/camera').mkdir(parents=True, exist_ok=True)
    scenes = {}
    for name in names:
        s = records[name]
        takes = cameras[s['hash']]
        camera = T['scene_camera'](takes)
        # Several takes play only as an edit's shots (CONTAINS_SUBTAKE keys); never take 0 by default.
        assert len(takes) == 1 or 'shots' in camera, f'{name}: {len(takes)} takes and no edit'
        scene = {'kind': s['kind'], 'origin': s['origin'], 'duration': s['duration'],
                 'camera': f'scene/camera/{name}.json', 'posts': T['post_ids'](takes), 'subtitles': subs.get(name, []),
                 # The bank's FastString block names are upper case (s1abin.fs); the scene names the clip in mixed case.
                 'tracks': [{**t, 'clip1': s1abin.export_clip(read('data/' + t['anim']), t['clip'].upper(), OUT/'anim1')}
                            for t in s['tracks']]}
        # OVERLAY is Skate 1's HUD overlay enum (old_online1: its ids differ from Skate 2's), kept out of the camera.
        if 'overlay' in camera:
            scene['s1_overlay'] = camera.pop('overlay')
        (OUT/f'scene/camera/{name}.json').write_text(json.dumps(camera, separators=(',', ':')), encoding='utf-8')
        scenes[name] = scene
    T['post_audio'](scenes, s1game/'data/audio/english/nis.big', OUT)
    return scenes


# ---------------------------------------------------------------- main
tutorial = json.loads((OUT.parent/'tutorial.json').read_text(encoding='utf-8'))  # old_challenges.py's
NIS = ('NISFlyThroughScene', 'NISIntroOfferScene', 'NISIntroDeclineScene', 'NISIntroAcceptScene', 'NISOutroScene')
rows = [*tutorial['lessons'].values(), *tutorial['missions'].values(), *tutorial['pro_tutorials'].values()]
wanted = ({c['self'][k] for c in rows for k in NIS if c['self'].get(k)} | set(C['ROP_SCENES'].values())
          | {s for c in rows for s in c['self'].get('NISFlyThroughScenes') or [] if s})
# Community Spirit's NISFlyThroughScene (its own name) is no scene.bin record, and its Lua never plays it.
wanted -= {'he_u4_besttrick01'}
assert set(SCENES) == wanted, (sorted(set(SCENES) - wanted), sorted(wanted - set(SCENES)))
if OUT.exists():
    shutil.rmtree(OUT)
scenes = tutorial_scenes(SCENES)
(OUT/'tutorial.json').write_text(json.dumps({'scenes': scenes}, indent=1), encoding='utf-8')
print(f'{len(scenes)} scenes, {sum(len(s["tracks"]) for s in scenes.values())} clips, '
      f'{sum(len(s["audio"]) for s in scenes.values())} streams, '
      f'{sum(len(s["subtitles"]) for s in scenes.values())} subtitle lines')
