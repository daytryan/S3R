# tools/pipeline/test_non_ascii.py
"""#19: a Windows user folder outside the ANSI code page (a user folder named Юлия) through the build's tool chain, without discs:
the runner's child scripts, ffmpeg, vgmstream and Java/unluac. A tool missing on this PC is skipped (set SKATE_FFMPEG,
SKATE_VGMSTREAM, SKATE_JAVA and SKATE_UNLUAC to the launcher's downloads to run them all)."""
import os, shutil, struct, subprocess, sys, tempfile, unittest
from pathlib import Path
from unittest import mock
from tools.pipeline import runner

NAME = 's3r-Юлия-José-測試-'  # Cyrillic and CJK are outside cp1252; é is inside it


def tool(env, name):
    found = os.environ.get(env) or shutil.which(name)
    return found if found and Path(found).is_file() else None


def lua51_chunk():
    """The smallest Lua 5.1 chunk: a main function that only returns (little-endian, 4-byte size_t)."""
    header = b'\x1bLua\x51\x00\x01\x04\x04\x04\x08\x00'
    function = (struct.pack('<I', 0) + struct.pack('<ii', 0, 0) + bytes([0, 0, 2, 2])
                + struct.pack('<iI', 1, 0x0080001E) + struct.pack('<iiiii', 0, 0, 0, 0, 0))
    return header + function


class NonAscii(unittest.TestCase):
    def setUp(self):
        self.home = Path(tempfile.mkdtemp(prefix=NAME))
        self.addCleanup(shutil.rmtree, self.home, True)

    def test_a_child_script_prints_and_writes_a_non_ascii_path(self):
        script = self.home/'step.py'
        script.write_text('import sys, pathlib\nout = pathlib.Path(sys.argv[1])\nprint("writing", out)\n'
                          'out.write_text("ok")\n', encoding='utf-8')
        ctx = runner.Context(self.home/'data', self.home/'game.exe', {})
        # stdout to a file is the ANSI code page in a child (frozen ones ignore PYTHONIOENCODING): print() of a
        # path outside it raised UnicodeEncodeError and failed the step
        ctx.script('step', script, self.home/'out-Юлия.txt')
        self.assertEqual((self.home/'out-Юлия.txt').read_text(), 'ok')
        self.assertIn('out-Юлия.txt', (self.home/'data/logs/step.log').read_text(encoding='utf-8'))

    @unittest.skipUnless(tool('SKATE_FFMPEG', 'ffmpeg'), 'ffmpeg not on this PC')
    def test_ffmpeg_reads_and_writes_in_a_non_ascii_folder(self):
        ffmpeg = tool('SKATE_FFMPEG', 'ffmpeg')
        wav, ogg = self.home/'tone-Юлия.wav', self.home/'tone-Юлия.ogg'
        for argv in ([ffmpeg, '-v', 'error', '-f', 'lavfi', '-i', 'sine=d=0.2', wav],
                     [ffmpeg, '-v', 'error', '-i', wav, '-c:a', 'libvorbis', ogg]):
            done = subprocess.run([str(a) for a in argv], capture_output=True)
            self.assertEqual(done.returncode, 0, done.stderr)
        self.assertGreater(ogg.stat().st_size, 0)

    @unittest.skipUnless(tool('SKATE_VGMSTREAM', 'vgmstream-cli'), 'vgmstream not on this PC')
    def test_vgmstream_decodes_into_a_non_ascii_folder(self):
        import wave
        source, out = self.home/'in-Юлия.wav', self.home/'out-Юлия.wav'
        with wave.open(str(source), 'wb') as w:
            w.setnchannels(1), w.setsampwidth(2), w.setframerate(22050), w.writeframes(b'\0\1' * 4410)
        done = subprocess.run([tool('SKATE_VGMSTREAM', 'vgmstream-cli'), '-o', str(out), str(source)], capture_output=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertGreater(out.stat().st_size, 44)

    @unittest.skipUnless(tool('SKATE_JAVA', 'java') and tool('SKATE_UNLUAC', 'unluac.jar'), 'Java or unluac not on this PC')
    def test_the_unluac_batch_runs_from_a_non_ascii_game_folder(self):
        # Java turns its arguments into the PC's ANSI code page: `java -jar C:/Users/Юлия/unluac.jar` on an English PC
        # says "Unable to access jarfile C:/Users/????/unluac.jar". The game folder (the jar, UnluacBatch.class) and TEMP
        # (the build sets it to data/tmp; the chunks go there) both sit under the player's folder.
        from tools import unluac_batch
        here = self.home/'game/support/skate3setup/_internal/tools'
        here.mkdir(parents=True)
        shutil.copy(Path(unluac_batch.__file__).with_name('UnluacBatch.class'), here)
        jar = self.home/'game/data/tools/unluac/unluac.jar'
        jar.parent.mkdir(parents=True)
        shutil.copyfile(tool('SKATE_UNLUAC', 'unluac.jar'), jar)
        (self.home/'game/data/tmp').mkdir()
        chunk = lua51_chunk()
        with mock.patch.object(unluac_batch, 'HERE', here), mock.patch.object(tempfile, 'tempdir', str(self.home/'game/data/tmp')):
            stdout, code = unluac_batch.run([chunk], java=tool('SKATE_JAVA', 'java'), unluac=jar)[chunk]
            self.assertEqual(code, 0, stdout)
            with mock.patch.object(unluac_batch, 'HERE', self.home/'no-class'):  # the one-JVM-per-chunk fallback
                stdout, code = unluac_batch.run([chunk], java=tool('SKATE_JAVA', 'java'), unluac=jar)[chunk]
            self.assertEqual(code, 0, stdout)



# The longest installed file in a real build (Desktop launcher copy, 10-13), below installations/<id>/: 314 characters
LONGEST = ('assets/private/create-skater/thumbs/assets/data/fe/cas/male/outertorso/shortsleeve_buttonup_shirt_closed_male_'
           'shortsleeve_buttonup_shirt_closed_male_duncombe_leafy_thing_material_ropa/0000_shortsleeve_buttonup_shirt_'
           'closed_male_shortsleeve_buttonup_shirt_closed_male_duncombe_leafy_thing_material_ropa.Texture.rgba')


@unittest.skipUnless(runner.long_paths(), "this PC has Windows' long paths off (the build refuses to start there)")
class LongBase(unittest.TestCase):
    """Finding #3: a game folder ~120 characters deep. A build writes, receipts, seals, verifies and cleans up there."""

    def test_a_build_runs_from_a_120_character_base(self):
        temp = Path(tempfile.mkdtemp(prefix='s3r-long-'))
        self.addCleanup(shutil.rmtree, temp, True)
        base = temp/('x' * (120 - len(str(temp)) - len('/x/game/data') + 1))/'game/data'
        self.assertEqual(len(str(base)), 120)
        base.mkdir(parents=True)
        script = temp/'step.py'
        script.write_text('import sys, pathlib\nout = pathlib.Path(sys.argv[1])\nout.parent.mkdir(parents=True, exist_ok=True)\n'
                          'print("writing", out)\nout.write_bytes(b"rgba")\n', encoding='utf-8')
        root = 'assets/private/create-skater/thumbs'
        stages = [runner.stages.Stage('thumbs', 0, (), (), (root,), (), lambda ctx: ctx.script('thumbs', script, ctx.install/LONGEST))]
        events = []
        with mock.patch.object(runner, 'STAGES', stages), mock.patch.object(runner, 'stage_fingerprints', lambda *a: {'thumbs': 'v1'}), \
                mock.patch.object(runner, 'emit', lambda event, **f: events.append({'event': event, **f})):
            ctx = runner.Context(base, temp/'game.exe', {})
            (base/'installations/0123456789abcdef0123456789abcdef').mkdir(parents=True)
            runner.atomic_json(base/'installation.json', {'version': 1, 'directory': 'installations/0123456789abcdef0123456789abcdef', 'pipelines': {}})
            self.assertTrue(runner.build(ctx, resume=True), events[-3:])
            longest = max(len(str(p)) for p in base.rglob('*'))
            print(f'\nlongest path from a 120-character base: {longest} characters (Windows without long paths: 260)')
            self.assertGreater(longest, 260)  # ~480: the 314-character thumbnail under installations/<id>/
            self.assertIn(LONGEST, runner.Context(base, temp/'game.exe', {}).marker['outputs']['stages'])
            with mock.patch.object(runner, 'customiser_current', return_value=True), \
                    mock.patch.object(runner.group_receipts, 'damaged', return_value=set()):  # no clothing set or setup groups here
                self.assertTrue(runner.complete(runner.Context(base, temp/'game.exe', {}), {'thumbs': 'v1'}))
            runner.verify_install(runner.Context(base, temp/'game.exe', {}))
            self.assertEqual((events[-1]['event'], events[-1]['ok']), ('verify', True))
            runner.store_sizes(runner.Context(base, temp/'game.exe', {}))


if __name__ == '__main__':
    unittest.main()
