# tools/pipeline/test_fetch_tools.py
import hashlib, io, ssl, tempfile, unittest, urllib.error, urllib.request, zipfile
from pathlib import Path
from unittest import mock
import certifi
from tools.pipeline import fetch_tools


def serves(*answers):
    """A stand-in for download(): each call writes the next answer into the file (or raises it)."""
    answers = iter(answers)

    def download(url, into, progress=None, name=None):
        answer = next(answers)
        if isinstance(answer, BaseException):
            raise answer
        Path(into).write_bytes(answer)
    return download


class Response(io.BytesIO):
    """urlopen's answer: `body` with an HTTP status and headers."""
    def __init__(self, body, status=200):
        super().__init__(body)
        self.status, self.headers = status, {'Content-Length': str(len(body))}


def zip_bytes():
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, 'w') as z:
        z.writestr('pkg/bin/tool.exe', b'MZ')
    return buf.getvalue()


class Fetch(unittest.TestCase):
    def test_download_verifies_hash_and_is_reused(self):
        data = zip_bytes()
        pins = {'tool': ('https://example.invalid/t.zip', hashlib.sha256(data).hexdigest(), 'pkg/bin/tool.exe')}
        with tempfile.TemporaryDirectory() as t, mock.patch.object(fetch_tools, 'PINS', pins), \
                mock.patch.object(fetch_tools, 'download', side_effect=serves(data)) as d:
            path = fetch_tools.tool(Path(t), 'tool', print)
            self.assertTrue(path.is_file())
            fetch_tools.tool(Path(t), 'tool', print)
            self.assertEqual(d.call_count, 1)

    def test_bad_hash_refused(self):
        pins = {'tool': ('https://example.invalid/t.zip', '0' * 64, 'pkg/bin/tool.exe')}
        with tempfile.TemporaryDirectory() as t, mock.patch.object(fetch_tools, 'PINS', pins), \
                mock.patch.object(fetch_tools, 'download', side_effect=serves(zip_bytes())):
            with self.assertRaisesRegex(ValueError, 'checksum'):
                fetch_tools.tool(Path(t), 'tool', print)
            self.assertFalse(Path(t, 'tool.download').exists())  # never continued

    def test_interrupted_unpack_is_not_reused(self):
        data = zip_bytes()
        pins = {'tool': ('https://example.invalid/t.zip', hashlib.sha256(data).hexdigest(), 'pkg/bin/tool.exe')}

        def dies_midway(self, path=None, members=None, pwd=None):
            (Path(path)/'pkg/bin').mkdir(parents=True)
            (Path(path)/'pkg/bin/tool.exe').write_bytes(b'M')
            raise OSError('disk full')

        with tempfile.TemporaryDirectory() as t, mock.patch.object(fetch_tools, 'PINS', pins), \
                mock.patch.object(fetch_tools, 'download', side_effect=serves(data, data)) as d:
            with mock.patch.object(zipfile.ZipFile, 'extractall', dies_midway), self.assertRaises(OSError):
                fetch_tools.tool(Path(t), 'tool', print)
            self.assertTrue(fetch_tools.tool(Path(t), 'tool', print).read_bytes() == b'MZ')
            self.assertEqual(d.call_count, 2)

    def test_ensure_maps_every_pin_to_its_env_var(self):
        pins = {n: ('https://example.invalid/x.zip', '0' * 64, 'x') for n in fetch_tools.ENV}
        with mock.patch.object(fetch_tools, 'PINS', pins), \
                mock.patch.object(fetch_tools, 'tool', side_effect=lambda d, n, p: Path(d)/n/'x'):
            env = fetch_tools.ensure('T', print)
        self.assertEqual(sorted(env), ['SKATE_FFMPEG', 'SKATE_JAVA', 'SKATE_UNLUAC', 'SKATE_VGMSTREAM', 'SKATE_XEX_KEY_SOURCE'])
        self.assertEqual(env['SKATE_JAVA'], str(Path('T')/'java'/'x'))

    def test_mozilla_roots_first_then_the_windows_store_on_a_certificate_error(self):
        # A friend's PC (build 8): "certificate verify failed: certificate has expired" from the Windows store.
        expired = urllib.error.URLError(ssl.SSLCertVerificationError(1, 'certificate has expired'))
        with tempfile.TemporaryDirectory() as t,                 mock.patch.object(urllib.request, 'urlopen', side_effect=[expired, Response(b'data')]) as urlopen:
            fetch_tools.download('https://example.invalid/t.zip', Path(t, 'f'))
            self.assertEqual(Path(t, 'f').read_bytes(), b'data')
        first, second = (call.kwargs['context'] for call in urlopen.call_args_list)
        mozilla = ssl.create_default_context(cafile=certifi.where())
        self.assertEqual(len(first.get_ca_certs()), len(mozilla.get_ca_certs()), 'certifi bundle only, first')
        self.assertIsNot(first, second)

    def test_network_errors_are_tried_three_times_with_a_pause_and_name_the_tool(self):
        offline = urllib.error.URLError(OSError('no network'))
        pins = {'tool': ('https://example.invalid/t.zip', '0' * 64, 'pkg/bin/tool.exe')}
        with tempfile.TemporaryDirectory() as t, mock.patch.object(fetch_tools, 'PINS', pins), \
                mock.patch.object(fetch_tools.time, 'sleep') as sleep, \
                mock.patch.object(urllib.request, 'urlopen', side_effect=offline) as urlopen:
            with self.assertRaisesRegex(OSError, r'Downloading tool from example\.invalid failed: no network'):
                fetch_tools.tool(Path(t), 'tool', print)
        self.assertEqual(urlopen.call_count, 3)  # not a certificate problem: one context per try (#11)
        self.assertEqual([c.args[0] for c in sleep.call_args_list], [2, 4])
        data = zip_bytes()
        pins = {'tool': ('https://example.invalid/t.zip', hashlib.sha256(data).hexdigest(), 'pkg/bin/tool.exe')}
        with tempfile.TemporaryDirectory() as t, mock.patch.object(fetch_tools, 'PINS', pins), \
                mock.patch.object(fetch_tools.time, 'sleep'), \
                mock.patch.object(fetch_tools, 'download', side_effect=serves(TimeoutError('timed out'), data)):
            self.assertTrue(fetch_tools.tool(Path(t), 'tool', print).is_file())  # a dropped try, then the file

    def test_a_dropped_download_continues_where_it_stopped_and_says_how_far(self):
        """E4: streamed to <name>.download; the next try asks for the rest (Range) and appends a 206 answer."""
        data = zip_bytes()
        pins = {'tool': ('https://example.invalid/t.zip', hashlib.sha256(data).hexdigest(), 'pkg/bin/tool.exe')}

        class Dropped(Response):
            def read(self, size=-1):
                if self.tell() >= 10:
                    raise ConnectionResetError('dropped')
                return super().read(10 - self.tell())
        asked, said = [], []

        def urlopen(request, timeout, context):
            asked.append(request.get_header('Range'))
            return Dropped(data) if len(asked) == 1 else Response(data[10:], status=206)
        with tempfile.TemporaryDirectory() as t, mock.patch.object(fetch_tools, 'PINS', pins),                 mock.patch.object(fetch_tools.time, 'sleep'), mock.patch.object(urllib.request, 'urlopen', urlopen):
            self.assertEqual(fetch_tools.tool(Path(t), 'tool', said.append).read_bytes(), b'MZ')
            self.assertFalse(Path(t, 'tool.download').exists())
        self.assertEqual(asked, [None, 'bytes=10-'])
        self.assertIn('Downloading tool (100%)', said)

    def test_pins_are_real_values(self):
        self.assertEqual(sorted(fetch_tools.PINS), sorted(fetch_tools.ENV))
        for name, (url, digest, inner) in fetch_tools.PINS.items():
            self.assertTrue(url.startswith('https://'), name)
            self.assertRegex(digest, r'^[0-9a-f]{64}$', name)
            self.assertNotIn('<', url + inner, name)  # no placeholder left from the plan


if __name__ == '__main__':
    unittest.main()
