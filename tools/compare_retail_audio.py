"""Compare a Rust playtest trace (SKATE_AUDIO_TRACE) with retail audio-trace sessions.

    python tools/compare_retail_audio.py <audiofiles.big> <rust-trace.log> <retail.audio-trace.log.gz>...

Retail sessions are the skate3-audio repo's probe/traces/sessions/*.audio-trace.log.gz. For each
side it collects the player sound objects posted (per minute) and every voice open, keyed by the
sample's EAAC header words (which name the sample; banks are resolved from audiofiles.big), with
the open's parameter records. It prints objects retail posts that the engine never does, samples
only one side opens, and for shared samples the median of every record value side by side.
"""
import gzip
import re
import statistics
import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path

RATES = {11025, 16000, 22050, 24000, 32000, 44100, 48000}
RETAIL_MSG = re.compile(r"skate3-audio-msg: \d+ obj=\d+ (\S+) ")
RETAIL_OPEN = re.compile(r"skate3-audio-open: \d+ sample=\S+ eaac=\[(\w+) (\w+)\].*?records=\[([^\]]*)\]")
RUST_POST = re.compile(r"^\d+ \d+ PO \S+ \S+ (\S+) ")
RUST_OPEN = re.compile(r"^\d+ \d+ OP \S+ eaac=\[(\w+) (\w+)\].*?records=\[([^\]]*)\]")
STAMP = re.compile(r"^\[[\d-]+ (\d+):(\d+):([\d.]+)\]")


def big_entries(data):
    count, = struct.unpack(">I", data[4:8])
    names, = struct.unpack(">I", data[12:16])
    width, shift = data[20], data[10]
    for i in range(count):
        offset, _, size, _ = struct.unpack(">IIII", data[0x30 + i * 16:0x40 + i * 16])
        name = data[names + i * width + 2:names + (i + 1) * width].split(b"\0")[0].decode()
        yield name, offset << shift, size


def sample_banks(audiofiles):
    """(eaac word 0, word 1) -> the banks holding a sample with that header."""
    data = Path(audiofiles).read_bytes()
    banks = defaultdict(set)
    for name, offset, size in big_entries(data):
        if not name.endswith((".abk", ".bnk")):
            continue
        member = data[offset:offset + size]
        for o in range(0, len(member) - 16, 2):
            if member[o] & 0x0F != 3 or member[o] >> 4 > 1:
                continue
            a, b, _, c = struct.unpack_from(">IIII", member, o)
            if (a & 0x3FFFF) in RATES and b and (b & 0x1FFFFFFF) == c:
                banks[(a, b)].add(name)
    return banks


def records(text):
    out = {}
    for item in text.split():
        rid, _, value = item.partition(":")
        out[int(rid)] = int(value.split("/")[1])
    return out


def read(path, retail):
    opener = gzip.open if str(path).endswith(".gz") else open
    posts, opens = Counter(), defaultdict(list)
    first = last = None
    with opener(path, "rt", errors="replace") as f:
        for line in f:
            if retail:
                m = STAMP.match(line)
                if m:
                    t = int(m.group(1)) * 3600 + int(m.group(2)) * 60 + float(m.group(3))
                    first = t if first is None else first
                    last = t
                post, op = RETAIL_MSG.search(line), RETAIL_OPEN.search(line)
            else:
                ms = line.split(" ", 1)[0]
                if ms.isdigit():
                    first = first if first is not None else int(ms) / 1000
                    last = int(ms) / 1000
                post, op = RUST_POST.match(line), RUST_OPEN.match(line)
            if post:
                posts[post.group(1)] += 1
            if op:
                opens[(int(op.group(1), 16), int(op.group(2), 16))].append(records(op.group(3)))
    minutes = max((last or 0) - (first or 0), 1) / 60
    return posts, opens, minutes


def median_records(entries):
    by_id = defaultdict(list)
    for entry in entries:
        for rid, value in entry.items():
            by_id[rid].append(value)
    return {rid: statistics.median(v) for rid, v in sorted(by_id.items())}


def main(audiofiles, rust_trace, *retail_traces):
    banks = sample_banks(audiofiles)
    label = lambda key: "/".join(sorted(banks.get(key, {"?"})))
    r_posts, r_opens, r_min = Counter(), defaultdict(list), 0
    for path in retail_traces:
        p, o, m = read(path, retail=True)
        r_posts.update(p)
        for k, v in o.items():
            r_opens[k] += v
        r_min += m
    e_posts, e_opens, e_min = read(rust_trace, retail=False)
    print(f"retail {r_min:.1f} min over {len(retail_traces)} sessions, engine {e_min:.1f} min\n")

    print("== Sound objects: posts per minute (retail vs engine)")
    for name in sorted(set(r_posts) | set(e_posts), key=lambda n: -r_posts[n]):
        r, e = r_posts[name] / r_min, e_posts[name] / e_min
        flag = "  MISSING" if r and not e else "  EXTRA" if e and not r else ""
        print(f"  {name:32s} {r:8.1f} {e:8.1f}{flag}")

    by_bank = lambda keys: Counter(label(k) for k in keys)
    only_r = [k for k in r_opens if k not in e_opens]
    only_e = [k for k in e_opens if k not in r_opens]
    print(f"\n== Samples retail opens that the engine never did ({len(only_r)}), by bank")
    for bank, n in by_bank(only_r).most_common():
        print(f"  {n:4d}  {bank}")
    print(f"\n== Samples the engine opens that retail never did ({len(only_e)}), by bank")
    for bank, n in by_bank(only_e).most_common():
        print(f"  {n:4d}  {bank}")

    print("\n== Shared samples: median record values, retail -> engine (differences only)")
    for key in sorted(set(r_opens) & set(e_opens), key=lambda k: -len(e_opens[k])):
        r, e = median_records(r_opens[key]), median_records(e_opens[key])
        diffs = [f"{rid}:{r.get(rid)}->{e.get(rid)}" for rid in sorted(set(r) | set(e)) if r.get(rid) != e.get(rid)]
        if diffs:
            print(f"  {label(key):34s} {key[0]:08X} {key[1]:08X} n={len(r_opens[key])}/{len(e_opens[key])}  "
                  + " ".join(diffs))


RETAIL_MARK = re.compile(r"input script: t=\d+ ms mark (\S+)")
RUST_MARK = re.compile(r"^\d+ \d+ MARK (\S+)")


def windows(path, retail):
    """Posts and opens per `@mark` window, in order: {mark: [(seconds into window, text)]}."""
    opener = gzip.open if str(path).endswith(".gz") else open
    out, mark, t0 = {}, None, 0.0
    with opener(path, "rt", errors="replace") as f:
        for line in f:
            if retail:
                m = STAMP.match(line)
                t = int(m.group(1)) * 3600 + int(m.group(2)) * 60 + float(m.group(3)) if m else None
                mk, post, op = RETAIL_MARK.search(line), RETAIL_MSG.search(line), RETAIL_OPEN.search(line)
            else:
                head = line.split(" ", 1)[0]
                t = int(head) / 1000 if head.isdigit() else None
                mk, post, op = RUST_MARK.match(line), RUST_POST.match(line), RUST_OPEN.match(line)
            if mk:
                mark, t0 = mk.group(1), t
                out.setdefault(mark, [])
            elif mark and post:
                out[mark].append((t - t0, f"post {post.group(1)}"))
            elif mark and op:
                r = records(op.group(3))
                out[mark].append((t - t0, f"  open {int(op.group(1), 16):08X} {int(op.group(2), 16):08X} "
                                          f"gain2={r.get(2)} dry8={r.get(8)} send5={r.get(5)} pitch0={r.get(0)}"))
    return out


def by_mark(audiofiles, rust_trace, retail_trace):
    banks = sample_banks(audiofiles)
    r, e = windows(retail_trace, True), windows(rust_trace, False)
    for mark in r:
        print(f"\n######## {mark}")
        for side, rows in (("RETAIL", r[mark]), ("ENGINE", e.get(mark, []))):
            print(f"  -- {side}")
            for t, text in rows:
                if text.startswith("  open"):
                    w = text.split()
                    text += "  " + "/".join(sorted(banks.get((int(w[1], 16), int(w[2], 16)), {"?"})))
                print(f"    {t:6.2f}s {text}")


if __name__ == "__main__":
    if len(sys.argv) == 5 and sys.argv[1] == "--by-mark":
        by_mark(*sys.argv[2:])
    elif len(sys.argv) >= 4:
        main(*sys.argv[1:])
    else:
        raise SystemExit(__doc__ + "\n    or: compare_retail_audio.py --by-mark <audiofiles.big> <rust-trace> <retail-trace>")
