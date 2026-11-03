# tools/pipeline/test_runner.py
"""Runner behaviour with a synthetic stage table: order, resume, partial stages, disk refusal, odd paths."""
import contextlib, dataclasses, hashlib, io, json, os, shutil, subprocess, sys, tempfile, time, types, unittest
from pathlib import Path
from unittest import mock
from tools.pipeline import runner
from tools.pipeline.stages import Stage


def table(log, fail=None, text=None):
    def make(name):
        def run(ctx):
            log.append(name)
            if name == fail:
                raise RuntimeError('boom')
            out = ctx.install/'assets/private'/name
            out.mkdir(parents=True, exist_ok=True)
            (out/'x.txt').write_text((text or {}).get(name, name))
        return run
    return [Stage('a', 0, (), (), ('assets/private/a',), ('pipeline/runner.py',), make('a')),
            Stage('b', 1, (), ('a',), ('assets/private/b',), ('pipeline/runner.py',), make('b')),
            Stage('c', 2, (), ('b',), ('assets/private/c',), ('pipeline/stages.py',), make('c'))]


class Runner(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name, "data & Ünï's")
        (self.base/'installations/0123456789abcdef0123456789abcdef').mkdir(parents=True)
        (self.base/'installation.json').write_text(json.dumps(
            {'version': 1, 'directory': 'installations/0123456789abcdef0123456789abcdef', 'pipelines': {}}))
        self.events = []
        self._real_emit = runner.emit
        p = mock.patch.object(runner, 'emit', lambda event, **f: self.events.append({'event': event, **f}))
        p.start(); self.addCleanup(p.stop); self.addCleanup(self.temp.cleanup)
        # the synthetic installs have no setup groups, which group_receipts reads as damaged; tests set what is damaged
        g = mock.patch.object(runner.group_receipts, 'damaged', return_value=set())
        self.group_damage = g.start(); self.addCleanup(g.stop)
        # nor a clothing set; tests that need one say so (test_a_stale_clothing_set_...)
        self.customiser = mock.patch.object(runner, 'customiser_current', return_value=True)
        self.customiser.start(); self.addCleanup(self.customiser.stop)
        # no PowerShell disk queries in tests (drive_kind); test_a_hard_drive_... says what a drive is
        d = mock.patch.object(runner, 'drive_kind', return_value='SSD NVMe')
        self.drive = d.start(); self.addCleanup(d.stop)
        # nor this PC's long path setting; test_a_pc_without_long_paths_... turns it off
        lp = mock.patch.object(runner, 'long_paths', return_value=True)
        self.long_paths = lp.start(); self.addCleanup(lp.stop)
        self.addCleanup(setattr, runner, 'HASHERS', runner.HASHERS)
        f = mock.patch('tools.pipeline.fetch_tools.ensure', return_value={})  # never the real downloads
        f.start(); self.addCleanup(f.stop)

    def build(self, stages, prints=None):
        prints = prints or {s.name: s.name + '-v1' for s in stages}
        with mock.patch.object(runner, 'STAGES', stages), mock.patch.object(runner, 'stage_fingerprints', lambda: prints):
            return runner.build(runner.Context(self.base, Path('game.exe'), {}), resume=True)

    def test_a_failed_update_leaves_the_last_build_playable_and_the_next_build_resumes(self):
        # #9: skate3-base moves an update into a new installation folder; a later stage fails. installation.json goes
        # back to the last sealed build (its folder untouched), and the update's own marker waits in installation.next.json
        log, made = [], []
        def base(ctx):
            log.append('skate3-base')
            old = ctx.install
            new = old.parent/f'{len(made) + 1:032x}'
            shutil.copytree(old, new)
            made.append(new)
            marker = ctx.marker
            marker['directory'] = new.relative_to(ctx.base).as_posix()
            runner.atomic_json(ctx.base/'installation.json', marker)
        after_base = lambda t: [dataclasses.replace(t[0], after=('skate3-base',)), *t[1:]]
        stages = [Stage('skate3-base', 0, (), (), (), (), base), *after_base(table(log))]
        prints = {s.name: s.name + '-v1' for s in stages}
        self.assertTrue(self.build(stages, prints))
        first = json.loads((self.base/'installation.json').read_text())
        self.assertTrue(first['sealed'])
        log.clear()
        update = {**prints, 'skate3-base': 'skate3-base-v2'}
        self.assertFalse(self.build([stages[0], *after_base(table(log, fail='b'))], update))
        self.assertEqual(log, ['skate3-base', 'a', 'b'])
        self.assertEqual(json.loads((self.base/'installation.json').read_text()), first)  # the last build plays
        self.assertTrue((self.base/first['directory']).is_dir())
        pending = json.loads((self.base/runner.NEXT).read_text())
        self.assertEqual(pending['directory'], made[-1].relative_to(self.base).as_posix())
        log.clear()
        self.assertTrue(self.build([stages[0], *after_base(table(log))], update))
        self.assertEqual(log, ['b', 'c'])  # resumed: skate3-base and a are not done again
        final = json.loads((self.base/'installation.json').read_text())
        self.assertEqual((final['directory'], final['sealed']), (pending['directory'], True))
        self.assertFalse((self.base/runner.NEXT).exists())
        self.assertFalse((self.base/first['directory']).exists())  # the old build goes once the update is sealed

    def test_order_then_resume_skips(self):
        log = []
        self.assertTrue(self.build(table(log)))
        self.assertEqual(log, ['a', 'b', 'c'])
        log.clear(); self.assertTrue(self.build(table(log)))
        self.assertEqual(log, [])
        self.assertEqual([e['status'] for e in self.events if e['event'] == 'stage'][-3:], ['skip'] * 3)

    def test_changed_fingerprint_reruns_stage_and_dependents_only(self):
        log = []; self.build(table(log)); log.clear()
        self.build(table(log, text={'b': 'b, changed'}), {'a': 'a-v1', 'b': 'b-v2', 'c': 'c-v1'})
        self.assertEqual(log, ['b', 'c'])

    def test_a_rerun_that_writes_the_same_files_stops_there(self):
        # #4 early cutoff: a shared-code edit reruns a stage; the same files out = what follows it is still current
        log = []; self.build(table(log)); log.clear()
        self.assertTrue(self.build(table(log), {'a': 'a-v2', 'b': 'b-v1', 'c': 'c-v1'}))
        self.assertEqual(log, ['a'])
        self.assertEqual([(e['name'], e['status']) for e in self.events if e['event'] == 'stage'][-2:], [('b', 'skip'), ('c', 'skip')])
        log.clear(); self.build(table(log), {'a': 'a-v2', 'b': 'b-v1', 'c': 'c-v1'})
        self.assertEqual(log, [], 'b and c stay finished for the next build too')
        # a stage after a changed one reruns, and so does everything after it, even when its own files come out the same
        log.clear(); self.build(table(log, text={'a': 'a, changed'}), {'a': 'a-v3', 'b': 'b-v1', 'c': 'c-v1'})
        self.assertEqual(log, ['a', 'b', 'c'])
        # a stage with its own change still runs after an unchanged one
        log.clear(); self.build(table(log, text={'a': 'a, changed'}), {'a': 'a-v4', 'b': 'b-v1', 'c': 'c-v2'})
        self.assertEqual(log, ['a', 'c'])

    def test_failed_stage_is_not_recorded_and_resume_retries_it(self):
        log = []
        self.assertFalse(self.build(table(log, fail='b')))
        marker = json.loads((self.base/'installation.json').read_text())
        self.assertIn('a', marker['pipelines']); self.assertNotIn('b', marker['pipelines'])
        self.assertEqual(self.events[-1]['kind'], 'failed')
        log.clear(); self.build(table(log))
        self.assertEqual(log, ['b', 'c'])

    def test_damaged_output_reruns_its_stage(self):
        log = []; self.build(table(log))
        for damage in ('changed!', 'c'):  # a different size, then the same size (only the sha differs)
            log.clear()
            (self.base/'installations/0123456789abcdef0123456789abcdef/assets/private/b/x.txt').write_text(damage)
            self.build(table(log))
            self.assertEqual(log, ['b'], damage)  # b's files are back as receipted: c is still current (#4)

    def test_a_resumed_build_reads_no_unchanged_file_again(self):
        log = []; self.build(table(log))
        self.assertTrue((self.base/runner.HASHES).is_file())
        runner._hashes.clear()  # as a new launcher process would start
        out = self.base/'installations/0123456789abcdef0123456789abcdef/assets/private/b/x.txt'
        out.write_text('bb')  # changed: hashed again and found damaged
        with mock.patch.object(runner, '_plain_sha', wraps=runner._plain_sha) as read:
            log.clear(); self.build(table(log))
        self.assertEqual(log, ['b'])
        self.assertIn(out.resolve(), [Path(c.args[0]).resolve() for c in read.call_args_list])
        self.assertNotIn('a', {Path(c.args[0]).parent.name for c in read.call_args_list})  # a's file: from hashes.json

    def test_stale_finds_missing_resized_and_rewritten_files(self):
        root = Path(self.temp.name, 'stale'); (root/'d').mkdir(parents=True)
        for name in ('same', 'd/resized', 'd/rewritten'):
            (root/name).write_text('abc')
        receipts = runner.receipt(root, [root/'same', root/'d/resized', root/'d/rewritten'])
        receipts['gone'] = receipts['same']
        (root/'d/resized').write_text('abcd'); (root/'d/rewritten').write_text('xyz')
        later = (root/'d/rewritten').stat().st_mtime_ns + 10**9  # a rewrite in the same mtime tick is _sha's known ceiling
        os.utime(root/'d/rewritten', ns=(later, later))
        self.assertEqual(runner.stale(root, receipts), {'gone', 'd/resized', 'd/rewritten'})

    def test_disk_refusal(self):
        with mock.patch('shutil.disk_usage', return_value=(0, 0, 3 << 30)):
            self.assertFalse(runner.enough_disk(self.base, first=True))
        self.assertEqual(self.events[-1]['kind'], 'disk')
        self.assertIn('45 GB', self.events[-1]['message'])

    def test_a_stage_ending_before_the_base_install_is_published_waits_for_it(self):
        marker = json.loads((self.base/'installation.json').read_text())
        directory = marker.pop('directory')
        (self.base/'installation.json').write_text(json.dumps(marker))  # a fresh build: skate3-base publishes it
        tables_done = []

        def base(ctx):  # install.publish_marker: merges into the file, without the runner's lock
            end = time.monotonic() + 30
            while not tables_done and time.monotonic() < end:
                time.sleep(0.05)
            time.sleep(0.2)  # tables is recording by now
            old = json.loads((self.base/'installation.json').read_text())
            (self.base/'installation.json').write_text(json.dumps({**old, 'directory': directory}))
        # side by side whatever this PC has free (memory admission would otherwise run them one after the other)
        with mock.patch.object(runner, 'budget_gb', return_value=64.0), mock.patch.object(runner, 'workers', return_value=2):
            self.assertTrue(self.build([Stage('skate3-base', 0, (), (), (), (), base),
                                        Stage('tables', 0, (), (), (), (), lambda ctx: tables_done.append(1))]))
        saved = json.loads((self.base/'installation.json').read_text())
        self.assertEqual((saved['directory'], sorted(saved['pipelines'])), (directory, ['skate3-base', 'tables']))

    def test_reserve_waits_for_memory_only_while_other_stages_can_free_it(self):
        ctx = runner.Context(self.base, Path('game.exe'), {})
        ctx.bar = types.SimpleNamespace(lock=runner.threading.RLock(), started={'map': 0, 'other': 0})
        checks = []

        def short():  # the PC never has 5.5 GB free; on the third look the other stage has ended
            checks.append(1)
            if len(checks) == 3:
                del ctx.bar.started['other']
            return 2.0
        with mock.patch('tools.pipeline.fanout.available_gb', short), mock.patch.object(runner.time, 'sleep'):
            with ctx.reserve('map', 5.5):
                self.assertEqual(runner.EXTRA['map'], 5.5 - runner.MEMORY.get('map', runner.DEFAULT_GB))
        self.assertEqual(len(checks), 3)  # not the 5 min: the check runs alone as soon as nothing else can free memory
        self.assertNotIn('map', runner.EXTRA)

    def test_script_arguments_with_odd_characters_survive(self):
        ctx = runner.Context(self.base, Path('game.exe'), {})
        odd = Path(self.temp.name, "Skate 1 & 2 ISO", "Danny Way's")
        odd.mkdir(parents=True)
        ctx.run('odd', [sys.executable, '-c', 'import sys,pathlib;pathlib.Path(sys.argv[1],"ok").write_text("1")', odd])
        self.assertTrue((odd/'ok').is_file())

    def test_the_build_report_gives_each_stage_its_measured_peak(self):
        """D2: a stage's script peak (the stage log's 'peak X GB') is in build-times.json, for re-planning MEMORY."""
        def a(ctx):
            ctx.run('a', [sys.executable, '-c', 'bytearray(64 << 20)'])
        self.assertTrue(self.build([Stage('a', 0, (), (), (), (), a), Stage('b', 0, (), (), (), (), lambda ctx: None)]))
        stages = {t['stage']: t for t in json.loads((self.base/'logs/build-times.json').read_text())['stages']}
        self.assertGreater(stages['a']['peak_gb'], 0.05)
        self.assertNotIn('peak_gb', stages['b'])  # nothing measured: in-process work

    def test_disk_full_inside_a_stage_is_disk_and_old_log_text_is_ignored(self):
        def full(ctx):
            ctx.run('a', [sys.executable, '-c', 'import sys;print("[Errno 28] No space left on device");sys.exit(1)'])
        self.assertFalse(self.build([Stage('a', 0, (), (), (), (), full)]))
        self.assertEqual((self.events[-1]['kind'], self.events[-1]['log']), ('disk', 'logs/a.log'))

        def broken(ctx):
            raise RuntimeError('boom')  # logs/a.log still holds the earlier run's "No space left"
        self.assertFalse(self.build([Stage('a', 0, (), (), (), (), broken)]))
        self.assertEqual(self.events[-1]['kind'], 'failed')

    def main(self, *args, found=None, stages=(), verify=None):
        found = found or {'x': {'status': 'ok', 'path': 'p', 'identity': 'i', 'detail': ''}}
        argv = ['--base', str(self.base), '--game-exe', 'game.exe', *args]
        with mock.patch.object(runner.sources, 'scan', return_value=found), mock.patch.object(runner, 'STAGES', list(stages)), \
                mock.patch.object(runner.sources, 'verify', side_effect=verify or (lambda *a, **k: ['verify stub'])) as self.verify, \
                mock.patch.object(runner, 'stage_fingerprints', lambda cache=None: {}):
            return runner.main(argv)

    def test_verify_rehashes_every_file_without_the_sources_and_the_next_build_repairs(self):
        """#5: a file corrupted in place (same size, same modified time) passes a resume but not --verify."""
        stages = table([])
        self.build(stages)
        missing = {'x': {'status': 'missing', 'path': None, 'identity': None, 'detail': ''}}
        self.main('--verify', stages=stages, found=missing)
        self.assertEqual(self.events[-1], {'event': 'verify', 'ok': True, 'files': 3, 'bad': 0, 'stages': [],
                                           'message': 'All 3 files OK.'})
        out = self.base/'installations/0123456789abcdef0123456789abcdef/assets/private/b/x.txt'
        stat = out.stat()
        out.write_text('B')  # same size
        os.utime(out, ns=(stat.st_atime_ns, stat.st_mtime_ns))  # same time: hashes.json would vouch for it
        runner._hashes.clear()
        self.main('--verify', stages=stages, found=missing)
        result = self.events[-1]
        self.assertEqual((result['ok'], result['bad'], result['stages']), (False, 1, ['b']))
        self.assertIn('b', result['message'])
        log = []
        runner._hashes.clear()
        self.build(table(log))
        self.assertEqual(log, ['b'])  # the resumed build no longer trusts the remembered hash

    def test_an_8_gb_pc_builds_in_low_memory_mode(self):
        """#15: two fan-out workers instead of half the cores, and the fan-out stages planned smaller to match."""
        self.addCleanup(setattr, runner, 'LOW_FANOUT', None)
        for total, fanout, base_gb in ((8, '2', 6 * (0.5 + 0.5 * 2 / 6)), (16, None, 6)):
            with mock.patch.dict(os.environ, {}, clear=False), mock.patch.object(runner, 'memory_gb', return_value=total):
                os.environ.pop('SKATE_FANOUT', None)
                ctx = runner.Context(self.base, Path('game.exe'), {})
                with mock.patch.object(runner, 'STAGES', []), mock.patch.object(runner, 'stage_fingerprints', lambda: {}):
                    runner.build(ctx)
                self.assertEqual((ctx.env.get('SKATE_FANOUT'), os.environ.get('SKATE_FANOUT')), (fanout, fanout))
                self.assertAlmostEqual(runner.peak_gb('skate3-base'), base_gb)
                self.assertEqual(runner.peak_gb('boot'), runner.MEMORY['boot'])  # no fan-out: as measured
        with mock.patch.dict(os.environ, {'SKATE_FANOUT': '3'}), mock.patch.object(runner, 'memory_gb', return_value=8):
            ctx = runner.Context(self.base, Path('game.exe'), {})
            with mock.patch.object(runner, 'STAGES', []), mock.patch.object(runner, 'stage_fingerprints', lambda: {}):
                runner.build(ctx)
            self.assertNotIn('SKATE_FANOUT', ctx.env)  # the player's own setting wins
            self.assertAlmostEqual(runner.peak_gb('skate3-base'), 6 * (0.5 + 0.5 * 3 / 6))

    def test_release_json_carries_each_stage_its_fingerprint_order_and_time(self):
        """U3: the launcher can say what an update rebuilds from release.json alone."""
        from tools.asset_pipeline import versions
        prints = {**{g: g for g in versions.GROUPS}, 'b': 'b-v2'}
        with mock.patch.object(runner, 'STAGES', self.base_table([])), mock.patch.object(versions, 'fingerprints', return_value=prints), \
                mock.patch.dict(runner.WEIGHTS, {'b': 40}):
            stages = runner.release_stages(Path('tools'))
        self.assertEqual(list(stages), ['skate3-base', 'b', 'c'])
        self.assertEqual(stages['skate3-base']['fingerprint'], runner.with_base(prints)['skate3-base'])
        self.assertEqual(stages['b'], {'fingerprint': 'b-v2', 'after': ['skate3-base'], 'seconds': 40})
        self.assertIsNone(stages['c']['fingerprint'])

    def test_the_launch_check_reuses_fingerprints_until_a_tool_file_changes(self):
        """Lightweight launcher (10-13): hashing every tool file took ~1.2 s of each --check; a listing takes ~0.03 s."""
        from tools.asset_pipeline import versions
        cache, files = self.base/'fingerprints.json', {Path('a.py'): (1, 1)}
        with mock.patch.object(versions, 'fingerprints', return_value={'a': 'a-v1'}) as hashing, \
                mock.patch.object(runner, 'listing', side_effect=lambda path: dict(files)):
            first = runner.stage_fingerprints(cache)
            self.assertEqual(runner.stage_fingerprints(cache), first)
            self.assertEqual(hashing.call_count, 1)
            files[Path('a.py')] = (1, 2)  # an update replaced a tool
            runner.stage_fingerprints(cache)
            self.assertEqual(hashing.call_count, 2)
            runner.stage_fingerprints()  # a build never trusts the cache
            self.assertEqual(hashing.call_count, 3)

    def test_different_discs_and_packages_extract_side_by_side(self):
        """B7: a stage waiting for its disc no longer waits for another disc or a package being unpacked."""
        import threading
        meet = threading.Barrier(3, timeout=10)

        class Iso:
            def __init__(self, path): pass
            def __enter__(self): return self
            def __exit__(self, *a): pass
            def extract(self, out):
                meet.wait()
                Path(out).mkdir(parents=True)
        module = types.ModuleType('stfs_extract')
        module.extract = lambda pkg, out: (meet.wait(), Path(out).mkdir(parents=True))
        ctx = runner.Context(self.base, Path('game.exe'), {i: {'path': f'{i}.iso'} for i in ('s1-disc', 's2-disc', 'pkg')})
        with mock.patch('tools.pipeline.xdvdfs.Iso', Iso), mock.patch.dict(sys.modules, {'stfs_extract': module}):
            jobs = [threading.Thread(target=ctx.disc, args=('s1-disc',)), threading.Thread(target=ctx.disc, args=('s2-disc',)),
                    threading.Thread(target=ctx.package, args=('pkg',))]
            for job in jobs:
                job.start()
            for job in jobs:
                job.join(20)
        self.assertFalse(meet.broken, 'the three extractions met: none waited for another')
        self.assertTrue(all((ctx.work/f).is_dir() for f in ('discs/s1-disc', 'discs/s2-disc', 'dlc/pkg')))

    def test_tools_download_while_the_source_files_are_checked(self):
        """#11: a first build's tool downloads no longer wait for the (minutes long) source check."""
        import threading
        (self.base/'installation.json').write_text(json.dumps({'directory': 'installations/0123456789abcdef0123456789abcdef',
                                                                'pipelines': {'a': 'a-v1'}}))
        fetching, order = threading.Event(), []

        def ensure(tools, progress):
            order.append('tools'); fetching.set()
            return {'SKATE_JAVA': 'java.exe'}

        def verify(*args, **kwargs):
            self.assertTrue(fetching.wait(10), 'the downloads started before the check ended')
            order.append('verify')
            return []
        with mock.patch('tools.pipeline.fetch_tools.ensure', ensure), mock.patch.object(runner, 'build', return_value=True) as build:
            self.assertEqual(self.main(verify=verify), 0)
        self.assertEqual(order, ['tools', 'verify'])
        self.assertEqual(build.call_args.args[0].env['SKATE_JAVA'], 'java.exe')

        def broken(tools, progress):
            raise OSError('Downloading java from github.com failed: no network')
        with mock.patch('tools.pipeline.fetch_tools.ensure', broken):
            self.assertEqual(self.main(verify=lambda *a, **k: []), 1)
        self.assertIn('no network', self.events[-1]['message'])

    def test_identify_and_check_events(self):
        self.assertEqual(self.main('--identify'), 0)
        self.assertEqual(self.events, [{'event': 'items', 'items': {'x': {'status': 'ok', 'path': 'p', 'detail': ''}}}])
        self.events.clear()
        self.assertEqual(self.main('--check'), 0)
        self.assertEqual((self.events[-1]['event'], self.events[-1]['state']), ('state', 'play'))
        (self.base/'installation.json').unlink()
        self.main('--check')
        self.assertEqual(self.events[-1]['state'], 'ready')
        self.main('--check', found={'x': {'status': 'missing', 'path': None, 'identity': None, 'detail': ''}})
        self.assertEqual(self.events[-1]['state'], 'missing')

    def test_planned_follows_fingerprints_and_after_without_hashing(self):
        stages = table([])
        marker = {'pipelines': {'a': 'a-v1', 'b': 'b-v1', 'c': 'c-v1'}}
        with mock.patch.object(runner, 'STAGES', stages):
            self.assertEqual(runner.planned(marker, {'a': 'a-v1', 'b': 'b-v2', 'c': 'c-v1'}, {}), ['b', 'c'])
            self.assertEqual(runner.planned(marker, {'a': 'a-v1', 'b': 'b-v1', 'c': 'c-v1'}, {}), [])
            self.assertEqual(runner.planned({}, {'a': 'a-v1', 'b': 'b-v1', 'c': 'c-v1'}, {}), ['a', 'b', 'c'])

    def test_estimate_scales_weights_by_the_last_build_and_sizes_an_update_from_its_receipts(self):
        stages = table([])
        marker = {'pipelines': {'a': 1}, 'outputs': {'stages': {'assets/private/b/x': {'size': 3 << 30},
                                                                'assets/private/a/x': {'size': 9 << 30}}}}
        ctx = runner.Context(self.base, Path('game.exe'), {})
        with mock.patch.object(runner, 'STAGES', stages), mock.patch.dict(runner.WEIGHTS, {'b': 100, 'c': 100}), \
                mock.patch('shutil.disk_usage', return_value=(0, 0, 50 << 30)), \
                mock.patch.object(runner, 'workers', return_value=2), mock.patch.object(runner, 'budget_gb', return_value=8):
            first = runner.estimate(ctx, ['b', 'c'], {})
            self.assertEqual(first['need'], runner.FIRST_BUILD_GB << 30)
            self.assertEqual((first['free'], first['drive']), (50 << 30, self.base.resolve().drive))
            self.assertGreaterEqual(first['min'], 200)  # b then c, plus checking the source files
            update = runner.estimate(ctx, ['b', 'c'], marker)
            self.assertEqual(update['need'], (3 << 30) + runner.MARGIN)  # b's roots again, not the whole install
            self.assertEqual((update['min'], update['max'] > update['min']), (200, True))
            (self.base/'logs').mkdir()
            (self.base/'logs/build-times.json').write_text(json.dumps({'stages': [
                {'stage': 'b', 'seconds': 200, 'ok': True}, {'stage': 'c', 'seconds': 200, 'ok': True}]}))
            self.assertEqual(runner.estimate(ctx, ['b', 'c'], marker)['min'], round(400 * 0.85))  # this PC takes twice

    def test_every_build_ends_by_storing_the_storage_sizes(self):
        (self.base/'work/discs/s1-disc').mkdir(parents=True); (self.base/'work/discs/s1-disc/f').write_bytes(b'x' * 10)
        self.assertFalse(self.build(table([], fail='c')))
        sizes = json.loads((self.base/runner.STORAGE).read_text())
        self.assertEqual(sizes['leftovers'], 10)  # a failed build's extracted disc
        self.assertGreater(sizes['logs'], 0)
        self.assertTrue(self.build(table([])))
        sizes = json.loads((self.base/runner.STORAGE).read_text())
        self.assertFalse((self.base/'work/discs').exists())  # a finished build moved it into data/trash (B8) ...
        self.assertEqual(sizes['leftovers'], 10)  # ... which counts until the launcher deletes it
        self.assertEqual(sizes['game'], 3)  # the receipts: a, b and c's 1-byte x.txt
        self.assertEqual(sizes['rebuild'], 3)  # what a Rebuild rewrites, for the launch check in play

    def test_the_launch_check_in_play_reads_no_receipt(self):
        # end review: 51k stage receipts x 38 stages' roots took ~1 s on every launcher open
        marker = json.loads((self.base/'installation.json').read_text())
        (self.base/'installation.json').write_text(json.dumps({**marker, 'pipelines': {'a': 'a-v1'}}))  # built before
        self.assertEqual(self.main('--check'), 0)
        self.assertEqual(self.events[-1]['state'], 'play')
        runner.atomic_json(self.base/runner.STORAGE, {'game': 7, 'rebuild': 5 << 30})
        with mock.patch.object(runner, 'owns', side_effect=AssertionError('walked the receipts')),                 mock.patch.object(runner, 'rewrites', side_effect=AssertionError('walked the receipts')):
            self.main('--check')
        state = self.events[-1]
        self.assertEqual((state['state'], state['estimate']['need']), ('play', 5 << 30))  # (an empty table: no margin)

    def test_the_build_report_times_what_runs_outside_the_stages(self):
        self.assertTrue(self.build(table([])))
        times = json.loads((self.base/'logs/build-times.json').read_text())
        self.assertEqual(sorted(p for p in times['phases'] if not p.startswith('record ')), ['check clothing set', 'check fingerprints', 'check hashes.json',
                                                   'check stage receipts', 'cleanup', 'seal'])  # the 0:49's parts too
        self.assertIn('Outside the steps: check fingerprints 0:00', times['report'])
        self.assertIn(' · seal ', times['report'])

    def test_the_build_report_times_each_stage_record(self):
        """M14: the output hashing each stage's dependents wait for is in build-times.json."""
        self.assertTrue(self.build(table([])))
        phases = json.loads((self.base/'logs/build-times.json').read_text())['phases']
        self.assertTrue({'record a', 'record b', 'record c'} <= set(phases), phases)

    def test_a_build_starts_with_fresh_logs_and_an_empty_temp_folder(self):
        for name, text in (('logs/a.log', 'last build'), ('logs.previous/a.log', 'the build before'), ('tmp/s3r-fanout-x/f', 'x')):
            (self.base/name).parent.mkdir(parents=True, exist_ok=True); (self.base/name).write_text(text)
        before = runner.tempfile.tempdir
        self.assertEqual(self.main(), 2)  # the (stubbed) source check fails after the fresh start
        self.assertEqual((self.base/'logs.previous/a.log').read_text(), 'last build')  # one earlier build is kept
        self.assertFalse((self.base/'logs/a.log').exists())
        self.assertEqual(list((self.base/'tmp').iterdir()), [])  # emptied at the start and at the end
        self.assertEqual(runner.tempfile.tempdir, before)
        ctx = runner.Context(self.base, Path('game.exe'), {})
        runner.fresh_start(ctx)
        self.addCleanup(setattr, runner.tempfile, 'tempdir', before)
        self.assertEqual((ctx.env['TMP'], ctx.env['TEMP'], runner.tempfile.tempdir), (str(self.base/'tmp'),) * 3)

    def test_a_pc_without_long_paths_is_told_before_the_build_starts(self):
        # end review (#3): installed files reach ~314 characters below the installation folder (Create a Skater
        # thumbnails), so every build passes 260 whatever the base; with Windows' long paths off it fails mid-build
        self.long_paths.return_value = False
        self.main('--check')
        self.assertIs(self.events[-1]['long_paths'], False)
        self.assertEqual(self.main(), 2)
        self.assertEqual((self.events[-1]['event'], self.events[-1]['kind']), ('error', 'failed'))
        self.assertIn('LongPathsEnabled', self.events[-1]['message'])
        self.verify.assert_not_called()  # before minutes of source checking
        self.long_paths.return_value = True
        self.main('--check')
        self.assertIs(self.events[-1]['long_paths'], True)

    def test_a_hard_drive_is_read_one_file_at_a_time(self):
        """#10: parallel reads make a hard drive's head seek back and forth."""
        (self.base/'installation.json').write_text(json.dumps({'directory': 'installations/0123456789abcdef0123456789abcdef',
                                                                'pipelines': {'a': 'a-v1'}}))  # an update: no 45 GB check
        found = {'x': {'status': 'ok', 'path': 'E:/sources/x.iso', 'identity': 'i', 'detail': ''}}
        self.drive.side_effect = lambda drive: 'HDD USB' if drive.upper().startswith('E') else 'SSD NVMe'
        self.main(found=found)
        self.assertEqual((self.verify.call_args.kwargs['threads'], runner.HASHERS), (1, 8))  # sources on E:, game on an SSD
        self.drive.side_effect = lambda drive: 'HDD SATA'
        self.main(found=found)
        self.assertEqual((self.verify.call_args.kwargs['threads'], runner.HASHERS), (1, 1))

    def test_check_reports_the_estimate_and_the_cached_sizes(self):
        (self.base/'installation.json').unlink()
        runner.atomic_json(self.base/runner.STORAGE, {'logs': 5, 'game': 7})
        with mock.patch('shutil.disk_usage', return_value=(0, 0, 99 << 30)):
            self.main('--check')
        state = self.events[-1]
        self.assertEqual(state['state'], 'ready')
        self.assertEqual(state['estimate']['need'], runner.FIRST_BUILD_GB << 30)
        self.assertEqual(state['sizes'], {'logs': 5, 'game': 7, 'free': 99 << 30, 'drive': self.base.resolve().drive})

    def test_probe_says_what_an_update_rebuilds_and_what_blocks_it(self):
        from tools.asset_pipeline import versions
        release = Path(self.temp.name, 'release.json')
        groups = {g: 'same' for g in versions.GROUPS}
        prints = runner.with_base(groups)
        (self.base/'installation.json').write_text(json.dumps({'directory': 'installations/0123456789abcdef0123456789abcdef',
                                                                'pipelines': {**prints, 'x': 'x-v1'}}))
        release.write_text(json.dumps({'asset_pipelines': {**groups, 'x': 'x-v1'}}))
        stage = Stage('x', 6, ('s1',), ('skate3-base',), ('assets/private/x',), (), None)
        base = Stage('skate3-base', 1, ('s1',), (), (), (), None)
        missing = {'s1': {'status': 'missing', 'path': None, 'identity': None, 'detail': ''}}
        run = lambda *args, **kw: self.main(*args, stages=[base, stage], **kw)
        run('--probe', str(release), found=missing)
        self.assertEqual(self.events[-1]['event'], 'probe')
        self.assertEqual((self.events[-1]['rebuild']['steps'], self.events[-1]['blocked']), (0, None))  # nothing to build
        release.write_text(json.dumps({'asset_pipelines': {**groups, 'x': 'x-v2'}}))
        run('--probe', str(release), '--sources', 'D:/Skate sources', found=missing)
        probe = self.events[-1]
        self.assertEqual((probe['rebuild']['steps'], probe['rebuild']['full']), (1, False))
        self.assertIn('Skate sources', probe['blocked'])
        release.write_text(json.dumps({'asset_pipelines': {**groups, 'hud': 'new', 'x': 'x-v1'}}))
        with mock.patch('shutil.disk_usage', return_value=(0, 0, 1 << 20)):
            run('--probe', str(release))  # sources fine, disk not
        probe = self.events[-1]
        self.assertEqual((probe['rebuild']['steps'], probe['rebuild']['full']), (2, True))
        self.assertIn('space', probe['blocked'])

    def test_build_refuses_when_sources_are_not_all_ok(self):
        found = {'x': {'status': 'mismatch', 'path': 'p', 'identity': None, 'detail': 'bad'}}
        self.assertEqual(self.main(found=found), 2)
        self.assertEqual((self.events[-1]['event'], self.events[-1]['kind']), ('error', 'sources'))

    # ---- fix round 1 ----
    def complete(self, stages, prints):
        with mock.patch.object(runner, 'STAGES', stages), mock.patch.object(runner, 'stage_fingerprints', lambda: prints):
            return runner.complete(runner.Context(self.base, Path('game.exe'), {}))

    def test_complete_needs_every_fingerprint_every_receipt_and_no_damage(self):
        log = []; stages = table(log); good = {'a': 'a-v1', 'b': 'b-v1', 'c': 'c-v1'}
        self.assertFalse(self.complete(stages, {}))  # nothing built and no fingerprints: None == None is not "done"
        self.build(stages)
        self.assertTrue(self.complete(stages, good))  # a real stage table, built and sealed
        self.assertFalse(self.complete(stages, {**good, 'b': 'b-v2'}))  # (a) mismatched fingerprint
        self.assertFalse(self.complete(stages, {'a': 'a-v1', 'c': 'c-v1'}))  # (b) missing fingerprint
        marker = json.loads((self.base/'installation.json').read_text())
        self.assertIn('stages', marker['outputs'])
        (self.base/'installation.json').write_text(json.dumps({**marker, 'outputs': {}}))
        self.assertFalse(self.complete(stages, good))  # (d) matching pipelines but no receipt for the stages
        (self.base/'installation.json').write_text(json.dumps(marker))
        self.assertTrue(self.complete(stages, good))
        damaged = self.base/'installations/0123456789abcdef0123456789abcdef/assets/private/c/x.txt'
        good_bytes = damaged.read_bytes(); size = len(good_bytes)
        damaged.write_bytes(bytes(b ^ 1 for b in good_bytes))  # different content, same size
        self.assertTrue(self.complete(stages, good))  # same size: the launcher's quick check passes; a build's hash check catches it
        with mock.patch.object(runner, 'STAGES', stages):
            self.assertEqual(runner.damaged_stages(runner.Context(self.base, Path('game.exe'), {}), marker), {'c'})
        damaged.write_text('z' * (size + 1))
        self.assertFalse(self.complete(stages, good))  # (c) damaged output

    def test_the_launch_check_reads_installation_json_once_not_once_per_receipt(self):
        self.build(table([]))
        marker = json.loads((self.base/'installation.json').read_text())
        marker['outputs']['stages'] = {f'assets/private/a/{i}.txt': {'size': 1, 'sha256': ''} for i in range(200)}
        reads, real = [], runner.setup_state.read_marker
        with mock.patch.object(runner.setup_state, 'read_marker', lambda path: reads.append(1) or real(path)):
            ctx = runner.Context(self.base, Path('game.exe'), {})
            self.assertEqual(runner.damaged_stages(ctx, marker, {name: 1 for name in marker['outputs']['stages']}), set())
        self.assertEqual(len(reads), 1)  # each read is a 23 MB parse on a real install: 51,146 of them took hours

    def test_a_finished_build_moves_old_installations_and_clothing_sets_to_the_trash_before_done(self):
        self.build(table([]))
        install = self.base/'installations/0123456789abcdef0123456789abcdef'
        old = self.base/'installations/fedcba9876543210fedcba9876543210'
        (old/'maps').mkdir(parents=True); (old/'maps/x.bin').write_text('x')
        sets = install/'assets/private/customisation/sets'
        for name in 'abc':
            (sets/(name*32)).mkdir(parents=True); (sets/(name*32)/'f.json').write_text('{}')
        (sets.parent/'current.json').write_text(json.dumps({'set': 'b'*32}))
        (sets.parent/'pending.json').write_text(json.dumps({'set': 'c'*32}))  # a generation the next build may finish
        marker = json.loads((self.base/'installation.json').read_text())
        marker['outputs']['stages'][f'assets/private/customisation/sets/{"a"*32}/f.json'] = {'size': 2, 'sha256': ''}
        (self.base/'installation.json').write_text(json.dumps(marker))
        runner._hashes[(str((old/'maps/x.bin').resolve()), 1, 1)] = 'dead'
        log, seen = [], []
        with mock.patch.object(runner, 'emit', lambda event, **f: seen.append((event, old.exists()))):
            self.assertTrue(self.build(table(log)))
        self.assertEqual(log, [])  # nothing rebuilt
        self.assertIn(('done', False), seen)  # B8: moved before "done", so the tool has truly finished by then
        self.assertFalse(old.exists() or (sets/('a'*32)).exists())
        trash = sorted(p.name.split('-', 1)[1] for p in (self.base/runner.TRASH).iterdir())
        self.assertEqual(trash, ['a'*32, 'fedcba9876543210fedcba9876543210'])  # the launcher deletes them, no lock held
        self.assertTrue((sets/('b'*32)/'f.json').is_file() and (sets/('c'*32)/'f.json').is_file())
        self.assertTrue((install/'assets/private/a/x.txt').is_file())
        marker = json.loads((self.base/'installation.json').read_text())
        self.assertFalse([n for n in marker['outputs']['stages'] if 'aaaa' in n])
        self.assertTrue(marker['sealed'])
        self.assertNotIn('fedcba', (self.base/runner.HASHES).read_text())

    def test_an_old_installation_with_newer_saves_is_kept(self):
        """F2 interim: saves made in an old installation during a build (or a lost installation.json) are never deleted."""
        self.build(table([]))
        install = self.base/'installations/0123456789abcdef0123456789abcdef'
        (install/'settings').mkdir(); (install/'settings/profile.json').write_text('new')
        old = self.base/'installations/fedcba9876543210fedcba9876543210'
        (old/'settings').mkdir(parents=True); (old/'settings/profile.json').write_text('played during the build')
        later = (install/'settings/profile.json').stat().st_mtime_ns + 10**9
        os.utime(old/'settings/profile.json', ns=(later, later))
        older = self.base/'installations/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
        (older/'settings').mkdir(parents=True); (older/'settings/profile.json').write_text('old')
        os.utime(older/'settings/profile.json', ns=(later - 2 * 10**9, later - 2 * 10**9))
        self.assertTrue(self.build(table([])))
        self.assertTrue((old/'settings/profile.json').is_file())
        self.assertFalse(older.exists())
        (install/'settings').rename(install/'gone')  # a fresh install with no saves: any old saves are newer
        old2 = self.base/'installations/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
        (old2/'settings').mkdir(parents=True); (old2/'settings/x').write_text('x')
        self.assertTrue(self.build(table([])))
        self.assertTrue(old2.exists())
        (self.base/'settings').mkdir(); (self.base/'settings/profile.json').write_text('migrated')  # F2: saves moved out
        os.utime(self.base/'settings/profile.json', ns=(later + 10**9, later + 10**9))
        self.assertTrue(self.build(table([])))
        self.assertFalse(old.exists() or old2.exists())

    def test_the_build_written_default_map_is_not_a_save(self):
        """R's follow-up: setup writes settings/default-map.json into every new installation; it must not make a fresh
        installation look newer than an old one's real saves."""
        self.build(table([]))
        install = self.base/'installations/0123456789abcdef0123456789abcdef'
        old = self.base/'installations/cccccccccccccccccccccccccccccccc'
        (old/'settings').mkdir(parents=True); (old/'settings/profile.json').write_text('real save')
        (install/'settings').mkdir(exist_ok=True); (install/'settings/default-map.json').write_text('"maps/x.skate"')
        later = (old/'settings/profile.json').stat().st_mtime_ns + 10**9
        os.utime(install/'settings/default-map.json', ns=(later, later))  # written by this build, after the save
        self.assertTrue(self.build(table([])))
        self.assertTrue((old/'settings/profile.json').is_file())

    def test_a_stopped_builds_half_converted_map_is_cleared_before_the_next(self):
        self.build(table([]))
        leftover = self.base/'installations/0123456789abcdef0123456789abcdef/conversion/DIST_MaloofMoneyCupDLC/raw/t.rx2'
        leftover.parent.mkdir(parents=True); leftover.write_text('half')
        self.assertTrue(self.build(table([])))
        self.assertFalse(leftover.parent.parent.parent.exists())

    def test_a_stale_clothing_set_is_remade_and_the_wardrobe_rebuilt(self):
        log, made = [], []
        stages = [Stage('dlc-clothing', 0, (), (), ('assets/private/w',), ('pipeline/runner.py',), table(log)[0].run),
                  Stage('old-clothing', 1, (), ('dlc-clothing',), ('assets/private/o',), ('pipeline/runner.py',), table(log)[1].run),
                  Stage('maps', 2, (), (), ('assets/private/m',), ('pipeline/runner.py',), table(log)[2].run)]
        prints = {s.name: s.name for s in stages}
        self.assertTrue(self.build(stages, prints)); log.clear()
        current = self.base/'installations/0123456789abcdef0123456789abcdef'/runner.CUSTOMISATION/'current.json'
        current.parent.mkdir(parents=True)
        current.write_text(json.dumps({'version': 1, 'set': 'a' * 32, 'fingerprint': 'build-5'}))
        self.customiser.stop()  # the real check from here on
        with mock.patch('tools.asset_pipeline.customiser_setup.fingerprint', return_value='build-6'), \
                mock.patch.object(runner, 'new_customiser', lambda ctx: made.append(1) or current.write_text(
                    json.dumps({'version': 1, 'set': 'b' * 32, 'fingerprint': 'build-6'}))):
            self.assertFalse(self.complete(stages, prints))  # the launcher says Build, not Play
            self.assertTrue(self.build(stages, prints))
            self.assertEqual((made, log), ([1], ['a', 'b']))  # one new set, then dlc- and old-clothing (table's a, b); maps untouched
            self.assertTrue(self.complete(stages, prints))
            log.clear(); self.assertTrue(self.build(stages, prints))
            self.assertEqual((made, log), ([1], []))  # current now: nothing more

    def test_stage_without_a_fingerprint_always_runs(self):
        log = []; prints = {'a': 'a-v1'}
        self.build(table(log), prints); log.clear()
        self.build(table(log), prints)
        self.assertEqual(log, ['b', 'c'])

    def test_resume_after_a_dependent_failed_reruns_only_what_is_stale(self):
        log = []; self.build(table(log)); log.clear()
        prints = {'a': 'a-v2', 'b': 'b-v1', 'c': 'c-v1'}
        self.assertFalse(self.build(table(log, fail='b', text={'a': 'a2'}), prints))
        self.assertEqual(log, ['a', 'b'])
        log.clear()
        self.assertTrue(self.build(table(log, text={'a': 'a2'}), prints))
        self.assertEqual(log, ['b', 'c'])  # a's new output is already receipted; b and c follow it

    def test_damage_is_found_after_a_failed_first_build(self):
        log = []
        self.assertFalse(self.build(table(log, fail='c')))
        (self.base/'installations/0123456789abcdef0123456789abcdef/assets/private/a/x.txt').unlink()
        log.clear()
        self.assertTrue(self.build(table(log)))
        self.assertEqual(log, ['a', 'c'])  # a's file back as before: b is still current (#4); c never finished

    def test_first_build_disk_check_follows_the_receipts_not_the_marker_file(self):
        with mock.patch('shutil.disk_usage', return_value=(0, 0, 30 << 30)):
            self.assertEqual(self.main(), 2)  # pipelines is empty: a first build needs 45 GB
            self.assertEqual((self.events[-1]['event'], self.events[-1]['kind']), ('error', 'disk'))
            self.assertIn('45 GB', self.events[-1]['message'])
            self.events.clear()
            (self.base/'installation.json').write_text(json.dumps(
                {'directory': 'installations/0123456789abcdef0123456789abcdef', 'pipelines': {'a': 'a-v1'}}))
            self.assertEqual(self.main(), 2)  # an update needs only 15 GB: on to the (stubbed) source check
            self.assertEqual(self.events[-1]['kind'], 'sources')

    def test_errors_name_the_step_in_plain_words(self):
        def failing(text):
            def run(ctx):
                ctx.run('mod-maps', [sys.executable, '-c', f'import sys;print({text!r});sys.exit(1)'])
            return [Stage('mod-maps', 0, (), (), (), (), run)]
        cases = {"OSError: [WinError 112] Auf dem Datenträger ist nicht genug Speicherplatz.": ('disk', 'Not enough disk space.'),
                 "OSError: [WinError 39] Le disque est plein.": ('disk', 'Not enough disk space.'),
                 'MemoryError': ('failed', 'New San Vanelona failed: the PC ran out of memory'),
                 "PermissionError: [WinError 32] The process cannot access the file": ('failed', 'New San Vanelona failed: a file is in use'),
                 "FileNotFoundError: [WinError 206] The filename or extension is too long": ('failed', 'New San Vanelona failed: a path is too long'),
                 'boom': ('failed', 'New San Vanelona failed: a build step exited with code 1')}
        for text, (kind, start) in cases.items():
            self.events.clear()
            self.assertFalse(self.build(failing(text)))
            self.assertEqual(self.events[-1]['kind'], kind, text)
            self.assertTrue(self.events[-1]['message'].startswith(start), self.events[-1]['message'])

    def test_log_write_failure_still_emits_the_error_event(self):
        def boom(ctx):
            raise RuntimeError('boom')
        with mock.patch.object(runner, 'open', side_effect=OSError(28, 'No space left on device'), create=True):
            self.assertFalse(self.build([Stage('a', 0, (), (), (), (), boom)]))
        self.assertEqual((self.events[-1]['event'], self.events[-1]['kind']), ('error', 'failed'))
        self.assertIn('boom', self.events[-1]['message'])

    def test_unreadable_marker_is_an_error_event_not_a_traceback(self):
        (self.base/'installation.json').write_text('{not json')
        for args in (('--check',), ()):
            self.events.clear()
            self.assertEqual(self.main(*args), 1)
            self.assertEqual((self.events[-1]['event'], self.events[-1]['kind']), ('error', 'failed'))
            self.assertEqual(self.events[-1]['stage'], None)

    def fake_extractor(self):
        def extract(pkg, out):
            print('extracting file 1 of 2')
            Path(out).mkdir(parents=True)
        module = types.ModuleType('stfs_extract'); module.extract = extract
        p = mock.patch.dict(sys.modules, {'stfs_extract': module}); p.start(); self.addCleanup(p.stop)
        return runner.Context(self.base, Path('game.exe'), {'x': {'path': 'x.pkg'}, 'y': {'path': 'y.pkg'}})

    def test_in_process_extractor_output_stays_off_the_event_stream(self):
        ctx = self.fake_extractor(); ctx.stage = 'a'
        seen = io.StringIO()
        with contextlib.redirect_stdout(seen):
            ctx.package('x')
        self.assertEqual(seen.getvalue(), '')  # stdout carries JSON events only
        self.assertIn('extracting file 1 of 2', (ctx.logs/'a.log').read_text())

    def test_package_adds_its_import_folder_to_sys_path_once(self):
        entry = str(runner.TOOLS/'pipeline/dlc')
        saved = list(sys.path); self.addCleanup(lambda: sys.path.__setitem__(slice(None), saved))
        sys.path[:] = [p for p in sys.path if p != entry]
        ctx = self.fake_extractor()
        with contextlib.redirect_stdout(io.StringIO()):
            ctx.package('x'); ctx.package('y')
        self.assertEqual(sys.path.count(entry), 1)

    def test_unknown_stage_is_refused_before_any_work(self):
        self.assertEqual(self.main('--stage', 'nope'), 2)
        self.assertEqual(self.events, [{'event': 'error', 'stage': None, 'kind': 'failed', 'log': None,
                                        'message': 'unknown stage nope'}])

    # ---- Task 11 ----
    def test_skate3_base_fingerprint_is_the_hash_of_the_five_groups(self):
        from tools.asset_pipeline import versions
        groups = {g: g + '-print' for g in versions.GROUPS}
        with mock.patch.object(versions, 'fingerprints', return_value={**groups, 'a': 'a-print'}):
            prints = runner.stage_fingerprints()
        lines = ''.join(f'{g}={groups[g]}\n' for g in sorted(groups))  # sorted key=value lines
        self.assertEqual(prints, {**groups, 'a': 'a-print', 'skate3-base': hashlib.sha256(lines.encode()).hexdigest()})
        with mock.patch.object(versions, 'fingerprints', return_value={**groups, 'hud': 'changed'}):
            self.assertNotEqual(runner.stage_fingerprints()['skate3-base'], prints['skate3-base'])

    def base_table(self, log, fail=None):
        a, b, c = table(log, fail)
        return [dataclasses.replace(a, name='skate3-base'), dataclasses.replace(b, after=('skate3-base',)), c]

    def marker(self):
        return json.loads((self.base/'installation.json').read_text())

    def test_sealed_marker_with_damaged_groups_is_not_complete_and_reruns_base_and_dependents(self):
        log = []; stages = self.base_table(log); prints = {'skate3-base': 'a-v1', 'b': 'b-v1', 'c': 'c-v1'}
        self.build(stages, prints)
        self.assertIs(self.marker()['sealed'], True)
        self.assertNotIn('sealed', self.marker().get('outputs', {}))  # the game checks every value under outputs as a receipt
        self.assertTrue(self.complete(stages, prints))
        self.group_damage.return_value = {'hud'}
        self.assertFalse(self.complete(stages, prints))
        log.clear(); self.build(stages, prints)
        self.assertEqual(log, ['a', 'b', 'c'])  # skate3-base (refresh) and everything after it
        self.group_damage.return_value = set()
        self.assertTrue(self.complete(stages, prints))

    def test_unsealed_marker_skips_the_group_check(self):
        log = []; stages = self.base_table(log); prints = {'skate3-base': 'a-v1', 'b': 'b-v1', 'c': 'c-v1'}
        self.build(stages, prints)
        marker = self.marker(); del marker['sealed']
        (self.base/'installation.json').write_text(json.dumps(marker))
        self.group_damage.return_value = {'hud'}
        self.assertTrue(self.complete(stages, prints))  # mid-build: setup's groups are not yet re-recorded
        log.clear(); self.build(stages, prints)
        self.assertEqual(log, [])
        # a stage that finishes unseals the marker: a failed rebuild leaves it unsealed, and the retry skips the group check
        self.group_damage.return_value = set()
        newer = {**prints, 'skate3-base': 'a-v2'}
        self.assertFalse(self.build(self.base_table(log, fail='b'), newer))
        self.assertNotIn('sealed', self.marker())
        self.group_damage.return_value = {'hud'}
        log.clear(); self.assertTrue(self.build(self.base_table(log), newer))
        self.assertEqual(log, ['b', 'c'])  # skate3-base was finished, and not forced again
        self.assertIs(self.marker()['sealed'], True)

    def side_by_side(self, root_b='assets/private/b'):
        """a, then b and c (both after a only); b and c each wait up to 10 s for the other to be running too."""
        import threading
        here, together = {'b': threading.Event(), 'c': threading.Event()}, []

        def meet(name):
            def run(ctx):
                here[name].set()
                together.append(here['c' if name == 'b' else 'b'].wait(timeout=10))  # the other one is running too
                out = ctx.install/'assets/private'/name
                out.mkdir(parents=True, exist_ok=True)
                (out/'x.txt').write_text(name)
            return run
        stages = [Stage('a', 1, (), (), ('assets/private/a',), (), lambda ctx: None),
                  Stage('b', 2, (), ('a',), (root_b,), (), meet('b')),
                  Stage('c', 3, (), ('a',), ('assets/private/c',), (), meet('c'))]
        with mock.patch.dict('os.environ', {'SKATE_BUILD_JOBS': '2'}):
            self.assertTrue(self.build(stages))
        return together

    def test_independent_stages_run_side_by_side(self):
        self.assertEqual(self.side_by_side(), [True, True])
        last = [e for e in self.events if e['event'] == 'progress'][-1]
        self.assertEqual((last['percent'], last['eta'], last['steps']), (100.0, 0, [1.0] * runner.STEPS))
        times = json.loads((self.base/'logs/build-times.json').read_text())
        self.assertTrue(times['ok'])
        self.assertEqual(sorted(t['stage'] for t in times['stages']), ['a', 'b', 'c'])
        self.assertTrue(times['report'].startswith('Built in '), times['report'])

    def test_time_left_runs_stages_one_at_a_time_when_memory_is_short(self):
        todo = [Stage('a', 1, (), (), ('assets/private/a',), (), None), Stage('b', 2, (), (), ('assets/private/b',), (), None)]
        with mock.patch.dict(runner.WEIGHTS, {'a': 100, 'b': 100}), mock.patch.dict(runner.MEMORY, {'a': 3, 'b': 3}):
            self.assertEqual(runner.Progress(todo, {'a': set(), 'b': set()}, 2, budget=8).now()['eta'], 100)  # side by side
            self.assertEqual(runner.Progress(todo, {'a': set(), 'b': set()}, 2, budget=4).now()['eta'], 200)  # in turn

    def test_a_stage_holding_for_its_record_is_done_for_the_bar_time_left_and_report(self):
        todo = [Stage('a', 1, (), (), (), (), None), Stage('b', 2, (), (), (), (), None)]
        clock = [0.0]
        with mock.patch.dict(runner.WEIGHTS, {'a': 200, 'b': 40}), mock.patch.object(runner.time, 'monotonic', lambda: clock[0]):
            bar = runner.Progress(todo, {'a': set(), 'b': set()}, 2)
            bar.start('a'); bar.start('b')
            clock[0] = 10; bar.hold('b')  # tables, done in 10 s, waiting for skate3-base's install
            clock[0] = 150
            now = bar.now()
            self.assertLess(now['eta'], 100)  # not a slow PC: b's wait is not 110 s past its 40
            self.assertEqual((now['steps'][2], now['text']), (1.0, 'a'))
            bar.finish('b')
        self.assertEqual(bar.timeline[-1]['seconds'], 10)

    def test_stages_writing_the_same_folder_never_overlap(self):
        self.assertEqual(self.side_by_side(root_b='assets/private/c/more'), [False, True])  # the second found the first done

    def test_the_wardrobe_stages_take_turns_with_their_dlc_folder_and_the_maps_run_beside_everything(self):
        def stage(name):
            return next(s for s in runner.STAGES if s.name == name)
        self.assertTrue(runner.overlaps(stage('dlc-clothing'), stage('old-clothing')))
        for a, b in (('s3-dlc-maps', 'old-clothing'), ('s3-dlc-maps', 'mod-maps'), ('mod-maps', 'skate1-map'), ('mod-maps', 'skate2-parks'),
                     ('s3-dlc-maps', 'skate1-map'), ('s3-dlc-maps', 'skate2-tutorial')):
            self.assertFalse(runner.overlaps(stage(a), stage(b)), (a, b))

    def test_stages_start_while_their_memory_fits_and_one_always_runs(self):
        big, small = types.SimpleNamespace(name='mod-maps'), types.SimpleNamespace(name='anything-else')
        with mock.patch.dict(runner.MEMORY, {'mod-maps': 5}, clear=True):
            self.assertTrue(runner.fits(big, [], budget=1))  # alone, even over the budget
            self.assertTrue(runner.fits(small, [big], budget=6))
            self.assertFalse(runner.fits(big, [small, small], budget=6.5))
            self.assertFalse(runner.fits(small, [big, small], budget=6.5))
            with mock.patch.dict(runner.EXTRA, {'skate1-map': 2}):  # a load check's brief peak holds new stages back
                self.assertFalse(runner.fits(small, [big], budget=6.5))

    def test_a_held_stage_starts_when_enough_running_stages_end_with_what_is_left_beside_it(self):
        maps, parks = types.SimpleNamespace(name='mod-maps'), types.SimpleNamespace(name='skate2-parks')
        clothes = types.SimpleNamespace(name='old-clothing')
        with mock.patch.dict(runner.MEMORY, {'mod-maps': 3.5, 'skate2-parks': 1.5, 'old-clothing': 4.5}, clear=True),                 mock.patch.dict(runner.WEIGHTS, {'mod-maps': 300, 'skate2-parks': 100}, clear=True),                 mock.patch.object(runner.time, 'monotonic', return_value=1000):
            # parks end at 1100 (frees 1.5: 3.5 + 4.5 > 6.5 still), New San Vanelona at 1300: then 2 GB beside it
            self.assertEqual(runner.reservation(clothes, [maps, parks], {'mod-maps': 1000, 'skate2-parks': 1000}, 6.5),
                             (1300, 2.0))
            self.assertEqual(runner.reservation(clothes, [maps], {'mod-maps': 0}, 6.5), (1000, 2.0))  # overdue: now
            self.assertEqual(runner.reservation(clothes, [maps], {'mod-maps': 1000}, 4), (1300, 0.0))  # then alone

    def test_the_budget_is_what_was_free_at_the_start(self):
        with mock.patch.object(runner, 'memory_gb', return_value=16),                 mock.patch('tools.pipeline.fanout.available_gb', return_value=8.5):
            self.assertEqual(runner.budget_gb(), 7.5)
        with mock.patch.object(runner, 'memory_gb', return_value=16),                 mock.patch('tools.pipeline.fanout.available_gb', return_value=15):
            self.assertEqual(runner.budget_gb(), 12.5)

    def test_the_map_catalog_keeps_one_order_however_the_map_stages_finish(self):
        from tools.pipeline import stages
        with tempfile.TemporaryDirectory() as temp:
            install = Path(temp)
            (install/'maps').mkdir()
            (install/'maps.json').write_text(json.dumps([{'name': 'University'}, {'name': 'Downtown'}]), encoding='utf-8')
            ctx = types.SimpleNamespace(install=install, shared=runner._MARKER)
            for name in ('Skate_1_San_Vanelona', 'Skate_2_School', 'Maloof_Money_Cup_NYC', 'Skate_2_School'):
                (install/'maps'/f'{name}.skate').write_bytes(name.encode())
                stages.catalog_map(ctx, name)
            self.assertEqual([m['name'] for m in json.loads((install/'maps.json').read_text(encoding='utf-8'))],
                             ['University', 'Downtown', 'Maloof_Money_Cup_NYC', 'Skate_2_School', 'Skate_1_San_Vanelona'])

    def test_a_failure_stops_the_stage_running_beside_it_and_is_reported_once(self):
        started = time.monotonic()

        def slow(ctx):
            ctx.run('slow', [sys.executable, '-c', 'import time; time.sleep(30)'])

        def broken(ctx):
            time.sleep(1)  # the slow script is running by now
            raise RuntimeError('boom')
        stages = [Stage('slow', 1, (), (), ('assets/private/slow',), (), slow),
                  Stage('broken', 2, (), (), ('assets/private/broken',), (), broken)]
        with mock.patch.dict('os.environ', {'SKATE_BUILD_JOBS': '2'}):
            self.assertFalse(self.build(stages))
        self.assertLess(time.monotonic() - started, 20, 'the slow script was stopped, not waited for')
        errors = [e for e in self.events if e['event'] == 'error']
        self.assertEqual([e['stage'] for e in errors], ['broken'])
        self.assertNotIn('slow', self.marker().get('pipelines', {}))

    def test_a_resumed_build_starts_its_bar_where_the_last_one_stopped(self):
        todo = [Stage('a', 1, (), (), (), (), None), Stage('b', 2, (), ('a',), (), (), None)]
        with mock.patch.dict(runner.WEIGHTS, {'a': 300, 'b': 100}):
            now = runner.Progress(todo, {'b': set()}, 2, finished=['a']).now()
        self.assertEqual((now['percent'], now['steps'][1], now['eta']), (75.0, 1.0, 100))

    def test_a_second_build_on_the_same_folder_is_refused(self):
        with runner.build_lock(self.base) as first:
            self.assertTrue(first)
            out = __import__('subprocess').run([sys.executable, '-c', 'import sys; from pathlib import Path; '
                                                'from tools.pipeline import runner; '
                                                'print(runner.build_lock(Path(sys.argv[1])).__enter__())', str(self.base)],
                                               capture_output=True, text=True, cwd=Path(runner.__file__).parents[2])
            self.assertEqual(out.stdout.strip(), 'False', out.stderr)
        with runner.build_lock(self.base) as again:
            self.assertTrue(again, 'released when the build ends')

    def test_a_closed_launcher_stops_the_build(self):
        class Gone:
            def write(self, text): raise OSError(22, 'Invalid argument')
            def flush(self): pass
        stopped = []
        ctx = types.SimpleNamespace(cancel=lambda: stopped.append(True))
        real = self._real_emit
        with mock.patch.object(runner, '_OUT', Gone()), mock.patch.object(runner, '_LIVE', [ctx]),                 mock.patch.object(runner.os, '_exit', side_effect=SystemExit(3)):
            with self.assertRaises(SystemExit):
                real('progress', percent=50)
        self.assertEqual(stopped, [True])

    def test_progress_fills_a_running_stage_evenly_then_eases_and_counts_the_rest(self):
        todo = [Stage('a', 1, (), (), (), (), None), Stage('b', 2, (), ('a',), (), (), None)]
        with mock.patch.dict(runner.WEIGHTS, {'a': 100, 'b': 300}):
            bar = runner.Progress(todo, {'a': set(), 'b': {'a'}}, 2)
            bar.started['a'] = time.monotonic() - 50  # half its expected time
            now = bar.now()
            self.assertEqual(now['percent'], 12.5)  # half of a's quarter
            self.assertEqual(now['steps'][1:3], [0.5, 0.0])
            self.assertEqual(now['eta'], 350)  # the chain: 50 s of a, then all of b
            bar.started['a'] = time.monotonic() - 1000  # far over its time: nearly its whole quarter
            self.assertTrue(24.9 < bar.now()['percent'] <= 25)
            self.assertGreaterEqual(bar.now()['eta'], 300, 'b is still all to do')

    def test_storage_and_verify_scan_no_source_folder(self):
        """A3: neither reads the sources, so no folder walk (and no 'items' event over the page's own list)."""
        argv = ['--base', str(self.base), '--game-exe', 'game.exe', '--sources', str(self.base)]
        for mode in ('--storage', '--verify'):
            self.events.clear()
            with mock.patch.object(runner.sources, 'scan') as scan:
                self.assertEqual(runner.main([*argv, mode]), 0)
            scan.assert_not_called()
            self.assertNotIn('items', [e['event'] for e in self.events])

    def test_a_finished_build_forgets_hashes_of_files_no_receipt_names(self):
        """D3: hashes.json keeps only the current installation's receipted files after a finished build."""
        self.assertTrue(self.build(table([])))
        gone = str(self.base.resolve()/'installations/0123456789abcdef0123456789abcdef/assets/private/audio/old.pcm')
        runner._hashes[(gone, 1, 1)] = 'dead'
        self.assertTrue(self.build(table([]), prints={'a': 'a-v2', 'b': 'b-v1', 'c': 'c-v1'}))
        saved = (self.base/runner.HASHES).read_text()
        self.assertNotIn('old.pcm', saved)
        self.assertIn('x.txt', saved)  # a, b and c's receipted files stay

    def test_storage_counts_the_build_cache_and_the_build_tools(self):
        """D4: the launcher's "cache" (work/cache + work/convert) and "tools" (data/tools) piles."""
        for rel, size in (('work/cache/a', 3), ('work/convert/b', 4), ('tools/java/c', 5)):
            (self.base/rel).parent.mkdir(parents=True, exist_ok=True); (self.base/rel).write_bytes(b'x' * size)
        sizes = runner.store_sizes(runner.Context(self.base, Path('game.exe'), {}))
        self.assertEqual((sizes['cache'], sizes['tools']), (7, 5))

    def test_a_stray_print_reaches_the_launcher_as_utf_8(self):
        """B6: the launcher reads the pipe as UTF-8 lines; Python would print in the PC's ANSI code page."""
        code = ('import sys; from tools.pipeline import runner; '
                f'runner.main(["--base", {str(self.base)!r}, "--game-exe", "g.exe", "--storage"]); print("caf\\u00e9")')
        out = subprocess.run([sys.executable, '-c', code], capture_output=True, cwd=runner.ROOT,
                             env={**os.environ, 'PYTHONPATH': str(runner.ROOT), 'PYTHONIOENCODING': ''}).stdout
        self.assertTrue(out.rstrip().endswith('café'.encode()), out[-40:])
        out.decode('utf-8')  # every line, the storage event included

    def test_installation_json_is_parsed_once_until_it_changes(self):
        """E2: ctx.install and ctx.marker reuse the parse while the file's size and time hold; our own write is not
        parsed back; a write by another writer (skate3-base's publish_marker, no lock) is."""
        reads, real = [], runner.setup_state.read_marker
        with mock.patch.object(runner.setup_state, 'read_marker', lambda path: reads.append(1) or real(path)):
            ctx = runner.Context(self.base, Path('game.exe'), {})
            for _ in range(50):
                ctx.install, ctx.marker
            self.assertEqual(len(reads), 1)
            mine = ctx.marker
            mine['pipelines']['a'] = 'v'
            self.assertEqual(ctx.marker['pipelines'], {})  # every caller gets its own copy to edit
            ctx.write_marker(mine)
            self.assertEqual((ctx.marker['pipelines'], len(reads)), ({'a': 'v'}, 1))
            other = {**mine, 'directory': 'installations/fedcba9876543210fedcba9876543210', 'pipelines': {}}
            runner.atomic_json(self.base/'installation.json', other)
            self.assertEqual((ctx.install.name, len(reads)), ('fedcba9876543210fedcba9876543210', 2))

    def test_the_budget_counts_what_running_stages_hold_now(self):
        """E1: asked again at each turn: what is free now, plus what the running stages already hold, capped."""
        with mock.patch.object(runner, 'memory_gb', return_value=16), mock.patch('tools.pipeline.fanout.available_gb', return_value=4):
            self.assertEqual(runner.budget_gb(using=3.0), 6.0)
            self.assertEqual(runner.budget_gb(using=20.0), 12.5)
        ctx = runner.Context(self.base, Path('game.exe'), {})
        ctx._jobs = {'mod-maps': {1, 2}, 'tiny': {3}}
        with mock.patch.object(runner, 'job_gb', return_value=1.5):
            self.assertEqual(ctx.using_gb(['mod-maps', 'tiny', 'idle']), 3.0 + 1.0)  # tiny: up to its DEFAULT_GB

    def test_a_stage_held_for_memory_starts_once_other_programs_free_some(self):
        """E1: the first turn has room for one big stage; memory freed meanwhile lets the second start beside it."""
        started = []

        def run(name):
            def go(ctx):
                started.append(name)
                time.sleep(4 if name == 'p' else 0)  # p still runs when the next turn (2 s) asks again
                out = ctx.install/'assets/private'/name
                out.mkdir(parents=True, exist_ok=True); (out/'x.txt').write_text(name)
            return go
        stages = [Stage(n, 0, (), (), (f'assets/private/{n}',), ('pipeline/runner.py',), run(n)) for n in 'pq']
        texts, emit = [], runner.emit
        def record(event, **fields):
            texts.append(fields.get('text', ''))
            emit(event, **fields)
        with mock.patch.dict(runner.MEMORY, {'p': 4, 'q': 4}), mock.patch.object(runner, 'workers', return_value=2),                 mock.patch.object(runner, 'memory_gb', return_value=32),                 mock.patch('tools.pipeline.fanout.available_gb', lambda: 20.0 if started else 2.0),                 mock.patch.object(runner, 'emit', record):
            self.assertTrue(self.build(stages))
        bar = runner.Progress(stages, {}, 2)
        bar.memory_wait = True
        self.assertIn(runner.MEMORY_WAIT, bar.now()['text'], 'E1 step 4: the player is told why a stage waits')
        self.assertFalse(any(runner.MEMORY_WAIT in t for t in texts[-3:]), 'and the note goes once nothing waits')
        times = {t['stage']: t['start'] for t in json.loads((self.base/'logs/build-times.json').read_text())['stages']}
        self.assertLess(times['q'], 3.5)  # not after p ended (4 s): the budget grew while p ran

    def test_a_marker_emptied_by_a_power_cut_falls_back_to_its_backup(self):
        """B5: atomic_json keeps the last good installation.json as .bak; read_marker uses it when the file is empty."""
        ctx = runner.Context(self.base, Path('game.exe'), {})
        good = ctx.marker
        runner.atomic_json(self.base/'installation.json', {**good, 'version': 2})
        (self.base/'installation.json').write_text('')  # the name kept, the contents never written
        self.assertEqual(runner.setup_state.read_marker(self.base/'installation.json'), good)


class DriveKind(unittest.TestCase):
    @unittest.skipUnless(os.name == 'nt', 'Windows volumes')
    def test_the_system_drive_is_named_without_powershell(self):
        """E3: an IOCTL on the volume, no process: what the system drive is, in milliseconds; '' for no such drive."""
        runner.drive_kind.cache_clear()
        with mock.patch.object(runner.subprocess, 'run', side_effect=AssertionError('no process')):
            began = time.monotonic()
            kind = runner.drive_kind(os.environ.get('SystemDrive', 'C:'))
            self.assertLess(time.monotonic() - began, 0.5)
            self.assertRegex(kind, r'^(SSD|HDD)\b')
            self.assertEqual(runner.drive_kind('Q:') if not Path('Q:/').exists() else '', '')
        runner.drive_kind.cache_clear()


class SetupModes(unittest.TestCase):
    """skate3setup.exe --pipeline / --task, as the frozen exe starts them (tools/setup.py main)."""
    def setUp(self):
        from tools import setup
        self.setup = setup
        argv, path = list(sys.argv), list(sys.path)
        self.addCleanup(lambda: (setattr(sys, 'argv', argv), sys.path.__setitem__(slice(None), path)))

    def test_pipeline_hands_the_rest_of_the_command_line_to_the_runner(self):
        sys.argv = ['skate3setup.exe', '--pipeline', '--check', '--base', 'data & Ünï']
        with mock.patch.object(runner, 'main', return_value=7) as main:
            self.assertEqual(self.setup.main(), 7)
        main.assert_called_once_with(['--check', '--base', 'data & Ünï'])

    def test_task_runs_a_script_as_main_with_its_folder_first_on_sys_path_and_its_own_argv(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            (root/'tools/a folder').mkdir(parents=True)
            script, out = root/'tools/a folder/probe.py', root/'out.json'
            script.write_text('import json, sys\nfrom pathlib import Path\n'
                              'Path(sys.argv[1]).write_text(json.dumps([sys.argv, sys.path[0], __name__]))\n', encoding='utf-8')
            sys.argv = ['skate3setup.exe', '--task', str(script), str(out), 'two words', '--flag']
            with mock.patch.object(self.setup, 'ROOT', root):
                self.assertEqual(self.setup.main(), 0)
            argv, first, name = json.loads(out.read_text())
            self.assertEqual(argv, [str(script), str(out), 'two words', '--flag'])
            self.assertEqual((first, name), (str(script.parent), '__main__'))
            outside = Path(temp)/'outside.py'
            outside.write_text('raise SystemExit(5)')
            sys.argv = ['skate3setup.exe', '--task', str(outside)]
            with mock.patch.object(self.setup, 'ROOT', root), self.assertRaises(RuntimeError):
                self.setup.main()  # only scripts inside the bundled tools folder run

    def test_the_tutorial_runs_its_dmos_script_through_task_when_frozen(self):
        source = (runner.TOOLS/'pipeline/skate2port/old_tutorial.py').read_text(encoding='utf-8')
        self.assertIn("[sys.executable, '--task'] if getattr(sys, 'frozen', False)", source)  # as derive.py and Context.script
        self.assertIn("subprocess.run([*task, str(here/'old_tutorial_dmos.py')", source)


if __name__ == '__main__':
    unittest.main()
