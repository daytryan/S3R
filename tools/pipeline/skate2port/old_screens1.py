"""Skate 1's tutorial-window screens (screens.big) for the engine's apt_screen runtime (research.md 2.0, D18 / D21):
the sign-up poster, the tutorial overlay and the movies it loads, lesson and mission end, Skate 101, the incoming call,
the action overlay, the objective HUD, the skate / follow / photo / film overlays, the loaders and CaCDetails.

Writes assets/private/hud-skate1/tutorial/ (old_screens.py's layout, for Skate 2):
  runtime/<stem>.json   old_screens.movie()'s manifest (imports merged, prepare_core_menu's scheme), Skate 1's gfn fonts
                        and its english.big strings
  assets/...            the RGBA payloads those reference
  natives.json          per movie, the identifiers its ActionScript pushes (what apt_screen must answer)

usage (numpy, PIL): py old_screens1.py REPO SKATE1_ROOT ASSETS
"""
import json, os, re, struct, sys
from pathlib import Path

import old_hud1  # same argv: REPO SKATE1_ROOT ASSETS
from old_hud1 import project, AssetCache
import old_screens  # Skate 2's merge (movie, natives); importing it only sets paths
from prepare_core_menu import CONSTRUCT_EVENT

S = 'data/fe/source/screens/'
MOVIES = {
    'challenge_signup': S + 'challenge/challenge_signup',
    'hud_tutorial_challenge_overlay': S + 'hud/hud_tutorial/hud_tutorial_challenge_overlay',
    # The overlay's loadMovie targets: the controller treatment and the gesture loops.
    'tutorial': S + 'tutorials/tutorial',
    'trick_gestures': 'data/fe/source/images/trick_gestures/trick_gestures',
    'tutorial_end': S + 'tutorials/tutorial_end',
    'endchallengeoptions': S + 'hudComponents/endchallengeoptions',  # mission end (FE 23 EndChallengeOptions)
    'skate101': S + 'tutorials/skate101',
    'incomingcalls': S + 'sidekick/calls/incomingcalls',
    'hud_action_overlay': S + 'hud/hud_action_overlay',
    'actionoverlay': S + 'hudComponents/actionoverlay',
    'inkaction': S + 'hudComponents/inkaction',
    'hudobjective': S + 'hud/hudobjective',
    'objectivehud': S + 'hudComponents/objectivehud',
    'hud_skate_challenge_overlay': S + 'hud/hud_skate/hud_skate_challenge_overlay',
    'hud_followme_challenge_overlay': S + 'hud/hud_followme/hud_followme_challenge_overlay',
    'hud_photo_challenge_overlay': S + 'hud/hud_photo_challenge_overlay',
    'hud_film_challenge_overlay': S + 'hud/hud_film_challenge_overlay',
    'welcomeloader': S + 'loading/welcomeloader',
    'quickloader': S + 'loading/quickloader',
    'tipsloader': S + 'loading/tipsloader',
    # CaCDetails (FE 43), the form between Game_Intro_1 and CaC: name, nickname, stance, style, gesture set.
    'cacinfo': S + 'cac/cacinfo',
    # hudbase's hudink (depth 6, 'right') loads it; InkHUD.Play('objective', clip) plays its mInk: the black splatter
    # behind the objective HUD (hudobjective, depth 7).
    'inkbottomright': S + 'hudComponents/inkbottomright',
    # The NIS state's tutorial screens (HUD overlay ids 55-62, 69, 70; table near 0x8309ef14, enter 0x82352018 ->
    # SetState(1) 0x8235266c): the controller treatment over the lesson and pro demo cutscenes.
    # Stems nis_<file> (the table's nis/tutorial_nis/<file>.swf).
    **{'nis_' + n: S + 'nis/tutorial_nis/' + n for n in (
        'basics', 'timing_tutorial', 'airs_tutorial', 'grind_tutorial', 'advancedflips_tutorial', 'prod_tutorial',
        'jdill_tutorial', 'dway_tutorial', 'mcarroll_tutorial', 'chaslam_tutorial_new')},
    # The pro intro scenes' cards (overnight-1015 C2): overlay ids 10/11 ProBioInfoOverlay_Left/Right -> FE screen 86
    # Pro_Bio_Info, 21-52 / 64-78 SponsorLogo_* -> FE screen 104 SponsorLogo_Setup (enter 0x82352018, exit 0x82352688).
    'pro_bioinfo_overlay': S + 'nis/pro_bioinfo_overlay',
    'sponsorlogo_setup': S + 'nis/sponsorlogo_setup',
    # LoadImage's loadMovie targets (source/images/sponsor_logos/<file>.swf, by the movie's own brand enum 0-44).
    # 7 'sponsorlogo_destrocuto.swf' is not on the disc (it has sponsorlogo_destructo): retail loads nothing for it.
    **{n: 'data/fe/source/images/sponsor_logos/' + n for n in ('sponsorlogo_' + f for f in (
        'adidas', 'alien', 'almost', 'autobahn', 'bakerboard', 'circa', 'dcshoes', 'emerica', 'enjoi', 'es',
        'etnies', 'fallen', 'flipboard', 'girl', 'globe', 'gold', 'hubba', 'independent', 'ipath', 'lakai', 'momentum',
        'mysteryboard', 'nike', 'osiris', 'planbBoard', 'real', 'royal', 'silver', 'spitfire', 'thunder', 'zeroBoard',
        'bakerwheels', 'flipwheels', 'mysterywheels', 'planbWheels', 'zeroWheels', 'venture', 'alienwheels', 'icecream',
        'fillmore', 'blacklabel', 'chocolate', 'krooked', 'supra'))},
}
SHARED = ('data/fe/source/controls/', 'data/fe/source/images/')
OUT = old_hud1.assets/'private/hud-skate1/tutorial'
WORK = old_hud1.WORK_ROOT/'screens1-cache'
old_hud1.WORK, old_hud1.OUT = WORK, OUT  # old_hud1's helpers read and write under its WORK/OUT
old_screens.WORK = WORK  # its movie() reads the raw apt/const there
old_screens.buttons = dict  # Skate 1's button art is button_item's own sprites (merged as an import), not a bank


def parameters(apt, acts, offset):
    """prepare_core_menu._parameters for Skate 1's construct events, which also set numbers: `name` (0xA1) then a
    string (0xA6), or a number (0xB5 byte, 0xB4 f32, 0x5A one; apt_vm's pushes) and setVariable (0x1D). The placed
    menu pickers (skate101, tutorial_end, endchallengeoptions) need them: no mMenuArrowType, gotoAndStop(NaN).
    UNTRACED: numbers go in as their decimal strings (placement init is text, as S2 / S3's pickers ship); typed init
    would need apt_display / apt_movie."""
    count, pointer = struct.unpack('>II', apt[offset:offset + 8])
    result = {}
    for i in range(count):
        flags, _, actions = struct.unpack('>III', apt[pointer + 12 * i:pointer + 12 * i + 12])
        stream = [row for row in acts.stream(actions) if row['opcode'] != 0]
        if flags != CONSTRUCT_EVENT:
            return None
        while stream:
            name, value, *rest = stream + [None]
            if name['opcode'] != 0xA1 or value is None:
                return None
            if value['opcode'] == 0xA6:
                result[name['operand']], stream = value['operand'], stream[2:]
                continue
            number = {0xB5: lambda o: (o & 0xFF) - ((o & 0x80) << 1), 0x5A: lambda o: 1,
                      0xB4: lambda o: struct.unpack('>f', struct.pack('>I', o))[0]}.get(value['opcode'])
            if number is None or not rest or rest[0] is None or rest[0]['opcode'] != 0x1D:
                return None
            result[name['operand']], stream = repr(number(value.get('operand'))), stream[3:]
    return result


old_screens._parameters = parameters  # movie() calls it for every placement's clip events


class Fonts:
    """The cache, with Skate 1's gfn banks (old_hud1.gfn_font) as the fonts old_screens.movie asks it for."""
    def __init__(self, c):
        self.c, self.mappings = c, {}

    def __getattr__(self, name):
        return getattr(self.c, name)

    def font_asset(self, name):
        # A name missing from the exe's font table (old_hud1.GFN; only Times New Roman, controls/menupicker's 'M'
        # placeholder) falls back to the table's index 0, Bit KitBold (0x827535b8, research5.md B3).
        font, self.mappings[name] = old_hud1.gfn_font(old_hud1.GFN.get(name, 'bitkitbold'))
        return font


def cache():
    project.BigArchive = old_hud1.S1Archive
    project.find_big_directory = lambda root: Path(root)/'data'
    project.CORE_ARCHIVES = ('fe/screens.big', 'big/miscboot.big')
    project.extract_project(old_hud1.s1game, WORK, prefixes=(*MOVIES.values(), *SHARED), force=True)
    for apt in (WORK/'metadata').rglob('*.apt.json'):
        key = apt.relative_to(WORK/'metadata').as_posix().removesuffix('.apt.json')
        if (WORK/'assets'/key/'manifest.json').is_file():
            old_hud1.name_textures(key)
    return AssetCache(WORK)


# Ids the game hands the screens at run time, which no ActionScript names: Skate 101's header / description for the
# tutorial window's four 101s and Skate.TriggerSkate101's n = 15 (Get101Header 0x82460b10 / Get101Desc 0x82461468,
# research8 1; REPEATED_TRICK strings in the exe at 0x82134e8b / 0x82135377) and the sign-up options (UNTRACED
# per-challenge table 0x830a3f50; Accept / Decline is the port's fallback).
RUNTIME_IDS = [f'ID_SKATE_101_{name}_{part}' for name in
               ('NEW_CHALLENGE_COMPLETED', 'SEQUENCE_SCORING', 'FILM_MENU', 'TRAIN_STATION', 'REPEATED_TRICK')
               for part in ('TITLE', 'DESC')]
# Mission.GetSignUpOption / GetEndMissionOptionText rows (RETAIL tables 0x830a3f50 / 0x830a3f30; skate1_screens.rs).
RUNTIME_IDS += ['ID_COMMON_ACCEPT_CAPS', 'ID_COMMON_DECLINE', 'ID_COMMON_SKIP_LESSON', 'ID_COMMON_WATCH_IT_CAPS',
                'ID_PAUSEMENU_EXITTUTORIAL', 'ID_COMMON_CONTINUE_CAPS', 'ID_COMMON_RETRY_CAPS',
                'ID_MEDIA_VIEW_PHOTO_CAPS', 'ID_REPLAY_EDITREPLAY', 'ID_COMMON_SAVE_REPLAY_CAPS',
                'ID_ONLINE_SCORES_CAPS', 'ID_DEMO_PHOTOCHALLENGE_VIEWPHOTO', 'ID_COMMON_QUIT_CAPS']
# Mission.GetChallengeTypeName: the type table's title ids (RETAIL 0x830a2c28 +16; skate1_screens.rs TYPES).
RUNTIME_IDS += [f'ID_MISSION_TEMPLATE_{t}_TITLE' for t in (
    'TUTORIAL', 'POINT_CONTEST', 'DEMO', 'RISK', 'SKATE', 'PHOTO', 'FILM', 'BEST_TRICK', 'OWN_THE_SPOT', 'SLALOM',
    'SLIDE', 'LAP', 'GATE', 'FAVOUR', 'JAM', 'CRASH', 'SECURITY', 'POINT_2_POINT', 'DEATH_RACE', 'SPOT_RACE',
    'LAST_MAN_STANDING', 'ONLINE_FREESKATE', 'FOLLOW', 'TRAIN_STATION', 'SKATE_SHOP', 'MEGARAMP', 'STREETSKATING',
    'PRO', 'TELEPORTER', 'GOTO')]
# NIS.GetProInfo's labels (RETAIL 0x8245eaa8 over the pro table built at 0x82327828; skate1_pros.rs pro_info).
RUNTIME_IDS += [f'ID_CHEAT_PROSKATER{n:02d}' for n in range(1, 21)] + [f'ID_CHEAT_PRODETAIL_LOCATION{n}' for n in range(1, 14)]
RUNTIME_IDS += ['ID_CHEAT_PRODETAIL_GOOFY', 'ID_CHEAT_PRODETAIL_REGULAR']
# Mission.GetDistanceHUDText's formats, looked up by hash alone (0x824838f0): "{0:d} m", "{0:d} ft".
UNIT_FORMATS = ('92099686', '413c56da')


# Audio.<Name>() (overnight-1015 B4; item10-flashes.md D2's chain, RETAIL unless marked):
# - 0x82441860 registers each Audio native as (name string, closure);
# - a closure is `li r3,<message>; bl 0x82467740`;
# - 0x82467740 plays the first skater.vlt FEMessageToSoundTrigger row (Hash_DCDC002EEFC2FF7D) whose message matches,
#   unless its sound is 3006 (no further search: the loop breaks at 0x824677d0, 0x8246781c drops 3006; review 10-15);
# - cMsgPlayMenuSound 0x822ac478 sends 2000-2999 to the PlayHudSample patch and 1000-1999 to PlayPauseMenuSample, with
#   index = sound mod 1000.
# INFERRED (as D2): the index picks the sample of HUD.abk / PAUSE_MENU.abk directly.
# UNTRACED: the per-index volume list (this+1428) -> unity gain; other ranges (< 1000, 3000+) export nothing.
# Each native the movies name gets sfx/<Name>.wav (apt_screen plays Skate 1's Audio.Play* calls by name).
AUDIO_NATIVES, PLAY_MESSAGE = (0x82441860, 0x82444e50), 0x82467740
SAMPLE_BANKS = {1: 'PAUSE_MENU.abk', 2: 'HUD.abk'}
VGMSTREAM = Path(os.environ.get('SKATE_VGMSTREAM') or old_hud1.here/'tools/vgmstream/vgmstream-cli.exe')


def audio_natives(exe):
    """{native name: FE message} from the registration code (lis / addi pairs: r3 = closure, r4 = its name)."""
    word = lambda a: struct.unpack_from('>I', exe, a - 0x82000000)[0]
    regs, closure, out = {}, None, {}
    for a in range(*AUDIO_NATIVES, 4):
        i = word(a)
        op, d, s, imm = i >> 26, i >> 21 & 31, i >> 16 & 31, (i & 0xFFFF) - (i & 0x8000) * 2
        if op == 15 and s == 0:  # lis
            regs[d] = imm << 16 & 0xFFFFFFFF
        elif op == 14 and s and s in regs:  # addi
            value = regs[d] = (regs[s] + imm) & 0xFFFFFFFF
            if d == 3:
                closure = value
            elif d == 4 and closure:
                name = exe[value - 0x82000000:exe.index(b'\0', value - 0x82000000)].decode('latin-1')
                code = [word(closure + 4 * k) for k in range(8)]
                for k, (li, bl) in enumerate(zip(code, code[1:])):
                    target = (closure + 4 * k + 4 + ((bl & 0x3FFFFFC) ^ 0x2000000) - 0x2000000) & 0xFFFFFFFF
                    if li >> 16 == 0x3860 and bl >> 26 == 18 and target == PLAY_MESSAGE:
                        out[name] = li & 0xFFFF
                closure = None
    return out


def message_sounds():
    """skater.vlt FEMessageToSoundTrigger: {message: sound}, first row wins (0x82467740 walks them in order)."""
    from tools.asset_pipeline.vlt import convert
    names = (old_hud1.repo/'tools/asset_pipeline/names.txt').read_text(encoding='utf-8').splitlines()
    stem = old_hud1.s1game/'data/db/skater'
    row = next(r for r in convert(stem, stem, names)['collections'] if r['key'] == 'Hash_DCDC002EEFC2FF7D')
    out = {}
    for item in row['fields']['Hash_B453290E03476D7B']['array']['items']:
        out.setdefault(int(item[:8], 16), int(item[8:16], 16))
    return out


def eaac_wav(header, body, out):
    """old_challenges.eaac_wav: vgmstream reads the header and the block chain as an .snr/.sns pair."""
    # ponytail: a copy of its 8 lines; importing old_challenges runs its main body.
    import subprocess, tempfile, wave
    with tempfile.TemporaryDirectory(prefix='skate1-eaac-') as work:
        snr = Path(work)/'a.snr'
        snr.write_bytes(header)
        snr.with_suffix('.sns').write_bytes(body)
        subprocess.run([str(VGMSTREAM), '-o', str(out), str(snr)], check=True, capture_output=True)
    with wave.open(str(out)) as w:
        return w.getnframes() / w.getframerate()


def sample(abk, index):
    """An ABKC bank's sample `index` (S10A table at +0x18) as (header, body); None for an empty slot."""
    bank, size = struct.unpack_from('>I', abk, 0x18)[0], struct.unpack_from('>I', abk, 0x24)[0]
    starts = struct.unpack_from(f'>{struct.unpack_from(">I", abk, bank + 8)[0]}I', abk, bank + 12)
    if index >= len(starts) or starts[index] == 0xFFFFFFFF:
        return None
    end = min([s for s in starts if starts[index] < s != 0xFFFFFFFF] or [size])
    data = abk[bank + starts[index]:bank + end]
    h1, h2 = struct.unpack_from('>II', data)
    assert h2 >> 29 == 0, f'sample {index}: stream type / loop bits {h2 >> 29}, want a plain RAM sound'
    # A RAM sound relabelled streamed (1): vgmstream reads it as an .snr/.sns pair (old_online1's spot_found).
    return struct.pack('>II', h1, h2 | 1 << 30), data[8:]


def audio(used):
    """sfx/<Name>.wav for every Audio native in `used` that plays a 1000 / 2000 sound; returns {name: (bank, index)}."""
    from tools.build_skate2_image import build
    natives, sounds = audio_natives(build(old_hud1.s1game/'default.xex')), message_sounds()
    assert natives.get('PlayObjectiveComplete') == 222 and sounds[222] == 2017, 'the trace changed'
    fe = old_hud1.Big(old_hud1.s1game/'data/audio/fe.big')
    banks = {k: fe.read(next(e for e in fe.entries if Path(e.path).name == v)) for k, v in SAMPLE_BANKS.items()}
    (OUT/'sfx').mkdir(parents=True, exist_ok=True)
    out = {}
    for name in sorted(used & natives.keys()):
        sound = sounds.get(natives[name], 0)
        if sound // 1000 not in banks or sound == 3006:
            continue
        pair = sample(banks[sound // 1000], sound % 1000)
        if pair:
            eaac_wav(*pair, OUT/'sfx'/f'{name}.wav')
            out[name] = (SAMPLE_BANKS[sound // 1000], sound % 1000)
    return out


def language(manifests):
    """Skate 1's english.big text of every ID_ label the screens name (its tables are keyed by hash alone), plus the
    ids the game hands them at run time."""
    labels = set(RUNTIME_IDS)
    for m in manifests.values():
        labels |= set(re.findall(r'ID_[A-Z0-9_]+', json.dumps([m['actions'], m['characters']])))
    units = old_hud1.english([int(k, 16) for k in UNIT_FORMATS])
    return old_hud1.english(sorted(labels)) | {k: units[int(k, 16)] for k in UNIT_FORMATS}


if __name__ == '__main__':
    c = Fonts(cache())
    manifests = {stem: old_screens.movie(c, name, c.mappings, {}) for stem, name in MOVIES.items()}
    strings = language(manifests)
    for m in manifests.values():
        m['source']['game'], m['language'] = 'skate1', strings
        m.pop('buttons')
    old_hud1.install(manifests)
    natives = {s: old_screens.natives(m) for s, m in manifests.items()}
    (OUT/'natives.json').write_text(json.dumps(natives, indent=1), encoding='utf-8')
    print('sfx', audio({n for names in natives.values() for n in names}))
    old_hud1.check(list(manifests))
    print(len(manifests), 'screens ->', OUT)
