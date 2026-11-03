"""The packages pass the checks the launcher makes (crates/skate-launcher/src/update.rs download/unpack/move_into_place)."""
import hashlib, json, tempfile, unittest, zipfile
from pathlib import Path
from tools.pipeline.package import package


def sha(data):
    return hashlib.sha256(data).hexdigest()


class Package(unittest.TestCase):
    def build(self):
        t = Path(self._t.name)
        stage = t/'stage'
        for rel, data in {'skate3rust.exe': b'MZ game', 'support/skate3setup.exe': b'MZ setup',
                          'steam-relay/skate-steam-relay.exe': b'MZ relay', 'mods/README.md': b'player mods'}.items():
            (stage/rel).parent.mkdir(parents=True, exist_ok=True)
            (stage/rel).write_bytes(data)
        files = {rel: sha((stage/rel).read_bytes()) for rel in ('skate3rust.exe', 'support/skate3setup.exe',
                                                               'steam-relay/skate-steam-relay.exe')}
        (stage/'release.json').write_text(json.dumps({'schema': 1, 'build': 7, 'repository': 'daytryan/S3R',
                                                      'files': files}))
        (t/'launcher.exe').write_bytes(b'MZ launcher')
        return stage, package(stage, t/'launcher.exe', t/'out'), t/'out'

    def setUp(self):
        self._t = tempfile.TemporaryDirectory()
        self.addCleanup(self._t.cleanup)

    def test_game_zip_passes_the_launchers_checks(self):
        stage, meta, out = self.build()
        body = (out/'game.zip').read_bytes()
        self.assertEqual(sha(body), meta['game']['sha256'])  # download(): the checksum release.json names
        with zipfile.ZipFile(out/'game.zip') as z:
            names = z.namelist()
            for name in names:  # unpack(): game/ prefix, no backslash, colon, '.' or '..'
                self.assertTrue(name.startswith('game/') and '\\' not in name and ':' not in name, name)
                rel = name[len('game/'):]
                if rel != 'release.json':  # move_into_place(): every program file matches its listed hash
                    self.assertEqual(sha(z.read(name)), meta['files'][rel], rel)
            self.assertEqual(json.loads(z.read('game/release.json'))['build'], 7)  # installed_build()
        self.assertNotIn('game/mods/README.md', names)

    def test_launcher_zip_and_standalone_release_json(self):
        stage, meta, out = self.build()
        with zipfile.ZipFile(out/'Skate3Remaster-launcher.zip') as z:
            self.assertEqual(z.read('Skate 3 Remaster/Skate 3 Remaster.exe'), b'MZ launcher')
        saved = json.loads((out/'release.json').read_text())
        self.assertEqual((saved['schema'], saved['build'], saved['repository']), (1, 7, 'daytryan/S3R'))
        self.assertEqual(saved['launcher']['sha256'], sha((out/'Skate3Remaster-launcher.zip').read_bytes()))
        self.assertIn(meta['game']['sha256'], (out/'game.zip.sha256').read_text())
        self.assertEqual(saved['sizes'], {rel: (stage/rel).stat().st_size for rel in saved['files']})  # F3

    def test_a_file_changed_after_release_json_stops_the_package(self):
        t = Path(self._t.name)/'stage'
        t.mkdir()
        (t/'a.exe').write_bytes(b'new')
        (t/'release.json').write_text(json.dumps({'files': {'a.exe': sha(b'old')}}))
        with self.assertRaises(ValueError):
            package(t, t/'a.exe', t/'out')

    def key(self, name='release-key.pem'):
        """An ed25519 key PEM in the temp folder (outside the repo) and its public half as update.rs carries it."""
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        private = Ed25519PrivateKey.generate()
        path = Path(self._t.name)/name
        path.write_bytes(private.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                               serialization.NoEncryption()))
        raw = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        return path, raw.hex(), private.public_key()

    def test_n8_the_signature_covers_release_json_whole(self):
        from cryptography.exceptions import InvalidSignature
        path, public, verify = self.key()
        stage, _, out = self.build()
        package(stage, Path(self._t.name)/'launcher.exe', out, key=path, public=public)
        body, sig = (out/'release.json').read_bytes(), (out/'release.json.sig').read_bytes()
        self.assertEqual(len(sig), 64)  # update.rs signed(): the raw 64-byte signature
        verify.verify(sig, body)
        meta = json.loads(body)
        meta['launcher']['sha256'] = '0' * 64  # a swapped launcher sum under the old signature
        with self.assertRaises(InvalidSignature):
            verify.verify(sig, json.dumps(meta, indent=2).encode())

    def test_n8_signing_refuses_a_key_in_the_repo_an_unset_launcher_or_another_key(self):
        from tools.pipeline.package import ROOT, launcher_key, sign
        path, public, _ = self.key()
        stage, _, out = self.build()
        with self.assertRaisesRegex(ValueError, 'not set'):
            sign(out/'release.json', path, '')
        with self.assertRaisesRegex(ValueError, 'not the key'):
            sign(out/'release.json', path, self.key('other.pem')[1])
        with self.assertRaisesRegex(ValueError, 'inside the repo'):
            sign(out/'release.json', ROOT/'release-key.pem', public)
        self.assertFalse((out/'release.json.sig').exists())
        self.assertIsInstance(launcher_key(), str)  # update.rs still has the constant the script reads


if __name__ == '__main__':
    unittest.main()
