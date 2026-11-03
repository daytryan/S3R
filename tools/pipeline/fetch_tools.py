# tools/pipeline/fetch_tools.py
"""Third-party tools the build needs, downloaded once from fixed URLs and checked against pinned SHA-256."""
import hashlib, shutil, ssl, time, urllib.error, urllib.parse, urllib.request, zipfile
from pathlib import Path
import certifi

PINS = {  # name: (url, sha256 of the download, path of the program inside it); from Task 5 Step 1
    'java': ('https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.9%2B10/'
             'OpenJDK21U-jre_x64_windows_hotspot_21.0.9_10.zip',
             '39c5e23f3ce4d420663afba8ffde28034b72e2b3e240943dc2321bc1f912eef9',
             'jdk-21.0.9+10-jre/bin/java.exe'),
    'unluac': ('https://downloads.sourceforge.net/project/unluac/Unstable/unluac_2015_06_13.jar',
               'c3533ad8d67905d449555edf5497a9898083fc966d247b8e7a39ab64b5218115',
               'unluac.jar'),
    'vgmstream': ('https://github.com/vgmstream/vgmstream/releases/download/r2117/vgmstream-win64.zip',
                  '6c4a8a3813864fefed081bbd337dbc0ad93bf88e0b92f5db98d7ab258b22dc6c',
                  'vgmstream-cli.exe'),
    'ffmpeg': ('https://github.com/GyanD/codexffmpeg/releases/download/9.0.1/ffmpeg-9.0.1-essentials_build.zip',
               'fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9',
               'ffmpeg-9.0.1-essentials_build/bin/ffmpeg.exe'),
    # Xenia's public source, for the retail XEX key build_audio_image.py decrypts the discs' exes with (not kept here)
    'xenia-key': ('https://raw.githubusercontent.com/xenia-project/xenia/d87bf995e148707c88389dda177cf6fbcfe57b34/'
                  'src/xenia/cpu/xex_module.cc',
                  '36a30e1bc54b9854cbd2572b74e366e733460c5df6edf2b29542de36575858f2',
                  'xex_module.cc'),
}
ENV = {'java': 'SKATE_JAVA', 'unluac': 'SKATE_UNLUAC', 'vgmstream': 'SKATE_VGMSTREAM', 'ffmpeg': 'SKATE_FFMPEG',
       'xenia-key': 'SKATE_XEX_KEY_SOURCE'}


def download(url, into, progress=lambda text: None, name='the file'):
    """Streams `url` into the file `into`, 1 MB at a time (E4: not held in memory), saying how far it got at most twice
    a second; a file already part-downloaded there is continued (Range) when the server allows, else started again.
    Verified against Mozilla's roots (certifi, the list the launcher's own downloads trust) first: Python reads the
    Windows store through OpenSSL, and some PCs' stores hold an expired certificate it picks ("certificate has
    expired", build 8). Then the Windows store, for antivirus and company proxies that re-sign HTTPS with a root only
    Windows knows. Either way the pinned SHA-256 is what vouches for the file."""
    into = Path(into)
    have = into.stat().st_size if into.is_file() else 0
    headers = {'User-Agent': 'Skate3Remaster-Pipeline/1', **({'Range': f'bytes={have}-'} if have else {})}
    request = urllib.request.Request(url, headers=headers)
    contexts = [ssl.create_default_context(cafile=certifi.where()), ssl.create_default_context()]
    for i, context in enumerate(contexts):
        try:
            with urllib.request.urlopen(request, timeout=60, context=context) as response:
                resumed = have and getattr(response, 'status', 200) == 206
                done = have if resumed else 0
                total = response.headers.get('Content-Length') if getattr(response, 'headers', None) else None
                total = done + int(total) if total and total.isdigit() else None
                said = 0.0
                with open(into, 'ab' if resumed else 'wb') as out:
                    while block := response.read(1 << 20):
                        out.write(block)
                        done += len(block)
                        if total and time.monotonic() - said >= 0.5:
                            said = time.monotonic()
                            progress(f'Downloading {name} ({100 * done // total}%)')
                return
        except urllib.error.URLError as error:
            if have and getattr(error, 'code', None) == 416:
                return  # nothing left past what is here: the whole file (the checksum says)
            if not isinstance(error.reason, ssl.SSLCertVerificationError) or i == len(contexts) - 1:
                raise


def file_sha256(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def tool(tools_dir, name, progress):
    url, digest, inner = PINS[name]
    home = Path(tools_dir)/name
    program = home/inner
    if program.is_file():
        return program
    progress(f'Downloading {name}')
    got = home.with_name(name + '.download')  # kept between tries (and builds): the next one continues it
    got.parent.mkdir(parents=True, exist_ok=True)
    for attempt in range(3):  # #11: a dropped connection or a timeout is tried again, after 2 s then 4 s
        try:
            download(url, got, progress, name)
            break
        except OSError as error:  # URLError, TimeoutError, ConnectionResetError
            if attempt == 2:
                reason = getattr(error, 'reason', error)
                raise OSError(f'Downloading {name} from {urllib.parse.urlsplit(url).hostname} failed: {reason}') from error
            time.sleep(2 << attempt)
    if file_sha256(got) != digest:
        got.unlink(missing_ok=True)  # a bad or mixed file is never continued
        raise ValueError(f'{name} download failed its checksum; try again later')
    part = home.with_name(name + '.part')  # unpack beside, then rename: a crash never leaves a half-unpacked tool
    shutil.rmtree(part, ignore_errors=True)
    part.mkdir(parents=True)
    if url.endswith('.zip'):
        with zipfile.ZipFile(got) as z:
            z.extractall(part)  # pinned by hash above, so the archive is a known one
    else:
        shutil.copyfile(got, part/inner)
    shutil.rmtree(home, ignore_errors=True)
    part.replace(home)
    got.unlink(missing_ok=True)
    return program


def ensure(tools_dir, progress):
    return {ENV[name]: str(tool(tools_dir, name, progress)) for name in PINS}
