"""Readers for the speech files of EA's Skate 3 audio banks (stage1_traces 1.5), and the vgmstream hand-off that turns a take
into a WAV.

A speech bank <bank>speech.big (maincast, livingworld, cameraman, announcer) holds, besides the audio:
- <bank>_Events.evt: the events and, for each, the leaves (speaker masks and a sample id) the game picks among (evt_events);
- <bank>hdr.big, one `<sample>.hdr` per sample (sample_id, takes); <bank>sth.big, one `<sample>.sth` per sample: 12 bytes a take;
- `<sample>.dat`: the sample's takes back to back, each an EAAC (XMA) block chain.
A scripted line (id 5000 and up) is a nisresident.big `.snr` header (8 bytes: a rate word and a sample-count word) with its
`.sns` in nis.big (one take, scripted_key names its character and line).

Not retail text: this file reads bytes only. Skate1's reader (skate2port/old_challenges.py) stays as it is.
"""
import os
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NamedTuple

EVT_MAGIC = 0x0315B007
LEAF_MAGIC = b"\x64\x04\x00"       # after the weight byte (0x39 or 0x2D): stage1_traces 1.5
HDR_MARK = b"\xff\xff\xff\xff"
HDR_OFFSETS = 0x10                  # MEASURED (cameraman, maincast, livingworld hdr entries): the u16 offsets start here
TAKE_BYTES = 12


class Take(NamedTuple):
    """One sth record {u32 offset in the .dat, u32 rate word, u32 sample-count word}, as stored. The rate word is the EAAC header
    word (codec in bits 24-27, channel configuration in bits 18-23, rate in bits 0-17); the count word keeps its flag bits."""
    offset: int
    rate: int
    samples: int

    @property
    def hertz(self):
        return self.rate & 0x3FFFF

    @property
    def frames(self):
        return self.samples & 0x1FFFFFFF


def _records(data):
    """[(event id, record start, leaf count)] of an .evt, with the layout checks both readers share."""
    if len(data) < 24 or struct.unpack_from(">I", data)[0] != EVT_MAGIC:
        raise ValueError(f".evt: bad magic {data[:4].hex()}")
    count, = struct.unpack_from(">H", data, 16)
    if 24 + 2 * count > len(data):
        raise ValueError(".evt: the record table runs past the file")
    out, seen = [], set()
    for at in struct.unpack_from(">%dH" % count, data, 24):
        p = 4 * at
        if p + 12 > len(data):
            raise ValueError(f".evt: a record at {p:#x} runs past the file")
        event, _, _, leaves = struct.unpack_from(">HHHB", data, p)
        if event in seen:
            raise ValueError(f".evt: event {event:#x} has two records")
        seen.add(event)
        if p + 12 + 2 * leaves > len(data):
            raise ValueError(f".evt: event {event:#x}: the leaf table runs past the file")
        out.append((event, p, leaves))
    return out


def evt_events(data):
    """<bank>_Events.evt -> {event id: [(masks, sample id)]}, the leaves in file order.
    Header: u32 0x0315B007, u32 string-table offset, u32, u32 hash, u16 event count at 0x10, then u16 record offsets (words) at 0x18.
    Record: u16 event id, u16 (90), u16 (500), u8 leaf count, u8, u8 flags, u8 0x64, u16 condition count, u16 leaf offsets (words
    from the record), the condition list ("ff <type> 04" each; evt_conditions). Leaf: weight (0x39 or 0x2D), 64 04 00, u32 LE
    mask count, u32 LE word count, the masks (u32 BE each), u16 BE sample id. The livingworld bank's leaves are 64 0C 00 with 16
    more bytes (MEASURED, livingworld_Events.evt): this reader refuses them rather than guess."""
    out = {}
    for event, p, leaves in _records(data):
        out[event] = []
        for off in struct.unpack_from(">%dH" % leaves, data, p + 12):
            r = p + 4 * off
            if r + 12 > len(data) or data[r + 1:r + 4] != LEAF_MAGIC:
                raise ValueError(f".evt: event {event:#x}: no leaf at {r:#x}")
            n, = struct.unpack_from("<I", data, r + 4)
            if r + 12 + 4 * n + 2 > len(data):
                raise ValueError(f".evt: event {event:#x}: the leaf at {r:#x} runs past the file")
            *masks, sample = struct.unpack_from(">%dIH" % n, data, r + 12)
            out[event].append((tuple(masks), sample))
    return out


def evt_conditions(data):
    """{event id: [condition type]}: the record's condition list, in file order. A leaf's masks follow this order. What a type
    means is per bank (INFERRED from the sample names, stage1_traces 1.5): in the cameraman bank type 1 is the speaker's mask
    (1 `_40_`, 2 `_30_`, 4 `_31_`) and type 2 the caller's argument (0x8003 lists 1, 2 and its leaves hold (speaker, 7); 0x8007
    lists 2, 1 and holds (argument, speaker)); in the main cast, type 1 is the first cast set and type 2 the second (Reda 8)."""
    out = {}
    for event, p, leaves in _records(data):
        count, = struct.unpack_from(">H", data, p + 10)
        at = p + 12 + 2 * (leaves + leaves % 2)
        if at + 3 * count > len(data):
            raise ValueError(f".evt: event {event:#x}: the condition list runs past the file")
        out[event] = []
        for k in range(count):
            mark, kind, tail = data[at + 3 * k:at + 3 * k + 3]
            if (mark, tail) != (0xFF, 0x04):
                raise ValueError(f".evt: event {event:#x}: condition {k} is not ff <type> 04")
            out[event].append(kind)
    return out


def sample_id(hdr):
    """The u16 sample id a `<sample>.hdr` starts with (what a leaf of the .evt names)."""
    if len(hdr) < 8:
        raise ValueError("hdr: shorter than its first 8 bytes")
    return struct.unpack_from(">H", hdr)[0]


def takes(hdr, sth, name):
    """The takes of one sample: [Take], in the hdr's order. hdr: u16 sample id, u16 take count, FFFFFFFF, 8 bytes, u16 BE offsets
    into the sth at 0x10 (one per take); sth: 12 bytes a take (Take). `name` is only for the messages."""
    if len(hdr) < HDR_OFFSETS or hdr[4:8] != HDR_MARK:
        raise ValueError(f"{name}: not a hdr (no FFFFFFFF after the take count)")
    count, = struct.unpack_from(">H", hdr, 2)
    if len(hdr) < HDR_OFFSETS + 2 * count:
        raise ValueError(f"{name}: the hdr is cut short ({count} takes, {len(hdr)} bytes)")
    out = []
    for at in struct.unpack_from(">%dH" % count, hdr, HDR_OFFSETS):
        if at % TAKE_BYTES or at + TAKE_BYTES > len(sth):
            raise ValueError(f"{name}: take record {at:#x} is not in the sth ({len(sth)} bytes)")
        out.append(Take(*struct.unpack_from(">III", sth, at)))
    return out


def snr_take(header):
    """A scripted line's `.snr` header (rate word, sample-count word) as the one Take of its `.sns` (offset 0)."""
    if len(header) != 8:
        raise ValueError(f"snr: {len(header)} bytes, not 8")
    return Take(0, *struct.unpack(">II", header))


def scripted_key(name):
    """(character, line id) of a nisresident name `CC_NNNN_...`: 0x824a5a08 reads atoi(name[0:2]) and atoi(name[3:7])."""
    found = re.match(r"(\d\d)_(\d{4})", name)
    if not found:
        raise ValueError(f"{name}: not a CC_NNNN scripted line name")
    return int(found.group(1)), int(found.group(2))


def vgmstream_command(vgmstream=None):
    """The command that runs vgmstream-cli: the argument (a path or a command list), else SKATE_VGMSTREAM."""
    vgmstream = vgmstream or os.environ.get("SKATE_VGMSTREAM")
    if not vgmstream:
        raise RuntimeError("SKATE_VGMSTREAM must name vgmstream-cli (tools/pipeline/fetch_tools.py fetches it)")
    command = [str(x) for x in vgmstream] if isinstance(vgmstream, (list, tuple)) else [str(vgmstream)]
    if len(command) == 1 and not Path(command[0]).is_file():
        raise RuntimeError(f"vgmstream-cli not found at {command[0]}")
    return command


def wav(dat, take, end=None, vgmstream=None):
    """One take as a 16-bit PCM WAV (bytes): vgmstream reads the take's header words and the block chain from `take.offset` (to
    `end`, else the .dat's end; the header's sample count stops the decode) as an .snr / .sns pair."""
    stop = len(dat) if end is None else end
    if not 0 <= take.offset < stop <= len(dat):
        raise ValueError(f"take at {take.offset:#x} is not inside the {len(dat)} byte stream")
    command = vgmstream_command(vgmstream)
    with tempfile.TemporaryDirectory(prefix="ea-speech-") as work:
        snr = Path(work) / "a.snr"
        snr.write_bytes(struct.pack(">II", take.rate, take.samples))
        snr.with_suffix(".sns").write_bytes(dat[take.offset:stop])
        out = Path(work) / "a.wav"
        done = subprocess.run([*command, "-o", str(out), str(snr)], capture_output=True)
        if done.returncode:
            raise RuntimeError(f"vgmstream failed ({done.returncode}) on a take at {take.offset:#x}: {done.stderr.decode(errors='replace')[:200]}")
        data = out.read_bytes() if out.exists() else b""
    if len(data) <= 44 or data[:4] != b"RIFF":
        raise RuntimeError(f"vgmstream wrote no audio for the take at {take.offset:#x}")
    return data


def big_archive():
    """owned_game.big.BigArchive, imported when first needed (it wants the repository root on sys.path, as skate3_online sets it)."""
    here = Path(__file__).resolve().parent
    for path in (here, here.parent):
        if str(path) not in sys.path:
            sys.path.insert(0, str(path))
    from owned_game.big import BigArchive
    return BigArchive


def _member(archive, name):
    """The one entry of `archive` whose file name is `name` (single entries are read, never the whole archive)."""
    for entry in archive.entries:
        if Path(entry.path).name == name:
            return entry
    raise ValueError(f"{archive.path.name} has no {name}")


class SpeechBank:
    """One <bank>speech.big of a disc's data/audio/english folder. Everything is read entry by entry: the events and the inner
    hdr / sth archives once, a sample's .dat only while it is being decoded. `work` is a folder for the inner archives."""

    def __init__(self, path, bank, work):
        self.archive = big_archive()(path)
        self.bank = bank
        self.work = Path(work)
        self._cache = {}

    def _read(self, name):
        return self.archive.read(_member(self.archive, name))

    def _inner(self, name):
        if name not in self._cache:
            path = self.work / f"{self.bank}-{name}"
            path.write_bytes(self._read(name))
            archive = big_archive()(path)
            self._cache[name] = (archive, {Path(x.path).stem: x for x in archive.entries})
        return self._cache[name]

    def _evt(self):
        if "evt" not in self._cache:
            self._cache["evt"] = self._read(f"{self.bank}_Events.evt")
        return self._cache["evt"]

    def events(self):
        """{event: [(masks, sample id)]}: evt_events of the bank's .evt (livingworld is refused, see evt_events)."""
        return evt_events(self._evt())

    def conditions(self):
        """{event: [condition type]}: evt_conditions of the bank's .evt."""
        return evt_conditions(self._evt())

    def samples(self):
        """{sample id: sample name} over the bank's hdr entries."""
        archive, entries = self._inner(f"{self.bank}hdr.big")
        out = {}
        for name, entry in entries.items():
            sid = sample_id(archive.read(entry))
            if sid in out:
                raise ValueError(f"{self.bank}: samples {out[sid]} and {name} share the id {sid:#x}")
            out[sid] = name
        return out

    def takes_of(self, name):
        """The sample's takes ([Take]) from its hdr and sth, without touching the audio."""
        hdr_archive, hdrs = self._inner(f"{self.bank}hdr.big")
        sth_archive, sths = self._inner(f"{self.bank}sth.big")
        if name not in hdrs or name not in sths:
            raise KeyError(f"{self.bank}: no sample {name}")
        return takes(hdr_archive.read(hdrs[name]), sth_archive.read(sths[name]), name)

    def wavs(self, name, vgmstream=None):
        """The WAVs of one sample's takes, one at a time (a generator, so only a take is held). A take ends where the next
        higher offset of the sample begins; the last one at the end of the .dat."""
        found = self.takes_of(name)
        dat = self._read(f"{name}.dat")
        offsets = sorted({t.offset for t in found})
        for t in found:
            later = [o for o in offsets if o > t.offset]
            yield wav(dat, t, end=later[0] if later else None, vgmstream=vgmstream)


class ScriptedLines:
    """The scripted lines of a disc's nis.big: nisresident.big's `.snr` headers (one 8-byte header a variant, in directory order)
    and nis.big's `.sns` streams."""

    def __init__(self, path, work):
        self.archive = big_archive()(path)
        self.work = Path(work)
        self._resident = None

    def _headers(self):
        if self._resident is None:
            entry = _member(self.archive, "nisresident.big")
            path = self.work / "nisresident.big"
            path.write_bytes(self.archive.read(entry))
            inner = big_archive()(path)
            streams = {Path(x.path).stem for x in self.archive.entries if x.path.endswith(".sns")}
            self._resident = {}
            for x in inner.entries:
                stem = Path(x.path).stem
                if stem not in streams:
                    raise ValueError(f"nis.big: {x.path} has no .sns stream")
                self._resident[stem] = (inner, x)
        return self._resident

    def variants(self):
        """{(character, line id): [variant name]}, in directory order (0x824a5a08 adds them in that order)."""
        out = {}
        for stem in self._headers():
            out.setdefault(scripted_key(stem), []).append(stem)
        return out

    def take(self, stem):
        """A variant's one Take (its .snr header), without touching the audio."""
        inner, entry = self._headers()[stem]
        return snr_take(inner.read(entry))

    def wav(self, stem, vgmstream=None):
        """One variant as a WAV."""
        inner, entry = self._headers()[stem]
        body = self.archive.read(_member(self.archive, f"{stem}.sns"))
        return wav(body, snr_take(inner.read(entry)), vgmstream=vgmstream)
