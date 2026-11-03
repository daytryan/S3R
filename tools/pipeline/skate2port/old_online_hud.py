"""The online HUD of the Skate 1 and Skate 2 maps (online_hud.rs), on top of what old_online.py / old_online1.py
installed.

--results   Skate 2's results screens: hudComponents/roundresults with its results_lb list (the stock roundresults.json
            lacks the list's dynamic_selectable_list and rank badges) -> hud-skate2/online/runtime/roundresults_lb.json,
            and S.K.A.T.E.'s / 1-Up's challenge/throwdownresults -> throwdownresults.json, with their payloads.
            Challenge type names the engine's lobby filters use but Skate 2's table lacks (BESTTRICK, JAM) come from
            leaderboardhud.json's language.
--dpo       hud2/dpo merged with its Image_Renderer_Component (the stock dpo.json lacks it), plus screens.big's
            images/dpoicons as the manifest's `images` (apt_screen's ImageRenderer_Load serves them by file name)
            -> hud-skate2/online/runtime/dpo.json.
--intro-overlays  the screens the online intros' OVERLAY track opens (skate2_contests.rs): challenge/spotfound (2-5,
            SpotFound_<corner>), hudComponents/phototreatment (54, Take_Photo) and nis/sanvantv (84, SanVanTV), by
            default.xex's screen table (0x82d6de98) -> hud-skate2/online/runtime/<name>.json.
--language  Skate 1's event log (hud-skate1/online/runtime/hud_online_eventlog.json) gets the strings the engine logs
            (online_roster / online_moves / online_boundary / online_menu) and online.json's (location changes): Skate
            1's own text (english.big) first, then Skate 2's (hud-skate2/online's manifest), then Skate 3's (online-ui).
            Skate 1's lost-connection and kicked lines send you "to the Sidekick", a screen the engine doesn't have, so
            those two take Skate 3's text. A first run keeps the manifest as <name>.pre-onlinehud.

usage (numpy, PIL): py -3 old_online_hud.py REPO SKATE2_ROOT SKATE1_ROOT INST_ASSETS [--results] [--dpo] [--language]
                    [--intro-overlays]
"""
import json, os, re, shutil, sys
from pathlib import Path

repo, s2game, s1game, assets = map(Path, sys.argv[1:5])
flags = set(sys.argv[5:])
here = Path(__file__).resolve().parent
WORK_ROOT = Path(os.environ.get('SKATE_WORK') or here/'work')        # ponytail: env set by the pipeline runner
TABLES = Path(os.environ.get('SKATE_TABLES') or here)                 # tables re-derived from the player's discs
private = assets/'private'
S2_ONLINE = private/'hud-skate2/online'
S1_LOG = private/'hud-skate1/online/runtime/hud_online_eventlog.json'
# Skate 1's own id for a line the engine logs under another.
S1_IDS = {'ID_ONLINE_EVENT_NEWHOST': 'ID_ONLINE_EVENT_HOST_CHANGE'}
S1_REFUSED = {'ID_ONLINE_GAME_LEFT', 'ID_ONLINE_KICKED_BY_GAME_HOST'}
ENGINE = ('online_roster.rs', 'online_moves.rs', 'online_boundary.rs', 'online_menu.rs')


def load(path):
    return json.loads(path.read_text(encoding='utf-8'))


def dpo_icons(old_hud):
    """screens.big's images/dpoicons/*.rx2 (what dpo's LoadRX2Texture names) as RGBA payloads: {name: texture}."""
    from vendor.skate3_ui.rw4 import read_textures
    from vendor.skate3_ui.xenos import decode_rgba, parse_fetch_constant
    big = old_hud.Big(s2game/'data/fe/screens.big')
    out = {}
    for e in big.entries:
        path = e.path.replace('\\', '/')
        if '/images/dpoicons/' not in path.lower() or not path.lower().endswith('.rx2'):
            continue
        rel = 'assets/' + path[:-4]
        src = old_hud.WORK/(rel + '.rx2')
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_bytes(big.read(e))
        (t,) = read_textures(src)[1]
        m = t.metadata
        rgba = decode_rgba(t.data, parse_fetch_constant(bytes.fromhex(m['fetch_constant_hex'])))
        assert len(rgba) == m['width']*m['height']*4, path
        (old_hud.WORK/(rel + '.rgba')).write_bytes(rgba)
        out[Path(path).stem.lower()] = {'rgba': rel + '.rgba', 'width': m['width'], 'height': m['height']}
    return out


def screens():
    sys.argv = ['old_hud.py', str(repo), str(s2game), str(assets)]
    sys.path.insert(0, str(here))
    import old_hud, old_screens
    old_screens.WORK = old_hud.WORK = WORK_ROOT/'online-hud-cache'
    old_hud.OUT = S2_ONLINE
    movies = {}
    if '--results' in flags:
        movies |= {'roundresults_lb': 'data/fe/source/screens/hudComponents/roundresults',
                   'throwdownresults': 'data/fe/source/screens/challenge/throwdownresults'}
    if '--dpo' in flags:
        movies['dpo'] = 'data/fe/source/screens/hud2/dpo'
    if '--intro-overlays' in flags:
        movies |= {stem: 'data/fe/source/screens/' + path for stem, path in
                   (('spotfound', 'challenge/spotfound'), ('phototreatment', 'hudComponents/phototreatment'), ('sanvantv', 'nis/sanvantv'))}
    old_screens.MOVIES = movies
    stock = load(S2_ONLINE/'runtime/roundresults.json')
    cache = old_screens.cache()
    mappings = old_screens.font_mapping(old_screens.collections(), cache)
    for name, (family, apt) in old_screens.STAND_INS.items():
        cache.add_font_alias(name, family, mappings[apt])
        mappings[name] = mappings[apt] | {'stand_in': apt}
    manifests = {stem: old_screens.movie(cache, path, mappings, stock['language']) for stem, path in movies.items()}
    for stem, manifest in manifests.items():
        assert not manifest.get('unresolved_fonts'), (stem, manifest['unresolved_fonts'])
    if 'roundresults_lb' in manifests:
        board = load(S2_ONLINE/'runtime/leaderboardhud.json')['language']
        for key in ('ID_MAP_FILTER_CHALLENGETYPES_BESTTRICK', 'ID_MAP_FILTER_CHALLENGETYPES_JAM'):
            if key in board:
                manifests['roundresults_lb']['language'].setdefault(key, board[key])
    if 'dpo' in manifests:
        manifests['dpo']['images'] = dpo_icons(old_hud)
        payloads = old_hud.payloads
        old_hud.payloads = lambda m: payloads(m) | {t['rgba'] for t in m.get('images', {}).values()}
    old_hud.install(manifests)


def label_key(s):
    x = 0xFFFFFFFF
    for c in s.encode():
        x = (x * 33 + c) & 0xFFFFFFFF
    return x


def language():
    sys.path[:0] = [str(repo/'tools/vendor/university/tools/vanilla_map_extraction/tools'), str(here)]
    from s2big import Big
    from s2lang import table
    s1 = {}
    big = Big(s1game/'data/big/english.big')
    for e in big.entries:
        if e.path.lower().endswith('.bin') and 'histogram' not in e.path.lower():
            s1.update(table(big.read(e)))
    src = repo/'crates/skate-game/src'
    ids = set()
    for name in ENGINE:
        ids |= set(re.findall(r'"(ID_[A-Z0-9_]+)"', (src/name).read_text(encoding='utf-8')))
    ids |= set(re.findall(r'"(ID_[A-Z0-9_]+)"', (private/'challenge-map/online/online.json').read_text(encoding='utf-8')))
    s2 = load(S2_ONLINE/'runtime/hud_online_eventlog2.json')['language']
    s3 = load(private/'online-ui/runtime/hud_online_eventlog2.json')['language']
    manifest = load(S1_LOG)
    backup = S1_LOG.with_name(S1_LOG.stem + '.pre-onlinehud')
    if not backup.exists():
        shutil.copyfile(S1_LOG, backup)
    lang, sources = manifest['language'], {'s1': 0, 's2': 0, 's3': 0}
    for i in sorted(ids):
        own = None if i in S1_REFUSED else s1.get(label_key(S1_IDS.get(i, i)))
        for where, text in (('s1', own if isinstance(own, str) else None), ('s3', s3.get(i) if i in S1_REFUSED else None),
                            ('s2', s2.get(i)), ('s3', s3.get(i))):
            if text is not None:
                lang[i] = text
                sources[where] += 1
                break
    S1_LOG.write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
    print('Skate 1 event log:', len(ids), 'ids,', sources, '->', S1_LOG)
    for i in ('ID_ONLINE_EVENT_JOIN_SKATER', 'ID_ONLINE_EVENT_NEWHOST', 'ID_ONLINE_GAME_LEFT', 'ID_ONLINE_EVENT_LOCATION_CHANGE'):
        print(' ', i, '=', repr(lang.get(i)))


if __name__ == '__main__':
    if flags & {'--results', '--dpo', '--intro-overlays'}:
        screens()
    if '--language' in flags:
        language()
