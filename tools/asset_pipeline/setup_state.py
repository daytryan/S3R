"""Portable setup locking, source validation and immutable output receipts."""
from contextlib import contextmanager
import hashlib
import json
import os
import time
from pathlib import Path


def source_directory(selected, require_core=True):
    selected = Path(selected).resolve()
    if selected.is_file() and selected.name.lower() == 'default.xex':
        selected = selected.parent
    elif not selected.is_dir():
        raise RuntimeError('Select your Skate 3 default.xex or Xbox 360 ISO')
    required = ('default.xex',)
    if require_core:
        required += ('data/big/miscload.big', 'data/big/miscboot.big',
                     'data/big/db.big', 'data/content/createacharacter.big')
    for name in required:
        if not (selected/name).is_file():
            raise RuntimeError('Keep the Skate 3 game content beside default.xex; missing '+name)
    return selected


@contextmanager
def setup_lock(base):
    base.mkdir(parents=True, exist_ok=True)
    # Kernel locks are released on process death. Keep the lock file in place:
    # unlinking it allows two processes to lock different file identities.
    with (base/'setup.lock').open('a+b') as stream:
        stream.seek(0, 2)
        if not stream.tell():
            stream.write(b'0'); stream.flush()
        stream.seek(0)
        try:
            if os.name == 'nt':
                import msvcrt
                msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise RuntimeError('Setup is already running for this copy') from error
        try:
            yield
        finally:
            stream.seek(0)
            if os.name == 'nt':
                msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(stream, fcntl.LOCK_UN)


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def receipt(root, paths):
    # Windows short names and directory junctions must use the same spelling
    # on both sides of relative_to; also reject outputs outside the root.
    root = root.resolve()
    paths = (p.resolve() for p in paths)
    return {p.relative_to(root).as_posix(): {'size': p.stat().st_size, 'sha256': sha(p)}
            for p in sorted(set(paths)) if p.is_file()}


def file_sizes(root):
    """{posix path under root: size} from one directory walk. On Windows the listing carries the sizes, so this is
    ~1 s for a whole installation where a stat per receipt entry (~70,000) takes many."""
    sizes, stack, cut = {}, [os.path.join(root, '')], len(os.path.join(root, ''))
    while stack:
        with os.scandir(stack.pop()) as entries:
            for entry in entries:
                if entry.is_dir(follow_symlinks=False):
                    stack.append(entry.path)
                elif entry.is_file(follow_symlinks=False):
                    sizes[entry.path[cut:].replace(os.sep, '/')] = entry.stat(follow_symlinks=False).st_size
    return sizes


def valid_receipt(root, files, sizes=None):
    """Every file is there at its size and its SHA-256 matches. With `sizes` (file_sizes(root)) only the sizes are
    compared: the launcher's "is it installed?" on every open, not the build's trust check."""
    if not isinstance(files, dict) or not files:
        return False
    try:
        if sizes is not None:
            return all(sizes.get(name) == item['size'] for name, item in files.items())
        for name, item in files.items():
            path = (root/name).resolve()
            if not path.is_relative_to(root.resolve()) or not path.is_file():
                return False
            if path.stat().st_size != item['size'] or sha(path) != item['sha256']:
                return False
        return True
    except (OSError, KeyError, TypeError, ValueError):
        return False


BACKED_UP = ('installation.json',)  # B5: the game refuses to start without it; readers fall back to <name>.bak


def atomic_json(path, value):
    """Written in full and flushed to disk, then renamed over `path`: a power cut leaves the old file or the new one,
    never an empty one (NTFS can keep a renamed name with unwritten contents). B5: an antivirus scan, the search
    indexer or OneDrive holding `path` open makes the rename fail for a moment (WinError 5/32), so it is retried."""
    path = Path(path)
    temporary = path.with_suffix(path.suffix+'.tmp')
    with open(temporary, 'w', encoding='utf-8') as stream:
        stream.write(json.dumps(value, separators=(',', ':')))
        stream.flush()
        os.fsync(stream.fileno())
    if path.name in BACKED_UP and path.is_file():
        backup = path.with_name(path.name+'.bak')
        try:  # a hardlink keeps the old contents once the rename below replaces `path`: no 16 MB copy
            backup.unlink(missing_ok=True)
            os.link(path, backup)
        except OSError:  # FAT/exFAT has no hardlinks
            import shutil
            try:
                shutil.copyfile(path, backup)
            except OSError:
                pass  # no backup this time; the write itself goes on
    pause = 0.05
    for attempt in range(20):
        try:
            return temporary.replace(path)
        except PermissionError:
            if attempt == 19:
                raise
            time.sleep(pause)
            pause = min(pause * 2, 0.5)


def read_marker(path):
    """installation.json, or its .bak (atomic_json) when the file is missing its contents after a power cut; {} when
    there is neither. A file that exists but can't be parsed, with no good .bak, raises as before."""
    path = Path(path)
    try:
        return json.loads(path.read_text(encoding='utf-8-sig')) if path.is_file() else {}
    except ValueError:
        backup = path.with_name(path.name+'.bak')
        try:
            return json.loads(backup.read_text(encoding='utf-8-sig'))
        except (OSError, ValueError):
            pass
        raise
