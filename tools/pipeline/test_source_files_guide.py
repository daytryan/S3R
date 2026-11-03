# tools/pipeline/test_source_files_guide.py
"""F17: the launcher links each source item to docs/source-files-guide.md#<id>; every id needs its anchor."""
import re, unittest
from pathlib import Path
from tools.pipeline.sources import ITEMS

GUIDE = Path(__file__).resolve().parents[2]/'docs'/'source-files-guide.md'


class Guide(unittest.TestCase):
    def test_every_item_has_its_anchor_and_section(self):
        text = GUIDE.read_text(encoding='utf-8')
        anchors = re.findall(r'<a id="([^"]+)"></a>\n## ', text)
        self.assertEqual(anchors, [item[0] for item in ITEMS])


if __name__ == '__main__':
    unittest.main()
