# tools/pipeline/test_crate_licenses.py
"""RUST_CRATE_LICENSES.txt lists the vendored Skate 2 port's crates.io crates, which the game links (Amendment A)."""
import unittest
from tools.pipeline import crate_licenses


class Listing(unittest.TestCase):
    def test_the_port_crates_are_listed(self):
        listed = crate_licenses.crates()
        for crate in (('quick-xml', '0.41.0'), ('memchr', '2.8.3')):
            self.assertEqual(listed.count(crate), 1, crate)

    def test_vendored_crates_carry_their_notices(self):
        # [patch.crates-io] copies under vendor/ have no registry source in Cargo.lock (v11: wgpu-hal, dlss_wgpu)
        listed = crate_licenses.crates()
        for crate in (('wgpu-hal', '27.0.4'), ('dlss_wgpu', '2.0.0'), ('bevy_audio', '0.18.1')):
            self.assertEqual(listed.count(crate), 1, crate)
        text = crate_licenses.notice('wgpu-hal', '27.0.4', crate_licenses.registry_dirs())
        self.assertIn('Copyright (c) 2025 The gfx-rs developers', text)


if __name__ == '__main__':
    unittest.main()
