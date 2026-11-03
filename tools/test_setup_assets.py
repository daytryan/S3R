"""Independent HUD installation checks without retail data."""
import json
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parent))
import prepare_runtime_huds as huds


def scoring(game, output, *args):
    (output/'runtime').mkdir(parents=True)
    (output/'runtime/trickdisplay.json').write_text(json.dumps(dict(
        version=1, format='skate3-scoring-hud', shapes={}, fonts={})))


def marker(game, output):
    output.mkdir(parents=True)
    (output/'hud.json').write_text(json.dumps(dict(version=1, canvas=[1280,720], meshes=[{}], textures=[])))


class HudWorkspace(unittest.TestCase):
    def test_long_install_uses_short_temporary_workspace_and_keeps_outputs(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); assets=root/'assets'; seen=[]
            long_work=root/('nested-install-'*12)/'conversion/hud'
            def prepare(game, output, *args):
                seen.append(output)
                self.assertNotIn(long_work,output.parents)
                self.assertLess(len(str(output/'source-cache/assets/data/fe/source/images/buttons/xbox360/buttons/0000_button_DPad_Down_hud.Texture.texture.bin')),260)
                scoring(game,output,*args)
            with patch.object(huds,'prepare_scoring',side_effect=prepare),patch.object(huds,'prepare_marker',side_effect=marker):
                self.assertEqual(len(huds.prepare(root/'game',assets,long_work)),2)
            self.assertTrue((assets/'private/hud/runtime/trickdisplay.json').is_file())
            self.assertTrue(all(not p.exists() for p in seen))

    def test_one_missing_hud_still_installs_other_and_receipts_allow_reuse(self):
        from tools.asset_pipeline.group_receipts import record, damaged
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); assets=root/'assets'
            with patch.object(huds,'prepare_scoring',side_effect=KeyError('missing font')),patch.object(huds,'prepare_marker',side_effect=marker):
                self.assertEqual(len(huds.prepare(root/'game',assets,root/'work')),1)
            self.assertEqual(json.loads((assets/'private/hud-availability.json').read_text())['status'],'unavailable')
            receipt=record(root,'hud')
            self.assertTrue(receipt)
            self.assertEqual(damaged(root,{'outputs':{'hud':receipt}},exclude=('core','character','environment','maps')),set())

    def test_failure_keeps_verified_existing_huds(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);assets=root/'assets'
            scoring(None,assets/'private/hud');marker(None,assets/'private/session-marker')
            before=(assets/'private/hud/runtime/trickdisplay.json').read_bytes()
            with patch.object(huds,'prepare_scoring',side_effect=RuntimeError('decode failed')),patch.object(huds,'prepare_marker',side_effect=FileNotFoundError('missing bank')):
                self.assertEqual(huds.prepare(root/'game',assets,root/'work'),{})
            self.assertEqual((assets/'private/hud/runtime/trickdisplay.json').read_bytes(),before)
            for name in ('hud','session-marker'):
                self.assertEqual(json.loads((assets/'private'/(name+'-availability.json')).read_text())['status'],'retained')

    def test_start_menu_failure_keeps_verified_copy_and_huds(self):
        import prepare_core_menu as menu
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);assets=root/'assets'
            installed=assets/'private/core-menu'
            (installed/'runtime').mkdir(parents=True)
            (installed/menu.MANIFEST).write_text(json.dumps(dict(format='skate3-core-menu',version=1,shapes={},fonts={})))
            with patch.object(huds,'prepare_scoring',side_effect=scoring),patch.object(huds,'prepare_marker',side_effect=marker),\
                 patch.object(menu,'prepare',side_effect=FileNotFoundError('no fedata.big')):
                self.assertEqual(len(huds.prepare(root/'game',assets,root/'work')),2)
            self.assertTrue((installed/menu.MANIFEST).is_file())
            self.assertEqual(json.loads((assets/'private/core-menu-availability.json').read_text())['status'],'retained')

    def test_create_skater_failure_keeps_verified_copy(self):
        import prepare_create_skater as screen
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);assets=root/'assets'
            installed=assets/'private'/screen.FOLDER
            (installed/'runtime').mkdir(parents=True)
            (installed/screen.MANIFEST).write_text(json.dumps(dict(format=screen.KIND,version=1,shapes={},fonts={})))
            # A verified copy includes the confirmation popup's own manifest (create_skater.rs loads both).
            (installed/screen.POPUP_MANIFEST).parent.mkdir(parents=True)
            (installed/screen.POPUP_MANIFEST).write_text(json.dumps(dict(format=screen.POPUP_KIND,version=1,shapes={},fonts={})))
            with patch.object(huds,'prepare_scoring',side_effect=scoring),patch.object(huds,'prepare_marker',side_effect=marker),                 patch.object(screen,'prepare',side_effect=FileNotFoundError('no fedynamic.big')):
                huds.prepare(root/'game',assets,root/'work')
            self.assertTrue((installed/screen.MANIFEST).is_file())
            self.assertEqual(json.loads((assets/'private/create-skater-availability.json').read_text())['status'],'retained')

    def test_button_glyph_png_filters_decode(self):
        import struct, zlib
        from prepare_create_skater import png_rgba
        pixels=[bytes([10*x+y,20,30,255]) for y in range(3) for x in range(2)]
        rows=[b''.join(pixels[y*2:y*2+2]) for y in range(3)]
        def chunk(kind,body): return struct.pack('>I',len(body))+kind+body+struct.pack('>I',zlib.crc32(kind+body))
        # Row 0 unfiltered, row 1 "up" (2), row 2 "sub" (1).
        up=bytes((a-b)&255 for a,b in zip(rows[1],rows[0]))
        sub=bytes((rows[2][i]-(rows[2][i-4] if i>=4 else 0))&255 for i in range(8))
        raw=bytes([0])+rows[0]+bytes([2])+up+bytes([1])+sub
        png=bytes.fromhex('89504e470d0a1a0a')+chunk(b'IHDR',struct.pack('>IIBBBBB',2,3,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
        self.assertEqual(png_rgba(png),(2,3,b''.join(rows)))

    def test_disk_errors_still_abort(self):
        with tempfile.TemporaryDirectory() as temp,patch.object(huds,'prepare_scoring',side_effect=PermissionError('read denied')):
            with self.assertRaises(PermissionError):huds.prepare(Path(temp),Path(temp)/'assets',Path(temp)/'work')
