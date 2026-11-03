"""Runs one build script, `task.py SCRIPT ARGS...`, with UTF-8 stdout and stderr (#19). A child's stdout goes to its
stage log, and Python writes a file in the PC's ANSI code page, so printing a path outside it (a user folder named in Chinese on an
English PC) raised UnicodeEncodeError and failed the step. The frozen setup ignores PYTHONIOENCODING, hence this."""
import runpy, sys
from pathlib import Path

for stream in (sys.stdout, sys.stderr):
    stream.reconfigure(encoding='utf-8', errors='backslashreplace')  # the stage log is read as UTF-8
here = str(Path(__file__).resolve().parent)
sys.path[:] = [p for p in sys.path if p and str(Path(p).resolve()) != here]  # pipeline/*.py must not shadow the script's imports
script = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(script.parent))
sys.argv = [str(script), *sys.argv[2:]]
runpy.run_path(str(script), run_name='__main__')
