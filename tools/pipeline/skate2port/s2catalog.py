"""Skate 2's cas_db_000.xml is the same v6.0 schema as Skate 3, with authoring leftovers:
id-less <mod> records (material lists with no mesh on disc) and empty <matinst> loops."""
import xml.etree.ElementTree as ET


def sanitize(xml: bytes) -> bytes:
    root = ET.fromstring(xml)
    for comp in root.findall('comp'):
        for mod in comp.findall('mod'):
            if 'id' not in mod.attrib:
                comp.remove(mod)
                continue
            for lod in mod.findall('lod'):
                for inst in lod.findall('matinst'):
                    if inst.find('matvar') is None:
                        lod.remove(inst)
    return ET.tostring(root)
