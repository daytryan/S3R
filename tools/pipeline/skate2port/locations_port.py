"""Skate 1 / Skate 2 Locations-tab content for the prepared Challenge Map screen: each city's
own map picture and pins (the game's own pin art), and every spot's photo and description
for the info panel shown before Teleport, as the retail Skate 3 locations have.

Pins: world (x, z) -> map picture pixels = (x, z) * K + (OX, OY), fitted by overlapping each
picture's filled city shape with the converted city's top-down vertex footprint
(loc/register.py over loc/skate_points.py; IoU 0.76 Skate 2, 0.64 Skate 1). Spots outside the
picture (Skate 2's Big Event area) get no pin.

Writes challenge-map/images/assets/oldskate/*, registers them in runtime/challengemap.json
(backup .pre-oldskate) and writes runtime/locations-extra.json, which challenge_map.rs reads.
usage (numpy + Pillow): py locations_port.py REPO SKATE2_ROOT SKATE1_ROOT MALOOF_DLC_BIG ASSETS
"""
import json, shutil, sys
from pathlib import Path
import numpy as np
from PIL import Image, ImageFilter

repo, s2game, s1game, maloof, assets = map(Path, sys.argv[1:6])
here = Path(__file__).resolve().parent
sys.path[:0] = [str(repo), str(repo/'tools'), str(here), str(here/'loc')]
from s2big import Big
from s2lang import strings, table
from decode import decode

ROOT = assets/'private/challenge-map'
OUT = ROOT/'images/assets/oldskate'
NSV, S1 = 'ID_COMMON_NEW_SAN_VANELONA', '#Skate 1 San Vanelona'
# map picture pixel = world (x, z) * k + (ox, oy); Skate 1's 1024 px map is shown at 512
FIT = {'New_San_Vanelona': (0.15197, 152.3, 271.2), 'Skate_1_San_Vanelona': (0.25678 / 2, 443.9 / 2, 636.2 / 2)}
BACKDROP = (8, 11, 15, 255)  # the retail own-map pictures sit on near-black

s2screens = Big(s2game/'data/fe/screens.big')
s2photos = Big(s2game/'data/fe/locations.big')
s1screens = Big(s1game/'data/fe/screens.big')
dlc = Big(maloof)
s2text = strings(s2game)


def texture(big, path, index=None):
    """A texture of an RW4 dictionary: by position, by name, or (None) the largest."""
    images = decode(big, path)
    if index is None:
        return max((i for _, i in images), key=lambda i: i.width * i.height)
    return images[index][1] if isinstance(index, int) else next(i for n, i in images if n == index)


def s2photo(name):
    return texture(s2photos, f'data/fe/source/images/locations/ss_{name}.rx2')


def s1photo(name, focus=0.5):
    """A Skate 1 location photo (256x256 polaroid: white or empty frame): the picture inside, as a
    2:1 band centred at `focus` of its height."""
    image = texture(s1screens, f'data/fe/source/images/locations/{name}.rx2').convert('RGBA')
    a = np.asarray(image).astype(int)
    frame = (a[..., 3] < 128) | (a[..., :3].min(2) > 232) | (a[..., :3].max(2) < 14)
    rows = np.nonzero(frame.mean(1) < 0.6)[0]
    cols = np.nonzero(frame.mean(0) < 0.6)[0]
    x0, x1, y0, y1 = cols.min() + 3, cols.max() - 3, rows.min() + 3, rows.max() - 3
    band = (x1 - x0) / 2
    top = min(max(y0, y0 + focus * (y1 - y0) - band / 2), y1 - band)
    return image.crop((x0, int(top), x1, int(top + band)))


def s1loading(name):
    """A Skate 1 loading card (tilted on black): straightened to its own rectangle."""
    image = texture(s1screens, f'data/fe/source/images/loading/{name}.rx2').convert('RGB')
    a = np.asarray(image).astype(int).sum(2) > 30
    ys, xs = np.nonzero(a)
    pick = lambda score: (int(xs[score.argmax()]), int(ys[score.argmax()]))
    tl, tr, br, bl = pick(-(xs + ys)), pick(xs - ys), pick(xs + ys), pick(ys - xs)
    card = image.transform((512, 256), Image.Transform.QUAD, (*tl, *bl, *br, *tr), Image.Resampling.BICUBIC)
    return card.crop((6, 5, 506, 251))


# spot id -> (photo, description). Skate 2 photos are its own location shots (ss_<district>_...);
# Skate 1 has photos for its challenge locations and loading cards for the rest.
DETAILS = {
    'new_san_vanelona_z_pj03_backalleystart': (lambda: s2photo('pj_3_slappys'), "Tucked in behind the Stacks, the back alley is packed with rails, ledges and gaps nobody ever bothered to cap."),
    'new_san_vanelona_hiprampstart': (lambda: s2photo('svm_5_danny'), "The Big Event's monster hip. Point it down the roll-in and hang on, this one is all about going huge."),
    'new_san_vanelona_siderampstart': (lambda: s2photo('svm_5_danny'), "The Big Event side ramp. Blast off the kicker and keep your eyes on the landing."),
    'new_san_vanelona_community_pool_teleport_locator': (lambda: s2photo('ur_5_communitypool'), "The neighbourhood pool, drained and waiting. Carve the deep end and grind the coping before the lifeguard shows up."),
    'new_san_vanelona_do_drop_inn_teleport_locator': (lambda: s2photo('ur_4_dodropinn'), "The Do-Drop-Inn's pool is empty and the manager's out. Check in, drop in."),
    'new_san_vanelona_z_dt4_skatelinestart': (lambda: s2photo('dt_0_downtown'), "Hit the heart of Downtown and link the whole line together: stairs, rails and ledges for days."),
    'new_san_vanelona_freeskate_megacompound_locator': (lambda: s2photo('svm_5_megapark'), "Danny Way's Mega Compound. Huge ramps, huge gaps and the kind of air you only get up on the mountain."),
    'new_san_vanelona_megarampclassicstart': (lambda: s2photo('svm_5_danny'), "The classic Mega Ramp: roll in, clear the gap and launch the quarterpipe. Go big or go home."),
    'new_san_vanelona_megaramptechstart': (lambda: s2photo('svm_5_danny'), "The Mega Ramp's tech setup. Rails and ledges on the deck, and the gap is still waiting for you."),
    'new_san_vanelona_old_pool_teleport_locator': (lambda: s2photo('ur_14_oldpool'), "An old backyard pool, cracked and weathered. The kidney shape still carves like a dream."),
    'new_san_vanelona_freeskate_oldspillway': (lambda: s2photo('svm_15_spillwayold'), "The original San Van spillway. Steep walls, long lines and plenty of room to pump for speed."),
    'new_san_vanelona_party_pool_teleport_locator': (lambda: s2photo('ur_16_navarrette'), "Somebody's throwing a party and the pool is bone dry. Session it before the neighbours call it in."),
    'new_san_vanelona_pink_pool_teleport_locator1': (lambda: s2photo('ur_16_pinkypool'), "Pinky's Motel pool: pink, peeling and perfect for a session."),
    'new_san_vanelona_poop_deck_teleport_locator': (lambda: s2photo('wf_3_lighthouse'), "Down on the Waterfront by the lighthouse, the Poop Deck sits right on the water. Watch your step."),
    'new_san_vanelona_a_robspark': (lambda: s2photo('wf_2_lilypads'), "Rob Dyrdek's playground by the Waterfront, built for flow and full of fun obstacles."),
    'new_san_vanelona_school_pool_teleport_locator': (lambda: s2photo('pj_1_oliverhigh'), "G.E.D. High's pool has been drained for maintenance. That's what they think."),
    'new_san_vanelona_z_svm01_damstart': (lambda: s2photo('svm_3_dam'), s2text['ID_LOCATION_DESC_DAM']),
    'new_san_vanelona_a_startwaterfront_seawall': (lambda: s2photo('wf_0_waterfront'), "Where it all started in New San Vanelona: the Waterfront sea wall, with the whole city behind you."),
    'skate_2_center_freeskate_online_dlc_communitycenter': (lambda: s1photo('08_location_photo_communitycenter'), "The Community Center from the original San Vanelona, back for Skate 2's online sessions."),
    'skate_2_factory_freeskate_online_dlc_dyrdek': (lambda: s1photo('24_location_photo_clankityclank'), "Rob Dyrdek's warehouse, wall to wall with Dyrdek-approved obstacles."),
    'skate_2_maloof_skate_2_maloof': (lambda: texture(dlc, 'data/fe/source/images/locations/ss_dlc_01_maloof.rx2'), None),
    'skate_2_parkade_freeskate_online_dlc_parkade': (lambda: s1photo('03_location_photo_parkade'), "The Parkade: stacked levels of banks, rails and concrete, all of it skateable."),
    'skate_2_school_freeskate_online_dlc_school': (lambda: s1photo('10_location_photo_elementaryschool'), "Oliver Elementary after hours. Benches, handrails and stairs, and no hall monitors."),
    'skate_1_san_vanelona_freeskate_sanvanelona_art_gallery_locator': (lambda: s1photo('07_location_photo_artGallery'), "San Van's Art Gallery: marble ledges, sculptures and plenty of security. Worth the risk."),
    'skate_1_san_vanelona_freeskate_sanvanelona_cathedral_locator': (lambda: s1loading('06_loadingimage_online'), "The Old Town cathedral grounds: stone ledges and big sets right under the bell towers."),
    'skate_1_san_vanelona_freeskate_sanvanelona_city_hall_locator': (lambda: s1loading('21_loadingimage_online'), "City Hall in Old Town. Perfect marble, long ledges and absolutely no skating allowed."),
    'skate_1_san_vanelona_freeskate_sanvanelona_community_center_locator': (lambda: s1photo('08_location_photo_communitycenter'), "The Community Center plaza: benches, banks and a park built by skaters, for skaters."),
    'skate_1_san_vanelona_freeskate_sanvanelona_diy_park_locator': (lambda: s1photo('location_photo_DIY'), "A DIY spot the locals built themselves. Rough concrete and raw transitions."),
    'skate_1_san_vanelona_freeskate_sanvanelona_dyrdek_locator': (lambda: s1loading('00_loadingimage_dist'), "Rob Dyrdek's spot in the middle of Downtown: planters, ledges and plazas to flare on."),
    'skate_1_san_vanelona_freeskate_sanvanelona_elementary_school_locator': (lambda: s1photo('10_location_photo_elementaryschool'), "The elementary schoolyard. Picnic tables, stairs and handrails, and class is out."),
    'skate_1_san_vanelona_freeskate_sanvanelona_industrial_warehouse_locator': (lambda: s1loading('12_loadingimage_online'), "An old Old Town warehouse turned indoor park. Keep it quiet, keep it moving."),
    'skate_1_san_vanelona_freeskate_sanvanelona_library_locator': (lambda: s1photo('34_location_photo_library'), "The Downtown library. Big sets, long rails and a plaza that never closes."),
    'skate_1_san_vanelona_freeskate_sanvanelona_matrix_plaza_locator': (lambda: s1photo('01_location_photo_matrixplaza'), "Matrix Plaza's ledges and gaps, right in the heart of Downtown."),
    'skate_1_san_vanelona_freeskate_sanvanelona_mega_park_locator': (lambda: s1photo('14_location_photo_megacompound'), "The Mega Park, home of the mega ramp. Drop in if you dare."),
    'skate_1_san_vanelona_freeskate_sanvanelona_monument_locator': (lambda: s1loading('03_loadingimage_dist'), "The Monument up in the Res: banks, ledges and a view over the whole city."),
    'skate_1_san_vanelona_freeskate_sanvanelona_parkade_locator': (lambda: s1photo('03_location_photo_parkade'), "The Downtown Parkade. Floor after floor of banks and ramps, all the way to the roof."),
    'skate_1_san_vanelona_freeskate_sanvanelona_pj_mansion_locator': (lambda: s1photo('11_location_photo_pjmansion'), "PJ Ladd's mansion out in the Suburbs. A backyard full of ramps and nobody home."),
    'skate_1_san_vanelona_freeskate_sanvanelona_regs_locator': (lambda: s1loading('36_loadingimage_online'), "Regs skate shop out by the Res. Pick up some new gear, then hit the spots out front."),
    'skate_1_san_vanelona_freeskate_sanvanelona_skate_plaza_locator': (lambda: s1photo('05_location_photo_loweroldtown'), "Old Town's skate plaza: smooth ground and ledges made for lines."),
    'skate_1_san_vanelona_freeskate_sanvanelona_slappys_locator': (lambda: s1loading('26_loadingimage_online'), "Slappy's skate shop in Old Town, with a spot waiting right out back."),
    'skate_1_san_vanelona_freeskate_sanvanelona_urban_res_top_locator': (lambda: s1loading('20_loadingimage_online'), "The top of the Urban Res. Steep concrete walls and a long drop to the bottom."),
    'skate_1_san_vanelona_freeskate_sanvanelona_zen_plaza_locator': (lambda: s1loading('14_loadingimage_online'), "Zen Plaza, under the glass domes Downtown. Calm, clean and full of perfect ledges."),
}


def maloof_description():
    r = {e.path.lower(): e for e in dlc.entries}
    labels = table(dlc.read(r['data/fe/languages/labels/language_labels_maloof_skate2ng-dlc_maloof.bin']))
    values = table(dlc.read(r['data/fe/languages/english/language_english_maloof_skate2ng-dlc_maloof.bin']))
    return next(values[k] for k, label in labels.items() if label == 'ID_DLC_LOCATION_MALOOF_101_DESC')


def install(key, image, size=None):
    """RGBA file under images/assets/oldskate, registered by the file name the movie asks for."""
    image = image.convert('RGBA')
    if size:
        image = image.resize(size, Image.Resampling.LANCZOS)
    folder = OUT/key
    folder.mkdir(parents=True, exist_ok=True)
    rgba = folder/f'{key}.rgba'
    rgba.write_bytes(image.tobytes())
    thumbnails[key] = {'rgba': rgba.relative_to(ROOT).as_posix(), 'width': image.width, 'height': image.height}


def on_backdrop(image, lift=1.0):
    image = image.convert('RGBA')
    if lift != 1.0:
        r, g, b, a = image.split()
        image = Image.merge('RGBA', [c.point(lambda v: min(255, int(v * lift))) for c in (r, g, b)] + [a])
    out = Image.new('RGBA', image.size, BACKDROP)
    out.alpha_composite(image)
    return out


manifest_path = ROOT/'runtime/challengemap.json'
if not manifest_path.with_suffix('.json.pre-oldskate').exists():
    shutil.copy2(manifest_path, manifest_path.with_suffix('.json.pre-oldskate'))
manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
thumbnails = manifest['thumbnails']
for key in [k for k in thumbnails if thumbnails[k]['rgba'].startswith('images/assets/oldskate/')]:
    del thumbnails[key]
shutil.rmtree(OUT, ignore_errors=True)

# Maps: Skate 2's line-art city (brightened on the dark backdrop), Skate 1's painted city.
install('map_skate2_sanvan.rx2', on_backdrop(texture(s2screens, 'data/fe/source/images/map/map512.rx2'), 1.5))
install('map_skate1_sanvan.rx2', on_backdrop(texture(s1screens, 'data/fe/source/screens/mapping/maps/mainmap.rx2')), (512, 512))
# Pins: Skate 2's teleport marker (its minimap atlas), Skate 1's map-screen star pin.
install('pin_skate2', texture(s2screens, 'data/fe/source/images/icons/minimap.rx2', 'mini_teleport.Texture'))
pin1 = texture(s1screens, 'data/fe/source/screens/mapping/mapscreen.rx2', 0)
assert pin1.size == (32, 64), pin1.size
install('pin_skate1', pin1, (24, 48))  # its map is shown at half size; 3/4 keeps Downtown's pins apart

catalog = json.loads((assets/'private/teleports.json').read_text())['destinations']
markers, details = {}, {}
for d in catalog:
    if d['map'] in FIT and d.get('matrix'):
        k, ox, oy = FIT[d['map']]
        x, _, z = d['matrix'][3][:3]
        at = [round(x * k + ox, 1), round(z * k + oy, 1)]
        if 6 <= at[0] <= 506 and 6 <= at[1] <= 506:
            markers[d['id']] = at
    if d['id'] in DETAILS:
        photo, text = DETAILS[d['id']]
        key = f"photo_{d['id']}.rx2"
        install(key, photo(), (512, 256))
        details[d['id']] = [key, '#' + (text or maloof_description())]
missing = sorted(set(DETAILS) - set(details))
assert not missing, f'teleports missing for {missing}'
extra = {
    'version': 1,
    'places': {NSV: {'map': 'map_skate2_sanvan.rx2', 'pin': 'pin_skate2', 'anchor': [0.5, 0.5]},
               S1: {'map': 'map_skate1_sanvan.rx2', 'pin': 'pin_skate1', 'anchor': [0.5, 1.0]}},
    'markers': markers,
    'details': details,
}
(ROOT/'runtime/locations-extra.json').write_text(json.dumps(extra, indent=1) + '\n', encoding='utf-8')
manifest_path.write_text(json.dumps(manifest, separators=(',', ':')) + '\n', encoding='utf-8')
print('OLD_SKATE_LOCATIONS_READY pins', len(markers), 'details', len(details))
