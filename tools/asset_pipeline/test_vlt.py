"""Synthetic fixtures for the binary array layout; contains no game data."""
import struct
import tempfile
import unittest
from pathlib import Path
from .vlt import array_items, array_text_items, convert

CLASS, FIELD, TYPE, ROW = 0x1E04F6C0A209B06A, 0x7AE8FEF77A54BB91, 0xC2367EA3DEE35B4B, 0x76CD5EFC315C8D6D


def write_vault(stem, vlt, binary):
    stem.with_suffix('.vlt').write_bytes(vlt)
    stem.with_suffix('.bin').write_bytes(binary)


def chunk(tag, body):
    return struct.pack('>4sI', tag, 8 + len(body)) + body


def vault_bytes(kind, record):
    """Vers, a one-export ExpN pointing at `record` (placed in DatN), EndC; no pointer fixups."""
    head = chunk(b'Vers', bytes(8))
    data_at = len(head) + 8
    exports = struct.pack('>Q', 1) + struct.pack('>QQII', 0, kind, len(record), data_at)
    return head + chunk(b'DatN', record) + chunk(b'ExpN', exports) + chunk(b'EndC', bytes(8))


def schema_vlt():
    # class: key, reserve, field count 1, field defs at .bin 0, static size/at, layout 0.
    return vault_bytes(0x2A7895AC4A876152, struct.pack('>QIIIIII', CLASS, 0, 1, 0, 0, 0, 0) + bytes(4))


def schema_bin():
    # tVisualEditorData: size 80, max 0xFFFF, flags 37 (array), alignment 3 (8 bytes), as the retail schema.
    return struct.pack('>QQHHHBB', FIELD, TYPE, 0, 80, 0xFFFF, 37, 3)


def rows_vlt():
    # collection: key, class, parent, reserve, pad, entry count 1, 0 types, layout 0; one entry -> .bin 0.
    record = struct.pack('>QQQIIIHHII', ROW, CLASS, 0, 0, 0, 1, 0, 0, 0, 0)
    return vault_bytes(0xAD303B8F42B3307E, record + struct.pack('>QIHBB', FIELD, 0, 0, 10, 0))


class ArrayLayoutTests(unittest.TestCase):
    def test_vector4_header_alignment(self):
        first = struct.pack('>4f', .75, .35, 0, 0)
        second = struct.pack('>4f', 1, 1, 1, 1)
        raw = struct.pack('>4H', 2, 2, 16, 0x8000) + bytes(8) + first + second
        result = array_items(raw, 0, 16, 4)
        self.assertEqual(result['items'], [first.hex().upper(), second.hex().upper()])
        self.assertEqual(result['alignment'], 16)

    def test_each_element_is_aligned_at_absolute_offset(self):
        a, b = struct.pack('>3f', 2, 3, 4), struct.pack('>3f', 5, 6, 7)
        raw = bytes(4) + struct.pack('>4H', 3, 2, 12, 0) + bytes(4)
        raw += a + bytes(4) + b + bytes(4) + bytes(12)
        result = array_items(raw, 4, 12, 4)
        self.assertEqual(result['capacity'], 3)
        self.assertEqual(result['items'], [a.hex().upper(), b.hex().upper()])

    def test_empty_and_pointer_arrays(self):
        self.assertEqual(array_items(struct.pack('>4H', 0, 0, 4, 0), 0, 4, 2)['items'], [])
        raw = struct.pack('>4H2I', 2, 2, 4, 0, 0x1234, 0)
        self.assertEqual(array_items(raw, 0, 4, 2)['items'], ['00001234', '00000000'])

    def test_invalid_header_or_truncated_capacity_fails(self):
        for raw in [struct.pack('>4H', 1, 2, 4, 0), struct.pack('>4H', 1, 1, 8, 0),
                    struct.pack('>4HI', 2, 1, 4, 0, 123)]:
            with self.assertRaises(ValueError):
                array_items(raw, 0, 4, 2)

    def test_empty_array_at_end_of_bin_converts(self):
        # street_indu_02 / vert_indu_02 / vert_indu_03: an empty 80-byte-stride array (tVisualEditorData) is the
        # last 8 bytes of the row's .bin; the legacy 80-byte window ran past the end.
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            write_vault(d/'schema', schema_vlt(), schema_bin())
            write_vault(d/'rows', rows_vlt(), struct.pack('>4H', 0, 0, 80, 0))
            row, = convert(d/'schema', d/'rows', ['default'])['collections']
        field = row['fields'][f'Hash_{FIELD:016X}']
        self.assertEqual(field['array'], {'capacity': 0, 'element_size': 80, 'alignment': 8, 'items': []})
        self.assertEqual(field['data'], '0000000000500000')

    def test_text_array_resolves_relocated_pointers(self):
        binary = bytes(8) + 'test\0caf\u00e9\0'.encode()
        self.assertEqual(array_text_items(binary, ['00000008', '0000000D', '00000000']),
                         ['test', 'caf\u00e9', ''])
        with self.assertRaises(ValueError):
            array_text_items(binary, ['0000FFFF'])


if __name__ == '__main__':
    unittest.main()
