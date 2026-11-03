# tools/pipeline/test_stages.py
"""The stage table is well formed: unique names, known inputs and dependencies, no cycles, versioned sources."""
import ast, contextlib, os, re, tempfile, unittest
from pathlib import Path
from tools.pipeline import sources, stages, runner
from tools.asset_pipeline import versions


class Table(unittest.TestCase):
    def test_well_formed(self):
        names = [s.name for s in stages.STAGES]
        self.assertTrue(names, 'the stage table is empty')
        self.assertEqual(len(names), len(set(names)))
        items = {i[0] for i in sources.ITEMS}
        for s in stages.STAGES:
            self.assertTrue(set(s.inputs) <= items, s.name)
            self.assertTrue(set(s.after) <= set(names), s.name)
            self.assertIn(s.step, range(8), s.name)
        self.assertEqual(len(runner.ordered()), len(names))
        self.assertTrue(set(names) <= set(runner.stage_fingerprints()), 'every stage has a fingerprint (Task 11)')
        self.assertNotIn('skate3-base', versions.fingerprints())  # it is the five groups; runner adds it (R30)

    def test_every_item_is_used(self):
        used = {i for s in stages.STAGES for i in s.inputs}
        self.assertEqual(used, {i[0] for i in sources.ITEMS})

    def test_shared_roots_are_ordered(self):
        def overlap(a, b):
            return a == b or a.startswith(b.rstrip('/') + '/') or b.startswith(a.rstrip('/') + '/')
        for i, a in enumerate(stages.STAGES):
            for b in stages.STAGES[i + 1:]:
                # runner.SHARED (the map catalogs) are edited under a lock by stages that run side by side
                if any(overlap(x, y) for x in a.roots for y in b.roots if not (x == y and x in runner.SHARED)):
                    self.assertTrue(b.name in runner.dependents(a.name) or a.name in runner.dependents(b.name),
                                    f'{a.name} and {b.name} write the same path but are not ordered')

    def test_dlc_map_constants_agree(self):
        names = {m[3] for m in stages.S3_DLC_MAPS}
        self.assertEqual(len(names), 7)
        self.assertEqual({t[0] for t in stages.S3_DLC_TELEPORTS}, names)
        packages = [(m[0], m[1]) for m in stages.S3_DLC_MAPS] + [(t[1], t[2]) for t in stages.S3_DLC_TELEPORTS] \
            + [(i[0], i[1]) for i in stages.S3_DLC_LOCATION_IMAGES]
        for item, big in packages:
            self.assertIn(item, stages.S3_DLC)
            self.assertTrue(big.endswith('.big'), big)
        for s in stages.STAGES:
            for r in s.roots:  # installation-relative, forward slashes
                self.assertTrue(r and not r.startswith('/') and '\\' not in r, r)


    def test_part2_constants_agree(self):
        maps = [m[0] for m in stages.MOD_MAPS]
        self.assertEqual(len(maps), len(set(maps)))
        self.assertEqual(len(maps), 6)
        for name, pres, sim, downscale, loc, spawn in stages.MOD_MAPS:
            self.assertTrue(pres.startswith('DIST_') and downscale in (1, 2), name)
            self.assertTrue(loc == '-' or loc.endswith('_Sim.loc'), name)
            self.assertTrue(spawn is None or loc != '-', name)  # '-' has no locators to spawn at
        self.assertEqual(len(stages.LOCATION_PHOTOS), 39)
        self.assertEqual(len(set(stages.LOCATION_PHOTOS)), 39)
        # every map a stage claims under maps/ is converted by that stage: no stray names
        claimed = {r[len('maps/'):].rsplit('.', 1)[0] for s in stages.STAGES for r in s.roots
                   if r.startswith('maps/') and s.name in ('mod-maps', 'skate2-parks', 'skate1-map')}
        self.assertEqual(claimed, {*maps, 'Skate_1_San_Vanelona'})

    def test_fingerprint_sources_name_what_each_stage_reads(self):
        by_name = {s.name: set(s.sources) for s in stages.STAGES}
        for name in ('mod-maps', 'skate2-parks', 'skate1-map', 'backdrops'):  # CONVERTER
            self.assertTrue({'asset_pipeline/retail_material.py', 'asset_pipeline/sky.py', 'asset_pipeline/environment.py'}
                            <= by_name[name], name)
        self.assertTrue({'asset_pipeline/optional_content.py', 'mixamo_to_skate/glb.py'} <= by_name['pro-clothing'])
        self.assertTrue({*stages.OLD_GAMES, *stages.VAULT} <= by_name['hall-of-meat-s3'])
        self.assertIn('pipeline/skate2port/tutorial_recovery.md', by_name['skate2-tutorial'])

    def test_every_hud_the_hud_group_writes_has_a_receipt_root(self):
        # hud-meters, music-ui and tip-loader come from tools/prepare_runtime_huds.py (setup's hud group), not from a stage
        from tools.asset_pipeline import group_receipts
        folders = set()
        for name in ('tip_loader', 'music_ui', 'hud_meters'):
            folders.update(re.findall(r'^FOLDER = "([\w-]+)"', (TOOLS/f'prepare_{name}.py').read_text(encoding='utf-8'), re.M))
        self.assertEqual(folders, {'tip-loader', 'music-ui', 'hud-meters'})
        for folder in folders:
            self.assertIn(f'assets/private/{folder}', group_receipts.ROOTS['hud'])
            self.assertFalse(any(f'assets/private/{folder}' in s.roots for s in stages.STAGES), folder)

    def test_every_source_glob_matches_a_file(self):
        tools = Path(stages.__file__).resolve().parents[1]
        for s in stages.STAGES:
            for pattern in s.sources:
                self.assertTrue(any(p.is_file() for p in tools.glob(pattern)), f'{s.name}: {pattern} matches nothing')


TOOLS = Path(stages.__file__).resolve().parents[1]
PORT = TOOLS/'pipeline/skate2port'


class Fake:
    """A build context that records what a stage runs. Scripts are not run; `script` only makes the few files a stage
    reads back from one (the tutorial's scratch export, the Skate 2 export folders)."""
    def __init__(self, root):
        self.base, self.work, self.install = root, root/'work', root/'inst'
        self.game_exe, self.env, self.calls, self.stage = root/'skate3rust.exe', {}, [], ''
        self.screens = stages.S2_SCREENS  # how many screens the fake old_screens.py manages to make
        (self.work/'tables').mkdir(parents=True)
        (self.work/'tables/.complete').write_text('')
        a = self.install/'assets/private'
        for rel in ('hud-skate2/assets/data/fe/source/images/icons/minimap/mini_hom.rgba', 'teleports.json',
                    'hud-skate2/online/runtime/hudonlinevoting.json'):
            (a/rel).parent.mkdir(parents=True, exist_ok=True)
            (a/rel).write_text('{}')

    def disc(self, item):
        return self.work/'discs'/item

    def item(self, item):
        path = self.base/'source'/item
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('x')
        return path

    def package(self, item):
        return self.work/'dlc'/item

    def progress(self, stage, text):
        pass

    def reserve(self, stage, gb):
        self.calls.append((stage, 'reserve', [str(gb)]))
        return contextlib.nullcontext()

    def run(self, stage, argv, env=None):
        self.calls.append((stage, 'run', [str(a) for a in argv]))

    def script(self, stage, rel, *args, env=None):
        args = [str(a) for a in args]
        self.calls.append((stage, rel, args))
        name = Path(rel).name
        if name == 'old_tutorial.py':
            (Path(args[2])/'private/skate2-tutorial').mkdir(parents=True)
            (Path(args[2])/'private/skate2-tutorial/tutorial.json').write_text('{}')
        elif name == 'skate3_online.py':
            (Path(args[args.index('--out') + 1])/args[args.index('--starts') + 1]).mkdir(parents=True, exist_ok=True)
        elif name == 'online_scoring.py':
            Path(args[args.index('--out') + 1]).write_text('{}')
        elif name == 'skate3_career.py':  # the export folder: a fresh one each run, so a file of an earlier export must not survive
            out = Path(args[args.index('--out') + 1])
            (out/'speech').mkdir(parents=True, exist_ok=True)
            (out/'career.json').write_text('{"version": 1}')
            (out/'speech/speech.json').write_text('{}')
        elif name == 'old_screens.py':  # exits 0 whatever happened; the stage counts what it left
            runtime = Path(args[2])/'private/skate2-tutorial/screens/runtime'
            runtime.mkdir(parents=True)
            for n in range(self.screens):
                (runtime/f'screen{n}.json').write_text('{}')
        elif name == 'old_online.py':  # reads Skate 2's minimap icons from its own export folder
            assert (Path(args[2])/'private/hud-skate2/assets/data/fe/source/images/icons/minimap/mini_hom.rgba').is_file()
        elif name in ('old_death_race.py', 'old_contests.py', 'old_skate_game.py', 'old_hall_of_meat.py'):
            (Path(args[2])/'screens').mkdir(parents=True)
            (Path(args[2])/'x.json').write_text('{}')
            if '--voting' in args:
                assert Path(args[args.index('--voting') + 1]).joinpath('runtime/hudonlinevoting.json').is_file()
                (Path(args[2])/'hud/runtime').mkdir(parents=True)
                (Path(args[2])/'hud/runtime/contests_voting.json').write_text('{}')


def positional(args):
    out = []
    for a in args:
        if a.startswith('--'):
            break
        out.append(a)
    return out


class Part3(unittest.TestCase):
    """Task 10: every script argument checked against the script's own sys.argv / argparse."""
    def run_stage(self, name, screens=None):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        ctx = Fake(Path(temp.name))
        ctx.screens = stages.S2_SCREENS if screens is None else screens
        next(s for s in stages.STAGES if s.name == name).run(ctx)
        return ctx

    def check_argv(self, ctx):
        for stage, rel, args in ctx.calls:
            if rel == 'run':
                continue
            source = (TOOLS/rel).read_text(encoding='utf-8')
            if 'sys.argv' not in source and 'argparse' not in source:
                source = (PORT/'old_hud.py').read_text(encoding='utf-8')  # old_screens.py: `import old_hud  # same argv`
            if 'argparse' in source:
                flags = set(re.findall(r'add_argument\(\s*"(--[\w-]+)"', source))
                needed = set(re.findall(r'add_argument\(\s*"(--[\w-]+)"[^)]*required=True', source))
                given = {a for a in args if a.startswith('--')}
                self.assertTrue(given <= flags and needed <= given, f'{rel}: {args}')
                continue
            wanted = int(re.search(r'sys\.argv\[1:(\d)\]', source).group(1)) - 1 if 'sys.argv[1:' in source \
                else max(int(i) for i in re.findall(r'sys\.argv\[(\d)\]', source))  # old_death_race reads argv[1], [2], [3]
            optional = Path(rel).name in ('old_death_race.py', 'old_tutorial_cast.py')  # these two take one optional last argument
            self.assertIn(len(positional(args)), (wanted, wanted + 1) if optional else (wanted,), f'{rel}: {args}')
            for flag in (a for a in args if a.startswith('--')):
                self.assertIn(f"'{flag}'", source, f'{rel} does not read {flag}')

    def test_every_new_stage_matches_its_scripts(self):
        for name in ('skate2-tutorial', 'skate1-challenges', 'skate2-ambience', 'skate2-traffic', 'online-ui', 'online-challenge-map',
                     'online-skate2', 'online-skate1', 'hall-of-meat-s3', 'online-skate3-export', 'online-skate3',
                     'retail-screens', 'skate2-physics', 'skate3-career'):
            if name not in {s.name for s in stages.STAGES}:
                continue  # skate3-career: story mode off (SKATE_STORY_MODE=0, a release's tool)
            with self.subTest(name):
                self.check_argv(self.run_stage(name))

    def test_skate1_map_points_convert_ps3_at_the_archive_pst(self):
        # the converter reads <--pres>/<district>_Pres.pst; it must be the path the Skate1Map archive really holds
        from unittest import mock
        from tools.owned_game import big
        archive = mock.Mock(entries=[])
        with mock.patch.object(big, 'BigArchive', return_value=archive), \
                mock.patch.object(stages, 'catalog_map'):
            ctx = self.run_stage('skate1-map')
        args = next(a for _, rel, a in ctx.calls if rel.endswith('convert_ps3.py'))
        rels = [rel for _, rel, _ in ctx.calls]  # the load check holds its 4.5 GB peak back from new stages
        self.assertEqual((ctx.calls[rels.index('reserve')][2], ctx.calls[rels.index('reserve') + 1][2][-1]), (['4.5'], '--check'))
        pres = Path(args[args.index('--pres') + 1])
        extracted = ctx.work/'skate1map'
        self.assertEqual((pres/f'{pres.name}_Pres.pst').relative_to(extracted).as_posix(),
                         'data/content/world/stream/DIST_SanVanelona/DIST_SanVanelona_Pres.pst')

    def test_weather_audio_installs_every_file_its_manifest_names(self):
        import json
        ctx = self.run_stage('weather-audio')
        out = ctx.install/'assets/weather-audio'
        manifest = json.loads((out/'manifest.json').read_text(encoding='utf-8'))
        named = set(re.findall(r'"([\w-]+\.ogg)"', json.dumps(manifest)))
        self.assertTrue(named, 'the manifest names no recordings')
        self.assertEqual(named, {p.name for p in out.glob('*.ogg')})
        self.assertFalse((ctx.install/'assets/weather-audio.new').exists())

    def test_work_copies_are_removed(self):
        for name in ('skate2-tutorial', 'online-ui', 'online-challenge-map', 'online-skate2', 'online-skate1', 'hall-of-meat-s3',
                     'online-skate3'):
            with self.subTest(name):
                ctx = self.run_stage(name)
                self.assertEqual(sorted(p.name for p in ctx.work.iterdir() if p.name not in ('tables', 'cache')), [])

    def test_tutorial_is_copied_whole_and_screens_come_after(self):
        ctx = self.run_stage('skate2-tutorial')
        self.assertTrue((ctx.install/'assets/private/skate2-tutorial/tutorial.json').is_file())
        order = [Path(rel).name for _, rel, _ in ctx.calls if rel != 'run']
        # old_tutorial clears its output folder, so everything else runs after the copy; old_indicators reads the vault old_screens caches
        self.assertEqual(order[0], 'old_tutorial.py')
        self.assertLess(order.index('old_screens.py'), order.index('old_indicators.py'))
        sounds = next(a for _, rel, a in ctx.calls if rel == 'run')
        self.assertEqual(sounds[1], '--s2-fe-sounds')  # R20: the 3rd argument is the converted SCREENS folder
        self.assertEqual(Path(sounds[4]), ctx.install/'assets/private/skate2-tutorial/screens')
        self.assertEqual(Path(sounds[3]), ctx.work/'cache/screens-cache/skater-collections.json')

    def test_the_tutorial_stage_adds_slappys_gate_to_the_finished_folder(self):
        # User 10-28: old_tutorial_gate.py (Slappy's gate, round2 10-14) only ever ran by hand, so launcher-built copies
        # had no gate. It runs on the finished folder after the export's copy (it re-runs old_tutorial_dmos.py there).
        ctx = self.run_stage('skate2-tutorial')
        calls = [(Path(rel).name, args) for _, rel, args in ctx.calls if rel != 'run']
        order = [name for name, _ in calls]
        self.assertIn('old_tutorial_gate.py', order)
        self.assertLess(order.index('old_tutorial.py'), order.index('old_tutorial_gate.py'))
        self.assertEqual(Path(dict(calls)['old_tutorial_gate.py'][-1]), ctx.install/'assets')
        self.check_argv(ctx)

    def test_skate2_online_parts_land_in_their_folders(self):
        ctx = self.run_stage('online-skate2')
        online = ctx.install/'assets/private/skate2-online'
        for sub in ('death-race', 'contests', 'skate', 'hall-of-meat'):
            self.assertTrue((online/sub/'x.json').is_file(), sub)
        self.assertFalse((online/'contests/hud').exists())  # the voting HUD goes to hud-skate2/online instead
        self.assertTrue((ctx.install/'assets/private/hud-skate2/online/runtime/contests_voting.json').is_file())
        sounds = [a for _, rel, a in ctx.calls if rel == 'run']
        self.assertEqual([(Path(a[3]).parent.name, Path(a[4]).name) for a in sounds],
                         [('skate-cache', 'screens'), ('hom-cache', 'hall-of-meat')])
        for a in sounds:  # the sounds are made in the scratch folder, before it is copied to the install
            self.assertIn('online-s2-parts', Path(a[4]).parts)
        death_race = next(a for _, rel, a in ctx.calls if rel.endswith('old_death_race.py'))
        self.assertEqual(Path(death_race[3]), ctx.install/'assets/private/teleports.json')

    def test_online_hud_runs_last_in_skate1_with_all_its_flags(self):
        ctx = self.run_stage('online-skate1')
        self.assertEqual([Path(rel).name for _, rel, _ in ctx.calls], ['old_online1.py', 'old_online1_modes.py', 'old_online_hud.py', 'run'])
        self.assertEqual(ctx.calls[2][2][-4:], ['--results', '--dpo', '--language', '--intro-overlays'])
        # audit I4: the Skate 2 online HUD layer plays roundresults_lb's sounds from its own sfx, after the HUD is written
        sounds = ctx.calls[3][2]
        self.assertEqual(sounds[1], '--s2-fe-sounds')
        self.assertEqual(Path(sounds[3]), ctx.work/'cache/online-hud-cache/skater-collections.json')
        self.assertEqual(Path(sounds[4]), ctx.install/'assets/private/hud-skate2/online')
        export = {a[2] for _, _, a in ctx.calls[:2]}  # old_online1_modes reads what old_online1 exported
        self.assertEqual(len(export), 1)
        self.assertNotEqual(Path(export.pop()), ctx.install/'assets')  # --install onto itself raises SameFileError

    def test_skate1_challenges_writes_the_cast_after_the_folder_is_cleared(self):
        # old_challenges rmtrees challenges-skate1/; the cast (P1 task 8) writes cast/ and anim1/ inside it
        ctx = self.run_stage('skate1-challenges')
        self.assertEqual([Path(rel).name for _, rel, _ in ctx.calls if rel.endswith('.py')],
                         ['old_challenges.py', 'old_scenes1.py', 'old_challenges_cast.py', 'old_ai1.py', 'old_indicator1.py'])
        stage = next(s for s in stages.STAGES if s.name == 'skate1-challenges')
        self.assertTrue({stages.S2_PORT + 'old_challenges_cast.py', stages.S2_PORT + 'old_ai1.py', 'skate3_ai_paths.py',
                         stages.S2_PORT + 'old_indicator1.py', *stages.CUSTOMISER} <= set(stage.sources))

    def test_skate1_hud_writes_the_tutorial_screens_after_the_hud(self):
        # P1 task 10: old_screens1 writes hud-skate1/tutorial/ with old_screens.py's merge (Skate 2's screens export)
        ctx = self.run_stage('hud-skate1')
        self.assertEqual([Path(rel).name for _, rel, _ in ctx.calls], ['old_hud1.py', 'old_screens1.py'])
        stage = next(s for s in stages.STAGES if s.name == 'hud-skate1')
        self.assertTrue({stages.S2_PORT + 'old_screens1.py', stages.S2_PORT + 'old_screens.py', stages.S2_PORT + 'old_hud.py',
                         'prepare_core_menu.py', 'prepare_hud.py'} <= set(stage.sources))

    def test_skate3_online_builds_the_modes_then_the_scoring_table(self):
        # retail-audit C1: the S3 mode data (assets/private/skate3-online/<mode>) and scoring.json beside it
        ctx = self.run_stage('online-skate3-export')
        self.assertEqual([Path(rel).name for _, rel, _ in ctx.calls], ['skate3_online.py', 'online_scoring.py'])
        modes, scoring = ctx.calls[0][2], ctx.calls[1][2]
        export = ctx.work/'online-s3'
        self.assertEqual(modes[-2:], ['--starts', 'skip'])  # online.json is not written yet
        self.assertEqual(Path(modes[modes.index('--assets') + 1]), ctx.install/'assets')
        self.assertEqual(Path(modes[modes.index('--out') + 1]), export)
        self.assertEqual(Path(scoring[scoring.index('--out') + 1]), export/'scoring.json')
        dlc = Path(scoring[scoring.index('--dlc') + 1])
        self.assertEqual(dlc.parent, ctx.work)  # a folder of exactly the nine packages, removed afterwards
        self.assertFalse(dlc.exists())
        self.assertTrue((export/'.complete').is_file())

        # after the online challenge map: the modes that read online.json again, then everything is installed
        next(s for s in stages.STAGES if s.name == 'online-skate3').run(ctx)
        self.assertEqual([(Path(rel).name, a[-1]) for _, rel, a in ctx.calls[2:]], [('skate3_online.py', 'only')])
        online = ctx.install/'assets/private/skate3-online'
        self.assertEqual(sorted(p.name for p in online.iterdir()), ['only', 'scoring.json', 'skip'])
        self.assertFalse(export.exists())
        self.assertEqual(next(s for s in stages.STAGES if s.name == 'online-skate3').roots, ('assets/private/skate3-online',))

    def test_skate3_online_remakes_a_lost_export(self):
        ctx = self.run_stage('online-skate3')  # a resumed build whose work/online-s3 is gone
        self.assertEqual([a[-1] for _, rel, a in ctx.calls if rel == 'skate3_online.py'], ['skip', 'only'])
        self.assertTrue((ctx.install/'assets/private/skate3-online/scoring.json').is_file())

    @unittest.skipUnless(stages.story_mode(), 'story mode is off: no skate3-career stage')
    def test_skate3_career_exports_into_its_folder_and_replaces_the_old_one(self):
        # Story mode, Part A: tools/skate3_career.py writes career.json, strings.json, scripts/, rows/, speech/, scenes/, cast/, screens/ and ai/
        # into assets/private/skate3-career. The folder is replaced whole, so a file of an earlier export that this one lacks is gone.
        stage = next(s for s in stages.STAGES if s.name == 'skate3-career')
        self.assertEqual((stage.step, stage.inputs, stage.after, stage.roots),
                         (1, ('s3-disc', *stages.S3_DLC), ('skate3-base',), ('assets/private/skate3-career',)))
        self.assertTrue({'skate3_career.py', 'skate3_career_cast.py', 'ea_speech.py', 'skate3_ai_paths.py', 'prepare_retail_screens.py',
                         'prepare_core_menu.py', 'skate3_online.py', *stages.UNLUAC_BATCH} <= set(stage.sources))
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        ctx = Fake(Path(temp.name))
        root = ctx.install/'assets/private/skate3-career'
        (root/'speech').mkdir(parents=True)
        (root/'speech/old_take_0.wav').write_text('x')
        stage.run(ctx)
        self.check_argv(ctx)
        (_, rel, args), = ctx.calls
        self.assertEqual(rel, 'skate3_career.py')
        self.assertEqual(Path(args[args.index('--disc') + 1]), ctx.disc('s3-disc'))
        self.assertEqual(Path(args[args.index('--assets') + 1]), ctx.install/'assets')
        dlc = Path(args[args.index('--dlc') + 1])
        self.assertEqual(dlc.parent, ctx.work)   # a folder of exactly the nine packages, removed afterwards
        self.assertFalse(dlc.exists())
        out = Path(args[args.index('--out') + 1])
        self.assertNotEqual(out, root)           # the export is made in the work folder and moved into place
        self.assertEqual(sorted(p.name for p in root.iterdir()), ['career.json', 'speech'])
        self.assertEqual(sorted(p.name for p in (root/'speech').iterdir()), ['speech.json'])
        self.assertFalse((ctx.install/'assets/private/skate3-career.new').exists())
        self.assertEqual(sorted(p.name for p in ctx.work.iterdir() if p.name not in ('tables', 'cache')), [])

    @unittest.skipUnless(stages.story_mode(), 'story mode is off: no skate3-career stage')
    def test_skate3_career_keeps_the_old_folder_when_the_export_fails(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        ctx = Fake(Path(temp.name))
        root = ctx.install/'assets/private/skate3-career'
        root.mkdir(parents=True)
        (root/'career.json').write_text('{"old": 1}')

        def failing(stage, rel, *args, env=None):
            raise RuntimeError('refused (missing: vgmstream)')
        ctx.script = failing
        with self.assertRaises(RuntimeError):
            next(s for s in stages.STAGES if s.name == 'skate3-career').run(ctx)
        self.assertEqual((root/'career.json').read_text(), '{"old": 1}')
        self.assertEqual(sorted(p.name for p in ctx.work.iterdir() if p.name not in ('tables', 'cache')), [])

    def test_skate3_career_has_its_weight_label_and_memory(self):
        self.assertIn('skate3-career', runner.WEIGHTS)
        self.assertIn('skate3-career', runner.LABELS)
        self.assertIn('skate3-career', runner.MEMORY)

    def test_party_play_reads_the_party_pack_and_installs_its_folder(self):
        ctx = self.run_stage('party-play')
        self.check_argv(ctx)
        (_, rel, args), = ctx.calls
        self.assertEqual(rel, 'prepare_party_play.py')
        self.assertEqual(Path(args[args.index('--dlc') + 1]), ctx.package('s3-dlc-sanvan')/'play_00000000.big')
        self.assertEqual(Path(args[args.index('--install') + 1]), ctx.install/'assets')
        self.assertEqual(sorted(p.name for p in ctx.work.iterdir() if p.name not in ('tables', 'cache')), [])
        self.assertEqual(next(s for s in stages.STAGES if s.name == 'party-play').roots, ('assets/private/party-play',))

    def test_replay_ui_reads_the_disc_and_skate_create(self):
        stage = next(s for s in stages.STAGES if s.name == 'replay-ui')
        self.assertEqual(stage.inputs, ('s3-disc', 's3-dlc-create'))
        self.assertEqual(stage.after, ('online-ui',))
        self.assertEqual(stage.roots, ('assets/private/replay-ui',))
        self.assertIn('prepare_replay_ui.py', stage.sources)
        ctx = self.run_stage('replay-ui')
        self.check_argv(ctx)
        (_, rel, args), = ctx.calls
        self.assertEqual(rel, 'prepare_replay_ui.py')
        self.assertEqual(Path(args[args.index('--dlc') + 1]), ctx.package('s3-dlc-create')/'creator_00000000.big')
        self.assertEqual(sorted(p.name for p in ctx.work.iterdir() if p.name not in ('tables', 'cache')), [])

    def test_online_movies_read_the_disc_and_install_their_folder(self):
        stage = next(s for s in stages.STAGES if s.name == 'online-movies')
        self.assertEqual((stage.inputs, stage.after, stage.roots), (('s3-disc',), ('online-ui',), ('assets/private/online-movies',)))
        self.assertIn('prepare_online_movies.py', stage.sources)
        # Item 30b's chyron, item 6's team results (screen 35) and proposal poster (screen 23).
        movies = (TOOLS/'prepare_online_movies.py').read_text(encoding='utf-8').split('MOVIES = {')[1].split('\n}')[0]
        self.assertEqual(re.findall(r'^\s*"(\w+)":', movies, re.M), ['online_chyron', 'endchallenge_teams', 'challenge_signup'])
        ctx = self.run_stage('online-movies')
        self.check_argv(ctx)
        (_, rel, args), = ctx.calls
        self.assertEqual(rel, 'prepare_online_movies.py')
        self.assertEqual(Path(args[args.index('--install') + 1]), ctx.install/'assets')
        self.assertEqual(sorted(p.name for p in ctx.work.iterdir() if p.name not in ('tables', 'cache')), [])

    def test_hall_of_meat_s3_installs(self):
        self.assertEqual(self.run_stage('hall-of-meat-s3').calls[0][2][-1], '--install')

    def test_hall_of_meat_s3_rebuild_drops_its_patched_manifest(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        ctx = Fake(Path(temp.name))
        runtime = ctx.install/'assets/private/online-ui/runtime'
        runtime.mkdir(parents=True)
        for name in ('homscoring.json', 'homscoring.json.pre-skate2online', 'homscoring.json.pre-skate1online', 'friends.json'):
            (runtime/name).write_text('{}')
        next(s for s in stages.STAGES if s.name == 'hall-of-meat-s3').run(ctx)
        self.assertEqual(sorted(p.name for p in runtime.iterdir()), ['friends.json'])

    def test_tutorial_stage_fails_when_a_screen_is_not_made(self):
        # old_screens.py prints the failed movie and exits 0; the stage must not call that a finished tutorial
        with self.assertRaisesRegex(RuntimeError, r'25 of 26 screens'):
            self.run_stage('skate2-tutorial', screens=25)
        movies = (PORT/'old_screens.py').read_text(encoding='utf-8').split('MOVIES = {')[1].split('\n}')[0]
        self.assertEqual(len(re.findall(r"^\s*'\w+':", movies, re.M)), stages.S2_SCREENS)  # the constant follows the script

    def test_order(self):
        for stage, after in (('online-skate2', 'hud-skate2'), ('online-skate1', 'hud-skate1'), ('online-skate1', 'hud-skate2'),
                             ('online-skate1', 'online-skate2'), ('online-skate2', 'online-challenge-map'),
                             ('online-challenge-map', 'online-ui'), ('hall-of-meat-s3', 'online-skate1'),
                             ('online-skate3', 'online-challenge-map'), ('online-skate3', 'online-skate3-export')):
            self.assertIn(stage, runner.dependents(after), f'{stage} must follow {after}')

    def test_roots_agree_with_the_scripts(self):
        by_name = {s.name: s for s in stages.STAGES}
        modes = (PORT/'old_online.py').read_text(encoding='utf-8').split('MODES = {')[1].split('S2_ICONS')[0]
        icons = set(re.findall(r"'(mini_\w+)'", modes))
        self.assertEqual(len(icons), 7)
        self.assertEqual({r for r in by_name['online-skate2'].roots if 'icons/skate2_' in r},
                         {f'assets/private/challenge-map/online/icons/skate2_{i}.rgba' for i in icons})
        for name in ('online-skate1', 'online-skate2'):  # both patch the same tables in place
            self.assertTrue({*stages.LANGUAGE_FILES, stages.ONLINE_JSON} <= set(by_name[name].roots), name)
        self.assertIn('assets/private/hud-skate1/online', by_name['online-skate1'].roots)
        self.assertNotIn('assets/private/hud-skate1', by_name['online-skate1'].roots)  # only the online subfolders
        self.assertNotIn('assets/private/hud-skate2', by_name['online-skate2'].roots)

    def test_old_challenges_parses_as_3_12(self):
        # the script says `py -3.12`; the build's interpreter is checked statically, the script is never run
        ast.parse((PORT/'old_challenges.py').read_text(encoding='utf-8'), feature_version=(3, 12))

    def test_no_exit_in_stage_code(self):
        for node in ast.walk(ast.parse(Path(stages.__file__).read_text(encoding='utf-8'))):
            self.assertFalse(isinstance(node, ast.Attribute) and node.attr == 'exit', 'in-process code raises, never exits')
            self.assertFalse(isinstance(node, ast.Name) and node.id == 'SystemExit', 'in-process code raises, never exits')


class PadGlyphs(unittest.TestCase):
    MOD = Path(os.environ.get('SKATE_SOURCES') or 'unset', '07 - RPCS3 New San Van Mod (Skate 2 maps for Skate 3 PS3)')

    def test_the_ps3_glyphs_install_under_their_xbox_names(self):
        if not (self.MOD/stages.PAD_GLYPHS_BIG).is_file():
            self.skipTest('the New San Van mod is not here')
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as tmp:
            stages.pad_glyphs(SimpleNamespace(item=lambda _: self.MOD, install=Path(tmp)))
            out = Path(tmp)/'assets/private/pad-glyphs'
            names = {p.stem for p in out.glob('*.png')}
            self.assertTrue({'A', 'B', 'X', 'Y', 'Start', 'Back', 'DPad_Up', 'Stick_Left'} <= names, names)
            self.assertTrue((out/'A.png').read_bytes().startswith(b'\x89PNG'))


class StoryModeSwitch(unittest.TestCase):
    """A release without story mode (user 10-22) builds no story mode export."""
    def test_the_switch_reads_the_build_then_the_bundled_flags(self):
        with tempfile.TemporaryDirectory() as tmp:
            flags = Path(tmp)/'build_flags.json'
            self.assertTrue(stages.story_mode({}, flags), 'a dev checkout has no flags: on')
            flags.write_text('﻿{"story_mode": false}', encoding='utf-8')  # PowerShell 5.1 writes a BOM
            self.assertFalse(stages.story_mode({}, flags))
            self.assertTrue(stages.story_mode({'SKATE_STORY_MODE': '1'}, flags), 'the release build decides')
            self.assertFalse(stages.story_mode({'SKATE_STORY_MODE': '0'}, Path(tmp)/'absent.json'))

    def test_a_build_without_story_mode_has_no_career_stage(self):
        import os, subprocess, sys
        code = ('from tools.pipeline import stages, runner; '
                'print(sorted(s.name for s in stages.STAGES) == sorted(s.name for s in runner.ordered()), '
                '"skate3-career" in [s.name for s in stages.STAGES])')
        root = Path(__file__).resolve().parents[2]
        for value, listed in (('0', 'False'), ('1', 'True')):
            out = subprocess.run([sys.executable, '-c', code], cwd=root, capture_output=True, text=True,
                                 env={**os.environ, 'SKATE_STORY_MODE': value}, check=True).stdout.split()
            self.assertEqual(out, ['True', listed], value)


if __name__ == '__main__':
    unittest.main()
