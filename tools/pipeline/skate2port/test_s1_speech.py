"""old_challenges speech (Skate 1 tutorial P1 task 4, research.md 4.6): (speaker, line, flags) -> the bank event's
matching leaves -> sample files, with the event's play chance. SKATE1_DISC (the extracted Skate 1 disc folder) runs the
disc test."""
import struct, unittest

from test_s1_progress import C, DISC


def evt(events):
    """<bank>Events.evt: 24-byte header, u16 event offsets (x4), then per event: id, two u16, leaf count, 0, flags, 0,
    condition count, u16 leaf offsets (x4 from the event), and leaves `39 64 04 00`, u32 LE n, u32 LE words, n masks,
    u16 sample id."""
    body, offsets = b'', []
    start = 24 + 2 * len(events) + (2 * len(events)) % 4
    for eid, leaves in events.items():
        head = 12 + 2 * len(leaves)
        head += -head % 4
        blobs = [b'\x39\x64\x04\x00' + struct.pack('<II', len(m), len(m) + 2) + struct.pack('>%dIH' % len(m), *m, sid)
                 + b'\0\0' for m, sid in leaves]
        at, offs = head, []
        for blob in blobs:
            offs.append(at // 4)
            at += len(blob)
        e = struct.pack('>HHHBBBBH', eid, 200, 500, len(leaves), 0, 0x10, 100, max(map(len, (m for m, _ in leaves))))
        e += struct.pack('>%dH' % len(offs), *offs)
        e += b'\0' * (head - len(e)) + b''.join(blobs)
        offsets.append((start + len(body)) // 4)
        body += e + b'\0' * (-len(e) % 4)
    head = struct.pack('>II4sIHHHH', 0x0315B007, 0, b'\x02\0\0\0', 0, len(offsets), 0, 100, 500)
    head += struct.pack('>%dH' % len(offsets), *offsets)
    return head + b'\0' * (start - len(head)) + body


class EventsTest(unittest.TestCase):
    def test_an_event_lists_each_leafs_masks_and_sample(self):
        got = C['evt_events'](evt({0x4147: [((4,), 0x41CE), ((1,), 0x41D0)], 0x40A1: [((), 0x4300)]}))
        self.assertEqual(got, {0x4147: [((4,), 0x41CE), ((1,), 0x41D0)], 0x40A1: [((), 0x4300)]})

    def test_a_leaf_matches_when_every_mask_meets_its_condition(self):
        match = C['leaf_matches']
        self.assertTrue(match((0x10000, 1), (0x10000, 1)))
        self.assertFalse(match((0x10000, 2), (0x10000, 1)))
        self.assertTrue(match((4,), ()))  # an event the switch passes no condition: every leaf
        self.assertFalse(match((1,), (0,)))


@unittest.skipUnless(DISC, 'set SKATE1_DISC to the extracted Skate 1 disc folder')
class DiscTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.v = C['Vault']()

    def resolve(self, *call, **kw):
        return C['resolve'](self.v, *call, **kw)

    def assertPlays(self, call, file, chance, **kw):
        got = self.resolve(*call, **kw)
        self.assertTrue(got and any(f.endswith(file) for f in got['files']), (call, got))
        self.assertEqual(got['chance'], chance)

    def test_the_window_lines_resolve_to_their_retail_files_and_chances(self):
        # 0x2044 is listed twice (100 %, then 50 %); the photographer loop 0x822c41f0 stops at the first match.
        self.assertPlays((29, 1003), 'Sketch_Pho1', 100)
        self.assertEqual(self.resolve(31, 906)['chance'], 25)
        self.assertPlays((24, 308), 'DemoSign_Shin_eng', 20)
        self.assertPlays((31, 1401, 1), 'C1401_Gen_Far', 70)
        self.assertPlays((31, 5100), '45_5100_Cam', 100)
        self.assertPlays((31, 314, 0x800000), 'C314_PostWin_Shin', 100)

    def test_unnamed_speech_fields_are_read_by_their_type(self):
        # P1 review: he_u1_film01 / he_u3_film01 hold line-low 702 and time-warning 705 in tSpeechInfo fields the
        # vault names only by hash (Hash_E6FE531FD83490B8, Hash_D8D76B15F453BE2C); a name test skipped both.
        for name in ('he_u1_film01', 'he_u3_film01'):
            calls = C['speech_calls']([], [C['speech_fields'](self.v, name)], [])
            self.assertLessEqual({(31, 702, 0), (31, 705, 0)}, calls, name)

    def test_lines_with_nothing_to_play_are_silent(self):
        self.assertIsNone(self.resolve(31, 1943))  # not in the cameraman table
        self.assertIsNone(self.resolve(31, 8195))  # no NIS line 45_8195
        self.assertIsNone(self.resolve(31, 1401, 0))  # no leaf of 0x4147 takes followee 0 (INFERRED)

    def test_the_cameraman_switch_picks_the_condition(self):
        self.assertEqual(self.resolve(31, 314)['files'], ['C314_PostWin_Shin'])  # 0x822c6c94: flags 0 -> 0x800000
        self.assertEqual(sorted(self.resolve(31, 708, rank=3)['files']),
                         ['C708_45_FlmFlTime_Am', 'C708_45_FlmFlTime_Ukn'])

    def test_a_swapped_generic_line_offers_both_events(self):
        files = self.resolve(24, 411)['files']  # 0x75 <-> 0xdb 50/50 (0x822c758c)
        self.assertTrue(any('411' in f for f in files) and any('410' in f for f in files), files)


if __name__ == '__main__':
    unittest.main()
