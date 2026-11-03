"""skate3_unlock_rules on the checked-in table (work/unlocks/s12_unlock_map.json, no install needed)."""
import json
import re
import unittest
from pathlib import Path

import skate3_unlock_rules as r

TABLE = Path(__file__).resolve().parents[1] / "docs/superpowers/specs/skate3-story-mode-reference/work/unlocks/s12_unlock_map.json"


class Generated(unittest.TestCase):
    def test_the_generated_table_is_sorted_and_complete(self):
        table = json.loads(TABLE.read_text(encoding="utf-8"))
        text = r.source(table)
        self.assertTrue(text.startswith(r.HEADER))
        tags = re.findall(r'^    \("([^"]+)", ', text, re.M)
        self.assertEqual(tags, sorted(tags))
        self.assertEqual(len(tags), len(set(tags)))
        for tag in table["tags"]:
            self.assertEqual(tags.count(tag), 1, tag)
        self.assertIn("pub const EVERYTHING_ONLY: &[&str] = &[];", text)
        self.assertNotIn("everything-only", text)
        for tag in ("s3:cac_dlc_unlock_test", "s3:cac_pack_dwgh_dlc_test"):
            self.assertIn('    ("%s", Rule::Hidden),' % tag, text)
        self.assertIn('    ("s2mq:dr_pepper", Rule::Rewards(&["bs_group_22"])),', text)
        self.assertIn('    ("s2:cac_throwback_pack_inventory", Rule::Start),', text)
        self.assertIn('    ("s2mq:fabio", Rule::Dlc("15")),', text)
        self.assertIn('    ("s2mq:isaak", Rule::Character("z_legend")),', text)


if __name__ == "__main__":
    unittest.main()
