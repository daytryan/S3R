"""Load the library built from a tools/**/<name>_native.rs (scripts/Build-Native.ps1): bundled beside its source in the
frozen setup, under target/native in a checkout. None when it isn't built or SKATE_NO_NATIVE is set; the caller then
keeps its Python code, which every native function must match byte for byte."""
import ctypes
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(name, beside):
    """`beside` is the wrapper's __file__ (the .rs and, frozen, the .dll sit in its folder)."""
    if os.environ.get('SKATE_NO_NATIVE'):
        return None
    for path in (Path(beside).with_name(f'{name}.dll'), ROOT/'target/native'/f'{name}.dll'):
        if path.is_file():
            return ctypes.CDLL(str(path))
    return None


def missing(tools=ROOT/'tools'):
    """F4: the helpers (every tools/**/<name>_native.rs, refpack's too: fast_refpack opens the same refpack.dll) that
    don't load here, so their Python fallback runs. [] when all of them load."""
    gone = []
    for source in sorted(Path(tools).rglob('*_native.rs')):
        name = source.stem.removesuffix('_native')
        try:
            if load(name, source) is None:
                gone.append(name)
        except OSError:
            gone.append(name)
    return sorted(gone)
