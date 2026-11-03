"""Content identities for independently refreshable owned-disc asset groups."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import re

GROUPS = ('core', 'hud', 'character', 'environment', 'maps')
COMMON = ('owned_game/**/*.py', 'asset_pipeline/fast_refpack.py', 'asset_pipeline/refpack_native.rs',
          'requirements-setup.txt')
PARSERS = ('vendor/utt/**/*.py', 'vendor/**/*_native.rs', 'vendor/university/**/*.py', 'vendor/utt/**/*.json', 'vendor/university/**/*.json')
SOURCES = {
    'core': ('asset_pipeline/vlt.py', 'asset_pipeline/names.txt', 'asset_pipeline/physics_skeleton.py',
             'asset_pipeline/audio_world.py', 'vendor/university/tools/vanilla_map_extraction/tools/skate3_streams.py'),
    'hud': ('asset_pipeline/optional_content.py', 'prepare_hud.py', 'prepare_runtime_huds.py', 'install_prepared_hud.py',
            'prepare_core_menu.py', 'prepare_create_skater.py', 'prepare_challenge_map.py',
            'prepare_tip_loader.py', 'prepare_music_ui.py', 'prepare_hud_meters.py', 'prepare_online_ui.py',
            'extract_session_marker.py', 'vendor/skate3_ui/**/*.py', 'vendor/skate3_ui/*_native.rs', 'vendor/skate3_ui/**/*.json'),
    'character': PARSERS + ('extract_default_skater.py', 'default_skater_retail_manifest.json',
                  'asset_pipeline/character*.py', 'asset_pipeline/retail_character.py',
                  'vendor/skate3_anim/abin_importer.py', 'vendor/skate3_anim/rx2_skeleton.py'),
    'environment': PARSERS + ('asset_pipeline/optional_content.py', 'asset_pipeline/sky.py', 'asset_pipeline/backdrop.py',
                  'asset_pipeline/render_parameters.py', 'asset_pipeline/teleports.py',
                  'asset_pipeline/environment.py', 'asset_pipeline/map_writer.py',
                  'asset_pipeline/retail_material.py', 'asset_pipeline/irradiance.py'),
    'maps': PARSERS + ('asset_pipeline/optional_content.py', 'asset_pipeline/map*.py', 'asset_pipeline/dynamic_props.py',
             'asset_pipeline/environment.py', 'asset_pipeline/irradiance.py',
             'asset_pipeline/retail_material.py', 'asset_pipeline/backdrop.py', 'asset_pipeline/sky.py'),
}


STAGES_FILE = 'pipeline/stages.py'
SHAPING_PINS = (b'numpy', b'pillow')  # #4: of requirements-setup.txt, the packages whose version can change a file


def content(path):
    """A source file's bytes as fingerprinted: line endings ignored, and of the setup's pins only SHAPING_PINS (a
    certifi or PyInstaller bump rebuilt every stage for every player)."""
    data = path.read_bytes().replace(b'\r\n', b'\n')
    if path.name == 'requirements-setup.txt':
        data = b'\n'.join(line for line in data.split(b'\n') if line.split(b'==')[0].strip().lower() in SHAPING_PINS)
    return data


def stage_code(tools, stage):
    """Digest of one stage's own code in stages.py: its run function and every module-level name that function reaches,
    so editing another stage (or the table) leaves this stage's fingerprint alone. Comments never change it."""
    digest = hashlib.sha256(repr((stage.inputs, stage.roots)).encode())  # a new root or input needs a rerun and a new receipt
    path = tools/STAGES_FILE
    if not path.is_file():
        return digest.digest()
    top = {}
    for node in ast.parse(path.read_text(encoding='utf-8')).body:
        for name in ([node.name] if isinstance(node, (ast.FunctionDef, ast.ClassDef)) else
                     [t.id for t in node.targets if isinstance(t, ast.Name)] if isinstance(node, ast.Assign) else []):
            top[name] = node
    todo, seen = [stage.run.__qualname__.split('.')[0]], set()  # 'wardrobe.<locals>.run' is defined by wardrobe
    while todo:
        name = todo.pop()
        if name not in seen and name in top and name != 'STAGES':
            seen.add(name)
            todo += [n.id for n in ast.walk(top[name]) if isinstance(n, ast.Name)]
    for name in sorted(seen):
        digest.update(ast.dump(top[name]).encode())
    # closures (wardrobe, simple) carry the stage's own script and arguments in their cells
    digest.update(repr([c.cell_contents for c in stage.run.__closure__ or () if not callable(c.cell_contents)]).encode())
    return digest.digest()


def fingerprints(tools=None):
    tools = tools or Path(__file__).resolve().parents[1]
    result = {}
    for group, patterns in SOURCES.items():
        paths = {p for pattern in COMMON + patterns for p in tools.glob(pattern)
                 if p.is_file() and not p.name.startswith('test_') and '__pycache__' not in p.parts}
        digest = hashlib.sha256()
        # Version the actual per-group recipe and shared extraction primitive,
        # without invalidating maps when a dialog/lock/publication rule changes.
        for filename, names in (('asset_pipeline/asset_exports.py', {group}),
                                ('asset_pipeline/install.py', {'extract'} | ({'convert_map'} if group == 'maps' else set()))):
            path = tools/filename
            if path.is_file():
                nodes = ast.parse(path.read_text()).body
                for node in nodes:
                    if isinstance(node, (ast.Import, ast.ImportFrom)) or getattr(node, 'name', None) in names:
                        digest.update(ast.dump(node, include_attributes=False).encode())
        for path in sorted(paths, key=lambda p: p.relative_to(tools).as_posix()):
            digest.update(path.relative_to(tools).as_posix().encode() + b'\0')
            digest.update(hashlib.sha256(content(path)).digest())
        result[group] = digest.hexdigest()
    from tools.pipeline.stages import STAGES  # ponytail: stage versions ride on the same dict the game compares
    seen = {}  # path -> content digest, shared by the stages (the parsers and UI folders are in most of them)
    for stage in STAGES:
        if stage.name == 'skate3-base':
            continue  # it is the five groups above (runner.stage_fingerprints hashes them)
        paths = {p for pattern in COMMON + stage.sources for p in tools.glob(pattern)
                 if p.is_file() and not p.name.startswith('test_') and '__pycache__' not in p.parts
                 and p != tools/STAGES_FILE}  # stages.py is versioned per stage by stage_code, not as one file
        digest = hashlib.sha256(stage_code(tools, stage))
        for path in sorted(paths, key=lambda p: p.relative_to(tools).as_posix()):
            if path not in seen:
                seen[path] = hashlib.sha256(content(path)).digest()
            digest.update(path.relative_to(tools).as_posix().encode() + b'\0')
            digest.update(seen[path])
        result[stage.name] = digest.hexdigest()
    return result


def changed_groups(previous, current):
    migrations = Path(__file__).with_name('pipeline-equivalence.json')
    equivalent = json.loads(migrations.read_text()) if migrations.is_file() else {}
    changed = {name for name in GROUPS if previous.get(name) != current[name]
               and [previous.get(name), current[name]] not in equivalent.get(name, [])}
    # Other exports consume the core VLT/skeleton/animation data.
    if 'core' in changed:
        changed.update(GROUPS)
    return changed


def installed(base):
    marker = base/'installation.json'
    if not marker.is_file():
        return None
    value = json.loads(marker.read_text(encoding='utf-8-sig'))
    directory = value.get('directory', '')
    if value.get('version') != 1 or not re.fullmatch(r'installations/[0-9a-f]{32}', directory):
        raise ValueError('Invalid installation record')
    root = (base/directory).resolve()
    if not root.is_relative_to(base.resolve()):
        raise ValueError('Installation escapes its package data directory')
    if not root.is_dir():
        return None
    return root, value


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--tools', type=Path)
    print(json.dumps(fingerprints(parser.parse_args().tools)))
