"""One-time rescue: copy the hand-run build scripts into tools/pipeline and record where each came from.

Copies Python sources only. Never copies game-derived data (ice_params*.json, online1_enums.json, s2_names.txt,
*.img), caches (work/, loc/ data) or third-party binaries (unluac, vgmstream; Task 5 downloads those).
Excluded from the frozen pipeline by Build-Release.ps1 (a one-time dev script; its folders come from the environment)."""
import hashlib
import os
import shutil
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEST = REPO/'tools/pipeline'


def env(name, what):
    """The originals' folders come from the environment, never from this (public) file."""
    return Path(os.environ.get(name) or sys.exit(f'set {name} to {what}'))


def sources():
    """([(destination folder, original folder, globs)], the launcher mockup)."""
    temp = env('SKATE_RESCUE_TEMP', 'the agent temp folder that held the session scratchpads')
    return [
    ('skate2port', env('SKATE_TABLES', 'the original skate2port folder'), ['*.py', 'loc/*.py']),
    ('dlc', env('SKATE_RESCUE_DLC', 'the folder with the DLC scripts (454108E6)'), ['*.py']),
    ('converter', env('SKATE_RESCUE_CONVERTER', 'the New San Van converter folder'), ['*.py']),
    ('derive/ice2', temp/'5372769d-bad4-4525-8bde-fd7bd8cbedbf/scratchpad/ice', ['*.py']),
    ('derive/ice1', temp/'b3702a96-4c91-46cc-8bdc-14544c6d61da/scratchpad/s1on', ['*.py']),
    ('derive/ice1', temp/'b3702a96-4c91-46cc-8bdc-14544c6d61da/scratchpad/s1modes', ['enums.py']),  # online1_enums
    ('derive/names-5372', temp/'5372769d-bad4-4525-8bde-fd7bd8cbedbf/scratchpad', ['names.py']),
    ('derive/names-b370', temp/'b3702a96-4c91-46cc-8bdc-14544c6d61da/scratchpad', ['names.py', 'vault.py']),
    ('scratch', temp/'b3702a96-4c91-46cc-8bdc-14544c6d61da/scratchpad', ['reset_pros.py', 'logo-audit/patch_bounds.py']),
    ], temp/'b3702a96-4c91-46cc-8bdc-14544c6d61da/scratchpad/launcher/skate3-remaster-launcher.html'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def copy(original, out, rows):
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists() and sha(out) != sha(original):
        raise SystemExit(f'{out} already exists and differs from {original}; not overwriting')
    shutil.copy2(original, out)
    rows.append((out.relative_to(REPO).as_posix(), str(original), sha(original)[:16]))


def rescue():
    rows = []
    folders, mockup = sources()
    for dest, folder, globs in folders:
        for pattern in globs:
            for original in sorted(folder.glob(pattern)):
                copy(original, DEST/dest/original.relative_to(folder), rows)
    copy(mockup, REPO/'crates/skate-launcher/ui/launcher.html', rows)
    lines = ['# Where the rescued files came from', '',
             'Copied on 2026-10-03. The originals are untouched.', '',
             '| Repo file | Original | SHA-256 (first 16) |', '|---|---|---|']
    lines += [f'| `{a}` | `{b}` | `{c}` |' for a, b, c in rows]
    origins = DEST/'ORIGINS.md'
    notes = origins.read_text(encoding='utf-8').partition('\n## Notes')[1:] if origins.is_file() else ()
    origins.write_text('\n'.join(lines) + '\n' + ''.join(notes), encoding='utf-8')  # hand-written Notes survive
    return rows


if __name__ == '__main__':
    print(len(rescue()), 'files copied')
