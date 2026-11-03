from pathlib import Path, PureWindowsPath
import tempfile
from PIL import Image
from s2big import Big
from vendor.skate3_ui.rw4 import extract_textures


def decode(big, path):
    """[(texture name, RGBA image)] of an RW4 texture dictionary in a Skate 1/2 BIG."""
    e = next(e for e in big.entries if e.path.lower() == path.lower())
    with tempfile.TemporaryDirectory() as w:
        w = Path(w)
        (w/'s.rx2').write_bytes(big.read(e))
        return [(PureWindowsPath(t['name']).name, Image.frombytes('RGBA', (t['width'], t['height']), (w/t['rgba_file']).read_bytes()))
                for t in extract_textures(w/'s.rx2', w, force=True, decode_png=True)['textures'] if t.get('rgba_file')]
