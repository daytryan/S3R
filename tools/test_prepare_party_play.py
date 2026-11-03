import struct
import unittest

import prepare_party_play as pp


def table(rows: dict, padding: int) -> bytes:
    """A language table as the pack ships it: header, (id, offset) index, pool descriptor, strings, padding."""
    pool = b"".join(text.encode("latin-1") + b"\0" for text in rows.values())
    index, offset = b"", 0
    for key, text in rows.items():
        index += struct.pack("<II", key, offset)
        offset += len(text) + 1
    header = struct.pack("<IIIII", 0, 0, len(rows), 28, 28 + len(index)) + b"\0" * 8
    return header + index + b"\0" * 8 + pool + b"\0" * padding


class PartyPlayTest(unittest.TestCase):
    def test_padded_tables_pair_by_id(self):
        labels = table({7: "ID_PARTY_PLAY", 9: "ID_PARTY_PLAYER"}, padding=3)
        english = table({9: "Player {0:d}", 7: "Party Play"}, padding=1)
        self.assertEqual(pp._table(labels), {7: "ID_PARTY_PLAY", 9: "ID_PARTY_PLAYER"})

        class Archive:
            entries = [type("E", (), {"path": pp.LABELS})(), type("E", (), {"path": pp.ENGLISH})()]

            def read(self, entry):
                return labels if entry.path == pp.LABELS else english
        self.assertEqual(pp.strings(Archive()), {"ID_PARTY_PLAY": "Party Play", "ID_PARTY_PLAYER": "Player {0:d}"})

    def test_movies_and_folder(self):
        self.assertEqual(pp.FOLDER, "party-play")
        self.assertEqual(sorted(pp.MOVIES), ["partyplay", "partyplay_start"])
        self.assertTrue(all(p.startswith("data/fe/source/screens/partyplay/") for p in pp.MOVIES.values()))


if __name__ == "__main__":
    unittest.main()
