"""py -3 tools/test_online_scoring.py: online_scoring.lookup / flag against synthetic collections."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from online_scoring import SCORING_MULTIPLIER as SM, flag, lookup  # noqa: E402

C = 1
records = {
    (C, 10): (0, {SM: b"\x01"}),      # main
    (C, 20): (10, {}),                 # inherits main's 1
    (C, 30): (10, {SM: b"\x00"}),     # framework override 0
    (C, 40): (99, {}),                 # parent not loaded: unset
    (C, 50): (60, {}), (C, 60): (50, {}),  # cycle
}
assert flag(lookup(records, C, 20, SM)) is True
assert flag(lookup(records, C, 30, SM)) is False
assert lookup(records, C, 40, SM) is None and flag(None) is False
assert lookup(records, C, 50, SM) is None
assert lookup(records, C, 77, SM) is None
print("ok")
