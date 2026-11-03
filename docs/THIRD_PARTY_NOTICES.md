# Third-party notices

Bevy sources retain their MIT and Apache licenses under `vendor/`.

The programs are built from Rust crates published on crates.io. Each crate's
license and notice files, as `Cargo.lock` pins them, are in
`licenses/RustCrates.txt` (written by `tools/pipeline/crate_licenses.py` at
packaging). Crates that name a license without shipping its file are covered by
the standard MIT and Apache 2.0 texts at the end of that file.

Retail world shading equations were referenced against Alex's Skate3Recomp
native renderer. Source revision and implementation scope are documented in
`docs/retail-renderer.md`. No game shaders or texture payloads are bundled.

`tools/vendor/utt` contains the MIT-licensed UTT RX2/model parsers by duckyinnit.
The full license is alongside the source. No UTT game payloads or executables
are bundled.

The map exporters and archive readers come from the MIT-licensed Skate 3
Custom Engine Layer. See `tools/vendor/university/LICENSE-PROJECT.md`.
The supplied preview.18 exporter has an explicit lossless v14 storage mode for
this runtime. The character scripts are the project's existing clone converters;
the ABIN/RX2 toolkit provenance is in `tools/vendor/skate3_anim/PROVENANCE.md`.
That upstream toolkit (chasmlol/skate-3-trick-toolkit) carries no license file
in the pinned copy; its terms are not confirmed here.

The AttribSys converter uses the publicly documented VaultLib layout as a
reference: https://github.com/NFSTools/VaultLib. Its lookup8 hash and record
reader preserve data from the user's BIN/VLT files; no settings payload is
bundled.

Release setup uses direct Python GLB and SKATE writers adapted from those
source converters. Blender is not downloaded, bundled or invoked. The retained
editor authoring scripts are excluded from release packages. XboxDev
extract-xiso retains its own license files. Game assets are never downloaded
from this project.

The setup executable bundles Python, NumPy, Pillow, Tcl/Tk and PyInstaller's
bootloader. Their license information is retained by the packager alongside
its bundled libraries. No game content is included in the release archive.

Character import also bundles Facebook FBX2glTF 0.9.7, including its Autodesk
FBX SDK dependency, and the Microsoft Visual C++ redistributable runtime.
FBX2glTF's BSD notice is in `licenses/FBX2glTF.txt` and inside the setup payload.
The native converter is fetched from its official release and checked against
the pinned SHA-256 before packaging. No private calibration or skater reference
is distributed.

The launcher's build downloads four more tools from their official hosts, each
checked against a pinned SHA-256 (`tools/pipeline/fetch_tools.py`). None is
bundled with or redistributed by this project; each is run as a separate
program.

Eclipse Temurin 21.0.9+10 JRE (Windows x64) runs unluac. Fetched from
https://github.com/adoptium/temurin21-binaries/releases/tag/jdk-21.0.9%2B10.
License: GNU GPL version 2 with the Classpath Exception.

unluac 1.2.2.155 (`unluac_2015_06_13.jar`) by tehtmi decompiles the Skate 1 and
Skate 2 Lua handlers. Fetched from
https://downloads.sourceforge.net/project/unluac/Unstable/unluac_2015_06_13.jar.
License: MIT.

vgmstream r2117 (`vgmstream-cli`, Windows x64) decodes the game's audio streams.
Fetched from https://github.com/vgmstream/vgmstream/releases/tag/r2117. License:
ISC (`COPYING` in the download). The decoder libraries shipped beside it (FFmpeg
and mpg123 under the LGPL, the rest under their own licenses) keep those
licenses.

FFmpeg 9.0.1 "essentials" build by gyan.dev decodes audio and the intro videos.
Fetched from https://github.com/GyanD/codexffmpeg/releases/tag/9.0.1. License:
GNU GPL version 3 (the build is configured with `--enable-gpl
--enable-version3`; `LICENSE` is in the download).

## Weather audio (CC0 1.0, Freesound)

The rain and thunder recordings (`tools/prepare_weather_audio.py` masters them into
`assets/weather-audio/`) are dedicated to the public domain under Creative Commons
CC0 1.0 (https://creativecommons.org/publicdomain/zero/1.0/). Credited with thanks:

- 119500 "gentle-rain-on-concrete" by ingudios: https://freesound.org/s/119500/
- 169534 "high-thunder-suburb-08" by klankbeeld: https://freesound.org/s/169534/
- 171979 "amb_m_city_rain_spout" by conleec: https://freesound.org/s/171979/
- 171980 "amb_m_city_rain_light" by conleec: https://freesound.org/s/171980/
- 171981 "amb_m_city_rain_heavy" by conleec: https://freesound.org/s/171981/
- 171983 "amb_m_city_rain_tunnel" by conleec: https://freesound.org/s/171983/
- 202898 "ext_raininpuddle" by jongrubbs: https://freesound.org/s/202898/
- 202899 "ext_hardrainonpatio-smooth" by jongrubbs: https://freesound.org/s/202899/
- 398738 "rain-garage-left-door" by anthousai: https://freesound.org/s/398738/
- 446753 "heavy-thunder-strike-no-rain-quadro" by bluedelta: https://freesound.org/s/446753/
- 451154 "rain-medium-on-group-with-leaves-soft-splatty-campground" by kyles: https://freesound.org/s/451154/
- 454142 "rain-medium-on-metal-roof-ext-like-porch" by kyles: https://freesound.org/s/454142/
- 476738 "thunder-long-tail" by elmoustachio: https://freesound.org/s/476738/
- 483580 "rain-on-concrete" by dblover: https://freesound.org/s/483580/
- 534023 "thunderclap" by fission9: https://freesound.org/s/534023/
- 567746 "200708-thunders-various-big-med-distant-crack-booming-hiss-of-rain" by trp: https://freesound.org/s/567746/
- 567945 "200817-thunder-pretty-close-crack-4pm" by trp: https://freesound.org/s/567945/
- 574412 "thunder-big-crack-explosive-close-ms-77mel-190903" by trp: https://freesound.org/s/574412/
- 574416 "thunder-med-crack-roll-77mel-190414" by trp: https://freesound.org/s/574416/
- 577315 "thunder-single-long-rolling-distant-light-rain-toronto-2011" by trp: https://freesound.org/s/577315/
- 577317 "thunder-dry-distant-rolling-field-notl-2011" by trp: https://freesound.org/s/577317/
- 584268 "rain-on-grass-and-leaves" by samsterbirdies: https://freesound.org/s/584268/
- 584272 "rain-on-leaves" by samsterbirdies: https://freesound.org/s/584272/
- 611421 "rain-on-wood-deck-extreme-quality" by cocaine: https://freesound.org/s/611421/
- 611424 "rain-on-metal-roof" by cocaine: https://freesound.org/s/611424/
- 637777 "rain-heavy-road-concrete-intensifies" by kyles: https://freesound.org/s/637777/
- 717888 "230812-thunder-med-crack-rain-stops-dry-rolling-stratford-9am-2am" by trp: https://freesound.org/s/717888/
- 778364 "heavy_urban_rain_01" by blondpanda: https://freesound.org/s/778364/
- 778372 "rolling_distant_thunder_in_heavy_rain_04" by blondpanda: https://freesound.org/s/778372/
- 845501 "continuous-light-rain-on-open-window-rural-ambience-altenthann-16-feb-2026-260216_001(1)" by tsp-talk: https://freesound.org/s/845501/
- 870824 "rain-heavy-falling-on-leaves" by challsmith: https://freesound.org/s/870824/

Builds with the `dlss` feature use NVIDIA DLSS through Bevy's `dlss_wgpu` (MIT
OR Apache-2.0). NVIDIA's runtime `nvngx_dlss.dll` and its notices,
`NVIDIA-DLSS-NOTICE.txt`, ship beside the exe. Neither the DLSS SDK nor the DLL
is in this repository. See `docs/dlss.md`.

The Steam relay ships Valve's `steam_api64.dll` from the Steamworks SDK
redistributables, under the Steamworks SDK Access Agreement. It is not in this
repository.

The launcher's interface embeds Big Shoulders Display, Barlow and IBM Plex Mono
(Latin subsets from Google Fonts) under the SIL Open Font License 1.1. Their
copyright lines and the full license text are in
`crates/skate-launcher/ui/fonts.html`, which is built into the launcher.

The launcher's Discord button shows the Discord logo, a trademark of Discord Inc., used to link to the project's
Discord server. Its outline is the `discord` icon of Simple Icons 13.21.0 (https://simpleicons.org, CC0 1.0).

## Skate 2 Rust port (MIT)

`vendor/skate2-rust-port/` holds the simulation crates (`skate2-core`, `skate2-formats`) of skate6743's Skate 2 Rust
port, https://github.com/skate6743/skate2-rust-port, commit f72f2c1302bf9cf176f842ace7b441d1824feba4 (v0.2.3). They are
copied unchanged except the lines marked `ENGINE HOOK (user 10-20)`, which `vendor/skate2-rust-port/VENDORED.md`
lists. The Skate 2 physics setting runs them in a separate program, `skate2-physics-host`. License: MIT, Copyright (c)
2026 skate6743 and contributors. The license text is `vendor/skate2-rust-port/LICENSE` in the source and
`licenses/skate2-rust-port.txt` in the release package. No Skate 2 game data is bundled: the player's own disc
supplies it.
