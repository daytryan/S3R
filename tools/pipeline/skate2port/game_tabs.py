"""Online Challenge Map: Skate 2 and Skate 1 get their own tabs after Versus (user's design, 2026-10-02).

Team / Career / Freeskate / Versus stay retail's Skate 3 rows. Every row on a Skate 2 or Skate 1 map moves into
"#Skate 2" / "#Skate 1": a Freeskate folder (the Freeskate tab's folder for that game's map) first, then one folder
per mode (the challenge type filter, so the lobby title and match_core::Mode::from_filter still work), each opened by
the Versus folder's Options header + Quick Match as the retail folders are. A mode folder left with no rows on Versus
(Best Trick, Jam) goes. The tab order is the engine's lobby key order (online_session::TABS): keep them last.

Idempotent: a rerun rebuilds the game tabs from wherever the rows are. old_online.py (and any Skate 1 export)
merge their rows into Versus/Freeskate as before, then call apply().
usage: py -3 game_tabs.py ONLINE_JSON
"""
import json, shutil, sys
from pathlib import Path

GAMES = [('#Skate 2', ('New_San_Vanelona', 'Skate_2_')), ('#Skate 1', ('Skate_1_',))]
FILTER = 'ID_MAP_FILTER_CHALLENGETYPES_'
# Skate 2's own mode order (Freeskate first); a filter not listed goes after these.
ORDER = ['SPOTBATTLE', 'BESTTRICK', 'JAM', 'DEATHRACE', 'HOMBATTLE', 'HALLOFMEAT', 'SKATE', '1UP']
FREESKATE = 'ID_FREESKATEOPTION_START'


def game_of(e, games):
    x = e.get('challenge') or e.get('location')
    return next((g for g, maps in games if x and any(x['map'].startswith(m) for m in maps)), None)


def rows_of(rows):
    return [r for r in rows if 'challenge' in r or 'location' in r]


def apply(online, games=GAMES):
    names = [g for g, _ in games]
    old = {t['name']: t for t in online['tabs'] if t['name'] in names}
    tabs = [t for t in online['tabs'] if t['name'] not in names]
    freeskate = {g: [] for g in names}
    modes = {g: {} for g in names}  # game -> filter -> (folder template, rows)

    def take(folder, g, rows):
        name = folder['name']
        if name == FREESKATE or not name.startswith(FILTER):
            freeskate[g] += rows
            return
        template, mine = modes[g].setdefault(name, (folder, []))
        mine += rows

    # Game tabs from a previous run give their rows back first.
    for g, t in old.items():
        for f in t['rows']:
            take(f, g, f['rows'] if f['name'] == FREESKATE else rows_of(f['rows']))
    for t in tabs:
        kept = []
        for e in t['rows']:
            g = game_of(e, games)
            if g:  # a row at a tab root
                take({'name': FREESKATE if 'location' in e else t['name']}, g, [e])
                continue
            if not e.get('folder'):
                kept.append(e)
                continue
            inner = [r for r in e['rows'] if 'challenge' in r or 'location' in r]
            by_game = {g: [r for r in inner if game_of(r, games) == g] for g in names}
            if any(len(v) == len(inner) for v in by_game.values() if v) and 'location' in inner[0]:
                # A whole freeskate folder for one game's map: headers and all.
                take({'name': FREESKATE}, next(g for g, v in by_game.items() if v), e['rows'])
                continue
            for g, mine in by_game.items():
                if mine:
                    take(e, g, mine)
            moved = {id(r) for v in by_game.values() for r in v}
            # Drop the moved rows and any header left with no row under it (the old "#New San Vanelona" header).
            left = [r for r in e['rows'] if id(r) not in moved]
            left = [r for i, r in enumerate(left)
                    if not (r.get('header') and r.get('game') and not any(
                        'challenge' in n or 'location' in n for n in left[i + 1:i + 2]))]
            if rows_of(left) or not inner:
                kept.append(dict(e, rows=left))
        t['rows'] = kept
    versus = next((t for t in tabs if t['name'] == 'ID_MAP_FILTER_VERSUS'), None)
    for g in names:
        rows = []
        if freeskate[g]:
            rows.append({'name': FREESKATE, 'folder': True, 'rows': freeskate[g]})
        rank = lambda f: (ORDER.index(f[len(FILTER):]) if f[len(FILTER):] in ORDER else len(ORDER), f)
        for name in sorted(modes[g], key=rank):
            template, mine = modes[g][name]
            # The Versus folder's opening rows (Options header + Quick Match), as retail folders begin.
            source = next((f for f in (versus or {}).get('rows', []) if f.get('name') == name), template)
            opening = []
            for r in source.get('rows', []):
                if 'challenge' in r or 'location' in r or (r.get('header') and opening and 'command' in opening[-1]):
                    break
                opening.append(r)
            folder = {k: v for k, v in template.items() if k in ('name', 'help')}
            rows.append(dict(folder, folder=True, rows=opening + mine))
        tabs.append({'name': g, 'rows': rows})
    online['tabs'] = tabs
    return online


def install(path):
    path = Path(path)
    keep = path.with_name(path.name + '.pre-gametabs')
    if not keep.exists():
        shutil.copy2(path, keep)
    online = apply(json.loads(path.read_text(encoding='utf-8')))
    path.write_text(json.dumps(online, separators=(',', ':'), ensure_ascii=False) + '\n', encoding='utf-8')
    for t in online['tabs']:
        print(t['name'], [(f['name'], len(rows_of(f.get('rows', [])))) for f in t['rows'] if f.get('folder')][-12:])


def _check():
    loc = lambda m, n: {'name': n, 'location': {'map': m}}
    ch = lambda m, i: {'name': i, 'challenge': {'id': i, 'map': m}}
    opening = [{'name': 'ID_MAP_HEADER_VERSUS_OPTIONS', 'header': True}, {'name': 'QM', 'command': 'qm'}]
    online = {'tabs': [
        {'name': FREESKATE, 'rows': [{'name': 'Opts', 'command': 'x'}, {'name': '#NSV', 'folder': True, 'rows': [
            {'name': '#Skate 2 Online', 'header': True, 'game': 'skate2'}, loc('New_San_Vanelona', 'a')]},
            {'name': 'DT', 'folder': True, 'rows': [loc('DownTown', 'd')]},
            {'name': '#S1', 'folder': True, 'rows': [loc('Skate_1_San_Vanelona', 'b')]}]},
        {'name': 'ID_MAP_FILTER_VERSUS', 'rows': [
            {'name': FILTER + 'SKATE', 'folder': True, 'rows': opening + [ch('DownTown', 's3'), {'name': '#NSV', 'header': True, 'game': 'skate2'}, ch('New_San_Vanelona', 's2')]},
            {'name': FILTER + 'JAM', 'folder': True, 'game': 'skate2', 'rows': opening + [{'name': '#NSV', 'header': True, 'game': 'skate2'}, ch('New_San_Vanelona', 'j')]}]}]}
    once = apply(json.loads(json.dumps(online)))
    names = [t['name'] for t in once['tabs']]
    assert names == [FREESKATE, 'ID_MAP_FILTER_VERSUS', '#Skate 2', '#Skate 1'], names
    versus = once['tabs'][1]['rows']
    assert [f['name'] for f in versus] == [FILTER + 'SKATE'], 'Jam had only Skate 2 rows'
    assert [r['name'] for r in versus[0]['rows']] == ['ID_MAP_HEADER_VERSUS_OPTIONS', 'QM', 's3'], 'S2 header gone'
    s2 = once['tabs'][2]['rows']
    assert [(f['name'], [r['name'] for r in f['rows']]) for f in s2] == [
        (FREESKATE, ['#Skate 2 Online', 'a']), (FILTER + 'JAM', ['ID_MAP_HEADER_VERSUS_OPTIONS', 'QM', 'j']),
        (FILTER + 'SKATE', ['ID_MAP_HEADER_VERSUS_OPTIONS', 'QM', 's2'])], s2
    assert [r['name'] for r in once['tabs'][3]['rows'][0]['rows']] == ['b']
    assert [f['name'] for f in once['tabs'][0]['rows']] == ['Opts', 'DT']
    assert apply(json.loads(json.dumps(once))) == once, 'rerun changes nothing'


_check()
if __name__ == '__main__':
    install(sys.argv[1])
