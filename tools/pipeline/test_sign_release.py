# tools/pipeline/test_sign_release.py
"""F13: scripts/Sign-Release.ps1 signs only when a certificate is configured, and never fails without one."""
import os, shutil, subprocess, tempfile, unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[2]/'scripts'/'Sign-Release.ps1'
SHELL = shutil.which('pwsh') or shutil.which('powershell')


@unittest.skipUnless(SHELL and os.name == 'nt', 'needs PowerShell on Windows')
class Sign(unittest.TestCase):
    def run_script(self, env, *files):
        clean = {k: v for k, v in os.environ.items() if not k.startswith('SKATE_SIGN')}
        return subprocess.run([SHELL, '-NoProfile', '-NonInteractive', '-File', str(SCRIPT), *files],
                              env={**clean, **env}, capture_output=True, text=True)

    def test_without_a_certificate_nothing_is_signed_and_the_build_goes_on(self):
        with tempfile.TemporaryDirectory() as t:
            exe = Path(t)/'a.exe'
            exe.write_bytes(b'MZ')
            done = self.run_script({'SKATE_SIGNTOOL': str(Path(t)/'missing.exe')}, str(exe))
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertEqual(exe.read_bytes(), b'MZ')

    def test_a_pfx_or_a_thumbprint_signs_every_file_with_a_timestamp(self):
        with tempfile.TemporaryDirectory() as t:
            t = Path(t)
            log = t/'args.txt'
            tool = t/'signtool.cmd'
            tool.write_text(f'@echo %* >> "{log}"\n')
            pfx = t/'cert.pfx'
            pfx.write_bytes(b'x')
            done = self.run_script({'SKATE_SIGNTOOL': str(tool), 'SKATE_SIGN_CERT': str(pfx),
                                    'SKATE_SIGN_PASSWORD': 'pw'}, str(t/'a.exe'), str(t/'b.exe'))
            self.assertEqual(done.returncode, 0, done.stderr)
            line = log.read_text()
            for part in ('sign', '/fd sha256', '/tr ', '/td sha256', f'/f {pfx}', '/p pw', 'a.exe', 'b.exe'):
                self.assertIn(part, line)
            log.unlink()
            done = self.run_script({'SKATE_SIGNTOOL': str(tool), 'SKATE_SIGN_CERT': 'ABCDEF0123'}, str(t/'a.exe'))
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertIn('/sha1 ABCDEF0123', log.read_text())

    def test_a_configured_certificate_that_fails_to_sign_stops_the_build(self):
        with tempfile.TemporaryDirectory() as t:
            tool = Path(t)/'signtool.cmd'
            tool.write_text('@exit /b 1\n')
            done = self.run_script({'SKATE_SIGNTOOL': str(tool), 'SKATE_SIGN_CERT': 'ABCDEF0123'}, str(Path(t)/'a.exe'))
            self.assertNotEqual(done.returncode, 0)


if __name__ == '__main__':
    unittest.main()
