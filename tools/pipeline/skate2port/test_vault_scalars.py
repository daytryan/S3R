"""Vault.scalar in old_tutorial.py (Skate 2) and old_challenges.py (Skate 1): an integer is as wide as its declared
type. vlt.convert exports an inline field (declared size <= 4) as its whole 4-byte slot, the value left-aligned: the
retail schemas declare Int8/UInt8 1 byte, Int16/UInt16 2, Int32/UInt32 and every enum 4, UInt64 8, and all 3133 inline
sub-4-byte slots in S2 ChallengeCore/skater and S1 skater have zero padding (peer-int16.md)."""
import sys, unittest
from pathlib import Path

here = Path(__file__).resolve().parent
repo = here.parents[2]


def lib(script, marker):
    """A script's library half, without its main (as old_online1.lib / old_hall_of_meat.tutorial_lib load them)."""
    ns = {'__name__': script[:-3] + '_lib', '__file__': str(here/script)}
    argv, sys.argv = sys.argv, [script, str(repo), str(here/'no-game'), str(here/'no-assets')]
    try:
        exec(compile((here/script).read_text(encoding='utf-8').split(marker)[0], str(here/script), 'exec'), ns)
    finally:
        sys.argv = argv
    return ns


CASES = [  # (type, exported data, value)
    ('Int16', '000F0000', 15),            # FreeskateGoalAdjustment
    ('Int16', 'FFF60000', -10),
    ('UInt16', 'FFF60000', 65526),
    ('Int8', 'FB000000', -5),
    ('UInt8', '42000000', 66),
    ('Int32', 'FFFFFFF6', -10),
    ('UInt32', '00010000', 65536),
    ('eChallengeTypes', '0000002A', 42),
    ('Int16', '000F', 15),                # stored in the .bin / an array item: exactly its own bytes
    ('UInt64', '0000000100000002', 0x100000002),
]


# Retail slots (survey of the discs' vaults) of the fields the exporters used to shift back by hand, and the value each
# exporter writes. The exporters now take scalar()'s value as-is, so these pin their output.
SKATE2_FIELDS = [
    ('Int16', '005A0000', 90),   # RoundTime: old_contests rounds[].time, old_online rounds[].RoundTime (90.0)
    ('Int16', '00020000', 2),    # AudioOutOfBoundsWarningTime: old_contests oob_warning, old_online
    ('Int8', '03000000', 3),     # NumToWin: old_contests num_to_win (1-UP)
    ('Int16', '00010000', 1),    # PlaceHigherToWin: old_death_race place_to_win
    ('Int16', '000A0000', 10),   # RaceTrickForTimeMultiplier: old_death_race trick_multiplier
    ('Int16', '00010000', 1),    # KnockoutPerLeg: old_death_race heat.knockout
    ('Int8', '05000000', 5),     # ABDListItemLimit: old_skate_game max_letters
]
SKATE1_FIELDS = [
    ('Int16', '000A0000', 10),   # ForfeitTimer: old_online1 self.ForfeitTimer (10.0)
    ('Int16', '00030000', 3),    # CompetitorNumber: old_online1
]


class Scalars(unittest.TestCase):
    def test_skate2_exported_fields(self):
        scalar = lib('old_tutorial.py', '\ndef db_vaults')['Vault'].scalar
        for t, data, want in SKATE2_FIELDS:
            self.assertEqual(scalar(None, t, data), want, data)

    def test_skate1_exported_fields(self):
        scalar = lib('old_challenges.py', '# ---------------------------------------------------------------- main')['Vault'].scalar
        for t, data, want in SKATE1_FIELDS:
            self.assertEqual(scalar(None, t, data), want, data)

    def check(self, scalar):
        for t, data, want in CASES:
            with self.subTest(type=t, data=data):
                self.assertEqual(scalar(None, t, data), want)

    def test_skate2_old_tutorial(self):
        self.check(lib('old_tutorial.py', '\ndef db_vaults')['Vault'].scalar)

    def test_skate1_old_challenges(self):
        self.check(lib('old_challenges.py', '# ---------------------------------------------------------------- main')
                   ['Vault'].scalar)


if __name__ == '__main__':
    unittest.main()
