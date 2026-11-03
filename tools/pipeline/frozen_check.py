"""Run inside the frozen skate3setup.exe (Build-Release: `skate3setup.exe --task tools/pipeline/frozen_check.py`).
Build scripts are bundled as data and run with runpy, so PyInstaller never sees their imports: import every
third-party and standard-library module they name here, where a missing one fails the release build instead of a player's install."""
import ast, importlib, importlib.util, re, sys
from pathlib import Path

THIRD_PARTY = ('PIL', 'numpy', 'cryptography')  # tools/requirements-setup.txt
NOT_ON_WINDOWS = {'fcntl', 'termios', 'grp', 'pwd', 'resource', 'posix', 'curses', 'readline'}  # imported behind guards


def modules(tools):
    found = set()
    for path in tools.rglob('*.py'):
        try:
            tree = ast.parse(path.read_bytes())
        except SyntaxError:
            continue
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                found |= {a.name for a in node.names}
            elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
                found.add(node.module)
                found |= {f'{node.module}.{a.name}' for a in node.names}  # `from PIL import ImageDraw` is a module
    # Python's own modules too: the bundle only carries the ones setup.py's import graph reaches (wave was missed).
    wanted = set(THIRD_PARTY) | (set(sys.stdlib_module_names) - NOT_ON_WINDOWS)
    return sorted(m for m in found if m.split('.')[0] in wanted)


def missing(names):
    gone = []
    for name in names:
        try:
            importlib.import_module(name)
        except ImportError:
            parent, _, attr = name.rpartition('.')
            try:
                present = bool(parent) and hasattr(importlib.import_module(parent), attr)  # a class, not a module
            except ImportError:
                present = False
            if not present:
                gone.append(name)
    return gone


def engine_paths(tools):
    """Engine sources the scripts read (`repo/'crates/skate-core/src/scoring/catalog.rs'`): Build-Release bundles them."""
    found = set()
    for path in tools.rglob('*.py'):
        found |= set(re.findall(r"['\"](crates/[\w-]+/src[\w./-]*)['\"]", path.read_text(encoding='utf-8', errors='replace')))
    return sorted(found)


def real_modules(names):
    """The names that are modules (not classes like Cipher), for PyInstaller's --hidden-import."""
    def is_module(name):
        try:
            return importlib.util.find_spec(name) is not None
        except (ImportError, ValueError):  # parent isn't a package, or the name is an attribute set up oddly
            return False
    return [n for n in names if is_module(n)]


def shadowed(tools):
    """Top-level names under tools/ that the bundle also froze. The frozen copy wins every import, so a script gets it
    instead of its own file (build 2: a frozen half of vendor.skate3_ui hid project.py). Build-Release excludes them.
    ponytail: top level only; a script's own extra sys.path folders (skate2port, vendor/utt) aren't checked."""
    names = {p.stem for p in tools.glob('*.py')} | {p.name for p in tools.iterdir() if p.is_dir() and p.name != '__pycache__'}
    found = []
    for name in sorted(names):
        try:
            spec = importlib.util.find_spec(name)
        except (ImportError, ValueError):
            continue
        if spec and 'Frozen' in type(spec.loader).__name__:
            found.append(name)
    return found


def onedir():
    """B1: the bundle is a folder (`--onedir`): its files sit beside the exe, not unpacked to %TEMP% on every start."""
    return Path(sys._MEIPASS).resolve().parent == Path(sys.executable).resolve().parent


if __name__ == '__main__':
    if sys.argv[1:2] == ['--list']:  # Build-Release, before packaging: what the bundle must carry
        print('\n'.join(real_modules(modules(Path(sys.argv[2])))))
        sys.exit(0)
    tools = Path(__file__).resolve().parents[1]
    names = modules(tools)
    gone = missing(names) + [p for p in engine_paths(tools) if not (tools.parent/p).exists()]
    # fetch_tools verifies its downloads against certifi's roots; the module imports fine without its data file.
    import certifi
    if not Path(certifi.where()).is_file():
        gone.append('certifi/cacert.pem')
    from tools import native  # F4: a missing DLL silently falls back to Python, many times slower for every player
    gone += [f'native helper {n}.dll' for n in native.missing(tools)]
    if not onedir():
        gone.append('the setup bundle was built --onefile (Build-Release builds it --onedir: B1)')
    hidden = shadowed(tools)
    print('\n'.join([f'missing in the setup bundle: {n}' for n in gone] +
                    [f'frozen copy hides tools/{n} (add --exclude-module {n})' for n in hidden])
          or f'setup bundle imports OK ({len(names)} modules)')
    sys.exit(1 if gone or hidden else 0)
