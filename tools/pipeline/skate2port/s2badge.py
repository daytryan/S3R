"""Skate 2 title logo (the Press Start screen's "skate" wordmark + "2") stamped as a tile badge.

The Edit Skater tile's own badge (owned tick / DLC plug) is a frame of the retail movie and
sits bottom-left, so the Skate 2 badge goes bottom-right of the thumbnail itself."""
import tempfile
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter
from s2big import Big

WIDTH = 58  # badge width on a 128 px thumbnail
MARGIN = 3


def logo_skate1(game):
    """Skate 1's title, "skate.": the series wordmark (from the Skate 2 disc) with the period
    cut from the wordmark's own paper texture. Skate 1's press-start logo is split into
    per-letter pieces placed by its movie, so the assembled wordmark is taken from Skate 2."""
    mark = logo(game, with_two=False)
    size = round(mark.height * 0.27)
    alpha = mark.getchannel('A')
    # the largest opaque patch of the paper (letter strokes are thinner than the dot), scaled up
    for patch in range(size, 4, -2):
        spot = next(((x, y) for y in range(0, mark.height - patch, 2) for x in range(0, mark.width - patch, 2)
                     if alpha.crop((x, y, x + patch, y + patch)).getextrema()[0] >= 250), None)
        if spot:
            break
    dot = mark.crop((*spot, spot[0] + patch, spot[1] + patch)).resize((size, size), Image.Resampling.LANCZOS)
    mask = Image.new('L', (size, size))
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1), radius=size // 4, fill=255)
    dot.putalpha(mask)
    gap = mark.height // 10
    out = Image.new('RGBA', (mark.width + gap + size, mark.height))
    out.alpha_composite(mark)
    out.alpha_composite(dot, (mark.width + gap, mark.height - size - mark.height // 14))
    return out


def logo(game, with_two=True):
    from vendor.skate3_ui.rw4 import extract_textures
    big = Big(f'{game}/data/fe/screens.big')
    entry = next(e for e in big.entries if e.path.lower() == 'data/fe/source/screens/main/pressstart.rx2')
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        (work/'source.rx2').write_bytes(big.read(entry))
        textures = extract_textures(work/'source.rx2', work, force=True, decode_png=True)['textures']

        def texture(name):
            t = next(t for t in textures if t['name'].endswith('\\' + name + '.Texture'))
            image = Image.frombytes('RGBA', (t['width'], t['height']), (work/t['rgba_file']).read_bytes())
            return image.crop(image.getbbox())
        skate, two = texture('10'), texture('5')
    if not with_two:
        return skate
    two = two.resize((round(two.width * skate.height / two.height), skate.height), Image.Resampling.LANCZOS)
    gap = skate.height // 8
    mark = Image.new('RGBA', (skate.width + gap + two.width, skate.height))
    mark.alpha_composite(skate)
    mark.alpha_composite(two, (skate.width + gap, 0))
    return mark


def badge(mark):
    """The logo on a dark translucent pill (legible over white garments), WIDTH px wide."""
    pad = 3
    inner = mark.resize((WIDTH - 2*pad, round((WIDTH - 2*pad) * mark.height / mark.width)), Image.Resampling.LANCZOS)
    size = (WIDTH, inner.height + 2*pad)
    out = Image.new('RGBA', size)
    ImageDraw.Draw(out).rounded_rectangle((0, 0, size[0]-1, size[1]-1), radius=size[1]//2, fill=(12, 12, 14, 190))
    shadow = Image.new('RGBA', inner.size, (0, 0, 0, 0))
    shadow.putalpha(inner.getchannel('A').filter(ImageFilter.GaussianBlur(1)))
    out.alpha_composite(shadow, (pad, pad + 1))
    out.alpha_composite(inner, (pad, pad))
    return out


def stamp(rgba_path, width, height, mark_badge):
    image = stamped(Image.frombytes('RGBA', (width, height), Path(rgba_path).read_bytes()), mark_badge)
    Path(rgba_path).write_bytes(image.tobytes())
    return image


def stamped(image, mark_badge):
    """The RGBA image with the badge bottom-right (stamp without the file)."""
    width, height = image.size
    image = image.copy()
    b = mark_badge if width == 128 else mark_badge.resize((round(mark_badge.width*width/128), round(mark_badge.height*width/128)))
    image.alpha_composite(b, (width - b.width - MARGIN*width//128, height - b.height - MARGIN*width//128))
    return image
