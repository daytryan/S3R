"""RUST_CRATE_LICENSES.txt: every crates.io crate in Cargo.lock, its license and the license files it ships, for the
release package (release review 10-18: the exes link them, and MIT/Apache/BSD ask for their notices to go along).
Offline: read from the local cargo registry the build already downloaded into. A crate whose source is missing there
stops it (the list must be whole).

    python -m tools.pipeline.crate_licenses OUT_FILE
"""
import os, re, sys, tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LICENSE_FILE = re.compile(r'^(licen[cs]e|copying|notice|copyright)', re.I)


def registry_dirs():
    home = Path(os.environ.get('CARGO_HOME', Path.home()/'.cargo'))
    return sorted((home/'registry'/'src').glob('*'))


def vendored(name):
    """A crates.io crate the workspace patches with its own copy (Cargo.toml [patch.crates-io], vendor/<name>)."""
    return ROOT/'vendor'/name if (ROOT/'vendor'/name/'Cargo.toml').is_file() else None


def crates(lock=ROOT/'Cargo.lock'):
    """(name, version) of every crates.io package in the lock file, the vendored copies included (Cargo.lock lists
    them without a source), sorted."""
    packages = tomllib.loads(Path(lock).read_text(encoding='utf-8')).get('package', [])
    return sorted({(p['name'], p['version']) for p in packages
                   if p.get('source', '').startswith('registry+') or ('source' not in p and vendored(p['name']))})


def notice(name, version, dirs):
    source = vendored(name) or next((d/f'{name}-{version}' for d in dirs if (d/f'{name}-{version}').is_dir()), None)
    if source is None:
        raise FileNotFoundError(f'{name} {version}: not in the local cargo registry; run a build first')
    manifest = tomllib.loads((source/'Cargo.toml').read_text(encoding='utf-8')).get('package', {})
    files = sorted(p for p in source.iterdir() if p.is_file() and LICENSE_FILE.match(p.name))
    named = manifest.get('license-file')
    if named and (source/named).is_file() and source/named not in files:
        files.append(source/named)
    lines = [f'== {name} {version}', f'License: {manifest.get("license") or "see the file below"}']
    if manifest.get('authors'):
        lines.append('Authors: ' + ', '.join(manifest['authors']))
    for f in files:
        lines += [f'--- {f.name}', f.read_text(encoding='utf-8', errors='replace').strip()]
    if not files:
        lines.append('(the crate ships no license file: its license is the one named above, standard texts at the end)')
    return '\n'.join(lines)


def write(out, lock=ROOT/'Cargo.lock'):
    dirs = registry_dirs()
    listed = crates(lock)
    text = ['Licenses of the Rust crates the Skate 3 Remaster programs are built from (crates.io, per Cargo.lock).', '']
    text += [notice(n, v, dirs) + '\n' for n, v in listed]
    # The standard texts, for the crates that name a license without shipping its file (the copyright holders are
    # the authors listed with each). Their other licenses (MIT-0, CC0) ask for no notice; MPL-2.0's source is crates.io.
    for name in ('MIT', 'APACHE'):
        text += [f'== The {name} license text', (ROOT/f'vendor/bevy_pbr/LICENSE-{name}').read_text(encoding='utf-8').strip(), '']
    Path(out).write_text('\n'.join(text), encoding='utf-8', newline='\n')
    return len(listed)


if __name__ == '__main__':
    print(f'{write(sys.argv[1])} crates')
