"""old_tutorial.py ai_tricks / ai_profiles (round2 T5 tricks): the skater-vault rows the AI PathController reads.
A scorable row (class 0x6918469984A8C596) gives a trick's lead (Hash_2702723D1A7F0ABF) and its 6-slot AI recipe
(Hash_F216FED54DCBEB50, {UInt32 signal, Float value, Int32 from, Int32 to}, signal 0 = unused); an AI profile (class
0xE9407D230E506420) its grabs flag (Hash_6DBAF7AD6CDE6A16) and weighted flip lists (Hash_E580B6284639E03F regular,
Hash_BB901D68361E9833 nollie; {UInt32 trick, Float weight}). Fields inherit from the parent row."""
import struct, unittest

from tools.pipeline.skate2port.test_vault_scalars import lib

SCORABLE, PROFILE = 'Hash_6918469984A8C596', 'Hash_E9407D230E506420'


def slot(sig, value, start, end):
    return (struct.pack('>Ifii', sig, value, start, end) + b'\0' * 8).hex().upper()


def flip(trick, weight):
    return struct.pack('>If', trick, weight).hex().upper()


def row(cls, key, fields, parent=''):
    return {'class': cls, 'key': key, 'parent': parent, 'fields': fields}


DATA = {'collections': [
    row(SCORABLE, 'gnd_bsgrab', {
        'Hash_2702723D1A7F0ABF': {'type': 'EA::Reflection::UInt32', 'data': '0000000F'},
        'Hash_F216FED54DCBEB50': {'type': 'Hash_X', 'array': {'items': [
            slot(21, 1.0, 0, -1), slot(0, 0.0, 0, 0), slot(17, 1.0, 0, -1), slot(0, 0.0, 0, 0)]}}}),
    row(SCORABLE, 'ollie', {}),
    row(PROFILE, 'skater_base', {'Hash_6DBAF7AD6CDE6A16': {'type': 'EA::Reflection::Bool', 'data': '01000000'},
                                 'Hash_E580B6284639E03F': {'type': 'Hash_Y', 'array': {'items': [flip(128, 1.0)]}}}),
    row(PROFILE, 'slappy', {
        'Hash_E580B6284639E03F': {'type': 'Hash_Y', 'array': {'items': [flip(96, 0.5), flip(97, 0.25)]}},
        'Hash_BB901D68361E9833': {'type': 'Hash_Y', 'array': {'items': [flip(117, 0.5)]}}}, 'skater_base'),
    row(PROFILE, 'mike_carroll', {}),
]}


class AiTricks(unittest.TestCase):
    def setUp(self):
        self.ns = lib('old_tutorial.py', '\ndef db_vaults')
        self.v = self.ns['Vault'](DATA, b'')

    def test_rows_by_index(self):
        got = self.ns['ai_tricks'](self.v, ['gnd_bsgrab', 'nosuchtrick', 'ollie'])
        self.assertEqual(got, [{'lead': 15, 'recipe': [[21, 1.0, 0, -1], [17, 1.0, 0, -1]]}, None,
                               {'lead': 0, 'recipe': []}])

    def test_profiles_inherit(self):
        got = self.ns['ai_profiles'](self.v, ['mike_carroll', 'nobody', 'slappy'])
        self.assertEqual(got, {'mike_carroll': {'grabs': False, 'regular': [], 'nollie': []},
                               'slappy': {'grabs': True, 'regular': [[96, 0.5], [97, 0.25]], 'nollie': [[117, 0.5]]}})

    def test_no_classes(self):
        v = self.ns['Vault']({'collections': []}, b'')
        self.assertEqual(self.ns['ai_tricks'](v, ['ollie']), [None])
        self.assertEqual(self.ns['ai_profiles'](v, ['slappy']), {})


if __name__ == '__main__':
    unittest.main()
