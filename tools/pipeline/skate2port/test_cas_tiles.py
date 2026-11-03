"""cas_tiles.resolve mirrors customiser_parts.rs's sock rule (sock_tests there)."""
import unittest
import cas_tiles


class SockRule(unittest.TestCase):
    def test_socks_follow_the_legs_as_retail_dresses_them(self):
        part = lambda slot, mats, **flags: {'slot': slot, 'materials': mats, 'flags': flags}
        lib = {'models': {'jeans': part('Pants', ['m']), 'shorts': part('Pants', ['m'], LegModelRequired='knee'),
                          'tights': part('Pants', ['m'], LegModelRequired='legs_tight'),
                          'knee': part('Leg', ['m'], LegModelType='knee'),
                          'knee_long': part('Leg', ['m'], LegModelType='knee', RequiresSockStyle='long'),
                          'tight': part('Leg', ['m'], LegModelType='legs_tight'),
                          'tight_socked': part('Leg', ['m'], LegModelType='legs_tight', RequiresSockStyle='tiny', Gender='unisex'),
                          'kneehigh': part('Sock', ['long']), 'tiny': part('Sock', ['tiny'])},
               'materials': {'m': {'flags': {}}, 'long': {'flags': {'SockStyle': 'long'}}, 'tiny': {'flags': {'SockStyle': 'tiny'}}}}
        wear = lambda pants, sock=('kneehigh', 'long'): cas_tiles.resolve(lib, 'male', {'Pants': (pants, 'm'), **({'Sock': sock} if sock else {})})
        self.assertEqual({s: v[0] for s, v in wear('shorts').items()}, {'Pants': 'shorts', 'Leg': 'knee_long', 'Sock': 'kneehigh'})
        self.assertEqual(set(wear('jeans')), {'Pants'}, 'long pants: no legs, no socks')
        self.assertEqual(wear('tights'), {'Pants': ('tights', 'm'), 'Sock': ('tiny', 'tiny')}, 'tight pants: tiny socks, no legs')
        self.assertEqual(wear('tights', None)['Leg'], ('tight', 'm'))


if __name__ == '__main__':
    unittest.main()
