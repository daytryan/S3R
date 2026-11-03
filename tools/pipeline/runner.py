# tools/pipeline/runner.py
"""skate3pipeline (`skate3setup.exe --pipeline`): build every stage from the player's own files.

Resume: a stage is finished when installation.json has its current code fingerprint (pipelines[name]), the same
input identities (inputs[name]), and none of its roots changed since the stage finished. Each finished stage records
a receipt of its own roots (outputs['stages']) in the same atomic write as its fingerprint, so the game's startup
check covers everything and a later failure never leaves a finished stage with a stale receipt.

Side by side: a stage starts once every stage in its `after` has finished, and never next to a stage whose roots
overlap its own; up to workers() stages run at once while their MEMORY peaks fit. The longest remaining chain starts
first."""
import argparse, concurrent.futures as cf, contextlib, ctypes, functools, hashlib, json, math, os, pickle, platform, shutil, subprocess, sys, tempfile, threading, time, traceback
from pathlib import Path

from tools.asset_pipeline import group_receipts, setup_state
from tools.asset_pipeline.setup_state import atomic_json, file_sizes, receipt, valid_receipt
from tools.pipeline import sources, stages
from tools.pipeline.stages import STAGES

ROOT = Path(getattr(sys, '_MEIPASS', Path(__file__).resolve().parents[2]))
TOOLS = ROOT/'tools'
FIRST_BUILD_GB, UPDATE_GB = 45, 15
# Windows words the message in the PC's language after its number: 112 disk full, 39 handle disk full.
NO_SPACE = ('[Errno 28]', 'No space left', 'not enough space on the disk', 'WinError 112]', 'WinError 39]')
PLAIN = (  # #12: (texts in the error or the log's tail, what the player reads)
    (('MemoryError',), 'the PC ran out of memory. Close other programs, then try again.'),
    (('WinError 5]', 'WinError 32]', 'WinError 33]'), 'a file is in use, often by antivirus or OneDrive. Close it, then try again.'),
    (('WinError 206]',), 'a path is too long. Move the game to a shorter folder, such as C:\\Games\\Skate 3 Remaster.'),
)
NO_WINDOW = 0x08000000 if os.name == 'nt' else 0

# Seconds each stage took on the user's 10-07 build (12-thread laptop, NVMe), receipt included; old-clothing as measured
# with its tiles drawn on several processes. Only the ratios matter: the progress bar scales them by how fast this PC
# has been so far, and the longest chains start first.
WEIGHTS = {  # seconds each stage took in the fresh build of 10-08 evening (6 cores, beside the others, 8.4 GB free;
    # tables: its work, 56 s, before it waited for skate3-base's install to record)
    'skate3-base': 223, 'tables': 56, 'audio-image': 1, 'runtime-disc-files': 15, 's3-dlc-maps': 126, 'mod-maps': 266,
    'skate2-parks': 66, 'skate1-map': 203, 'backdrops': 3, 'dlc-clothing': 45, 'old-clothing': 184,
    'pro-clothing': 88, 'stock-graphics': 23, 'rainbow-deck': 24, 'old-skies': 3, 'old-cameras': 1, 'old-grading': 5,
    'hud-skate2': 3, 'hud-skate1': 22, 'locations': 16, 'skate2-tutorial': 57, 'skate1-challenges': 7,
    'skate2-ambience': 1, 'online-ui': 4, 'replay-ui': 10, 'online-movies': 2, 'online-challenge-map': 18, 'online-skate2': 59,
    'online-skate1': 31, 'hall-of-meat-s3': 2, 'online-skate3-export': 72, 'online-skate3': 20, 'boot': 36,
    'pad-glyphs': 1, 'party-play': 10, 'retail-screens': 10, 'weather-audio': 1, 'skate2-physics': 2,
    'skate3-career': 67, 'old-cas': 3, 'game-sounds': 55,
}
PRIOR = 60  # weight-seconds of "as fast as WEIGHTS' PC" the bar starts from, until finished stages say otherwise
# E1: shown with the running stages while one is held back for memory (build()).
MEMORY_WAIT = "Waiting for memory: closing other programs speeds this up"
# What the launcher says a running stage is doing, until the stage reports something itself.
LABELS = {
    'skate3-base': 'Unpacking Skate 3', 'tables': 'Skate 1 and 2 tables', 'audio-image': 'Skate 3 audio', 'runtime-disc-files': 'Skate 3 audio files',
    's3-dlc-maps': 'Skate 3 DLC maps', 'mod-maps': 'New San Vanelona', 'skate2-parks': 'Skate 2 parks', 'skate1-map': 'The Skate 1 map',
    'backdrops': 'Edit Skater backdrops', 'dlc-clothing': 'DLC clothing', 'old-clothing': 'Skate 1 and 2 clothing',
    'pro-clothing': 'Pro skater clothing', 'stock-graphics': 'Graphics and logos', 'rainbow-deck': 'Rainbow deck',
    'old-skies': 'Skate 1 and 2 skies', 'old-cameras': 'Skate 1 and 2 cameras', 'old-cas': 'Skate 2 Edit Skater',
    'old-grading': 'Colour grading',
    'hud-skate2': 'Skate 2 HUD', 'hud-skate1': 'Skate 1 HUD', 'locations': 'Locations',
    'skate2-tutorial': 'Skate 2 tutorial', 'skate1-challenges': 'Skate 1 challenges', 'skate2-ambience': 'Skate 1 and 2 ambience',
    'online-ui': 'Online menus', 'replay-ui': 'Replay editor', 'online-movies': 'Online messages', 'online-challenge-map': 'Online challenge map',
    'online-skate2': 'Skate 2 online modes', 'online-skate1': 'Skate 1 online modes', 'hall-of-meat-s3': 'Hall of Meat',
    'online-skate3-export': 'Skate 3 online modes', 'online-skate3': 'Skate 3 online modes', 'boot': 'Start-up screens', 'pad-glyphs': 'Controller buttons',
    'party-play': 'Party Play', 'retail-screens': 'Front-end screens', 'weather-audio': 'Weather sounds',
    'skate2-physics': 'Skate 2 physics',
    'skate3-career': 'Skate 3 story mode', 'game-sounds': 'Preparing game sounds',
}
STEPS = 8  # the launcher's build plan: 0 check, 1 Skate 3 ... 7 finish (Stage.step)
# Scratch folders outside the roots that several stages rebuild from scratch: those stages never run side by side.
SCRATCH = {name: ('work/dlc-s3',) for name in ('dlc-clothing', 'old-clothing', 'pro-clothing')}
# Scripts that rewrite installation.json themselves: they hold the marker lock while they run, like record() does.
WRITES_MARKER = {'pipeline/converter/add_teleports.py'}
# The map catalogs every map stage edits. Each edit holds _MARKER (stages.catalog_map, add_teleports.py), so the map
# stages run side by side, and record() hashes these under _MARKER too, never an older copy.
SHARED = frozenset(stages.TELEPORT_ROOTS)

_OUT = None                 # main() pins the real stdout: package() points sys.stdout at a log while it extracts
_EMIT, _MARKER = threading.Lock(), threading.RLock()  # _MARKER: every read and write of installation.json
_LIVE = []                  # contexts of running builds, stopped if the launcher goes away


def emit(event, **fields):
    line = json.dumps({'event': event, **fields})
    with _EMIT:
        try:
            print(line, file=_OUT or sys.stdout, flush=True)
        except OSError:
            if _OUT is None:
                raise
            # The launcher is gone (closed, crashed or ended in Task Manager) and the build ticks every 2 s, so this is
            # noticed at once: stop every script, keep what finished (each stage was recorded as it ended), and exit.
            for ctx in list(_LIVE):
                ctx.cancel()
            os._exit(3)


_plain_sha, _hashes = setup_state.sha, {}


def _sha(path, known=None):
    """setup_state.sha, remembered for this run: a file hashed earlier and unchanged since (size, modified time) is not
    read again. Every stage's receipt hashes its roots, and the five wardrobe stages share one large root. `known`: the
    file's (size, modified ns) from a listing (listing()), instead of a stat of its own.
    ponytail: a same-size rewrite inside one mtime tick would keep its old hash; the next build would call that stage
    damaged and redo it, nothing worse."""
    if known is None:
        stat = os.stat(path)
        known = stat.st_size, stat.st_mtime_ns
    key = (str(path), *known)
    if key not in _hashes:
        _hashes[key] = _plain_sha(path)
    return _hashes[key]


setup_state.sha = _sha  # receipt() and valid_receipt() look sha up when they run
HASHES = 'hashes.json'  # _sha's memory between builds, beside installation.json


def load_hashes(base):
    """A resumed or updated build trusts a file whose size and modified time match its last hash (re-reading every
    finished file took over a minute on a 12 GB install). ponytail: a rewrite that keeps both is missed; Rebuild clears it."""
    with contextlib.suppress(OSError, ValueError, TypeError):
        for path, size, mtime, digest in json.loads((Path(base)/HASHES).read_text(encoding='utf-8')):
            _hashes[(path, size, mtime)] = digest


_SAVED = [0.0]  # when save_hashes last wrote


def save_hashes(base, every=0, keep=None):
    """`every`: seconds since the last save below which this one is skipped (E2: a stage's record rewrote all of
    hashes.json; build() saves once more at its end, stopped or not). Losing some only means hashing them again.
    `keep` (D3, a finished build): only these paths are saved, so files gone since (Clear audio, a renamed output, a
    stage that stopped making one) stop riding along in every later load."""
    if every and time.monotonic() - _SAVED[0] < every:
        return
    _SAVED[0] = time.monotonic()
    latest = {}
    for (path, size, mtime), digest in list(_hashes.items()):  # one C-level copy: other threads may be adding
        if keep is not None and path not in keep:
            continue
        if path not in latest or mtime > latest[path][1]:
            latest[path] = (size, mtime, digest)
    with contextlib.suppress(OSError):
        atomic_json(Path(base)/HASHES, [[path, *entry] for path, entry in latest.items()])


def listing(path):
    """{file: (size, modified ns)} under `path` (a file or a folder) from one directory walk: on Windows the listing
    carries both, where rglob and a stat per file took 10-25 s for each wardrobe stage's ~47,000 files (10-08)."""
    if path.is_file():
        stat = path.stat()
        return {path: (stat.st_size, stat.st_mtime_ns)}
    found, stack = {}, [str(path)] if path.is_dir() else []
    while stack:
        with os.scandir(stack.pop()) as entries:
            for entry in entries:
                if entry.is_dir(follow_symlinks=False):
                    stack.append(entry.path)
                elif entry.is_file():
                    stat = entry.stat()
                    found[Path(entry.path)] = stat.st_size, stat.st_mtime_ns
    return found


def tree(root):
    """{posix path under root: (size, modified ns)} from one directory walk: file_sizes() with the times, which the
    Windows listing carries too, so _sha needs no stat per file (51,000 of them took ~6.5 s, 10-17)."""
    found, stack, cut = {}, [os.path.join(root, '')], len(os.path.join(root, ''))
    while stack:
        with os.scandir(stack.pop()) as entries:
            for entry in entries:
                if entry.is_dir(follow_symlinks=False):
                    stack.append(entry.path)
                elif entry.is_file(follow_symlinks=False):
                    stat = entry.stat(follow_symlinks=False)
                    found[entry.path[cut:].replace(os.sep, '/')] = stat.st_size, stat.st_mtime_ns
    return found


def stale(root, receipts, files=None):
    """The receipt entries ({name: {size, sha256}}) whose file is missing or changed: the sizes from one directory walk,
    then the SHA-256s 8 at a time, each file opened once (valid_receipt's resolve-and-stat per file took minutes).
    `files`: tree(root) already walked (build() checks the stage and the group receipts from one walk)."""
    root = Path(root).resolve()  # the spelling valid_receipt hashes, so its later checks find these in _sha's memory
    files = tree(root) if files is None else files
    bad = {name for name, entry in receipts.items()
           if not isinstance(entry, dict) or (files.get(name) or (None,))[0] != entry.get('size')}
    todo = [name for name in receipts if name not in bad]

    def digest(name):
        try:
            return _sha(root/name, files[name])
        except OSError:
            return None
    with cf.ThreadPoolExecutor(HASHERS) as pool:
        bad.update(name for name, found in zip(todo, pool.map(digest, todo)) if found != receipts[name].get('sha256'))
    return bad


HASHERS = 8  # threads hashing the install's files (stale, record): 1 when the game is on a hard drive (#10)


def memory_gb():
    if os.name != 'nt':
        return os.sysconf('SC_PAGE_SIZE') * os.sysconf('SC_PHYS_PAGES') / (1 << 30)

    class Status(ctypes.Structure):  # MEMORYSTATUSEX
        _fields_ = [('length', ctypes.c_ulong), ('load', ctypes.c_ulong), ('total', ctypes.c_ulonglong),
                    ('rest', ctypes.c_ulonglong * 6)]
    status = Status(length=ctypes.sizeof(Status))
    ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status))
    return status.total / (1 << 30)


class _JobLimits(ctypes.Structure):  # JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    _fields_ = [('process_time', ctypes.c_int64), ('job_time', ctypes.c_int64), ('flags', ctypes.c_uint32),
                ('min_set', ctypes.c_size_t), ('max_set', ctypes.c_size_t), ('processes', ctypes.c_uint32),
                ('affinity', ctypes.c_size_t), ('priority', ctypes.c_uint32), ('scheduling', ctypes.c_uint32),
                ('io', ctypes.c_uint64 * 6), ('process_limit', ctypes.c_size_t), ('job_limit', ctypes.c_size_t),
                ('peak_process', ctypes.c_size_t), ('peak_job', ctypes.c_size_t)]


def watch_memory(child):
    """A job holding the child and everything it starts, for peak_memory(). None off Windows or when it can't be made."""
    if os.name != 'nt':
        return None
    kernel = ctypes.windll.kernel32
    kernel.CreateJobObjectW.restype = ctypes.c_void_p
    job = kernel.CreateJobObjectW(None, None)
    if job and kernel.AssignProcessToJobObject(ctypes.c_void_p(job), ctypes.c_void_p(int(child._handle))):
        limits = _JobLimits(flags=0x2000)  # KILL_ON_JOB_CLOSE: a runner that dies takes its scripts with it
        kernel.SetInformationJobObject(ctypes.c_void_p(job), 9, ctypes.byref(limits), ctypes.sizeof(limits))
        return job
    if job:
        kernel.CloseHandle(ctypes.c_void_p(job))
    return None


def peak_memory(job):
    """The combined peak committed memory (GB) of a watch_memory job's processes. Closes the job."""
    limits = _JobLimits()
    kernel = ctypes.windll.kernel32
    kernel.QueryInformationJobObject(ctypes.c_void_p(job), 9, ctypes.byref(limits), ctypes.sizeof(limits), None)
    kernel.CloseHandle(ctypes.c_void_p(job))
    return limits.peak_job / (1 << 30)


def workers():
    """Most stages at once: half the hardware threads (a stage is mostly one Python process). Memory decides the rest
    (fits). SKATE_BUILD_JOBS overrides."""
    if os.environ.get('SKATE_BUILD_JOBS', '').isdigit():
        return max(1, int(os.environ['SKATE_BUILD_JOBS']))
    return max(2, (os.cpu_count() or 4) // 2)


# Peak memory (GB) of a stage's scripts together: the `peak` its log prints. Unlisted stages take DEFAULT_GB.
# ponytail: one fixed peak per stage, measured on this PC's build; per-PC scaling if testers' logs disagree
# Peak GB of a stage's biggest script and its children, from the runner's `peak` log lines (fresh build 10-08); the
# map stages' load check runs after their converter, with its own brief peak (Context.reserve, stages.CHECK_GB).
# The three clothing stages: measured again after their 10-08 memory cuts (lazy tile designs, small sets in one process).
MEMORY = {'skate3-base': 6, 's3-dlc-maps': 1.5, 'mod-maps': 3.5, 'skate1-map': 2.5, 'skate2-parks': 1.5,
          'dlc-clothing': 1.7, 'old-clothing': 3.8, 'pro-clothing': 3.5, 'skate2-tutorial': 2.5, 'skate1-challenges': 2.5,
          'online-skate2': 2.5, 'online-skate1': 2.5, 'online-skate3-export': 1.5, 'boot': 1.5, 'hud-skate1': 1, 'skate3-career': 1}
DEFAULT_GB = 1.0
# #15: the stages that spread work over fan-out workers (fanout.py); MEMORY was measured with MEASURED_FANOUT of them.
FANNED = frozenset({'skate3-base', 'dlc-clothing', 'old-clothing', 'pro-clothing', 'skate2-tutorial', 'skate1-challenges'})
MEASURED_FANOUT, LOW_MEMORY_GB = 6, 8.5
LOW_FANOUT = None  # fan-out workers this build is held to (build(): low memory mode, or the player's SKATE_FANOUT)


def peak_gb(name):
    """A stage's planned peak: MEMORY, and for a fan-out stage held to fewer workers, scaled down with them.
    ponytail: half of a fanned peak is taken as the workers' (unmeasured split); testers' `peak` lines can replace it."""
    gb = MEMORY.get(name, DEFAULT_GB)
    if LOW_FANOUT and name in FANNED and LOW_FANOUT < MEASURED_FANOUT:
        gb *= 0.5 + 0.5 * LOW_FANOUT / MEASURED_FANOUT
    return gb


RESERVE_GB = 3.5  # Windows, the launcher and whatever else the player has open
SPARE_GB = 1.0    # of what was free when the build started
EXTRA, _RESERVE = {}, threading.Lock()  # stage -> GB it holds above its MEMORY for a moment (Context.reserve)


def budget_gb(using=0.0):
    """The memory the stages may plan on: what is free now, less SPARE_GB, plus `using` (what the running stages hold
    now, which is already out of "free" and inside their planned peaks), and at most the total less RESERVE_GB. Other
    programs keep what they hold (10-08: ~7 GB in use with only the launcher open, and a build planned on the total ran
    Windows out of memory). E1: build() asks again at every scheduling turn (every 2 s), so a browser closed during the
    build lets the big stages overlap, and one opened shrinks the plan."""
    from tools.pipeline.fanout import available_gb
    free = available_gb()
    return memory_gb() - RESERVE_GB if free is None else min(memory_gb() - RESERVE_GB, free - SPARE_GB + using)


class _JobMemory(ctypes.Structure):  # JOBOBJECT_MEMORY_USAGE_INFORMATION (JobObjectMemoryUsageInformation, 28)
    _fields_ = [('job', ctypes.c_uint64), ('peak', ctypes.c_uint64)]


def job_gb(job):
    """The committed memory (GB) a watch_memory job's processes hold now; 0 when Windows can't say (counted as
    nothing used yet, the cautious side)."""
    info = _JobMemory()
    ok = ctypes.windll.kernel32.QueryInformationJobObject(ctypes.c_void_p(job), 28, ctypes.byref(info), ctypes.sizeof(info), None)
    return info.job / (1 << 30) if ok else 0.0


def fits(stage, running, budget):
    """A stage may start beside the running ones while their peaks (and any brief EXTRA) add up to the budget. Alone,
    anything starts."""
    need = lambda s: peak_gb(s.name)
    return not running or sum(map(need, running)) + sum(EXTRA.values()) + need(stage) <= budget


def reservation(stage, running, started, budget):
    """EASY backfilling for a stage that doesn't fit yet: when it can start (the running stages end at their WEIGHTS
    time from `started`, freeing their MEMORY) and the GB spare beside it then. A stage behind it starts now only if
    it is done by then or fits in that spare, so small stages fill the cores without pushing the longest chain back."""
    need = lambda s: peak_gb(s.name)
    now = time.monotonic()
    ends = sorted((max(now, started.get(s.name, now) + WEIGHTS.get(s.name, 30)), need(s)) for s in running)
    used = sum(n for _, n in ends) + sum(EXTRA.values())
    for when, freed in ends:
        used -= freed
        if used + need(stage) <= budget:
            return when, budget - used - need(stage)
    return (ends[-1][0] if ends else now), 0.0  # it runs alone once the others are done


def stage_fingerprints(cache=None):
    """versions.fingerprints() leaves out skate3-base (it is the five groups); the stage table still needs its fingerprint.
    `cache` (the launch check's): reused while no file under tools/ changed size or time (a listing, ~0.03 s, against
    ~1.2 s of hashing on every launcher open). A build always hashes."""
    from tools.asset_pipeline.versions import fingerprints
    stamp = None
    if cache:
        stamp = hashlib.sha256(repr(sorted((str(p), *v) for p, v in listing(TOOLS).items())).encode()).hexdigest()
        with contextlib.suppress(OSError, ValueError, TypeError, KeyError):
            saved = json.loads(Path(cache).read_text(encoding='utf-8'))
            if saved['stamp'] == stamp:
                return saved['prints']
    prints = with_base(fingerprints())
    if cache:
        with contextlib.suppress(OSError):
            atomic_json(Path(cache), {'stamp': stamp, 'prints': prints})
    return prints


def with_base(prints):
    """`prints` (versions.fingerprints(), or a release.json's asset_pipelines) plus skate3-base's: its five groups."""
    from tools.asset_pipeline.versions import GROUPS
    lines = ''.join(f'{g}={prints.get(g)}\n' for g in sorted(GROUPS))
    return {**prints, 'skate3-base': hashlib.sha256(lines.encode()).hexdigest()}


class Context:
    def __init__(self, base, game_exe, found):
        self.base, self.game_exe, self.found = Path(base), Path(game_exe), found
        self.work, self.logs, self.tools = self.base/'work', self.base/'logs', self.base/'tools'
        # PYTHONHASHSEED: the scripts walk sets of names; a fixed seed makes the same discs give the same files
        self.env = {'PYTHONPATH': str(ROOT), 'SKATE_WORK': str(self.work/'cache'),
                    'SKATE_TABLES': str(self.work/'tables'), 'PYTHONHASHSEED': '0'}
        self.bar = None  # build()'s Progress while stages run
        self.shared = _MARKER  # held by a stage while it edits a SHARED file
        self.cancelled, self._children = False, set()
        self._local, self._lock, self._extracting = threading.local(), threading.Lock(), threading.Lock()
        self._discs = {}  # item id -> its extraction lock (disc)
        self.phases = {}  # B9: seconds spent outside the stages (source check, tool downloads, seal, cleanup)
        self.unchanged = set()  # #4: stages this build that ran and wrote the same files, or were cut off by that
        self._marker = None  # E2: _cached()'s ((size, mtime ns), pickled marker, directory)
        self._jobs = {}  # E1: stage -> the watch_memory jobs of its scripts running now
        self.peaks = {}  # D2: stage -> the highest peak GB of its scripts, for build-times.json

    @contextlib.contextmanager
    def timed(self, phase):
        began = time.monotonic()
        try:
            yield
        finally:
            self.phases[phase] = round(self.phases.get(phase, 0) + time.monotonic() - began, 1)

    @property
    def stage(self):
        """The stage running on this thread; in-process output goes to its log."""
        return getattr(self._local, 'stage', 'package')

    @stage.setter
    def stage(self, name):
        self._local.stage = name

    @contextlib.contextmanager
    def reserve(self, stage, gb):
        """A brief peak of `gb` inside a stage (a big map's load check), one at a time: no new stage starts while it is
        held, and it waits (at most 5 min) until that much memory is free, or until no other stage runs: then nothing more
        will be freed (a PC with less than `gb` free even at idle would sit out the 5 min)."""
        from tools.pipeline.fanout import available_gb

        def others():
            if self.bar is None:
                return True
            with self.bar.lock:
                return any(n != stage for n in self.bar.started)
        with _RESERVE:
            EXTRA[stage] = max(0.0, gb - peak_gb(stage))
            try:
                end = time.monotonic() + 300
                while (available_gb() or gb) < gb and time.monotonic() < end and not self.cancelled and others():
                    time.sleep(1)
                yield
            finally:
                EXTRA.pop(stage, None)

    def _cached(self, marker=None):
        """E2: (pickled marker, its directory) of installation.json, parsed again only when the file's size or modified
        time changed (~0.08 s a parse of 16-23 MB, and stages reach it through ctx.install dozens of times). skate3-base
        writes it without _MARKER (install.publish_marker) and moves the install to a new folder, hence the stat on
        every call. `marker`: what this process just wrote, remembered instead of parsed back. None: no file."""
        path = self.base/'installation.json'
        with _MARKER:  # Windows can't replace a file another thread has open
            try:
                stat = path.stat()
            except FileNotFoundError:
                return None
            key = stat.st_size, stat.st_mtime_ns
            if marker is None and self._marker and self._marker[0] == key:
                return self._marker[1:]
            if marker is None:
                marker = setup_state.read_marker(path)
            directory = marker.get('directory') if isinstance(marker, dict) else None
            self._marker = key, pickle.dumps(marker, pickle.HIGHEST_PROTOCOL), directory
            return self._marker[1:]

    @property
    def marker(self):
        """installation.json as a fresh copy (unpickled: ~0.03 s, where deepcopy took 0.16 s): callers edit it."""
        cached = self._cached()
        return {} if cached is None else pickle.loads(cached[0])

    def write_marker(self, marker):
        """installation.json written under _MARKER and remembered, so the next ctx.marker doesn't parse it back."""
        with _MARKER:
            atomic_json(self.base/'installation.json', marker)
            self._cached(marker)

    @property
    def install(self):
        cached = self._cached()
        if cached is None or cached[1] is None:
            raise KeyError('directory')  # before skate3-base publishes it, as before
        return self.base/cached[1]

    def item(self, item_id):
        return Path(self.found[item_id]['path'])

    def disc(self, item_id):
        """The extracted disc folder, extracted on first use (work/discs/<id>/.complete marks a full copy)."""
        out = self.work/'discs'/item_id
        if not (out/'.complete').is_file():
            # stages side by side may want the same disc: one extracts, the others wait; other discs and the packages
            # extract meanwhile (B7: one lock per disc, setdefault is atomic)
            with self._discs.setdefault(item_id, threading.Lock()):
                if not (out/'.complete').is_file():
                    from tools.pipeline.xdvdfs import Iso
                    shutil.rmtree(out, ignore_errors=True)
                    with Iso(self.item(item_id)) as iso:
                        iso.extract(out)
                    (out/'.complete').write_text('')
        return out

    def package(self, item_id):
        """The unpacked STFS package folder, unpacked on first use."""
        out = self.work/'dlc'/item_id
        if not (out/'.complete').is_file():
            with self._extracting:  # also the only place that swaps sys.stdout, so two swaps never interleave
                if not (out/'.complete').is_file():
                    entry = str(TOOLS/'pipeline/dlc')
                    if entry not in sys.path:
                        sys.path.insert(0, entry)
                    from stfs_extract import extract
                    shutil.rmtree(out, ignore_errors=True)
                    self.logs.mkdir(parents=True, exist_ok=True)
                    # the extractor prints a line per file; stdout is the launcher's event stream, so it goes to the log
                    with open(self.logs/f'{self.stage}.log', 'a', encoding='utf-8', errors='replace') as log, \
                            contextlib.redirect_stdout(log):
                        extract(str(self.item(item_id)), str(out))
                    (out/'.complete').write_text('')
        return out

    def run(self, stage, argv, env=None):
        self.logs.mkdir(parents=True, exist_ok=True)
        with open(self.logs/f'{stage}.log', 'a', encoding='utf-8', errors='replace') as out:
            out.write(f'\n$ {[str(a) for a in argv]}\n')
            out.flush()
            started = time.monotonic()
            with self._lock:
                if self.cancelled:
                    raise RuntimeError('stopped: another stage failed')
                child = subprocess.Popen([str(a) for a in argv], stdout=out, stderr=subprocess.STDOUT,
                                         env={**os.environ, **self.env, **(env or {})}, creationflags=NO_WINDOW)
                self._children.add(child)
            job = watch_memory(child)
            if job:
                with self._lock:
                    self._jobs.setdefault(stage, set()).add(job)
            try:
                code = child.wait()
            finally:
                with self._lock:
                    self._children.discard(child)
                    self._jobs.get(stage, set()).discard(job)
            gb = peak_memory(job) if job else None
            peak = f', peak {gb:.2f} GB' if job else ''  # what the stage admission's MEMORY table is from
            if job:
                self.peaks[stage] = max(self.peaks.get(stage, 0), round(gb, 2))
            out.write(f'# exit code {code} after {time.monotonic() - started:.0f} s{peak}\n')
        if code:
            raise RuntimeError(f'a build step exited with code {code} (see logs/{stage}.log)')

    def using_gb(self, names):
        """E1: what the running stages `names` hold now, each counted up to its planned peak (peak_gb). A stage running
        in this process (skate3-base, a copy) counts 0: its use is already out of what is free."""
        with self._lock:  # held while asking: run() closes a job (peak_memory) once it is out of _jobs
            return sum(min(peak_gb(n), sum(map(job_gb, self._jobs.get(n, ())))) for n in names)

    def cancel(self):
        """Stop every running script and what it started (a frozen script is two processes): a stage failed."""
        with self._lock:
            self.cancelled = True
            children = list(self._children)
        for child in children:
            if os.name == 'nt':
                subprocess.call(['taskkill', '/T', '/F', '/PID', str(child.pid)], stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, creationflags=NO_WINDOW)
            else:
                child.kill()

    def script(self, stage, rel, *args, env=None):
        path = TOOLS/rel
        task = TOOLS/'pipeline/task.py'  # UTF-8 stdout (#19)
        head = [sys.executable, '--task', task, path] if getattr(sys, 'frozen', False) else [sys.executable, task, path]
        with _MARKER if rel in WRITES_MARKER else contextlib.nullcontext():
            self.run(stage, [*head, *args], env)

    def progress(self, stage, text):
        bar = self.bar
        if bar:
            emit('progress', **bar.note(stage, text))
        else:
            emit('progress', stage=stage, text=text)


class Progress:
    """One bar for stages running side by side. Each stage to run owns its WEIGHTS share of the bar. A running stage
    fills its share with elapsed time against its expected time (weight x this PC's speed so far): evenly at first,
    then easing towards the end, so a slow stage still moves. Time left plays build()'s scheduling forward (finish_in).
    `timeline` keeps when each stage ran, for the build report."""

    def __init__(self, todo, deps, jobs, finished=(), budget=float('inf')):
        # todo: Stages in build order (finished ones included: a resumed build's bar starts where the last one stopped)
        self.todo, self.deps, self.jobs, self.budget = todo, deps, jobs, budget   # deps: name -> names it waits for
        self.weight = {s.name: WEIGHTS.get(s.name, 30) for s in todo}
        self.longest = {}  # a stage's weight plus its longest chain of followers: the longest starts first
        for s in reversed(todo):
            self.longest[s.name] = self.weight[s.name] + max((self.longest[t.name] for t in todo if s.name in deps.get(t.name, ())), default=0)
        self.started, self.done, self.notes = {}, set(finished), {}
        self.held = {}  # name -> when its work ended, for a stage still running only to wait for its record (hold)
        self.spent = self.expected = 0.0
        self.memory_wait = False  # E1: a stage is held back for memory (build() sets it each scheduling turn)
        self.began, self.timeline = time.monotonic(), []
        self.lock = threading.RLock()  # stages report from worker threads while the main thread starts and ends them

    def start(self, name):
        with self.lock:
            self.started[name] = time.monotonic()

    def hold(self, name):
        """The stage's work is done and it only waits to record (record(): until skate3-base has published the install).
        The bar, time left and the build report count it as done from here."""
        with self.lock:
            self.held[name] = time.monotonic()

    def finish(self, name, ok=True):
        with self.lock:
            began, now = self.started.pop(name, None), self.held.pop(name, None) or time.monotonic()
            self.notes.pop(name, None)
            if began is not None:
                self.timeline.append({'stage': name, 'start': round(began - self.began), 'seconds': round(now - began), 'ok': ok})
            if ok and began is not None:
                self.spent += now - began
                self.expected += self.weight[name]
                self.done.add(name)

    def note(self, name, text):
        with self.lock:
            self.notes[name] = text
            return self.now()

    def now(self):
        with self.lock:
            return self._now()

    def finish_in(self, left):
        """Seconds until the last stage ends: build()'s rules (longest chain first, `jobs` at a time, after its deps,
        never beside a stage with overlapping roots, within the memory budget) played forward on each stage's time left.
        Unlike stage time alone this sees a PC with little free memory run fewer stages at once."""
        # ponytail: no EASY-backfill hold (build() may keep a small stage back for a held one), so slightly optimistic
        stage = {s.name: s for s in self.todo}
        clock, done = 0.0, set(self.done)
        running = {n: left[n] for n in self.started}
        waiting = sorted((n for n in stage if n not in done and n not in running), key=lambda n: -self.longest[n])
        while running or waiting:
            for n in list(waiting):
                if len(running) >= self.jobs:
                    break
                beside = [stage[r] for r in running]
                if self.deps.get(n, set()) - done or any(overlaps(stage[n], o) for o in beside) or not fits(stage[n], beside, self.budget):
                    continue
                waiting.remove(n)
                running[n] = clock + left[n]
            if not running:
                break  # the rest waits on a stage that failed
            n = min(running, key=running.get)
            clock = running.pop(n)
            done.add(n)
        return clock

    def _shares(self, speed, now):
        """Each stage's share done and seconds left, its expected time being its weight x `speed`."""
        share, left = {}, {}
        for s in self.todo:
            expect = self.weight[s.name] * speed
            if s.name in self.done or s.name in self.held:
                share[s.name] = 1.0
            elif s.name in self.started:
                t = (now - self.started[s.name]) / expect
                share[s.name] = min(t, 1 - math.exp(-2 * t))
            else:
                share[s.name] = 0.0
            left[s.name] = expect * (1 - share[s.name])
        return share, left

    def _now(self):
        now = time.monotonic()
        spent, expected = self.spent + PRIOR, self.expected + PRIOR
        speed = spent / expected  # this PC against WEIGHTS' PC, from finished stages
        share, left = self._shares(speed, now)
        # Time left also counts a running stage already past its expected time (it takes at least that long): on a
        # slow PC nothing may finish for minutes. Not the bar, which would then move back.
        for n, began in self.started.items():
            if n not in self.held and now - began > self.weight[n] * speed:
                spent, expected = spent + now - began, expected + self.weight[n]
        left = self._shares(spent / expected, now)[1]
        total = sum(self.weight.values()) or 1
        steps = []
        for step in range(STEPS):
            mine = [s.name for s in self.todo if s.step == step]
            weight = sum(self.weight[n] for n in mine)
            steps.append(round(sum(self.weight[n] * share[n] for n in mine) / weight, 3) if weight else 1.0)
        running = [self.notes.get(n) or LABELS.get(n, n) for n in self.started if n not in self.held]
        if self.memory_wait:
            running.append(MEMORY_WAIT)
        return {'percent': round(100 * sum(self.weight[n] * share[n] for n in share) / total, 1),
                'eta': round(self.finish_in(left)),
                'steps': steps, 'text': ' · '.join(running)}


@contextlib.contextmanager
def build_lock(base):
    """One build per game folder: a second launcher, or a build left running, is refused instead of writing the same
    files. The kernel lock goes away with the process, so a crash never leaves the folder locked."""
    base.mkdir(parents=True, exist_ok=True)
    with (base/'build.lock').open('a+b') as stream:
        if not stream.seek(0, 2):
            stream.write(b'0'); stream.flush()
        stream.seek(0)
        try:
            if os.name == 'nt':
                import msvcrt
                msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            yield False
            return
        try:
            yield True
        finally:
            stream.seek(0)
            with contextlib.suppress(OSError):
                if os.name == 'nt':
                    msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)


def enough_disk(base, first, need=None):
    """`need`: GB this build needs (estimate()); without it the fixed FIRST_BUILD_GB / UPDATE_GB."""
    need = math.ceil(need) if need is not None else FIRST_BUILD_GB if first else UPDATE_GB
    base.mkdir(parents=True, exist_ok=True)
    free = shutil.disk_usage(base)[2] / (1 << 30)  # (total, used, free)
    if free < need:
        emit('error', stage=None, kind='disk', log=None,
             message=f'Not enough disk space: needs about {need} GB free on {base.anchor}, {free:.0f} GB free.')
        return False
    return True


def ordered():
    done, out, names = set(), [], {s.name for s in STAGES}
    while len(out) < len(STAGES):
        ready = [s for s in STAGES if s.name not in done and all(a in done or a not in names for a in s.after)]
        if not ready:
            raise RuntimeError('stage table has a cycle')
        out.append(ready[0]); done.add(ready[0].name)
    return out


def owns(stage, name):
    return any(name == r or name.startswith(r.rstrip('/') + '/') for r in stage.roots)


def overlaps(a, b):
    """Two stages that write into the same files or folders never run side by side. The SHARED files are the
    exception: every edit of them holds _MARKER (stages.catalog_map, add_teleports.py)."""
    return (any(owns(a, r) for r in b.roots if r not in SHARED) or any(owns(b, r) for r in a.roots if r not in SHARED)
            or bool(set(SCRATCH.get(a.name, ())) & set(SCRATCH.get(b.name, ()))))


def dependents(name):
    """Every stage that follows `name`, directly or through other stages."""
    out = set()
    while True:
        more = {s.name for s in STAGES if s.name not in out and (name in s.after or out & set(s.after))}
        if not more:
            return out
        out |= more


def ancestors(name):
    """Every stage `name` follows, directly or through other stages."""
    after = {s.name: set(s.after) for s in STAGES}
    out, more = set(), set(after.get(name, ()))
    while more:
        out |= more
        more = {a for m in more for a in after.get(m, ())} - out
    return out


def damaged_stages(ctx, marker, sizes=None, files=None):
    saved = marker.get('outputs', {}).get('stages')
    if not saved:
        return set()
    root = ctx.install  # once: each ctx.install re-reads installation.json (23 MB); once per receipt took hours (10-08)
    if sizes is None:
        bad = stale(root, saved, files)
    else:
        bad = [name for name, entry in saved.items() if not valid_receipt(root, {name: entry}, sizes)]
    return {s.name for s in STAGES if any(owns(s, b) for b in bad)}


def record(ctx, stage, fingerprint, identities):
    """One atomic write when a stage finishes: its fingerprint, its input identities and a fresh receipt of its roots.
    The stages that follow it are no longer finished, because their input just changed.
    A stage that ends while skate3-base is still installing (`tables`, from the start) waits for it: the install it
    receipts into does not exist yet, and skate3-base writes installation.json without _MARKER (install.publish_marker)."""
    published = lambda: (ctx._cached() or (None, None))[1] is not None  # no unpickling while it waits
    if not published() and ctx.bar and 'skate3-base' in ctx.bar.started:
        ctx.bar.hold(stage.name)
    while not published() and ctx.bar and 'skate3-base' in ctx.bar.started and not ctx.cancelled:
        time.sleep(1)
    root = ctx.install.resolve()  # once: receipt()'s resolve() per file opens each one (seconds per 10,000 files)
    found = {}
    for rel in stage.roots:
        found |= listing(root/rel)
    # hashed outside the lock: stages finishing together don't wait for each other. New files 8 at a time (hashlib lets
    # go of the GIL); the rest are looked up in _sha's memory. A file that can't be read here is tried once more below.
    def warm(p):
        with contextlib.suppress(OSError):
            _sha(p, found[p])
    with cf.ThreadPoolExecutor(HASHERS) as pool:
        list(pool.map(warm, [p for p in found if (str(p), *found[p]) not in _hashes]))
    fresh ={p.relative_to(root).as_posix(): {'size': found[p][0], 'sha256': _sha(p, found[p])} for p in sorted(found)}
    entry = lambda p: {'size': p.stat().st_size, 'sha256': _sha(p)}
    with _MARKER:
        for name in SHARED & set(fresh):  # other stages edit these under this lock: hash the copy that stays
            fresh[name] = entry(root/name)
        marker = ctx.marker
        marker.pop('sealed', None)  # a stage finished: the groups are no longer the sealed state
        saved = marker.setdefault('outputs', {}).setdefault('stages', {})
        old = {n: saved[n] for n in saved if owns(stage, n)}
        # #4 early cutoff: the same files as before leave what follows current. Not skate3-base: later stages edit its
        # files in place (its receipts can't say), nor a stage that left no files (nothing to compare).
        if old == fresh and fresh and stage.name != 'skate3-base':
            ctx.unchanged.add(stage.name)
        else:
            for name in dependents(stage.name):
                marker.get('pipelines', {}).pop(name, None)
                marker.get('inputs', {}).pop(name, None)
        for name in old:
            del saved[name]
        saved.update(fresh)
        marker.setdefault('pipelines', {})[stage.name] = fingerprint
        marker.setdefault('inputs', {})[stage.name] = identities
        ctx.write_marker(marker)
    save_hashes(ctx.base, every=30)  # a build stopped after this stage resumes without re-reading most of its files


CUSTOMISATION = 'assets/private/customisation'


def customiser_current(install):
    """The game's own check (setup.rs customiser_current): the clothing set was made by this build's customiser code;
    current.json (or a customiser-availability.json) carries that code's fingerprint. skate3-base makes the set but only
    reruns when the five groups change: build 5 changed customisation_library.py alone, built "ok", and the game refused
    to start (10-08)."""
    from tools.asset_pipeline.customiser_setup import fingerprint
    want = fingerprint()
    for name in ('current.json', 'customiser-availability.json'):
        with contextlib.suppress(OSError, ValueError, AttributeError):
            if json.loads((install/CUSTOMISATION/name).read_text()).get('fingerprint') == want:
                return True
    return False


def new_customiser(ctx):
    """A clean set from this build's customiser code, made as dlc-clothing starts (build() reruns the wardrobe chain on
    it). Nothing is reused from the old set: the wardrobe stages edited it in place. leftovers() deletes it after."""
    from tools.asset_pipeline.customiser_setup import prepare
    from tools.asset_pipeline.setup_state import source_directory
    for name in ('current.json', 'pending.json'):  # the pointers only; the old set stays until the build is done
        (ctx.install/CUSTOMISATION/name).unlink(missing_ok=True)
    prepare(source_directory(ctx.disc('s3-disc')), ctx.install/'assets', lambda text: ctx.progress('dlc-clothing', text))


def attempt(ctx, stage, fingerprint, identities):
    """One stage on a worker thread, then its record. None when it finished, else the error event's fields."""
    ctx.stage = stage.name
    log = ctx.logs/f'{stage.name}.log'
    old = log.stat().st_size if log.is_file() else 0  # logs append across runs; only this run's text counts
    try:
        if stage.name == 'dlc-clothing' and not customiser_current(ctx.install):
            new_customiser(ctx)
        stage.run(ctx)
        with ctx.timed(f'record {stage.name}'):  # M14: output hashing the next steps wait for, in build-times.json
            record(ctx, stage, fingerprint, identities)
        return None
    except Exception as error:  # noqa: BLE001 - every failure becomes a readable event
        tail = ''
        with contextlib.suppress(OSError):
            tail = log.read_bytes()[old:][-4000:].decode('utf-8', 'replace') if log.is_file() else ''
        disk = any(s in str(error) or s in tail for s in NO_SPACE)
        with contextlib.suppress(OSError):  # a full disk is exactly when this write fails; the event still goes out
            ctx.logs.mkdir(parents=True, exist_ok=True)
            with open(log, 'a', encoding='utf-8') as out:
                out.write(traceback.format_exc())
        plain = next((words for marks, words in PLAIN if any(m in str(error) or m in tail for m in marks)), error)
        return {'stage': stage.name, 'kind': 'disk' if disk else 'failed', 'log': log.relative_to(ctx.base).as_posix(),
                'message': 'Not enough disk space.' if disk else f'{LABELS.get(stage.name, stage.name)} failed: {plain}'}


def cpu_name():
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r'HARDWARE\DESCRIPTION\System\CentralProcessor\0') as key:
            return winreg.QueryValueEx(key, 'ProcessorNameString')[0].strip()
    except (ImportError, OSError):
        return platform.processor()


LONG_PATHS = r'SYSTEM\CurrentControlSet\Control\FileSystem'


def long_paths():
    """Windows' LongPathsEnabled (off by default): installed files reach ~314 characters below installations/<id>/
    (Create a Skater thumbnails named after their items), so every build passes 260 characters whatever its folder."""
    if os.name != 'nt':
        return True
    import winreg
    with contextlib.suppress(OSError):
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, LONG_PATHS) as key:
            return winreg.QueryValueEx(key, 'LongPathsEnabled')[0] == 1
    return False


# STORAGE_BUS_TYPE names, as Get-PhysicalDisk's BusType gave them
BUS = {1: 'SCSI', 2: 'ATAPI', 3: 'ATA', 4: '1394', 6: 'Fibre Channel', 7: 'USB', 8: 'RAID', 9: 'iSCSI', 10: 'SAS',
       11: 'SATA', 12: 'SD', 13: 'MMC', 14: 'Virtual', 15: 'File Backed Virtual', 16: 'Storage Spaces', 17: 'NVMe'}


@functools.lru_cache(maxsize=None)  # the build's drive check and its report ask about the same drives
def drive_kind(drive):
    """'SSD NVMe', 'HDD USB', ... for the disk holding a drive ('C:'); '' if unknown. E3: asked of the volume itself
    (IOCTL_STORAGE_QUERY_PROPERTY: seek penalty, then bus type), no admin and no process: the PowerShell storage
    cmdlets took 1-4 s a drive, and gave '' (taken as an SSD) where PowerShell is blocked."""
    if os.name != 'nt' or not drive:
        return ''
    from ctypes import wintypes
    kernel = ctypes.windll.kernel32
    kernel.CreateFileW.restype = wintypes.HANDLE
    kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD,
                                   wintypes.DWORD, wintypes.HANDLE]
    kernel.DeviceIoControl.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p,
                                       wintypes.DWORD, ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]
    handle = kernel.CreateFileW(rf'\\.\{drive[0]}:', 0, 3, None, 3, 0, None)  # no access, share read|write, OPEN_EXISTING
    if handle in (None, wintypes.HANDLE(-1).value):
        return ''
    try:
        def query(prop, size):
            request = (ctypes.c_uint32 * 3)(prop, 0, 0)  # STORAGE_PROPERTY_QUERY: PropertyId, PropertyStandardQuery
            out, got = ctypes.create_string_buffer(size), wintypes.DWORD()
            ok = kernel.DeviceIoControl(handle, 0x2D1400, request, ctypes.sizeof(request), out, size, ctypes.byref(got), None)
            return out.raw[:got.value] if ok else b''
        seek = query(7, 12)    # StorageDeviceSeekPenaltyProperty: DEVICE_SEEK_PENALTY_DESCRIPTOR (version, size, flag)
        device = query(0, 1024)  # StorageDeviceProperty: STORAGE_DEVICE_DESCRIPTOR, BusType at byte 28
        media = ('HDD' if seek[8] else 'SSD') if len(seek) >= 9 else ''
        bus = BUS.get(int.from_bytes(device[28:32], 'little'), '') if len(device) >= 32 else ''
        return ' '.join(filter(None, (media, bus)))
    finally:
        kernel.CloseHandle(handle)


def pc_info(ctx):
    """What the build report says about this PC: the CPU, memory, and the kind of disk the game and the sources are on."""
    from tools.pipeline.fanout import available_gb
    info = {'cpu': cpu_name(), 'threads': os.cpu_count(), 'memory_gb': round(memory_gb(), 1),
            'free_gb': round(available_gb() or 0, 1), 'drives': {}}
    roles = {Path(ctx.base).resolve().drive: ['game']}
    for drive in sorted({Path(v['path']).drive for v in ctx.found.values() if v.get('path')}):
        roles.setdefault(drive, []).append('sources')
    info['drives'] = {d: f"{d} {' + '.join(r)} on {drive_kind(d) or 'an unknown disk'}" for d, r in roles.items() if d}
    from tools import native  # F4: a missing helper DLL falls back to Python silently, many times slower
    gone, total = native.missing(TOOLS), len(list(TOOLS.rglob('*_native.rs')))
    info['native'] = f"{total - len(gone)} of {total} loaded" + (f" (not: {', '.join(gone)})" if gone else '')
    return info


def clock(seconds):
    m, s = divmod(round(seconds), 60)
    return f'{m // 60}:{m % 60:02}:{s:02}' if m >= 60 else f'{m}:{s:02}'


def write_report(ctx, bar, pc, ok, started, ran, of, jobs, budget):
    """logs/build-times.json: how long the build and each stage took, and on what PC (the launcher's Copy build report
    copies its `report` text). Nothing in it names a folder. A report that can't be written never fails the build."""
    seconds = time.monotonic() - started
    lines = [f"{'Built' if ok else 'Stopped'} in {clock(seconds)} ({ran} of {of} steps ran; "
             f"checking what was built took {clock(bar.began - started)})",
             f"PC: {pc.get('cpu', '?')} ({pc.get('threads', '?')} threads), {pc.get('memory_gb', '?')} GB memory, "
             f"{pc.get('free_gb', '?')} GB free at the start",
             'Drives: ' + (' · '.join(pc.get('drives', {}).values()) or 'not checked'),
             f"Native helpers: {pc.get('native', 'not checked')}",
             f'Up to {jobs} steps at once, {budget:.1f} GB memory budget',
             'Outside the steps: ' + (' · '.join(f'{k} {clock(v)}' for k, v in ctx.phases.items()
                                                 if not k.startswith('record ')) or 'nothing timed'),  # M14's: phases only
             'Steps, slowest first (time taken, started at):']
    for t in sorted(bar.timeline, key=lambda t: -t['seconds']):
        lines.append(f"  {t['stage']:<22} {clock(t['seconds']):>6}  at {clock(t['start'])}{'' if t['ok'] else '  (failed or stopped)'}")
    with contextlib.suppress(OSError):
        ctx.logs.mkdir(parents=True, exist_ok=True)
        atomic_json(ctx.logs/'build-times.json', {'ok': ok, 'seconds': round(seconds), 'pc': pc, 'jobs': jobs,
                                                  'budget_gb': round(budget, 1), 'phases': dict(ctx.phases),
                                                  'stages': [{**t, **({'peak_gb': ctx.peaks[t['stage']]} if t['stage'] in ctx.peaks else {})}
                                                             for t in bar.timeline], 'report': '\n'.join(lines)})


NEXT = 'installation.next.json'  # #9: a failed update's own marker; installation.json keeps the last playable build


def adopt_update(ctx, resume):
    """The last sealed marker, for keep_playable. A failed update waiting in NEXT becomes the marker again, so this build
    resumes it (not on Rebuild, nor when its folder is gone: an older build's leftovers() deletes other installations)."""
    before, pending = ctx.marker, ctx.base/NEXT
    if pending.is_file():
        with contextlib.suppress(OSError, ValueError):
            update = json.loads(pending.read_text(encoding='utf-8'))
            if resume and update.get('directory') and (ctx.base/update['directory']).is_dir():
                with _MARKER:
                    ctx.write_marker(update)
        pending.unlink(missing_ok=True)
    return before if before.get('sealed') else {}


def keep_playable(ctx, before):
    """#9: an update that failed after skate3-base moved it into a new installation folder. installation.json goes back
    to the last sealed build, whose folder nothing touched (leftovers() deletes it only once a build is done), so with
    the launcher's restored game.bak the previous game plays; the update's marker waits in NEXT for the next build.
    ponytail: an update that reran no group edits the installation in place, so it still has no way back."""
    marker = ctx.marker
    if before and marker.get('directory') != before.get('directory'):
        with _MARKER:
            atomic_json(ctx.base/NEXT, marker)
            ctx.write_marker(before)


def build(ctx, resume=True, only=None):
    global LOW_FANOUT
    started = time.monotonic()
    # the parts of "checking what was built" (the report's line), timed one by one for the next build's report
    with ctx.timed('check fingerprints'):
        prints = stage_fingerprints()
    before = adopt_update(ctx, resume)
    own = os.environ.get('SKATE_FANOUT', '')
    if own.isdigit():
        LOW_FANOUT = max(1, int(own))
    elif memory_gb() <= LOW_MEMORY_GB:  # #15 low memory mode: an 8 GB PC runs two fan-out workers, not half its cores
        LOW_FANOUT = 2
        os.environ['SKATE_FANOUT'] = ctx.env['SKATE_FANOUT'] = '2'  # in-process skate3-base reads os.environ
    else:
        LOW_FANOUT = None
    pc = {}  # pc_info, found while the build runs (the disk query takes a few seconds)
    threading.Thread(target=lambda: pc.update(pc_info(ctx)), daemon=True).start()
    order = ordered()
    position = {s.name: i for i, s in enumerate(order)}
    marker = ctx.marker
    emit('progress', text="Checking what's already built", steps=[1.0] + [0.0] * (STEPS - 1))
    if 'directory' in marker:  # a stopped build's half-converted DLC map: map_job's extract() refuses to overwrite it (10-08)
        shutil.rmtree(ctx.install/'conversion', ignore_errors=True)  # here, not in stages.py: no stage fingerprint changes
    with ctx.timed('check hashes.json'):
        if resume:
            load_hashes(ctx.base)
        else:  # Rebuild: trust nothing from before
            (ctx.base/HASHES).unlink(missing_ok=True)
    with ctx.timed('check stage receipts'):
        files = tree(ctx.install.resolve()) if marker.get('directory') and ctx.install.is_dir() else None  # one walk
        damaged = damaged_stages(ctx, marker, files=files) if marker else set()
    with ctx.timed('check clothing set'):
        if marker.get('directory') and not customiser_current(ctx.install):
            damaged.add('dlc-clothing')  # a new set first (attempt), then the wardrobe chain fills it
    if marker.get('sealed'):
        saved = marker.get('outputs', {})
        groups = {k: v for g in group_receipts.ROOTS if isinstance(saved.get(g), dict) for k, v in saved[g].items()}
        # the hashes here, the rest of the group rules (legacy installs without receipts) on sizes
        with ctx.timed('check setup groups'):
            sizes = {name: size for name, (size, _) in files.items()} if files is not None else file_sizes(ctx.install)
            if stale(ctx.install, groups, files) or group_receipts.damaged(ctx.install, marker, sizes=sizes):
                damaged.add('skate3-base')  # a file setup wrote was changed since the last seal: refresh, then all after it
    # What runs is known before anything runs: a stage reruns when it isn't finished or a stage in its `after` reruns.
    todo, identities, current = [], {}, set()
    for stage in order:
        identities[stage.name] = {i: ctx.found.get(i, {}).get('identity') for i in stage.inputs}
        own = (resume and prints.get(stage.name) is not None  # no fingerprint: never "finished" (None == None)
               and marker.get('pipelines', {}).get(stage.name) == prints[stage.name]
               and marker.get('inputs', {}).get(stage.name) == identities[stage.name]
               and stage.name not in damaged)
        finished = own and not set(stage.after) & {s.name for s in todo}
        if own and not finished:
            current.add(stage.name)  # runs only because a stage before it does: maybe not at all (#4, below)
        if (only and stage.name != only) or finished:
            emit('stage', name=stage.name, step=stage.step, index=position[stage.name], total=len(order), status='skip')
        else:
            todo.append(stage)
    names = {s.name for s in todo}
    deps = {s.name: set(s.after) & names for s in todo}
    upstream = {s.name: ancestors(s.name) & names for s in todo}
    jobs = 1 if only else workers()
    budget = start_budget = budget_gb()
    bar = ctx.bar = Progress(order, deps, jobs, finished=[s.name for s in order if s.name not in names], budget=budget)
    waiting = sorted(todo, key=lambda s: -bar.longest[s.name])
    running, failed = {}, False
    _LIVE.append(ctx)
    try:
        with cf.ThreadPoolExecutor(jobs) as pool:
            while waiting or running:
                held = None  # (when, spare) of the first stage that must wait for memory: see reservation()
                if running and not failed:  # E1: memory freed (or taken) by other programs since the last turn
                    bar.budget = budget = budget_gb(ctx.using_gb([s.name for s in running.values()]))
                for stage in list(waiting):
                    if failed or len(running) >= jobs:
                        break
                    if deps[stage.name] - bar.done or any(overlaps(stage, other) for other in running.values()):
                        continue
                    if stage.name in current and upstream[stage.name] <= ctx.unchanged:  # #4: all before it came out the same
                        waiting.remove(stage)
                        ctx.unchanged.add(stage.name)
                        with bar.lock:
                            bar.done.add(stage.name)
                        emit('stage', name=stage.name, step=stage.step, index=position[stage.name], total=len(order), status='skip')
                        continue
                    if not fits(stage, running.values(), budget):
                        held = held or reservation(stage, running.values(), dict(bar.started), budget)
                        continue
                    if held:
                        if time.monotonic() + WEIGHTS.get(stage.name, 30) <= held[0]:
                            pass  # done before the held stage can start
                        elif peak_gb(stage.name) <= held[1]:
                            held = held[0], held[1] - peak_gb(stage.name)  # fits beside it then
                        else:
                            continue
                    waiting.remove(stage)
                    emit('stage', name=stage.name, step=stage.step, index=position[stage.name], total=len(order), status='start')
                    bar.start(stage.name)
                    running[pool.submit(attempt, ctx, stage, prints.get(stage.name), identities[stage.name])] = stage
                with bar.lock:
                    bar.memory_wait = held is not None
                if not running:
                    break  # a stage failed and the rest of its chain can't start
                finished, _ = cf.wait(running, timeout=2, return_when=cf.FIRST_COMPLETED)
                for future in finished:
                    stage, error = running.pop(future), future.result()
                    bar.finish(stage.name, ok=error is None)
                    if error is None:
                        emit('stage', name=stage.name, step=stage.step, index=position[stage.name], total=len(order), status='done')
                    elif not failed:  # the first failure is the one to report; the others were stopped because of it
                        failed = True
                        emit('error', **error)
                        ctx.cancel()
                if not failed:  # after an error the launcher shows that, not a moving bar
                    emit('progress', **bar.now())
    finally:
        ctx.bar = None
        _LIVE.remove(ctx)
        save_hashes(ctx.base)  # what record() skipped (save_hashes every=30), also after a failure
    report = lambda ok: write_report(ctx, bar, dict(pc), ok, started, len(todo), len(order), jobs, start_budget)
    if failed:
        report(False)
        keep_playable(ctx, before)
        store_sizes(ctx)
        return False
    with ctx.timed('seal'):
        seal(ctx)
    with ctx.timed('cleanup'):
        gone = leftovers(ctx)
        save_hashes(ctx.base, keep=receipted(ctx))
        # B8: moved into data/trash (a rename, instant on one volume) for the launcher to delete once this tool has
        # exited, so "done" means done: no build lock or running tool left behind Play while ~14 GB is deleted
        for folder in [*gone, ctx.work/'discs', ctx.work/'dlc']:
            discard(ctx.base, folder)
    report(True)
    emit('done', seconds=round(time.monotonic() - started))
    store_sizes(ctx)  # after "done", as before: a walk of the small folders and the trash, seconds at most
    return True


def receipted(ctx):
    """Every receipted file of the current installation (its stages' and setup's groups), spelled as _sha keys them."""
    marker, root = ctx.marker, ctx.install.resolve()
    return {str(root/name) for group in marker.get('outputs', {}).values() if isinstance(group, dict) for name in group}


TRASH = 'trash'  # data/trash/<ms>-<name>: folders a build no longer needs; the launcher deletes them with no lock held


def discard(base, folder):
    """Move `folder` into data/trash, or delete it here when it can't be moved (another drive, a file held open)."""
    folder = Path(folder)
    if not folder.exists():
        return
    trash = Path(base)/TRASH
    try:
        trash.mkdir(parents=True, exist_ok=True)
        folder.rename(trash/f'{time.time_ns() // 1_000_000}-{folder.name}')
    except OSError:
        shutil.rmtree(folder, ignore_errors=True)  # ponytail: one left half-deleted is finished by the next build


def leftovers(ctx):
    """What a finished build no longer needs: earlier installations (setup copies the last one into a new folder on
    every asset update and kept the old one, ~14 GB each) and clothing sets current.json no longer names (~5 GB each,
    carried into every new installation). Their receipts and remembered hashes are dropped here, so installation.json,
    hashes.json and the launcher's check stop carrying them; build() moves the folders into data/trash (10-08: 37 GB).
    ponytail: a game still running from an old installation during a build could lose files under it."""
    marker = ctx.marker
    if 'directory' not in marker:
        return []
    install = (ctx.base/marker['directory']).resolve()
    # newest save, ns; default-map.json is setup's own file in every new installation, not a save (R, 10-13)
    saved_at = lambda p: max((m for f, (_, m) in listing(p/'settings').items() if f.name != 'default-map.json'), default=0)
    mine = max(saved_at(install), saved_at(ctx.base))  # data/settings: where the game keeps saves once F2 moved them
    # F2 interim: an installation with saves newer than this one's (played during the build, or installation.json was
    # lost and this install started empty) is kept: the game's save migration finds it there.
    gone = [p.resolve() for p in (ctx.base/'installations').iterdir()
            if p.is_dir() and p.resolve() != install and saved_at(p) <= mine]
    customisation = install/'assets/private/customisation'
    named = {}
    for name in ('current.json', 'pending.json'):  # pending: a generation a later build may finish (customiser_setup)
        with contextlib.suppress(OSError, ValueError, AttributeError):
            named[name] = json.loads((customisation/name).read_text()).get('set')
    if isinstance(named.get('current.json'), str) and (customisation/'sets').is_dir():
        sets = [p for p in (customisation/'sets').iterdir() if p.is_dir() and p.name not in named.values()]
        old = tuple(f'assets/private/customisation/sets/{p.name}/' for p in sets)
        saved = marker.get('outputs', {}).get('stages', {})
        for name in [n for n in saved if n.startswith(old)] if old else []:
            del saved[name]
        ctx.write_marker(marker)
        gone += sets
    under = tuple(str(p) + os.sep for p in gone)
    for key in [k for k in _hashes if k[0].startswith(under)] if under else []:
        del _hashes[key]
    return gone


def seal(ctx):
    """Stages edit files inside setup's groups; re-record the groups setup wrote (stage roots are receipted per stage)."""
    marker = ctx.marker
    for group in group_receipts.ROOTS:
        if group in marker.get('outputs', {}):
            marker['outputs'][group] = group_receipts.record(ctx.install, group)
    marker['sealed'] = True
    ctx.write_marker(marker)


def complete(ctx, prints=None):
    """The launcher's "Play or Build?" on every open: sizes from one directory walk (about a second, not minutes of
    hashing tens of GB). A build still hashes every receipt before it trusts a stage."""
    marker, prints = ctx.marker, prints or stage_fingerprints()
    pipelines = marker.get('pipelines', {})
    if not marker or any(prints.get(s.name) is None or pipelines.get(s.name) != prints[s.name] for s in STAGES):
        return False
    if STAGES and marker.get('outputs', {}).get('stages') is None:  # built stages always leave receipts
        return False
    if not customiser_current(ctx.install):  # the game would refuse to start: Build, not Play
        return False
    sizes = file_sizes(ctx.install)
    if marker.get('sealed') and group_receipts.damaged(ctx.install, marker, sizes=sizes):  # mid-build markers are not checked
        return False
    return not damaged_stages(ctx, marker, sizes)


def planned(marker, prints, found, damaged=()):
    """The stages a resumed build would run, from fingerprints and input identities only (no hashing): build()'s rule
    without its receipt check. An input that isn't found now is not counted as changed."""
    todo = []
    for stage in ordered():
        same = lambda i: found.get(i, {}).get('status') != 'ok' or marker.get('inputs', {}).get(stage.name, {}).get(i) == found[i].get('identity')
        if not (prints.get(stage.name) is not None and marker.get('pipelines', {}).get(stage.name) == prints[stage.name]
                and all(map(same, stage.inputs)) and stage.name not in damaged and not set(stage.after) & set(todo)):
            todo.append(stage.name)
    return todo


MARGIN = 2 << 30   # bytes spare beside an update's own need: logs, temp files, the receipts
VERIFY = 29 << 30  # bytes the first build's source check reads (every disc, every mod file)


def last_speed(base):
    """This PC against WEIGHTS' PC (seconds taken / WEIGHTS seconds) from the last build's report; None without one."""
    with contextlib.suppress(OSError, ValueError, TypeError, KeyError, AttributeError):
        report = Path(base)/'logs/build-times.json'
        if not report.is_file():  # a build that stopped before its stages: the one before it (P5)
            report = Path(base)/'logs.previous/build-times.json'
        ran = [t for t in json.loads(report.read_text(encoding='utf-8'))['stages'] if t.get('ok') and t.get('stage') in WEIGHTS]
        weight = sum(WEIGHTS[t['stage']] for t in ran)
        if weight >= PRIOR:
            return sum(t['seconds'] for t in ran) / weight
    return None


def rewrites(receipts, todo, base):
    """Bytes an update of `todo` writes again: its stages' receipted roots (owns(), by path prefix: one set lookup per
    folder level, not per stage), plus the whole install when skate3-base reruns (install.py copies it)."""
    roots = {r.rstrip('/') for s in todo for r in s.roots}
    def mine(name):
        parts = name.split('/')
        return any('/'.join(parts[:i]) in roots for i in range(1, len(parts) + 1))
    size = lambda entries: sum(e.get('size', 0) for e in entries.values() if isinstance(e, dict))
    out = sum(e.get('size', 0) for n, e in receipts.get('stages', {}).items() if isinstance(e, dict) and mine(n))
    return out + (sum(size(v) for v in receipts.values() if isinstance(v, dict)) if base else 0)


def package_copies(ctx, names):
    """Bytes the nine Skate 3 packages cost when they sit on another drive than the game (#11, player build review):
    each stage folder that holds them (online-challenge-map's, the Skate 3 online stages' shared one, the career's) is
    hard-linked on the same drive, a full copy from another, and those stages can run at once.
    shortcut: the same drive on exFAT/FAT32 copies too and is not counted; check os.link there if players hit it."""
    from tools.pipeline.stages import S3_DLC
    folders = sum(any(n in names for n in group) for group in (
        ('online-challenge-map',), ('online-skate3-export', 'online-skate3'), ('skate3-career',)))
    drive = ctx.base.resolve().drive.lower()
    size = 0
    for item in S3_DLC:
        with contextlib.suppress(KeyError, TypeError, OSError):
            path = Path(ctx.found[item]['path'])
            if path.resolve().drive.lower() != drive:
                size += path.stat().st_size
    return size * folders


def estimate(ctx, names, marker, rebuild=None):
    """#6: the time and disk a build of `names` (planned()) needs, before it starts. Seconds: build()'s scheduling
    played forward on WEIGHTS, scaled by the last build's speed when there is one. Bytes: a first build's FIRST_BUILD_GB;
    an update rewrites its stages' roots (their receipts' sizes), copies the whole install when skate3-base reruns
    (install.py) and extracts the discs and packages its stages read. Temp files go to data/tmp (#2), so only the game
    drive counts.
    `rebuild`: storage.json's bytes for a build of every stage (the launch check in play), so no receipt is read."""
    names = set(names)
    todo = [s for s in ordered() if s.name in names]
    speed = last_speed(ctx.base)
    bar = Progress(todo, {s.name: set(s.after) & names for s in todo}, workers(), budget=budget_gb())
    seconds = bar.finish_in({s.name: WEIGHTS.get(s.name, 30) * (speed or 1) for s in todo})
    first = not marker.get('pipelines')
    if first:
        seconds += VERIFY / (400 << 20)  # an SSD's read; an HDD is the range's top
    low, high = (0.85, 1.3) if speed else (1.0, 2.5)
    if first:
        need = (FIRST_BUILD_GB << 30) + package_copies(ctx, names)
    else:
        everything = names >= {s.name for s in STAGES}
        need = rebuild if everything and isinstance(rebuild, int) else rewrites(marker.get('outputs', {}), todo, 'skate3-base' in names)
        for item in {i for s in todo for i in s.inputs}:
            folder = ctx.work/('discs' if item.endswith('-disc') else 'dlc')/item
            with contextlib.suppress(KeyError, TypeError, OSError):
                if not (folder/'.complete').is_file():
                    need += os.stat(ctx.found[item]['path']).st_size
        need += package_copies(ctx, names) + (MARGIN if todo else 0)
    ctx.base.mkdir(parents=True, exist_ok=True)
    return {'min': round(seconds * low), 'max': round(seconds * high), 'need': need,
            'free': shutil.disk_usage(ctx.base)[2], 'drive': ctx.base.resolve().drive}


STORAGE = 'storage.json'  # the Storage panel's sizes, measured at the end of each build (store_sizes), beside installation.json


def store_sizes(ctx):
    """P8: the size of each pile the Storage panel can clear, from a walk of the small folders only (the install's size
    comes from its receipts). Written at the end of every build and on `--storage`; the launcher's open never walks."""
    def walk(path):
        with contextlib.suppress(OSError):
            return sum(size for size, _ in listing(Path(path)).values())
        return 0
    marker = ctx.marker
    named = (ctx.base/marker['directory']).resolve() if 'directory' in marker else None
    installs = ctx.base/'installations'
    old = [p for p in installs.iterdir() if p.is_dir() and p.resolve() != named] if installs.is_dir() else []
    local = Path(os.environ.get('LOCALAPPDATA', Path.home()))/'Skate3RustEngine'
    receipts = marker.get('outputs', {})
    sizes = {'leftovers': sum(map(walk, [ctx.work/'discs', ctx.work/'dlc', ctx.base/TRASH, *old])),
             'temp': walk(ctx.base/'tmp'), 'audio': walk(local/'audio-pcm-cache') + walk(local/'bc7-cache'),
             'cache': walk(ctx.work/'cache') + walk(ctx.work/'convert'), 'tools': walk(ctx.tools),  # D4
             'logs': walk(ctx.logs) + walk(ctx.base/'logs.previous'), 'crashes': walk(local/'CrashReports'),
             'game': sum(e.get('size', 0) for v in receipts.values() if isinstance(v, dict) for e in v.values() if isinstance(e, dict)),
             'rebuild': rewrites(receipts, STAGES, any(s.name == 'skate3-base' for s in STAGES))}  # estimate() in play
    with contextlib.suppress(OSError):
        atomic_json(ctx.base/STORAGE, sizes)
    return sizes


def free_space(base):
    base.mkdir(parents=True, exist_ok=True)
    return {'free': shutil.disk_usage(base)[2], 'drive': base.resolve().drive}


def probe(ctx, release, folders):
    """U2 and U3 before an update is installed: what its build reruns here (`release`: its release.json), how long that
    takes, and whether it can run: the source files connected and the disk big enough."""
    marker = ctx.marker
    damaged = set()
    want = release.get('character_customiser')
    if want and marker.get('directory'):
        with contextlib.suppress(OSError, ValueError, AttributeError):
            if json.loads((ctx.install/CUSTOMISATION/'current.json').read_text()).get('fingerprint') != want:
                damaged.add('dlc-clothing')
    todo = planned(marker, with_base(release.get('asset_pipelines', {})), ctx.found, damaged)
    guess = estimate(ctx, todo, marker)
    labels = {item[0]: item[2] for item in sources.ITEMS}
    missing = [labels.get(item, item) for item, slot in ctx.found.items() if slot.get('status') != 'ok']
    steps = f"{len(todo)} step{'s' if len(todo) != 1 else ''}"
    blocked = None
    if todo and missing:
        where = str(folders[0]) if folders else 'your source files'
        blocked = f'Connect {where} first: this update rebuilds {steps} from your source files.'
    elif todo and guess['need'] > guess['free']:
        blocked = (f"Free up space on {guess['drive']} first: this update needs about {math.ceil(guess['need'] / (1 << 30))} GB, "
                   f"{guess['free'] / (1 << 30):.0f} GB free.")
    emit('probe', rebuild={'steps': len(todo), 'seconds': guess['max'] if todo else 0, 'full': 'skate3-base' in todo},
         stages=todo, estimate=guess, missing=missing, blocked=blocked)


def verify_install(ctx):
    """#5 Check and repair: every receipted file (each stage's and setup's groups) hashed again, nothing taken from
    hashes.json, no source files needed. The true hashes are saved, so the next resumed build reruns exactly the
    damaged stages instead of trusting a file corrupted in place."""
    marker = ctx.marker
    receipts = {k: v for group in marker.get('outputs', {}).values() if isinstance(group, dict) for k, v in group.items()}
    if not marker.get('directory') or not receipts:
        emit('verify', ok=False, files=0, bad=0, stages=[], message='Nothing is built yet.')
        return
    emit('progress', text=f'Checking {len(receipts):,} game files', percent=0, steps=[0.0] * STEPS)
    _hashes.clear()
    bad = stale(ctx.install, receipts)
    save_hashes(ctx.base)
    stages = [s.name for s in ordered() if any(owns(s, b) for b in bad)]
    groups = {k for g in group_receipts.ROOTS for k in marker.get('outputs', {}).get(g, {}) if isinstance(marker['outputs'][g], dict)}
    if bad & groups and 'skate3-base' not in stages:
        stages.insert(0, 'skate3-base')
    if bad:
        message = (f"{len(bad):,} of {len(receipts):,} files are damaged or missing. To rebuild: "
                   + ', '.join(LABELS.get(s, s) for s in stages) + '.')
    else:
        message = f'All {len(receipts):,} files OK.'
    emit('verify', ok=not bad, files=len(receipts), bad=len(bad), stages=stages, message=message)


def main(argv):
    global _OUT
    with contextlib.suppress(AttributeError, ValueError):  # B6: a stray print (a library, ANSI text) stays valid UTF-8
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
    _OUT = sys.stdout
    p = argparse.ArgumentParser(prog='skate3setup --pipeline')
    p.add_argument('--sources', type=Path, action='append', default=[])
    p.add_argument('--base', type=Path, required=True)
    p.add_argument('--game-exe', type=Path, required=True)
    p.add_argument('--resume', action='store_true')
    p.add_argument('--stage')
    p.add_argument('--check', action='store_true')
    p.add_argument('--identify', action='store_true')
    p.add_argument('--probe', type=Path, help="a release's release.json: what installing it would rebuild here (U2)")
    p.add_argument('--storage', action='store_true', help='measure the Storage panel sizes now')
    p.add_argument('--verify', action='store_true', help='hash every built file again (Check and repair, #5)')
    a = p.parse_args(argv)
    if a.stage and a.stage not in {s.name for s in STAGES}:
        emit('error', stage=None, kind='failed', log=None, message=f'unknown stage {a.stage}')
        return 2
    try:
        return execute(a)
    except Exception as error:  # noqa: BLE001 - the launcher reads events, never a traceback
        emit('error', stage=None, kind='failed', log=None, message=f'{type(error).__name__}: {error}')
        return 1


def execute(a):
    if a.storage or a.verify:  # A3: neither reads the source files, so no scan (and no 'items': the page keeps its own)
        found = {}
    else:
        found = sources.scan(a.sources, a.base/'sources-identified.json')
        emit('items', items={k: {f: v[f] for f in ('status', 'path', 'detail')} for k, v in found.items()})
    ctx = Context(a.base, a.game_exe, found)
    if a.identify:
        return 0
    if a.probe:
        probe(ctx, json.loads(a.probe.read_text(encoding='utf-8-sig')), a.sources)
        return 0
    if a.storage:
        emit('storage', **store_sizes(ctx), **free_space(ctx.base))
        return 0
    if a.verify:  # before the source check: an install can be checked with the sources unplugged
        with build_lock(a.base) as mine:
            if not mine:
                emit('error', stage=None, kind='failed', log=None, message='A build is running for this game.')
                return 2
            verify_install(ctx)
        return 0
    if a.check:
        ok = all(v['status'] == 'ok' for v in found.values())
        prints = stage_fingerprints(ctx.base/'fingerprints.json')
        state = 'play' if complete(ctx, prints) else 'ready' if ok else 'missing'
        marker = ctx.marker
        # The main button's build; in `play` the only build left is Rebuild, which reruns everything.
        names = [s.name for s in STAGES] if state == 'play' else planned(marker, prints, found)
        sizes = {}
        with contextlib.suppress(OSError, ValueError):
            sizes = json.loads((ctx.base/STORAGE).read_text(encoding='utf-8'))
        emit('state', state=state, estimate=estimate(ctx, names, marker, sizes.get('rebuild') if state == 'play' else None),
             sizes={**sizes, **free_space(ctx.base)}, long_paths=long_paths())
        return 0
    if not long_paths():  # before minutes of source checking: the build would fail at its first long file
        emit('error', stage=None, kind='failed', log=None, message=(
            "Windows on this PC limits file paths to 260 characters, and the game's files need longer ones. Turn on long "
            f'paths (as an administrator: reg add "HKLM\\{LONG_PATHS}" /v LongPathsEnabled /t REG_DWORD /d 1 /f), then build again.'))
        return 2
    if not all(v['status'] == 'ok' for v in found.values()):
        emit('error', stage=None, kind='sources', log=None, message='Some source files are missing or not supported yet.')
        return 2
    with build_lock(a.base) as mine:
        if not mine:
            emit('error', stage=None, kind='failed', log=None,
                 message='Another build is already running for this game. Close the other Skate 3 Remaster window, '
                         'or restart Windows if none is open.')
            return 2
        return locked_build(a, ctx)


def fresh_start(ctx):
    """Under the build lock, before anything else. #2: every script's temp files go to data/tmp on the game's drive (the
    launcher sets the same for its own spawns), emptied here and when the build ends, so a cancel or crash leaves nothing
    in %TEMP% on C:. P5: last build's logs move to logs.previous (one earlier build kept for bug reports), file by file:
    the launcher may hold its own logs there open."""
    tmp = ctx.base/'tmp'
    shutil.rmtree(tmp, ignore_errors=True)
    shutil.rmtree(ctx.base/TRASH, ignore_errors=True)  # B8: what the launcher didn't get to delete (closed first)
    tmp.mkdir(parents=True, exist_ok=True)
    ctx.env.update(TMP=str(tmp), TEMP=str(tmp))
    tempfile.tempdir = str(tmp)  # this process's own temp files (in-process stages)
    previous = ctx.base/'logs.previous'
    if ctx.logs.is_dir():
        shutil.rmtree(previous, ignore_errors=True)
        previous.mkdir(exist_ok=True)
        for path in ctx.logs.iterdir():
            with contextlib.suppress(OSError):
                path.replace(previous/path.name)


def locked_build(a, ctx):
    before = tempfile.tempdir
    fresh_start(ctx)
    try:
        return build_from_sources(a, ctx)
    finally:
        tempfile.tempdir = before
        for path in (ctx.base/'tmp').iterdir():
            with contextlib.suppress(OSError):  # a file still open is the launcher's next sweep's
                shutil.rmtree(path, ignore_errors=True) if path.is_dir() else path.unlink()


def build_from_sources(a, ctx):
    marker = ctx.marker
    first = not marker.get('pipelines')  # nothing finished yet: a first build
    names = [a.stage] if a.stage else [s.name for s in STAGES] if not a.resume else planned(marker, stage_fingerprints(), ctx.found)
    need = FIRST_BUILD_GB + package_copies(ctx, names) / (1 << 30) if first else estimate(ctx, names, marker)['need'] / (1 << 30)
    if not enough_disk(a.base, first, need):
        return 2
    # A build resumes on every update: source files that passed before, unchanged on disk, are not re-hashed.
    # A fresh build (Settings > Rebuild) forgets that and checks everything again.
    checked = ctx.base/'sources-verified.json'
    if not a.resume:
        checked.unlink(missing_ok=True)
    # #10: one read at a time on a hard drive (the game's: receipts; the sources': verify), several on an SSD
    global HASHERS
    game, drives = ctx.base.resolve().drive, {Path(v['path']).drive for v in ctx.found.values() if v.get('path')} - {''}
    asked = sorted(drives | {game} - {''})
    with cf.ThreadPoolExecutor(4) as pool:
        kinds = dict(zip(asked, pool.map(drive_kind, asked)))
    HASHERS = 1 if 'HDD' in kinds.get(game, '') else 8
    # #11: the tool downloads (first build only; kept in data/tools) run beside the source check, not after it
    from tools.pipeline.fetch_tools import ensure
    fetched = {}

    def fetch():
        with ctx.timed('tool downloads'):
            try:
                fetched['env'] = ensure(ctx.tools, lambda t: fetched.get('stop') or ctx.progress('tools', t))
            except Exception as error:  # noqa: BLE001 - raised on the main thread once the check has passed
                fetched['error'] = error
    fetcher = threading.Thread(target=fetch, daemon=True)
    fetcher.start()
    with ctx.timed('source check'):
        problems = sources.verify(ctx.found, checked, lambda done, total: emit(
            'progress', text=f'Checking your files ({done} of {total})', percent=0, steps=[done / total] + [0] * (STEPS - 1)),
            threads=1 if any('HDD' in kinds.get(d, '') for d in drives) else 4)
    if problems:
        fetched['stop'] = True  # the error stays the last event the launcher reads
        emit('error', stage=None, kind='sources', log=None, message='\n'.join(problems))
        return 2
    fetcher.join()
    if 'error' in fetched:
        raise fetched['error']
    ctx.env.update(fetched['env'])
    os.environ.update(fetched['env'])  # stages that run in-process (derive_tables' XEX images) read the key source too
    return 0 if build(ctx, resume=a.resume, only=a.stage) else 1


def release_stages(tools):
    """U3, for release.json (Build-Release): each stage's fingerprint (skate3-base's included), the stages it follows and
    its WEIGHTS seconds, in build order. With installation.json's `pipelines` the launcher can tell what an update
    rebuilds and roughly how long it takes (`--probe` says it exactly, sources and disk included)."""
    from tools.asset_pipeline.versions import fingerprints
    prints = with_base(fingerprints(tools))
    return {s.name: {'fingerprint': prints.get(s.name), 'after': list(s.after), 'seconds': WEIGHTS.get(s.name, 30)}
            for s in ordered()}


if __name__ == '__main__':  # Build-Release: python -m tools.pipeline.runner --release-stages TOOLS
    assert sys.argv[1:2] == ['--release-stages'], sys.argv
    print(json.dumps(release_stages(Path(sys.argv[2]))))
