# tools/test_prepare_weather_audio.py
import json, math, os, struct, subprocess, sys, tempfile, unittest, wave
sys.path.insert(0, os.path.dirname(__file__))
import prepare_weather_audio as pwa

def write_wav(path, seconds, freq, rate=44100, channels=1, amp=0.3):
    n = int(seconds * rate)
    with wave.open(path, "wb") as w:
        w.setnchannels(channels); w.setsampwidth(2); w.setframerate(rate)
        frames = bytearray()
        for i in range(n):
            # noise-ish: sum of sines so loudness measurement is stable
            v = amp * (math.sin(2*math.pi*freq*i/rate) + 0.5*math.sin(2*math.pi*freq*2.71*i/rate)) / 1.5
            frames += struct.pack("<h", int(v * 32767)) * channels
        w.writeframes(bytes(frames))

class Stage(unittest.TestCase):
    def test_role_files_found_by_id_prefix_and_crdownload_ignored(self):
        with tempfile.TemporaryDirectory() as d:
            write_wav(os.path.join(d, "119500__x__gentle.wav"), 1, 500)
            open(os.path.join(d, "Unconfirmed 1.crdownload"), "wb").close()
            found = pwa.find_sources(d)
            self.assertIn(119500, found)
            self.assertEqual(len(found), 1)

    def test_loop_seam_is_continuous(self):
        # A looped file's last and first samples must join with no step larger
        # than the signal's own max sample-to-sample step (equal-power crossfade).
        with tempfile.TemporaryDirectory() as d:
            src = os.path.join(d, "119500__x__gentle.wav"); write_wav(src, 8, 440)
            pcm = pwa.decode(src, rate=48000, channels=2)
            looped = pwa.make_loop(pcm, rate=48000, seconds=5, fade=0.5)
            steps = [abs(looped[i+1][0]-looped[i][0]) for i in range(len(looped)-1)]
            seam = abs(looped[0][0] - looped[-1][0])
            self.assertLessEqual(seam, max(steps) * 1.05)

    def test_loudness_target_within_half_lu(self):
        with tempfile.TemporaryDirectory() as d:
            src = os.path.join(d, "119500__x__g.wav"); write_wav(src, 10, 300, amp=0.05)
            out = os.path.join(d, "o.ogg")
            pwa.master(src, out, lufs=-30.0, loop_seconds=None, eq="bed")
            self.assertAlmostEqual(pwa.measure_lufs(out), -30.0, delta=0.5)

    def test_manifest_lists_only_present_sources(self):
        with tempfile.TemporaryDirectory() as d:
            srcd = os.path.join(d, "src"); os.mkdir(srcd)
            write_wav(os.path.join(srcd, "119500__x__g.wav"), 50, 300)
            out = os.path.join(d, "out")
            pwa.run(srcd, out, install=None)
            m = json.load(open(os.path.join(out, "weather-audio", "manifest.json")))
            self.assertEqual([e["source"] for e in m["roles"]["hard_light"]], [119500])
            self.assertEqual(m["roles"]["bed_heavy"], [])

    def test_swap_puts_the_old_folder_back_when_the_new_one_cannot_move_in(self):
        from pathlib import Path
        from unittest import mock
        with tempfile.TemporaryDirectory() as d:
            src, dest = Path(d, "new"), Path(d, "weather-audio")
            src.mkdir(); (src / "new.txt").write_text("new")
            dest.mkdir(); (dest / "keep.txt").write_text("old")
            rename = pwa.Path.rename
            def locked(self, target):
                if self.name.endswith(".partial"):
                    raise PermissionError("locked by a virus scanner")
                return rename(self, target)
            with mock.patch.object(pwa.Path, "rename", locked):
                with self.assertRaises(PermissionError):
                    pwa._swap(src, dest)
            self.assertEqual((dest / "keep.txt").read_text(), "old")

    def test_install_refuses_a_short_or_empty_bank(self):
        from pathlib import Path
        with tempfile.TemporaryDirectory() as d:
            srcd, install = Path(d, "src"), Path(d, "assets")
            srcd.mkdir(); (install / "weather-audio").mkdir(parents=True)
            (install / "weather-audio" / "keep.txt").write_text("installed")
            with self.assertRaises(ValueError):  # a wrong folder: nothing matches
                pwa.run(srcd, Path(d, "out"), install=install)
            write_wav(str(srcd / "119500__x__g.wav"), 1, 300)
            with self.assertRaises(ValueError):  # most roles missing
                pwa.run(srcd, Path(d, "out"), install=install)
            self.assertEqual((install / "weather-audio" / "keep.txt").read_text(), "installed")

    def test_bed_eq_curve(self):
        # White noise through BED_EQ: 4 kHz sits under 1 kHz (the -3 dB bell), and the 90 Hz high-pass and 12 kHz
        # low-pass take the ends down.
        import numpy as np
        rate = 48000
        with tempfile.TemporaryDirectory() as d:
            src = os.path.join(d, "noise.wav")
            noise = np.random.default_rng(1).uniform(-0.3, 0.3, rate * 4)
            with wave.open(src, "wb") as w:
                w.setnchannels(1); w.setsampwidth(2); w.setframerate(rate)
                w.writeframes((noise * 32767).astype("<i2").tobytes())
            dry = pwa.decode(src, rate, 1)[:, 0]
            wet = pwa.decode(src, rate, 1, pwa.BED_EQ)[:, 0]
            n = min(len(dry), len(wet))
            f = np.fft.rfftfreq(n, 1 / rate)
            def db(lo, hi):
                band = (f >= lo) & (f < hi)
                return 10 * np.log10(np.sum(np.abs(np.fft.rfft(wet[:n])[band]) ** 2) / np.sum(np.abs(np.fft.rfft(dry[:n])[band]) ** 2))
            ref = db(800, 1200)
            self.assertLess(db(3600, 4400), ref - 1.5)
            self.assertLess(db(20, 50), ref - 6)
            self.assertLess(db(16000, 20000), ref - 3)

if __name__ == "__main__":
    unittest.main()
