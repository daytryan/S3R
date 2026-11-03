from __future__ import annotations

import struct
import sys
from pathlib import Path


class FormatError(ValueError):
    """Raised when an input is malformed or outside supported bounds."""


class Reader:
    def __init__(self, data: bytes, label: str = "input"):
        self.data = data
        self.label = label

    def require(self, offset: int, size: int) -> None:
        if offset < 0 or size < 0 or offset > len(self.data) - size:
            raise FormatError(
                f"{self.label}: range {offset:#x}+{size:#x} is outside "
                f"{len(self.data):#x} bytes"
            )

    def bytes(self, offset: int, size: int) -> bytes:
        self.require(offset, size)
        return self.data[offset : offset + size]

    def u8(self, offset: int) -> int:
        self.require(offset, 1)
        return self.data[offset]

    def u16be(self, offset: int) -> int:
        self.require(offset, 2)
        return struct.unpack_from(">H", self.data, offset)[0]

    def u32be(self, offset: int) -> int:
        self.require(offset, 4)
        return struct.unpack_from(">I", self.data, offset)[0]

    def i32be(self, offset: int) -> int:
        self.require(offset, 4)
        return struct.unpack_from(">i", self.data, offset)[0]

    def u64be(self, offset: int) -> int:
        self.require(offset, 8)
        return struct.unpack_from(">Q", self.data, offset)[0]

    def f32be(self, offset: int) -> float:
        self.require(offset, 4)
        return struct.unpack_from(">f", self.data, offset)[0]

    def cstring(self, offset: int, limit: int | None = None) -> str:
        if offset <= 0 or offset >= len(self.data):
            return ""
        end_limit = len(self.data) if limit is None else min(len(self.data), offset + limit)
        end = self.data.find(b"\0", offset, end_limit)
        if end < 0:
            end = end_limit
        return self.data[offset:end].decode("utf-8", errors="replace")


def align(value: int, alignment: int) -> int:
    return (value + alignment - 1) & ~(alignment - 1)


def native_library(name: str, beside: Path):
    """tools/native.py's load (None when not built or SKATE_NO_NATIVE is set). A script run without the repo root on
    sys.path (setup's HUD task) gets it appended, so the native path does not depend on how the script was started."""
    try:
        from tools import native
    except ImportError:
        sys.path.append(str(Path(__file__).resolve().parents[3]))
        from tools import native
    return native.load(name, beside)


# Files this process wrote, kept to be read back from memory: Windows Defender scans a file on its first open after a
# write (~8 ms; a write is ~0.7 ms), and the UI cache is written, then read back, file by file.
# ponytail: valid while the file's size and mtime are what the write left (a same-size rewrite by code that bypasses
# write_file within one mtime tick would be missed); capped at _MEMORY_LIMIT bytes, past which reads go to disk.
# Off (0) until remember_writes(): only the UI cache export turns it on, so other rw4 users keep no copies.
_WRITTEN: dict = {}
_MEMORY_LIMIT, _memory = 0, 0


def remember_writes(limit: int = 768 << 20) -> None:
    """Keep up to `limit` bytes of this process's write_file data for read_file (project.extract_project calls it)."""
    global _MEMORY_LIMIT
    _MEMORY_LIMIT = max(_MEMORY_LIMIT, limit)


def _key(path) -> str:
    import os
    return os.path.normcase(os.path.abspath(path))


def write_file(path, data: bytes) -> None:
    """Path(path).write_bytes(data), remembered for read_file."""
    global _memory
    path = Path(path)
    path.write_bytes(data)
    if not _MEMORY_LIMIT:
        return
    key = _key(path)
    old = _WRITTEN.pop(key, None)
    _memory -= len(old[0]) if old else 0
    if _memory + len(data) <= _MEMORY_LIMIT:
        stat = path.stat()
        _WRITTEN[key] = (bytes(data), stat.st_size, stat.st_mtime_ns)
        _memory += len(data)


def read_file(path) -> bytes:
    """Path(path).read_bytes(), from memory when write_file wrote it and it is unchanged since."""
    entry = _WRITTEN.get(_key(path))
    if entry:
        try:
            stat = Path(path).stat()
            if (stat.st_size, stat.st_mtime_ns) == entry[1:]:
                return entry[0]
        except OSError:
            pass
    return Path(path).read_bytes()


def read_text(path, encoding: str = "utf-8") -> str:
    """Path(path).read_text(encoding) through read_file (with its universal newlines)."""
    return read_file(path).decode(encoding).replace("\r\n", "\n").replace("\r", "\n")


def write_text(path, text: str, encoding: str = "utf-8") -> None:
    """Path(path).write_text(text, encoding) through write_file: the same bytes ("\n" written as os.linesep)."""
    import os
    write_file(path, text.replace("\n", os.linesep).encode(encoding))
