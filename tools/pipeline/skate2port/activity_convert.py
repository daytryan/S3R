"""Skate 2 activity objectives in the format crates/skate-game/src/skate3_objectives.rs loads (`ObjectiveDef` / `GroupDef`):
snippets as `{src, tree, natives}`, the 20 trigger bytes, the activation mode in Skate 3's numbering. old_activities.py
calls this; the functions are pure so test_old_activities.py runs without a disc.

- snippets: tools/skate3_online.py's `snippet_tree` (imported, not copied). It raises ValueError outside its subset; a
  rejected snippet is reported, never turned into the empty handler (the row is left without a `group`).
- trigger ids: an `EnteredVolume` / `IsInVolume` id is the decimal string in the tree (Skate 3's rules.json,
  skate3_online.objective_entry: "the natives parse argument 1 as the decimal instance id"). Skate 2's Lua passes the id as a
  number; the runtime's numbers are f32 (skate3_objectives.rs `Value::Num`), which cannot hold a 50-bit id, so the number
  becomes its exact decimal string here.
- triggers: the export's 5 words are big-endian u32s; the 20 bytes are their concatenation (word 0 = bytes 0-3, ...).
- activation: Skate 2 inserted no mode 1; Skate 3 did (peer-s2-activities.md, Task 4): 0 stays 0, n >= 1 becomes n + 1.
"""
import struct
import sys
from pathlib import Path

sys.path[:0] = [str(Path(__file__).resolve().parents[3])]  # the repo: `tools` is a namespace package
from tools.skate3_online import COUNT_KIND, FAIL_FIRST, TRIGGER_NATIVES, snippet_tree  # noqa: E402

ID_LIMIT = 2 ** 53  # a Lua number is a double: the ids the snippets spell out are exact below this


def activation(mode):
    """Skate 2 eTriggerActivationMode -> Skate 3's (0..4 in Skate 2)."""
    if mode not in (0, 1, 2, 3, 4):
        raise ValueError(f'activation mode {mode!r}')
    return mode if mode == 0 else mode + 1


def trigger_bytes(words):
    """tObjectiveTriggers: 5 big-endian u32 words -> the 20 bytes (ints)."""
    if len(words) != 5 or any(not isinstance(w, int) or not 0 <= w < 2 ** 32 for w in words):
        raise ValueError(f'triggers {words!r}')
    return list(struct.pack('>5I', *words))


def volume_ids_to_strings(tree):
    """Argument 1 of EnteredVolume / IsInVolume, a number literal, becomes its decimal string (in place)."""
    if isinstance(tree, list):
        if tree[:1] == ['native'] and tree[1] in TRIGGER_NATIVES and tree[2] and tree[2][0][0] == 'num':
            x = tree[2][0][1]
            if x != int(x) or not 0 <= x < ID_LIMIT:
                raise ValueError(f'volume id {x!r} is not an exact integer')
            tree[2][0] = ['str', str(int(x))]
        for x in tree:
            volume_ids_to_strings(x)
    return tree


def volume_ids(tree):
    """The ids (decimal strings) the trigger natives of a converted tree name."""
    out = set()

    def walk(t):
        if isinstance(t, list):
            if t[:1] == ['native'] and t[1] in TRIGGER_NATIVES and t[2] and t[2][0][0] == 'str':
                out.add(t[2][0][1])
            for x in t:
                walk(x)
    walk(tree)
    return out


class Snippets:
    """Converts snippets, numbering each distinct text (`src`: Skate 3 keys the oracle's text by main.bin offset)."""

    def __init__(self):
        self.src, self.sources, self.rejected = {}, {}, []

    def convert(self, text, where):
        """{src, tree, natives}; None for an empty snippet (the handler's default) or one the parser rejects (listed in
        `rejected` as (where, error))."""
        if not (text or '').strip():
            return None
        try:
            tree, natives = snippet_tree(text)
            volume_ids_to_strings(tree)
        except (ValueError, IndexError) as error:
            self.rejected.append((where, f'{type(error).__name__}: {error}'))
            return None
        src = self.src.setdefault(text, len(self.src))
        self.sources[str(src)] = text
        return {'src': src, 'tree': tree, 'natives': natives}


def objective_def(o, raw, snippets, where):
    """One objective as skate3_objectives.rs's `ObjectiveDef`. `o` is the exporter's objective entry, `raw` the vault row's
    fields (v.fields): fail_first and count_kind are the unnamed fields Skate 3 reads (skate3_online.Objectives.objective)."""
    if o['fail_definition'] or o['fail_events']:
        snippets.rejected.append((where, 'fail_definition / fail_events present: not converted'))
    obj = snippets.convert(o['source'], where)
    points_valid = snippets.convert(o['points_valid'], where + ' points_valid')
    return {
        'key': o['key'], 'activation': activation(o['activation']), 'triggers': trigger_bytes(o['triggers']),
        'fail_first': FAIL_FIRST in raw and raw[FAIL_FIRST]['data'][:2] != '00',
        'count_kind': struct.unpack('>h', bytes.fromhex(raw[COUNT_KIND]['data'])[:2])[0] if COUNT_KIND in raw else 0,
        'master': bool(o['master']), 'requires_highlight': bool(o['requires_highlight']),
        'display_in_hud': o['display_in_hud'] is not False,
        'obj': obj, 'points_valid': points_valid, 'failures': [],
    }


def trigger_volumes(defs, by_id):
    """`trigger_volumes` as rules.json has it: every id the row's trees name -> its placed volume (None: no volume of the row
    has that instance id, the row is not playable)."""
    ids = set()
    for d in defs:
        for s in (d['obj'], d['points_valid']):
            if s:
                ids |= volume_ids(s['tree'])
    return {i: by_id.get(i) for i in sorted(ids)}
