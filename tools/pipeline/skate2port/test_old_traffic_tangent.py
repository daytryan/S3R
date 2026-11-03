"""old_traffic._tangent (Skate 1 car normal maps): glTF TANGENT w is the side of the N-T plane the mesh's own BINORMAL lies
on, as Bevy rebuilds the binormal as w * cross(N, T). Lifted by ast: old_traffic.py runs its whole export at import."""
import ast, unittest
from pathlib import Path

SRC = Path(__file__).with_name('old_traffic.py')
ns = {}
for node in ast.parse(SRC.read_text(encoding='utf-8')).body:
    if isinstance(node, ast.FunctionDef) and node.name == '_tangent':
        exec(compile(ast.Module([node], []), str(SRC), 'exec'), ns)


class Tangent(unittest.TestCase):
    def test_w_follows_the_retail_binormal(self):
        n, t = (0.0, 0.0, 1.0), (1.0, 0.0, 0.0)  # cross(N, T) = +Y
        self.assertEqual(ns['_tangent'](n, t, (0.0, 1.0, 0.0)), [1.0, 0.0, 0.0, 1.0])
        self.assertEqual(ns['_tangent'](n, t, (0.0, -1.0, 0.0)), [1.0, 0.0, 0.0, -1.0])
        # a skewed frame keeps the side, not the angle
        self.assertEqual(ns['_tangent'](n, t, (0.9, 0.1, 0.0))[3], 1.0)

    def test_a_binormal_on_the_plane_takes_plus_one(self):
        # Honda_Fit's chassis has binormals at |cos| 0.000 to cross(N, T): exactly on the N-T plane, w is +1
        n, t = (0.0, 0.0, 1.0), (1.0, 0.0, 0.0)
        self.assertEqual(ns['_tangent'](n, t, (1.0, 0.0, 0.0))[3], 1.0)
        self.assertEqual(ns['_tangent'](n, t, (0.0, 0.0, -1.0))[3], 1.0)


if __name__ == '__main__':
    unittest.main()
