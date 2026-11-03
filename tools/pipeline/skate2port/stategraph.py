"""Compile a camera/state graph XML (with <include filename=".."/>) to the binary .stategraph the
engine loads (skate-data state_graph.rs): per element a NUL-terminated tag, a big-endian attribute
count, per attribute name\\0 text\\0 float-bits(be u32) bool-byte, then a child count and children.
Includes are expanded in place. `py stategraph.py` checks Skate 2's shipped graph reproduces."""
import os, struct, sys
import xml.etree.ElementTree as ET
from pathlib import Path


def _float_bits(text):
    try:
        return struct.unpack('>I', struct.pack('>f', float(text)))[0]
    except (ValueError, OverflowError):
        return 0


def _children(element, folder):
    for child in element:
        if child.tag == 'include':
            yield from _root_elements(folder/child.get('filename'))
        else:
            yield child


def _root_elements(path):
    # Included files hold several top-level elements; wrap to parse them.
    return list(ET.fromstring('<root>' + path.read_text(encoding='utf-8') + '</root>'))


def _encode(element, folder, out):
    out += element.tag.encode() + b'\0' + struct.pack('>I', len(element.attrib))
    for name, text in element.attrib.items():
        out += name.encode() + b'\0' + text.encode() + b'\0'
        out += struct.pack('>IB', _float_bits(text), text.lower() == 'true')
    children = list(_children(element, folder))
    out += struct.pack('>I', len(children))
    for child in children:
        _encode(child, folder, out)


def compile_graph(path):
    root, = _root_elements(Path(path))
    out = bytearray()
    _encode(root, Path(path).parent, out)
    return bytes(out)


def decompile(data):
    """A compiled .stategraph back to its element tree (the inverse of _encode)."""
    def read(i):
        def text():
            nonlocal i
            j = data.index(b'\0', i)
            s, i = data[i:j].decode(), j + 1
            return s
        element = ET.Element(text())
        count, = struct.unpack_from('>I', data, i)
        i += 4
        for _ in range(count):
            name = text()
            element.set(name, text())
            i += 5  # float bits + bool byte, derived from the text
        count, = struct.unpack_from('>I', data, i)
        i += 4
        for _ in range(count):
            child, i = read(i)
            element.append(child)
        return element, i
    root, end = read(0)
    assert end == len(data), (end, len(data))
    return root


if __name__ == '__main__':
    cam = Path(os.environ.get('SKATE_WORK') or Path(__file__).parent/'work')/'cam/s2'
    built, shipped = compile_graph(cam/'Default_cameragraph.xml'), (cam/'Default_cameragraph.stategraph').read_bytes()
    first = next((i for i, (a, b) in enumerate(zip(built, shipped)) if a != b), None)
    print('identical' if built == shipped else f'differs at {first} (built {len(built)}, shipped {len(shipped)})')
    again = bytearray()
    _encode(decompile(shipped), cam, again)
    print('decompile round-trips' if again == shipped else 'decompile does NOT round-trip')
    sys.exit(built != shipped or again != shipped)
