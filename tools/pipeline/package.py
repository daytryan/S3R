"""Release packages in the launcher's format (crates/skate-launcher/src/update.rs):

- game.zip: `game/<path>` for every file release.json's `files` lists, plus `game/release.json` (mods/ stay out:
  the launcher never replaces a player's mods);
- Skate3Remaster-launcher.zip: `Skate 3 Remaster/Skate 3 Remaster.exe`;
- release.json (the standalone release asset): the game's release.json plus `game` and `launcher` {name, sha256};
- a .sha256 file beside each zip;
- release.json.sig: the raw ed25519 signature of the standalone release.json's exact bytes, by the owner's offline
  key (N8, release review 10-18). That file names every sha256, the launcher zip's included, so the one signature
  covers the whole update; the launcher takes no release without it (update.rs RELEASE_KEY). The key's PEM is read
  from a path outside the repo and must be the key whose public half update.rs carries.

Python's zipfile writes forward-slash names; the launcher refuses a backslash as an unsafe path.

    python -m tools.pipeline.package STAGE_DIR LAUNCHER_EXE OUT_DIR [RELEASE_KEY_PEM]
"""
import hashlib, json, re, sys, zipfile
from pathlib import Path

GAME_ZIP, LAUNCHER_ZIP = 'game.zip', 'Skate3Remaster-launcher.zip'
ROOT = Path(__file__).resolve().parents[2]
UPDATE_RS = ROOT/'crates/skate-launcher/src/update.rs'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_zip(out, entries):
    with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, path in entries:
            z.write(path, name)
    digest = sha(out.read_bytes())
    out.with_name(out.name + '.sha256').write_text(f'{digest}  {out.name}\n', encoding='ascii')
    return digest


def package(stage, launcher_exe, out, key=None, public=None):
    stage, out = Path(stage), Path(out)
    out.mkdir(parents=True, exist_ok=True)
    meta = json.loads((stage/'release.json').read_text(encoding='utf-8-sig'))
    files = meta['files']
    for rel, want in files.items():
        if sha((stage/rel).read_bytes()) != want.lower():
            raise ValueError(f'{rel} changed after release.json was written')
    game = write_zip(out/GAME_ZIP, [(f'game/{rel}', stage/rel) for rel in sorted(files)] +
                     [('game/release.json', stage/'release.json')])
    launcher = write_zip(out/LAUNCHER_ZIP, [('Skate 3 Remaster/Skate 3 Remaster.exe', Path(launcher_exe))])
    meta.update(game={'name': GAME_ZIP, 'sha256': game}, launcher={'name': LAUNCHER_ZIP, 'sha256': launcher},
                sizes={rel: (stage/rel).stat().st_size for rel in files})  # F3: the launcher skips hashing a size that differs
    (out/'release.json').write_text(json.dumps(meta, indent=2), encoding='utf-8')
    if key is not None:
        sign(out/'release.json', key, public or launcher_key())
    return meta


def launcher_key(update_rs=UPDATE_RS):
    """The public key the launcher being packaged checks with (update.rs RELEASE_KEY), '' when not set."""
    found = re.search(r'pub const RELEASE_KEY: &str = "([^"]*)";', Path(update_rs).read_text(encoding='utf-8'))
    if not found:
        raise ValueError(f'no RELEASE_KEY in {update_rs}')
    return found.group(1)


def sign(release_json, key, public):
    """release.json.sig beside `release_json`. Refused: a key inside the repo (it would be committed), a launcher with
    no key (it would take no release), or a key other than the one the launcher carries (it would take none of these)."""
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    key = Path(key).resolve()
    if key.is_relative_to(ROOT):
        raise ValueError(f'the release key {key} is inside the repo: keep it outside, offline')
    if len(public) != 64:
        raise ValueError('update.rs RELEASE_KEY is not set: paste the key\'s public half there first (its doc comment)')
    private = serialization.load_pem_private_key(key.read_bytes(), password=None)
    if not isinstance(private, Ed25519PrivateKey):
        raise ValueError(f'{key} is not an ed25519 key')
    raw = private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    if raw.hex() != public.lower():
        raise ValueError(f'{key} is not the key update.rs RELEASE_KEY names')
    release_json = Path(release_json)
    release_json.with_name(release_json.name + '.sig').write_bytes(private.sign(release_json.read_bytes()))


if __name__ == '__main__':
    package(*sys.argv[1:4], key=sys.argv[4] if len(sys.argv) > 4 else None)
    print(f'Release packages in {sys.argv[3]}')
