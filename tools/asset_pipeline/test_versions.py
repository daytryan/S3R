"""Installation selection and refresh checks using synthetic files only."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from tools.asset_pipeline import versions as v
from tools.asset_pipeline.install import install


class AssetVersions(unittest.TestCase):
    def test_orchestration_does_not_invalidate_exporters_and_line_endings_are_stable(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); (root/'asset_pipeline').mkdir()
            install=root/'asset_pipeline/install.py'
            install.write_text('def extract(): return 1\ndef convert_map(): return 2\ndef install(): return 3\n')
            before=v.fingerprints(root)
            install.write_text(install.read_text().replace('return 3','return 4'))
            self.assertEqual(v.fingerprints(root),before)
            install.write_text(install.read_text().replace('return 2','return 5'))
            self.assertEqual(v.changed_groups(before,v.fingerprints(root)),{'maps'})
            exporter=root/'asset_pipeline/asset_exports.py'
            exporter.write_text('def core(): return 1\ndef hud(): return 2\n')
            before=v.fingerprints(root)
            exporter.write_text(exporter.read_text().replace('return 2','return 3'))
            self.assertEqual(v.changed_groups(before,v.fingerprints(root)),{'hud'})
            resource=root/'asset_pipeline/names.txt';resource.write_bytes(b'one\ntwo\n')
            before=v.fingerprints(root);resource.write_bytes(b'one\r\ntwo\r\n')
            self.assertEqual(v.fingerprints(root),before)

    def test_equivalences_only_accept_exact_old_and_new_pair(self):
        migrations=json.loads(Path(v.__file__).with_name('pipeline-equivalence.json').read_text())
        # Equivalences describe historical pairs, not every future exporter.
        current={g:migrations[g][0][1] for g in v.GROUPS}
        old={g:migrations[g][0][0] for g in v.GROUPS}
        self.assertEqual(v.changed_groups(old,current),set())
        changed={**current,'maps':'future-exporter'}
        self.assertEqual(v.changed_groups(old,changed),{'maps'})

    def test_hud_change_does_not_reconvert_maps(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            (root/'prepare_hud.py').write_text('before')
            before=v.fingerprints(root)
            (root/'prepare_hud.py').write_text('after')
            self.assertEqual(v.changed_groups(before,v.fingerprints(root)), {'hud'})
            (root/'asset_pipeline').mkdir()
            (root/'asset_pipeline/vlt.py').write_text('new decoder')
            self.assertEqual(v.changed_groups(before,v.fingerprints(root)),set(v.GROUPS))

    def test_fresh_copy_never_adopts_other_installation(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            (root/'installation.json').write_text(json.dumps({'version':1,'directory':'../old'}))
            with self.assertRaises(ValueError):v.installed(root)
            self.assertIsNone(v.installed(root/'new-copy/data'))

    def fixture(self, root):
        base=root/'copy/data';old=base/('installations/'+'a'*32)
        private=old/'assets/private';(private/'stock').mkdir(parents=True)
        (private/'game.json').write_text('{}');(private/'stock/skater-collections.json').write_text('{}')
        (private/'hud').mkdir();(private/'hud/old.txt').write_text('old')
        (old/'maps').mkdir();(old/'maps/University.skate').write_bytes(b'unchanged map')
        (old/'settings').mkdir();(old/'settings/default-map.json').write_text('"custom.skate"')
        (old/'maps.json').write_text('[{"name":"University","path":"maps/University.skate"}]')
        current={group:'new' for group in v.GROUPS}
        marker={'version':1,'directory':'installations/'+'a'*32,'pipelines':{**current,'hud':'old'}}
        (base/'installation.json').write_text(json.dumps(marker))
        from tools.asset_pipeline.group_receipts import record
        for group in v.GROUPS:
            # Minimal synthetic receipt; tests stub converters, never retail input.
            marker.setdefault('outputs',{})[group] = record(old,group) or record(old,'character')
        (base/'installation.json').write_text(json.dumps(marker))
        source=root/'disc'
        for name in ('default.xex','data/big/miscload.big','data/big/miscboot.big','data/big/db.big',
                     'data/content/createacharacter.big','data/content/marquee.big','data/content/worldDIST_University.big'):
            p=source/name;p.parent.mkdir(parents=True,exist_ok=True);p.touch()
        return base,old,source,current,marker

    def test_refresh_preserves_maps_and_publishes_only_after_validation(self):
        with tempfile.TemporaryDirectory() as temp:
            base,old,source,current,marker=self.fixture(Path(temp))
            calls=[]
            def run(args,*unused):
                calls.append([str(a) for a in args])
            with patch.object(v,'fingerprints',return_value=current), patch('tools.asset_pipeline.install.run',side_effect=run):
                new=install(source,base,Path('unused.exe'),lambda _:None,refresh=True)
            self.assertEqual((new/'maps/University.skate').read_bytes(),b'unchanged map')
            self.assertEqual((new/'settings/default-map.json').read_text(),'"custom.skate"')
            self.assertTrue((old/'assets/private/hud/old.txt').is_file())
            self.assertTrue((new/'assets/private/hud/old.txt').exists())  # Kept until replacement validates.
            self.assertEqual(v.installed(base)[1]['pipelines'],current)
            self.assertTrue(any(any('prepare_runtime_huds.py' in a for a in args) for args in calls))
            self.assertFalse(any(any('map_job.py' in a for a in args) for args in calls))

    def test_a_map_refresh_fixes_the_default_map_where_the_game_reads_it(self):
        # R's follow-up (10-13): since F2 the game reads data/settings/default-map.json; a map that is gone falls back there too
        with tempfile.TemporaryDirectory() as temp:
            base,old,source,current,marker=self.fixture(Path(temp))
            marker['pipelines']['maps']='old';(base/'installation.json').write_text(json.dumps(marker))
            (base/'settings').mkdir();(base/'settings/default-map.json').write_text('"maps/Gone.skate"')
            (base/'settings/profile.json').write_text('{}')
            def run(args,*unused):  # map_job.py: the University map converts again
                args=[str(x) for x in args]
                if any('map_job.py' in x for x in args):
                    stage=Path(args[args.index('--stage')+1]);(stage/'maps').mkdir(exist_ok=True)
                    (stage/'maps/University.skate').write_bytes(b'new map')
                    Path(args[args.index('--result')+1]).write_text(json.dumps({'name':'University','path':'maps/University.skate'}))
            with patch.object(v,'fingerprints',return_value=current), patch('tools.asset_pipeline.install.run',side_effect=run):
                new=install(source,base,Path('unused.exe'),lambda _:None,refresh=True)
            self.assertEqual(json.loads((base/'settings/default-map.json').read_text()),'maps/University.skate')
            self.assertEqual((new/'settings/default-map.json').read_text(),'"maps/University.skate"')  # 'custom.skate' is gone too
            self.assertEqual((base/'settings/profile.json').read_text(),'{}')

    def test_failed_refresh_keeps_previous_record(self):
        with tempfile.TemporaryDirectory() as temp:
            base,old,source,current,marker=self.fixture(Path(temp))
            with patch.object(v,'fingerprints',return_value=current), patch('tools.asset_pipeline.install.run',side_effect=RuntimeError('failed')):
                with self.assertRaises(RuntimeError):install(source,base,Path('unused.exe'),lambda _:None,refresh=True)
            self.assertEqual(v.installed(base)[1],marker)
            from tools.asset_pipeline.setup_state import setup_lock
            with setup_lock(base):pass  # Released even on failure; file persists.

    def test_stage_fingerprints_follow_their_sources(self):
        from tools.pipeline.stages import STAGES
        prints = v.fingerprints()
        self.assertTrue({s.name for s in STAGES if s.name != 'skate3-base'} <= set(prints))
        self.assertIn('prepare_hud_meters.py', v.SOURCES['hud'])

    def test_a_stage_fingerprint_moves_with_its_own_code_only(self):
        from tools.pipeline import stages
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/'pipeline').mkdir()
            table = root/'pipeline/stages.py'
            table.write_text(Path(stages.__file__).read_text(encoding='utf-8'), encoding='utf-8')
            before = v.fingerprints(root)
            def edit(old, new):
                text = table.read_text(encoding='utf-8')
                self.assertEqual(text.count(old), 1, old)
                table.write_text(text.replace(old, new), encoding='utf-8')
            edit("        ctx.script('skate1-challenges', S2_PORT + script, ROOT, ctx.disc('s1-disc'), assets(ctx))",
                 "        ctx.script('skate1-challenges', S2_PORT + script, ROOT, ctx.disc('s1-disc'), assets(ctx), '-x')")
            after = v.fingerprints(root)
            self.assertNotEqual(after['skate1-challenges'], before['skate1-challenges'])
            self.assertEqual({k: x for k, x in after.items() if k != 'skate1-challenges'},
                             {k: x for k, x in before.items() if k != 'skate1-challenges'})  # mod-maps and every other stage
            edit("    ctx.script(stage, 'pipeline/converter/convert_ps3.py', *args)", "    ctx.script(stage, 'pipeline/converter/convert_ps3.py', *args, '-x')")
            moved = {k for k, x in v.fingerprints(root).items() if x != after[k]}
            self.assertEqual(moved, {'mod-maps', 'skate2-parks', 'skate1-map'})  # the stages that call convert_mod_map
            edited = v.fingerprints(root)
            table.write_text('# a comment\n' + table.read_text(encoding='utf-8'), encoding='utf-8')
            self.assertEqual(v.fingerprints(root), edited)  # comments are not code

    def test_a_stage_fingerprint_moves_with_its_inputs_and_roots(self):
        import dataclasses
        from tools.pipeline import stages
        before = v.fingerprints()
        for field, value in (('roots', ('assets/private/audio-extra',)), ('inputs', ('s2-disc',))):
            changed = [dataclasses.replace(s, **{field: (*getattr(s, field), *value)}) if s.name == 'audio-image' else s
                       for s in stages.STAGES]
            with patch.object(stages, 'STAGES', changed):
                after = v.fingerprints()
            self.assertNotEqual(after['audio-image'], before['audio-image'], field)
            self.assertEqual({k: x for k, x in after.items() if k != 'audio-image'},
                             {k: x for k, x in before.items() if k != 'audio-image'}, field)

    def test_owned_game_is_hashed_for_every_group_and_stage(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/'owned_game/sub').mkdir(parents=True)
            (root/'owned_game/sub/deep.py').write_text('one')
            before = v.fingerprints(root)
            (root/'owned_game/sub/deep.py').write_text('two')
            after = v.fingerprints(root)
            self.assertTrue(all(before[k] != after[k] for k in before))

    def test_only_the_pins_that_shape_the_files_move_the_fingerprints(self):
        # #4 narrower COMMON: a certifi or PyInstaller bump rebuilt everything for every player; numpy and Pillow make
        # the files, so they still do
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            pins = root/'requirements-setup.txt'
            pins.write_text('pyinstaller==6.19.0\nnumpy==2.2.6\nPillow==11.3.0\ncryptography==50.0.1\ncertifi==2026.7.22\n')
            before = v.fingerprints(root)
            pins.write_bytes(b'pyinstaller==6.20.0\r\nnumpy==2.2.6\r\nPillow==11.3.0\r\ncryptography==51.0.0\r\ncertifi==2026.9.1\r\n')
            self.assertEqual(v.fingerprints(root), before)
            for bump in ('numpy==2.3.0', 'Pillow==12.0.0'):
                pins.write_text('\n'.join(bump if line.startswith(bump.split('=')[0]) else line
                                          for line in ('pyinstaller==6.19.0', 'numpy==2.2.6', 'Pillow==11.3.0')))
                after = v.fingerprints(root)
                self.assertTrue(all(before[k] != after[k] for k in before), bump)


if __name__=='__main__':unittest.main()
