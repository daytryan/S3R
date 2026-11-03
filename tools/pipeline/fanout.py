"""Spread one script's independent jobs over several processes, the way install.py converts maps: each worker is the
same script run again (`skate3setup.exe --task SCRIPT` when frozen) with `--lane PLAN --out DIR` and the caller's
arguments. A worker reads its jobs from PLAN as [[index, job], ...] and writes job `index`'s results under DIR/index.
Processes, not threads: the drawing code is numpy and Python, which hold the GIL."""
import contextlib, ctypes, json, os, subprocess, sys, tempfile
from pathlib import Path

NO_WINDOW = 0x08000000 if os.name == 'nt' else 0


def command(script, *args):
    head = [sys.executable, '--task'] if getattr(sys, 'frozen', False) else [sys.executable]
    return [*head, str(script), *map(str, args)]


def available_gb():
    if os.name != 'nt':
        return None

    class Status(ctypes.Structure):  # MEMORYSTATUSEX
        _fields_ = [('length', ctypes.c_ulong), ('load', ctypes.c_ulong), ('total', ctypes.c_ulonglong),
                    ('available', ctypes.c_ulonglong), ('rest', ctypes.c_ulonglong * 5)]
    status = Status(length=ctypes.sizeof(Status))
    return status.available / (1 << 30) if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)) else None


def workers(gb_each=0.75):
    """Half the logical cores (the build runs other stages beside this one, and the desktop stays usable), fewer when
    free memory is short. SKATE_FANOUT overrides (1 = everything in this process)."""
    if os.environ.get('SKATE_FANOUT', '').isdigit():
        return max(1, int(os.environ['SKATE_FANOUT']))
    count = max(1, (os.cpu_count() or 2) // 2)
    free = available_gb()
    return count if free is None else max(1, min(count, int((free - 1) / gb_each)))


def split(jobs, lanes, size=lambda job: 1):
    """[[index, job], ...] per lane: the biggest jobs first, each to the least loaded lane."""
    loads = [[0, []] for _ in range(lanes)]
    for index in sorted(range(len(jobs)), key=lambda i: -size(jobs[i])):
        lane = min(loads, key=lambda load: load[0])
        lane[0] += size(jobs[index])
        lane[1].append([index, jobs[index]])
    return [lane for _, lane in loads if lane]


@contextlib.contextmanager
def fan_out(script, jobs, args=(), size=lambda job: 1, count=None):
    """Run the workers over `jobs` and yield the folder holding DIR/<index> for every job (removed afterwards)."""
    with tempfile.TemporaryDirectory(prefix='s3r-fanout-') as temp:
        lanes = split(jobs, count or workers(), size)
        children = []
        try:
            for number, lane in enumerate(lanes):
                plan = Path(temp, f'lane{number}.json')
                plan.write_text(json.dumps(lane), encoding='utf-8')
                children.append(subprocess.Popen(command(script, '--lane', plan, '--out', temp, *args),
                                                 creationflags=NO_WINDOW))
            codes = [child.wait() for child in children]
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
        if any(codes):
            raise RuntimeError(f'{Path(script).name}: a worker failed (exit codes {codes})')
        yield Path(temp)


def lane(argv):
    """A worker's side: (its [[index, job], ...], the output folder, the caller's arguments)."""
    assert argv[0] == '--lane' and argv[2] == '--out', argv
    return json.loads(Path(argv[1]).read_text(encoding='utf-8')), Path(argv[3]), argv[4:]
