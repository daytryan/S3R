"""py -3 test_build_retail_collision_archive.py: trigger volumes stay out of the collision archive."""
import struct

from build_retail_collision_archive import collision_sections, trigger_sections

MESH, VOLUME, OWNER = 0x00080006, 0x00080001, 0x00EB000A


def rx2(*records: tuple[int, bytes]) -> bytes:
    table = 0x40
    data = bytearray(b"\x89RW4xb2".ljust(table, b"\0"))
    struct.pack_into(">I", data, 0x20, len(records))
    struct.pack_into(">I", data, 0x30, table)
    data += bytes(24 * len(records))
    for i, (kind, payload) in enumerate(records):
        struct.pack_into(">6I", data, table + 24 * i, len(data), 0, len(payload), 0, 0, kind)
        data += payload
    return bytes(data)


def volume(mesh: int) -> bytes:
    return bytes(0x40) + struct.pack(">2I", 6, mesh)


def owner(kind: int, volume: int) -> bytes:
    return struct.pack(">5I", kind, 0, 0, volume, 0)


def test_only_meshes_owned_by_trigger_objects_alone_are_triggers():
    # Records: three meshes (a trigger's, one the world shares with a trigger, an unowned one), their volumes, owners.
    data = rx2(
        (MESH, b"A" * 96), (MESH, b"B" * 96), (MESH, b"C" * 96),
        (VOLUME, volume(0)), (VOLUME, volume(1)),
        (OWNER, owner(9, 3)), (OWNER, owner(0, 4)), (OWNER, owner(5, 4)),
    )
    assert len(collision_sections(data)) == 3
    assert trigger_sections(data) == {0}


if __name__ == "__main__":
    test_only_meshes_owned_by_trigger_objects_alone_are_triggers()
    print("ok")
