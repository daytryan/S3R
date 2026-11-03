"""unluac on many Lua chunks in one JVM (UnluacBatch.class, compiled from UnluacBatch.java) instead of one JVM start-up per
chunk (~0.2 s each, more beside a virus scanner; 2,000+ of them per build). Each chunk's result is exactly what
`java -jar unluac.jar chunk` gives: its stdout bytes and exit code. Chunks the batch leaves without a result (a JVM that
died or could not load the class) fall back to one `java -jar` each, several at once.

    results = run(chunks)              # {chunk: (stdout bytes, exit code)}, one entry per distinct chunk
    text(results[chunk][0])            # that stdout as subprocess.run(..., text=True) handed it over
"""
import locale, os, subprocess, sys, tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent  # UnluacBatch.class


def _near(path, work):
    """path relative to work, where the JVMs start: Java turns its arguments into the PC's ANSI code page, so a folder
    named outside it (a user folder named in Chinese on an English PC) arrives as '????' (#19). Relative to TEMP, the game folder's
    own name drops out. Another drive keeps the full path."""
    try:
        return os.path.relpath(path, work)
    except ValueError:
        return str(path)


def _one(java, unluac, work, name):
    run = subprocess.run([java, '-jar', _near(unluac, work), name], capture_output=True, cwd=work)
    return run.stdout, run.returncode


def run(chunks, java=None, unluac=None):
    java = java or os.environ.get('SKATE_JAVA', 'java')
    unluac = str(unluac or os.environ['SKATE_UNLUAC'])
    chunks = list(dict.fromkeys(chunks))
    # a JVM decompiles one chunk at a time: a big batch goes to a few JVMs side by side (~250 chunks keep one busy)
    parts = max(1, min(4, (os.cpu_count() or 2) // 2, len(chunks) // 250))
    results = {}
    with tempfile.TemporaryDirectory(prefix='s3r-unluac-') as work:
        work = Path(work)
        for i, chunk in enumerate(chunks):
            (work/f'{i}.luac').write_bytes(chunk)
        lists = [work/f'list{p}.txt' for p in range(parts if chunks else 0)]
        for p, listed in enumerate(lists):
            listed.write_text(''.join(f'{work/f"{i}.luac"}\t{work/f"{i}.out"}\n' for i in range(p, len(chunks), parts)),
                              encoding='utf-8')
        with ThreadPoolExecutor(parts) as pool:
            classpath = _near(unluac, work) + os.pathsep + _near(HERE, work)
            list(pool.map(lambda listed: subprocess.run([java, '-cp', classpath, 'UnluacBatch', listed.name],
                                                        capture_output=True, cwd=work), lists))
        missing = []
        for i, chunk in enumerate(chunks):
            code = work/f'{i}.out.rc'
            if code.is_file():
                results[chunk] = (work/f'{i}.out').read_bytes(), int(code.read_text())
            else:
                missing.append(i)
        with ThreadPoolExecutor(min(8, os.cpu_count() or 1)) as pool:
            for i, result in zip(missing, pool.map(lambda i: _one(java, unluac, work, f'{i}.luac'), missing)):
                results[chunks[i]] = result
    return results


def text(stdout, encoding=None):
    """stdout decoded as subprocess.run(..., text=True) (or encoding=...) decodes it, newlines as \\n."""
    encoding = encoding or ('utf-8' if sys.flags.utf8_mode else locale.getencoding())
    return stdout.decode(encoding).replace('\r\n', '\n').replace('\r', '\n')
