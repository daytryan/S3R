import json
import os
import re
import struct
import tempfile
import unittest
from collections import Counter
from pathlib import Path
from unittest import mock

import skate3_career as c
from asset_pipeline.vlt import hash64


def H(name):
    return hash64(name)


def text(s):
    return {"type": "EA::Reflection::Text", "data": s}


def bool_(v):
    return {"type": "EA::Reflection::Bool", "data": ("01" if v else "00") + "000000"}


def int_(v, kind="EA::Reflection::Int32"):
    return {"type": kind, "data": "%08X" % v}


def one_ref(h):
    return {"type": "Attrib::Gen::ClassRefSpec_x", "data": "%016X%016X" % (h, 0)}


def refs(field_type, hashes):
    items = ["%016X%016X" % (h, 0) for h in hashes]
    return {"type": field_type, "data": "00000000", "array": {"capacity": len(items), "element_size": 16, "alignment": 8, "items": items}}


def trigger(kind, key=0, value=0):
    return "%08X%08X%016X%016X" % (kind, 0, key, value)


def triggers(*items):
    return {"type": "Observatory::tObservatoryTrigger", "data": "00000000",
            "array": {"capacity": len(items), "element_size": 24, "alignment": 8, "items": list(items)}}


def event(kind, a, b, flags):
    return "%08X%08X%08X%08X" % (kind, a, b, flags)


def events(*items):
    return {"type": "Sk8::Challenge::tChallengePresentationEvent", "data": "00000000",
            "array": {"capacity": len(items), "element_size": 16, "alignment": 4, "items": list(items)}}


def blob(size, offset):
    return {"type": "Attrib::Blob", "data": "%08X%08X" % (size, offset)}


def rec(cls, key, parent="", **fields):
    return {"class": cls, "key": key, "parent": parent, "fields": fields}


def hkey(name):
    """The key a record whose name the database could not resolve carries."""
    return "Hash_%016X" % H(name)


class FakeDb:
    """The two calls progression() makes of skate3_online.Db, over records built here."""

    def __init__(self, records, binary=b"StrE\0\0\0\0" + b"\0" * 64, names=()):
        self.records, self.binary, self.names = records, binary, list(names)

    def collections(self, rel):
        assert rel == "progressionbanks/main"
        return self.records

    def bin(self, rel):
        assert rel == "progressionbanks/main"
        return self.binary


def bank(*, conditions=None, extra=(), states=None, graph_states=None, binary=None, names=("challenge_a", "challenge_b"),
         handler_fields=None):
    """A small valid bank: action `act` -> graph `g1` (states `initial`, `complete`), group `grp`, reward `rwd`."""
    states = states if states is not None else ["initial", "complete"]
    recs = [rec(c.ACTION, "default", Name=text("default"), **{c.AUTO: bool_(False)}),
            rec(c.GRAPH, "default", Name=text("default")),
            rec(c.STATE, "default"),
            rec(c.HANDLER, "default", **{c.BLOB: blob(0, 0)}),
            rec(c.GROUP, "default", Name=text("default")),
            rec(c.REWARD, "default", **{c.CATEGORY: int_(12), c.ITEM: text("none"), c.TITLE: text("#MISSING HALID"),
                                          c.DESCRIPTION: text("#Description HAL ID"), c.ICON: text("missingtexture"),
                                          c.RAW_A: int_(0x68), c.RAW_B: int_(0)}),
            rec(c.ACTION, "act", "default", Name=text("act"), **{c.AUTO: bool_(True), c.STATEGRAPH: one_ref(H("g1")),
                                                                 c.TRIGGERS: triggers(*(conditions if conditions is not None else []))}),
            rec(c.GROUP, "grp", "default", Name=text("grp"), **{c.GRAPHS: refs("x", [H("challenge_a"), H("challenge_b")])})]
    state_recs, handler_recs, refs_ = [], [], []
    for n in states:
        sk, hk = hkey("state_" + n), hkey("handler_" + n)
        refs_.append(H("state_" + n))
        state_recs.append(rec(c.STATE, sk, "default", Name=text(n), **{c.HANDLERS: refs("x", [H("handler_" + n)])}))
        handler_recs.append(rec(c.HANDLER, hk, "default", Name=text("stateenter"),
                                **(handler_fields if handler_fields is not None else {c.BLOB: blob(4, 8)})))
    recs += state_recs + handler_recs
    recs.append(rec(c.GRAPH, "g1", "default", Name=text("g1"),
                    **{c.STATES: refs("x", graph_states if graph_states is not None else refs_)}))
    recs.append(rec(c.REWARD, "rwd", "default", **{c.TITLE: text("ID_TITLE"), c.ITEM: text("deck")}))
    recs += list(extra)
    return FakeDb(recs, binary if binary is not None else b"StrE\0\0\0\0" + b"\0" * 64, names)


# ---------------------------------------------------------------- rows, worlds, characters: synthetic challenge banks
def float_(v):
    return {"type": "EA::Reflection::Float", "data": struct.pack(">f", v).hex().upper()}


def ref_spec(cls, key):
    return {"type": "Attrib::RefSpec", "data": "%016X%016X%016X" % (cls, key, 0)}


def class_ref(key):
    return {"type": "Attrib::Gen::ClassRefSpec_x", "data": "%016X%016X" % (key, 0)}


def ref_array(cls, keys):
    items = ["%016X%016X%016X" % (cls, k, 0) for k in keys]
    return {"type": "Attrib::RefSpec", "data": "00000000",
            "array": {"capacity": len(items), "element_size": 24, "alignment": 8, "items": items}}


class Bin:
    """A .bin string area: s() gives the offset of a new NUL-terminated string (offset 0 is the StrE header: no string)."""

    def __init__(self):
        self.b = bytearray(b"StrE\0\0\0\0")

    def s(self, text):
        at = len(self.b)
        self.b += text.encode("latin-1") + b"\0"
        return at

    def refs(self, keys):
        at = len(self.b)
        for k in keys:
            self.b += struct.pack(">QQQ", int(c.REWARD[5:], 16), k, 0)
        return at


def loc(bin_, text):
    return {"type": "Sk8::Challenge::tLocationID", "data": "%08X" % (bin_.s(text) if text else 0)}


def world_ref(name):
    return {"type": "Attrib::Gen::ClassRefSpec_world", "data": "%016X%016X" % (H(name), 0)}


def world_loc(bin_, world, text):
    """tWorldLocationID: {u64 field-name hash, u64 world key, u64 0, u32 locator offset, u32 0}."""
    return "%016X%016X%016X%08X%08X" % (H("world"), H(world), 0, bin_.s(text), 0)


def one_world_loc(bin_, world, text):
    return {"type": "Sk8::Challenge::tWorldLocationID", "data": world_loc(bin_, world, text)}


def reward_of(bin_, cash=0, boards=0, rewards=()):
    """tObservatoryProgressionReward {u32 cash, u32 board sales, u32 .bin offset of the RefSpecs, u32 count}."""
    at = bin_.refs([H(r) if isinstance(r, str) else r for r in rewards]) if rewards else 0
    return {"type": "Observatory::tObservatoryProgressionReward",
            "data": "%08X%08X%08X%08X" % (cash, boards, at, len(rewards))}


def global_type(v):
    return int_(v, "Sk8::Challenge::eGlobalType")


class Fix:
    """Challenge banks built row by row: add() puts a `challenges` record and its challenge_global_data record in."""

    GLOBAL_CLASS = int(c.GLOBAL_DATA[5:], 16)

    def __init__(self, bin_=None):
        self.bin = bin_ or Bin()
        self.chal, self.glob, self.local, self.local_bin, self.framework, self.sc = [], [], {}, {}, [], []
        self.sc_bin = Bin()
        self.names = ["dist_one", "dist_two", "dist_three", "rwd_one", "rwd_two", "signup", "gr", "follow"]
        self.add("default", "", {c.GLOBAL_TYPE: global_type(0), c.OWN: reward_of(self.bin), c.KILL: reward_of(self.bin),
                                  c.BONUS: int_(0, "EA::Reflection::UInt32"), c.DISABLED: bool_(False)})
        if bin_ is not None:     # a package: its own copy of `default`; its rows' templates are the disc ones
            return
        self.add("career_tmpl", "default", {c.GLOBAL_TYPE: global_type(3), c.KIND: int_(23, "Sk8::Challenge::eChallengeTypes"),
                                             c.INDICATOR: ref_spec(0, H("signup")), c.SIGNUP_HALF: float_(3.0)},
                 challenge={c.STATEGRAPH: ref_spec(0, H("gr"))})
        self.add("online_tmpl", "default", {})

    def add(self, key, parent, fields, challenge=None):
        self.chal.append(rec(c.CHALLENGE, key, parent, **{c.GLOBAL_REF: ref_spec(self.GLOBAL_CLASS, H(key)), **(challenge or {})}))
        self.glob.append(rec(c.GLOBAL_DATA, key, parent, **fields))
        return self

    def row(self, key, parent="career_tmpl", world="dist_one", fields=None, challenge=None):
        base = {c.ROW_TITLE: text("ID_" + key.upper()), c.ROW_DESCRIPTION: text("ID_%s_DESC" % key.upper()),
                c.LOCATION: loc(self.bin, key + "_loc")}
        if world:
            base[c.WORLD] = world_ref(world)
        return self.add(key, parent, {**base, **(fields or {})}, challenge)

    def world(self, key, parent="default_skate3"):
        self.sc.append(rec(c.WORLD_CLASS, key, parent))
        return self

    def local_data(self, row, parent="", bin_=None, **fields):
        self.local[row] = [rec(c.LOCAL, row, parent, **fields)]
        self.local_bin[row] = bin_ or Bin()
        return self.local_bin[row]

    def db(self):
        return RowsDb(self)


class RowsDb:
    """The calls career_rows / fe_locations / characters make of a Db, over a Fix."""

    def __init__(self, fix):
        self.fix, self.names = fix, list(fix.names)

    def collections(self, rel):
        f = self.fix
        if rel == "challengebanks/main":
            return f.chal + f.glob
        if rel == "challenge_local_data_framework":
            return f.framework
        if rel == "skatercollections":
            return f.sc
        if rel.startswith("challenge_local_data/") and rel.split("/", 1)[1] in f.local:
            return f.local[rel.split("/", 1)[1]]
        raise FileNotFoundError(rel)

    def bin(self, rel):
        f = self.fix
        if rel == "challengebanks/main":
            return bytes(f.bin.b)
        if rel == "skatercollections":
            return bytes(f.sc_bin.b)
        return bytes(f.local_bin[rel.split("/", 1)[1]].b)


class ProgressionShapeTest(unittest.TestCase):
    def test_a_small_bank_exports_the_four_parts_in_record_order(self):
        out = c.progression(bank(conditions=[trigger(2, H("challenge_a"), 0), trigger(1, H("grp"), 2), trigger(0, 0, 500)]))
        self.assertEqual([a["name"] for a in out["actions"]], ["default", "act"])
        act = out["actions"][1]
        self.assertEqual(act, {"name": "act", "key": H("act"), "auto": True, "stategraph": "g1",
                               "conditions": [[2, "challenge_a", 0], [1, "grp", 2], [0, "", 500]]})
        self.assertEqual(out["actions"][0]["conditions"], [])
        self.assertIs(out["actions"][0]["auto"], False)
        self.assertEqual(out["actions"][0]["stategraph"], "")
        self.assertEqual(out["groups"], {"default": [], "grp": ["challenge_a", "challenge_b"]})
        self.assertEqual(out["stategraphs"]["g1"], {"initial": "initial", "states": {
            "initial": {"handlers": {"stateenter": "h_%016X" % H("handler_initial")}, "events": []},
            "complete": {"handlers": {"stateenter": "h_%016X" % H("handler_complete")}, "events": []}}})
        self.assertEqual(out["stategraphs"]["default"], {"initial": "", "states": {}})

    def test_rewards_read_through_their_parents_and_keep_three_raw_fields(self):
        r = c.progression(bank())["rewards"]
        self.assertEqual(r["rwd"], {"category": 12, "item": "deck", "title": "ID_TITLE", "description": "#Description HAL ID",
                                    "icon": "missingtexture", "raw": [0x68, 0, 0]})
        self.assertEqual(r["default"]["item"], "none")

    def test_a_bool_in_the_raw_fields_is_its_value_not_its_bytes(self):
        extra = rec(c.REWARD, "flagged", "default", **{c.RAW_C: bool_(True)})
        self.assertEqual(c.progression(bank(extra=[extra]))["rewards"]["flagged"]["raw"], [0x68, 0, 1])

    def test_a_field_not_on_the_record_is_the_nearest_parent_s(self):
        extra = [rec(c.ACTION, "base", "default", Name=text("base"), **{c.STATEGRAPH: one_ref(H("g1")), c.TRIGGERS: triggers(trigger(0, 0, 7))}),
                 rec(c.ACTION, "child", "base", Name=text("child"))]
        child = c.progression(bank(extra=extra))["actions"][-1]
        self.assertEqual((child["stategraph"], child["conditions"]), ("g1", [[0, "", 7]]))

    def test_the_field_hashes_are_the_names_the_corpus_gives_them(self):
        for const, name in ((c.ACTION, "progression_action"), (c.GROUP, "progression_group"), (c.GRAPH, "progression_stategraph"),
                            (c.STATE, "progression_state"), (c.HANDLER, "progression_handler"), (c.REWARD, "progression_rewards"),
                            (c.TRIGGERS, "Triggers"), (c.STATEGRAPH, "StateGraph"), (c.GRAPHS, "Challenges"),
                            (c.HANDLERS, "Handlers"), (c.TITLE, "HALID"), (c.ICON, "Icon"), (c.DESCRIPTION, "DescriptionHALID")):
            self.assertEqual(const, "Hash_%016X" % H(name), name)


class RefusalTest(unittest.TestCase):
    def refused(self, db, reason):
        with self.assertRaises(c.Refused) as ctx:
            c.progression(db)
        self.assertEqual(ctx.exception.reason, reason)
        return ctx.exception

    def test_a_condition_kind_above_five_is_refused(self):
        e = self.refused(bank(conditions=[trigger(2, H("challenge_a"), 0), trigger(6, 0, 1)]), "condition kind")
        self.assertEqual(e.name, "act")

    def test_every_kind_from_zero_to_five_is_accepted(self):
        got = c.progression(bank(conditions=[trigger(k, H("challenge_a") if k in (1, 2) else 0, 1) for k in range(6)]))
        self.assertEqual([x[0] for x in got["actions"][1]["conditions"]], [0, 1, 2, 3, 4, 5])

    def test_a_state_naming_a_missing_state_is_refused(self):
        e = self.refused(bank(graph_states=[H("state_initial"), 0x1234567890ABCDEF]), "state")
        self.assertEqual(e.name, "g1")

    def test_the_three_retail_graphs_with_dangling_states_export_as_no_states(self):
        # Retail's own bank: 1up_dwtn_05_stategraph, tut_univ_01_stategraph and monster_contest_complete name two states
        # the bank does not hold. Retail ran them as empty graphs (progression.lua 594-596), so they are exported so.
        extra = [rec(c.GRAPH, "1up_dwtn_05_stategraph", "default", Name=text("1up_dwtn_05_stategraph"),
                     **{c.STATES: refs("x", [0xA084CD383295A484, 0x10D4E703317B2393])})]
        got = c.progression(bank(extra=extra))["stategraphs"]["1up_dwtn_05_stategraph"]
        self.assertEqual(got, {"initial": "", "states": {}})

    def test_another_graph_naming_those_missing_keys_is_still_refused(self):
        extra = [rec(c.GRAPH, "other", "default", Name=text("other"), **{c.STATES: refs("x", [0xA084CD383295A484])})]
        self.assertEqual(self.refused(bank(extra=extra), "state").name, "other")

    def test_a_graph_with_states_but_no_initial_one_is_refused(self):
        self.assertEqual(self.refused(bank(states=["complete"]), "state").name, "g1")

    def test_a_handler_with_no_code_is_refused(self):
        self.assertEqual(self.refused(bank(handler_fields={}), "missing").reason, "missing")

    def test_a_handler_whose_code_runs_past_the_bin_is_refused(self):
        self.refused(bank(handler_fields={c.BLOB: blob(1000, 8)}), "missing")

    def test_a_condition_naming_an_unknown_challenge_is_refused(self):
        e = self.refused(bank(conditions=[trigger(2, 0xDEADBEEF, 0)]), "missing")
        self.assertEqual(e.name, "act")

    def test_two_states_of_one_name_are_a_key_collision(self):
        dup = [rec(c.STATE, hkey("state_dup"), "default", Name=text("initial"), **{c.HANDLERS: refs("x", [])})]
        db = bank(extra=dup, graph_states=[H("state_initial"), H("state_complete"), H("state_dup")])
        self.assertEqual(self.refused(db, "key collision").name, "g1")

    def test_two_records_of_one_name_are_a_key_collision(self):
        dup = [rec(c.GROUP, "grp", "default", Name=text("grp"))]
        self.assertEqual(self.refused(bank(extra=dup), "key collision").name, "grp")

    def test_one_event_handled_twice_in_a_state_is_a_key_collision(self):
        extra = [rec(c.HANDLER, hkey("handler_twin"), "default", Name=text("stateenter"), **{c.BLOB: blob(4, 8)})]
        states = [rec(c.STATE, hkey("state_initial"), "default", Name=text("initial"),
                      **{c.HANDLERS: refs("x", [H("handler_initial"), H("handler_twin")])})]
        db = bank(extra=extra)
        db.records[:] = [r for r in db.records if r["key"] != hkey("state_initial")] + states
        self.assertEqual(self.refused(db, "key collision").name, "g1/initial")


class EventTest(unittest.TestCase):
    STRINGS = b"StrE\0\0\0\0" + b"dist_university\0" + b"Create_start_intro\0" + b"\0"

    def test_events_decode_to_kind_args_flags(self):
        base = 8
        uni, intro, empty = base, base + len(b"dist_university\0"), base + len(b"dist_university\0Create_start_intro\0")
        evs = events(event(8, intro, uni, 0), event(5, intro, empty, 3), event(68, 0, 0, 0))
        state = rec(c.STATE, hkey("state_initial"), "default", Name=text("initial"),
                    **{c.HANDLERS: refs("x", [H("handler_initial")]), "Events": evs})
        db = bank(binary=self.STRINGS)
        db.records[:] = [r for r in db.records if r["key"] != hkey("state_initial")] + [state]
        got = c.progression(db)["stategraphs"]["g1"]["states"]["initial"]["events"]
        self.assertEqual(got, [{"kind": 8, "args": ["Create_start_intro", "dist_university"], "flags": 0},
                               {"kind": 5, "args": ["Create_start_intro", ""], "flags": 3},
                               {"kind": 68, "args": ["", ""], "flags": 0}])

    def test_an_event_string_outside_the_bin_is_refused(self):
        state = rec(c.STATE, hkey("state_initial"), "default", Name=text("initial"),
                    **{c.HANDLERS: refs("x", [H("handler_initial")]), "Events": events(event(5, 99999, 0, 0))})
        db = bank()
        db.records[:] = [r for r in db.records if r["key"] != hkey("state_initial")] + [state]
        with self.assertRaises(c.Refused) as ctx:
            c.progression(db)
        self.assertEqual(ctx.exception.reason, "missing")


# ---------------------------------------------------------------- the Lua: unluac stubbed with made-up code
BODY = b'Unlock.UnlockProgressionReward("rwd", false)\nChangeState("complete")\n'


def fake_run(chunks, java=None, unluac=None):
    return {ch: (BODY, 0) for ch in chunks}


def failing_run(chunks, java=None, unluac=None):
    return {ch: (BODY, 1) for ch in chunks}      # unluac exits 1 but keeps what it printed so far


def fake_graphs(db, graphs):
    return {g: f"-- {g}\nfunction made_up()\nend\n" for g in graphs}


class Scripted(unittest.TestCase):
    """export() decompiles the handlers and the graph chains: these tests give it made-up Lua instead of unluac's."""

    def setUp(self):
        for target, attr, fake in ((c.unluac_batch, "run", fake_run), (c, "graphs_lua", fake_graphs)):
            patch = mock.patch.object(target, attr, fake)
            patch.start()
            self.addCleanup(patch.stop)


class WriteTest(Scripted):
    def test_career_json_carries_version_one_and_every_part(self):
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d) / "out", {"actions": [1], "groups": {"g": []}})
            raw = (Path(d) / "out/career.json").read_bytes()
            self.assertNotIn(b"\r", raw)
            self.assertEqual(json.loads(raw), {"version": 1, "actions": [1], "groups": {"g": []}})
            self.assertEqual([p.name for p in (Path(d) / "out").iterdir()], ["career.json"])

    def test_a_second_write_replaces_the_first(self):
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"a": 1})
            c.write(Path(d), {"b": 2})
            self.assertEqual(json.loads((Path(d) / "career.json").read_text(encoding="utf-8")), {"version": 1, "b": 2})
            self.assertEqual([p.name for p in Path(d).iterdir()], ["career.json"])

    def test_nothing_is_written_when_a_part_is_refused(self):
        def refuse():
            raise c.Refused("decompile", "h_1")
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / "out"
            with self.assertRaises(c.Refused):
                c.write(out, {"actions": [], "scripts": refuse})
            self.assertFalse((out / "career.json").exists())
            self.assertEqual(list(out.iterdir()) if out.exists() else [], [])

    def test_a_refused_export_keeps_the_career_json_already_there(self):
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"a": 1})
            with self.assertRaises(c.Refused):
                c.export(bank(conditions=[trigger(9, 0, 0)]), Path(d))
            self.assertEqual(json.loads((Path(d) / "career.json").read_text(encoding="utf-8")), {"version": 1, "a": 1})
            self.assertEqual([p.name for p in Path(d).iterdir()], ["career.json"])

    def test_export_writes_the_progression_parts(self):
        with tempfile.TemporaryDirectory() as d:
            c.export(CareerFake(minimal_world(), bank()), Path(d))
            got = json.loads((Path(d) / "career.json").read_text(encoding="utf-8"))
            self.assertEqual(got["version"], 1)
            self.assertTrue({"actions", "groups", "rewards", "stategraphs"} <= set(got))

    def test_refused_names_its_reason_and_record(self):
        e = c.Refused("condition kind", "act", "kind 6")
        self.assertEqual((e.reason, e.name), ("condition kind", "act"))
        self.assertIn("act", str(e))
        with self.assertRaises(ValueError):
            c.Refused("not a reason", "x")


CHALLENGE_CLASS = int(c.CHALLENGE[5:], 16)


class RowsTest(unittest.TestCase):
    def rows(self, fix, **kw):
        return c.career_rows(fix.db(), **kw)

    def refused(self, fix, reason, **kw):
        with self.assertRaises(c.Refused) as ctx:
            self.rows(fix, **kw)
        self.assertEqual(ctx.exception.reason, reason)
        return ctx.exception

    def test_a_row_with_no_world_is_refused(self):
        e = self.refused(Fix().row("row_x", world=None), "missing")
        self.assertEqual(e.name, "row_x")

    def test_a_row_exports_every_field(self):
        fix = Fix()
        b = fix.bin
        fix.row("row_a", fields={c.MAP_START: loc(b, "row_a_map"), c.OWN: reward_of(b, 100, 5, ["rwd_one", "rwd_two"]),
                                 c.KILL: reward_of(b, 7, 9), c.BONUS: int_(250, "EA::Reflection::UInt32"),
                                 c.SIGNUP_HALF: float_(10.0), c.END_TELEPORT: bool_(True),
                                 c.END_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": world_loc(b, "dist_one", "row_a_end")}})
        self.assertEqual(self.rows(fix), {"row_a": {
            "key": H("row_a"), "title": "ID_ROW_A", "description": "ID_ROW_A_DESC", "kind": 23, "graph": "gr",
            "world": "dist_one", "location": "row_a_loc", "map_start": "row_a_map", "start": None, "offboard": False,
            "indicator": "signup", "own": {"cash": 100, "boards": 5, "rewards": ["rwd_one", "rwd_two"]},
            "kill": {"cash": 7, "boards": 9, "rewards": []}, "online_bonus": 250, "killable": False, "lot": None,
            "lot_rows": [], "disabled": False, "global": 3, "signup_half": 10.0, "end_teleport": "row_a_end",
            "end_world": "dist_one", "pack": None, "exits": []}})

    def test_a_field_not_on_the_row_is_the_nearest_parent_s(self):
        fix = Fix()
        fix.add("lesson_tmpl", "career_tmpl", {c.OWN: reward_of(fix.bin, 40, 2, ["rwd_one"]), c.SIGNUP_HALF: float_(4.0),
                                               c.INDICATOR: ref_spec(0, H("follow"))})
        fix.row("row_b", "lesson_tmpl")
        fix.row("row_c", "lesson_tmpl", fields={c.SIGNUP_HALF: float_(0.0)})
        got = self.rows(fix)
        self.assertEqual((got["row_b"]["signup_half"], got["row_b"]["indicator"], got["row_b"]["graph"]), (4.0, "follow", "gr"))
        self.assertEqual(got["row_b"]["own"], {"cash": 40, "boards": 2, "rewards": ["rwd_one"]})
        self.assertEqual(got["row_c"]["signup_half"], 0.0)    # stored as is: the 0-or-less rule is the game's (spec 6.2)

    def test_a_row_with_no_map_start_or_indicator_says_so(self):
        fix = Fix()
        fix.add("bare_tmpl", "default", {c.GLOBAL_TYPE: global_type(3)})
        fix.row("row_d", "bare_tmpl")
        fix.row("row_d2")        # career_tmpl needs a child, or it is a row itself
        got = self.rows(fix)["row_d"]
        self.assertEqual((got["map_start"], got["indicator"], got["graph"], got["kind"]), (None, "", "", 0))

    def test_only_the_rows_the_career_can_reach_are_exported(self):
        fix = Fix()
        fix.row("career_a")
        fix.row("tele", fields={c.GLOBAL_TYPE: global_type(2)})
        fix.row("online_a", "online_tmpl")
        fix.row("pooled", "online_tmpl")
        fix.row("lot_x", "online_tmpl", fields={c.GLOBAL_TYPE: global_type(3), c.LOT_ROWS: ref_array(CHALLENGE_CLASS, [H("act_a")])})
        fix.row("act_a", "online_tmpl", fields={c.LOT: class_ref(H("lot_x"))})
        got = self.rows(fix, members={"pooled", "not_a_row"})
        self.assertEqual(sorted(got), ["act_a", "career_a", "lot_x", "pooled", "tele"])
        self.assertEqual((got["act_a"]["lot"], got["act_a"]["global"], got["lot_x"]["lot_rows"]), ("lot_x", 0, ["act_a"]))
        self.assertEqual(got["tele"]["global"], 2)

    def test_killable_is_a_death_race_or_a_killed_it_objective(self):
        fix = Fix()
        killed = ref_array(CHALLENGE_CLASS, [H("obj_1")])
        fix.add("killer_tmpl", "career_tmpl", {c.KILLED_IT: killed})
        fix.row("r_death", fields={c.KIND: int_(0x13, "Sk8::Challenge::eChallengeTypes")})
        fix.row("r_kill", fields={c.KILLED_IT: killed})
        fix.row("r_none")
        fix.row("r_inherits", "killer_tmpl")
        fix.row("r_overrides", "killer_tmpl", fields={c.KILLED_IT: ref_array(CHALLENGE_CLASS, [])})
        got = {k: v["killable"] for k, v in self.rows(fix).items()}
        self.assertEqual(got, {"r_death": True, "r_kill": True, "r_none": False, "r_inherits": True, "r_overrides": False})

    def test_a_reward_naming_no_record_is_refused_and_an_empty_one_skipped(self):
        fix = Fix()
        fix.row("row_e", fields={c.OWN: reward_of(fix.bin, 0, 0, [0, "rwd_one"])})
        self.assertEqual(self.rows(fix)["row_e"]["own"]["rewards"], ["rwd_one"])
        fix.row("row_f", fields={c.OWN: reward_of(fix.bin, 0, 0, [0x1234567890ABCDEF])})
        e = self.refused(fix, "missing")
        self.assertEqual(e.name, "row_f")

    def test_a_stategraph_that_names_no_record_is_refused(self):
        fix = Fix()
        fix.row("row_h", challenge={c.STATEGRAPH: ref_spec(0, 0x1234)})
        self.assertEqual(self.refused(fix, "missing").name, "row_h")

    def test_an_indicator_the_name_table_cannot_name_keeps_the_converter_s_spelling(self):
        fix = Fix()
        fix.row("row_g", fields={c.INDICATOR: ref_spec(0, 0x1234)})
        fix.row("row_g2")
        self.assertEqual(self.rows(fix)["row_g"]["indicator"], "Hash_0000000000001234")
        for name in c.INDICATORS:      # the keys career_flow 6.2 names, each resolvable
            self.assertEqual(c.name_table(fix.db())[H(name)], name)

    def test_a_world_the_name_table_cannot_name_is_refused(self):
        fix = Fix()
        fix.row("row_w", world="dist_unknown")
        fix.row("row_w2")
        e = self.refused(fix, "missing")
        self.assertEqual(e.name, "row_w")
        self.assertIn("%016X" % H("dist_unknown"), str(e))

    def test_an_unnamed_exit_or_end_world_is_refused_too(self):
        fix = Fix()
        lb = Bin()
        fix.row("row_x1")
        fix.row("row_x2")
        fix.local_data("row_x1", "", lb, **{c.EXIT_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": "00000000",
                                                             "array": {"items": [world_loc(lb, "dist_unknown", "exit_x")]}}})
        self.assertEqual(self.refused(fix, "missing").name, "row_x1")
        fix = Fix()
        fix.row("row_x3", fields={c.END_TELEPORT: bool_(True),
                                  c.END_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": world_loc(fix.bin, "dist_unknown", "end_x")}})
        fix.row("row_x4")
        self.assertEqual(self.refused(fix, "missing").name, "row_x3")

    def test_the_one_world_retail_leaves_unnamed_passes_and_maps_to_no_engine_map(self):
        # street_ind_01 (a cut Global 3 row, order 5) and two DLC templates name EC763034105B866B; no record or string names it
        self.assertEqual(set(c.UNNAMED_WORLDS), {0xEC763034105B866B})
        fix = Fix()
        fix.row("row_u")
        fix.row("row_u2")
        fix.glob[[r["key"] for r in fix.glob].index("row_u")]["fields"][c.WORLD] = {
            "type": "Attrib::Gen::ClassRefSpec_world", "data": "%016X%016X" % (0xEC763034105B866B, 0)}
        got = self.rows(fix)["row_u"]["world"]
        self.assertEqual(got, "Hash_EC763034105B866B")
        self.assertEqual(c.world_maps(fix.db(), [got]), {"Hash_EC763034105B866B": None})

    def test_an_anywhere_row_has_no_world_and_is_not_refused(self):
        fix = Fix()
        fix.row("row_any", world=None, fields={c.KIND: int_(0x25, "Sk8::Challenge::eChallengeTypes")})
        fix.row("row_any2", world=None, fields={c.KIND: int_(0x27, "Sk8::Challenge::eChallengeTypes")})
        fix.row("row_w2")
        got = self.rows(fix)
        self.assertEqual((got["row_any"]["world"], got["row_any2"]["world"]), ("", ""))

    def test_a_row_whose_global_data_is_missing_is_refused(self):
        fix = Fix()
        fix.row("row_i")
        fix.glob[:] = [r for r in fix.glob if r["key"] != "row_i"]
        self.assertEqual(self.refused(fix, "missing").name, "row_i")

    def test_the_local_data_gives_start_offboard_and_exits(self):
        fix = Fix()
        fix.framework.append(rec("fw", "local_tmpl", **{c.SPAWN_OFFBOARD: bool_(True)}))
        lb = Bin()
        fix.row("row_l")
        fix.row("row_m")
        fix.local_data("row_l", "local_tmpl", lb, **{
            c.START_LOCATION: loc(lb, "row_l_start"),
            c.EXIT_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": "00000000", "array": {"items": [
                world_loc(lb, "dist_two", "exit_a"), world_loc(lb, "dist_three", "exit_b")]}}})
        got = self.rows(fix)
        self.assertEqual((got["row_l"]["start"], got["row_l"]["offboard"]), ("row_l_start", True))
        self.assertEqual(got["row_l"]["exits"], [{"world": "dist_two", "location": "exit_a"}, {"world": "dist_three", "location": "exit_b"}])
        # no local data file at all: no start, not offboard, no exits
        self.assertEqual((got["row_m"]["start"], got["row_m"]["offboard"], got["row_m"]["exits"]), (None, False, []))

    def test_a_row_s_own_offboard_value_beats_its_framework_parent(self):
        fix = Fix()
        fix.framework.append(rec("fw", "local_tmpl", **{c.SPAWN_OFFBOARD: bool_(True)}))
        fix.row("row_n")
        fix.local_data("row_n", "local_tmpl", **{c.SPAWN_OFFBOARD: bool_(False)})
        self.assertIs(self.rows(fix)["row_n"]["offboard"], False)

    def test_an_end_teleport_needs_its_flag(self):
        fix = Fix()
        fix.row("row_o", fields={c.END_TELEPORT: bool_(False),
                                 c.END_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": world_loc(fix.bin, "dist_one", "end_o")}})
        fix.row("row_p", fields={c.END_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": "00" * 32}})
        got = self.rows(fix)
        self.assertEqual((got["row_o"]["end_teleport"], got["row_p"]["end_teleport"]), (None, None))
        self.assertEqual((got["row_o"]["end_world"], got["row_p"]["end_world"]), (None, None))

    def test_an_end_teleport_can_name_another_world(self):
        fix = Fix()
        fix.row("row_s", fields={c.END_TELEPORT: bool_(True),
                                 c.END_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": world_loc(fix.bin, "dist_two", "end_s")}})
        fix.row("row_s2")
        got = self.rows(fix)["row_s"]
        self.assertEqual((got["end_teleport"], got["end_world"]), ("end_s", "dist_two"))

    def test_a_disabled_row_is_exported_with_its_flag(self):
        fix = Fix()
        fix.row("row_q", fields={c.GLOBAL_TYPE: global_type(2), c.DISABLED: bool_(True)})
        self.assertIs(self.rows(fix)["row_q"]["disabled"], True)

    def test_records_named_by_their_resolved_names_read_like_hash_keys(self):
        fix = Fix()
        fix.row("row_r", fields={c.OWN: reward_of(fix.bin, 5, 6, ["rwd_one"])})
        plain = self.rows(fix)
        names = {c.ROW_TITLE: "Title", c.ROW_DESCRIPTION: "Description", c.WORLD: "World", c.LOCATION: "Location",
                 c.KIND: "ChallengeType", c.GLOBAL_TYPE: "Global", c.END_TELEPORT: "EndTeleport", c.STATEGRAPH: "StateGraph",
                 c.INDICATOR: "SignUpIndicator", c.KILLED_IT: "ObjectivesKilledIt", c.LOT_ROWS: "OnlineFreeSkateActivities"}
        for r in fix.chal + fix.glob:
            r["class"] = "challenges" if r["class"] == c.CHALLENGE else r["class"]
            r["fields"] = {names.get(k, k): v for k, v in r["fields"].items()}
        self.assertEqual(self.rows(fix), plain)

    def test_the_named_fields_are_the_names_the_corpus_gives_them(self):
        for const, name in ((c.CHALLENGE, "challenges"), (c.GLOBAL_DATA, "challenge_global_data"), (c.LOCAL, "challenge_local_data"),
                            (c.ROW_TITLE, "Title"), (c.ROW_DESCRIPTION, "Description"), (c.KIND, "ChallengeType"),
                            (c.WORLD, "World"), (c.LOCATION, "Location"), (c.MAP_START, "MapStartLocation"),
                            (c.INDICATOR, "SignUpIndicator"), (c.KILLED_IT, "ObjectivesKilledIt"),
                            (c.LOT_ROWS, "OnlineFreeSkateActivities"), (c.GLOBAL_TYPE, "Global"), (c.END_TELEPORT, "EndTeleport"),
                            (c.START_LOCATION, "StartLocation"), (c.SPAWN_OFFBOARD, "SpawnOffboard"),
                            (c.EXIT_LOCATION, "TeleportExitLocation"), (c.WORLD_CLASS, "world"),
                            (c.FE_LOCATIONS, "fe_locations"), (c.MARQUEE, "characters_marquee"), (c.RECIPE, "recipe"),
                            (c.VALID, "valid"), (c.FE_LOCATION, "location")):
            self.assertEqual(const, "Hash_%016X" % H(name), name)


class DlcFake(RowsDb):
    """A DlcDb as career_rows sees it: the disc's records, then the package's own (with the `default` copies the
    packages carry), bank .bin shared."""

    def __init__(self, base, own, pack="dlc_pack", delta=0):
        super().__init__(own)
        self.base_fix, self.base, self.banks, self.names = base, RowsDb(base), [Path(pack + ".vlt")], list(base.names)
        self.delta = delta     # where the package's own .bin sits in the combined one (0: they share one)

    def collections(self, rel):
        if rel == "challengebanks/main":
            return self.base.collections(rel) + self.fix.chal + self.fix.glob
        return self.base.collections(rel)

    def bin(self, rel):
        main = self.base.bin(rel)
        if rel != "challengebanks/main" or not self.delta:
            return main
        return main + bytes(self.delta - len(main)) + bytes(self.fix.bin.b)


class DlcRowsTest(unittest.TestCase):
    def test_a_package_s_rows_carry_its_pack_id_and_its_default_copies_are_dropped(self):
        base = Fix()
        base.row("disc_a")
        own = Fix(base.bin)       # a package carries its own copy of `default`: the disc one wins
        own.row("pkg_a", fields={c.GLOBAL_TYPE: global_type(3), c.WORLD: world_ref("dist_two")})
        d = DlcFake(base, own)
        got = c.career_rows(d, pack="dlc_pack", base=d.base)
        self.assertEqual(list(got), ["pkg_a"])
        self.assertEqual((got["pkg_a"]["pack"], got["pkg_a"]["world"], got["pkg_a"]["global"]), ("dlc_pack", "dist_two", 3))
        self.assertEqual(got["pkg_a"]["graph"], "gr")    # the row's chain reaches the disc's template
        self.assertIsNone(c.career_rows(base.db())["disc_a"]["pack"])

    def test_a_package_template_nothing_inherits_from_is_not_a_row(self):
        # dlc_dwgh_anywhere_photos / _films are templates no row uses; every package names its templates dlc_<...> (order 9)
        base = Fix()
        base.row("disc_a")
        own = Fix(base.bin)
        own.row("pkg_a")
        own.row("dlc_pack_anywhere_photos")
        d = DlcFake(base, own)
        self.assertEqual(list(c.career_rows(d, pack="dlc_pack", base=d.base)), ["pkg_a"])

    def test_a_package_s_reward_list_is_read_at_its_rebased_offset(self):
        # skate3_online.rebase moves the offsets of the types it knows (tLocationID, ...) by the package's place in the
        # combined .bin, but not a tObservatoryProgressionReward's list offset: career_rows adds the package's delta to those.
        base = Fix()
        base.row("disc_a", fields={c.OWN: reward_of(base.bin, 1, 2, ["rwd_two"])})
        delta = 4096
        own_bin = Bin()
        own = Fix(own_bin)
        own.row("pkg_c", fields={c.OWN: reward_of(own_bin, 10, 20, ["rwd_one"]), c.KILL: reward_of(own_bin, 3, 4, ["rwd_two"])})
        own.glob[-1]["fields"][c.LOCATION] = {"type": "Sk8::Challenge::tLocationID", "data": "%08X" % (delta + own_bin.s("pkg_c_loc"))}
        own.row("pkg_d", "lesson_tmpl")      # inherits the disc template's reward (the disc's, so not moved)
        base.add("lesson_tmpl", "career_tmpl", {c.OWN: reward_of(base.bin, 5, 6, ["rwd_one"])})
        d = DlcFake(base, own, delta=delta)
        got = c.career_rows(d, pack="dlc_pack", base=d.base)
        self.assertEqual(got["pkg_c"]["own"], {"cash": 10, "boards": 20, "rewards": ["rwd_one"]})
        self.assertEqual(got["pkg_c"]["kill"], {"cash": 3, "boards": 4, "rewards": ["rwd_two"]})
        self.assertEqual(got["pkg_c"]["location"], "pkg_c_loc")
        self.assertEqual(got["pkg_d"]["own"], {"cash": 5, "boards": 6, "rewards": ["rwd_one"]})

    def test_the_package_s_strings_are_read_at_its_own_offsets(self):
        base = Fix()
        base.row("disc_a")
        own = Fix(base.bin)       # the package's rows were rebased onto the same .bin
        own.row("pkg_b")
        d = DlcFake(base, own)
        self.assertEqual(c.career_rows(d, pack="dlc_pack", base=d.base)["pkg_b"]["location"], "pkg_b_loc")


def minimal_world():
    """Two rows on one world, two worlds, the two fe_locations, three characters, as skatercollections holds them."""
    fix = Fix()
    fix.row("challenge_a")
    fix.row("challenge_b", fields={c.OWN: reward_of(fix.bin, 0, 0, ["rwd_one"])})
    fix.world("dist_one").world("dist_two").world("dist_one_child", "dist_one")
    b = fix.sc_bin
    fix.sc += [
        rec(c.FE_LOCATIONS, "default", "", **{c.WORLD: world_ref("dist_two"), c.FE_LOCATION: loc(b, "")}),
        rec(c.FE_LOCATIONS, "base_start", "default", **{c.WORLD: world_ref("dist_one")}),
        rec(c.FE_LOCATIONS, "schoolstart", "default", **{c.FE_LOCATION: loc(b, "school_locator")}),
        rec(c.FE_LOCATIONS, "gamestart", "base_start", **{c.FE_LOCATION: loc(b, "game_locator")}),
        rec(c.MARQUEE, "default", "", **{c.RECIPE: text(""), c.VALID: bool_(False), c.DISPLAY: text("")}),
        rec(c.MARQUEE, hkey("pro_skaters"), "default"),
        rec(c.MARQUEE, "ip", "default"),
        rec(c.MARQUEE, "mike", hkey("pro_skaters"), **{c.RECIPE: text("mike_r"), c.VALID: bool_(True), c.DISPLAY: text("ID_COMMON_MIKE")}),
        rec(c.MARQUEE, "reda", "ip", **{c.RECIPE: text("giovanni_r"), c.VALID: bool_(True), c.DISPLAY: text("ID_COMMON_REDA")}),
        rec(c.MARQUEE, "mike_prop", hkey("pro_skaters_props"), **{c.RECIPE: text("mike_prop"), c.VALID: bool_(True)}),
        rec(c.MARQUEE, "tutorial", "default", **{c.RECIPE: text("tutorial_male"), c.VALID: bool_(True)}),
        rec(c.MARQUEE, "hidden", "ip", **{c.RECIPE: text("hidden_r"), c.VALID: bool_(False)}),
    ]
    return fix


class WorldsTest(unittest.TestCase):
    MAPS = {"dist_one": "MapOne", "dist_two": "MapTwo"}

    def test_a_world_maps_to_its_engine_map_through_its_parents_else_null(self):
        fix = minimal_world().world("dist_nomap")
        got = c.world_maps(fix.db(), ["dist_one", "dist_one_child", "dist_nomap", "dist_two"], maps=self.MAPS)
        self.assertEqual(got, {"dist_one": "maps/MapOne.skate", "dist_one_child": "maps/MapOne.skate",
                               "dist_nomap": None, "dist_two": "maps/MapTwo.skate"})

    def test_a_world_with_no_record_and_no_map_is_null_and_one_with_a_map_needs_no_record(self):
        fix = Fix()
        self.assertEqual(c.world_maps(fix.db(), ["dist_one", "dist_ghost"], maps=self.MAPS),
                         {"dist_one": "maps/MapOne.skate", "dist_ghost": None})

    def test_the_default_map_table_is_the_engine_s_online_map_table(self):
        import online_challenge_map as ocm
        self.assertIs(c.WORLD_MAPS, ocm.WORLD_MAPS)
        got = c.world_maps(Fix().db(), ["dist_skateschool", "dist_university"])
        self.assertEqual(got, {"dist_skateschool": "maps/SkateSchool.skate", "dist_university": "maps/University.skate"})


class FeLocationsTest(unittest.TestCase):
    def test_schoolstart_and_gamestart_give_world_and_locator_through_their_parents(self):
        self.assertEqual(c.fe_locations(minimal_world().db()), {
            "schoolstart": {"world": "dist_two", "location": "school_locator"},
            "gamestart": {"world": "dist_one", "location": "game_locator"}})

    def test_a_missing_fe_location_is_refused(self):
        fix = minimal_world()
        fix.sc[:] = [r for r in fix.sc if r["key"] != "gamestart"]
        with self.assertRaises(c.Refused) as ctx:
            c.fe_locations(fix.db())
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "gamestart"))

    def test_a_locator_that_is_empty_is_refused(self):
        fix = minimal_world()
        fix.sc[:] = [r for r in fix.sc if r["key"] != "schoolstart"] + [
            rec(c.FE_LOCATIONS, "schoolstart", "default", **{c.FE_LOCATION: loc(fix.sc_bin, "")})]
        with self.assertRaises(c.Refused) as ctx:
            c.fe_locations(fix.db())
        self.assertEqual(ctx.exception.name, "schoolstart")


class CharactersTest(unittest.TestCase):
    def test_the_roster_is_the_valid_pro_and_other_skaters_in_record_order(self):
        got, key = c.characters(minimal_world().db())
        self.assertEqual(got, [
            {"key": "mike", "recipe": "mike_r", "name": "ID_COMMON_MIKE", "category": "pro", "unlock": "mike_r"},
            {"key": "reda", "recipe": "giovanni_r", "name": "ID_COMMON_REDA", "category": "other", "unlock": "giovanni_r"}])
        self.assertEqual(key, "all_characters_unlock")

    def test_the_three_keys_the_name_table_cannot_resolve_are_named_here(self):
        # steak, isaak and dr_pepper are record keys freeplay 3.3 names; the exporter's name table has no string for them.
        for name in ("steak", "isaak", "dr_pepper"):
            self.assertEqual(c.ROSTER_KEYS[H(name)], name)
        fix = minimal_world()
        fix.sc.append(rec(c.MARQUEE, hkey("steak"), "ip", **{c.RECIPE: text("security_r"), c.VALID: bool_(True), c.DISPLAY: text("ID_COMMON_STEAK")}))
        self.assertEqual(c.characters(fix.db())[0][-1]["key"], "steak")

    def test_an_unnamed_roster_key_is_refused(self):
        fix = minimal_world()
        fix.sc.append(rec(c.MARQUEE, hkey("nobody_knows"), "ip", **{c.RECIPE: text("x"), c.VALID: bool_(True)}))
        with self.assertRaises(c.Refused) as ctx:
            c.characters(fix.db())
        self.assertEqual(ctx.exception.reason, "missing")

    def test_a_listed_character_with_no_recipe_is_refused(self):
        fix = minimal_world()
        fix.sc.append(rec(c.MARQUEE, "norecipe", "ip", **{c.VALID: bool_(True), c.DISPLAY: text("ID_X")}))
        fix.sc[-1]["fields"][c.RECIPE] = text("")
        with self.assertRaises(c.Refused) as ctx:
            c.characters(fix.db())
        self.assertEqual(ctx.exception.name, "norecipe")


class StringsTest(unittest.TestCase):
    # Synthetic ids and text only: the real table is the player's own and never goes into the repo.
    def test_the_table_holds_every_id_the_career_names_and_nothing_else(self):
        career = {"rows": {"r": {"title": "ID_A", "description": "ID_NOPE", "tags": ["ID_B", "#a literal"]}},
                  "n": 3, "world": "dist_one", "ID_KEY_ONLY": "x"}
        language = {"ID_A": "Alpha", "ID_B": "Beta", "ID_UNUSED": "Unused", "ID_KEY_ONLY": "key", "#a literal": "never a table entry"}
        table, missing = c.strings(career, language)
        self.assertEqual(table, {"ID_A": "Alpha", "ID_B": "Beta"})
        self.assertEqual(missing, ["ID_NOPE"])

    def test_missing_covers_titles_descriptions_and_character_names_only(self):
        career = {"rows": {"r": {"title": "ID_T", "description": ""}}, "rewards": {"w": {"title": "ID_RT", "description": "#lit"}},
                  "characters": [{"name": "ID_CN"}], "stategraphs": {"g": {"states": {"s": {"events": [{"args": ["Some_Nis", "ID_ARG"]}]}}}}}
        table, missing = c.strings(career, {"ID_T": "t"})
        self.assertEqual(missing, ["ID_CN", "ID_RT"])
        self.assertEqual(table, {"ID_T": "t"})


class ReportNamesTest(unittest.TestCase):
    def test_a_package_s_reports_name_its_records(self):
        from types import SimpleNamespace as NS
        files = {"data/db/progressionbanks/dlc_x_summaryreport.txt": b"/progression_rewards.class/rwd_x.xml\n/progression_rewards.class/rwd_y.xml\n",
                 "data/db/challengebanks/dlc_x_summaryreport.txt": b"/challenges.class/row_x.xml\n",
                 "data/db/elsewhere_summaryreport.txt": b"/other.class/not_read.xml\n",
                 "data/db/progressionbanks/dlc_x.vlt": b"/ignored.class/binary.xml"}
        archive = NS(entries=[NS(path=p) for p in files], read=lambda e: files[e.path])
        self.assertEqual(c.report_names(archive), ["rwd_x", "rwd_y", "row_x"])
        self.assertEqual(c.report_names(None), [])


def fake_stfs(path, short=0, total=17):
    """A file that starts like an STFS package: magic, header size, volume descriptor with `total` blocks. It is `short` bytes
    shorter than those blocks need, and zeros after the header."""
    h = bytearray(0x1000)
    h[:4] = b"CON "
    struct.pack_into(">I", h, 0x340, 0xA000)
    h[0x379] = 0x24
    h[0x379 + 2] = 1
    struct.pack_into(">I", h, 0x379 + 0x1C, total)
    size = c.stfs_end(bytes(h)) - short
    Path(path).write_bytes(bytes(h) + bytes(size - len(h)))
    return size


class DlcFolderTest(unittest.TestCase):
    def load(self, folder, say=None):
        said = []
        got = c.load_dlcs(folder, RowsDb(Fix()), "unused", say or said.append)
        return got, said

    def test_a_folder_that_does_not_exist_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(c.Refused) as ctx:
                self.load(Path(d) / "nowhere")
            self.assertEqual(ctx.exception.reason, "missing")

    def test_a_folder_with_no_packages_gives_the_disc_alone_and_one_notice(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "readme.txt").write_text("not a package")
            got, said = self.load(Path(d))
            self.assertEqual((got, len(said)), ([], 1))
            self.assertIn("disc rows only", said[0])

    def test_a_truncated_package_is_refused_not_skipped(self):
        with tempfile.TemporaryDirectory() as d:
            fake_stfs(Path(d) / "good", short=0)
            fake_stfs(Path(d) / "cut", short=4096)
            from unittest import mock
            with mock.patch.object(c, "dlc_packages", side_effect=AssertionError("must refuse before reading")):
                with self.assertRaises(c.Refused) as ctx:
                    self.load(Path(d))
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "cut"))

    def test_a_package_with_only_a_header_stub_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "stub").write_bytes(b"CON " + bytes(100))
            with self.assertRaises(c.Refused) as ctx:
                self.load(Path(d))
            self.assertEqual(ctx.exception.name, "stub")

    def test_complete_packages_are_handed_on_and_a_small_unlock_package_may_be_skipped(self):
        from unittest import mock

        def fake(folder, base, work):
            print("skipped DLC package unlock (118784 bytes): IndexError('x')")
            return ["db"]
        with tempfile.TemporaryDirectory() as d:
            fake_stfs(Path(d) / "unlock", short=0)
            with mock.patch.object(c, "dlc_packages", side_effect=fake):
                got, said = self.load(Path(d))
            self.assertEqual(got, ["db"])
            self.assertEqual(said, ["skipped DLC package unlock (118784 bytes): IndexError('x')"])

    def test_a_large_package_the_reader_skipped_is_refused(self):
        from unittest import mock

        def fake(folder, base, work):
            print("skipped DLC package big (5000000 bytes): IndexError('x')")
            return []
        with tempfile.TemporaryDirectory() as d:
            fake_stfs(Path(d) / "big", short=0)
            with mock.patch.object(c, "dlc_packages", side_effect=fake):
                with self.assertRaises(c.Refused) as ctx:
                    self.load(Path(d))
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "big"))


class WriteStringsTest(unittest.TestCase):
    def test_strings_json_is_written_beside_career_json_as_lf_utf8(self):
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"rows": {"r": {"title": "ID_A"}}}, strings=lambda built: {"ID_A": "Café"})
            raw = (Path(d) / "strings.json").read_bytes()
            self.assertNotIn(b"\r", raw)
            self.assertEqual(json.loads(raw.decode("utf-8")), {"ID_A": "Café"})
            self.assertIn("Café".encode("utf-8"), raw)
            self.assertEqual(sorted(p.name for p in Path(d).iterdir()), ["career.json", "strings.json"])

    def test_a_refused_strings_part_writes_nothing(self):
        def refuse(built):
            raise c.Refused("missing", "ID_X")
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"a": 1})
            with self.assertRaises(c.Refused):
                c.write(Path(d), {"b": 2}, strings=refuse)
            self.assertEqual(json.loads((Path(d) / "career.json").read_text(encoding="utf-8")), {"version": 1, "a": 1})
            self.assertEqual([p.name for p in Path(d).iterdir()], ["career.json"])


class CareerFake(RowsDb):
    """Progression records and the challenge banks in one db, as export() reads them."""

    def __init__(self, fix, prog):
        super().__init__(fix)
        self.prog = prog
        self.names = list(prog.names) + list(fix.names)

    def collections(self, rel):
        return self.prog.records if rel == "progressionbanks/main" else super().collections(rel)

    def bin(self, rel):
        return self.prog.binary if rel == "progressionbanks/main" else super().bin(rel)


class ExportTest(Scripted):
    LANGUAGE = {"ID_CHALLENGE_A": "A", "ID_TITLE": "Reward", "ID_COMMON_MIKE": "Mike", "ID_UNUSED": "never named"}

    def test_export_writes_every_part_and_the_strings(self):
        with tempfile.TemporaryDirectory() as d:
            c.export(CareerFake(minimal_world(), bank()), Path(d), language=self.LANGUAGE)
            got = json.loads((Path(d) / "career.json").read_text(encoding="utf-8"))
            self.assertEqual(sorted(got), ["actions", "all_characters_key", "characters", "fe_locations", "groups", "indicators",
                                           "locators", "reward_sites", "rewards", "rows", "stategraphs", "version", "volumes",
                                           "worlds"])
            self.assertEqual(sorted(got["rows"]), ["challenge_a", "challenge_b"])
            self.assertEqual(got["fe_locations"]["gamestart"], {"world": "dist_one", "location": "game_locator"})
            # every world a row or an fe_location names is in `worlds` (engine map: the online map table; dist_two has none here)
            self.assertEqual(sorted(got["worlds"]), ["dist_one", "dist_two"])
            self.assertEqual([x["key"] for x in got["characters"]], ["mike", "reda"])
            strings = json.loads((Path(d) / "strings.json").read_text(encoding="utf-8"))
            self.assertEqual(strings, {"ID_CHALLENGE_A": "A", "ID_COMMON_MIKE": "Mike", "ID_TITLE": "Reward"})

    def test_an_anywhere_row_and_an_end_world_reach_worlds_but_an_empty_world_does_not(self):
        fix = minimal_world()
        fix.row("any_a", world=None, fields={c.KIND: int_(0x25, "Sk8::Challenge::eChallengeTypes")})
        fix.row("end_b", fields={c.END_TELEPORT: bool_(True),
                                 c.END_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": world_loc(fix.bin, "dist_three", "end_b_loc")}})
        with tempfile.TemporaryDirectory() as d:
            c.export(CareerFake(fix, bank()), Path(d))
            got = json.loads((Path(d) / "career.json").read_text(encoding="utf-8"))
            self.assertEqual(sorted(got["worlds"]), ["dist_one", "dist_three", "dist_two"])
            self.assertEqual(got["rows"]["any_a"]["world"], "")

    def test_a_world_reached_only_through_a_teleport_exit_is_in_worlds(self):
        fix = minimal_world()
        lb = Bin()
        fix.row("tele_x", fields={c.GLOBAL_TYPE: global_type(2)})
        fix.local_data("tele_x", "", lb, **{c.EXIT_LOCATION: {"type": "Sk8::Challenge::tWorldLocationID", "data": "00000000",
                                                              "array": {"items": [world_loc(lb, "dist_three", "exit_x")]}}})
        with tempfile.TemporaryDirectory() as d:
            c.export(CareerFake(fix, bank()), Path(d))
            got = json.loads((Path(d) / "career.json").read_text(encoding="utf-8"))
            self.assertEqual(got["rows"]["tele_x"]["exits"], [{"world": "dist_three", "location": "exit_x"}])
            self.assertNotIn("dist_three", {r["world"] for r in got["rows"].values()} | {v["world"] for v in got["fe_locations"].values()})
            self.assertIn("dist_three", got["worlds"])

    def test_export_without_a_language_writes_no_strings(self):
        with tempfile.TemporaryDirectory() as d:
            c.export(CareerFake(minimal_world(), bank()), Path(d))
            self.assertEqual(sorted(p.name for p in Path(d).iterdir()), ["career.json", "scripts"])

    def test_a_package_row_named_like_a_disc_row_is_a_key_collision(self):
        base = minimal_world()
        own = Fix(base.bin)
        own.row("challenge_a")
        d = DlcFake(base, own)
        with tempfile.TemporaryDirectory() as out:
            with self.assertRaises(c.Refused) as ctx:
                c.export(CareerFake(base, bank()), Path(out), dlcs=[d])
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("key collision", "challenge_a"))
            self.assertEqual(list(Path(out).iterdir()), [])

    def test_export_adds_a_package_s_rows_and_worlds(self):
        base = minimal_world()
        own = Fix(base.bin)
        own.row("pkg_a", world="dist_three")
        d = DlcFake(base, own)
        with tempfile.TemporaryDirectory() as out:
            c.export(CareerFake(base, bank()), Path(out), dlcs=[d])
            got = json.loads((Path(out) / "career.json").read_text(encoding="utf-8"))
            self.assertEqual(got["rows"]["pkg_a"]["pack"], "dlc_pack")
            self.assertIsNone(got["rows"]["challenge_a"]["pack"])
            self.assertEqual(got["worlds"]["dist_three"], None)


HEAD = "(self, message, vars, sharedvars, globalvars)"


def orphan():
    """A handler record no state lists (retail has one, h_70612941ADEDC0EE)."""
    return rec(c.HANDLER, hkey("handler_orphan"), "default", Name=text("stateenter"), **{c.BLOB: blob(4, 12)})


class HandlerScriptsTest(Scripted):
    def test_every_named_handler_record_is_a_chunk_the_abstract_default_is_not(self):
        chunks = c.handler_chunks(bank(extra=[orphan()]))
        self.assertEqual(sorted(chunks), sorted("h_%016X" % H(n) for n in ("handler_initial", "handler_complete", "handler_orphan")))
        self.assertTrue(all(len(ch) == 4 for ch in chunks.values()))

    def test_a_handler_whose_code_runs_past_the_bank_is_refused(self):
        with self.assertRaises(c.Refused) as ctx:
            c.handler_chunks(bank(handler_fields={c.BLOB: blob(4, 4096)}))
        self.assertEqual(ctx.exception.reason, "missing")

    def test_a_handler_that_fails_to_decompile_is_refused(self):
        with mock.patch.object(c.unluac_batch, "run", failing_run):
            with self.assertRaises(c.Refused) as ctx:
                c.handler_functions(c.handler_chunks(bank()))
        self.assertEqual(ctx.exception.reason, "decompile")
        self.assertTrue(ctx.exception.name.startswith("h_"))

    def test_a_failed_decompile_writes_nothing(self):
        with tempfile.TemporaryDirectory() as d, mock.patch.object(c.unluac_batch, "run", failing_run):
            with self.assertRaises(c.Refused) as ctx:
                c.export(CareerFake(minimal_world(), bank()), Path(d) / "out")
            self.assertEqual(ctx.exception.reason, "decompile")
            self.assertEqual(list((Path(d) / "out").iterdir()) if (Path(d) / "out").exists() else [], [])

    def test_unluac_missing_is_a_refusal_not_a_traceback(self):
        def no_unluac(chunks, java=None, unluac=None):
            raise KeyError("SKATE_UNLUAC")
        with mock.patch.object(c.unluac_batch, "run", no_unluac):
            with self.assertRaises(c.Refused) as ctx:
                c.handler_functions(c.handler_chunks(bank()))
        self.assertEqual(ctx.exception.reason, "decompile")
        self.assertIn("SKATE_UNLUAC", str(ctx.exception))

    def test_handlers_are_wrapped_with_five_arguments(self):
        fns = c.handler_functions(c.handler_chunks(bank(extra=[orphan()])))
        self.assertEqual(len(fns), 3)
        for name, fn in fns.items():
            self.assertEqual(fn, f'function {name}{HEAD}\n  Unlock.UnlockProgressionReward("rwd", false)\n  ChangeState("complete")\nend')
        text = c.progression_lua(fns)
        self.assertEqual(sorted(re.findall(r"^function (h_\w+)\(", text, re.M)), sorted(fns))
        self.assertEqual(text.count("function h_"), 3)
        self.assertNotIn("\r", text)

    def test_a_chunk_that_is_already_the_function_is_kept_as_it_is(self):
        name = "h_0123456789ABCDEF"
        lua = f"function {name}{HEAD}\n  local x = 1\n\n  ChangeState(x)\nend"
        self.assertEqual(c.handler_function(name, lua + "\n"), lua)

    def test_blank_lines_stay_blank_when_a_body_is_wrapped(self):
        self.assertEqual(c.handler_function("h_A", "a()\n\nb()\n"), f"function h_A{HEAD}\n  a()\n\n  b()\nend")

    def test_a_function_of_another_name_or_arguments_is_refused(self):
        for lua in ("function h_B%s\nend" % HEAD, "function h_A(self)\nend", "function other()\nend"):
            with self.assertRaises(c.Refused) as ctx:
                c.handler_function("h_A", lua)
            self.assertEqual(ctx.exception.reason, "decompile", lua)

    def test_nothing_printed_is_a_failure_even_with_exit_zero(self):
        with self.assertRaises(c.Refused) as ctx:
            c.handler_function("h_A", " \n")
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("decompile", "h_A"))

    def test_two_records_with_one_key_are_refused(self):
        twin = rec(c.HANDLER, "Hash_%016X" % H("handler_initial"), "default", Name=text("stateenter"), **{c.BLOB: blob(4, 8)})
        with self.assertRaises(c.Refused) as ctx:
            c.handler_chunks(bank(extra=[twin]))
        self.assertEqual(ctx.exception.reason, "key collision")


class GraphScriptsTest(unittest.TestCase):
    def test_each_graph_lands_under_scripts_graphs(self):
        with mock.patch.object(c, "graphs_lua", fake_graphs):
            got = c.graph_scripts(object())
        self.assertEqual(sorted(got), sorted(f"scripts/graphs/{g}.lua" for g in ("tutorials", "teleport", "objectives_shared", "photo_skate2")))
        self.assertEqual(got["scripts/graphs/teleport.lua"], fake_graphs(None, ["teleport"])["teleport"])

    def test_no_java_or_unluac_is_a_refusal(self):
        with mock.patch.object(c, "graphs_lua", lambda db, graphs: None):
            with self.assertRaises(c.Refused) as ctx:
                c.graph_scripts(object())
        self.assertEqual(ctx.exception.reason, "decompile")
        self.assertIn("SKATE_JAVA", str(ctx.exception))

    def test_a_graph_unluac_fails_on_is_a_refusal_naming_it(self):
        def dies(db, graphs):
            raise SystemExit("unluac failed on teleport")
        with mock.patch.object(c, "graphs_lua", dies):
            with self.assertRaises(c.Refused) as ctx:
                c.graph_scripts(object())
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("decompile", "teleport"))

    def test_a_graph_the_disc_lacks_is_a_refusal(self):
        def lacks(db, graphs):
            raise KeyError("challenge_stategraph/tutorials")
        with mock.patch.object(c, "graphs_lua", lacks):
            with self.assertRaises(c.Refused) as ctx:
                c.graph_scripts(object())
        self.assertEqual(ctx.exception.reason, "missing")

    def test_no_graphs_need_no_java(self):
        self.assertEqual(c.graph_scripts(object(), ()), {})


class RewardSitesTest(unittest.TestCase):
    FUNCTIONS = {
        "h_A": 'function h_A%s\n  Unlock.UnlockProgressionReward("cap", false)\n  Unlock.UnlockProgressionReward("cap", true)\nend' % HEAD,
        "h_B": "function h_B%s\n  Unlock.UnlockProgressionReward('deck', false)\n  ChangeState(\"x\")\nend" % HEAD,
        "h_O": 'function h_O%s\n  Unlock.UnlockProgressionReward("orphaned", false)\nend' % HEAD,
    }

    @staticmethod
    def graph(*handlers):
        return {"initial": "initial", "states": {f"s{i}": {"handlers": {"stateenter": h}, "events": []} for i, h in enumerate(handlers)}}

    def sites(self, rows, graphs, functions=None):
        return c.reward_sites(rows, {"stategraphs": graphs}, functions or self.FUNCTIONS)

    def test_rows_and_graphs_are_both_sites_and_each_is_listed_once_sorted(self):
        rows = {"r1": {"own": {"rewards": ["reda", "deck"]}, "kill": {"rewards": ["deck"]}},
                "r2": {"own": {"rewards": []}, "kill": {"rewards": []}}}
        got = self.sites(rows, {"g1": self.graph("h_A"), "g2": self.graph("h_B", "h_A"), "empty": {"initial": "", "states": {}}})
        self.assertEqual(got, {"cap": ["graph:g1", "graph:g2"], "deck": ["graph:g2", "row:r1"], "reda": ["row:r1"]})
        self.assertEqual(list(got), sorted(got))

    def test_a_handler_no_state_lists_grants_nothing(self):
        self.assertNotIn("orphaned", self.sites({}, {"g1": self.graph("h_A")}))

    def test_a_computed_reward_name_is_refused_rather_than_missed(self):
        fns = {"h_C": "function h_C%s\n  Unlock.UnlockProgressionReward(name, false)\nend" % HEAD}
        with self.assertRaises(c.Refused) as ctx:
            self.sites({}, {"g1": self.graph("h_C")}, fns)
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("decompile", "h_C"))

    def test_a_state_naming_a_handler_that_was_not_decompiled_is_refused(self):
        with self.assertRaises(c.Refused) as ctx:
            self.sites({}, {"g1": self.graph("h_Z")})
        self.assertEqual(ctx.exception.reason, "missing")


class WriteScriptsTest(unittest.TestCase):
    def test_scripts_are_written_whole_with_lf_beside_career_json(self):
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d) / "out", {"a": 1}, scripts={"scripts/progression.lua": "x\ny\n", "scripts/graphs/g.lua": "z\n"})
            out = Path(d) / "out"
            self.assertEqual((out / "scripts/progression.lua").read_bytes(), b"x\ny\n")
            self.assertEqual((out / "scripts/graphs/g.lua").read_bytes(), b"z\n")
            self.assertEqual(sorted(str(p.relative_to(out)).replace("\\", "/") for p in out.rglob("*") if p.is_file()),
                             ["career.json", "scripts/graphs/g.lua", "scripts/progression.lua"])

    def test_a_second_write_replaces_a_script(self):
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"a": 1}, scripts={"scripts/p.lua": "one"})
            c.write(Path(d), {"a": 1}, scripts={"scripts/p.lua": "two"})
            self.assertEqual((Path(d) / "scripts/p.lua").read_text(), "two")
            self.assertEqual([p.name for p in (Path(d) / "scripts").iterdir()], ["p.lua"])


class ScriptedExportTest(Scripted):
    def export(self, d, fix=None):
        return c.export(CareerFake(fix or minimal_world(), bank()), Path(d))

    def test_export_writes_progression_lua_the_graph_scripts_and_the_reward_sites(self):
        with tempfile.TemporaryDirectory() as d:
            summary = self.export(d)
            out = Path(d)
            lua = (out / "scripts/progression.lua").read_text(encoding="utf-8")
            self.assertEqual(len(re.findall(r"^function h_", lua, re.M)), 2)      # the fake bank's two states
            for g in ("tutorials", "teleport", "objectives_shared", "photo_skate2"):
                self.assertTrue((out / "scripts/graphs" / f"{g}.lua").is_file(), g)
            got = json.loads((out / "career.json").read_text(encoding="utf-8"))
            self.assertEqual(got["reward_sites"], {"rwd": ["graph:g1"], "rwd_one": ["row:challenge_b"]})
            self.assertEqual((summary["handlers"], summary["scripts"], summary["reward_sites"]), (2, 5, 2))
            for p in out.rglob("*"):
                if p.is_file():
                    self.assertNotIn(b"\r", p.read_bytes(), p.name)

    def test_a_row_s_own_and_kill_rewards_are_row_sites(self):
        fix = minimal_world()
        fix.row("row_x", fields={c.OWN: reward_of(fix.bin, 0, 0, ["rwd_two"]), c.KILL: reward_of(fix.bin, 0, 0, ["rwd_one"])})
        with tempfile.TemporaryDirectory() as d:
            self.export(d, fix)
            sites = json.loads((Path(d) / "career.json").read_text(encoding="utf-8"))["reward_sites"]
            self.assertEqual(sites["rwd_two"], ["row:row_x"])
            self.assertEqual(sites["rwd_one"], ["row:challenge_b", "row:row_x"])


# ---------------------------------------------------------------- locators, volumes, ribbons, the stage-1 key check (Task 5)
def matrix(x):
    return [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [x, 2.0, 3.0, 1.0]]


def eb0021(splines):
    """A hand-built RW4 arena holding one 0xEB0021 object. splines: [(instance id, flags, [(x, y, z), ...])]. Layout
    (stage1_traces 3.2): u32 spline count, total points, record offset 0x10, points offset; 0x40-byte records {u64 hash,
    u64 instance id, min xyz + pad, max xyz + pad, u32 flags, u32 point count, u32 point offset, u32 0}; 64-byte points
    {vec4 A, vec4 B, vec4 0, vec4 position (w = 1)}."""
    total = sum(len(p) for _, _, p in splines)
    records_at, points_at = 0x10, 0x10 + 0x40 * len(splines)
    obj = bytearray(struct.pack(">4I", len(splines), total, records_at, points_at)) + bytearray(0x40 * len(splines) + 64 * total)
    at = points_at
    for i, (iid, flags, points) in enumerate(splines):
        struct.pack_into(">QQ", obj, records_at + 0x40 * i, 0xABCD0000 + i, iid)
        struct.pack_into(">3f", obj, records_at + 0x40 * i + 16, *[min(p[a] for p in points) for a in range(3)])
        struct.pack_into(">3f", obj, records_at + 0x40 * i + 32, *[max(p[a] for p in points) for a in range(3)])
        struct.pack_into(">4I", obj, records_at + 0x40 * i + 48, flags, len(points), at, 0)
        for x, y, z in points:
            struct.pack_into(">16f", obj, at, 1, 0, 0, 0, -1.5, 0, 0, 0, 0, 0, 0, 0, x, y, z, 1)
            at += 64
    head = bytearray(0x60)
    head[:4] = b"\x89RW4"
    struct.pack_into(">I", head, 0x20, 1)
    struct.pack_into(">I", head, 0x30, 0x40)
    struct.pack_into(">6I", head, 0x40, 0x60, len(obj), 0, 0, 0, 0xEB0021)
    return bytes(head) + bytes(obj)


class Eb0021Test(unittest.TestCase):
    def test_an_eb0021_blob_decodes_to_spline_points(self):
        import skate3_online as o
        blob = eb0021([(0x2C7017070014000A, 2, [(1.5, -2.25, 3.0), (4.0, 5.5, -6.0)]), (0x2C7017070014000F, 8, [(0.5, 0.25, 0.125)])])
        locators, volumes, splines = {}, {}, {}
        o.rw4_placement(blob, locators, volumes, splines)
        self.assertEqual(splines, {0x2C7017070014000A: {"flags": 2, "points": [[1.5, -2.25, 3.0], [4.0, 5.5, -6.0]]},
                                   0x2C7017070014000F: {"flags": 8, "points": [[0.5, 0.25, 0.125]]}})
        self.assertEqual((locators, volumes), ({}, {}))

    def test_without_a_splines_dict_the_object_is_skipped_as_it_always_was(self):
        import skate3_online as o
        locators, volumes = {}, {}
        o.rw4_placement(eb0021([(7, 2, [(1.0, 2.0, 3.0)])]), locators, volumes)
        self.assertEqual((locators, volumes), ({}, {}))


class StageKeyTest(unittest.TestCase):
    def rows(self, *keys):
        return {name: {"key": key} for name, key in zip("abcd", keys)}

    def test_two_rows_whose_keys_round_alike_are_refused(self):
        # retail compares the keys as 32-bit floats (stage1_teleport 1.1, spec 18.27): 2**40 and 2**40 + 1 are one float
        self.assertEqual(c.activity_key(2 ** 40), c.activity_key(2 ** 40 + 1))
        with self.assertRaises(c.Refused) as ctx:
            c.check_activity_keys(self.rows(2 ** 40, 2 ** 41, 2 ** 40 + 1), stage=("a", "b", "c"))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("key collision", "c"))
        self.assertIn("a", str(ctx.exception))

    def test_keys_that_stay_apart_pass_and_a_row_outside_the_stage_is_not_compared(self):
        c.check_activity_keys(self.rows(2 ** 40, 2 ** 40 + 2 ** 20, 2 ** 40 + 1), stage=("a", "b"))
        c.check_activity_keys(self.rows(2 ** 40), stage=("a", "not_a_row"))      # a stage row the export lacks is not this check's
        self.assertEqual(c.activity_key(2 ** 63 + 2 ** 62), 2 ** 63 + 2 ** 62)   # a hash64-sized key
        self.assertEqual(c.activity_key(0x2C7017060025128F), struct.unpack(">f", struct.pack(">f", float(0x2C7017060025128F)))[0])

    def test_the_conversion_rounds_to_a_double_first_then_to_a_single(self):
        # 0x82F57EE0 converts the u64 to a double, frsp rounds that to a single: 2**60 + 2**36 + 1 is a hair above the
        # halfway point of a single, but the double already dropped the 1, so the tie breaks to even (2**60), not up
        key = 2 ** 60 + 2 ** 36 + 1
        self.assertEqual(float(key), 2.0 ** 60 + 2 ** 36)
        self.assertEqual(c.activity_key(key), 2.0 ** 60)


def mission(locators=None, volumes=None, splines=None):
    return (locators or {}, volumes or {}, splines or {})


class FakePlaces:
    """The three sources DiscPlaces reads: a mission per row, a world stream per engine map, the global locators."""

    def __init__(self, missions=None, worlds=None, globals_=None):
        self.missions, self.worlds, self.globals_ = missions or {}, worlds or {}, globals_ or {}

    def mission(self, row):
        return self.missions.get(row)

    def world(self, map_name):
        return self.worlds.get(map_name, ({}, {}))

    def globals(self):
        return self.globals_


def place_row(**kw):
    return {"world": "dist_one", "location": "", "start": None, "map_start": None, "end_teleport": None, "end_world": None,
            "exits": [], **kw}


def visual(bin_, kind, locator="", spline_id=0, type_key=0, style_key=0, param=0, name=""):
    """A tVisualEditorData item (80 bytes, stage1_traces 3.1)."""
    return (struct.pack(">IIII", kind, param, bin_.s(name) if name else 0, 0) + struct.pack(">QI4x", spline_id, bin_.s(locator) if locator else 0)
            + struct.pack(">QQQQQQ", 0x3297556BECB605E2, type_key, 0, 0xF0DB8767D611AF94, style_key, 0)).hex().upper()


def visuals(*items):
    return {"type": "Sk8::Challenge::tVisualEditorData", "data": "00000000",
            "array": {"capacity": len(items), "element_size": 80, "alignment": 8, "items": list(items)}}


def volume_field(bin_, text, instance):
    return {"type": "Sk8::Challenge::tTriggerVolumeInstanceID", "data": "%08X%08X%016X" % (bin_.s(text), 0, instance)}


class StagePlacesTest(unittest.TestCase):
    WORLDS = {"dist_one": "maps/MapOne.skate", "dist_two": "maps/MapTwo.skate", "dist_nomap": None}

    def fix(self):
        fix = Fix()
        fix.world("dist_one").world("dist_two")
        return fix

    def places(self, fix, rows, places, stage=None, prog=None, fe=None, worlds=None):
        return c.stage_places(fix.db(), rows, prog or {"stategraphs": {}}, fe or {}, worlds or self.WORLDS, places, stage=stage or tuple(rows))

    def test_an_unresolved_locator_is_refused(self):
        fix = self.fix()
        with self.assertRaises(c.Refused) as ctx:
            self.places(fix, {"lesson": place_row(location="lesson_loc")}, FakePlaces(missions={"lesson": mission()}))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("locator", "lesson_loc"))
        self.assertIn("lesson", str(ctx.exception))

    def test_every_name_a_row_gives_is_looked_up_and_each_kind_can_refuse(self):
        fix = self.fix()
        have = {"a_loc": {"matrix": matrix(1.0), "slots": []}}
        for field, extra in (("start", {"start": "x_start"}), ("map_start", {"map_start": "x_map"}),
                             ("end_teleport", {"end_teleport": "x_end", "end_world": "dist_two"}),
                             ("exit", {"exits": [{"world": "dist_two", "location": "x_exit"}]})):
            with self.assertRaises(c.Refused, msg=field) as ctx:
                self.places(fix, {"lesson": place_row(location="a_loc", **extra)}, FakePlaces(missions={"lesson": mission(have)}))
            self.assertEqual(ctx.exception.reason, "locator", field)

    def test_a_row_with_no_world_map_is_refused(self):
        with self.assertRaises(c.Refused) as ctx:
            self.places(self.fix(), {"lesson": place_row(world="dist_nomap", location="a_loc")}, FakePlaces(missions={"lesson": mission()}))
        self.assertEqual(ctx.exception.reason, "locator")

    def test_a_mission_s_locators_and_volumes_come_out_whole_on_the_rows_map(self):
        have = {"a_loc": {"matrix": matrix(0.1), "slots": [matrix(5.0)]}, "b_loc": {"matrix": matrix(2.0), "slots": []}}
        vols = {"gate_01": {"min": [1.0, 2.0, 3.0], "max": [4.0, 5.0, 6.0], "id": 77}}
        got = self.places(self.fix(), {"lesson": place_row(location="a_loc")}, FakePlaces(missions={"lesson": mission(have, vols)}))
        locators, volumes, indicators = got
        self.assertEqual(sorted(locators), ["a_loc", "b_loc"])      # b_loc is named by nothing; a mission's placements are all kept
        self.assertEqual(locators["a_loc"], {"map": "maps/MapOne.skate", "matrix": matrix(0.1), "slots": [matrix(5.0)]})
        self.assertEqual(volumes, {"gate_01": {"map": "maps/MapOne.skate", "min": [1.0, 2.0, 3.0], "max": [4.0, 5.0, 6.0], "id": 77}})
        self.assertEqual(indicators, {"lesson": []})

    def test_floats_come_out_as_the_shortest_text_of_their_single(self):
        self.assertEqual(c.single(struct.unpack(">f", struct.pack(">f", 0.42))[0]), 0.42)
        self.assertEqual(c.single(-1234.5), -1234.5)
        self.assertEqual(c.single(0.0), 0.0)
        have = {"a_loc": {"matrix": matrix(struct.unpack(">f", struct.pack(">f", 0.41))[0]), "slots": []}}
        got = self.places(self.fix(), {"lesson": place_row(location="a_loc")}, FakePlaces(missions={"lesson": mission(have)}))
        self.assertEqual(got[0]["a_loc"]["matrix"][3][0], 0.41)

    def test_a_locator_in_no_mission_comes_from_the_world_stream_then_the_global_locators_of_its_map(self):
        stream = ({"hub": {"matrix": matrix(8.0), "slots": []}}, {"hub_vol": {"min": [0, 0, 0], "max": [1, 1, 1], "id": 5}})
        glob = {"g_loc": [("MapTwo", {"matrix": matrix(1.0), "slots": [matrix(2.0)]}), ("MapOne", {"matrix": matrix(3.0), "slots": []})],
                "only_two": [("MapTwo", {"matrix": matrix(9.0), "slots": []})]}
        rows = {"lesson": place_row(location="hub", start="g_loc", map_start="G_LOC")}
        got = self.places(self.fix(), rows, FakePlaces(missions={"lesson": mission()}, worlds={"MapOne": stream}, globals_=glob))
        self.assertEqual(got[0]["hub"]["matrix"], matrix(8.0))
        self.assertEqual(got[0]["g_loc"], {"map": "maps/MapOne.skate", "matrix": matrix(3.0), "slots": []})   # the global one of its own map
        self.assertEqual(got[0]["G_LOC"], got[0]["g_loc"])        # the name as the row spells it (global names are lower case)
        self.assertNotIn("hub_vol", got[1])                       # a world stream is kept whole only where WHOLE_WORLDS says
        # the same name in another map's file is not this map's
        with self.assertRaises(c.Refused) as ctx:
            self.places(self.fix(), {"lesson": place_row(location="only_two")}, FakePlaces(missions={"lesson": mission()}, globals_=glob))
        self.assertEqual(ctx.exception.reason, "locator")

    def test_a_world_stream_in_whole_worlds_is_kept_whole(self):
        stream = ({"hub": {"matrix": matrix(8.0), "slots": []}}, {"hub_vol": {"min": [0, 0, 0], "max": [1, 1, 1], "id": 5}})
        with mock.patch.object(c, "WHOLE_WORLDS", ("dist_one",)):
            got = self.places(self.fix(), {"lesson": place_row(location="hub")}, FakePlaces(missions={"lesson": mission()}, worlds={"MapOne": stream}))
        self.assertEqual(got[1]["hub_vol"], {"map": "maps/MapOne.skate", "min": [0, 0, 0], "max": [1, 1, 1], "id": 5})

    def test_the_same_name_placed_differently_by_two_missions_is_a_key_collision(self):
        one = mission({"shared": {"matrix": matrix(1.0), "slots": []}})
        two = mission({"shared": {"matrix": matrix(2.0), "slots": []}})
        with self.assertRaises(c.Refused) as ctx:
            self.places(self.fix(), {"r1": place_row(), "r2": place_row()}, FakePlaces(missions={"r1": one, "r2": two}))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("key collision", "shared"))
        self.places(self.fix(), {"r1": place_row(), "r2": place_row()}, FakePlaces(missions={"r1": one, "r2": one}))

    def test_a_local_volume_the_row_names_is_looked_up_in_its_map_and_its_id_must_match(self):
        fix, lb = self.fix(), Bin()
        fix.local_data("lesson", "", lb, **{"Hash_DA9330F647B19DF2": volume_field(lb, "DIST_One|lesson_volume_01|0x2a", 0x2A)})
        vols = {"lesson_volume_01": {"min": [0, 0, 0], "max": [1, 1, 1], "id": 0x2A}}
        got = self.places(fix, {"lesson": place_row()}, FakePlaces(missions={"lesson": mission(volumes=vols)}))
        self.assertIn("lesson_volume_01", got[1])
        with self.assertRaises(c.Refused) as ctx:
            self.places(fix, {"lesson": place_row()}, FakePlaces(missions={"lesson": mission(volumes={})}))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("locator", "lesson_volume_01"))
        wrong = {"lesson_volume_01": {"min": [0, 0, 0], "max": [1, 1, 1], "id": 0x2B}}
        with self.assertRaises(c.Refused) as ctx:
            self.places(fix, {"lesson": place_row()}, FakePlaces(missions={"lesson": mission(volumes=wrong)}))
        self.assertEqual(ctx.exception.reason, "locator")

    def test_the_fe_locations_and_the_progression_teleport_events_name_places_too(self):
        glob = {"school_loc": [("MapTwo", {"matrix": matrix(1.0), "slots": []})], "ev_loc": [("MapOne", {"matrix": matrix(2.0), "slots": []})]}
        fe = {"schoolstart": {"world": "dist_two", "location": "school_loc"}}
        prog = {"stategraphs": {"g": {"initial": "s", "states": {"s": {"handlers": {}, "events": [
            {"kind": 8, "args": ["ev_loc", "dist_one"], "flags": 0}, {"kind": 5, "args": ["not_a_place", ""], "flags": 0}]}}}}}
        got = self.places(self.fix(), {}, FakePlaces(globals_=glob), fe=fe, prog=prog)
        self.assertEqual(sorted(got[0]), ["ev_loc", "school_loc"])
        bad = {"stategraphs": {"g": {"initial": "s", "states": {"s": {"handlers": {}, "events": [
            {"kind": 8, "args": ["nowhere", "dist_one"], "flags": 0}]}}}}}
        with self.assertRaises(c.Refused) as ctx:
            self.places(self.fix(), {}, FakePlaces(globals_=glob), prog=bad)
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("locator", "nowhere"))

    def test_the_one_event_locator_retail_leaves_unplaced_passes_and_no_other_does(self):
        (name, world), = c.UNPLACED_EVENTS
        worlds = {**self.WORLDS, world: "maps/MapOne.skate"}
        prog = {"stategraphs": {"g": {"initial": "s", "states": {"s": {"handlers": {}, "events": [
            {"kind": 8, "args": [name, world], "flags": 0}]}}}}}
        got = self.places(self.fix(), {}, FakePlaces(), prog=prog, worlds=worlds)
        self.assertEqual(got, ({}, {}, {}))
        prog["stategraphs"]["g"]["states"]["s"]["events"][0]["args"][1] = "dist_one"     # the same name in another world is not listed
        with self.assertRaises(c.Refused):
            self.places(self.fix(), {}, FakePlaces(), prog=prog, worlds=worlds)


class IndicatorsTest(unittest.TestCase):
    WORLDS = {"dist_one": "maps/MapOne.skate"}

    def setUp(self):
        self.fix = Fix()
        self.fix.world("dist_one")
        self.sp_id = 0x2C7017070014000A

    def objective(self, key, *items, parent="lesson"):
        self.fix.chal.append(rec(c.OBJECTIVE, key, parent, **({c.VISUAL: visuals(*items)} if items else {})))

    def go(self, places, rows=None, stage=("lesson",), **kw):
        rows = rows or {"lesson": place_row()}
        return c.stage_places(self.fix.db(), rows, {"stategraphs": {}}, {}, self.WORLDS, places, stage=stage, **kw)

    def test_a_kind_1_item_takes_the_points_and_flags_of_the_spline_its_id_names(self):
        b = self.fix.bin
        self.objective("lesson_1", visual(b, 1, spline_id=self.sp_id, name="lesson_gate_vi_01"))
        spl = {self.sp_id: {"flags": 2, "points": [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]}}
        got = self.go(FakePlaces(missions={"lesson": mission(splines=spl)}))
        self.assertEqual(got[2], {"lesson": [{"objective": "lesson_1", "kind": 1, "locator": None, "type": "", "style": "",
                                              "spline": [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]], "flags": [2]}]})

    def test_a_kind_1_item_whose_spline_the_stream_lacks_is_refused(self):
        self.objective("lesson_1", visual(self.fix.bin, 1, spline_id=self.sp_id, name="lesson_gate_vi_01"))
        with self.assertRaises(c.Refused) as ctx:
            self.go(FakePlaces(missions={"lesson": mission(splines={999: {"flags": 2, "points": [[0, 0, 0]]}})}))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("locator", "lesson_gate_vi_01"))

    def test_a_kind_0_item_names_its_locator_type_and_style_and_the_locator_must_resolve(self):
        b = self.fix.bin
        arrow, default = c.hash64("arrow_nonfade"), c.hash64("default")
        self.objective("lesson_0", visual(b, 0, locator="vi_01", type_key=arrow, style_key=default))
        have = {"vi_01": {"matrix": matrix(1.0), "slots": []}}
        got = self.go(FakePlaces(missions={"lesson": mission(have)}))
        self.assertEqual(got[2]["lesson"], [{"objective": "lesson_0", "kind": 0, "locator": "vi_01", "type": "arrow_nonfade",
                                             "style": "default", "spline": [], "flags": []}])

    def test_an_unlisted_unplaced_objective_locator_is_refused(self):
        self.objective("lesson_0", visual(self.fix.bin, 0, locator="vi_01"))
        with self.assertRaises(c.Refused) as ctx:
            self.go(FakePlaces(missions={"lesson": mission()}))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("locator", "vi_01"))
        # a listed objective does not make another objective's, or another locator of its own, pass
        with mock.patch.object(c, "UNPLACED_INDICATORS", {("lesson", "lesson_1", "vi_01"): "x", ("lesson", "lesson_0", "vi_02"): "x"}):
            with self.assertRaises(c.Refused):
                self.go(FakePlaces(missions={"lesson": mission()}))

    def test_a_listed_unplaced_objective_locator_passes_and_is_null(self):
        self.objective("lesson_0", visual(self.fix.bin, 0, locator="vi_01"), visual(self.fix.bin, 0, locator="vi_02"))
        have = {"vi_02": {"matrix": matrix(1.0), "slots": []}}
        with mock.patch.object(c, "UNPLACED_INDICATORS", {("lesson", "lesson_0", "vi_01"): "evidence"}):
            got = self.go(FakePlaces(missions={"lesson": mission(have)}))
        self.assertEqual([(i["locator"], i["kind"]) for i in got[2]["lesson"]], [(None, 0), ("vi_02", 0)])
        self.assertEqual(sorted(got[0]), ["vi_02"])
        # a listed locator some file does place is an ordinary one
        with mock.patch.object(c, "UNPLACED_INDICATORS", {("lesson", "lesson_0", "vi_01"): "evidence"}):
            got = self.go(FakePlaces(missions={"lesson": mission({**have, "vi_01": {"matrix": matrix(2.0), "slots": []}})}))
        self.assertEqual([i["locator"] for i in got[2]["lesson"]], ["vi_01", "vi_02"])

    def test_the_rows_own_indicator_locator_must_resolve_though(self):
        lb = Bin()
        self.fix.local_data("lesson", "", lb, **{c.VISUAL: visuals(visual(lb, 0, locator="vi_09"))})
        with self.assertRaises(c.Refused) as ctx:
            self.go(FakePlaces(missions={"lesson": mission()}))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("locator", "vi_09"))

    def test_a_type_key_the_name_table_lacks_is_spelled_by_its_hash(self):
        self.objective("lesson_0", visual(self.fix.bin, 0, locator="vi_01", type_key=0x1122334455667788, style_key=0))
        have = {"vi_01": {"matrix": matrix(1.0), "slots": []}}
        item = self.go(FakePlaces(missions={"lesson": mission(have)}))[2]["lesson"][0]
        self.assertEqual((item["type"], item["style"]), ("Hash_1122334455667788", ""))

    def test_the_ribbon_names_hash_to_the_keys_the_items_carry(self):
        # the type keys of tut_sksc_01_0 and ftl_univ_02_0 and the style key every stage-1 item has (stage1_traces 3.1, 3.3)
        self.assertEqual({n: "%016X" % c.hash64(n) for n in c.RIBBON_NAMES},
                         {"left_chevron": "F4F7728CEA0DC79B", "arrow_nonfade": "03E671FA364B1D9F", "default": "D7EDBD362D7D2152"})

    def test_a_ribbon_parameter_the_export_would_drop_is_refused(self):
        self.objective("lesson_1", visual(self.fix.bin, 1, spline_id=self.sp_id, param=3))
        spl = {self.sp_id: {"flags": 2, "points": [[0, 0, 0]]}}
        with self.assertRaises(c.Refused) as ctx:
            self.go(FakePlaces(missions={"lesson": mission(splines=spl)}))
        self.assertEqual(ctx.exception.reason, "missing")

    def test_a_kind_above_1_is_refused_because_retail_does_nothing_with_it(self):
        self.objective("lesson_1", visual(self.fix.bin, 2))
        with self.assertRaises(c.Refused) as ctx:
            self.go(FakePlaces(missions={"lesson": mission()}))
        self.assertEqual(ctx.exception.reason, "missing")

    def test_objectives_are_the_records_whose_parent_is_the_row_in_number_order_the_row_s_own_record_is_not(self):
        b = self.fix.bin
        have = {"vi_01": {"matrix": matrix(1.0), "slots": []}}
        for n in (10, 2, 33):
            self.objective(f"lesson_{n}", visual(b, 0, locator="vi_01"))
        self.objective("lesson", visual(b, 0, locator="vi_01"), parent="tutorial")       # the template the objectives inherit from
        self.objective("other_1", visual(b, 0, locator="vi_01"), parent="other")
        self.objective("lesson_5")                                                      # no VisualIndicators
        got = self.go(FakePlaces(missions={"lesson": mission(have)}))
        self.assertEqual([i["objective"] for i in got[2]["lesson"]], ["lesson_2", "lesson_10", "lesson_33"])

    def test_the_rows_own_visual_indicators_come_first_under_an_empty_objective(self):
        lb = Bin()
        self.fix.local_data("lesson", "", lb, **{c.VISUAL: visuals(visual(lb, 0, locator="vi_09"))})
        self.objective("lesson_1", visual(self.fix.bin, 0, locator="vi_01"))
        have = {"vi_01": {"matrix": matrix(1.0), "slots": []}, "vi_09": {"matrix": matrix(9.0), "slots": []}}
        got = self.go(FakePlaces(missions={"lesson": mission(have)}))
        self.assertEqual([(i["objective"], i["locator"]) for i in got[2]["lesson"]], [("", "vi_09"), ("lesson_1", "vi_01")])


class PlacementExportTest(Scripted):
    MAPS = {"dist_one": "MapOne", "dist_two": "MapTwo"}

    def test_export_writes_locators_volumes_and_indicators_from_the_places_it_is_given(self):
        fix = minimal_world()
        fix.row("lesson", world="dist_one")
        have = {"lesson_loc": {"matrix": matrix(1.0), "slots": []}, "game_locator": {"matrix": matrix(3.0), "slots": []}}
        places = FakePlaces(missions={"lesson": mission(have, {"v": {"min": [0, 0, 0], "max": [1, 1, 1], "id": 1}})},
                            globals_={"school_locator": [("MapTwo", {"matrix": matrix(2.0), "slots": []})]})
        with tempfile.TemporaryDirectory() as d, mock.patch.object(c, "STAGE_ONE", ("lesson",)), mock.patch.dict(c.WORLD_MAPS, self.MAPS):
            c.export(CareerFake(fix, bank()), Path(d), places=places)
            got = json.loads((Path(d) / "career.json").read_text(encoding="utf-8"))
        self.assertEqual(sorted(got["locators"]), ["game_locator", "lesson_loc", "school_locator"])
        self.assertEqual(got["locators"]["lesson_loc"]["map"], "maps/MapOne.skate")
        self.assertEqual(got["locators"]["school_locator"]["map"], "maps/MapTwo.skate")
        self.assertEqual(sorted(got["volumes"]), ["v"])
        self.assertEqual(got["indicators"], {"lesson": []})

    def test_export_refuses_two_stage_rows_whose_keys_round_alike_and_writes_nothing(self):
        fix = minimal_world()
        fix.row("lesson", world="dist_one")
        fix.row("lesson_b", world="dist_one")
        with tempfile.TemporaryDirectory() as d, mock.patch.object(c, "STAGE_ONE", ("lesson", "lesson_b")), \
                mock.patch.object(c, "activity_key", lambda key: 1.0):
            with self.assertRaises(c.Refused) as ctx:
                c.export(CareerFake(fix, bank()), Path(d), places=FakePlaces())
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("key collision", "lesson_b"))
            self.assertEqual(list(Path(d).iterdir()), [])

    def test_a_db_with_no_disc_and_no_places_has_nothing_to_place(self):
        with tempfile.TemporaryDirectory() as d:
            c.export(CareerFake(minimal_world(), bank()), Path(d))
            got = json.loads((Path(d) / "career.json").read_text(encoding="utf-8"))
        self.assertEqual((got["locators"], got["volumes"], got["indicators"]), ({}, {}, {}))


# ---------------------------------------------------------------- Task 6: objectives, hints, failure records, presentation, speech
def definition(bin_, source):
    """tObjectiveDefinition: 5 x u32 {source offset, 0, 0x10, HAL text offset, parameter offset} (skate3_online.Objectives.definitions)."""
    return {"type": "Sk8::Challenge::tObjectiveDefinition", "data": "%08X%08X%08X%08X%08X" % (bin_.s(source), 0, 16, 16, 16)}


def speech(character, ident):
    return {"type": "Sk8::Challenge::tSpeechInfo", "data": "%08X%04X0000" % (character, ident)}


def character(n):
    return {"type": "Sk8::Audio::eSk8Characters", "data": "%08X" % n}


def objective_refs(keys, cls=None):
    return ref_array(int((cls or c.OBJECTIVE)[5:], 16), [H(k) for k in keys])


def flow(kind, count):
    return {"type": "Sk8::Challenge::tObjectiveFlow", "data": "%08X%08X" % (kind, count)}


class RowFilesTest(Scripted):
    """Task 6: rows/<row>.json from synthetic challenge banks (stage1_traces 4.1-4.7, stage1_school 2)."""

    MAPS = {"dist_one": "MapOne", "dist_two": "MapTwo"}

    def setUp(self):
        super().setUp()
        self.reset()
        self.rows = {"lesson": {"title": "ID_LESSON", "kind": 0}}

    def reset(self):
        self.fix = minimal_world()
        self.b = self.fix.bin

    def evs(self, *items):
        """A tChallengePresentationEvent list: items are (kind, a, b, flags) with strings put in the bank's .bin."""
        return events(*[event(k, self.b.s(a) if a else 0, self.b.s(b) if b else 0, f) for k, a, b, f in items])

    def objective(self, key, source="return true", parent="lesson", **fields):
        self.fix.chal.append(rec(c.OBJECTIVE, key, parent, **{c.OBJECTIVE_DEFINITION: definition(self.b, source), **fields}))

    def failure(self, key, source, *evs):
        fields = {c.OBJECTIVE_DEFINITION: definition(self.b, source)}
        if evs:
            fields[c.FAILURE_EVENTS] = self.evs(*evs)
        self.fix.chal.append(rec(c.FAILURE_OBJECTIVE, key, "", **fields))

    def lesson(self, objectives=(), local=None, local_bin=None, **fields):
        base = {c.OBJECTIVES: objective_refs(objectives)} if objectives else {}
        self.fix.row("lesson", fields={**base, **fields})
        if local is not None:
            self.fix.local_data("lesson", "", local_bin or Bin(), **local)

    def files(self, locators=None, volumes=None, stage=("lesson",)):
        return c.row_files(self.fix.db(), self.rows, locators or {}, volumes or {}, stage=stage)

    def one(self, **kw):
        return self.files(**kw)[0]["lesson"]

    def test_events_decode_to_kind_args_and_flags_from_the_binary_they_are_given(self):
        b = Bin()
        got = c.events_of(events(event(20, b.s("tutorial_push"), b.s("ID_HINT"), 0), event(4, 0, 0, 3)), bytes(b.b), "owner")
        self.assertEqual(got, [{"kind": 20, "args": ["tutorial_push", "ID_HINT"], "flags": 0}, {"kind": 4, "args": ["", ""], "flags": 3}])
        self.assertEqual(c.events_of(None, bytes(b.b), "owner"), [])

    def test_the_field_hashes_are_the_names_the_research_gives(self):
        self.assertEqual({n: "Hash_%016X" % c.hash64(n) for n in ("Objectives", "ObjectivesOwned", "ObjectivesKilledIt", "ObjectiveFlow",
                                                                  "FailureObjectives", "ObjectiveDefinition", "ActivationMode")},
                         {"Objectives": c.OBJECTIVES, "ObjectivesOwned": c.OBJECTIVES_OWNED, "ObjectivesKilledIt": c.OBJECTIVES_KILLED_IT,
                          "ObjectiveFlow": c.FLOW, "FailureObjectives": c.INLINE_FAILURES, "ObjectiveDefinition": c.OBJECTIVE_DEFINITION,
                          "ActivationMode": c.ACTIVATION})
        # the lists whose names no table holds (stage1_school 2, stage1_traces 4.2): the hints and the failure records
        self.assertEqual((c.HINTS, c.FAILURES), ("Hash_ECAEB8E619A68AF3", "Hash_B69AF085F7A6D275"))
        self.assertEqual((c.OBJ_INTRO, c.OBJ_COMPLETE, c.OBJ_FAIL, c.FAILURE_EVENTS),
                         ("Hash_E73C9CF0F75A6FCD", "Hash_D404BA3319E2C389", "Hash_607EC72F932997C1", "Hash_BD96B775B855E129"))

    def test_an_objective_carries_its_activation_flags_and_compiled_snippet(self):
        self.objective("lesson_1", "return vars.x", **{c.ACTIVATION: int_(1, "Sk8::Challenge::eTriggerActivationMode"), c.MASTER: bool_(True)})
        self.lesson(["lesson_1"])
        o = self.one()["groups"]["Objectives"][0]["objectives"][0]
        self.assertEqual((o["key"], o["activation"], o["master"], o["fail_first"]), ("lesson_1", 1, True, False))
        self.assertEqual(o["obj"]["natives"], [])
        self.assertEqual(o["obj"]["tree"], c.snippet_tree("return vars.x")[0])
        self.assertEqual(o["hints"], [])
        self.assertEqual(o["failures"], [])

    def test_hints_come_in_list_order_with_their_own_events_in_the_same_order(self):
        self.failure("hint_b", "return false", (20, "tutorial_push", "ID_B", 0))
        self.failure("hint_a", "return vars.once", (4, "", "", 0), (3, "", "", 0))
        self.failure("hint_c", "")
        self.objective("lesson_1", **{c.HINTS: objective_refs(["hint_b", "hint_a", "hint_c"], c.FAILURE_OBJECTIVE)})
        self.lesson(["lesson_1"])
        o = self.one()["groups"]["Objectives"][0]["objectives"][0]
        self.assertEqual(o["hint_keys"], ["hint_b", "hint_a", "hint_c"])
        self.assertEqual([h and h["tree"] for h in o["hints"]], [c.snippet_tree("return false")[0], c.snippet_tree("return vars.once")[0], None])
        self.assertEqual(o["groups"]["hints"], [[{"kind": 20, "args": ["tutorial_push", "ID_B"], "flags": 0}],
                                                [{"kind": 4, "args": ["", ""], "flags": 0}, {"kind": 3, "args": ["", ""], "flags": 0}], []])
        self.assertEqual(o["failures"], [])       # a hint is not a failure record (stage1_traces 4.1)

    def test_the_failure_list_is_b69a_and_the_inline_list_is_kept_apart(self):
        self.failure("fail_a", "return vars.dead", (23, "ID_FAIL", "", 0))
        self.objective("lesson_1", **{c.FAILURES: objective_refs(["fail_a"], c.FAILURE_OBJECTIVE),
                                      c.INLINE_FAILURES: definition(self.b, "return false")})
        self.lesson(["lesson_1"])
        o = self.one()["groups"]["Objectives"][0]["objectives"][0]
        self.assertEqual(o["failure_keys"], ["fail_a"])
        self.assertEqual([f["tree"] for f in o["failures"]], [c.snippet_tree("return vars.dead")[0]])
        self.assertEqual(o["groups"]["failures"], [[{"kind": 23, "args": ["ID_FAIL", ""], "flags": 0}]])
        self.assertEqual([f["tree"] for f in o["inline_failures"]], [c.snippet_tree("return false")[0]])
        self.assertEqual(o["hints"], [])

    def test_fail_first_is_read_per_objective(self):
        self.objective("lesson_1", **{c.FAIL_FIRST: bool_(True)})
        self.objective("lesson_2")
        self.lesson(["lesson_1", "lesson_2"])
        got = [g["objectives"][0]["fail_first"] for g in self.one()["groups"]["Objectives"]]
        self.assertEqual(got, [True, False])

    def test_an_objective_s_intro_complete_and_fail_groups_are_event_lists(self):
        self.objective("lesson_1", **{c.OBJ_INTRO: self.evs((16, "", "", 0)), c.OBJ_COMPLETE: self.evs((4, "", "", 0), (65, "skateschool_gate", "", 0))})
        self.lesson(["lesson_1"])
        g = self.one()["groups"]["Objectives"][0]["objectives"][0]["groups"]
        self.assertEqual(g["intro"], [{"kind": 16, "args": ["", ""], "flags": 0}])
        self.assertEqual([e["kind"] for e in g["complete"]], [4, 65])
        self.assertEqual(g["fail"], [])
        self.assertEqual(sorted(g), ["complete", "fail", "failures", "hints", "intro"])

    def test_an_objective_inherits_its_parents_groups_and_flags(self):
        self.objective("lesson", parent="", **{c.OBJ_COMPLETE: self.evs((41, "", "", 0)), c.ACTIVATION: int_(1, "Sk8::Challenge::eTriggerActivationMode"),
                                               c.HINTS: objective_refs([], c.FAILURE_OBJECTIVE)})
        self.objective("lesson_1")
        self.lesson(["lesson_1"])
        o = self.one()["groups"]["Objectives"][0]["objectives"][0]
        self.assertEqual((o["activation"], [e["kind"] for e in o["groups"]["complete"]]), (1, [41]))

    def test_a_bare_objective_is_a_group_of_one_and_a_group_keeps_its_requirement(self):
        self.objective("lesson_1")
        self.objective("lesson_2")
        self.objective("lesson_3")
        self.fix.chal.append(rec(c.OBJECTIVE_GROUP, "lesson_g", "", **{c.OBJECTIVES: objective_refs(["lesson_2", "lesson_3"]),
                                                                       c.POINT_REQUIREMENT: int_(500)}))
        self.fix.row("lesson", fields={c.OBJECTIVES: ref_array(int(c.OBJECTIVE[5:], 16), [H("lesson_1")]),
                                       c.OBJECTIVES_OWNED: ref_array(int(c.OBJECTIVE_GROUP[5:], 16), [H("lesson_g")])})
        r = self.one()
        self.assertEqual([(g["PointRequirement"], [o["key"] for o in g["objectives"]]) for g in r["groups"]["Objectives"]], [(None, ["lesson_1"])])
        self.assertEqual([(g["PointRequirement"], [o["key"] for o in g["objectives"]]) for g in r["groups"]["ObjectivesOwned"]],
                         [(500, ["lesson_2", "lesson_3"])])
        self.assertEqual(r["groups"]["ObjectivesKilledIt"], [])
        self.assertEqual(r["objectives"], 1)             # the Lua's self.Objectives.Count: the Objectives list only (skate3_online mode_rules)

    def test_a_missing_hint_or_objective_record_refuses(self):
        self.objective("lesson_1", **{c.HINTS: objective_refs(["nowhere"], c.FAILURE_OBJECTIVE)})
        self.lesson(["lesson_1"])
        with self.assertRaises(c.Refused) as ctx:
            self.files()
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "lesson_1"))
        self.reset()
        self.lesson(["nowhere"])
        with self.assertRaises(c.Refused) as ctx:
            self.files()
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "lesson"))

    def test_a_snippet_the_port_cannot_read_refuses_and_names_the_objective(self):
        self.objective("lesson_1", "return foo:bar()")
        self.lesson(["lesson_1"])
        with self.assertRaises(c.Refused) as ctx:
            self.files()
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("decompile", "lesson_1"))

    def test_the_flow_and_the_objective_count_are_the_global_data_s(self):
        self.objective("lesson_1")
        self.objective("lesson_2")
        self.lesson(["lesson_1", "lesson_2"], **{c.FLOW: flow(2, 2)})
        r = self.one()
        self.assertEqual((r["flow"], r["objectives"], r["challenge_type"], r["title"]), ([2, 2], 2, 0, "ID_LESSON"))
        self.reset()
        self.lesson()
        r = self.one()
        self.assertEqual((r["flow"], r["objectives"], r["groups"]), (None, 0, {"Objectives": [], "ObjectivesOwned": [], "ObjectivesKilledIt": []}))

    def test_a_row_keeps_the_rules_json_row_shape_plus_its_extensions(self):
        self.lesson()
        self.assertTrue({"title", "online", "intro", "objectives", "challenge_type", "groups", "natives", "trigger_volumes", "trick_groups", "trick_group_members",
                         "flow", "presentation", "speech"} <= set(self.one()))
        self.assertIsInstance(json.dumps(self.one()), str)

    def test_trigger_volumes_are_looked_up_by_the_decimal_id_in_every_snippet_including_hints_and_failures(self):
        self.failure("hint_a", "return ObjectiveTrigger.IsInVolume('222', 'hintvolume_01')")
        self.failure("fail_a", "return ObjectiveTrigger.EnteredVolume('333', 'x')")
        self.objective("lesson_1", "return ObjectiveTrigger.IsInVolume('111', 'gate_01') and ObjectiveTrigger.IsInVolume('999', 'gone')",
                       **{c.HINTS: objective_refs(["hint_a"], c.FAILURE_OBJECTIVE), c.FAILURES: objective_refs(["fail_a"], c.FAILURE_OBJECTIVE)})
        self.lesson(["lesson_1"])
        vols = {"gate_01": {"map": "maps/M.skate", "min": [0, 0, 0], "max": [1, 1, 1], "id": 111},
                "hintvolume_01": {"map": "maps/M.skate", "min": [2, 2, 2], "max": [3, 3, 3], "id": 222},
                "other": {"map": "maps/M.skate", "min": [4, 4, 4], "max": [5, 5, 5], "id": 333}}
        with mock.patch.object(c, "UNPLACED_VOLUMES", {("lesson", "999"): "evidence"}):
            r = self.one(volumes=vols)
        self.assertEqual(sorted(r["trigger_volumes"]), ["111", "222", "333", "999"])
        self.assertEqual(r["trigger_volumes"]["111"], dict(vols["gate_01"], name="gate_01"))
        self.assertEqual(r["trigger_volumes"]["333"]["name"], "other")
        self.assertIsNone(r["trigger_volumes"]["999"])         # spec 18.20: a listed volume some files do not hold stays unplaced
        # an id no file places that the list does not name refuses, naming it; a listed id of another row does not pass either
        for listed in ({}, {("other_row", "999"): "evidence"}):
            with mock.patch.object(c, "UNPLACED_VOLUMES", listed):
                with self.assertRaises(c.Refused) as ctx:
                    self.one(volumes=vols)
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("locator", "999"))
            self.assertIn("lesson", str(ctx.exception))
        self.assertEqual(r["natives"], ["ObjectiveTrigger.EnteredVolume", "ObjectiveTrigger.IsInVolume"])

    def trick_group(self, name, *tricks):
        """A skatercollections trick group whose members are scorables the engine catalog names (crates/skate-core scoring/catalog.rs)."""
        members = ["%016X%016X" % (0, H(t)) for t in tricks]        # {class, key}: the scorable's key is the second half
        self.fix.sc.append(rec(c.TRICK_GROUP, hkey(name), "", **{c.TRICK_MEMBERS: {"type": "Attrib::RefSpec", "data": "00000000", "array": {
            "capacity": len(members), "element_size": 16, "alignment": 8, "items": members}}}))

    def test_a_local_record_s_same_file_parents_are_followed_all_the_way_to_the_framework(self):
        # stage1_school 3: tut_sksc_01 < skateschool (same file, @skateschool) < tutorial (framework). Three levels here: row < mid < top < framework.
        lb, fb = Bin(), Bin()

        def levs(bin_, *items):
            return events(*[event(k, bin_.s(a), 0, 0) for k, a in items])
        key = lambda n: "Hash_%016X" % H(n)       # noqa: E731
        self.fix.framework.append(rec(c.LOCAL, "tpl", "", **{key("AudioPlayerWonChallenge"): speech(0x28, 3), key("OutroPresentationEvents"): levs(fb, (5, "fw_outro")),
                                                           key("HostCharacter"): character(1)}))
        self.lesson()
        self.fix.local["lesson"] = [
            rec(c.LOCAL, "lesson", "mid", **{key("IntroPresentationEvents"): levs(lb, (5, "row_intro"))}),
            rec(c.LOCAL, "mid", "top", **{key("AudioPlayerQuitChallenge"): speech(0x1F, 5706), key("AudioCharacterOverride"): character(0x1F),
                                          key("RestartPresentationEvents"): levs(lb, (8, "mid_restart"))}),
            rec(c.LOCAL, "top", "tpl", **{key("HostCharacter"): character(0x1F), key("GotoPresentationEvents"): levs(lb, (4, "top_goto"))})]
        self.fix.local_bin["lesson"] = lb
        db = self.fix.db()
        db.bin = lambda rel, real=db.bin: bytes(fb.b) if rel == "challenge_local_data_framework" else real(rel)
        r = c.row_files(db, self.rows, {}, {}, stage=("lesson",))[0]["lesson"]
        self.assertEqual(r["speech"], {"calls": {"AudioPlayerQuitChallenge": [0x1F, 5706], "AudioPlayerWonChallenge": [0x28, 3]},
                                       "HostCharacter": 0x1F, "AudioCharacterOverride": 0x1F})      # the nearest record's value wins; every level adds
        # a same-file record's strings are in the row's .bin, the framework record's in the framework's
        self.assertEqual({k: v[0]["args"][0] for k, v in r["presentation"].items()},
                         {"Intro": "row_intro", "Restart": "mid_restart", "Outro": "fw_outro", "Goto": "top_goto"})

    def test_trick_groups_the_snippets_name_are_listed_with_their_members(self):
        self.objective("lesson_1", "return Tricks.DidSpecificTrickGroup('flips')")
        self.lesson(["lesson_1"])
        self.trick_group("flips", "ollie", "kickflip")
        r = self.one()
        self.assertEqual((r["trick_groups"], r["trick_group_members"]), (["flips"], {"flips": ["kickflip", "ollie"]}))
        self.reset()
        self.lesson()
        self.assertEqual((self.one()["trick_groups"], self.one()["trick_group_members"]), ([], {}))

    def test_a_trick_group_no_record_holds_refuses(self):
        self.objective("lesson_1", "return Tricks.DidSpecificTrickGroup('flips')")
        self.lesson(["lesson_1"])
        with self.assertRaises(c.Refused) as ctx:
            self.files()
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "lesson"))

    def test_the_presentation_groups_are_the_local_event_lists_by_name_and_the_intro_scene_is_the_nis(self):
        lb = Bin()

        def levs(*items):
            return events(*[event(k, lb.s(a) if a else 0, lb.s(b) if b else 0, f) for k, a, b, f in items])
        self.lesson(local={"Hash_%016X" % H("IntroPresentationEvents"): levs((5, "lesson_intro", "", 0), (49, "", "", 0)),
                           "Hash_%016X" % H("RestartPresentationEvents"): levs((8, "start_locator", "", 0)),
                           "Hash_%016X" % H("OutroPresentationEvents"): levs(),
                           "Hash_%016X" % H("GotoPresentationEvents"): levs((4, "", "", 0))}, local_bin=lb)
        r = self.one()
        self.assertEqual(sorted(r["presentation"]), ["Goto", "Intro", "Outro", "Restart"])
        self.assertEqual(r["presentation"]["Intro"], [{"kind": 5, "args": ["lesson_intro", ""], "flags": 0}, {"kind": 49, "args": ["", ""], "flags": 0}])
        self.assertEqual(r["presentation"]["Restart"][0]["args"], ["start_locator", ""])
        self.assertEqual(r["presentation"]["Outro"], [])
        self.assertEqual(r["intro"], "lesson_intro")

    def test_the_sign_up_group_is_the_global_data_s(self):
        self.lesson(**{c.SIGNUP_EVENTS: self.evs((3, "", "", 0))})
        r = self.one()
        self.assertEqual(r["presentation"], {"SignUp": [{"kind": 3, "args": ["", ""], "flags": 0}]})
        self.assertIsNone(r["intro"])

    def test_a_template_s_event_list_reads_the_framework_strings_not_the_rows(self):
        fb = Bin()
        item = event(5, fb.s("template_scene"), 0, 0)
        self.fix.framework.append(rec(c.LOCAL, "skateschool", "", **{"Hash_%016X" % H("IntroPresentationEvents"): events(item)}))
        self.lesson(local={})
        self.fix.local_data("lesson", "skateschool")
        db = self.fix.db()
        db.bin = lambda rel, real=db.bin: bytes(fb.b) if rel == "challenge_local_data_framework" else real(rel)
        r = c.row_files(db, self.rows, {}, {}, stage=("lesson",))[0]["lesson"]
        self.assertEqual(r["presentation"]["Intro"][0]["args"], ["template_scene", ""])

    def test_speech_lists_the_tspeechinfo_calls_by_name_and_the_two_characters(self):
        self.lesson(**{"Hash_%016X" % H("AudioPlayerDeclineChallenge"): speech(0x1F, 5706),
                       "Hash_%016X" % H("SignupSpeech"): speech(0x68, 0x1A76)},
                    local={"Hash_%016X" % H("HostCharacter"): character(0x68), "Hash_%016X" % H("AudioCharacterOverride"): character(0x1E),
                           "Hash_%016X" % H("AudioPlayerRestartChallenge"): speech(0x1E, 0x2179),
                           "Hash_0123456789ABCDEF": speech(0x28, 7)})
        s = self.one()["speech"]
        self.assertEqual(s["calls"], {"AudioPlayerDeclineChallenge": [0x1F, 5706], "SignupSpeech": [0x68, 0x1A76],
                                      "AudioPlayerRestartChallenge": [0x1E, 0x2179], "Hash_0123456789ABCDEF": [0x28, 7]})
        self.assertEqual((s["HostCharacter"], s["AudioCharacterOverride"]), (0x68, 0x1E))

    def test_a_row_without_the_characters_has_none(self):
        self.lesson()
        s = self.one()["speech"]
        self.assertEqual(s, {"calls": {}, "HostCharacter": None, "AudioCharacterOverride": None})

    def test_only_the_stage_rows_the_export_has_get_a_file_and_the_sources_come_with_them(self):
        self.objective("lesson_1", "return vars.x")
        self.lesson(["lesson_1"])
        files, sources = c.row_files(self.fix.db(), self.rows, {}, {}, stage=("lesson", "not_exported"))
        self.assertEqual(sorted(files), ["lesson"])
        self.assertEqual(list(sources.values()), ["return vars.x"])
        files, sources = c.row_files(self.fix.db(), self.rows, {}, {}, stage=())
        self.assertEqual((files, sources), ({}, {}))

    def places(self):
        have = {"lesson_loc": {"matrix": matrix(1.0), "slots": []}, "game_locator": {"matrix": matrix(3.0), "slots": []}}
        return FakePlaces(missions={"lesson": mission(have)}, globals_={"school_locator": [("MapTwo", {"matrix": matrix(2.0), "slots": []})]})

    def test_export_writes_each_stage_row_and_the_snippet_sources(self):
        self.objective("lesson_1", "return vars.x")
        self.lesson(["lesson_1"])
        with tempfile.TemporaryDirectory() as d, mock.patch.object(c, "STAGE_ONE", ("lesson",)), mock.patch.dict(c.WORLD_MAPS, self.MAPS):
            c.export(CareerFake(self.fix, bank()), Path(d), places=self.places())
            names = sorted(str(p.relative_to(d)).replace("\\", "/") for p in Path(d).rglob("*") if p.is_file())
            row = json.loads((Path(d) / "rows/lesson.json").read_text(encoding="utf-8"))
            src = json.loads((Path(d) / "scripts/objectives.json").read_text(encoding="utf-8"))
            raw = (Path(d) / "rows/lesson.json").read_bytes()
        self.assertIn("rows/lesson.json", names)
        self.assertIn("scripts/objectives.json", names)
        self.assertEqual(row["groups"]["Objectives"][0]["objectives"][0]["key"], "lesson_1")
        self.assertEqual(list(src.values()), ["return vars.x"])
        self.assertNotIn(b"\r", raw)

    def test_a_refusal_in_a_row_file_leaves_the_folder_empty(self):
        self.objective("lesson_1", "return foo:bar()")
        self.lesson(["lesson_1"])
        with tempfile.TemporaryDirectory() as d, mock.patch.object(c, "STAGE_ONE", ("lesson",)), mock.patch.dict(c.WORLD_MAPS, self.MAPS):
            with self.assertRaises(c.Refused) as ctx:
                c.export(CareerFake(self.fix, bank()), Path(d), places=self.places())
            self.assertEqual(ctx.exception.reason, "decompile")
            self.assertEqual(list(Path(d).iterdir()), [])


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class MainBankTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-career-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.out = c.progression(c.CareerDb(Path(os.environ["SKATE3_DISC"]), Path(cls.tmp.name)))

    def test_the_main_bank_counts(self):
        o = self.out
        self.assertEqual((len(o["actions"]), len(o["groups"]), len(o["stategraphs"]), len(o["rewards"])), (204, 53, 210, 222))
        starting = next(a for a in o["actions"] if a["name"] == "starting_action")
        self.assertIs(starting["auto"], True)
        self.assertEqual([a["name"] for a in o["actions"] if a["auto"]], ["starting_action"])
        kinds = {k for a in o["actions"] for k, _, _ in a["conditions"]}
        self.assertEqual(kinds, {0, 1, 2, 5})
        self.assertTrue(kinds <= set(range(6)))
        self.assertEqual(sum(len(a["conditions"]) for a in o["actions"]), 222)   # 108 + 79 + 33 + 2 (order 2.1)

    def test_every_name_a_condition_or_group_uses_is_a_word_not_a_hash(self):
        o = self.out
        words = [n for a in o["actions"] for _, n, _ in a["conditions"]] + [m for g in o["groups"].values() for m in g]
        self.assertFalse([w for w in words if w.startswith("Hash_")])
        self.assertTrue([w for w in o["rewards"] if w.startswith("Hash_")] == [])
        self.assertIn("reda", o["rewards"])

    def test_every_graph_has_an_initial_state_or_none_at_all(self):
        empty = sorted(g for g, v in self.out["stategraphs"].items() if not v["states"])
        self.assertEqual(len(empty), 29)    # 26 abstract graphs + the 3 retail graphs whose two states are not in the bank
        for g, v in self.out["stategraphs"].items():
            self.assertTrue(v["initial"] in v["states"] if v["states"] else v["initial"] == "", g)
        for g in ("1up_dwtn_05_stategraph", "tut_univ_01_stategraph", "monster_contest_complete"):
            self.assertIn(g, empty)

    def test_the_branding_level_ten_condition_is_the_cash_one(self):
        a = next(a for a in self.out["actions"] if a["name"] == "branding_level_10_complete")
        self.assertEqual(a["conditions"], [[5, "", 250000]])

    def test_handlers_are_245_distinct_functions_one_state_shared_by_two_graphs(self):
        # 246 handler records less `default` and `main` = 246, one of them (stateenter, key 70612941ADEDC0EE) belongs to no
        # state; the other 245 are each used once, except that otl_mmcp_01_activate and otl_first_stategraph list the same
        # `initial` state record, so 246 references reach 245 functions.
        used = {}
        for g, v in self.out["stategraphs"].items():
            for s, st in v["states"].items():
                for h in st["handlers"].values():
                    used.setdefault(h, []).append(g)
        self.assertEqual((sum(map(len, used.values())), len(used)), (246, 245))
        self.assertEqual({h: gs for h, gs in used.items() if len(gs) > 1},
                         {"h_9E65BE68B41F48A1": ["otl_mmcp_01_activate", "otl_first_stategraph"]})
        self.assertNotIn("h_70612941ADEDC0EE", used)

    def test_a_series_ender_counts_its_group(self):
        a = next(a for a in self.out["actions"] if a["name"] == "skateschool_unlock_intermediate")
        self.assertEqual(a["conditions"], [[2, "tut_sksc_04", 0], [2, "photo_anyw_11", 0]])


STAGE_ONE = ["tut_sksc_01", "tut_sksc_02", "tut_sksc_03", "tut_sksc_04", "tut_sksc_06", "tele_sksc_01", "tele_sksc_02",
             "tele_sksc_03", "tele_sksc_04", "tele_school_to_world", "ftl_univ_02", "photo_univ_08", "film_univ_05", "ftl_univ_03"]
LESSONS = ["tut_sksc_01", "tut_sksc_02", "tut_sksc_03", "tut_sksc_04", "tut_sksc_06"]
TELEPORTS = ["tele_sksc_01", "tele_sksc_02", "tele_sksc_03", "tele_sksc_04", "tele_school_to_world"]
UNIVERSITY = ["ftl_univ_02", "photo_univ_08", "film_univ_05", "ftl_univ_03"]
CLOSURE = Path(__file__).resolve().parent.parent / "docs/superpowers/specs/skate3-story-mode-reference/work/order/closure.json"


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class CareerDataTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-career-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.db = c.CareerDb(Path(os.environ["SKATE3_DISC"]), Path(cls.tmp.name))
        cls.prog = c.progression(cls.db)
        cls.rows = c.career_rows(cls.db, {m for g in cls.prog["groups"].values() for m in g})
        cls.fe = c.fe_locations(cls.db)
        cls.chars, cls.all_key = c.characters(cls.db)

    def test_stage_one_rows_are_exported(self):
        self.assertEqual([r for r in STAGE_ONE if r not in self.rows], [])
        self.assertEqual(len(STAGE_ONE), 14)

    def test_tele_world_to_school_is_disabled_and_the_stage_rows_are_not(self):
        self.assertIs(self.rows["tele_world_to_school"]["disabled"], True)
        self.assertEqual([r for r in STAGE_ONE if self.rows[r]["disabled"]], [])

    def test_ftl_univ_03_pays_reda_and_the_other_university_rows_pay_nothing(self):
        self.assertEqual(self.rows["ftl_univ_03"]["own"], {"cash": 0, "boards": 0, "rewards": ["reda"]})
        for r in ("ftl_univ_02", "photo_univ_08", "film_univ_05"):
            self.assertEqual(self.rows[r]["own"], {"cash": 0, "boards": 0, "rewards": []}, r)
        for r in UNIVERSITY:
            self.assertEqual(self.rows[r]["kill"], {"cash": 0, "boards": 0, "rewards": []}, r)
            self.assertIs(self.rows[r]["killable"], False, r)       # gates Q4: Owned is the only win

    def test_fe_locations_are_the_two_the_opening_uses(self):
        self.assertEqual(self.fe, {"schoolstart": {"world": "dist_skateschool", "location": "tele_world_to_school_dest_locator_01"},
                                   "gamestart": {"world": "dist_university", "location": "ftl_univ_02_challengelocator_01"}})

    def test_the_roster_is_41_characters_whose_unlocks_are_character_rewards_but_five(self):
        self.assertEqual(len(self.chars), 41)
        self.assertEqual(self.all_key, "all_characters_unlock")
        self.assertEqual(sorted({ch["category"] for ch in self.chars}), ["other", "pro"])
        self.assertEqual(sum(ch["category"] == "pro" for ch in self.chars), 32)
        self.assertEqual(len({ch["key"] for ch in self.chars}), 41)
        items = {r["item"] for r in self.prog["rewards"].values() if r["category"] == 1}
        # freeplay 3.3: shingo / seb / cuz come from the new-store reset, isaak from the deadspacetoo cheat, dr_pepper from nowhere found
        self.assertEqual(sorted(ch["key"] for ch in self.chars if ch["unlock"] not in items),
                         ["cuz", "dr_pepper", "isaak", "seb", "shingo"])
        by = {ch["key"]: ch for ch in self.chars}
        self.assertEqual((by["reda"]["recipe"], by["reda"]["category"]), ("giovanni_reda", "other"))
        self.assertEqual((by["steak"]["recipe"], by["isaak"]["recipe"]), ("security_03", "z_legend"))

    def test_lessons_and_university_rows_are_sign_up_rows_and_the_teleports_are_not(self):
        for r in LESSONS + UNIVERSITY:
            self.assertEqual(self.rows[r]["global"], 3, r)
        for r in TELEPORTS:
            self.assertEqual(self.rows[r]["global"], 2, r)

    def test_the_box_sizes_are_stored_as_the_data_has_them(self):
        # spec 6.2: 3.0 by default, tutorial 4.0, teleports 3.5 (never read: Global 2)
        self.assertEqual([self.rows[r]["signup_half"] for r in LESSONS], [4.0] * 5)
        self.assertEqual([self.rows[r]["signup_half"] for r in TELEPORTS], [3.5] * 5)
        self.assertEqual([self.rows[r]["signup_half"] for r in UNIVERSITY], [3.0] * 4)

    def test_the_stage_rows_carry_the_locators_graphs_and_indicators_the_stage_documents_give(self):
        want = {"tut_sksc_01": ("tutorials", "tutorial", "tut_sksc_01_challengelocator_01", "tut_sksc_01_startlocator_01", True),
                "ftl_univ_02": ("objectives_shared", "follow", "ftl_univ_02_challengelocator_01", "ftl_univ_02_startlocator_01", False),
                "photo_univ_08": ("photo_skate2", "spotphoto", "photo_univ_08_challengelocator_01", "photo_univ_08_startlocator_01", False),
                "film_univ_05": ("objectives_shared", "spotfilm", "film_univ_05_challengelocator_A", "film_univ_05_startlocator_01", False),
                "ftl_univ_03": ("objectives_shared", "follow", "ftl_univ_03_challengelocator_01", "ftl_univ_03_startlocator_01", False),
                "tele_sksc_01": ("teleport", "teleport", "tele_sksc_01_challengelocator_01", None, False)}
        for row, (graph, indicator, location, start, offboard) in want.items():
            r = self.rows[row]
            self.assertEqual((r["graph"], r["indicator"], r["location"], r["start"], r["offboard"]), (graph, indicator, location, start, offboard), row)
            self.assertEqual(r["map_start"], location if r["global"] == 3 else None, row)
        # stage1_univ 2.1: three rows send the player to their end locator, ftl_univ_03 hands over to the progression teleport
        self.assertEqual([self.rows[r]["end_teleport"] for r in UNIVERSITY], [
            "ftl_univ_02_end_locator_01", "photo_univ_08_end_locator_01", "film_univ_05_end_locator_01", None])
        self.assertEqual([self.rows[r]["end_world"] for r in UNIVERSITY], ["dist_university"] * 3 + [None])

    def test_the_teleports_name_their_exits(self):
        self.assertEqual(self.rows["tele_school_to_world"]["exits"],
                         [{"world": "dist_university", "location": "tele_school_to_world_dest_locator_01"}])
        self.assertEqual(self.rows["tele_world_to_school"]["exits"],
                         [{"world": "dist_skateschool", "location": "tele_world_to_school_dest_locator_01"}])
        self.assertEqual(self.rows["tele_world_to_prog_park"]["exits"][0]["world"], "dist_startpark_progression")
        self.assertTrue(all(not r["exits"] for k, r in self.rows.items() if r["kind"] != 45))

    def test_every_row_has_the_fields_the_game_reads_and_rewards_are_names(self):
        keys = {"key", "title", "description", "kind", "graph", "world", "location", "map_start", "start", "offboard", "indicator",
                "own", "kill", "online_bonus", "killable", "lot", "lot_rows", "disabled", "global", "signup_half", "end_teleport",
                "end_world", "pack", "exits"}
        self.assertTrue(all(set(r) == keys for r in self.rows.values()))
        self.assertEqual(len(self.rows), 312)
        self.assertTrue(all(r["pack"] is None for r in self.rows.values()))
        self.assertTrue(all(n in self.prog["rewards"] for r in self.rows.values() for s in (r["own"], r["kill"]) for n in s["rewards"]))
        self.assertTrue(all(r["graph"] for r in self.rows.values()))

    @unittest.skipUnless(CLOSURE.is_file(), "the order research's closure.json is not in the repo")
    def test_every_one_of_the_267_rows_retail_activates_is_exported(self):
        closure = set(json.loads(CLOSURE.read_text(encoding="utf-8"))["challenges"])
        self.assertEqual(len(closure), 267)
        self.assertEqual(sorted(closure - set(self.rows)), [])

    def test_the_lots_list_their_activities_and_the_activities_name_their_lot(self):
        lot = self.rows["otl_dwtn_01"]
        self.assertEqual(len(lot["lot_rows"]), 9)
        self.assertTrue(all(self.rows[a]["lot"] == "otl_dwtn_01" for a in lot["lot_rows"]))
        self.assertTrue(all(r["lot"] in self.rows for r in self.rows.values() if r["lot"]))

    def test_the_worlds_cover_every_world_a_row_an_fe_location_or_an_exit_names(self):
        named = {r["world"] for r in self.rows.values() if r["world"]} | {v["world"] for v in self.fe.values()}
        named |= {e["world"] for r in self.rows.values() for e in r["exits"]} | {r["end_world"] for r in self.rows.values() if r["end_world"]}
        worlds = c.world_maps(self.db, named)
        self.assertEqual(sorted(worlds), sorted(named))
        self.assertEqual(worlds["dist_skateschool"], "maps/SkateSchool.skate")
        self.assertEqual(worlds["dist_university"], "maps/University.skate")
        self.assertEqual(worlds["dist_startpark_progression"], "maps/StartPark.skate")    # a child of dist_startpark
        # the one world nothing names (street_ind_01, a cut row) has no engine map
        self.assertEqual(sorted(w for w, m in worlds.items() if m is None), ["Hash_EC763034105B866B"])

    @unittest.skipUnless(os.environ.get("SKATE3_ASSETS"), "set SKATE3_ASSETS to the install's assets folder (read only)")
    def test_the_stage_rows_have_their_strings(self):
        language = json.loads((Path(os.environ["SKATE3_ASSETS"]) / "private/challenge-map/runtime/challengemap.json").read_text(
            encoding="utf-8"))["language"]
        career = {"rows": {r: self.rows[r] for r in STAGE_ONE}, "rewards": {"reda": self.prog["rewards"]["reda"]}}
        table, missing = c.strings(career, language)
        self.assertEqual(missing, [])
        for r in STAGE_ONE:
            self.assertIn(self.rows[r]["title"], table, r)
        self.assertIn(self.prog["rewards"]["reda"]["title"], table)


def near(test, got, want, tol=0.006):
    test.assertEqual(len(got), len(want))
    for g, w in zip(got, want):
        test.assertAlmostEqual(g, w, delta=tol)


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class PlacementDataTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-career-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.disc = Path(os.environ["SKATE3_DISC"])
        cls.db = c.CareerDb(cls.disc, Path(cls.tmp.name))
        cls.prog = c.progression(cls.db)
        cls.rows = c.career_rows(cls.db, {m for g in cls.prog["groups"].values() for m in g})
        cls.fe = c.fe_locations(cls.db)
        cls.worlds = c.world_maps(cls.db, c.named_worlds(cls.rows, cls.fe, cls.prog))
        cls.locators, cls.volumes, cls.indicators = c.stage_places(cls.db, cls.rows, cls.prog, cls.fe, cls.worlds, c.DiscPlaces(cls.disc))

    def test_the_stage_one_rows_are_the_fourteen_plus_the_disabled_way_back(self):
        self.assertEqual(sorted(c.STAGE_ONE), sorted(STAGE_ONE + ["tele_world_to_school"]))
        self.assertEqual(sorted(self.indicators), sorted(c.STAGE_ONE))

    def test_the_stage_one_keys_stay_apart_as_32_bit_floats(self):
        c.check_activity_keys(self.rows)
        self.assertEqual(len({c.activity_key(self.rows[r]["key"]) for r in c.STAGE_ONE}), 15)

    def test_tut_sksc_01_has_23_matched_splines(self):
        ind = self.indicators["tut_sksc_01"]
        ribbons = [i for i in ind if i["kind"] == 1]
        self.assertEqual(len(ribbons), 23)
        self.assertTrue(all(i["spline"] and i["locator"] is None for i in ribbons))
        self.assertEqual(sorted(tuple(i["flags"]) for i in ribbons), [(2,)] * 22 + [(8,)])    # stage1_traces 3.3: 22 gates open, the finish closed
        by = {i["objective"]: i for i in ribbons}
        self.assertEqual(len(by), 23)
        gate, end = by["tut_sksc_01_1"], by["tut_sksc_01_33"]
        self.assertEqual((len(gate["spline"]), gate["flags"], len(end["spline"]), end["flags"]), (23, [2], 24, [8]))
        near(self, gate["spline"][0], (11.69, -2.85, -76.82), 0.006)
        near(self, gate["spline"][-1], (27.09, -2.34, -76.43), 0.006)
        near(self, end["spline"][0], (432.08, -48.68, -355.35), 0.006)
        near(self, end["spline"][-1], (432.80, -48.68, -354.69), 0.006)
        near(self, by["tut_sksc_01_22"]["spline"][0], (426.40, -51.51, -337.33), 0.006)
        chevrons = [i for i in ind if i["kind"] == 0]
        self.assertEqual([(i["objective"], i["locator"], i["type"], i["style"]) for i in chevrons],
                         [("tut_sksc_01_0", "tut_sksc_01_vi_%02d" % n, "left_chevron", "default") for n in range(1, 7)])
        self.assertTrue(all(i["locator"] in self.locators for i in chevrons))

    def test_every_kind_1_item_of_every_stage_row_has_a_spline(self):
        n = 0
        for row, items in self.indicators.items():
            for i in items:
                self.assertEqual(i["kind"] in (0, 1), True, row)
                if i["kind"] == 1:
                    n += 1
                    self.assertTrue(len(i["spline"]) >= 1 and len(i["flags"]) == 1, (row, i["objective"]))
                else:
                    self.assertTrue(i["spline"] == [] and i["flags"] == [] and (i["locator"] or any(k[:2] == (row, i["objective"]) for k in c.UNPLACED_INDICATORS)), (row, i["objective"]))
        self.assertGreaterEqual(n, 23)

    def test_the_only_indicator_locators_no_file_places_belong_to_objectives_their_row_never_lists(self):
        OBJECTIVES = "Hash_C4B345415E8537BE"      # the global data's Objectives list
        bank = c.Bank(c.normalised(self.db.collections("challengebanks/main")), ())
        listed = sorted([("tut_sksc_02", "tut_sksc_02_11", "tut_sksc_02_vi_011"), ("tut_sksc_02", "tut_sksc_02_3", "tut_sksc_02_vi_03"),
                         ("tut_sksc_02", "tut_sksc_02_5", "tut_sksc_02_vi_05"), ("tut_sksc_03", "tut_sksc_03_3", "tut_school_03_vi_04"),
                         ("tut_sksc_03", "tut_sksc_03_6", "tut_school_03_vi_07")])
        self.assertEqual(sorted(c.UNPLACED_INDICATORS), listed)
        self.assertTrue(all(c.UNPLACED_INDICATORS.values()))
        # exactly those items export "locator": null; every other kind 0 item names a locator that is in `locators`
        nulls = sorted((row, i["objective"]) for row, items in self.indicators.items() for i in items if i["kind"] == 0 and i["locator"] is None)
        self.assertEqual(nulls, sorted((row, objective) for row, objective, _ in listed))
        self.assertTrue(all(i["locator"] in self.locators for items in self.indicators.values() for i in items
                            if i["kind"] == 0 and i["locator"] is not None))
        unplaced = listed
        for row, objective, _ in unplaced:
            listed = {c.ref_key({"data": it}) for it in c.items(bank.get(bank.by[(c.GLOBAL_DATA, c.key_hash(row))], OBJECTIVES))}
            self.assertNotIn(c.key_hash(objective), listed, objective)

    def test_ftl_univ_02_has_one_arrow(self):
        ind = self.indicators["ftl_univ_02"]
        self.assertEqual(len(ind), 1)
        self.assertEqual([(i["kind"], i["type"], i["style"], i["locator"], i["spline"], i["flags"]) for i in ind],
                         [(0, "arrow_nonfade", "default", "ftl_univ_02_vi_01", [], [])])
        near(self, self.locators["ftl_univ_02_vi_01"]["matrix"][3][:3], (170.96, 66.94, -178.54), 0.006)
        self.assertEqual(self.locators["ftl_univ_02_vi_01"]["map"], "maps/University.skate")

    def test_every_stage_one_place_resolves(self):
        for row in c.STAGE_ONE:
            r = self.rows[row]
            for name in (r["location"], r["start"], r["map_start"], r["end_teleport"]):
                if name:
                    self.assertIn(name, self.locators, (row, name))
                    self.assertEqual(self.locators[name]["map"], self.worlds[r["world"] if name != r["end_teleport"] else r["end_world"]], (row, name))
            for e in r["exits"]:
                self.assertEqual(self.locators[e["location"]]["map"], self.worlds[e["world"]], (row, e))
        # TeleporterTrigger of the six teleports, FTLEndVolume of the two follow-the-leader rows (stage1_teleport 4, stage1_univ)
        for name in ["tele_sksc_0%d_volume_01" % n for n in range(1, 5)] + ["tele_school_to_world_volume_01", "tele_world_to_school_volume_01"]:
            self.assertIn(name, self.volumes)
        near(self, self.volumes["tele_sksc_01_volume_01"]["min"], (408.5, -53.5, -404.4), 0.06)
        near(self, self.volumes["tele_sksc_01_volume_01"]["max"], (493.6, -43.9, -324.7), 0.06)
        near(self, self.volumes["tele_school_to_world_volume_01"]["min"], (-32.6, -0.6, -42.3), 0.06)
        near(self, self.volumes["tele_world_to_school_volume_01"]["max"], (415.4, 72.4, -336.0), 0.06)
        near(self, self.volumes["ftl_univ_02_spotvolume_01"]["min"], (161.44, 63.89, -188.06), 0.006)
        near(self, self.volumes["ftl_univ_02_spotvolume_01"]["max"], (180.48, 68.89, -169.02), 0.006)
        near(self, self.volumes["ftl_univ_03_spotvolume_01"]["min"], (460.08, 67.43, -341.43), 0.006)
        self.assertEqual(self.volumes["ftl_univ_02_spotvolume_01"]["id"], 3202084649600156457)    # the decimal id stage1_univ gives the snippets
        self.assertEqual(self.volumes["tut_sksc_01_gate_end"]["map"], "maps/SkateSchool.skate")
        near(self, self.volumes["tut_sksc_01_gate_end"]["min"], (431.1, -53.2, -357.2), 0.06)
        # the SkateSchool world stream is kept whole: the hub volumes and the coach (stage1_school 5)
        for name in ("tut_sksc_inthehub_vol_01", "tut_sksc_inthehub_vol_02", "tut_sksc_wrongway_vol_01", "tut_sksc_reset_vol01"):
            self.assertIn(name, self.volumes)
        self.assertEqual(self.volumes["tut_sksc_inthehub_vol_01"]["map"], "maps/SkateSchool.skate")
        # the six hint volumes no disc file holds stay absent, so IsInVolume is false as in retail (spec 18.20)
        for n in (7, 8, 11, 12, 23, 24):
            self.assertNotIn("tut_sksc_02_hintvolume_%02d" % n, self.volumes)
            self.assertNotIn("tut_sksc_02_hintvolume_%d" % n, self.volumes)
        self.assertIn("tut_sksc_02_hintvolume_09", self.volumes)

    def test_the_fe_locations_and_the_global_locators_land_on_their_districts_maps(self):
        near(self, self.locators["tele_world_to_school_dest_locator_01"]["matrix"][3][:3], (-31.28, 0.11, -31.50), 0.006)
        self.assertEqual(self.locators["tele_world_to_school_dest_locator_01"]["map"], "maps/SkateSchool.skate")
        near(self, self.locators["tele_school_to_world_dest_locator_01"]["matrix"][3][:3], (407.52, 69.93, -333.30), 0.006)
        near(self, self.locators["tele_school_to_world_dest_locator_01"]["slots"][0][3][:3], (407.53, 69.93, -330.30), 0.006)
        near(self, self.locators["tele_sksc_01_challengelocator_01"]["matrix"][3][:3], (466.49, -57.64, -392.06), 0.006)
        near(self, self.locators["tele_sksc_01_challengelocator_01"]["slots"][0][3][:3], (-24.59, 0.34, 7.39), 0.006)
        near(self, self.locators["tut_sksc_01_challengelocator_01"]["matrix"][3][:3], (-7.46, 0.41, -23.53), 0.006)

    def test_the_progression_teleport_events_name_locators_that_resolve_but_one(self):
        events = [(e["args"][0], e["args"][1]) for g in self.prog["stategraphs"].values() for s in g["states"].values()
                  for e in s["events"] if e["kind"] == 8 and e["args"][0]]
        self.assertEqual(len(events), 20)
        missing = [(n, w) for n, w in events if n not in self.locators]
        self.assertEqual(missing, [("dist_university_clocktower", "dist_university")] * 1)
        self.assertEqual(list(c.UNPLACED_EVENTS), [("dist_university_clocktower", "dist_university")])

    def test_the_json_round_trips_and_has_no_nan(self):
        text = json.dumps({"l": self.locators, "v": self.volumes, "i": self.indicators}, allow_nan=False)
        self.assertEqual(json.loads(text)["l"]["tut_sksc_01_startlocator_01"]["map"], "maps/SkateSchool.skate")


@unittest.skipUnless(os.environ.get("SKATE3_DISC") and os.environ.get("SKATE3_DLC"),
                     "set SKATE3_DISC and SKATE3_DLC (the folder of the DLC packages)")
class DlcDataTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import skate3_online as o
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-career-dlc-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.db = c.CareerDb(Path(os.environ["SKATE3_DISC"]), Path(cls.tmp.name))
        cls.dlcs = o.dlc_packages(Path(os.environ["SKATE3_DLC"]), cls.db, Path(cls.tmp.name) / "dlc")
        cls.rows = {d.banks[0].stem: c.career_rows(d, pack=d.banks[0].stem, base=cls.db) for d in cls.dlcs}

    def test_four_packages_carry_their_pack_id_on_every_row(self):
        self.assertEqual(sorted(self.rows), ["dlc_dwgh", "dlc_mmcn", "dlc_play", "dlc_player1"])
        for pack, rows in self.rows.items():
            self.assertTrue(rows, pack)
            self.assertTrue(all(r["pack"] == pack for r in rows.values()), pack)
        self.assertEqual([len(self.rows[p]) for p in sorted(self.rows)], [44, 19, 18, 4])

    def test_after_dark_s_rows_are_the_four_order_9_1_names(self):
        self.assertEqual(sorted(self.rows["dlc_player1"]), ["film_dark_01", "ots_dark_01", "ots_dark_02", "photo_dark_01"])
        self.assertTrue(all(r["world"] == "dist_sanitariumdlc" for r in self.rows["dlc_player1"].values()))

    def test_danny_way_s_teleports_and_lots_are_there_and_no_template_is_a_row(self):
        rows = self.rows["dlc_dwgh"]
        self.assertEqual(sorted(k for k in rows if k.startswith("teleport_dwmc_")), ["teleport_dwmc_0%d" % i for i in range(1, 5)])
        self.assertEqual(rows["otl_dwmc_01"]["global"], 3)
        self.assertEqual(len(rows["otl_dwmc_01"]["lot_rows"]), 6)
        self.assertFalse([k for k in rows if k.startswith("dlc_")])
        self.assertTrue(all(r["global"] in (2, 3) or r["lot"] for r in rows.values()))

    def test_no_package_row_needs_the_unnamed_world_allow_list(self):
        # the DLC worlds all have names (the name table, EXTRA_WORLDS); the one unnamed key is the disc's street_ind_01
        for pack, rows in self.rows.items():
            for k, r in rows.items():
                worlds = [r["world"], r["end_world"] or ""] + [e["world"] for e in r["exits"]]
                self.assertFalse([w for w in worlds if w.startswith("Hash_")], (pack, k))
        self.assertIn("dist_dannywaydlc", {r["world"] for r in self.rows["dlc_dwgh"].values()}
                      | {e["world"] for r in self.rows["dlc_dwgh"].values() for e in r["exits"]})

    def test_no_real_package_is_shorter_than_its_header_counts_and_the_folder_loads(self):
        folder = Path(os.environ["SKATE3_DLC"])
        sizes = []
        for p in sorted(q for q in folder.rglob("*") if q.is_file()):
            with p.open("rb") as f:
                head = f.read(0x1000)
            if head[:4] in c.STFS_MAGIC:
                self.assertGreaterEqual(p.stat().st_size, c.stfs_end(head), p.name)
                sizes.append(p.stat().st_size)
        self.assertEqual(len(sizes), 9)
        said = []
        got = c.load_dlcs(folder, self.db, Path(self.tmp.name) / "dlc_again", said.append)
        self.assertEqual(sorted(d.banks[0].stem for d in got), ["dlc_dwgh", "dlc_mmcn", "dlc_play", "dlc_player1"])
        self.assertEqual(len(said), 1)     # Time is Money, 122880 bytes: an unlock package with no database
        self.assertTrue(said[0].startswith("skipped DLC package") and "(122880 bytes)" in said[0])

    def test_no_package_row_shares_a_name_with_another_package_or_the_disc(self):
        main = c.career_rows(self.db)
        names = [k for rows in self.rows.values() for k in rows] + list(main)
        self.assertEqual(len(names), len(set(names)))


def java_ready():
    unluac = os.environ.get("SKATE_UNLUAC")
    return bool(unluac) and Path(unluac).is_file()


@unittest.skipUnless(os.environ.get("SKATE3_DISC") and java_ready(),
                     "set SKATE3_DISC to the extracted Skate 3 disc folder, and SKATE_JAVA / SKATE_UNLUAC to java and unluac.jar")
class ScriptsDataTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-career-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.db = c.CareerDb(Path(os.environ["SKATE3_DISC"]), Path(cls.tmp.name))
        cls.prog = c.progression(cls.db)
        cls.chunks = c.handler_chunks(cls.db)
        cls.functions = c.handler_functions(cls.chunks)
        cls.lua = c.progression_lua(cls.functions)

    def test_every_handler_decompiles(self):
        names = re.findall(r"^function (h_[0-9A-F]{16})\(self, message, vars, sharedvars, globalvars\)$", self.lua, re.M)
        self.assertEqual(len(names), 246)
        self.assertEqual(self.lua.count("function h_"), 246)
        self.assertEqual(len(set(names)), 246)
        used = {h for g in self.prog["stategraphs"].values() for s in g["states"].values() for h in s["handlers"].values()}
        self.assertEqual(len(used), 245)
        self.assertEqual(used - set(names), set())
        self.assertEqual(set(names) - used, {"h_70612941ADEDC0EE"})      # the handler no state lists is wrapped too
        self.assertNotIn("\r", self.lua)

    def test_every_handler_starts_with_its_header_and_ends_in_end(self):
        for name, fn in self.functions.items():
            self.assertTrue(fn.startswith(f"function {name}(") and fn.endswith("\nend"), name)

    def test_the_graph_scripts_are_the_four_decompiled_chains(self):
        got = c.graph_scripts(self.db)
        self.assertEqual(sorted(got), ["scripts/graphs/objectives_shared.lua", "scripts/graphs/photo_skate2.lua",
                                       "scripts/graphs/teleport.lua", "scripts/graphs/tutorials.lua"])
        for path, text in got.items():
            self.assertGreater(len(text), 200, path)
            self.assertIn("function ", text, path)
            self.assertNotIn("\r", text, path)

    def test_reda_is_granted_by_ftl_univ_03(self):
        members = {m for g in self.prog["groups"].values() for m in g}
        rows = c.career_rows(self.db, members)
        sites = c.reward_sites(rows, self.prog, self.functions)
        self.assertIn("row:ftl_univ_03", sites["reda"])
        self.assertTrue(all(s.startswith(("row:", "graph:")) for v in sites.values() for s in v))
        self.assertTrue(all(g in self.prog["stategraphs"] for v in sites.values() for s in v if s.startswith("graph:") for g in [s[6:]]))


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class StoryRowsDataTest(unittest.TestCase):
    """Task 6 on the real disc: rows/<row>.json of the stage-1 rows (stage1_school 3.1-3.5, stage1_traces 1.6 and 4.1)."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-career-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.disc = Path(os.environ["SKATE3_DISC"])
        cls.db = c.CareerDb(cls.disc, Path(cls.tmp.name))
        cls.prog = c.progression(cls.db)
        cls.rows = c.career_rows(cls.db, {m for g in cls.prog["groups"].values() for m in g})
        cls.fe = c.fe_locations(cls.db)
        cls.worlds = c.world_maps(cls.db, c.named_worlds(cls.rows, cls.fe, cls.prog))
        cls.locators, cls.volumes, _ = c.stage_places(cls.db, cls.rows, cls.prog, cls.fe, cls.worlds, c.DiscPlaces(cls.disc))
        cls.files, cls.sources = c.row_files(cls.db, cls.rows, cls.locators, cls.volumes)

    def objectives(self, row, field="Objectives"):
        return [o for g in self.files[row]["groups"][field] for o in g["objectives"]]

    def test_every_stage_one_row_has_a_file(self):
        self.assertEqual(sorted(self.files), sorted(c.STAGE_ONE))

    def test_lesson_failure_records_are_hints(self):
        # stage1_traces 4.1: every lesson failure record is in the Hash_ECAEB8E619A68AF3 list, all 57 Hash_B69AF085F7A6D275 lists
        # are empty, there is no inline FailureObjectives, and FailFirst is 0 on all 57
        every = [o for row in LESSONS for o in self.objectives(row)]
        self.assertEqual(len(every), 57)
        self.assertEqual([o["key"] for o in every if o["failures"] or o["failure_keys"] or o["inline_failures"]], [])
        self.assertEqual([o["key"] for o in every if o["fail_first"]], [])
        self.assertTrue(sum(len(o["hints"]) for o in every) > 100)
        self.assertTrue(all(len(o["hints"]) == len(o["hint_keys"]) == len(o["groups"]["hints"]) for o in every))
        # every failure_objective record the lessons reach is one of those hints
        bank = c.Bank(c.normalised(self.db.collections("challengebanks/main")), ())
        reached = set()
        for o in every:
            rec_ = bank.by[(c.OBJECTIVE, c.key_hash(o["key"]))]
            for field in (c.HINTS, c.FAILURES, c.INLINE_FAILURES):
                reached |= {c.ref_key({"data": it}) for it in c.items(bank.get(rec_, field))}
        self.assertEqual(reached, {c.key_hash(k) for o in every for k in o["hint_keys"]})

    def test_tut_sksc_01_has_22_objectives_and_flow_0_1(self):
        f = self.files["tut_sksc_01"]
        self.assertEqual((len(self.objectives("tut_sksc_01")), f["objectives"], f["flow"]), (22, 22, [0, 1]))
        by = {o["key"]: o for o in self.objectives("tut_sksc_01")}
        self.assertEqual([k for k, o in by.items() if o["master"]], ["tut_sksc_01_33"])        # stage1_school 3.1: only _33 is the master
        self.assertEqual({o["activation"] for o in by.values()}, {1})                          # every one is mode 1: Enabled, then Active at start
        # the 12 failure records of the master, in list order (stage1_school 3.1 table)
        self.assertEqual(by["tut_sksc_01_33"]["hint_keys"],
                         ["tut_sksc_01_33_5", "tut_sksc_misc_onboard_tip", "tut_sksc_misc_onboard1st_tip", "tut_sksc_01_33_7", "tut_sksc_01_33_8",
                          "tut_sksc_01_33_0", "tut_sksc_01_33_6", "tut_sksc_01_33_1", "tut_sksc_01_33_2", "tut_sksc_01_33_3", "tut_sksc_01_33_4",
                          "tut_sksc_misc_inthehub_01"])
        self.assertEqual(by["tut_sksc_01_33"]["groups"]["complete"][0]["kind"], 4)             # Complete: Wait 1.5 first
        self.assertEqual([e["kind"] for e in by["tut_sksc_01_33"]["groups"]["intro"]], [16])  # Intro: HideMiniMap

    def test_tut_sksc_02_flow_is_2_2(self):
        f = self.files["tut_sksc_02"]
        self.assertEqual((len(self.objectives("tut_sksc_02")), f["flow"]), (15, [2, 2]))

    def test_the_other_lessons_have_the_objective_counts_and_flows_of_stage1_school(self):
        want = {"tut_sksc_01": (22, [0, 1]), "tut_sksc_02": (15, [2, 2]), "tut_sksc_06": (5, [1, 1]), "tut_sksc_03": (11, [0, 1]),
                "tut_sksc_04": (4, [1, 2])}
        self.assertEqual({r: (len(self.objectives(r)), self.files[r]["flow"]) for r in LESSONS}, want)

    def test_tut_sksc_06_has_5_objectives_and_no_master(self):
        objs = self.objectives("tut_sksc_06")
        self.assertEqual(len(objs), 5)
        self.assertEqual([o["master"] for o in objs], [False] * 5)
        self.assertEqual(self.files["tut_sksc_06"]["flow"], [1, 1])
        self.assertEqual(objs[0]["key"], "tut_sksc_06_4")                                     # stage1_school 3.3: the first, mode 1
        self.assertEqual(objs[0]["activation"], 1)
        self.assertEqual(objs[0]["hint_keys"], ["tut_sksc_06_12", "tut_sksc_06_11", "tut_sksc_06_0", "tut_sksc_06_0_0", "tut_sksc_06_4", "tut_sksc_06_2"])

    def test_ftl_univ_02_s_speech_triggers_are_hints_of_its_follow_objective(self):
        objs = self.objectives("ftl_univ_02", "ObjectivesOwned")
        self.assertEqual(len(objs), 1)
        self.assertEqual(objs[0]["hint_keys"], ["ftl_univ_02_0_0", "ftl_univ_02_0_1", "ftl_univ_02_0_2", "ftl_univ_02_0_3"])   # stage1_univ 2.x
        self.assertEqual(self.files["ftl_univ_02"]["flow"], [0, 1])

    def test_the_teleport_rows_have_no_objectives_no_events_and_the_default_speech(self):
        for row in TELEPORTS + ["tele_world_to_school"]:
            f = self.files[row]
            self.assertEqual((f["objectives"], f["flow"], f["presentation"]), (0, None, {}), row)
            self.assertEqual([o for field in f["groups"].values() for g in field for o in g["objectives"]], [], row)
            self.assertEqual(f["speech"]["HostCharacter"], 0x28, row)

    def test_the_lessons_presentation_groups_are_the_intro_and_restart_events(self):
        # stage1_school 3: tut_sksc_01 Intro NIS + ActivateMinimalScoringHUD, Restart Teleport + HideTutorialCalloutHUD + NIS
        kinds = {r: ([e["kind"] for e in self.files[r]["presentation"]["Intro"]], [e["kind"] for e in self.files[r]["presentation"]["Restart"]])
                 for r in LESSONS}
        self.assertEqual(kinds, {"tut_sksc_01": ([5, 49], [8, 21, 5]), "tut_sksc_02": ([51, 5, 6, 49], [21, 8, 5, 51, 6]),
                                 "tut_sksc_06": ([5, 49], [21, 8, 5]), "tut_sksc_03": ([5, 49], [21, 8, 5]), "tut_sksc_04": ([5], [21, 8, 5])})
        e = self.files["tut_sksc_01"]["presentation"]["Intro"][0]
        self.assertEqual(e["args"], ["tut_sksc_01", ""])
        self.assertEqual(self.files["tut_sksc_01"]["intro"], "tut_sksc_01")
        self.assertEqual(self.files["tut_sksc_01"]["presentation"]["Restart"][0]["args"][0], "tut_sksc_01_startlocator_01")

    def test_the_lessons_speak_with_the_skate_coach(self):
        for row in LESSONS:
            s = self.files[row]["speech"]
            # stage1_school 3 "Common to all five": HostCharacter / AudioCharacterOverride 0x1F, local @row / @skateschool; AudioPlayerQuitChallenge
            # 0x1F id 5706, local @skateschool (the row's same-file parent record); Succeed and Won come from @tutorial, the framework record above it
            self.assertEqual((s["HostCharacter"], s["AudioCharacterOverride"]), (0x1F, 0x1F), row)
            self.assertEqual(s["calls"]["AudioPlayerQuitChallenge"], [0x1F, 5706], row)
            self.assertEqual(s["calls"]["AudioPlayerDeclineChallenge"], [0x1F, 5706], row)       # the global data's own Decline field, kept apart from Quit
            self.assertEqual((s["calls"]["AudioPlayerSucceedChallenge"], s["calls"]["AudioPlayerWonChallenge"]), ([0, 0], [0x28, 0]), row)
            self.assertEqual(self.files[row]["presentation"]["Outro"], [], row)

    def test_university_speech_fields_match_the_trace(self):
        # stage1_traces 1.6: HostCharacter ftl 0x68, photo_univ_08 and film_univ_05 0x1E; AudioCharacterOverride ftl 0x1E, photo / film 0x28
        got = {r: (self.files[r]["speech"]["HostCharacter"], self.files[r]["speech"]["AudioCharacterOverride"]) for r in UNIVERSITY}
        self.assertEqual(got, {"ftl_univ_02": (0x68, 0x1E), "photo_univ_08": (0x1E, 0x28), "film_univ_05": (0x1E, 0x28), "ftl_univ_03": (0x68, 0x1E)})
        calls = {r: self.files[r]["speech"]["calls"] for r in UNIVERSITY}
        self.assertEqual(calls["ftl_univ_02"]["AudioPhotographerSetupPlayerSpeech"], [0x1E, 8572])
        self.assertEqual(calls["ftl_univ_03"]["AudioPhotographerSetupPlayerSpeech"], [0x1E, 8575])
        self.assertEqual((calls["ftl_univ_02"]["AudioPlayerRestartChallenge"], calls["ftl_univ_02"]["AudioPlayerWonChallenge"]), ([0x1E, 8569], [0x1E, 8566]))
        self.assertEqual(calls["ftl_univ_03"]["AudioPlayerWonChallenge"], [0x1E, 8564])
        self.assertEqual((calls["ftl_univ_02"]["AudioPlayerQuitChallenge"], calls["ftl_univ_02"]["AudioPlayerDeclineChallenge"]), ([0x1E, 1010], [0x1E, 1703]))
        self.assertEqual(calls["ftl_univ_02"]["AudioPlayerSucceedChallenge"], [0x1E, 0])
        self.assertEqual(calls["photo_univ_08"]["AudioPhotoPickAShot"], [0x27, 1015])
        self.assertEqual(calls["photo_univ_08"]["AudioPlayerLeftChallenge"], [0x28, 709])
        self.assertEqual(calls["photo_univ_08"]["AudioPlayerQuitChallenge"], [0x1E, 1016])
        self.assertEqual(calls["photo_univ_08"]["AudioPlayerSucceedChallenge"], [0x28, 1011])
        self.assertEqual(calls["film_univ_05"]["AudioBeginReplay"], [0x28, 712])
        self.assertEqual(calls["film_univ_05"]["AudioPlayerSucceedChallenge"], [0x27, 705])
        self.assertEqual(calls["film_univ_05"]["AudioPlayerQuitChallenge"], [0x27, 801])
        self.assertEqual(calls["film_univ_05"]["Hash_701931776A0643A6"], [0x28, 701])          # a field no name table holds keeps its hash (1.6)
        self.assertEqual(calls["film_univ_05"]["Hash_D8D76B15F453BE2C"], [0x28, 713])

    def test_every_trigger_volume_the_objectives_name_is_placed_but_hint_volumes_some_files_lack(self):
        for row in LESSONS + UNIVERSITY:
            f = self.files[row]
            for o in [o for field in f["groups"].values() for g in field for o in g["objectives"]]:
                used = {t[2][0][1] for t in c.native_calls(o["obj"], o["points_valid"]) if t[1] in c.TRIGGER_NATIVES and t[2] and t[2][0][0] == "str"}
                self.assertEqual([i for i in used if f["trigger_volumes"][i] is None], [], (row, o["key"]))
        # spec 18.20: exactly six hint volumes of tut_sksc_02 are in no file, and nothing else of any row is unplaced
        missing = sorted((r, i) for r, f in self.files.items() for i, v in f["trigger_volumes"].items() if v is None)
        self.assertEqual(missing, sorted(c.UNPLACED_VOLUMES))
        self.assertEqual(len(missing), 6)
        self.assertEqual({r for r, _ in missing}, {"tut_sksc_02"})
        names = {}
        for row, i in missing:
            hits = re.findall(r"%s[^|]*\|(tut_sksc_02_hintvolume_\d+)" % i, "".join(self.sources.values()))
            self.assertEqual(len(set(hits)), 1, i)
            names[i] = hits[0]
        self.assertEqual(sorted(names.values()), ["tut_sksc_02_hintvolume_%02d" % n for n in (7, 8, 11, 12, 23, 24)])
        self.assertTrue(all(c.UNPLACED_VOLUMES.values()))

    def test_the_trick_groups_the_rows_name_have_members(self):
        named = {r: f["trick_groups"] for r, f in self.files.items() if f["trick_groups"]}
        self.assertEqual(named, {"tut_sksc_02": ["fliptricks", "nollies", "ollies"], "tut_sksc_03": ["grinds"], "film_univ_05": ["edge_tricks", "multi_trick"],
                                 "photo_univ_08": ["ground_tricks"]})
        for row, groups in named.items():
            self.assertEqual(sorted(self.files[row]["trick_group_members"]), groups)
            self.assertTrue(all(self.files[row]["trick_group_members"][g] for g in groups), row)
        self.assertEqual(self.files["tut_sksc_02"]["trick_group_members"]["ollies"], ["ollie"])

    def test_a_row_file_is_plain_json_and_every_snippet_has_its_source(self):
        text = json.dumps(self.files)
        self.assertNotIn("\r", text)
        src = {str(s["src"]) for f in self.files.values() for field in f["groups"].values() for g in field for o in g["objectives"]
               for s in [o["obj"], o["points_valid"], *o["hints"], *o["failures"], *o["inline_failures"]] if s}
        self.assertTrue(src and src <= set(self.sources))
        self.assertTrue(all(isinstance(v, str) and v.strip() for v in self.sources.values()))

    # ---- Task 7: the in-challenge speech against stage1_traces 1.3-1.6 (the layout reads hdr / sth / snr headers; no audio is decoded)
    @classmethod
    def speech_json(cls):
        if not hasattr(cls, "_speech"):
            work = Path(cls.tmp.name) / "speech-work"
            work.mkdir(exist_ok=True)
            cls._speech = c.speech_layout(cls.disc, cls.files, work)[0]
        return cls._speech

    def plays(self, row, field, **kw):
        """The takes (sample -> count) the row's own speech field plays: its character, line, host and override, the plain form for the
        end-dialog fields."""
        s = self.files[row]["speech"]
        character, line = s["calls"][field]
        got = c.resolve_line(self.speech_json(), character, line, s["HostCharacter"], s["AudioCharacterOverride"], plain=field in c.PLAIN_SPEECH, **kw)
        return Counter(re.sub(r"_\d+\.wav$", "", f) for f in got)

    def line(self, character, line, host=c.NONE_CHARACTER, **kw):
        return Counter(re.sub(r"_\d+\.wav$", "", f) for f in c.resolve_line(self.speech_json(), character, line, host, **kw))

    def test_university_lines_match_the_trace(self):
        # stage1_traces 1.6, row by row: the files each line resolves to and their take counts, or nothing
        p, ln = self.plays, self.line
        for row, sample, field, key in (("ftl_univ_02", "30_8572_PRO-CHRI", "AudioPhotographerSetupPlayerSpeech", 2),
                                        ("ftl_univ_03", "30_8575_PRO-CHRI", "AudioPhotographerSetupPlayerSpeech", 3),
                                        ("ftl_univ_02", "30_8569_PRO-CHRI", "AudioPlayerRestartChallenge", 3),
                                        ("ftl_univ_03", "30_8569_PRO-CHRI", "AudioPlayerRestartChallenge", 3),
                                        ("ftl_univ_02", "30_8566_PRO-CHRI", "AudioPlayerWonChallenge", 2),
                                        ("ftl_univ_03", "30_8564_PRO-CHRI", "AudioPlayerWonChallenge", 3)):
            s = self.files[row]["speech"]["calls"][field]
            got = c.resolve_line(self.speech_json(), s[0], s[1], self.files[row]["speech"]["HostCharacter"], self.files[row]["speech"]["AudioCharacterOverride"],
                                 plain=field in c.PLAIN_SPEECH)
            self.assertEqual(got, [f"{sample}_{n}_0.wav" for n in range(1, key + 1)], (row, field))   # variants in directory order
        for row in ("ftl_univ_02", "ftl_univ_03"):
            self.assertEqual(p(row, "AudioPlayerFailedChallenge"), {"1010_30_Reda_wrap": 7}, row)     # main cast event 0xC3
            self.assertEqual(p(row, "AudioPlayerQuitChallenge"), {"1010_30_Reda_wrap": 7}, row)
            self.assertEqual(p(row, "AudioPlayerDeclineChallenge"), {"1703_30_Reda_decline": 11}, row)   # main cast 0x87
            self.assertEqual(c.resolve_line(self.speech_json(), 30, 0), [], row)                         # Succeed Reda:0
        self.assertEqual(self.speech_json()["banks"]["maincast"]["195"][2]["masks"], [8, 0])
        self.assertEqual(self.files["ftl_univ_02"]["speech"]["calls"]["AudioPlayerSucceedChallenge"], [0x1E, 0])
        # the in-run SpeechNoWait Reda 309 / 308 / 310 are events 8 / 7 / 9 and are named by the ftl rows' hint events
        for row in ("ftl_univ_02", "ftl_univ_03"):
            named = {(e["args"][0], e["args"][1]) for events in c.event_lists(self.files[row]) for e in events if e["kind"] in c.SPEECH_EVENTS}
            self.assertTrue({("Reda", "308"), ("Reda", "309"), ("Reda", "310")} <= named, (row, named))
        self.assertEqual((ln(30, 309), ln(30, 308), ln(30, 310)),
                         ({"309_30_follow_behind": 13}, {"308_30_follow_chatter": 21}, {"310_30_follow_pass": 8}))
        self.assertEqual([c.table_event(c.speech_tables(), "maincast", n) for n in (309, 308, 310)], [8, 7, 9])
        # photo_univ_08 (Reda host 0x1E, override 0x28)
        self.assertEqual(p("photo_univ_08", "AudioPhotoPickAShot"), {"1015_30_Reda_pickone": 10})
        self.assertEqual(c.table_event(c.speech_tables(), "cameraman", 1015), 0x81D6)
        self.assertEqual([x["masks"] for x in self.speech_json()["banks"]["cameraman"]["33238"]], [[2]])           # mask 2: Reda
        self.assertEqual(p("photo_univ_08", "AudioPhotographerSetupPlayerSpeech"), {"1307_30_Reda_start": 8})
        self.assertEqual(p("photo_univ_08", "AudioPlayerRestartChallenge"), {"1307_30_Reda_start": 8})
        self.assertEqual(p("photo_univ_08", "AudioPlayerLeftChallenge"), {"709_30_oob": 8})
        self.assertEqual(p("photo_univ_08", "AudioPlayerQuitChallenge"), {"1016_30_wrap_idea": 10})          # main cast 0x125
        self.assertEqual(p("photo_univ_08", "AudioPlayerSucceedChallenge"), {"1011_30_Reda_PC_Win": 11})
        self.assertEqual(ln(40, 0, host=0x1E), Counter())                                                    # Won CameraMan:0
        # photo / film Decline CameraMan:1703 (0x81DA, mask 2)
        for row in ("photo_univ_08", "film_univ_05"):
            self.assertEqual(p(row, "AudioPlayerDeclineChallenge"), {"1703_30_decline": 6}, row)
        # film_univ_05
        film = "film_univ_05"
        self.assertEqual(p(film, "AudioBeginReplay"), {"712_30_replay": 10})
        self.assertEqual(p(film, "Hash_5F64085D585016AB"), {"712_30_replay": 10})
        self.assertEqual(p(film, "AudioPhotographerSetupPlayerSpeech"), {"701_30_start": 10})
        self.assertEqual(p(film, "Hash_701931776A0643A6"), {"701_30_start": 10})
        self.assertEqual(p(film, "AudioPlayerRestartChallenge"), {"702_30_retry_rt3": 7})
        self.assertEqual((p(film, "AudioPlayerLeftChallenge"), p(film, "AudioPlayerNotEnoughPoint")), ({"709_30_oob": 8}, {"711_30_almost": 8}))
        self.assertEqual(p(film, "AudioPlayerSucceedChallenge"), {"705_30_win_sc1": 5, "705_30_win_sc2": 4})       # the argument picks one (INFERRED)
        self.assertEqual(p(film, "AudioPlayerSucceedChallenge", argument=1), {"705_30_win_sc1": 5})
        self.assertEqual(p(film, "AudioPlayerSucceedChallenge", argument=2), {"705_30_win_sc2": 4})
        self.assertEqual(p(film, "AudioPlayerQuitChallenge"), {"801_30_FilmChalQuit": 10})
        self.assertEqual(p(film, "AudioPlayerFailedChallenge"), Counter())                                          # Failed Unknown:801, plain: nothing
        self.assertEqual(p(film, "Hash_D8D76B15F453BE2C"), {"713_30_timewarn": 13})
        self.assertEqual((p(film, "Hash_623959719AFF582D"), p(film, "Hash_681DDE8CCEAC357C")), (Counter(), Counter()))   # 707 / 706, Reda host
        self.assertEqual(ln(39, 0, host=0x1E, override=40), Counter())                                               # Won Unknown:0
        # Reda's cameraman-bank files exist only as _30_; with a CameraMan host (0x28 / 0x68) the same events give the _40_ files
        self.assertEqual(ln(40, 709, host=0x28), {"709_40_oob": 15})
        self.assertEqual(ln(40, 709, host=0x1F), {"709_31_oob": 7})         # the coach as host: mask 4, the _31_ files
        self.assertEqual(set(ln(40, 706, host=0x68)), {"706_40_loseslam_lv1", "706_40_loseslam_lv2", "706_40_loseslam_lv3"})
        self.assertEqual(set(ln(40, 707, host=0x28)), {"707_40_lose_time"})
        # 1.4: Slappy is Vacant10 (62) and 5033 has no file for anyone
        self.assertEqual(c.speaker("Slappy"), 62)
        self.assertEqual(self.speech_json()["scripted"]["62_5033"], [])
        self.assertEqual(self.speech_json()["scripted"]["31_5033"], [])
        self.assertIn((62, 5033), c.row_speech_lines(self.files["tut_sksc_03"]))

    def test_scripted_takes_have_the_traced_durations(self):
        # stage1_traces 1.3 / 1.6: the .snr headers of the University and School lines (rate word, sample count word)
        import ea_speech
        lines = ea_speech.ScriptedLines(self.disc / "data/audio/english/nis.big", Path(self.tmp.name))
        seconds = lambda stem: round(lines.take(stem).frames / lines.take(stem).hertz, 2)      # noqa: E731
        want = {"30_8572_PRO-CHRI_1": 1.93, "30_8572_PRO-CHRI_2": 2.61, "30_8575_PRO-CHRI_1": 3.67, "30_8575_PRO-CHRI_2": 3.32,
                "30_8575_PRO-CHRI_3": 3.77, "30_8569_PRO-CHRI_1": 1.25, "30_8569_PRO-CHRI_2": 1.7, "30_8569_PRO-CHRI_3": 1.62,
                "30_8566_PRO-CHRI_1": 1.94, "30_8566_PRO-CHRI_2": 2.2, "30_8564_PRO-CHRI_1": 1.14, "30_8564_PRO-CHRI_2": 1.19,
                "30_8564_PRO-CHRI_3": 1.41, "31_5706_TUT-JORD_1": 1.26, "31_5706_TUT-JORD_2": 1.72, "31_5706_TUT-JORD_3": 2.64,
                "31_5706_TUT-JORD_4": 0.88, "31_5706_TUT-JORD_5": 1.15, "31_5706_TUT-JORD_6": 1.63}
        self.assertEqual({stem: seconds(stem) for stem in want}, want)
        self.assertEqual(sum(map(len, lines.variants().values())), 927)                      # 927 headers, each a variant of a (character, id) line

    def test_the_school_lines_the_lessons_name_have_files_but_5033(self):
        # stage1_traces 1.3: the takes per School id; 1.4: 5033 has none. The lessons' hint speech also names 5096-5103 (the 1.3 list was cut short)
        speech = self.speech_json()
        scripted = speech["scripted"]
        self.assertEqual(sorted(k for k, v in scripted.items() if not v), ["104_6774", "104_7762", "31_5033", "62_5033"])    # + the two SignupSpeech lines
        counts = {int(k[3:]): len(v) for k, v in scripted.items() if k.startswith("31_") and v}
        for ident, takes_ in {5002: 1, 5004: 1, 5021: 6, 5024: 3, 5026: 4, 5043: 1, 5148: 9, 5706: 6, 5067: 7, 5068: 4, 5690: 9}.items():
            self.assertEqual(counts[ident], takes_, ident)
        self.assertEqual(scripted["31_5043"], ["31_5043_TUT-JORD_5_360_0.wav"])         # 5043 has only a _360 take (1.3)

    def test_every_leaf_file_is_a_take_of_a_sample_the_bank_holds_and_the_json_is_plain(self):
        speech = self.speech_json()
        self.assertEqual(sorted(speech), ["banks", "conditions", "scripted"])
        self.assertEqual(sorted(speech["banks"]), ["cameraman", "maincast"])             # the living world is named by no stage-1 line
        self.assertEqual({b: sorted(int(e) for e in ev) for b, ev in speech["banks"].items()},
                         {"cameraman": [0x8001, 0x8003, 0x8007, 0x8009, 0x800B, 0x800F, 0x8013, 0x808D, 0x808F, 0x810A, 0x81D4, 0x81D6, 0x81D7, 0x81DA],
                          "maincast": [7, 8, 9, 0x87, 0xC3, 0x125]})
        for bank, events in speech["banks"].items():
            for event, leaves in events.items():
                self.assertEqual(len(speech["conditions"][bank][event]), len(leaves[0]["masks"]), (bank, event))
                self.assertTrue(all(len(x["masks"]) == len(leaves[0]["masks"]) for x in leaves), (bank, event))
        text = json.dumps(speech)
        self.assertNotIn("\r", text)


class SpeechNamesTest(unittest.TestCase):
    """The speaker lookup of stage1_traces 1.1 over the 104 names of speech_tables.json (Task 1)."""

    def test_the_elf_hash_gives_the_two_hashes_the_trace_names(self):
        # 1.4: "Slappy" is 0x05A28779 and the lower_bound lands on Vacant10 (0x07985CC0)
        self.assertEqual((c.elf_hash("Slappy"), c.elf_hash("Vacant10")), (0x05A28779, 0x07985CC0))

    def test_the_lookup_has_no_equality_test_so_slappy_is_vacant10(self):
        self.assertEqual(c.speaker("Slappy"), 62)
        self.assertEqual([c.speaker(n) for n in ("Reda", "SkateCoach", "Unknown", "CameraMan", "Cuz", "Cyrus")], [30, 31, 39, 40, 35, 36])

    def test_every_listed_name_finds_its_own_index(self):
        for name, index in c.speech_tables()["characters"]:
            self.assertEqual(c.speaker(name), index, name)

    def test_a_name_hashing_above_every_listed_one_is_refused(self):
        tables = {"characters": [["A", 1]]}
        above = next(n for n in ("Zz", "Zy", "Zx", "Yy", "Xx") if c.elf_hash(n) > c.elf_hash("A"))
        with self.assertRaises(c.Refused) as ctx:
            c.speaker(above, tables)
        self.assertEqual(ctx.exception.reason, "missing")


def speech_row(calls=None, host=0x68, override=None, presentation=None, groups=None):
    return {"speech": {"calls": calls or {}, "HostCharacter": host, "AudioCharacterOverride": override},
            "presentation": presentation or {}, "groups": groups or {"Objectives": [], "ObjectivesOwned": [], "ObjectivesKilledIt": []}}


def say(kind, who, line):
    return {"kind": kind, "args": [who, str(line)], "flags": 0}


class SpeechLinesTest(unittest.TestCase):
    def lines(self, row):
        return c.row_speech_lines(row)

    def test_a_field_names_its_character_and_line_and_a_zero_line_names_nothing(self):
        row = speech_row({"AudioPlayerQuitChallenge": [30, 1010], "AudioPlayerSucceedChallenge": [30, 0], "SignupSpeech": [104, 7762]})
        self.assertEqual(self.lines(row), {(30, 1010), (104, 7762)})

    def test_unknown_becomes_the_override_in_the_override_form_only(self):
        # objectives_shared.lua L1175-1190: Won / Failed / Bailed / LoseTimer use the plain form; the rest use the override form
        row = speech_row({"AudioPlayerRestartChallenge": [39, 702], "AudioPlayerFailedChallenge": [39, 801], "AudioPlayerWonChallenge": [39, 5],
                          "AudioPlayerBailed": [39, 6], "AudioLoseTimerExpired": [39, 7]}, override=40)
        self.assertEqual(self.lines(row), {(40, 702), (39, 801), (39, 5), (39, 6), (39, 7)})

    def test_an_override_of_none_or_nothing_leaves_unknown_alone(self):
        for override in (None, 0x68):
            self.assertEqual(self.lines(speech_row({"AudioPlayerQuitChallenge": [39, 801]}, override=override)), {(39, 801)})

    def test_a_named_character_is_never_overridden(self):
        self.assertEqual(self.lines(speech_row({"AudioPlayerQuitChallenge": [30, 1016]}, override=40)), {(30, 1016)})

    def test_speech_events_name_a_character_and_a_line_through_the_name_map(self):
        events = [say(2, "Reda", 308), say(3, "SkateCoach", 5033), say(3, "Slappy", 5033), say(4, "Reda", 999), say(3, "Reda", "x"),
                  say(3, "Unknown", 801)]
        row = speech_row(presentation={"Intro": events}, override=40)
        self.assertEqual(self.lines(row), {(30, 308), (31, 5033), (62, 5033), (40, 801)})

    def test_events_in_every_objective_group_count(self):
        objective = {"groups": {"intro": [say(2, "Reda", 1)], "complete": [say(3, "Reda", 2)], "fail": [say(3, "Reda", 3)],
                                "hints": [[say(3, "Reda", 4)], []], "failures": [[say(3, "Reda", 5)]]}}
        row = speech_row(groups={"Objectives": [{"PointRequirement": 0, "objectives": [objective]}], "ObjectivesOwned": [], "ObjectivesKilledIt": []})
        self.assertEqual(self.lines(row), {(30, n) for n in range(1, 6)})


class SpeechPlanTest(unittest.TestCase):
    def plan(self, *rows):
        return c.speech_plan({f"row{i}": r for i, r in enumerate(rows)})

    def test_lines_from_5000_are_scripted_and_the_rest_go_to_their_bank_by_the_id_tables(self):
        plan = self.plan(speech_row({"a": [30, 1010], "b": [40, 709], "c": [31, 5706], "d": [30, 308], "e": [30, 0]}))
        self.assertEqual(plan["scripted"], [(31, 5706)])
        self.assertEqual(plan["banks"], {"maincast": [7, 0xC3], "cameraman": [0x800F]})

    def test_the_cameraman_and_the_unknown_speaker_route_by_character(self):
        plan = self.plan(speech_row({"a": [40, 801], "b": [39, 1010]}))
        self.assertEqual(plan["banks"], {"cameraman": [0x810A], "maincast": [0xC3]})

    def test_an_id_in_no_table_adds_nothing(self):
        plan = self.plan(speech_row({"a": [39, 801], "b": [30, 801]}))
        self.assertEqual(plan, {"scripted": [], "banks": {}})

    def test_a_speaker_the_port_has_no_bank_for_is_refused(self):
        for who in (35, 36, 5):
            with self.assertRaises(c.Refused) as ctx:
                self.plan(speech_row({"a": [who, 1010]}))
            self.assertEqual(ctx.exception.reason, "missing")


def synthetic_disc(root):
    """data/audio/english with a main cast, a cameraman bank and nis.big, laid out as stage1_traces 1.5 says."""
    from test_ea_speech import bank_big, evt, leaf, make_big, record, RATE
    folder = Path(root) / "data/audio/english"
    folder.mkdir(parents=True)
    main = evt([record(0xC3, [leaf((4, 0), 0x8001), leaf((8, 0), 0x8000)], conditions=(2, 1))])
    (folder / "maincastspeech.big").write_bytes(bank_big("maincast", main, {"1010_30_Reda_wrap": [(0, 3), (40, 3)], "1010_37_burn_wrap": [(0, 3)]}))
    camera = evt([record(0x800F, [leaf((1,), 0x8001), leaf((2,), 0x8000), leaf((4,), 0x9999)], conditions=(1,))])
    (folder / "cameramanspeech.big").write_bytes(bank_big("cameraman", camera, {"709_30_oob": [(0, 3), (40, 3), (90, 3)], "709_40_oob": [(0, 3)]}))
    resident = {f"{n}.snr": struct.pack(">II", RATE, 0x40000000 | f) for n, f in
                (("30_8572_PRO-CHRI_1", 7), ("31_5706_TUT-JORD_1", 4), ("30_8572_PRO-CHRI_2", 9))}
    (folder / "nis.big").write_bytes(make_big({"30_8572_PRO-CHRI_1.sns": bytes(7), "31_5706_TUT-JORD_1.sns": bytes(4),
                                               "30_8572_PRO-CHRI_2.sns": bytes(9), "nisresident.big": make_big(resident)}))
    return Path(root)


class SpeechDiscCase(unittest.TestCase):
    """A synthetic disc, a stand-in vgmstream and one row file naming lines on it."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.disc = synthetic_disc(self.dir / "disc")
        (self.dir / "work").mkdir()
        from test_ea_speech import FAKE
        fake = self.dir / "fake_vgmstream.py"
        fake.write_text(FAKE, encoding="utf-8", newline="\n")
        self.cmd = [__import__("sys").executable, str(fake)]
        self.files = {"r": speech_row({"AudioPlayerQuitChallenge": [30, 1010], "AudioPlayerLeftChallenge": [40, 709],
                                       "AudioPhotographerSetupPlayerSpeech": [30, 8572], "AudioPlayerDeclineChallenge": [31, 5706]},
                                      presentation={"Intro": [say(3, "Slappy", 5033)]})}

    def layout(self):
        return c.speech_layout(self.disc, self.files, self.dir / "work")


class SpeechExportTest(SpeechDiscCase):
    def test_the_layout_is_the_speech_json_of_the_brief(self):
        speech, jobs = self.layout()
        self.assertEqual(speech["scripted"], {"30_8572": ["30_8572_PRO-CHRI_1_0.wav", "30_8572_PRO-CHRI_2_0.wav"],
                                              "31_5706": ["31_5706_TUT-JORD_1_0.wav"], "62_5033": []})
        self.assertEqual(speech["banks"], {
            "cameraman": {"32783": [{"masks": [1], "files": ["709_40_oob_0.wav"]},
                                    {"masks": [2], "files": ["709_30_oob_0.wav", "709_30_oob_1.wav", "709_30_oob_2.wav"]},
                                    {"masks": [4], "files": []}]},
            "maincast": {"195": [{"masks": [4, 0], "files": ["1010_37_burn_wrap_0.wav"]},
                                 {"masks": [8, 0], "files": ["1010_30_Reda_wrap_0.wav", "1010_30_Reda_wrap_1.wav"]}]}})
        self.assertEqual(speech["conditions"], {"cameraman": {"32783": [1]}, "maincast": {"195": [2, 1]}})
        self.assertEqual(sorted(n for names, _ in jobs for n in names),
                         sorted(["30_8572_PRO-CHRI_1_0.wav", "30_8572_PRO-CHRI_2_0.wav", "31_5706_TUT-JORD_1_0.wav", "709_40_oob_0.wav", "709_30_oob_0.wav",
                                 "709_30_oob_1.wav", "709_30_oob_2.wav", "1010_37_burn_wrap_0.wav", "1010_30_Reda_wrap_0.wav", "1010_30_Reda_wrap_1.wav"]))

    def test_the_layout_decodes_nothing(self):
        with mock.patch("ea_speech.wav", side_effect=AssertionError("decoded")):
            self.layout()

    def test_export_decodes_each_take_to_a_wav_file_and_reports_the_counts(self):
        speech, wavs, summary = c.speech_export(self.disc, self.files, self.dir / "work", vgmstream=self.cmd)
        self.assertEqual(sorted(wavs), sorted(f"speech/{n}" for n in {n for e in speech["banks"].values() for l in e.values() for x in l for n in x["files"]}
                                              | {n for v in speech["scripted"].values() for n in v}))
        self.assertTrue(all(p.read_bytes()[:4] == b"RIFF" for p in wavs.values()))
        self.assertEqual(summary["wavs"], 10)
        self.assertEqual(summary["bytes"], sum(p.stat().st_size for p in wavs.values()))
        self.assertEqual((summary["scripted"], summary["bank_entries"]), (3, 5))
        self.assertEqual(summary["scripted_missing"], ["62_5033"])
        self.assertEqual(summary["leaves_without_sample"], 1)

    def test_a_take_is_one_wav_of_the_right_length(self):
        import wave
        _, wavs, _ = c.speech_export(self.disc, self.files, self.dir / "work", vgmstream=self.cmd)
        with wave.open(str(wavs["speech/709_30_oob_2.wav"])) as w:
            self.assertEqual(w.getnframes(), 512 - 90)

    def test_no_vgmstream_is_a_refusal_not_a_traceback(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("SKATE_VGMSTREAM", None)
            with self.assertRaises(c.Refused) as ctx:
                c.speech_export(self.disc, self.files, self.dir / "work")
        self.assertEqual(ctx.exception.reason, "missing")
        self.assertIn("SKATE_VGMSTREAM", str(ctx.exception))

    def test_two_banks_making_one_wav_name_are_a_key_collision(self):
        from test_ea_speech import bank_big, evt, leaf, record
        camera = evt([record(0x800F, [leaf((1,), 0x8001), leaf((2,), 0x8000)], conditions=(1,))])
        (self.disc / "data/audio/english/cameramanspeech.big").write_bytes(
            bank_big("cameraman", camera, {"1010_30_Reda_wrap": [(0, 3), (40, 3)], "709_40_oob": [(0, 3)]}))
        with self.assertRaises(c.Refused) as ctx:
            self.layout()
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("key collision", "1010_30_Reda_wrap_0.wav"))

    def test_a_missing_bank_file_is_a_refusal(self):
        (self.disc / "data/audio/english/cameramanspeech.big").unlink()
        with self.assertRaises(c.Refused):
            self.layout()

    def test_an_id_the_table_names_but_the_bank_lacks_is_refused(self):
        self.files["r"]["speech"]["calls"]["x"] = [30, 308]          # main cast event 7: not in this synthetic .evt
        with self.assertRaises(c.Refused) as ctx:
            self.layout()
        self.assertEqual(ctx.exception.reason, "missing")

    def test_the_livingworld_bank_is_read_only_when_a_line_needs_it(self):
        self.layout()                                                  # no livingworldspeech.big on this disc, and none is wanted
        self.files["r"]["speech"]["calls"]["y"] = [39, 603]            # livingworld 0x2002
        with self.assertRaises(c.Refused):
            self.layout()


class SpeechResolveTest(SpeechDiscCase):
    """resolve_line is the reference reading of stage1_traces 1.2 / 1.5 that the Rust speech.rs ports."""

    def resolve(self, character, line, host=0x68, **kw):
        speech, _ = self.layout()
        return c.resolve_line(speech, character, line, host, **kw)

    def test_a_scripted_line_gives_its_variants_in_directory_order(self):
        self.assertEqual(self.resolve(30, 8572), ["30_8572_PRO-CHRI_1_0.wav", "30_8572_PRO-CHRI_2_0.wav"])

    def test_a_scripted_line_with_no_file_plays_nothing(self):
        self.assertEqual(self.resolve(62, 5033), [])
        self.assertEqual(self.resolve(31, 5555), [])

    def test_line_zero_plays_nothing(self):
        self.assertEqual(self.resolve(30, 0), [])

    def test_the_main_cast_picks_the_leaf_of_the_speaker(self):
        self.assertEqual(self.resolve(30, 1010), ["1010_30_Reda_wrap_0.wav", "1010_30_Reda_wrap_1.wav"])
        self.assertEqual(self.resolve(31, 1010), [])                   # the coach's bit (0x10) is in no leaf

    def test_the_cameraman_bank_follows_the_host(self):
        self.assertEqual(self.resolve(40, 709, host=0x1E), ["709_30_oob_0.wav", "709_30_oob_1.wav", "709_30_oob_2.wav"])
        self.assertEqual(self.resolve(40, 709, host=0x28), ["709_40_oob_0.wav"])
        self.assertEqual(self.resolve(40, 709, host=0x68), ["709_40_oob_0.wav"])
        self.assertEqual(self.resolve(40, 709, host=0x1F), [])         # the 4 leaf names a sample the bank lacks

    def test_an_id_in_no_table_plays_nothing(self):
        self.assertEqual(self.resolve(39, 801), [])
        self.assertEqual(self.resolve(40, 55555), [])

    def test_unknown_in_the_override_form_takes_the_override_character(self):
        self.assertEqual(self.resolve(39, 709, host=0x28, override=40), ["709_40_oob_0.wav"])
        self.assertEqual(self.resolve(39, 709, host=0x28, override=40, plain=True), [])

    def test_the_argument_picks_the_leaf_of_a_cameraman_event_that_lists_one(self):
        # a leaf's masks follow the record's condition types: the type 2 mask is the caller's argument
        speech = {"scripted": {}, "banks": {"cameraman": {"32775": [{"masks": [1, 2], "files": ["a.wav"]}, {"masks": [6, 2], "files": ["b.wav"]},
                                                                    {"masks": [1, 1], "files": ["c.wav"]}]}},
                  "conditions": {"cameraman": {"32775": [2, 1]}}}
        self.assertEqual(c.resolve_line(speech, 40, 705, 0x1E), ["a.wav", "b.wav"])
        self.assertEqual(c.resolve_line(speech, 40, 705, 0x1E, argument=1), ["a.wav"])
        self.assertEqual(c.resolve_line(speech, 40, 705, 0x1E, argument=2), ["b.wav"])
        self.assertEqual(c.resolve_line(speech, 40, 705, 0x28), ["c.wav"])


class WritePathsTest(unittest.TestCase):
    def test_a_path_in_scripts_is_copied_byte_for_byte(self):
        with tempfile.TemporaryDirectory() as d:
            source = Path(d) / "src.wav"
            source.write_bytes(b"RIFF" + bytes(range(256)) * 3)
            c.write(Path(d) / "out", {"a": 1}, scripts={"speech/x.wav": source, "speech/speech.json": {"scripted": {}}})
            self.assertEqual((Path(d) / "out/speech/x.wav").read_bytes(), source.read_bytes())
            self.assertEqual(json.loads((Path(d) / "out/speech/speech.json").read_text(encoding="utf-8")), {"scripted": {}})
            self.assertEqual(sorted(p.name for p in (Path(d) / "out/speech").iterdir()), ["speech.json", "x.wav"])

    def test_a_missing_source_writes_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(OSError):
                c.write(Path(d) / "out", {"a": 1}, scripts={"speech/x.wav": Path(d) / "absent.wav"})
            self.assertFalse((Path(d) / "out/career.json").exists())
            self.assertEqual([p for p in (Path(d) / "out").rglob("*") if p.is_file()], [])


class ExportSpeechTest(Scripted):
    def test_export_writes_speech_beside_the_career_and_reports_it(self):
        def speech(files, work):
            wav = Path(work) / "one.wav"
            wav.write_bytes(b"RIFFxxxx")
            return {"scripted": {}, "banks": {}, "conditions": {}}, {"speech/one.wav": wav}, {"wavs": 1, "bytes": 8}
        with tempfile.TemporaryDirectory() as d:
            summary = c.export(CareerFake(minimal_world(), bank()), Path(d), speech=speech)
            self.assertEqual(summary["speech"], {"wavs": 1, "bytes": 8})
            self.assertEqual(json.loads((Path(d) / "speech/speech.json").read_text(encoding="utf-8")), {"scripted": {}, "banks": {}, "conditions": {}})
            self.assertEqual((Path(d) / "speech/one.wav").read_bytes(), b"RIFFxxxx")
            self.assertTrue((Path(d) / "career.json").exists())

    def test_a_refused_speech_leaves_the_folder_as_it_was(self):
        def speech(files, work):
            raise c.Refused("missing", "cameraman", "no bank")
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"a": 1})
            with self.assertRaises(c.Refused):
                c.export(CareerFake(minimal_world(), bank()), Path(d), speech=speech)
            self.assertEqual([p.name for p in Path(d).iterdir()], ["career.json"])
            self.assertEqual(json.loads((Path(d) / "career.json").read_text(encoding="utf-8")), {"version": 1, "a": 1})

    def test_without_a_speech_reader_no_speech_folder_is_written(self):
        with tempfile.TemporaryDirectory() as d:
            c.export(CareerFake(minimal_world(), bank()), Path(d))
            self.assertFalse((Path(d) / "speech").exists())


# ---------------------------------------------------------------- the stage-1 scenes (Task 8)
def close(test, got, want, tol=0.006):
    test.assertAlmostEqual(got, want, delta=tol)


def reader_number(test, value, what):
    test.assertTrue(isinstance(value, (int, float)) and not isinstance(value, bool), f"{what}: {value!r} is not a number")


def reader_accepts(test, doc):
    """What challenge::cutscene's serde types deserialize (cutscene.rs `Scenes` -> `SceneData` -> `TrackData`, `SceneAudio`,
    `Transition`, `Clip1Meta`): a field of the wrong type fails the whole file, an unknown field is ignored (no deny_unknown_fields)."""
    test.assertIsInstance(doc["scenes"], dict)
    for name, scene in doc["scenes"].items():
        test.assertEqual(len(scene["origin"]), 3, name)                       # required
        for v in scene["origin"]:
            reader_number(test, v, name)
        for key in ("duration", "turn"):                                       # #[serde(default)]
            if key in scene:
                reader_number(test, scene[key], f"{name}.{key}")
        for track in scene["tracks"]:                                          # required
            test.assertIsInstance(track["character"], str, name)
            test.assertIsInstance(track["anim"], str, name)
            clip1 = track.get("clip1")                                         # Option<Clip1Meta>: a struct or null, never a string
            if clip1 is not None:
                test.assertIsInstance(clip1, dict, f"{name}: clip1 must be a Clip1Meta struct")
                for key in ("file", "fps", "frames", "bones", "parents"):
                    test.assertIn(key, clip1, name)
        test.assertTrue(scene.get("camera") is None or isinstance(scene["camera"], str), name)
        for audio in scene.get("audio", []):
            test.assertIsInstance(audio["audio"], str, name)
        test.assertIn(scene.get("transition"), (None, "black", "bloom"), name)
        for at, value in scene.get("s1_overlay", []):
            reader_number(test, at, name)
            test.assertIsInstance(value, int, name)


def camera_accepts(test, doc):
    """cutscene.rs `Camera`: space u8, look_space Option<u8>, samples rows of 10..=17 numbers, overlay [(f32, u8)], freeze [(f32, f32, f32)],
    shots [(f32, f32)]."""
    test.assertIsInstance(doc["space"], int)
    test.assertTrue(doc.get("look_space") is None or isinstance(doc["look_space"], int))
    for row in doc["samples"]:
        test.assertIn(len(row), range(10, 18))
        for v in row:
            reader_number(test, v, "sample")
    for at, v in doc.get("overlay", []):
        reader_number(test, at, "overlay")
        test.assertIsInstance(v, int)
    for row in doc.get("freeze", []):
        test.assertEqual(len(row), 3)
    for row in doc.get("shots", []):
        test.assertEqual(len(row), 2)


class ReaderShapeTest(unittest.TestCase):
    def good(self):
        return {"scenes": {"s": {"origin": [0, 0, 0], "duration": 5.0, "tracks": [{"character": "a", "anim": "scene/anim/a.abin"}],
                                 "camera": "scene/camera/s.json", "audio": [{"audio": "nis/a.wav", "at": 0.0}], "transition": None}}}

    def test_the_mirror_accepts_a_scene_and_refuses_a_clip_name_in_clip1(self):
        reader_accepts(self, self.good())
        bad = self.good()
        bad["scenes"]["s"]["tracks"][0]["clip1"] = "tut_sksc_01"          # the brief's {character, anim, clip1}: a clip's name is not a Clip1Meta
        with self.assertRaises(AssertionError):
            reader_accepts(self, bad)


def scene_file(scenes):
    """A scene_000.bin ('scen', stage1_nis 2): scenes = [(name, hash, kind, origin, duration, [(character, package, anim, clip)])]."""
    records, tracks, first = b"", b"", 0
    for name, h, kind, origin, duration, items in scenes:
        rec = bytearray(0x68)
        struct.pack_into(">IBxB", rec, 0, h, kind, 7)
        struct.pack_into(">7f", rec, 8, *origin, 0.0, 0.0, 0.0, duration)
        struct.pack_into(">HH", rec, 0x24, len(items), first)
        rec[0x28:0x28 + len(name)] = name.encode()
        records += bytes(rec)
        for character, package, anim, clip in items:
            t = bytearray(180)
            for at, value in ((0, character), (32, package), (48, anim), (112, clip)):
                t[at:at + len(value)] = value.encode()
            tracks += bytes(t)
        first += len(items)
    return struct.pack(">4sIII", b"scen", 1, len(scenes), 0) + records + tracks


class SceneRecordsTest(unittest.TestCase):
    def data(self):
        return scene_file([
            ("a", 0x10, 19, (0.0, 0.0, 0.0), 12.0, [("benny", "marquee", "scene/anim/a1.abin", "a_clip")]),
            ("b", 0x11, 18, (1.5, 2.5, -3.5), 25.0, [("p", "marquee", "scene/anim/b1.abin", "b_p"), ("q", "livingworld", "scene/anim/b2.abin", "b_q")]),
            ("c", 0x12, 2, (0.0, 0.0, 0.0), 12.0, []),
            ("other", 0x13, 2, (0.0, 0.0, 0.0), 1.0, [("z", "marquee", "scene/anim/z.abin", "z")])])

    def test_a_record_gives_its_kind_origin_duration_and_tracks_in_order(self):
        got = c.parse_scene_records(self.data(), ("a", "b", "c"))
        self.assertEqual(sorted(got), ["a", "b", "c"])
        self.assertEqual((got["b"]["hash"], got["b"]["kind"], got["b"]["origin"], got["b"]["duration"]), (0x11, 18, [1.5, 2.5, -3.5], 25.0))
        self.assertEqual(got["b"]["tracks"], [{"character": "p", "package": "marquee", "anim": "scene/anim/b1.abin", "clip": "b_p"},
                                              {"character": "q", "package": "livingworld", "anim": "scene/anim/b2.abin", "clip": "b_q"}])
        self.assertEqual(got["a"]["tracks"][0]["character"], "benny")
        self.assertEqual(got["c"]["tracks"], [])

    def test_a_scene_the_file_lacks_is_refused(self):
        with self.assertRaises(c.Refused) as ctx:
            c.parse_scene_records(self.data(), ("a", "missing_scene"))
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "missing_scene"))

    def test_a_scene_named_twice_is_a_key_collision(self):
        data = scene_file([("a", 1, 2, (0, 0, 0), 1.0, []), ("a", 2, 2, (0, 0, 0), 1.0, [])])
        with self.assertRaises(c.Refused) as ctx:
            c.parse_scene_records(data, ("a",))
        self.assertEqual(ctx.exception.reason, "key collision")

    def test_a_file_that_is_not_scen_is_refused(self):
        with self.assertRaises(c.Refused):
            c.parse_scene_records(b"xxxx" + self.data()[4:], ("a",))

    def test_a_track_run_past_the_file_is_refused(self):
        with self.assertRaises(c.Refused):
            c.parse_scene_records(self.data()[:-100], ("other",))


def ice_for(*rows):
    """old_tutorial's ICE section over a table of (name, track, flag) rows: floats default 0, flags 0."""
    import skate3_online as o
    return o.ice_module([dict(pos=i, name=n, track=t, flag=f, type=1 if f else 4, bits=1 if f else 32, default=0.0, min=0.0, max=1.0,
                              cubic=0xFFFF, tension=0xFFFF) for i, (n, t, f) in enumerate(rows)])


def ice_take(duration, tracks, values):
    """A decoded ICE take: tracks = {track: (index, times as fractions)}; every other track has one key."""
    keys = [([0], [0.0, 1.0]) for _ in range(37)]
    for t, k in tracks.items():
        keys[t] = k
    return {"duration": duration, "keys": keys, "values": values}


BRIGHTNESS_ROWS = (("BRIGHTNESS_OVERRIDE", 17, 1), ("BRIGHTNESS_NEAR", 17, 0), ("BRIGHTNESS_FAR", 17, 0))
DIP = {"brightness": ("BRIGHTNESS_OVERRIDE", "BRIGHTNESS_NEAR", "BRIGHTNESS_FAR")}
EYE = {"avatar_eye": ("AVATAR_EYE",)}


def dip_take():
    """03_Reda_Leaves_Shingo's brightness (stage1_nis 3.2) as the ICE decode gives it: keys at 0, 39.33, 40.92 and 43.84 s of 50, the override on
    throughout, near and far 0, 0 -> -1 -> hold -> a cut back to 0."""
    fraction = [0.0, 39.33 / 50, 40.92 / 50, 43.84 / 50, 1.0]
    return ice_take(50.0, {17: ([0, 1, 2, 4], fraction)},
                    {"BRIGHTNESS_OVERRIDE": [1, 1, 1, 1], "BRIGHTNESS_NEAR": [0.0, 0.0, -1.0, -1.0, 0.0, 0.0],
                     "BRIGHTNESS_FAR": [0.0, 0.0, -1.0, -1.0, 0.0, 0.0]})


class EffectKeysTest(unittest.TestCase):
    def keys(self, take, segment=None, effects=DIP, rows=BRIGHTNESS_ROWS):
        return c.effect_keys(ice_for(*rows), [segment or (take, 0.0, take["duration"], 0.0)], effects)

    def test_the_dip_keeps_only_the_stretch_that_changes_the_picture(self):
        got = self.keys(dip_take())["brightness"]
        self.assertEqual(got, [[39.33, 1.0, 0.0, 0.0], [40.92, 1.0, -1.0, -1.0], [43.84, 1.0, -1.0, -1.0], [43.84, 1.0, 0.0, 0.0]])

    def test_a_scene_that_never_changes_the_picture_has_no_keys(self):
        flat = ice_take(10.0, {}, {"BRIGHTNESS_OVERRIDE": [1], "BRIGHTNESS_NEAR": [0.0, 0.0], "BRIGHTNESS_FAR": [0.0, 0.0]})
        self.assertEqual(self.keys(flat)["brightness"], [])

    def test_a_stretch_of_a_shot_is_placed_on_the_edit_clock(self):
        take = dip_take()
        # the shot plays 38..45 s of the take from edit time 100: the dip starts at 39.33 - 38 + 100 = 101.33 s
        got = self.keys(take, segment=(take, 38.0, 45.0, 100.0 - 38.0))["brightness"]
        self.assertEqual([row[0] for row in got], [101.33, 102.92, 105.84, 105.84])

    def test_a_shot_that_ends_inside_the_dip_ends_on_the_value_it_reached(self):
        take = dip_take()
        got = self.keys(take, segment=(take, 0.0, 41.92, 0.0))["brightness"]
        self.assertEqual(got[0], [39.33, 1.0, 0.0, 0.0])
        self.assertEqual(got[-1], [41.92, 1.0, -1.0, -1.0])

    def test_a_constant_run_is_its_two_ends(self):
        take = ice_take(12.0, {0: ([0, 0, 0, 0], [0.0, 0.25, 0.5, 0.75, 1.0])}, {"AVATAR_EYE": [1, 1, 1, 1]})
        self.assertEqual(self.keys(take, effects=EYE, rows=(("AVATAR_EYE", 0, 1),))["avatar_eye"], [[0.0, 1.0], [12.0, 1.0]])

    def test_a_step_is_two_rows_at_one_time_and_the_default_stretches_outside_it_are_dropped(self):
        take = ice_take(12.0, {0: ([0, 0, 0], [0.0, 0.5, 0.75, 1.0])}, {"AVATAR_EYE": [0, 1, 0]})
        self.assertEqual(self.keys(take, effects=EYE, rows=(("AVATAR_EYE", 0, 1),))["avatar_eye"],
                         [[6.0, 0.0], [6.0, 1.0], [9.0, 1.0], [9.0, 0.0]])

    def test_a_default_everywhere_is_no_keys(self):
        take = ice_take(10.0, {}, {"AVATAR_EYE": [0]})
        self.assertEqual(self.keys(take, effects=EYE, rows=(("AVATAR_EYE", 0, 1),))["avatar_eye"], [])

    def test_a_parameter_the_table_lacks_is_refused(self):
        with self.assertRaises(c.Refused) as ctx:
            self.keys(dip_take(), effects={"brightness": ("BRIGHTNESS_OVERRIDE", "NOT_A_PARAMETER")})
        self.assertEqual(ctx.exception.reason, "missing")

    def test_the_effects_are_the_six_the_spec_names(self):
        self.assertEqual(sorted(c.EFFECTS), ["avatar_eye", "brightness", "dof", "far_clip", "near_clip", "shadow"])
        self.assertEqual(c.EFFECTS["brightness"], ("BRIGHTNESS_OVERRIDE", "BRIGHTNESS_NEAR", "BRIGHTNESS_FAR"))
        self.assertEqual(c.EFFECTS["near_clip"], ("NEAR_CLIP",))
        self.assertEqual(c.EFFECTS["far_clip"], ("CLIPPING_FAR",))
        self.assertEqual(c.EFFECTS["avatar_eye"], ("AVATAR_EYE",))


AUDIO_ROWS = (("AUDIO_POST_CMD", 33, 1), ("AUDIO_POST_ID", 33, 1), ("AUDIO_SPEECH_CMD", 32, 1), ("AUDIO_SPEECH_ID", 32, 1),
              ("AUDIO_SPEECH_CHARACTER", 32, 1))


def audio_take(duration=12.0, post=None, speech=None):
    """post = [(key start, cmd, id)], speech = [(key start, cmd, id, character)]: the first key starts at 0, as the ICE decode has it."""
    values = {"AUDIO_POST_CMD": [], "AUDIO_POST_ID": [], "AUDIO_SPEECH_CMD": [], "AUDIO_SPEECH_ID": [], "AUDIO_SPEECH_CHARACTER": []}
    tracks = {}
    if post:
        values["AUDIO_POST_CMD"], values["AUDIO_POST_ID"] = [p[1] for p in post], [p[2] for p in post]
        tracks[33] = ([0] * len(post), [p[0] / duration for p in post] + [1.0])
    if speech:
        values["AUDIO_SPEECH_CMD"] = [s[1] for s in speech]
        values["AUDIO_SPEECH_ID"] = [s[2] for s in speech]
        values["AUDIO_SPEECH_CHARACTER"] = [s[3] for s in speech]
        tracks[32] = ([0] * len(speech), [s[0] / duration for s in speech] + [1.0])
    return ice_take(duration, tracks, values)


class AudioKeysTest(unittest.TestCase):
    def keys(self, takes, edit=False):
        return c.audio_keys(ice_for(*AUDIO_ROWS), takes, edit)

    def test_a_post_key_with_command_two_gives_its_id_at_its_time(self):
        self.assertEqual(self.keys([audio_take(post=[(0.0, 2, 5001)])]), ([(0.0, 5001)], []))

    def test_a_speech_key_gives_its_id_and_character_at_its_time(self):
        posts, speech = self.keys([audio_take(post=[(0.0, 0, 8756)], speech=[(0.0, 0, 5000, 0), (0.16, 2, 8756, 40)])])
        self.assertEqual(posts, [])                                   # POST_CMD 0 plays nothing
        self.assertEqual(speech, [(0.16, 8756, 40)])

    def test_the_same_id_keyed_in_several_takes_plays_once(self):
        posts, _ = self.keys([audio_take(post=[(0.0, 2, 5001)]), audio_take(post=[(0.0, 2, 5001)])], edit=True)
        self.assertEqual(posts, [(0.0, 5001)])

    def test_a_later_key_in_an_edit_is_refused(self):
        with self.assertRaises(c.Refused) as ctx:
            self.keys([audio_take(post=[(3.0, 2, 5001)])], edit=True)
        self.assertEqual(ctx.exception.reason, "missing")


def wav_bytes(channels, frames=5, rate=48000, width=2, sample=lambda k: 1000 * (k + 1)):
    import io
    import wave
    out = io.BytesIO()
    with wave.open(out, "wb") as w:
        w.setnchannels(channels)
        w.setsampwidth(width)
        w.setframerate(rate)
        w.writeframes(b"".join(struct.pack("<h", sample(k)) if width == 2 else bytes([k]) for _ in range(frames) for k in range(channels)))
    return out.getvalue()


def wav_first_frame(data):
    import io
    import wave
    with wave.open(io.BytesIO(data)) as w:
        return w.getnchannels(), w.getframerate(), w.getnframes(), struct.unpack(f"<{w.getnchannels()}h", w.readframes(1))


class SceneStreamTest(unittest.TestCase):
    VARIANTS = {(99, 5001): ["99_5001_A_1"], (30, 6774): ["30_6774_B_1", "30_6774_B_2"]}

    def test_the_stream_of_an_id_is_its_one_variant(self):
        self.assertEqual(c.scene_stream(self.VARIANTS, 99, 5001, "s"), "99_5001_A_1")

    def test_an_id_with_no_stream_is_none(self):
        self.assertIsNone(c.scene_stream(self.VARIANTS, 99, 6774, "s"))

    def test_an_id_with_several_streams_is_refused_not_guessed(self):
        with self.assertRaises(c.Refused) as ctx:
            c.scene_stream(self.VARIANTS, 30, 6774, "photo")
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("key collision", "30_6774"))


class FoldWavTest(unittest.TestCase):
    def test_mono_and_stereo_stay_as_vgmstream_wrote_them(self):
        for channels in (1, 2):
            data = wav_bytes(channels)
            self.assertEqual(c.fold_wav(data), data)

    def test_five_channels_fold_to_the_retail_stereo(self):
        # skate3_online.fold_stereo: L C R Ls Rs -> (L + .7071 C + .7071 Ls, R + .7071 C + .7071 Rs) x .7071
        channels, rate, frames, first = wav_first_frame(c.fold_wav(wav_bytes(5)))
        self.assertEqual((channels, rate, frames), (2, 48000, 5))
        k = 0.70710678
        self.assertEqual(first, (round((1000 + k * 2000 + k * 4000) * k), round((3000 + k * 2000 + k * 5000) * k)))

    def test_a_loud_fold_is_clamped_not_wrapped(self):
        self.assertEqual(wav_first_frame(c.fold_wav(wav_bytes(3, sample=lambda k: 32767)))[3], (32767, 32767))

    def test_a_wav_that_is_not_16_bit_is_refused(self):
        with self.assertRaises(c.Refused):
            c.fold_wav(wav_bytes(3, width=1))


class SubtitleCuesTest(unittest.TestCase):
    VALUES = {1: "first^line", 2: "second \x81 cue", 3: "third"}

    def sub(self, *records):
        return b"".join(struct.pack("<HHI", t, 0, k) for t, k in records) + struct.pack("<HHI", 0xFFFF, 0, 0x1A20BA)

    def test_a_cue_runs_to_the_next_record_and_a_blank_ends_it(self):
        raw = self.sub((0, 1), (44, 2), (157, 0x1A20BA))
        self.assertEqual(c.subtitle_cues(raw, self.VALUES, 22.3), [[0.0, 4.4, "first\nline"], [4.4, 15.7, "second ’ cue"]])

    def test_a_gap_between_cues_is_kept(self):
        raw = self.sub((20, 1), (60, 0x1A20BA), (100, 3), (150, 0x1A20BA))
        self.assertEqual(c.subtitle_cues(raw, self.VALUES, 9.0), [[2.0, 6.0, "first\nline"], [10.0, 15.0, "third"]])

    def test_a_last_cue_with_no_blank_runs_to_the_scene_s_end(self):
        self.assertEqual(c.subtitle_cues(self.sub((0, 3)), self.VALUES, 7.5), [[0.0, 7.5, "third"]])

    def test_a_key_no_table_holds_is_refused(self):
        with self.assertRaises(c.Refused) as ctx:
            c.subtitle_cues(self.sub((0, 99)), self.VALUES, 5.0)
        self.assertEqual(ctx.exception.reason, "missing")

    def test_a_file_with_only_the_terminator_has_no_cues(self):
        self.assertEqual(c.subtitle_cues(self.sub(), self.VALUES, 5.0), [])


class ExportScenesTest(Scripted):
    def part(self, files, summary=None):
        return lambda work: (files, summary or {"scenes": 1})

    def test_export_writes_the_scene_files_beside_the_career_and_reports_them(self):
        with tempfile.TemporaryDirectory() as d:
            wav = Path(d) / "a.wav"
            wav.write_bytes(b"RIFFxxxx")
            files = {"scenes/tutorial.json": {"scenes": {}}, "scenes/scene/camera/s.json": '{"space":1}\n', "scenes/nis/a.wav": wav}
            out = Path(d) / "out"
            summary = c.export(CareerFake(minimal_world(), bank()), out, scenes=self.part(files, {"scenes": 10}))
            self.assertEqual(summary["scenes"], {"scenes": 10})
            self.assertEqual(json.loads((out / "scenes/tutorial.json").read_text(encoding="utf-8")), {"scenes": {}})
            self.assertEqual((out / "scenes/nis/a.wav").read_bytes(), b"RIFFxxxx")
            self.assertEqual((out / "scenes/scene/camera/s.json").read_text(encoding="utf-8"), '{"space":1}\n')
            self.assertTrue((out / "career.json").exists())

    def test_a_refused_scene_part_leaves_the_folder_as_it_was(self):
        def scenes(work):
            raise c.Refused("missing", "tut_sksc_01", "no camera")
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"a": 1})
            with self.assertRaises(c.Refused):
                c.export(CareerFake(minimal_world(), bank()), Path(d), scenes=scenes)
            self.assertEqual([p.name for p in Path(d).iterdir()], ["career.json"])

    def test_without_a_scene_part_no_scenes_folder_is_written(self):
        with tempfile.TemporaryDirectory() as d:
            summary = c.export(CareerFake(minimal_world(), bank()), Path(d))
            self.assertFalse((Path(d) / "scenes").exists())
            self.assertIsNone(summary["scenes"])


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class SceneDiscTest(unittest.TestCase):
    """Task 8 on the real disc (stage1_nis 3.1-3.5): scenes/tutorial.json, the cameras with their effects, the actor banks, the subtitles."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-scenes-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.disc = Path(os.environ["SKATE3_DISC"])
        cls.tutorial, cls.files, cls.jobs, cls.summary = c.scene_layout(cls.disc, Path(cls.tmp.name))
        cls.scenes = cls.tutorial["scenes"]

    def camera(self, name):
        return json.loads(Path(self.files[f"scenes/scene/camera/{name}.json"]).read_text(encoding="utf-8"))

    def test_twelve_scenes_export_with_actors(self):
        """The brief counts 12; the scenes spec 10 names are the nine NIS plus Create_start_intro = 10 (stage1_nis 1: M1 and M2 are movies,
        01_Reda_Welcome and 06_Coach_Frank_Intro are never played)."""
        self.assertEqual(sorted(self.scenes), sorted(c.SCENES))
        self.assertEqual(len(self.scenes), 10)
        actors = {name: len(scene["tracks"]) for name, scene in self.scenes.items()}
        self.assertEqual(actors, {"tut_sksc_01": 1, "tut_sksc_02": 1, "tut_sksc_06": 1, "tut_sksc_03": 1, "tut_sksc_04": 0, "photo_univ_08": 0,
                                  "film_univ_05": 0, "02_Reda_Passes_Baton": 9, "03_Reda_Leaves_Shingo": 3, "Create_start_intro": 0})
        baton = self.scenes["02_Reda_Passes_Baton"]
        self.assertEqual([(t["character"], t["package"]) for t in baton["tracks"]],
                         [("current_player", "marquee"), ("giovanni_reda_prop", "marquee"), ("shingo_prop", "marquee"), ("female_adult_2", "livingworld"),
                          ("male_adult_1", "livingworld"), ("male_adult_2", "livingworld"), ("male_adult_2", "livingworld"), ("ai_skater_04", "marquee"),
                          ("ai_skater_03", "marquee")])
        self.assertEqual([t["character"] for t in self.scenes["03_Reda_Leaves_Shingo"]["tracks"]], ["current_player", "giovanni_reda_prop", "shingo"])
        close(self, baton["origin"][0], 77.831)
        close(self, baton["origin"][1], 63.943)
        close(self, baton["origin"][2], -229.474)
        close(self, baton["duration"], 25.0)
        self.assertEqual(self.scenes["tut_sksc_01"]["origin"], [0.0, 0.0, 0.0])
        for name, scene in self.scenes.items():
            for track in scene["tracks"]:
                self.assertNotIn("clip1", track, name)             # Option<Clip1Meta>: the clip's name is "clip"
                self.assertTrue(track["clip"] and track["anim"].startswith("scene/anim/") and track["anim"].endswith(".abin"), (name, track))
                self.assertIn("scenes/" + track["anim"], self.files, (name, track))
        # the player's bank has its female take beside it (the reader swaps _Male1 for _Female1)
        self.assertIn("scenes/scene/anim/02_PassesBaton_Female1.abin", self.files)
        self.assertIn("scenes/scene/anim/03_RedaLeaves_Female1.abin", self.files)
        reader_accepts(self, self.tutorial)
        for name in c.SCENES:
            camera_accepts(self, self.camera(name))
            self.assertEqual(self.scenes[name]["camera"], f"scene/camera/{name}.json")
        self.assertEqual(self.scenes["tut_sksc_01"]["transition"], "black")
        self.assertIsNone(self.scenes["film_univ_05"]["transition"])

    def test_reda_leaves_shingo_dips_brightness(self):
        rows = self.camera("03_Reda_Leaves_Shingo")["effects"]["brightness"]
        close(self, rows[0][0], 39.33)
        close(self, rows[-1][0], 43.84)
        self.assertTrue(all(row[1] == 1 for row in rows))
        self.assertEqual(min(row[2] for row in rows), -1.0)
        low = [row[0] for row in rows if row[2] == -1.0]
        close(self, low[0], 40.92)
        close(self, low[-1], 43.84)
        self.assertEqual(rows[-1][2:], [0.0, 0.0])                   # the cut back
        self.assertEqual(self.camera("02_Reda_Passes_Baton")["effects"]["brightness"], [])

    def test_tut_sksc_06_keeps_its_avatar_eye(self):
        rows = self.camera("tut_sksc_06")["effects"]["avatar_eye"]
        self.assertTrue(rows and all(row[1] == 1 for row in rows))
        self.assertEqual(rows[0][0], 0.0)
        close(self, rows[-1][0], 11.27, 0.02)                         # the edit's tail (stage1_nis 3.2)
        for name in c.SCENES:
            if name != "tut_sksc_06":
                self.assertEqual(self.camera(name)["effects"]["avatar_eye"], [], name)

    def test_every_camera_carries_the_six_effects_and_their_columns(self):
        for name in c.SCENES:
            camera = self.camera(name)
            self.assertEqual(sorted(camera["effects"]), sorted(c.EFFECTS), name)
            self.assertEqual({k: tuple(v) for k, v in camera["effect_columns"].items()}, {k: ("t", *v) for k, v in c.EFFECTS.items()})
            for key, rows in camera["effects"].items():
                self.assertTrue(all(len(row) == 1 + len(c.EFFECTS[key]) for row in rows), (name, key))
        self.assertTrue(self.camera("tut_sksc_01")["effects"]["far_clip"])      # CLIPPING_FAR 0.538 in take 0 (stage1_nis 3.2)
        self.assertTrue(self.camera("03_Reda_Leaves_Shingo")["effects"]["shadow"])

    def test_the_cameras_are_the_online_exporters_cameras(self):
        cam = self.camera("03_Reda_Leaves_Shingo")
        self.assertEqual(cam["space"], 1)
        close(self, cam["samples"][-1][0], 50.0, 0.02)
        self.assertEqual(len(cam["samples"][0]), 17)
        self.assertEqual(self.camera("tut_sksc_01")["shots"][0][0], 0.0)    # the edit's shots

    def test_scenes_carry_their_sub_cues(self):
        counts = {name: len(scene["subtitles"]) for name, scene in self.scenes.items()}
        self.assertEqual(counts, {"tut_sksc_01": 2, "tut_sksc_02": 2, "tut_sksc_06": 2, "tut_sksc_03": 2, "tut_sksc_04": 2, "photo_univ_08": 1,
                                  "film_univ_05": 1, "02_Reda_Passes_Baton": 4, "03_Reda_Leaves_Shingo": 9, "Create_start_intro": 0})
        starts = {n: [cue[0] for cue in s["subtitles"]] for n, s in self.scenes.items()}
        ends = {n: [cue[1] for cue in s["subtitles"]] for n, s in self.scenes.items()}
        self.assertEqual((starts["tut_sksc_01"], ends["tut_sksc_01"]), ([0.0, 4.4], [4.4, 15.7]))
        self.assertEqual(starts["02_Reda_Passes_Baton"], [2.0, 6.0, 11.4, 22.0])
        self.assertEqual(ends["02_Reda_Passes_Baton"], [6.0, 11.4, 22.0, 25.4])
        self.assertEqual(starts["03_Reda_Leaves_Shingo"], [0.8, 8.7, 13.2, 16.2, 21.5, 27.2, 30.0, 36.0, 42.0])
        self.assertEqual(ends["03_Reda_Leaves_Shingo"], [8.7, 13.2, 16.2, 21.5, 27.2, 30.0, 34.5, 42.0, 49.5])
        self.assertEqual(ends["photo_univ_08"], [11.2])                 # outlasts its 10 s scene: the player cuts it there (spec 15.5)
        for scene in self.scenes.values():
            for start, end, text in scene["subtitles"]:
                self.assertTrue(isinstance(text, str) and text.strip() and 0 <= start < end)

    def test_the_audio_keys_pick_the_streams_by_id(self):
        wanted = {"tut_sksc_01": [("post", "99_5001_TUT-JORD-NIS_1", 0.0)], "tut_sksc_02": [("post", "99_5040_TUT-JORD-NIS_3", 0.0)],
                  "tut_sksc_06": [("post", "99_5231_TUT-JORD_2", 0.0)], "tut_sksc_03": [("post", "99_5089_TUT-JORD-NIS_1", 0.0)],
                  "tut_sksc_04": [("post", "99_5132_TUT-JORD-NIS_3", 0.0)], "photo_univ_08": [("speech", "30_6774_PHO-JACK-NIS_1", 0.0)],
                  "film_univ_05": [("speech", "30_7762_FIL-JACK-NIS_1", 0.0)], "02_Reda_Passes_Baton": [("post", "99_6013_PRO-PHIL-NIS_1", 0.0)],
                  "03_Reda_Leaves_Shingo": [("post", "99_6020_PRO-PHIL-NIS_1", 0.0)], "Create_start_intro": [("speech", "40_8756_TUT-CHAL-NIS_1", 0.16)]}
        got = {n: [(a["kind"], Path(a["audio"]).stem, a["at"]) for a in s["audio"]] for n, s in self.scenes.items()}
        self.assertEqual({n: [(k, stem) for k, stem, _ in v] for n, v in got.items()}, {n: [(k, stem) for k, stem, _ in v] for n, v in wanted.items()})
        for n, v in got.items():
            for (_, stem, at), (_, _, want) in zip(v, wanted[n]):
                close(self, at, want, 0.01)       # the speech key is at 0.155 s (spec 15.3 rounds it to 0.16); every other key at 0
        self.assertEqual(self.scenes["photo_univ_08"]["audio"][0]["character"], 30)
        self.assertEqual(self.scenes["tut_sksc_01"]["audio"][0]["character"], 99)
        close(self, self.scenes["tut_sksc_01"]["audio"][0]["seconds"], 13.226, 0.001)
        self.assertEqual(len(self.jobs), 10)

    def test_the_summary_counts_what_was_laid_out(self):
        self.assertEqual(self.summary["scenes"], 10)
        self.assertEqual(self.summary["actors"]["02_Reda_Passes_Baton"], 9)
        self.assertEqual(self.summary["cues"]["03_Reda_Leaves_Shingo"], 9)
        self.assertEqual(self.summary["audio_files"], 10)


@unittest.skipUnless(os.environ.get("SKATE3_DISC") and os.environ.get("SKATE_VGMSTREAM"),
                     "set SKATE3_DISC and SKATE_VGMSTREAM (vgmstream-cli)")
class SceneBedsDataTest(unittest.TestCase):
    """The decoded beds: every scene's audio file exists, is a 16-bit WAV of the stream's length, and has at most two channels."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-scenes-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.files, cls.summary = c.scene_export(Path(os.environ["SKATE3_DISC"]), Path(cls.tmp.name))
        cls.scenes = cls.files["scenes/tutorial.json"]["scenes"]

    def test_each_scene_finds_its_bed(self):
        import wave
        for name, scene in self.scenes.items():
            self.assertTrue(scene["audio"], name)
            for audio in scene["audio"]:
                path = self.files["scenes/" + audio["audio"]]
                self.assertTrue(Path(path).is_file(), (name, audio))
                with wave.open(str(path)) as w:
                    self.assertIn(w.getnchannels(), (1, 2), (name, audio))
                    self.assertEqual(w.getsampwidth(), 2)
                    close(self, w.getnframes() / w.getframerate(), audio["seconds"], 0.001)

    def test_the_summary_counts_the_wavs_and_their_bytes(self):
        wavs = [p for k, p in self.files.items() if k.startswith("scenes/nis/")]
        self.assertEqual(len(wavs), 10)
        self.assertEqual(self.summary["audio_files"], 10)
        self.assertEqual(self.summary["audio_bytes"], sum(Path(p).stat().st_size for p in wavs))
        for p in wavs:
            self.assertEqual(Path(p).read_bytes()[:4], b"RIFF")


# ---------------------------------------------------------------- the menu movies and the Follow the Leader paths (Task 10)
MOVIES = ("endchallengeoptions endchallenge_small challengefailed_small photoselect skate101 unlockoverlay unlock_milestone unlock_teammate "
          "challenge_signup hudtutorial spawnskater freeskate_options sk8popup").split()


def fake_cache(root, movies, textures=None, unresolved=(), kind="skate3-retail-screen"):
    """A made-up screens cache (what prepare_core_menu.prepare leaves): runtime/<movie>.json naming one 1 x 1 texture each."""
    for name in movies:
        texture = f"assets/{name}/0.rgba"
        (root / texture).parent.mkdir(parents=True, exist_ok=True)
        (root / texture).write_bytes(b"\x01\x02\x03\x04")
        manifest = {"format": kind, "version": 1, "unresolved_fonts": list(unresolved), "fonts": {}, "backdrop": None,
                    "shapes": {"1": [{"texture": {"rgba": texture, "width": 1, "height": 1}}]}}
        (root / f"runtime/{name}.json").parent.mkdir(parents=True, exist_ok=True)
        (root / f"runtime/{name}.json").write_text(json.dumps(manifest), encoding="utf-8")
    shared = root / "runtime/white.rgba"
    shared.write_bytes(b"\xff\xff\xff\xff")


class ScreenTableTest(unittest.TestCase):
    def test_thirteen_movies_each_a_screen_under_data_fe_source(self):
        # presentation 1.5 (screens 23 challenge_signup, 33 endchallengeoptions, 34 endchallenge_small, 36 challengefailed_small, 37 skate101,
        # 89 photoselect, 109 unlockoverlay, 111 unlock_milestone, 112 unlock_teammate), freeplay 2.4 (spawnskater), 5.1 (freeskate_options),
        # the spec's screen 50 (hud2/hudtutorial) and the popup movie sk8popup.
        self.assertEqual(sorted(c.SCREEN_MOVIES), sorted(MOVIES))
        for name, path in c.SCREEN_MOVIES.items():
            self.assertTrue(path.startswith("screens/") and path.endswith("/" + name), (name, path))
        self.assertEqual(c.SCREEN_MOVIES["hudtutorial"], "screens/hud2/hudtutorial")
        self.assertEqual(c.SCREEN_MOVIES["spawnskater"], "screens/cas/spawnskater")
        self.assertEqual(c.SCREEN_MOVIES["freeskate_options"], "screens/main/freeskate_options")
        self.assertEqual(c.SCREEN_MOVIES["challenge_signup"], "screens/challenge/challenge_signup")
        self.assertEqual(c.SCREEN_MOVIES["endchallengeoptions"], "screens/hudComponents/endchallengeoptions")


class ScreenFilesTest(unittest.TestCase):
    def test_the_files_are_every_manifest_and_texture_under_screens(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fake_cache(root, ["skate101", "photoselect"])
            files = c.screen_files(root, ["skate101", "photoselect"])
            self.assertEqual(sorted(files), ["screens/assets/photoselect/0.rgba", "screens/assets/skate101/0.rgba",
                                             "screens/runtime/photoselect.json", "screens/runtime/skate101.json"])
            for rel, path in files.items():
                self.assertEqual(Path(path), root / rel.removeprefix("screens/"))

    def test_a_texture_two_movies_share_is_one_file(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fake_cache(root, ["a", "b"])
            for name in ("a", "b"):
                manifest = json.loads((root / f"runtime/{name}.json").read_text(encoding="utf-8"))
                manifest["shapes"]["2"] = [{"texture": {"rgba": "runtime/white.rgba", "width": 1, "height": 1}}]
                (root / f"runtime/{name}.json").write_text(json.dumps(manifest), encoding="utf-8")
            files = c.screen_files(root, ["a", "b"])
            self.assertEqual(sorted(k for k in files if "white" in k), ["screens/runtime/white.rgba"])

    def test_an_unresolved_font_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            fake_cache(Path(d), ["skate101"], unresolved=["Mystery"])
            with self.assertRaises(c.Refused) as ctx:
                c.screen_files(Path(d), ["skate101"])
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("conversion", "skate101"))

    def test_a_missing_or_wrong_size_texture_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            fake_cache(Path(d), ["skate101"])
            (Path(d) / "assets/skate101/0.rgba").write_bytes(b"\0" * 3)
            with self.assertRaises(c.Refused):
                c.screen_files(Path(d), ["skate101"])
            (Path(d) / "assets/skate101/0.rgba").unlink()
            with self.assertRaises(c.Refused):
                c.screen_files(Path(d), ["skate101"])

    def test_a_movie_that_was_not_prepared_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            fake_cache(Path(d), ["skate101"])
            with self.assertRaises(c.Refused) as ctx:
                c.screen_files(Path(d), ["skate101", "photoselect"])
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("missing", "photoselect"))


class ScreensPartTest(unittest.TestCase):
    """screens_part drives prepare_core_menu.prepare once per movie, as prepare_retail_screens.prepare does (here prepare writes a made-up cache)."""

    def setUp(self):
        import prepare_core_menu
        import prepare_retail_screens
        self.calls = []

        def prepare(game, output, collections, path, rel, kind):
            self.calls.append((Path(game), Path(output), Path(collections), path, rel, kind))
            fake_cache(Path(output), [Path(rel).stem])
            return {**json.loads((Path(output) / rel).read_text(encoding="utf-8")), "characters": [], "font_mappings": {}}
        for target, attr, fake in ((prepare_core_menu, "prepare", prepare),
                                   (prepare_retail_screens, "_glyphs", lambda game, output: {})):
            patch = mock.patch.object(target, attr, fake)
            patch.start()
            self.addCleanup(patch.stop)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.assets = Path(self.tmp.name) / "assets"
        (self.assets / "private/stock").mkdir(parents=True)
        (self.assets / "private/stock/skater-collections.json").write_text("{}", encoding="utf-8")

    def test_each_movie_is_prepared_from_its_path_into_runtime(self):
        files, summary = c.screens_part(Path("disc"), self.assets, Path(self.tmp.name) / "work")
        self.assertEqual([x[3] for x in self.calls], ["data/fe/source/" + p for p in c.SCREEN_MOVIES.values()])
        self.assertEqual([x[4] for x in self.calls], [f"runtime/{n}.json" for n in c.SCREEN_MOVIES])
        self.assertEqual({x[5] for x in self.calls}, {"skate3-retail-screen"})
        self.assertEqual({x[0] for x in self.calls}, {Path("disc")})
        self.assertEqual({x[2] for x in self.calls}, {self.assets / "private/stock/skater-collections.json"})
        for name in c.SCREEN_MOVIES:
            self.assertTrue(Path(files[f"screens/runtime/{name}.json"]).is_file(), name)
            self.assertNotIn(b"\r", Path(files[f"screens/runtime/{name}.json"]).read_bytes(), name)
        self.assertEqual(summary["movies"], 13)
        self.assertEqual(summary["files"], len(files))
        self.assertEqual(summary["bytes"], sum(Path(p).stat().st_size for p in files.values()))

    def test_no_stock_collections_is_refused_before_anything_is_prepared(self):
        (self.assets / "private/stock/skater-collections.json").unlink()
        with self.assertRaises(c.Refused) as ctx:
            c.screens_part(Path("disc"), self.assets, Path(self.tmp.name) / "work")
        self.assertEqual(ctx.exception.reason, "missing")
        self.assertEqual(self.calls, [])

    def test_a_movie_that_cannot_be_merged_is_refused_by_name(self):
        import prepare_core_menu

        def failing(game, output, collections, path, rel, kind):
            raise ValueError("Unresolved core-menu shape")
        with mock.patch.object(prepare_core_menu, "prepare", failing):
            with self.assertRaises(c.Refused) as ctx:
                c.screens_part(Path("disc"), self.assets, Path(self.tmp.name) / "work")
        self.assertEqual((ctx.exception.reason, ctx.exception.name), ("conversion", "endchallengeoptions"))

    def test_the_prepare_hook_is_put_back(self):
        import prepare_core_menu
        before = prepare_core_menu.extract_project
        c.screens_part(Path("disc"), self.assets, Path(self.tmp.name) / "work")
        self.assertIs(prepare_core_menu.extract_project, before)


class ExportScreensTest(Scripted):
    def part(self, files, summary=None):
        return lambda work: (files, summary or {"movies": 1})

    def test_export_writes_the_screens_beside_the_career_and_reports_them(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "cache"
            root.mkdir()
            fake_cache(root, ["skate101"])
            out = Path(d) / "out"
            summary = c.export(CareerFake(minimal_world(), bank()), out, screens=self.part(c.screen_files(root, ["skate101"]), {"movies": 1}))
            self.assertEqual(summary["screens"], {"movies": 1})
            self.assertEqual(json.loads((out / "screens/runtime/skate101.json").read_text(encoding="utf-8"))["format"], "skate3-retail-screen")
            self.assertEqual((out / "screens/assets/skate101/0.rgba").read_bytes(), b"\x01\x02\x03\x04")
            self.assertTrue((out / "career.json").exists())

    def test_a_refused_screens_part_leaves_the_folder_as_it_was(self):
        def screens(work):
            raise c.Refused("conversion", "skate101", "no font")
        with tempfile.TemporaryDirectory() as d:
            c.write(Path(d), {"a": 1})
            with self.assertRaises(c.Refused):
                c.export(CareerFake(minimal_world(), bank()), Path(d), screens=screens)
            self.assertEqual([p.name for p in Path(d).iterdir()], ["career.json"])

    def test_without_a_screens_part_no_screens_folder_is_written(self):
        with tempfile.TemporaryDirectory() as d:
            summary = c.export(CareerFake(minimal_world(), bank()), Path(d))
            self.assertFalse((Path(d) / "screens").exists())
            self.assertIsNone(summary["screens"])


class AiPartTest(Scripted):
    """ai_part gives skate3_ai_paths the two Follow the Leader rows; export writes what it returns beside the rest."""

    def row(self, name, tricks=(), anomalies=(), missing=()):
        paths = [{"name": f"ch_{name}_1", "tricks": [{"node": i, "trick": t} for i, t in enumerate(tricks)]}]
        return {"row": name, "paths": paths, "coverage": {"anomalies": list(anomalies), "branch_targets_missing": list(missing)}}

    def test_the_rows_are_the_two_stage_one_ftl_rows(self):
        self.assertEqual(c.AI_ROWS, ("ftl_univ_02", "ftl_univ_03"))

    def test_ai_part_writes_a_file_per_row_and_the_trick_ids(self):
        import skate3_ai_paths as ai
        outs = {r: self.row(r, (5, 3)) for r in c.AI_ROWS}
        with mock.patch.object(ai, "export_rows", return_value=(outs, {})) as rows:
            files, summary = c.ai_part(Path("disc"), Path("work"))
        self.assertEqual(rows.call_args.args[0], Path("disc"))
        self.assertEqual(list(rows.call_args.args[2]), ["ftl_univ_02", "ftl_univ_03"])
        self.assertEqual(sorted(files), ["ai/ftl_univ_02.json", "ai/ftl_univ_03.json", "ai/tricks.json"])
        self.assertEqual(json.loads(files["ai/tricks.json"]), [3, 5])
        self.assertEqual(summary, {"rows": 2, "paths": 2, "tricks": 2})

    def test_a_row_with_no_file_is_refused(self):
        import skate3_ai_paths as ai
        with mock.patch.object(ai, "export_rows", side_effect=KeyError("ftl_univ_02: no .rx2")):
            with self.assertRaises(c.Refused) as ctx:
                c.ai_part(Path("disc"), Path("work"))
        self.assertEqual(ctx.exception.reason, "missing")

    def test_a_damaged_file_or_an_inconsistent_decode_is_refused(self):
        import skate3_ai_paths as ai
        with mock.patch.object(ai, "export_rows", side_effect=ValueError("rx2 truncated")):
            with self.assertRaises(c.Refused) as ctx:
                c.ai_part(Path("disc"), Path("work"))
        self.assertEqual(ctx.exception.reason, "conversion")
        for bad in (self.row("ftl_univ_02", anomalies=["ch_x: branch 0: node 9 of 3"]), self.row("ftl_univ_02", missing=["ch_y"])):
            with mock.patch.object(ai, "export_rows", return_value=({"ftl_univ_02": bad, "ftl_univ_03": self.row("ftl_univ_03")}, {})):
                with self.assertRaises(c.Refused) as ctx:
                    c.ai_part(Path("disc"), Path("work"))
            self.assertEqual((ctx.exception.reason, ctx.exception.name), ("conversion", "ftl_univ_02"))

    def test_export_writes_the_ai_files_and_a_refusal_writes_nothing(self):
        files = {"ai/ftl_univ_02.json": "{}\n", "ai/tricks.json": "[1]\n"}
        with tempfile.TemporaryDirectory() as d:
            summary = c.export(CareerFake(minimal_world(), bank()), Path(d), ai=lambda work: (files, {"rows": 1}))
            self.assertEqual(summary["ai"], {"rows": 1})
            self.assertEqual((Path(d) / "ai/tricks.json").read_text(encoding="utf-8"), "[1]\n")
            self.assertEqual(c.export(CareerFake(minimal_world(), bank()), Path(d) / "other")["ai"], None)
            self.assertFalse((Path(d) / "other/ai").exists())

            def refused(work):
                raise c.Refused("missing", "ftl_univ_02", "no file")
            c.write(Path(d) / "third", {"a": 1})
            with self.assertRaises(c.Refused):
                c.export(CareerFake(minimal_world(), bank()), Path(d) / "third", ai=refused)
            self.assertEqual([p.name for p in (Path(d) / "third").iterdir()], ["career.json"])


@unittest.skipUnless(os.environ.get("SKATE3_DISC") and os.environ.get("SKATE3_ASSETS"),
                     "set SKATE3_DISC and SKATE3_ASSETS (the install's assets folder, read only)")
class ScreensDataTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="skate3-screens-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.files, cls.summary = c.screens_part(Path(os.environ["SKATE3_DISC"]), Path(os.environ["SKATE3_ASSETS"]), Path(cls.tmp.name))

    def test_every_movie_has_a_manifest_the_runtime_can_validate(self):
        import prepare_core_menu
        for name in c.SCREEN_MOVIES:
            path = Path(self.files[f"screens/runtime/{name}.json"])
            manifest = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual((manifest["format"], manifest["version"], manifest["unresolved_fonts"]), ("skate3-retail-screen", 1, []), name)
            self.assertTrue(manifest["characters"] and manifest["bundles"][0] == (c.SCREEN_SOURCE + c.SCREEN_MOVIES[name]).lower(), name)
            # The files the manifest names are all in the export, at the same relative paths.
            for rel in prepare_core_menu.runtime_files(path.parents[1], f"runtime/{name}.json", "skate3-retail-screen"):
                self.assertIn("screens/" + rel, self.files, (name, rel))

    def test_the_summary_counts_the_movies_and_bytes(self):
        self.assertEqual(self.summary["movies"], 13)
        self.assertEqual(sum(1 for k in self.files if k.startswith("screens/runtime/") and k.endswith(".json")), 13)
        self.assertEqual(self.summary["bytes"], sum(Path(p).stat().st_size for p in self.files.values()))

    def test_the_menu_movies_read_the_same_as_the_installed_ones(self):
        # sk8popup is also in retail-screens: the same movie, merged the same way, gives the same manifest.
        installed = Path(os.environ["SKATE3_ASSETS"]) / "private/retail-screens/runtime/sk8popup.json"
        if not installed.is_file():
            self.skipTest("the install has no retail-screens/sk8popup.json")
        mine = json.loads(Path(self.files["screens/runtime/sk8popup.json"]).read_text(encoding="utf-8"))
        theirs = json.loads(installed.read_text(encoding="utf-8"))
        for manifest in (mine, theirs):      # each font records the work folder its bank was extracted to: the one difference
            for font in manifest["fonts"].values():
                font["definition"].pop("source", None)
        self.assertEqual(mine, theirs)


@unittest.skipUnless(os.environ.get("SKATE3_DISC"), "set SKATE3_DISC to the extracted Skate 3 disc folder")
class AiDataTest(unittest.TestCase):
    def test_ai_part_on_the_disc(self):
        files, summary = c.ai_part(Path(os.environ["SKATE3_DISC"]), Path(tempfile.gettempdir()))
        self.assertEqual(sorted(files), ["ai/ftl_univ_02.json", "ai/ftl_univ_03.json", "ai/tricks.json"])
        self.assertEqual((summary["rows"], summary["paths"]), (2, 17))
        ids = json.loads(files["ai/tricks.json"])
        self.assertEqual(ids, sorted(set(ids)))
        self.assertTrue(ids and ids[0] >= 0 and ids[-1] < 332)
        ftl3 = json.loads(files["ai/ftl_univ_03.json"])
        self.assertEqual((len(ftl3["paths"]), ftl3["paths"][0]["name"].startswith("ch_ftl_univ_03_")), (10, True))


if __name__ == "__main__":
    unittest.main()
