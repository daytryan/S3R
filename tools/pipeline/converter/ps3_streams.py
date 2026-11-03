"""Read Skate 3 PS3 district streams (.pst ATOC + .psf SFIL) into the StreamAsset shape the Xbox pipeline uses."""
from __future__ import annotations
from pathlib import Path
import struct
from skate3_streams import AssetRecord, StreamAsset, StreamFormatError, decompress_refpack

PS3_ATOC_RECORD_SIZE = 72


def rw4_type(data: bytes) -> int:
    """RW4 arena type word (PS3 header offset 0x70; 0x10 model)."""
    return struct.unpack_from('>I', data, 0x70)[0]


def read_atoc(path: Path) -> list[AssetRecord]:
    data = path.read_bytes()
    if data[:4] != b'ATOC' or struct.unpack_from('>I', data, 4)[0] != 3:
        raise StreamFormatError(f'{path} is not an ATOC v3 table')
    count = struct.unpack_from('>I', data, 16)[0]
    if 24 + count * PS3_ATOC_RECORD_SIZE != len(data):
        raise StreamFormatError(f'{path}: {len(data)} bytes for {count} PS3 records')
    records = []
    for i in range(count):
        raw = data[24 + i * 72:24 + (i + 1) * 72]
        aid, proc, total, header, align = struct.unpack_from('>QIIII', raw)
        # Five (size, alignment) memory sections from 0x18; the last is RSX memory.
        gpu_size = struct.unpack_from('>I', raw, 56)[0]
        records.append(AssetRecord(aid, proc, total, header, align, gpu_size, 0, raw))
    return records


def decode_payload(payload: bytes) -> bytes:
    """Whole-asset RefPack, or a chunk table (u32 header size, u32 count, padded sizes from 0x20) of RefPack/raw chunks.

    Word 2 counts the main-memory chunks; the rest hold the RSX section. Callers check the total size.
    """
    if payload[:2] == b'\x10\xfb':
        return decompress_refpack(payload)
    header, count = struct.unpack_from('>II', payload)
    if not 0x20 + 4 * count <= header <= len(payload):
        raise StreamFormatError(f'unknown PS3 asset payload header {payload[:16].hex()}')
    sizes = struct.unpack_from(f'>{count}I', payload, 0x20)
    out, cursor = bytearray(), header
    for size in sizes:
        chunk = payload[cursor:cursor + size]
        out += decompress_refpack(chunk) if chunk[:2] == b'\x10\xfb' else chunk
        cursor += size
    return bytes(out)


def read_sfil(path: Path, index: dict[int, AssetRecord]) -> list[StreamAsset]:
    source = path.read_bytes()
    if source[:4] != b'SFIL':
        raise StreamFormatError(f'{path} is not an SFIL stream')
    first = struct.unpack_from('>I', source, 16)[0]
    cursor, assets = first, []
    while cursor + 32 <= len(source) and any(source[cursor:cursor + 32]):
        aid, stored, header_size, stride = struct.unpack_from('>QIII', source, cursor)
        record = index.get(aid)
        if record is None:
            raise StreamFormatError(f'{path}: asset 0x{aid:016X} absent from ATOC')
        payload = source[cursor + header_size:cursor + header_size + stored]
        data = payload if stored == record.total_size else decode_payload(payload)
        if len(data) != record.total_size:
            raise StreamFormatError(f'{path}: asset 0x{aid:016X} decoded to {len(data)} bytes, ATOC says {record.total_size}')
        if data[:7] != b'\x89RW4ps3':
            raise StreamFormatError(f'{path}: asset 0x{aid:016X} is not an RW4 PS3 arena')
        record = AssetRecord(record.asset_id, record.processor_id, record.total_size, record.header_size,
                             record.alignment, record.gpu_size, rw4_type(data), record.raw_record)
        assets.append(StreamAsset(record, path, cursor, stored, data))
        if stride < header_size + stored or cursor + stride > len(source):
            raise StreamFormatError(f'{path}: asset 0x{aid:016X} has invalid stride')
        cursor += stride
    return assets


def load_district_stream(stream_directory, stream_name: str, district_name: str, *, allow_missing: bool = False) -> list[StreamAsset]:
    root = Path(stream_directory)
    records = read_atoc(root / f'{district_name}_{stream_name}.pst')
    if not records:
        return []
    index = {r.asset_id: r for r in records}
    found: dict[int, StreamAsset] = {}
    for f in sorted(root.glob(f'c{stream_name}_*.psf')):
        for asset in read_sfil(f, index):
            found.setdefault(asset.record.asset_id, asset)
    missing = set(index) - set(found)
    if missing and not allow_missing:
        raise StreamFormatError(f'{len(missing)} ATOC assets missing from {root.name} {stream_name} SFILs')
    return [found[r.asset_id] for r in records if r.asset_id in found]
