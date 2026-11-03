# tools/pipeline/stages.py
"""The build: one stage per former hand-run step (the 10-03 inventory), with its inputs and outputs."""
import contextlib, json, os, shutil, sys, threading
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

ROOT = Path(getattr(sys, '_MEIPASS', Path(__file__).resolve().parents[2]))
S3_DLC = ('s3-dlc-sanvan', 's3-dlc-hawaii', 's3-dlc-maloof', 's3-dlc-afterdark', 's3-dlc-create',
          's3-dlc-blackbox', 's3-dlc-filmer', 's3-dlc-share', 's3-dlc-money')


@dataclass(frozen=True)
class Stage:
    name: str
    step: int        # the page's 8 visual steps: 0 check, 1 Skate 3, 2 DLC, 3 Skate 2 maps, 4 Skate 1, 5 wardrobe, 6 world, 7 finish
    inputs: tuple    # item ids from sources.ITEMS
    after: tuple     # stages that must finish first
    roots: tuple     # installation-relative paths this stage writes
    sources: tuple   # globs under tools/ that version this stage (versions.py). stages.py itself is never listed:
                     # versions.py hashes each stage's own code in it (and its inputs and roots) instead, so one stage's edit moves no other
    run: Callable


def assets(ctx):
    return ctx.install/'assets'


_TABLES = threading.Lock()  # the stage below makes them; a resumed consumer whose copy is gone makes them here, one at a time


def tables(ctx):
    out = ctx.work/'tables'
    with _TABLES:
        if not (out/'.complete').is_file():
            from tools.pipeline.derive import derive_tables
            derive_tables(ctx.disc('s2-disc'), ctx.disc('s1-disc'), out, env=ctx.env)
            (out/'.complete').write_text('')
    return out


def fresh_tables(ctx):
    """The tables stage derives afresh (it reruns when derive.py changes); a consumer reuses a complete copy."""
    (ctx.work/'tables/.complete').unlink(missing_ok=True)
    tables(ctx)


def skate3_base(ctx):
    from tools.asset_pipeline.customiser_setup import install
    install(ctx.disc('s3-disc')/'default.xex', ctx.base, ctx.game_exe,
            lambda text: ctx.progress('skate3-base', text), refresh=(ctx.base/'installation.json').is_file())


def audio_image(ctx):
    ctx.script('audio-image', 'build_audio_image.py', ctx.disc('s3-disc')/'default.xex',
               ctx.package('s3-tu3')/'default.xexp', assets(ctx)/'private/stock/audio-runtime-image')


def runtime_disc_files(ctx):
    """The disc files the game reads at runtime (audio/title.rs roots(): stock comes first), copied into stock."""
    src, dst = ctx.disc('s3-disc')/'data/audio', assets(ctx)/'private/stock/data/audio'
    if not src.is_dir():
        raise RuntimeError('the Skate 3 disc has no data/audio folder')
    for path in src.rglob('*'):
        out = dst/path.relative_to(src)
        if path.is_file() and not (out.is_file() and out.stat().st_size == path.stat().st_size):
            out.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, out)


# Skate 3 DLC maps. Recovered read-only from the 09-27/09-28 hand runs in the session transcripts
# (15fe1e83 l.190/205/285/1557, 21c97fe9 l.196/224, 6f7b5431 l.358/363/367/381, b3702a96 l.7482).
# (package item, .big inside the unpacked package, district stem, installed map name). map_job names its output after
# the district minus a leading 'DIST_' (the label); _publish_map renames it to the installed name.
S3_DLC_MAPS = (
    ('s3-dlc-maloof',    'maloof_money_cup_00000000.big', 'DIST_MaloofMoneyCupDLC',         'Maloof_Money_Cup_NYC'),
    ('s3-dlc-hawaii',    'dway_park_00000000.big',        'DLC_DW_MegaCompund',             'Danny_Way_Hawaiian_Dream'),
    ('s3-dlc-sanvan',    'play_00000000.big',             'DIST_AGPark',                    'Art_Gallery'),
    ('s3-dlc-sanvan',    'play_00000000.big',             'Dist_Zen',                       'Zen_Plaza'),
    ('s3-dlc-afterdark', 'player1p_00000000.big',         'DIST_SanitariumDLC',             'Sanatorium'),
    ('s3-dlc-create',    'creator_00000000.big',          'DIST_BackLotParkDLC',            'Back_Lot_Park'),
    ('s3-dlc-create',    'creator_00000000.big',          'DIST_DownTownSkateParkNightDLC', 'Downtown_Night_Park'),
)
# add_teleports.py DATA_DIR MAP SOURCE_BIG SPAWN_LOCATOR: (map, package item, .big, spawn locator).
S3_DLC_TELEPORTS = (
    ('Maloof_Money_Cup_NYC',     's3-dlc-maloof',    'maloof_money_cup_00000000.big', 'Z_MaloofMCNYC_Start'),
    ('Danny_Way_Hawaiian_Dream', 's3-dlc-hawaii',    'dway_park_00000000.big',        'freeskate_dannyway_megacompound_locator'),
    ('Art_Gallery',              's3-dlc-sanvan',    'play_00000000.big',             'freeskate_artgallery_locator'),
    ('Zen_Plaza',                's3-dlc-sanvan',    'play_00000000.big',             'freeskate_zenplazapark_locator'),
    ('Sanatorium',               's3-dlc-afterdark', 'player1p_00000000.big',         'Z_Sanitarium_Start'),
    ('Back_Lot_Park',            's3-dlc-create',    'creator_00000000.big',          'freeskate_backlotpark_locator'),
    ('Downtown_Night_Park',      's3-dlc-create',    'creator_00000000.big',          'teleport_dwtn_skpk_01_ToDwtn_locator_01'),
)
_FE = 'data/fe/source/images/'
# dlc_location_image.py REPO --each CHALLENGE_MAP_DIR BIG ARCHIVE_PATH ...: (package item, .big, path inside it).
S3_DLC_LOCATION_IMAGES = (
    ('s3-dlc-maloof',    'maloof_money_cup_00000000.big', _FE+'map/DLC_Maloof_Map_512.rx2'),
    ('s3-dlc-hawaii',    'dway_park_00000000.big',        _FE+'map/map_danny.rx2'),
    ('s3-dlc-sanvan',    'play_00000000.big',             _FE+'map/map_partypack.rx2'),
    ('s3-dlc-sanvan',    'play_00000000.big',             _FE+'map/screen_zen_park_empty.rx2'),
    ('s3-dlc-afterdark', 'player1p_00000000.big',         _FE+'map/map_afterdark.rx2'),
    ('s3-dlc-afterdark', 'player1p_00000000.big',         _FE+'locations/DLC_location_after_dark.rx2'),
    ('s3-dlc-create',    'creator_00000000.big',          _FE+'map/screen_backlot_park_empty.rx2'),
    ('s3-dlc-create',    'creator_00000000.big',          _FE+'map/screen_dtnight_park_empty.rx2'),
)


def MAP_JOB_ARGS(ctx, archive, label):
    """map_job.py arguments, the same shape install.py uses for the base maps, with the install as --stage."""
    return ['--archive', archive, '--stage', ctx.install, '--game-exe', ctx.game_exe,
            '--result', ctx.work/'worlds'/f'{label}.json']


def _publish_map(ctx, label, name):
    """The hand runs' step after map_job: rename to the installed name, drop the props note, record it in maps.json."""
    maps = ctx.install/'maps'
    for ext in ('skate', 'irradiance'):
        os.replace(maps/f'{label}.{ext}', maps/f'{name}.{ext}')
    (assets(ctx)/f'private/native-props/{label}-availability.json').unlink(missing_ok=True)  # no movable-prop catalog
    catalog_map(ctx, name)


def catalog_map(ctx, name):
    """Record a converted map in maps.json. The map stages run side by side, so the edit holds ctx.shared and the
    catalog keeps one order however they finish: setup's own maps, then MAP_ORDER (the order they ran one by one)."""
    from tools.asset_pipeline.setup_state import atomic_json, sha
    path = ctx.install/'maps.json'
    entry = {'name': name, 'path': f'maps/{name}.skate', 'sha256': sha(ctx.install/'maps'/f'{name}.skate')}
    with ctx.shared:
        catalog = [m for m in json.loads(path.read_text(encoding='utf-8')) if m['name'] != name] + [entry]
        catalog.sort(key=lambda m: MAP_ORDER.index(m['name']) if m['name'] in MAP_ORDER else -1)  # stable sort
        atomic_json(path, catalog)


def _link_or_copy(source, target):
    try:
        os.link(source, target)
    except OSError:  # another drive, or a filesystem without hardlinks
        shutil.copy2(source, target)


def s3_dlc_maps(ctx):
    worlds = ctx.work/'worlds'
    worlds.mkdir(parents=True, exist_ok=True)
    for item, big, district, name in S3_DLC_MAPS:
        label = district.removeprefix('DIST_')
        archive, source = worlds/f'world{district}.big', ctx.package(item)/big
        archive.unlink(missing_ok=True)
        _link_or_copy(source, archive)
        ctx.script('s3-dlc-maps', 'asset_pipeline/map_job.py', *MAP_JOB_ARGS(ctx, archive, label))
        archive.unlink()  # a hardlink costs nothing; a copy (other drive) would keep ~1.6 GB
        _publish_map(ctx, label, name)
    for name, item, big, spawn in S3_DLC_TELEPORTS:
        ctx.script('s3-dlc-maps', 'pipeline/converter/add_teleports.py', ctx.base, name, ctx.package(item)/big, spawn)
    # dlc_skies merges every package folder it is given. Hand it exactly the nine Skate 3 packages, never work/dlc itself,
    # where the Skate 2 stages unpack their own DLC. Its own folder (not the wardrobe's work/dlc-s3): they run side by side.
    skies = ctx.work/'dlc-s3-skies'
    shutil.rmtree(skies, ignore_errors=True)
    for item in S3_DLC:
        for big in ctx.package(item).glob('*.big'):
            (skies/item).mkdir(parents=True, exist_ok=True)
            _link_or_copy(big, skies/item/big.name)
    ctx.script('s3-dlc-maps', 'pipeline/dlc/dlc_skies.py', ROOT, ctx.disc('s3-disc'), skies, assets(ctx))
    shutil.rmtree(skies, ignore_errors=True)
    ctx.script('s3-dlc-maps', 'pipeline/dlc/dlc_location_image.py', ROOT, '--each', assets(ctx)/'private/challenge-map',
               *(a for item, big, path in S3_DLC_LOCATION_IMAGES for a in (ctx.package(item)/big, path)))  # one start (B2)


_CM = 'assets/private/challenge-map'
S3_DLC_MAP_ROOTS = (
    *(f'maps/{name}.{ext}' for *_, name in S3_DLC_MAPS for ext in ('skate', 'irradiance')),
    'maps.json', 'assets/private/teleports.json',
    # dlc_skies.py: <world stream>.rgba/.sun.rgba/.json per map, plus <installed name>.json
    *(f'assets/private/native-skies/{n}' for _, _, district, name in S3_DLC_MAPS
      for n in (f'{district.removeprefix("DIST_")}.rgba', f'{district.removeprefix("DIST_")}.sun.rgba',
                f'{district.removeprefix("DIST_")}.json', f'{name}.json')),
    'assets/private/native-backdrops/DLC_DW_MegaCompund.skate',  # Hawaiian Dream's scenery, the only DLC sky with any
    f'{_CM}/runtime/challengemap.json',
    *(f'{_CM}/images/assets/{path.removesuffix(".rx2")}' for *_, path in S3_DLC_LOCATION_IMAGES),
)

STAGES = [
    Stage('skate3-base', 1, ('s3-disc',), (), (), (), skate3_base),
    Stage('audio-image', 1, ('s3-disc', 's3-tu3'), ('skate3-base',),
          ('assets/private/stock/audio-runtime-image',), ('build_audio_image.py', 'lzx_native.rs'), audio_image),
    Stage('runtime-disc-files', 1, ('s3-disc',), ('skate3-base',),
          ('assets/private/stock/data/audio',), (), runtime_disc_files),
    Stage('s3-dlc-maps', 2, ('s3-disc', *S3_DLC), ('skate3-base',), S3_DLC_MAP_ROOTS,
          ('asset_pipeline/map*.py', 'pipeline/dlc/*.py', 'pipeline/converter/add_teleports.py'),
          s3_dlc_maps),
]


# --- Part 2 (Task 9): mod maps, the Skate 1 map, backdrops, wardrobe, world look, HUDs, Locations.
MOD_STREAM = 'PS3_GAME/USRDIR/data/content/world/stream'
# (map, --pres, --sim, --lightmap-downscale, add_teleports source under the stream folder, spawn locator).
# Recovered from the hand runs: 15fe1e83 l.900/1004/1917 (Maloof), l.1557 (teleport sources) and l.1948 (the loop over
# Center/Factory/Parkade/School, then New San Van with downscale 2 and spawn A_StartWaterFront_SeaWall). Checked against the
# user's install (maps.json, teleports.json sources, each .skate's spawn): all six agree.
MOD_MAPS = (
    ('New_San_Vanelona', 'DIST_NewSanVan',     None,           2, 'DIST_NewSanVan/DIST_NewSanVan_Sim.loc', 'A_StartWaterFront_SeaWall'),
    ('Skate_2_Maloof',   'DIST_Maloof_Proxy',  'DIST_Maloof',  1, '-', None),  # no authored locators: one destination at the spawn
    ('Skate_2_Center',   'DIST_Center_Proxy',  'DIST_Center',  1, 'DIST_Center/DIST_Center_Sim.loc', 'freeskate_online_DLC_communitycenter'),
    ('Skate_2_Factory',  'DIST_Factory_Proxy', 'DIST_Factory', 1, 'DIST_Factory/DIST_Factory_Sim.loc', 'freeskate_online_DLC_dyrdek'),
    ('Skate_2_Parkade',  'DIST_Parkade_Proxy', 'DIST_Parkade', 1, 'DIST_Parkade/DIST_Parkade_Sim.loc', 'freeskate_online_DLC_parkade'),
    ('Skate_2_School',   'DIST_School_Proxy',  'DIST_School',  1, 'DIST_School/DIST_School_Sim.loc', 'freeskate_online_DLC_School'),
)
# The Skate1Map mod is one plain BIG (checked 10-03). Only two folders are needed: the district stream, and the
# global_locators folder whose .loc files hold the 19 named destinations (teleports.json's source is that folder's name).
SKATE1MAP_EDAT = 'CUSTOMM000000000/custom_00000000.big.edat'
SKATE1_STREAM = 'data/content/world/stream/DIST_SanVanelona'
SKATE1_LOCATORS = 'data/content/global_locators/DLC_BAM/DIST_SanVanelona'
SKATE1_SPAWN = 'freeskate_sanvanelona_zen_plaza_locator'  # the installed map's spawn is this locator + 1 m (checked)
# The Skate 2 Maloof pack's BIG (locations_port.py reads its photo and description). Checked by unpacking the package.
S2_MALOOF_BIG = 'maloof_money_cup_xbox2.big'
# The 39 retail Skate 3 location photos in fedynamic.big on the disc: 09a3adc2 l.169.
LOCATION_PHOTO_DIR = 'data/fe/source/images/locations/'
LOCATION_PHOTOS = (
    'aletown allmart memorial crystalplaza hotel kube midtown_promenade parkade plaza_of_solitude rippon rosalita slappys '
    'tresoutta uptown_promenade 2nd_and_navy quarry drydocks factory_roofs ghetto hastings loading_docks miracle_bowl '
    'pipeworks spillway_entrance old_factory tanker maloof blackboxpark skate_school campus_entrance chancenter clocktower '
    'daly_estates stadium library peterson_pavilion super_ultramega_park carvatron observatory').split()


# The game's load check of a big map peaks well above its converter (10-08: 5.3 GB New San Vanelona, 4.3 GB Skate 1):
# the runner holds that much back while it runs (Context.reserve). The parks' checks stay under 1.1 GB.
CHECK_GB = {'New_San_Vanelona': 5.5, 'Skate_1_San_Vanelona': 4.5}


def convert_mod_map(ctx, stage, stream, name, pres, sim, downscale):
    args = ['--repo', ROOT, '--pres', stream/pres, '--name', name, '--install', ctx.install,
            '--game-exe', ctx.game_exe, '--work', ctx.work/'convert', '--lightmap-downscale', downscale]
    if sim:
        args += ['--sim', stream/sim]
    ctx.script(stage, 'pipeline/converter/convert_ps3.py', *args)
    with ctx.reserve(stage, CHECK_GB[name]) if name in CHECK_GB else contextlib.nullcontext():
        ctx.script(stage, 'pipeline/converter/convert_ps3.py', *args, '--check')  # the game never runs beside the converter
    catalog_map(ctx, name)  # add_teleports then refreshes its sha


def add_teleports(ctx, stage, name, source, spawn):
    ctx.script(stage, 'pipeline/converter/add_teleports.py', ctx.base, name, source, *([spawn] if spawn else []))


def convert_mod_maps(ctx, stage, maps):
    stream = ctx.item('mod-newsanvan')/MOD_STREAM
    for name, pres, sim, downscale, loc, spawn in maps:
        convert_mod_map(ctx, stage, stream, name, pres, sim, downscale)
        add_teleports(ctx, stage, name, loc if loc == '-' else stream/loc, spawn)


def mod_maps(ctx):  # New San Vanelona, the long one
    convert_mod_maps(ctx, 'mod-maps', MOD_MAPS[:1])


def skate2_parks(ctx):  # the five Skate 2 parks, beside it
    convert_mod_maps(ctx, 'skate2-parks', MOD_MAPS[1:])


def skate1_map(ctx):
    from tools.owned_game.big import BigArchive
    extracted = ctx.work/'skate1map'
    shutil.rmtree(extracted, ignore_errors=True)
    try:
        archive = BigArchive(ctx.item('mod-skate1map')/SKATE1MAP_EDAT)
        wanted = (SKATE1_STREAM + '/', SKATE1_LOCATORS + '/')
        archive.extract_entries([e for e in archive.entries if e.path.startswith(wanted)], extracted)
        # --pres is <stream>/<district>, as for the mod maps: SKATE1_STREAM already ends in the district folder
        convert_mod_map(ctx, 'skate1-map', (extracted/SKATE1_STREAM).parent, 'Skate_1_San_Vanelona', 'DIST_SanVanelona',
                        None, 2)
        add_teleports(ctx, 'skate1-map', 'Skate_1_San_Vanelona', extracted/SKATE1_LOCATORS, SKATE1_SPAWN)
    finally:
        shutil.rmtree(extracted, ignore_errors=True)  # ~3.3 GB of loose stream files


def s3_dlc_folder(ctx):
    """work/dlc-s3/<item>/*.big for exactly the nine Skate 3 packages. The clothing scripts glob */*.big, and work/dlc
    also holds Skate 2's packages, so they never see work/dlc itself."""
    view = ctx.work/'dlc-s3'
    shutil.rmtree(view, ignore_errors=True)
    for item in S3_DLC:
        for big in ctx.package(item).glob('*.big'):
            (view/item).mkdir(parents=True, exist_ok=True)
            _link_or_copy(big, view/item/big.name)
    return view


def wardrobe(stage, script, old_games):
    """dlc_clothing: REPO GAME DLC ASSETS. old_skate_clothing and pro_clothing add SKATE2_ROOT SKATE1_ROOT before ASSETS."""
    def run(ctx):
        games = [ctx.disc('s2-disc'), ctx.disc('s1-disc')] if old_games else []
        view = s3_dlc_folder(ctx)
        try:
            ctx.script(stage, script, ROOT, ctx.disc('s3-disc'), view, *games, assets(ctx))
        finally:
            shutil.rmtree(view, ignore_errors=True)
    return run


def stock_graphics(ctx):
    """The script wants the customiser set folder (its assets = parents[3]); the set id is in current.json."""
    base = assets(ctx)/'private/customisation'
    current = json.loads((base/'current.json').read_text(encoding='utf-8'))['set']
    ctx.script('stock-graphics', 'pipeline/dlc/stock_graphics.py', ROOT, ctx.disc('s3-disc'), base/'sets'/current)


def locations(ctx):
    cm = assets(ctx)/'private/challenge-map'
    big = ctx.disc('s3-disc')/'data/big/fedynamic.big'
    ctx.script('locations', 'pipeline/dlc/dlc_location_image.py', ROOT, '--each', cm,  # one start for all (B2)
               *(a for photo in LOCATION_PHOTOS for a in (big, f'{LOCATION_PHOTO_DIR}{photo}.rx2')))
    ctx.script('locations', 'pipeline/skate2port/locations_port.py', ROOT, ctx.disc('s2-disc'), ctx.disc('s1-disc'),
               ctx.package('s2-dlc-maloof')/S2_MALOOF_BIG, assets(ctx))


def simple(stage, script, *games, flags=()):
    """A script run as `script ROOT <discs...> ASSETS [flags]`, the shape every HUD, sky, camera and grading script takes."""
    return lambda ctx: ctx.script(stage, f'pipeline/skate2port/{script}', ROOT, *(ctx.disc(g) for g in games), assets(ctx), *flags)


def hud_skate1(ctx):
    """The HUD, then the tutorial window's screens into hud-skate1/tutorial (Skate 1 tutorial P1 task 10)."""
    for script in ('old_hud1.py', 'old_screens1.py'):
        simple('hud-skate1', script, 's1-disc')(ctx)


PARSERS = ('vendor/utt/**/*.py', 'vendor/**/*_native.rs', 'vendor/university/**/*.py')
CONVERTER = ('pipeline/converter/*.py', 'pipeline/converter/*_native.rs', 'asset_pipeline/map_writer.py', 'asset_pipeline/irradiance.py',
             'asset_pipeline/backdrop.py', 'asset_pipeline/setup_state.py', 'asset_pipeline/retail_material.py',
             'asset_pipeline/sky.py', 'asset_pipeline/environment.py', *PARSERS)
OLD_GAMES = ('pipeline/skate2port/s1*.py', 'pipeline/skate2port/s2*.py', 'pipeline/skate2port/skate1.py')
FIT = ('pipeline/skate2port/fit_clothing.py', 'pipeline/skate2port/fit_corrections.npz')  # the 10-04 clothing fit (apply())
CUSTOMISER = ('asset_pipeline/customis*.py', 'asset_pipeline/native_roster.py', 'asset_pipeline/retail_character.py',
              'asset_pipeline/marquee_assets.py', 'asset_pipeline/vlt.py', 'asset_pipeline/environment.py', 'asset_pipeline/names.txt',
              'extract_default_skater.py', 'pipeline/fanout.py', 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs', *PARSERS)
VAULT = ('asset_pipeline/vlt.py', 'asset_pipeline/environment.py', 'asset_pipeline/names.txt')
WARDROBE_ROOTS = ('assets/private/customisation', 'assets/private/create-skater/thumbs',
                  'assets/private/create-skater/runtime/createskater.json')
OLD_MAPS = ('New_San_Vanelona', 'Skate_2_Center', 'Skate_2_Factory', 'Skate_2_Maloof', 'Skate_2_Parkade', 'Skate_2_School',
            'Skate_1_San_Vanelona')  # the converted Skate 1 / Skate 2 maps; old_skies.py writes one sky for each
MAP_FILES = lambda names: tuple(f'maps/{n}.{ext}' for n in names for ext in ('skate', 'irradiance'))
TELEPORT_ROOTS = ('maps.json', 'assets/private/teleports.json')  # every map stage edits these: runner.SHARED
MAP_ORDER = (*(name for *_, name in S3_DLC_MAPS), *(m[0] for m in MOD_MAPS), 'Skate_1_San_Vanelona')

STAGES += [
    # The three map stages run side by side (their only common files are TELEPORT_ROOTS, edited under a lock).
    Stage('mod-maps', 3, ('mod-newsanvan',), ('skate3-base',),
          (*MAP_FILES(m[0] for m in MOD_MAPS[:1]), *TELEPORT_ROOTS), (*CONVERTER, 'asset_pipeline/teleports.py'), mod_maps),
    Stage('skate2-parks', 3, ('mod-newsanvan',), ('skate3-base',),
          (*MAP_FILES(m[0] for m in MOD_MAPS[1:]), *TELEPORT_ROOTS), (*CONVERTER, 'asset_pipeline/teleports.py'), skate2_parks),
    Stage('skate1-map', 4, ('mod-skate1map',), ('skate3-base',),
          (*MAP_FILES(['Skate_1_San_Vanelona']), *TELEPORT_ROOTS), (*CONVERTER, 'asset_pipeline/teleports.py'), skate1_map),
    # The Skate 2 Edit Skater room: Skate 2's own DIST_petes, placed where Skate 3's room stands (brief 10-31).
    Stage('backdrops', 3, ('s2-disc',), ('skate3-base',), ('assets/private/native-backdrops/CreateSkater_Skate2.skate',),
          ('pipeline/skate2port/old_room.py', 'pipeline/skate2port/s2big.py', *CONVERTER),
          lambda ctx: ctx.script('backdrops', 'pipeline/skate2port/old_room.py', ROOT, ctx.disc('s2-disc'),
                                 assets(ctx)/'private/native-backdrops')),
    Stage('dlc-clothing', 5, ('s3-disc', *S3_DLC), ('skate3-base',), WARDROBE_ROOTS,
          ('pipeline/dlc/dlc_clothing.py', *CUSTOMISER), wardrobe('dlc-clothing', 'pipeline/dlc/dlc_clothing.py', False)),
    Stage('old-clothing', 5, ('s3-disc', *S3_DLC, 's2-disc', 's1-disc'), ('dlc-clothing',), WARDROBE_ROOTS,
          ('pipeline/skate2port/old_skate_clothing.py', 'pipeline/skate2port/families.py', 'pipeline/skate2port/cas_tiles.py',
           'pipeline/skate2port/tiles_native.rs',
           *OLD_GAMES, *FIT, *CUSTOMISER),
          wardrobe('old-clothing', 'pipeline/skate2port/old_skate_clothing.py', True)),
    Stage('pro-clothing', 5, ('s3-disc', *S3_DLC, 's2-disc', 's1-disc'), ('old-clothing',), WARDROBE_ROOTS,
          ('pipeline/dlc/pro_clothing.py', 'pipeline/dlc/pro_fit_corrections.npz', 'pipeline/skate2port/tiles_native.rs', 'asset_pipeline/optional_content.py', 'mixamo_to_skate/glb.py', *OLD_GAMES, *FIT, *CUSTOMISER), wardrobe('pro-clothing', 'pipeline/dlc/pro_clothing.py', True)),
    # Redundant on a fresh build (customisation_library.prepare indexes the logos itself); kept as the hand step it was.
    Stage('stock-graphics', 5, ('s3-disc',), ('pro-clothing',), WARDROBE_ROOTS,
          ('pipeline/dlc/stock_graphics.py', *CUSTOMISER), stock_graphics),
    # The user's "Everyone Is Welcome Here" rainbow deck, painted over the player's own Canada deck texture.
    Stage('rainbow-deck', 5, ('s3-disc',), ('stock-graphics',), WARDROBE_ROOTS,
          ('pipeline/dlc/rainbow_deck.py', 'pipeline/skate2port/cas_tiles.py', 'pipeline/skate2port/tiles_native.rs', 'pipeline/fanout.py', 'mixamo_to_skate/glb.py', 'asset_pipeline/setup_state.py'),
          lambda ctx: ctx.script('rainbow-deck', 'pipeline/dlc/rainbow_deck.py', ROOT, assets(ctx))),
    Stage('old-skies', 6, ('s2-disc', 's1-disc'), ('mod-maps', 'skate2-parks', 'skate1-map'),
          tuple(f'assets/private/native-skies/{n}{ext}' for n in OLD_MAPS for ext in ('.json', '.rgba', '.sun.rgba')),
          ('pipeline/skate2port/old_skies.py', *OLD_GAMES, *VAULT, *PARSERS), simple('old-skies', 'old_skies.py', 's2-disc', 's1-disc')),
    # New San Vanelona's sea: Skate 2's global DIST_Water, which the mod's stream lacks (brief 10-29).
    Stage('old-sea', 6, ('s2-disc',), ('skate3-base',), ('assets/private/native-backdrops/New_San_Vanelona.skate',),
          ('pipeline/skate2port/old_sea.py', 'pipeline/skate2port/s2big.py', 'asset_pipeline/backdrop.py',
           'asset_pipeline/sky.py', 'asset_pipeline/map_writer.py', *PARSERS), simple('old-sea', 'old_sea.py', 's2-disc')),
    Stage('old-cameras', 6, ('s2-disc', 's1-disc'), ('skate3-base',),
          tuple(f'assets/private/camera-styles/{game}/{f}' for game in ('skate2', 'skate1')
                for f in ('collections.json', 'Default_cameragraph.stategraph')),
          ('pipeline/skate2port/old_cameras.py', 'pipeline/skate2port/stategraph.py', *OLD_GAMES, *VAULT),
          simple('old-cameras', 'old_cameras.py', 's2-disc', 's1-disc')),
    # Skate 2's own Edit Skater (clips, spots, cameras) for the Skate 2 room, placed by old_room's move.
    Stage('old-cas', 6, ('s2-disc',), ('skate3-base',), ('assets/private/cas-skate2',),
          ('pipeline/skate2port/old_cas.py', 'pipeline/skate2port/old_room.py', *OLD_GAMES, *VAULT, *CONVERTER),
          lambda ctx: ctx.script('old-cas', 'pipeline/skate2port/old_cas.py', ROOT, ctx.disc('s2-disc'), assets(ctx))),
    # No argument runs both games: the optional s2|s1 only restricts which maps are rewritten.
    Stage('old-grading', 6, ('s2-disc', 's1-disc'), ('skate3-base',), ('assets/private/colour-grading',),
          ('pipeline/skate2port/old_grading.py', *OLD_GAMES, *VAULT, *PARSERS), simple('old-grading', 'old_grading.py', 's2-disc', 's1-disc')),
    # The full run (no --minimap): trick display, district card, phone list and minimap.
    Stage('hud-skate2', 6, ('s2-disc',), ('skate3-base',), ('assets/private/hud-skate2',),
          ('pipeline/skate2port/old_hud.py', *OLD_GAMES, 'prepare_hud.py', 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs', *VAULT),
          simple('hud-skate2', 'old_hud.py', 's2-disc')),
    Stage('hud-skate1', 6, ('s1-disc',), ('skate3-base',), ('assets/private/hud-skate1',),
          ('pipeline/skate2port/old_hud1.py', 'pipeline/skate2port/old_screens1.py', 'pipeline/skate2port/old_screens.py',
           'pipeline/skate2port/old_hud.py', 'prepare_core_menu.py', 'prepare_hud.py', *OLD_GAMES, 'vendor/skate3_ui/**/*.py',
           'vendor/skate3_ui/*_native.rs'), hud_skate1),
    # after s3-dlc-maps too: both edit challengemap.json, and in this order
    Stage('locations', 6, ('s3-disc', 's2-disc', 's1-disc', 's2-dlc-maloof'), ('s3-dlc-maps', 'mod-maps', 'skate2-parks', 'skate1-map'),
          (f'{_CM}/images/assets/oldskate', f'{_CM}/runtime/locations-extra.json', f'{_CM}/runtime/challengemap.json',
           *(f'{_CM}/images/assets/{LOCATION_PHOTO_DIR}{n}' for n in LOCATION_PHOTOS)),
          ('pipeline/skate2port/locations_port.py', 'pipeline/skate2port/loc/*.py', 'pipeline/dlc/dlc_location_image.py',
           *OLD_GAMES, 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs'), locations),
]


# --- Part 3 (Task 10): the Skate 2 tutorial, the Skate 1 challenges, ambience and the online stages.
S2_PORT = 'pipeline/skate2port/'
S2_SCREENS = 26  # old_screens.py's MOVIES: it exits 0 even when a movie fails, so the stage counts screens/runtime/*.json
# What every screen export (old_hud, old_screens and the movies built on them) imports.
SCREEN_EXPORT = (S2_PORT + 'old_hud.py', S2_PORT + 'old_screens.py', 'prepare_core_menu.py', 'prepare_hud.py',
                 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs')
S2_DATA = (*OLD_GAMES, *VAULT, *PARSERS, S2_PORT + 'old_tutorial.py', 'asset_pipeline/dynamic_props.py')
UNLUAC_BATCH = ('unluac_batch.py', 'UnluacBatch.class')  # unluac in one JVM: names.py, the prefetches, skate3_online.py
DERIVED = ('pipeline/derive.py', 'pipeline/derive/**/*.py', *UNLUAC_BATCH)  # the tables (names, ICE parameters, enums) the exports read
# (folder under skate2-online, script). Each runs into its own scratch folder; its output is that folder's contents
# (hand-run 10-03: `OUT=$scratch; old_death_race.py ... $OUT`, then copied). The scripts say so in their docstrings.
S2_ONLINE_PARTS = (('death-race', 'old_death_race.py'), ('contests', 'old_contests.py'),
                   ('skate', 'old_skate_game.py'), ('hall-of-meat', 'old_hall_of_meat.py'))
ONLINE_JSON = f'{_CM}/online/online.json'
# old_online.py's icons (MODES), registered in online.json and copied beside Skate 3's.
S2_ONLINE_ICONS = tuple(f'{_CM}/online/icons/skate2_{n}.rgba' for n in
                        ('mini_1up', 'mini_besttrick', 'mini_deathrace', 'mini_hom', 'mini_jam', 'mini_skate', 'mini_spotfound'))
# The tables old_online.py / old_online1.py patch in place (they add the Best Trick / Jam / Spot Race folder labels to the
# `language` of every manifest that has the challenge type names). The user's install has exactly these, each with the
# scripts' .pre-skate2online / .pre-skate1online backup beside it; homscoring.json (Skate 3's own panel, built last) has none.
LANGUAGE_FILES = (f'{_CM}/runtime/challengemap.json',
                  *(f'assets/private/online-ui/runtime/{n}.json' for n in ('friends', 'hud_online_eventlog2', 'leaderboardhud', 'sk8popup')))


def _replace_tree(src, dst, skip=()):
    """dst becomes a copy of src (minus the top-level names in `skip`)."""
    shutil.rmtree(dst, ignore_errors=True)
    shutil.copytree(src, dst, ignore=lambda folder, names: [n for n in names if n in skip and Path(folder) == Path(src)])


def _install_new(src, dst):
    """Copy a tree into dst: manifests (runtime/) are replaced, payloads must be new or byte-identical (as hall-of-meat-s3)."""
    import filecmp
    for path in sorted(p for p in Path(src).rglob('*') if p.is_file()):
        rel = path.relative_to(src)
        target = Path(dst)/rel
        if target.exists() and rel.parts[0] != 'runtime' and not filecmp.cmp(path, target, shallow=False):
            raise RuntimeError(f'{target} exists and differs from the new export')
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)


def fe_sounds(ctx, stage, cache, screens):
    """Skate 2's front-end one-shots, `skate3rust.exe --s2-fe-sounds S2 VAULT SCREENS` (no window): the wavs of every
    FE_SOUND_ key the converted screens in `screens` play. The vault is the one that export cached under work/cache/<cache>."""
    ctx.run(stage, [ctx.game_exe, '--s2-fe-sounds', ctx.disc('s2-disc'), ctx.work/'cache'/cache/'skater-collections.json', screens])


def skate2_tutorial(ctx):
    """old_tutorial.py deletes its output folder before writing, so it runs into scratch and the result is copied. It runs
    old_tutorial_dmos.py itself at the end (its G8 note); old_tutorial_gate.py then re-runs it on the finished folder with
    Slappy's gate added (user 10-28: the gate had only ever been run by hand). The rest write into the finished folder
    and none of them clears it. old_screens.py caches the Skate 2 vault old_indicators.py reads."""
    tables(ctx)
    s2, a, out = ctx.disc('s2-disc'), assets(ctx), assets(ctx)/'private/skate2-tutorial'
    scratch = ctx.work/'tutorial'
    shutil.rmtree(scratch, ignore_errors=True)
    try:
        ctx.script('skate2-tutorial', S2_PORT + 'old_tutorial.py', ROOT, s2, scratch)
        _replace_tree(scratch/'private/skate2-tutorial', out)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    ctx.script('skate2-tutorial', S2_PORT + 'old_tutorial_gate.py', ROOT, s2, a)
    ctx.script('skate2-tutorial', S2_PORT + 'old_screens.py', ROOT, s2, a)
    made = len(list((out/'screens/runtime').glob('*.json')))
    if made != S2_SCREENS:
        raise RuntimeError(f'old_screens.py made {made} of {S2_SCREENS} screens (see logs/skate2-tutorial.log)')
    fe_sounds(ctx, 'skate2-tutorial', 'screens-cache', out/'screens')
    for script in ('old_indicators.py', 'old_tutorial_cast.py'):
        ctx.script('skate2-tutorial', S2_PORT + script, ROOT, s2, a)


def skate1_challenges(ctx):
    tables(ctx)
    # old_challenges rmtrees challenges-skate1/ and writes the tutorial.json old_scenes1 checks its scenes against.
    for script in ('old_challenges.py', 'old_scenes1.py', 'old_challenges_cast.py', 'old_ai1.py', 'old_indicator1.py'):
        ctx.script('skate1-challenges', S2_PORT + script, ROOT, ctx.disc('s1-disc'), assets(ctx))


def ONLINE_UI_ARGS(ctx, work):
    return ['--game', ctx.disc('s3-disc'), '--output', work, '--collections',
            assets(ctx)/'private/stock/skater-collections.json', '--install', assets(ctx)]


def online_ui(ctx):
    work = ctx.work/'online-ui'
    shutil.rmtree(work, ignore_errors=True)
    try:
        ctx.script('online-ui', 'prepare_online_ui.py', *ONLINE_UI_ARGS(ctx, work))
    finally:
        shutil.rmtree(work, ignore_errors=True)


def replay_ui(ctx):
    """The Replay Editor: RE3 + replay_help2 from Skate.Create, skatereel_tags/sk8popup, timeline and watermark textures."""
    work = ctx.work/'replay-ui'
    shutil.rmtree(work, ignore_errors=True)
    try:
        ctx.script('replay-ui', 'prepare_replay_ui.py', '--game', ctx.disc('s3-disc'),
                   '--dlc', ctx.package('s3-dlc-create')/'creator_00000000.big', '--output', work,
                   '--collections', assets(ctx)/'private/stock/skater-collections.json', '--install', assets(ctx))
    finally:
        shutil.rmtree(work, ignore_errors=True)
        shutil.rmtree(ctx.work/'replay-ui-game', ignore_errors=True)


def online_movies(ctx):
    """Item 30b's Skate 3 online movies (online_chyron) from the disc, drawn on the online layer."""
    work = ctx.work/'online-movies'
    shutil.rmtree(work, ignore_errors=True)
    try:
        ctx.script('online-movies', 'prepare_online_movies.py', '--disc', ctx.disc('s3-disc'), '--output', work,
                   '--collections', assets(ctx)/'private/stock/skater-collections.json', '--install', assets(ctx))
    finally:
        shutil.rmtree(work, ignore_errors=True)


def ONLINE_MAP_ARGS(ctx, packages, work):
    return ['--disc', ctx.disc('s3-disc'), '--dlc', packages, '--assets', assets(ctx), '--out', work, '--install']


def online_challenge_map(ctx):
    """online_challenge_map.py reads the nine STFS packages itself (every file under --dlc that starts CON/LIVE/PIRS), so it
    gets a folder of exactly those, never the unpacked work/dlc."""
    packages, work = ctx.work/'dlc-s3-packages', ctx.work/'online-map'
    for folder in (packages, work):
        shutil.rmtree(folder, ignore_errors=True)
    packages.mkdir(parents=True, exist_ok=True)
    try:
        for item in S3_DLC:
            _link_or_copy(ctx.item(item), packages/item)
        ctx.script('online-challenge-map', 'online_challenge_map.py', *ONLINE_MAP_ARGS(ctx, packages, work))
    finally:
        for folder in (packages, work):
            shutil.rmtree(folder, ignore_errors=True)


def online_skate2(ctx):
    """old_online.py exports into one folder and --install copies it into the installation, so the two must differ (it
    copies rows.json and intros onto themselves otherwise). The export folder is also where it looks for Skate 2's minimap
    icons, so those are copied in first. The four mode scripts each write their own scratch folder (hand-run 10-03)."""
    tables(ctx)
    s2, a = ctx.disc('s2-disc'), assets(ctx)
    private, export, parts = a/'private', ctx.work/'online-s2', ctx.work/'online-s2-parts'
    for folder in (export, parts):
        shutil.rmtree(folder, ignore_errors=True)
    try:
        icons = 'private/hud-skate2/assets/data/fe/source/images/icons/minimap'
        shutil.copytree(a/icons, export/icons)
        ctx.script('online-skate2', S2_PORT + 'old_online.py', ROOT, s2, export, '--install', a)
        extra = {'death-race': [private/'teleports.json'],  # checks the NSV teleports are the BAM_Sim.loc frame
                 'contests': ['--voting', private/'hud-skate2/online']}  # the voting screen, rebuilt with its imports merged
        for sub, script in S2_ONLINE_PARTS:
            ctx.script('online-skate2', S2_PORT + script, ROOT, s2, parts/sub, *extra.get(sub, []))
        fe_sounds(ctx, 'online-skate2', 'skate-cache', parts/'skate/screens')
        fe_sounds(ctx, 'online-skate2', 'hom-cache', parts/'hall-of-meat')  # it scans <folder>/runtime for the FE_SOUND_ keys
        for sub, _ in S2_ONLINE_PARTS:
            _replace_tree(parts/sub, private/'skate2-online'/sub, skip=('hud',))
        _install_new(parts/'contests/hud', private/'hud-skate2/online')  # the voting screen's new files
    finally:
        for folder in (export, parts):
            shutil.rmtree(folder, ignore_errors=True)


def online_skate1(ctx):
    """old_online1.py and old_online1_modes.py share one export folder (the second reads what the first wrote) and each
    --install copies it into the installation. old_online_hud.py then edits the installed Skate 2 and Skate 1 online HUDs."""
    tables(ctx)
    s1, s2, a = ctx.disc('s1-disc'), ctx.disc('s2-disc'), assets(ctx)
    export = ctx.work/'online-s1'
    shutil.rmtree(export, ignore_errors=True)
    try:
        for script in ('old_online1.py', 'old_online1_modes.py'):
            ctx.script('online-skate1', S2_PORT + script, ROOT, s1, export, '--install', a)
        ctx.script('online-skate1', S2_PORT + 'old_online_hud.py', ROOT, s2, s1, a, '--results', '--dpo', '--language',
                   '--intro-overlays')
        # roundresults_lb's challenge_end_success and core_nav_up/down, the intro overlays' spot_found and
        # photo_flythrough: the layer plays from its own sfx (audit I4).
        fe_sounds(ctx, 'online-skate1', 'online-hud-cache', a/'private/hud-skate2/online')
    finally:
        shutil.rmtree(export, ignore_errors=True)


def hall_of_meat_s3(ctx):
    """--install is a flag here (it copies the new files into assets/private/online-ui)."""
    staging = ctx.work/'hom3'
    shutil.rmtree(staging, ignore_errors=True)
    # Its manifest is its own: on a rebuild over an earlier install, online-skate2/1 have already patched the old copy
    # (and backed it up), so the script would find it "differs". Start from none, as a fresh build does.
    runtime = assets(ctx)/'private/online-ui/runtime'
    for old in runtime.glob('homscoring.json*'):
        old.unlink()
    try:
        ctx.script('hall-of-meat-s3', S2_PORT + 'old_hall_of_meat3.py', ROOT, ctx.disc('s3-disc'), assets(ctx), staging, '--install')
    finally:
        shutil.rmtree(staging, ignore_errors=True)


def _skate3_online(ctx, stage, out, starts, scoring=False):
    """skate3_online.py exports the Skate 3 online modes into `out` (--starts: see its help), online_scoring.py the
    scoring.json beside them. Java/unluac come from fetch_tools through ctx.env (SKATE_JAVA / SKATE_UNLUAC) for
    scripts/*.lua. online_scoring.py reads every file under --dlc that starts CON/LIVE/PIRS, so both get a folder of
    exactly the nine packages, as online-challenge-map does; skate3_online.py reads the DLC objective rows (OTS etc.)."""
    packages = ctx.work/'dlc-s3-scoring'
    shutil.rmtree(packages, ignore_errors=True)
    packages.mkdir(parents=True)
    try:
        for item in S3_DLC:
            _link_or_copy(ctx.item(item), packages/item)
        ctx.script(stage, 'skate3_online.py', '--disc', ctx.disc('s3-disc'), '--dlc', packages, '--assets', assets(ctx),
                   '--out', out, '--starts', starts)
        if scoring:
            ctx.script(stage, 'online_scoring.py', '--disc', ctx.disc('s3-disc'), '--dlc', packages, '--out', out/'scoring.json')
    finally:
        shutil.rmtree(packages, ignore_errors=True)


def online_s3_export(ctx, stage):
    """work/online-s3: every mode and hud/ without online.json's starts, and scoring.json. Everything they read but
    online.json is skate3-base's, so online-skate3-export makes it from the start; online-skate3 remakes it when a
    resumed build has lost it."""
    out = ctx.work/'online-s3'
    if not (out/'.complete').is_file():
        shutil.rmtree(out, ignore_errors=True)
        out.mkdir(parents=True)
        _skate3_online(ctx, stage, out, 'skip', scoring=True)
        (out/'.complete').write_text('')
    return out


def online_skate3_export(ctx):
    (ctx.work/'online-s3/.complete').unlink(missing_ok=True)  # a rerun of this stage exports afresh
    online_s3_export(ctx, 'online-skate3-export')


def online_skate3(ctx):
    """Once the online challenge map has written online.json: the modes that read it (Domination's Location) are
    exported again over the early export, then every <mode>/, hud/ and scoring.json go into assets/private/skate3-online."""
    out = online_s3_export(ctx, 'online-skate3')
    _skate3_online(ctx, 'online-skate3', out, 'only')
    dest = assets(ctx)/'private/skate3-online'
    dest.mkdir(parents=True, exist_ok=True)
    for path in sorted(out.iterdir()):
        if path.is_dir():
            shutil.rmtree(dest/path.name, ignore_errors=True)
            shutil.copytree(path, dest/path.name)
        elif path.name != '.complete':
            shutil.copy2(path, dest/path.name)
    shutil.rmtree(out)


S3_ONLINE = ('skate3_online.py', 'online_scoring.py', 'online_challenge_map.py', 'build_skate2_image.py', 'build_audio_image.py',
             'lzx_native.rs', 'owned_game/big.py', 'vendor/university/tools/vanilla_map_extraction/tools/skate3_streams.py',
             'pipeline/skate2port/old_tutorial.py', 'pipeline/skate2port/old_indicators.py', 'prepare_online_ui.py',
             'prepare_core_menu.py', 'asset_pipeline/vlt.py',
             'asset_pipeline/names.txt', 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs', *UNLUAC_BATCH)
STAGES += [
    # From the start, beside skate3-base: the Skate 1/2 exe images and tables the tutorial, challenges and online stages read.
    Stage('tables', 3, ('s2-disc', 's1-disc'), (), (),
          (*DERIVED, 'build_skate2_image.py', 'build_audio_image.py', 'lzx_native.rs', S2_PORT + 's2big.py'), fresh_tables),
    Stage('skate2-tutorial', 6, ('s2-disc', 's1-disc'), ('hud-skate2', 'tables'), ('assets/private/skate2-tutorial',),
          (S2_PORT + 'old_tutorial*.py', S2_PORT + 'tutorial_recovery.md', S2_PORT + 'old_indicators.py', *SCREEN_EXPORT, *S2_DATA, *CUSTOMISER, *DERIVED),
          skate2_tutorial),
    Stage('skate1-challenges', 4, ('s1-disc', 's2-disc'), ('hud-skate1', 'tables'), ('assets/private/challenges-skate1',),
          (S2_PORT + 'old_challenges.py', S2_PORT + 'old_scenes1.py', S2_PORT + 'old_tutorial.py',
           S2_PORT + 'old_challenges_cast.py', S2_PORT + 'old_ai1.py', S2_PORT + 'old_indicator1.py', 'skate3_ai_paths.py',
           *OLD_GAMES, *VAULT,
           *CUSTOMISER, 'asset_pipeline/character_glb.py',
           *UNLUAC_BATCH, 'build_skate2_image.py', 'build_audio_image.py', 'lzx_native.rs', *DERIVED), skate1_challenges),
    Stage('skate2-ambience', 6, ('s2-disc', 's1-disc'), ('skate3-base',), ('assets/private/skate2-audio', 'assets/private/skate1-audio'),
          (S2_PORT + 'old_ambience.py', *OLD_GAMES, *VAULT, 'owned_game/refpack.py'),
          simple('skate2-ambience', 'old_ambience.py', 's2-disc', 's1-disc')),
    # Skate 1/2/3 traffic (single player, each game's own city maps): road network, census, vehicle GLBs.
    Stage('skate2-traffic', 6, ('s2-disc', 's1-disc', 's3-disc'), ('skate3-base',), ('assets/private/traffic',),
          (S2_PORT + 'old_traffic.py', S2_PORT + 'old_ambience.py', *OLD_GAMES, *CUSTOMISER, 'owned_game/refpack.py',
           'owned_game/big.py', 'asset_pipeline/audio_world.py', 'vendor/skate3_anim/rx2_skeleton.py',
           'asset_pipeline/character_glb.py'),
          simple('skate2-traffic', 'old_traffic.py', 's2-disc', 's1-disc', 's3-disc')),
    # The Skate 2 physics switch's data: the few hundred entries of nine Skate 2 archives that the Skate 2 Rust port
    # (vendor/skate2-rust-port) reads, copied byte for byte with a manifest the host checks at boot. Not tested in play.
    Stage('skate2-physics', 6, ('s2-disc',), ('skate3-base',), ('assets/private/skate2-physics',),
          (S2_PORT + 'old_physics.py', S2_PORT + 'physics_big.py', S2_PORT + 'old_physics_entries.tsv', 'pipeline/xdvdfs.py'),
          simple('skate2-physics', 'old_physics.py', 's2-disc')),
    Stage('online-ui', 7, ('s3-disc',), ('skate3-base',), ('assets/private/online-ui',),
          ('prepare_online_ui.py', 'prepare_core_menu.py', 'prepare_create_skater.py', 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs'), online_ui),
    Stage('replay-ui', 7, ('s3-disc', 's3-dlc-create'), ('online-ui',), ('assets/private/replay-ui',),
          ('prepare_replay_ui.py', 'prepare_online_ui.py', 'prepare_core_menu.py', 'prepare_create_skater.py',
           'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs'), replay_ui),
    Stage('online-movies', 7, ('s3-disc',), ('online-ui',), ('assets/private/online-movies',),
          ('prepare_online_movies.py', 'prepare_online_ui.py', 'prepare_core_menu.py', 'prepare_create_skater.py',
           'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs'), online_movies),
    Stage('online-challenge-map', 7, ('s3-disc', *S3_DLC), ('online-ui', 's3-dlc-maps', 'locations'), (f'{_CM}/online',),
          ('online_challenge_map.py', 'online_challenge_map_reference.json', 'asset_pipeline/vlt.py', 'asset_pipeline/names.txt',
           'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs'), online_challenge_map),
    # Writes into online.json, the manifests' language tables and the Skate 2 online HUD, which the stages in `after` wrote
    # first; the roots are the exact shared files, so they are receipted again after this stage.
    # ponytail: the two Skate 2 DLC packs are required inputs that no script reads yet (their items are on the disc); they
    # sit here, the first Skate 2 stage that would read them.
    Stage('online-skate2', 7, ('s2-disc', 's3-disc', 's1-disc', 's2-dlc-filmer', 's2-dlc-throwback'),
          ('online-challenge-map', 'skate2-tutorial', 'hud-skate2'),
          ('assets/private/skate2-online', 'assets/private/hud-skate2/online', ONLINE_JSON, *S2_ONLINE_ICONS, *LANGUAGE_FILES),
          (S2_PORT + 'old_online.py', S2_PORT + 'old_death_race.py', S2_PORT + 'old_contests.py', S2_PORT + 'old_skate_game.py',
           S2_PORT + 'old_hall_of_meat.py', S2_PORT + 'old_indicators.py', S2_PORT + 'game_tabs.py', 'online_challenge_map.py',
           *SCREEN_EXPORT, *S2_DATA, *DERIVED),
          online_skate2),
    Stage('online-skate1', 7, ('s1-disc', 's2-disc', 's3-disc'), ('online-skate2', 'hud-skate1'),
          ('assets/private/skate1-online', 'assets/private/hud-skate1/online', 'assets/private/hud-skate2/online', ONLINE_JSON,
           *LANGUAGE_FILES),
          (S2_PORT + 'old_online1*.py', S2_PORT + 'old_online_hud.py', S2_PORT + 'old_challenges.py', S2_PORT + 'old_hud1.py',
           S2_PORT + 'old_screens1.py', 'build_skate2_image.py', S2_PORT + 'game_tabs.py', 'online_challenge_map.py',
           *SCREEN_EXPORT, *OLD_GAMES, *VAULT, *PARSERS, *DERIVED),
          online_skate1),
    # Last: it installs new files into online-ui, whose language tables the two stages above have just patched.
    Stage('hall-of-meat-s3', 7, ('s3-disc',), ('online-ui', 'online-skate1'), ('assets/private/online-ui',),
          (S2_PORT + 'old_hall_of_meat3.py', *SCREEN_EXPORT, *OLD_GAMES, *VAULT), hall_of_meat_s3),
    # retail-audit C1: the Skate 3 online modes' data and the per-challenge scoring gate, exported from the start (into
    # work/online-s3, no roots); online-skate3 re-exports the modes that read online.json and installs it all. Beside
    # online-skate2/1: it only reads online.json/challengemap.json rows they never touch, through whole_json (retries a
    # half-rewritten file).
    Stage('online-skate3-export', 7, ('s3-disc', *S3_DLC), ('skate3-base',), (), S3_ONLINE, online_skate3_export),
    Stage('online-skate3', 7, ('s3-disc', *S3_DLC), ('online-challenge-map', 'online-skate3-export'),
          ('assets/private/skate3-online',), S3_ONLINE, online_skate3),
]


# The power-on sequence (crates/skate-game/src/boot.rs): ESRB, the EA logo, Press Start, popups 23/263/302, the new-career
# welcome, team-name prompt, first team member and Coach Frank screens, the movie skip prompt and loading tag, and the boot
# movies with their audio (vgmstream, SKATE_VGMSTREAM from fetch_tools through ctx.env). Also Edit Skaters / Import Skater
# (team_select.rs) and runtime/team.json: the team logos (createacharacter.big), photo backgrounds and preset skaters
# (miscload.big SavedRecipe0-9). It reads the disc's fedynamic.big, miscload.big, createacharacter.big and data/movies,
# and the stock collections skate3-base installs.
def BOOT_ARGS(ctx, work):
    return ['--game', ctx.disc('s3-disc'), '--output', work, '--collections',
            assets(ctx)/'private/stock/skater-collections.json', '--install', assets(ctx)]


def boot(ctx):
    work = ctx.work/'boot'
    shutil.rmtree(work, ignore_errors=True)
    try:
        ctx.script('boot', 'prepare_boot.py', *BOOT_ARGS(ctx, work))
    finally:
        shutil.rmtree(work, ignore_errors=True)


STAGES += [
    Stage('boot', 1, ('s3-disc',), ('skate3-base',), ('assets/private/boot',),
          ('prepare_boot.py', 'prepare_core_menu.py', 'prepare_create_skater.py', 'prepare_hud.py', 'prepare_online_ui.py', 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs',
           'extract_default_skater.py', 'owned_game/*.py', 'asset_pipeline/fast_refpack.py'),
          boot),
]


# PlayStation pads' button glyphs (crates/skate-game/src/apt_scene.rs PS_GLYPHS): the PS3 set in the New San Van mod's
# miscboot.big (40x30 PNGs, installed as-is: the game decodes PNG). Start and Select stand in for Options and Create.
PAD_GLYPHS_BIG = 'PS3_GAME/USRDIR/data/big/miscboot.big'
PAD_GLYPHS_DIR = 'data/webkit/Content/images/Buttons/PS3/'


def pad_glyphs(ctx):
    from tools.owned_game.big import BigArchive
    out = assets(ctx)/'private/pad-glyphs'
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    archive = BigArchive(ctx.item('mod-newsanvan')/PAD_GLYPHS_BIG)
    names = set()
    for entry in archive.entries:
        if entry.path.startswith(PAD_GLYPHS_DIR) and entry.path.endswith('.png'):
            name = entry.path[len(PAD_GLYPHS_DIR):-4].removeprefix('button_')
            (out/f'{name}.png').write_bytes(archive.read(entry))
            names.add(name)
    missing = {'A', 'B', 'X', 'Y', 'Start', 'Back', 'L_Bumper', 'R_Bumper', 'L_Trigger', 'R_Trigger'} - names
    if missing:
        raise ValueError(f'miscboot.big lacks the PS3 button glyphs {sorted(missing)}')


STAGES += [
    Stage('pad-glyphs', 3, ('mod-newsanvan',), ('skate3-base',), ('assets/private/pad-glyphs',), ('owned_game/big.py',),
          pad_glyphs),
]


# Party Play (crates/skate-game/src/party_play.rs): the San Van Party Pack's setup screen and Press-A card, its string
# table and its audio zone file. The movies import the disc's controls and fonts; prepare_party_play.py --install
# replaces assets/private/party-play.
PARTY_PACK_BIG = 'play_00000000.big'


def party_play(ctx):
    work = ctx.work/'party-play'
    shutil.rmtree(work, ignore_errors=True)
    try:
        ctx.script('party-play', 'prepare_party_play.py', '--game', ctx.disc('s3-disc'),
                   '--dlc', ctx.package('s3-dlc-sanvan')/PARTY_PACK_BIG, '--output', work,
                   '--collections', assets(ctx)/'private/stock/skater-collections.json', '--install', assets(ctx))
    finally:
        shutil.rmtree(work, ignore_errors=True)


STAGES += [
    Stage('party-play', 2, ('s3-disc', 's3-dlc-sanvan'), ('skate3-base',), ('assets/private/party-play',),
          ('prepare_party_play.py', 'prepare_core_menu.py', 'prepare_hud.py', 'prepare_create_skater.py',
           'prepare_online_ui.py', 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs'), party_play),
]


# Retail front-end screens the crossbar opens (crates/skate-game/src/retail_ui.rs, retail-audit F): Trick Guide, Extras
# (Credits, Licenses; its Videos play the `boot` stage's movies), Video Calibration and the movie subtitles, with the
# subtitle, credits and licence data. Disc movies (fedynamic.big included), merged with their controls.
def retail_screens(ctx):
    work = ctx.work/'retail-screens'
    shutil.rmtree(work, ignore_errors=True)
    try:
        ctx.script('retail-screens', 'prepare_retail_screens.py', '--game', ctx.disc('s3-disc'), '--output', work,
                   '--collections', assets(ctx)/'private/stock/skater-collections.json', '--install', assets(ctx))
    finally:
        shutil.rmtree(work, ignore_errors=True)


STAGES += [
    Stage('retail-screens', 2, ('s3-disc',), ('skate3-base',), ('assets/private/retail-screens',),
          ('prepare_retail_screens.py', 'prepare_core_menu.py', 'prepare_create_skater.py', 'prepare_online_ui.py',
           'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs'), retail_screens),
]


def weather_audio(ctx):
    """The finished CC0 rain and thunder mix (tools/weather_audio, made by prepare_weather_audio.py from the user's
    Freesound downloads) ships with the tools, since no player owns those recordings. Copied in whole, swapped by rename."""
    source, out = ROOT/'tools/weather_audio', assets(ctx)/'weather-audio'
    if not (source/'manifest.json').is_file():
        raise FileNotFoundError(f'{source} has no manifest.json: the release is missing the weather audio')
    temp = out.with_name('weather-audio.new')
    shutil.rmtree(temp, ignore_errors=True)
    shutil.copytree(source, temp, ignore=shutil.ignore_patterns('__pycache__'))
    shutil.rmtree(out, ignore_errors=True)
    temp.rename(out)


STAGES += [
    Stage('weather-audio', 6, (), ('skate3-base',), ('assets/weather-audio',), ('weather_audio/*',), weather_audio),
]


# Story mode (docs/superpowers/specs/2026-10-20-skate3-story-mode-stage1-design.md): tools/skate3_career.py exports the retail Skate 3 career
# (career.json, strings.json, the progression and challenge Lua, the stage-1 rows, speech, cutscenes, cast models, menu movies and the Follow
# the Leader paths) into assets/private/skate3-career, which the game's career layer reads. It reads the disc, the nine DLC packages (their
# rows and strings: a folder of exactly those, as the online stages make), java and unluac (SKATE_JAVA, SKATE_UNLUAC) and vgmstream
# (SKATE_VGMSTREAM) through ctx.env, and the install's language table and stock collections (--assets). The export is made in the work folder and
# replaces the old folder whole. Not tested in play.
SKATE3_CAREER = ('skate3_career.py', 'skate3_career_cast.py', 'ea_speech.py', 'skate3_ai_paths.py', 'prepare_retail_screens.py',
                 'prepare_create_skater.py', 'prepare_hud.py', 'asset_pipeline/character_glb.py', 'asset_pipeline/optional_content.py',
                 'vendor/skate3_anim/*.py', *CUSTOMISER, *S3_ONLINE)


def skate3_career(ctx):
    work, packages = ctx.work/'skate3-career', ctx.work/'dlc-s3-career'
    dest = assets(ctx)/'private/skate3-career'
    new = dest.with_name('skate3-career.new')
    folders = (work, packages, new)
    for folder in folders:
        shutil.rmtree(folder, ignore_errors=True)
    packages.mkdir(parents=True)
    try:
        for item in S3_DLC:
            _link_or_copy(ctx.item(item), packages/item)
        ctx.script('skate3-career', 'skate3_career.py', '--disc', ctx.disc('s3-disc'), '--dlc', packages, '--assets', assets(ctx),
                   '--out', work)
        shutil.move(str(work), str(new))
        shutil.rmtree(dest, ignore_errors=True)
        new.rename(dest)
    finally:
        for folder in folders:
            shutil.rmtree(folder, ignore_errors=True)


def story_mode(env=os.environ, flags=ROOT/'tools/pipeline/build_flags.json'):
    """Whether this build makes the story mode export (user 10-22): a release made without scripts/Build-Release.ps1
    -StoryMode leaves it out, as its game leaves the career out. SKATE_STORY_MODE while the release is built; the
    bundled build_flags.json in a player's setup tool; neither (a dev checkout): on."""
    if 'SKATE_STORY_MODE' in env:
        return env['SKATE_STORY_MODE'] != '0'
    return bool(json.loads(flags.read_text(encoding='utf-8-sig'))['story_mode']) if flags.is_file() else True


if story_mode():
    STAGES += [
        Stage('skate3-career', 1, ('s3-disc', *S3_DLC), ('skate3-base',), ('assets/private/skate3-career',), SKATE3_CAREER,
              skate3_career),
    ]


def game_sounds(ctx):
    """A first play has its sounds at once (user 11-02): the game decodes the banks it starts with into its sound cache
    (LOCALAPPDATA, not the install). Never fails the build: without it those banks decode on the first play, as before."""
    try:
        ctx.run('game-sounds', [ctx.game_exe, '--warm-audio', assets(ctx)])
    except (OSError, RuntimeError):
        if ctx.cancelled:
            raise


# Last: it reads what the sound stages write (audio-image, runtime-disc-files, the ambience and traffic banks).
STAGES += [Stage('game-sounds', 7, (), tuple(s.name for s in STAGES), (), (), game_sounds)]
