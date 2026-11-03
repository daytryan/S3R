"""ea_speech on synthetic bytes laid out as stage1_traces 1.5 says (no retail data): the .evt, hdr and sth layouts, and the
vgmstream hand-off with a stand-in for vgmstream-cli."""
import struct
import sys
import tempfile
import unittest
import wave
from pathlib import Path
from unittest import mock

import ea_speech as e


def leaf(masks, sample, weight=0x39):
    """An .evt entry: weight, 64 04 00, u32 LE mask count, u32 LE word count, the masks (u32 BE), u16 BE sample id, six zero bytes."""
    n = len(masks)
    return bytes([weight, 0x64, 0x04, 0x00]) + struct.pack("<II", n, n + 3) + struct.pack(">%dI" % n, *masks) + struct.pack(">H", sample) + bytes(6)


def record(event, leaves, conditions=(1,)):
    """An .evt record: u16 event, u16 90, u16 500, u8 leaf count, u8 0, u8 flags, u8 0x64, u16 condition count, u16 leaf offsets
    (words from the record, padded to an even count), the condition list ("ff <type> 04" each), padded to a word, then the leaves."""
    offsets = len(leaves) + len(leaves) % 2
    head = struct.pack(">HHHBBBBH", event, 90, 500, len(leaves), 0, 0x10 * len(conditions), 0x64, len(conditions))
    conds = b"".join(bytes([0xFF, c, 0x04]) for c in conditions)
    conds += bytes(-(12 + 2 * offsets + len(conds)) % 4)
    at = (12 + 2 * offsets + len(conds)) // 4
    starts, body = [], b""
    for item in leaves:
        starts.append(at + len(body) // 4)
        body += item
    table = struct.pack(">%dH" % len(starts), *starts) + bytes(2 * (offsets - len(starts)))
    return head + table + conds + body


def evt(records):
    """An .evt: magic 0315B007, string table offset, u32, hash, u16 count at 0x10, u16 record offsets (words) at 0x18, the records."""
    at = 24 + 2 * len(records)
    at += at % 4 and 4 - at % 4
    starts, body = [], b""
    for r in records:
        starts.append((at + len(body)) // 4)
        body += r
    head = struct.pack(">IIII", 0x0315B007, 0xC80, 4, 0x0256B9C9) + struct.pack(">HH", len(records), 0) + bytes(4)
    return head + struct.pack(">%dH" % len(starts), *starts) + bytes(at - 24 - 2 * len(starts)) + body


def hdr(sample, count, order=None):
    """<name>.hdr: u16 sample id, u16 take count, FFFFFFFF, the count again (LE) and two bytes, four zero bytes, then u16 BE offsets
    into the sth (12 bytes a take), then a trailer."""
    order = list(range(count)) if order is None else order
    return (struct.pack(">HH", sample, count) + b"\xff" * 4 + struct.pack("<H", count) + b"\x34\x05" + bytes(4)
            + struct.pack(">%dH" % count, *[12 * i for i in order]) + b"\x70\x70\x00")


def sth(*items):
    return b"".join(struct.pack(">III", *i) for i in items)


RATE = 0x0304AC44      # codec 3, channel config 1, 44100 Hz (low 18 bits)


class EvtTest(unittest.TestCase):
    def test_events_give_each_leaf_its_masks_and_sample(self):
        data = evt([record(0x8001, [leaf((1,), 0x8018), leaf((2,), 0x81CE)], conditions=(1,)),
                    record(0x8003, [leaf((1, 7), 0x8150), leaf((2, 7), 0x81D0), leaf((4, 7), 0x81F7)], conditions=(1, 2)),
                    record(9, [leaf((0x1000000, 8), 0x0042, weight=0x2D)])])
        self.assertEqual(e.evt_events(data), {
            0x8001: [((1,), 0x8018), ((2,), 0x81CE)],
            0x8003: [((1, 7), 0x8150), ((2, 7), 0x81D0), ((4, 7), 0x81F7)],
            9: [((0x1000000, 8), 0x0042)]})

    def test_a_leaf_weight_of_0x2d_or_0x39_is_read_alike(self):
        got = e.evt_events(evt([record(7, [leaf((1,), 5, weight=0x39), leaf((1,), 6, weight=0x2D)])]))
        self.assertEqual(got, {7: [((1,), 5), ((1,), 6)]})

    def test_an_event_with_no_leaf_is_an_empty_list(self):
        self.assertEqual(e.evt_events(evt([record(4, [])])), {4: []})

    def test_the_wrong_magic_is_refused(self):
        data = bytearray(evt([record(1, [leaf((1,), 2)])]))
        data[0] ^= 0xFF
        with self.assertRaises(ValueError):
            e.evt_events(bytes(data))

    def test_a_leaf_that_is_not_a_leaf_is_refused(self):
        data = bytearray(evt([record(1, [leaf((1,), 2)])]))
        at = bytes(data).index(b"\x39\x64\x04\x00")
        data[at + 1] = 0
        with self.assertRaises(ValueError):
            e.evt_events(bytes(data))

    def test_a_record_running_past_the_file_is_refused(self):
        data = evt([record(1, [leaf((1,), 2)])])
        with self.assertRaises(ValueError):
            e.evt_events(data[:-10])

    def test_the_condition_list_of_each_event_is_its_types_in_order(self):
        data = evt([record(0x8001, [leaf((1,), 0x8018)], conditions=(1,)),
                    record(0x8007, [leaf((1, 1), 5), leaf((6, 1), 6), leaf((6, 2), 7)], conditions=(2, 1)),
                    record(4, [], conditions=())])
        self.assertEqual(e.evt_conditions(data), {0x8001: [1], 0x8007: [2, 1], 4: []})

    def test_a_condition_that_is_not_ff_type_04_is_refused(self):
        data = bytearray(evt([record(1, [leaf((1,), 2)], conditions=(1,))]))
        data[bytes(data).index(b"\xff\x01\x04")] = 0
        with self.assertRaises(ValueError):
            e.evt_conditions(bytes(data))

    def test_a_livingworld_leaf_is_refused_not_guessed(self):
        data = bytearray(evt([record(1, [leaf((1, 2), 2)], conditions=(1, 2))]))
        data[bytes(data).index(b"\x39\x64\x04\x00") + 2] = 0x0C
        with self.assertRaises(ValueError):
            e.evt_events(bytes(data))

    def test_two_records_of_one_event_are_refused(self):
        with self.assertRaises(ValueError):
            e.evt_events(evt([record(1, [leaf((1,), 2)]), record(1, [leaf((1,), 3)])]))


class TakesTest(unittest.TestCase):
    def test_a_sample_s_takes_are_its_sth_records_in_hdr_order(self):
        s = sth((0, RATE, 0x40012000), (0x6A40, RATE, 0x4001A000), (0xF480, RATE, 0x40019000))
        self.assertEqual(e.takes(hdr(0x8208, 3), s, "x"),
                         [e.Take(0, RATE, 0x40012000), e.Take(0x6A40, RATE, 0x4001A000), e.Take(0xF480, RATE, 0x40019000)])

    def test_the_hdr_offsets_pick_the_records(self):
        s = sth((10, RATE, 1), (20, RATE, 2))
        self.assertEqual([t.offset for t in e.takes(hdr(1, 2, order=[1, 0]), s, "x")], [20, 10])

    def test_the_decoded_rate_and_sample_count(self):
        t = e.Take(0, RATE, 0x40012000 | 0x1000)
        self.assertEqual((t.hertz, t.frames), (44100, 0x13000))

    def test_the_sample_id_is_the_hdr_s_first_word(self):
        self.assertEqual(e.sample_id(hdr(0x8208, 2)), 0x8208)

    def test_a_take_count_the_sth_cannot_fill_is_refused(self):
        with self.assertRaises(ValueError):
            e.takes(hdr(1, 3), sth((0, RATE, 1), (4, RATE, 2)), "short")

    def test_an_hdr_without_its_marker_is_refused(self):
        bad = bytearray(hdr(1, 1))
        bad[4] = 0
        with self.assertRaises(ValueError):
            e.takes(bytes(bad), sth((0, RATE, 1)), "bad")

    def test_a_truncated_hdr_is_refused(self):
        with self.assertRaises(ValueError):
            e.takes(hdr(1, 4)[:20], sth(*[(0, RATE, 1)] * 4), "cut")

    def test_a_scripted_line_s_snr_header_is_one_take(self):
        self.assertEqual(e.snr_take(struct.pack(">II", RATE, 0x40012000)), e.Take(0, RATE, 0x40012000))
        with self.assertRaises(ValueError):
            e.snr_take(b"\x00" * 7)


FAKE = '''import sys, struct, wave
# stand-in for vgmstream-cli: -o OUT SNR (the .sns beside it); writes one mono 16-bit frame per body byte at the header's rate
args = sys.argv[1:]
out, snr = args[args.index("-o") + 1], args[-1]
head = open(snr, "rb").read()
body = open(snr[:-4] + ".sns", "rb").read()
rate = struct.unpack(">I", head[:4])[0] & 0x3FFFF
with wave.open(out, "wb") as w:
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(rate)
    w.writeframes(struct.pack("<H", head[7]) * len(body))
'''


class WavTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.fake = Path(self.tmp.name) / "fake_vgmstream.py"
        self.fake.write_text(FAKE, encoding="utf-8", newline="\n")
        self.cmd = [sys.executable, str(self.fake)]

    def frames(self, data):
        path = Path(self.tmp.name) / "got.wav"
        path.write_bytes(data)
        with wave.open(str(path)) as w:
            return w.getframerate(), w.getnframes()

    def test_wav_hands_vgmstream_the_take_s_header_and_its_body(self):
        dat = bytes(range(100))
        data = e.wav(dat, e.Take(20, RATE, 0x40000007), vgmstream=self.cmd)
        self.assertEqual(data[:4], b"RIFF")
        self.assertEqual(self.frames(data), (44100, 80))                  # the body runs from the take's offset to the .dat's end

    def test_wav_ends_a_take_at_the_next_one(self):
        data = e.wav(bytes(100), e.Take(20, RATE, 7), end=50, vgmstream=self.cmd)
        self.assertEqual(self.frames(data), (44100, 30))

    def test_a_take_outside_the_dat_is_refused(self):
        with self.assertRaises(ValueError):
            e.wav(bytes(10), e.Take(11, RATE, 1), vgmstream=self.cmd)

    def test_a_failing_vgmstream_is_refused_with_its_name_in_the_message(self):
        bad = Path(self.tmp.name) / "bad.py"
        bad.write_text("import sys\nsys.exit(3)\n", encoding="utf-8", newline="\n")
        with self.assertRaises(RuntimeError):
            e.wav(bytes(10), e.Take(0, RATE, 1), vgmstream=[sys.executable, str(bad)])

    def test_wav_needs_vgmstream_named(self):
        with self.assertRaises(RuntimeError) as ctx:
            e.wav(bytes(10), e.Take(0, RATE, 1), vgmstream=Path(self.tmp.name) / "nothing-here.exe")
        self.assertIn("vgmstream", str(ctx.exception))

    def test_a_take_that_vgmstream_decodes_to_nothing_is_refused(self):
        empty = Path(self.tmp.name) / "empty.py"
        empty.write_text("import sys\nargs = sys.argv[1:]\nopen(args[args.index('-o') + 1], 'wb').close()\n", encoding="utf-8", newline="\n")
        with self.assertRaises(RuntimeError):
            e.wav(bytes(10), e.Take(0, RATE, 1), vgmstream=[sys.executable, str(empty)])


class NamesTest(unittest.TestCase):
    def test_a_scripted_name_is_its_character_and_line(self):
        # 0x824a5a08: atoi(name[0:2]) is the character, atoi(name[3:7]) the id
        self.assertEqual(e.scripted_key("30_8575_PRO-CHRI_1"), (30, 8575))
        self.assertEqual(e.scripted_key("31_5049_TUT-JORD_2_360"), (31, 5049))
        with self.assertRaises(ValueError):
            e.scripted_key("nonsense")


def make_big(files):
    """A minimal EB BIG v3 archive (16-byte entries, no compression, one empty directory) holding {name: bytes}."""
    names = list(files)
    count = len(names)
    align = lambda n: (n + 15) & ~15           # noqa: E731
    record = 2 + max(len(n) for n in names) + 1
    compression_at = 48 + align(16 * count)
    index_size = compression_at + align(count)
    names_size = align(record * count) + 16
    start = align(index_size + names_size)
    entries, at, payload = b"", start, b""
    for n in names:
        entries += struct.pack(">IIII", at, len(files[n]), len(files[n]), 0)
        padded = files[n] + bytes(align(len(files[n])) - len(files[n]))
        payload += padded
        at += len(padded)
    head = struct.pack(">HHIHBBIIBB", 0x4542, 3, count, 0, 0, 0, index_size, names_size, record, 2).ljust(48, b"\0")
    table = (entries.ljust(align(16 * count), b"\0") + bytes(count)).ljust(index_size - 48, b"\0")
    name_block = b"".join(struct.pack(">H", 0) + n.encode().ljust(record - 2, b"\0") for n in names)
    return head + table + name_block.ljust(names_size, b"\0") + bytes(start - index_size - names_size) + payload


def bank_big(bank, events, samples):
    """<bank>speech.big: {sample: [(offset, body length), ...]} and the .dat each takes from."""
    members = {f"{bank}_Events.evt": events}
    hdrs, sths = {}, {}
    for number, (name, takes_) in enumerate(samples.items()):
        dat = bytes(range(256)) * 2
        hdrs[f"{name}.hdr"] = hdr(0x8000 + number, len(takes_))
        sths[f"{name}.sth"] = sth(*[(off, RATE, frames) for off, frames in takes_])
        members[f"{name}.dat"] = dat
    members[f"{bank}hdr.big"] = make_big(hdrs)
    members[f"{bank}sth.big"] = make_big(sths)
    return make_big(members)


class BigTest(unittest.TestCase):
    def test_the_test_archive_is_readable_by_the_real_reader(self):
        e.big_archive()
        from owned_game.big import BigArchive
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "x.big").write_bytes(make_big({"a.bin": b"abc", "long_name_1.bin": bytes(100)}))
            archive = BigArchive(Path(d) / "x.big")
            self.assertEqual([x.path for x in archive.entries], ["a.bin", "long_name_1.bin"])
            self.assertEqual([archive.read(x) for x in archive.entries], [b"abc", bytes(100)])


class SpeechBankTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        fake = self.dir / "fake_vgmstream.py"
        fake.write_text(FAKE, encoding="utf-8", newline="\n")
        self.cmd = [sys.executable, str(fake)]
        events = evt([record(0x8001, [leaf((1,), 0x8000), leaf((2,), 0x8001)], conditions=(1,)),
                      record(0x8007, [leaf((6, 2), 0x8002), leaf((1, 2), 0x9999)], conditions=(2, 1))])
        self.path = self.dir / "cameramanspeech.big"
        self.path.write_bytes(bank_big("cameraman", events, {"709_30_oob": [(0, 3), (40, 3), (90, 3)], "705_30_win_sc2": [(0, 5)],
                                                             "705_40_win_sc1": [(0, 5), (16, 5)]}))
        (self.dir / "work").mkdir()

    def bank(self):
        return e.SpeechBank(self.path, "cameraman", self.dir / "work")

    def test_the_bank_gives_its_events_conditions_and_samples(self):
        bank = self.bank()
        self.assertEqual(bank.events(), {0x8001: [((1,), 0x8000), ((2,), 0x8001)], 0x8007: [((6, 2), 0x8002), ((1, 2), 0x9999)]})
        self.assertEqual(bank.conditions(), {0x8001: [1], 0x8007: [2, 1]})
        self.assertEqual(bank.samples(), {0x8000: "709_30_oob", 0x8001: "705_30_win_sc2", 0x8002: "705_40_win_sc1"})

    def test_takes_of_a_sample_counts_its_takes_without_decoding(self):
        with mock.patch("ea_speech.wav", side_effect=AssertionError("decoded")):
            found = self.bank().takes_of("709_30_oob")
        self.assertEqual([t.offset for t in found], [0, 40, 90])
        with self.assertRaises(KeyError):
            self.bank().takes_of("nothing")

    def test_a_sample_decodes_one_wav_per_take_each_ending_where_the_next_begins(self):
        sizes = [len(w) for w in self.bank().wavs("709_30_oob", self.cmd)]
        frames = []
        for data in self.bank().wavs("709_30_oob", self.cmd):
            path = self.dir / "t.wav"
            path.write_bytes(data)
            with wave.open(str(path)) as w:
                frames.append(w.getnframes())
        self.assertEqual(len(sizes), 3)
        self.assertEqual(frames, [40, 50, 512 - 90])       # the last take runs to the end of the 512 byte .dat

    def test_wavs_is_lazy_so_only_one_take_is_held_at_a_time(self):
        it = self.bank().wavs("709_30_oob", self.cmd)
        self.assertEqual(iter(it), it)
        self.assertEqual(next(it)[:4], b"RIFF")

    def test_a_sample_the_bank_does_not_hold_is_refused(self):
        with self.assertRaises(KeyError):
            list(self.bank().wavs("nothing", self.cmd))

    def test_a_missing_bank_file_is_refused(self):
        with self.assertRaises(FileNotFoundError):
            e.SpeechBank(self.dir / "absent.big", "cameraman", self.dir / "work")

    def test_a_bank_missing_its_events_is_refused(self):
        (self.dir / "nobank.big").write_bytes(make_big({"cameramansth.big": b"", "cameramanhdr.big": b""}))
        with self.assertRaises(ValueError):
            e.SpeechBank(self.dir / "nobank.big", "cameraman", self.dir / "work").events()


class ScriptedLinesTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        fake = self.dir / "fake_vgmstream.py"
        fake.write_text(FAKE, encoding="utf-8", newline="\n")
        self.cmd = [sys.executable, str(fake)]
        resident = {f"{n}.snr": struct.pack(">II", RATE, 0x40000000 | f) for n, f in
                    (("30_8572_PRO-CHRI_1", 7), ("31_5706_TUT-JORD_1", 4), ("30_8572_PRO-CHRI_2", 9))}
        self.path = self.dir / "nis.big"
        self.path.write_bytes(make_big({"30_8572_PRO-CHRI_1.sns": bytes(7), "31_5706_TUT-JORD_1.sns": bytes(4),
                                        "30_8572_PRO-CHRI_2.sns": bytes(9), "nisresident.big": make_big(resident)}))
        (self.dir / "work").mkdir()

    def lines(self):
        return e.ScriptedLines(self.path, self.dir / "work")

    def test_the_variants_of_a_line_are_its_names_in_directory_order(self):
        self.assertEqual(self.lines().variants(), {(30, 8572): ["30_8572_PRO-CHRI_1", "30_8572_PRO-CHRI_2"], (31, 5706): ["31_5706_TUT-JORD_1"]})

    def test_a_variant_s_take_is_its_snr_header(self):
        self.assertEqual(self.lines().take("30_8572_PRO-CHRI_2"), e.Take(0, RATE, 0x40000009))
        with self.assertRaises(KeyError):
            self.lines().take("nothing")

    def test_a_variant_is_one_wav_of_its_sns(self):
        data = self.lines().wav("30_8572_PRO-CHRI_2", self.cmd)
        path = self.dir / "t.wav"
        path.write_bytes(data)
        with wave.open(str(path)) as w:
            self.assertEqual((w.getframerate(), w.getnframes()), (44100, 9))

    def test_a_name_with_no_sns_is_refused(self):
        resident = {"30_1234_X_1.snr": struct.pack(">II", RATE, 1)}
        (self.dir / "lonely.big").write_bytes(make_big({"nisresident.big": make_big(resident), "other.sns": b"x"}))
        with self.assertRaises(ValueError):
            e.ScriptedLines(self.dir / "lonely.big", self.dir / "work").variants()


if __name__ == "__main__":
    unittest.main()
