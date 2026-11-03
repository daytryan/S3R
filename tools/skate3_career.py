"""Export Skate 3's career (story mode) data from the player's own disc.

py -3 tools/skate3_career.py --disc DISC [--dlc PACKAGES] [--assets INST/assets] --out DIR

Inputs
- DISC is the extracted Skate 3 disc: data/big/db.big (progressionbanks/main and the record-name reports).
- --dlc and --assets are read by the later parts (DLC rows, strings); the progression part needs neither.

Output: DIR/career.json {"version": 1, ...}. This part writes:
- actions: every progression_action record, in record order:
  {"name", "key" (the name's hash64), "auto" (the auto-start flag), "conditions": [[kind, name, value], ...], "stategraph"}.
  A condition is the 24-byte Observatory::tObservatoryTrigger {u32 kind, u32 0, u64 key, u64 value} (order 2.1, gates Q1);
  `name` is the group (kind 1) or challenge row (kind 2) the key hashes, "" for the board-sales / cash kinds. The export
  refuses any kind above 5: the rules read an unknown kind as true (TU3 0x82C1CADC), so a future bank cannot carry one
  unseen. `stategraph` is "" for the abstract records that name none.
- groups: {group: [row, ...]}, every progression_group with its Challenges members in record order.
- stategraphs: {graph: {"initial": state, "states": {state: {"handlers": {event: "h_<key>"}, "events": [...]}}}}.
  `h_<key>` is "h_" + the handler record key's 16 hex digits (the name task 4 gives the decompiled function).
  An event is {"kind", "args": [a, b], "flags"} from the 16-byte Sk8::Challenge::tChallengePresentationEvent
  {u32 type, u32 string offset a, u32 string offset b, u32 skip flags}; the offsets read main.bin (0 = "").
- rewards: {name: {"category", "item", "title", "description", "icon", "raw": [a, b, c]}}, each field as the record
  chain gives it (rewards.md section 6). title / description are the string ids; raw is the three unnamed fields
  Hash_F950C13D02F83230 (an eSk8Characters enum), Hash_CBD4A095BED89D19 (Int32) and Hash_E373A52FB7D81920 (a Bool, 0 or 1),
  0 where no record in the chain sets one.
- rows: {row: {...}}, every challenge row the career can reach, from challengebanks/main (challenge + challenge_global_data
  chain) and the row's challenge_local_data (spec 2; order 3, rewards 2-3, gates Q4):
  {"key" (the name's hash64), "title" / "description" (string ids), "kind" (ChallengeType), "graph" (the challenge's
  stategraph), "world", "location", "map_start" (str or null), "start" (the local StartLocation, str or null), "offboard"
  (SpawnOffboard), "indicator" (SignUpIndicator key, "" if none), "own" / "kill" {"cash", "boards", "rewards": [name]},
  "online_bonus", "killable", "lot" (the Own the Lot row an activity belongs to, or null), "lot_rows", "disabled"
  (field FD40E4A31027C984), "global" (0-3), "signup_half" (field 8022BA64CB8E431D as stored), "end_teleport" (the
  EndTeleport locator, or null), "pack" (a DLC package's id, else null), "exits" (a teleport row's TeleportExitLocation,
  [{"world", "location"}]), "end_world" (the world of end_teleport, null with it; the row's own but for vert_skp5_01)}.
  A row is exported when its Global is 2 or 3, a progression group names it, or an exported lot lists it. The 267 rows
  retail activates (order 3) are all among them; the rest are the career-type rows no Lua or group activates (order 5).
- fe_locations: {"schoolstart": {"world", "location"}, "gamestart": {...}} (skatercollections fe_locations, boot 17-19).
- worlds: {world: "maps/<Name>.skate" or null} for every world a row, an fe_location or a teleport exit names; the engine's
  maps are the table tools/online_challenge_map.py WORLD_MAPS (a world with no map there, nor in its parents: null).
- characters / all_characters_key: the Free Play roster (freeplay 3.2-3.3).
- reward_sites: {reward: [site]} (sorted): where the career grants each reward. A site is "row:<row>" (the reward is in the
  row's Own or Kill list) or "graph:<stategraph>" (a handler of that stategraph calls Unlock.UnlockProgressionReward with its
  name, rewards.md section 6). Task 16 keeps an item available when no stage builds its earning moment.
- locators / volumes: {name: {"map": "maps/<Name>.skate", "matrix": [[f32; 4]; 4] row-major, "slots": [matrix, ...]}} and
  {name: {"map", "min": [x, y, z], "max": [x, y, z], "id": u64}}: every locator and volume the stage-1 rows (STAGE_ONE) name or
  place, a progression teleport event (kind 8) names, or an fe_location names, resolved to the engine map of the world they
  stand in. They come from the row's own missions/<row>/cSim_Global.xsf (all of its placements), the world stream of the
  SkateSchool (whole) and of any other world a name needs, and the global locators of that world's district
  (data/content/global_locators). A name none of them holds refuses ("locator"), but for the one event locator UNPLACED_EVENTS lists
  and the five kind 0 indicator locators of UNPLACED_INDICATORS (exported as null); the six hint volumes no file holds stay absent.
- indicators: {row: [{"objective", "kind" 0|1, "locator" (kind 0, else null), "type", "style", "spline": [[x, y, z], ...],
  "flags": [u32]}]} for each STAGE_ONE row: its own VisualIndicators (objective "") then those of its objectives (the challenge
  bank's objective records whose parent is the row) in number order (stage1_traces 3.1-3.3). A kind 1 ribbon takes its points and
  flags from the 0xEB0021 spline record of the row's mission that its instance id names; a kind 0 item names a locator.
- Two STAGE_ONE rows whose u64 keys give one f32 refuse ("key collision", spec 18.27).
- rows/<row>.json for each STAGE_ONE row (row_files): the rules.json row shape skate3_modes::ModeData reads (title, online, intro, objectives,
  challenge_type, groups {Objectives | ObjectivesOwned | ObjectivesKilledIt: [{"PointRequirement", "objectives": [objective]}]}, natives,
  trigger_volumes {decimal volume id: volume or null}, trick_groups, flow) plus trick_group_members, points_objective, presentation
  {Intro | Restart | Outro | Goto | SignUp: [event]} and speech {"calls": {field: [character, id]}, "HostCharacter",
  "AudioCharacterOverride"}. An objective is skate3_online.Objectives.objective's plus "hints" (the Hash_ECAEB8E619A68AF3 list in list order),
  "failures" (the Hash_B69AF085F7A6D275 list), "inline_failures" (FailureObjectives), "hint_keys" / "failure_keys" and "groups" {intro,
  complete, fail, hints [[event]], failures [[event]]} (stage1_traces 4.1-4.7). scripts/objectives.json holds each snippet's Lua text by
  its offset (retail text: --out only). Not tested in play.
- scripts/progression.lua (beside career.json; needs SKATE_JAVA and SKATE_UNLUAC): every progression handler record that names
  an event (246 of the 248; `default` and `main` are abstract), decompiled by unluac as one
  `function h_<key>(self, message, vars, sharedvars, globalvars) ... end` each, <key> the name stategraphs[g].states[s].handlers
  gives. Retail compiled these chunks as that function already; a chunk that is only a body is wrapped. The handler no state
  lists (h_70612941ADEDC0EE) is written too.
- scripts/graphs/<graph>.lua for tutorials, teleport, objectives_shared and photo_skate2: the decompiled challenge stategraphs
  (skate3_online.graphs_lua). Retail Lua: it goes only to --out, never into the repo.
- speech/ (needs SKATE_VGMSTREAM, vgmstream-cli): the in-challenge speech the stage-1 rows name (rows/<row>.json "speech" fields and
  Speech / SpeechNoWait events; spec 15.3, stage1_traces 1.1-1.6). speech/<name>_<n>.wav is one take of a bank sample (a main cast,
  cameraman or living world `.dat` split by its sth records) or `<name>_0.wav` one scripted variant (nis.big `.sns`); retail audio:
  --out only. speech/speech.json: {"scripted": {"<character>_<id>": [wav, ...]} (a line with no file is []), "banks": {bank:
  {"<event>": [{"masks": [u32], "files": [wav, ...]}]}} (every leaf of each event a named line maps to through speech_tables.json),
  "conditions": {bank: {"<event>": [type, ...]}} (the event's condition types: the order of each leaf's masks)}. The wav names are
  file names under speech/. Not tested in play.
- scenes/ (needs SKATE_VGMSTREAM; spec 10 "Cutscenes", 15.1, 15.3, 15.5, 15.7; stage1_nis 3.1-3.5): the 10 NIS stage 1 plays or holds (SCENES: the five
  tutorial intros, photo_univ_08, film_univ_05, 02_Reda_Passes_Baton, 03_Reda_Leaves_Shingo and Create_start_intro) in the cutscene player's
  format, `scenes/` being the root it reads: tutorial.json {"scenes": {scene: {kind, origin, duration, tracks, audio, camera, transition,
  subtitles}}}. A track is {character, package, anim, clip}: anim is scene/anim/<bank>.abin, copied from scene.big as it is (the player's
  Female1 take beside each Male1 one); the clip's name is `clip` because the reader's `clip1` is Skate 1's Clip1Meta struct. camera is
  scene/camera/<scene>.json: skate3_online.intro_cameras' 17 columns (camera_only, so the actor scenes stay) plus "effects" {brightness [t,
  override, near, far], dof, shadow, near_clip, far_clip, avatar_eye: [[t, value, ...], ...]} and "effect_columns" naming each column
  (EFFECTS; spec 18.4: only the brightness dip is drawn, DOF / shadow / clip keys are exported and not drawn; spec 18.3: AVATAR_EYE is exported and
  ignored). audio is [{audio nis/<stream>.wav, kind "post"|"speech", id, character, at, seconds}]: AUDIO_POST id N plays 99_N_*, AUDIO_SPEECH id N
  of character C plays C_N_*, at the key's time (a post with no 99_ stream plays nothing, spec 18.2), from nis.big, decoded by vgmstream and folded to stereo as the online beds are.
  subtitles is [[start, end, text]] from the scene's English .sub (the reader of prepare_retail_screens.data). Retail data (audio, text,
  banks): --out only. Not tested in play.
- cast/<key>.glb (needs --assets: private/stock/data/anim/OnBoard.abin, read only; spec 15.2; tools/skate3_career_cast.py): the stage-1 cast
  as character GLBs from the disc's own recipes, each with the joints and hierarchy of the scene bank that animates it: 133 for benny_fairfax,
  giovanni_reda, giovanni_reda_prop, shingo, shingo_prop, ai_skater_03 and ai_skater_04, 131 for player_skater (the Male1 / Female1 banks' rig),
  50 for the pedestrians female_adult_2, male_adult_1 and male_adult_2. Every actor track of scenes/tutorial.json has its model. Retail models:
  --out only. Not tested in play.
- screens/ (needs --assets: private/stock/skater-collections.json, read only; the disc's fedata.big and fedynamic.big): the 13 menu movies the career
  opens (SCREEN_MOVIES: the end-of-challenge dialogs, the photo picker, Skate 101, the reward chain, the sign-up poster, the tutorial HUD, the
  Free Play spawn and options screens, and sk8popup; presentation 1.5, freeplay 2.4 and 5.1), each merged with its controls by
  prepare_core_menu exactly as prepare_retail_screens merges the crossbar screens. screens/runtime/<movie>.json is the manifest and the
  textures, fonts and button glyphs it names sit beside it (screens/ is the root the manifests' paths are relative to). The career reads every
  screen from here, so the install's own screen folders are never changed. Retail screens: --out only. Not tested in play.
- ai/ (tools/skate3_ai_paths.py; stage1_univ section 5): ai/<row>.json for ftl_univ_02 and ftl_univ_03, the decode of their
  AIPATH_Challenge_ch_<row>.rx2 recordings (the Reda leader's 7 and 10 paths), and ai/tricks.json, the sorted trick ids (indexes into the TU3 trick
  table) the two rows use. The same files `skate3_ai_paths.py --rows ftl_univ_02,ftl_univ_03 --install DIR` writes. Retail data: --out only.
- strings.json (beside career.json, with --assets): {id: text} for every string id career.json names, from the install's
  challengemap.json["language"] (read only) and each DLC package's own table. It is retail text: written only to --out,
  never into the repo.
Every part is read the way work/corpus/prog_lua.py walks the bank.
"""
import argparse
import functools
import json
import os
import re
import shutil
import struct
import sys
import tempfile
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

import ea_speech
import unluac_batch
from skate3_online import (Db, big, TOOLS, Template, dlc_packages, graphs_lua, whole_json, mission_placement, world_placement, Objectives,
                           snippet_tree, TRIGGER_NATIVES, TRICK_GROUP_NATIVES, TRICK_GROUP, TRICK_MEMBERS, trick_groups, OBJECTIVE_GROUP, FAIL_FIRST,
                           GLOBAL_POINTS, AVAILABLE_ONLINE)
from online_challenge_map import WORLD_MAPS, global_locators   # the engine's maps by retail world (names the maps/<Name>.skate files)
import asset_pipeline.vlt as vlt
import skate3_online as online     # the ICE helpers the scenes reuse (intro_cameras, ice_state, scene_posts ...)

hash64 = vlt.hash64   # skate3_online has swapped in its memoised hash64

# The class and field hashes below are copied from docs/superpowers/specs/skate3-story-mode-reference/work/corpus/prog_lua.py
# (the research walk this part follows), which that folder's names.py names; test_skate3_career pins the named ones.
GROUP, GRAPH, STATE, HANDLER = "Hash_C04CEAAE6645A838", "Hash_E1A1E7B8F2EA6496", "Hash_EAE76EDE1246A650", "Hash_B0764F22F87D1B47"
ACTION, REWARD = "Hash_A5970491380FF1BE", "Hash_9A821DB88551C7C1"
GRAPHS, STATES, HANDLERS, BLOB, EVENT = ("Hash_EC1C213BBDBF1A68", "Hash_3CB7BA2AFC9BDE42", "Hash_2CCC81EDC5EC2AAA",
                                         "Hash_C3CD4B4183274819", "Hash_6931057318D2132E")
# Fields prog_lua.py does not name, from work/order/decode_prog.py and work/rewards/decode_rewards.py.
TRIGGERS, STATEGRAPH, AUTO = "Hash_40417FF1EFEFD7DB", "Hash_7867A00C3A6F893C", "Hash_EC7A860A5187B15D"
CATEGORY, ITEM, TITLE, DESCRIPTION, ICON = ("Hash_3DBC2F9C0083DF86", "Hash_8915E3FCB76529BA", "Hash_AA753A536859233D",
                                            "Hash_EC2ED691DF1C9F4C", "Hash_786255B8FF426A3D")
RAW_A, RAW_B, RAW_C = "Hash_F950C13D02F83230", "Hash_CBD4A095BED89D19", "Hash_E373A52FB7D81920"
EVENTS, NAME = "Events", "Name"

# challengebanks/main and challenge_local_data (the field hashes of tools/online_challenge_map.py, tools/skate3_online.py
# and work/rewards/decode_rewards.py; test_skate3_career pins the named ones). The converter spells a key by its name when
# it knows one, else `Hash_<16 hex>`; norm() puts every class and field in the second spelling.
CHALLENGE, GLOBAL_DATA, LOCAL = "Hash_8876E0B556740E36", "Hash_A4F35B159D7432B5", "Hash_46A56ADDF284C381"
GLOBAL_REF = "Hash_24491BF00316AA45"       # challenge -> its challenge_global_data record (a RefSpec)
ROW_TITLE, ROW_DESCRIPTION = "Hash_E2A50788B6C20C92", "Hash_6B0BD2BAE508B742"
KIND, GLOBAL_TYPE = "Hash_02B828CCDB4785E4", "Hash_122DB7B0F026DBEC"   # ChallengeType; Global (eGlobalType, order 3)
WORLD, LOCATION, MAP_START = "Hash_7565CD2A80E93A5E", "Hash_F1C6D5FA4A23951E", "Hash_A6E8924715A15CEC"
INDICATOR = "Hash_284B07325D587CC4"        # SignUpIndicator (career_flow 6.2)
OWN, KILL, BONUS = "Hash_047ED7FFBB024AB8", "Hash_67699C4B88712759", "Hash_2E4824C81FDAE87C"   # rewards 2
KILLED_IT = "Hash_3677267F3AECF0C8"        # ObjectivesKilledIt (gates Q4)
LOT, LOT_ROWS = "Hash_9233A3E52805D3FC", "Hash_ABE749C80E8ADBCE"   # an activity's lot; a lot's OnlineFreeSkateActivities (rewards 3)
DISABLED = "Hash_FD40E4A31027C984"         # stage1_teleport 3.1: AddActivity skips the row (0x82BCF57C)
SIGNUP_HALF = "Hash_8022BA64CB8E431D"      # the sign-up volume size (career_flow 6.2, 0x82BD2858)
END_TELEPORT, END_LOCATION = "Hash_89B3580572F41DA3", "Hash_89ADD2C42D6D58AE"   # EndTeleport and its tWorldLocationID
START_LOCATION, SPAWN_OFFBOARD, EXIT_LOCATION = "Hash_E9B699E31F7C45CA", "Hash_42CF56DDEA069244", "Hash_E7D448870EC1A6E9"
# skatercollections
WORLD_CLASS, FE_LOCATIONS, MARQUEE = "Hash_BC69C54E6E102D88", "Hash_DDCD6BC23255E508", "Hash_91BCA6693EFC9AA7"
RECIPE, VALID, FE_LOCATION = "Hash_6C9F05D8DBE7A492", "Hash_95E4332C22316FF9", "Hash_61422BD0D7AC4BF6"
DISPLAY = "Hash_174B301910A902C9"          # characters_marquee display string id (ID_COMMON_*), unnamed
ALL_CHARACTERS_KEY = "all_characters_unlock"    # unlocks A.5: 0x82F72180 initialises the key cheat id 1 unlocks (freeplay 3.2)
# Roster record keys the name table cannot resolve (freeplay 3.3 names them; the test pins each hash).
ROSTER_KEYS = {hash64(n): n for n in ("steak", "isaak", "dr_pepper")}
# SignUpIndicator keys: ribbon_indicator records career_flow 6.2 names (the name table holds only some of them; the test pins each hash).
INDICATORS = ("signup", "arrow", "teleport", "tutorial", "hallofmeat", "spotfilm", "spotphoto", "follow", "race")
# The Danny Way package's test chain plays in dist_dannywaydlc (order 9.2): a world with no engine map and no table to name it.
EXTRA_WORLDS = ("dist_dannywaydlc",)
# The world keys retail itself leaves unnamed: no skatercollections world record and no string holds the name, so the converter
# can only spell them Hash_<16 hex> (MEASURED on the disc and the four DLC packages; the exporter refuses any other unnamed
# world). They map to no engine map (worlds). Only these exact keys pass, like DANGLING_STATES.
UNNAMED_WORLDS = {
    0xEC763034105B866B: "World of street_ind_01, a Global 3 row of the disc bank that no group, lot or Lua activates (not "
                        "among the 267 of order 3); also the World of the two childless dlc_dwgh_anywhere_* templates, "
                        "which are not rows (the dlc_ rule of career_rows)",
}
GLOBAL_KINDS = (2, 3)       # Global 2 (teleports, gotos) and 3 (sign-up rows) are the career's (order 3, stage1_teleport 2.1)
# AnywherePhoto 0x25 and AnywhereFilm 0x27 rows run wherever the player is: no World, and the teleport manager never moves the
# player to them (gates.md teleport row: 0x82D55BF0 exempts types 0x25 and 0x27).
ANYWHERE = (0x25, 0x27)
DEATH_RACE = 0x13           # gates Q4: IsChallengeKillable is ChallengeType 0x13 or a non-empty ObjectivesKilledIt

# Handler chunks are compiled as `function h_<key>(self, message, vars, sharedvars, globalvars)`: the states' Lua runs them with the
# message, the state's vars, the graph's shared vars and the globals (the rules task's handler call, spec 4).
HANDLER_ARGS = "self, message, vars, sharedvars, globalvars"
# The challenge stategraphs whose Lua the oracle tests read (the plan's Task 4).
GRAPH_SCRIPTS = ("tutorials", "teleport", "objectives_shared", "photo_skate2")
REWARD_CALL = "UnlockProgressionReward("       # rewards.md section 6: Unlock.UnlockProgressionReward(name, bool), 0x82839958
REWARD_LITERAL = re.compile(r"UnlockProgressionReward\(\s*([\"'])([^\"']+)\1")
NO_JAVA = "SKATE_JAVA and SKATE_UNLUAC must name a working java and unluac.jar"

# The rows stage 1 builds: the plan's 14 (five lessons, four school teleports, the way into the University, the four University
# rows) and the disabled way back, tele_world_to_school (plan Task 3 list; stage.rs stage_1()).
STAGE_ONE = ("tut_sksc_01", "tut_sksc_02", "tut_sksc_03", "tut_sksc_04", "tut_sksc_06", "tele_sksc_01", "tele_sksc_02", "tele_sksc_03",
             "tele_sksc_04", "tele_school_to_world", "tele_world_to_school", "ftl_univ_02", "photo_univ_08", "film_univ_05", "ftl_univ_03")
# Challenge banks: an objective record (its parent is its row) and its VisualIndicators (tVisualEditorData, 80 bytes, stage1_traces 3.1).
OBJECTIVE, VISUAL = "Hash_59A34A6C5074F88A", "Hash_7AE8FEF77A54BB91"
VISUAL_TYPE, VOLUME_TYPE, LOCATION_TYPE = ("Sk8::Challenge::tVisualEditorData", "Sk8::Challenge::tTriggerVolumeInstanceID",
                                           "Sk8::Challenge::tLocationID")
# The worlds whose whole stream is kept: the lessons' Lua names its volumes by id (tut_sksc_inthehub_vol_*, wrongway, reset;
# stage1_school 5). Another world's stream is only searched for a name a row, event or indicator gives.
WHOLE_WORLDS = ("dist_skateschool",)
# A progression event of kind 8 is the presentation event TeleportLocalPlayer (stage1_school 3.6): args [locator, world].
TELEPORT_EVENT = 8
# The one teleport event locator that is on no disc file (MEASURED: not in a mission, a global locators file or the University
# world stream): public_chan_otl_stategraph's, a stage-2 Own the Lot graph. Only this exact (name, world) passes; any other
# unplaced event locator refuses, as DANGLING_STATES does for states.
UNPLACED_EVENTS = {("dist_university_clocktower", "dist_university"): "public_chan_otl_stategraph initial (progression.lua); "
                                                                      "in no placement file, nothing of stage 1 runs it"}
# The five kind 0 indicator locators of objectives that no mission, world stream or global locator file places (MEASURED on the disc).
# Each belongs to an objective its row's Objectives list never names, so retail never shows it (INFERRED: nothing draws for a missing
# locator; stage1_traces 3.1 sends kind 0 to 0x82d4f328, whose missing-locator path was not traced). They export "locator": null.
# Only these exact (row, objective, locator) triples pass; any other unplaced kind 0 locator refuses, as UNPLACED_EVENTS does for events.
_UNLISTED = "objective %s is not in %s's Objectives list (challenge_global_data Hash_C4B345415E8537BE); locator in no file"
UNPLACED_INDICATORS = {
    ("tut_sksc_02", "tut_sksc_02_3", "tut_sksc_02_vi_03"): _UNLISTED % ("tut_sksc_02_3", "tut_sksc_02"),
    ("tut_sksc_02", "tut_sksc_02_5", "tut_sksc_02_vi_05"): _UNLISTED % ("tut_sksc_02_5", "tut_sksc_02"),
    ("tut_sksc_02", "tut_sksc_02_11", "tut_sksc_02_vi_011"): _UNLISTED % ("tut_sksc_02_11", "tut_sksc_02"),
    ("tut_sksc_03", "tut_sksc_03_3", "tut_school_03_vi_04"): _UNLISTED % ("tut_sksc_03_3", "tut_sksc_03"),
    ("tut_sksc_03", "tut_sksc_03_6", "tut_school_03_vi_07"): _UNLISTED % ("tut_sksc_03_6", "tut_sksc_03"),
}
# Indicator type and style names (the vault names them; the converter's name table holds only "default"); test_skate3_career pins each hash.
RIBBON_NAMES = ("left_chevron", "arrow_nonfade", "default")

REASONS = ("condition kind", "decompile", "locator", "state", "key collision", "missing", "conversion")
# Kinds 0-5 are the evaluator's (TU3 0x82C1CAD8..0x82C1CC74, gates Q1); the main bank uses 0, 1, 2 and 5 (order 2.1).
CONDITION_KINDS = range(6)
GROUP_KINDS = (1, 2)    # their key names a progression group (1) or a challenge row (2); the others carry no key

# The retail bank itself names states it does not hold: 1up_dwtn_05_stategraph, tut_univ_01_stategraph and
# monster_contest_complete each list two (progression.lua 594-596, 1158-1161, 2703-2706, prog_lua.py "-- missing state").
# Retail ran them as graphs with no states (gates Q9). Not retail (user 10-20) does not apply: this reproduces retail.
# Only these exact keys are let through; any other missing state still refuses the export.
DANGLING_STATES = {
    "1up_dwtn_05_stategraph": frozenset({0xA084CD383295A484, 0x10D4E703317B2393}),
    "tut_univ_01_stategraph": frozenset({0xE4C78B45D3D44604, 0x7F72F6D2A2234E76}),
    "monster_contest_complete": frozenset({0x69F3ACBCE7A11A77, 0xB3EEEB02E73E82E6}),
}
# db.big record-name reports (one `/<class>.class/<record>.xml` line per record): the only disc source that names the
# progression rewards, the groups' challenge rows and the actions' challenge conditions.
NAME_REPORTS = ("data/db/progressionbanks/main_summaryreport.txt", "data/db/challengebanks/main_summaryreport.txt")


class Refused(Exception):
    """The export stops: `reason` is one of REASONS, `name` the record that caused it."""

    def __init__(self, reason, name, detail=""):
        if reason not in REASONS:
            raise ValueError(f"unknown refusal reason {reason!r}")
        super().__init__(f"{reason}: {name}" + (f" ({detail})" if detail else ""))
        self.reason, self.name, self.detail = reason, name, detail


class CareerDb(Db):
    """skate3_online.Db plus progressionbanks/main, with the record names the reports list added to the name table."""

    def __init__(self, disc, work):
        super().__init__(disc, work)
        self.disc = Path(disc)      # the placements (missions.big, world streams, global locators) are read from it later
        archive = big(disc, "data/big/db.big")
        # A challenge's StateGraph names a challenge_stategraph record; the record is named by its file (kept apart from
        # self.names, which the converter spells every key by, so no key in a converted bank changes its spelling).
        self.graph_names = [Path(e.path).stem for e in archive.entries if e.path.lower().startswith("data/db/challenge_stategraph/")
                            and e.path.lower().endswith(".vlt")]
        for e in archive.entries:
            low = e.path.lower()
            if low in ("data/db/progressionbanks/main.vlt", "data/db/progressionbanks/main.bin"):
                (self.work / e.path).parent.mkdir(parents=True, exist_ok=True)
                (self.work / e.path).write_bytes(archive.read(e))
            elif low in NAME_REPORTS:
                self.names = self.names + re.findall(r"/([^/\n]+)\.xml", archive.read(e).decode("latin-1"))


def key_hash(key):
    """The 64-bit hash a record key stands for: the digits of an unresolved `Hash_<16 hex>` key, else hash64 of the name."""
    return int(key[5:], 16) if key.startswith("Hash_") else hash64(key) if key else 0


def handler_name(record):
    return "h_%016X" % key_hash(record["key"])


class Bank:
    """The records of one bank, found by (class, key hash), with Attrib's parent-chain field lookup."""

    def __init__(self, records, names=()):
        self.records, self.by = records, {}
        for r in records:
            k = (r["class"], key_hash(r["key"]))
            if k in self.by:
                raise Refused("key collision", r["key"], "two records of one class and key")
            self.by[k] = r
        self.names = {hash64(n): n for n in names}
        self.names.update({key_hash(r["key"]): r["key"] for r in records if not r["key"].startswith("Hash_")})

    def of(self, cls):
        return [r for r in self.records if r["class"] == cls]

    def chain(self, record):
        seen = 0
        while record is not None and seen < 64:
            yield record
            record = self.by.get((record["class"], key_hash(record["parent"]))) if record["parent"] else None
            seen += 1

    def get(self, record, field):
        """The field as the nearest record of the parent chain holds it, else None."""
        return next((r["fields"][field] for r in self.chain(record) if field in r["fields"]), None)


def items(value):
    return value.get("array", {}).get("items", []) if value else []


def ref_keys(record, value, field):
    """The key hashes of a ClassRefSpec array (the key is whichever 8-byte half is not zero)."""
    out = []
    for it in items(value):
        a, b = struct.unpack(">QQ", bytes.fromhex(it))
        if not (a or b):
            raise Refused("missing", record["key"], f"empty reference in {field}")
        out.append(a or b)
    return out


def u32(value):
    return struct.unpack(">I", bytes.fromhex(value["data"])[:4])[0]


def flag(value):
    """A Bool field: its first byte (the other three are padding)."""
    return bool(value) and bytes.fromhex(value["data"])[0] != 0


def cstr(binary, at, owner):
    if not at:
        return ""
    end = binary.find(b"\0", at) if 0 < at < len(binary) else -1
    if end < 0:
        raise Refused("missing", owner, f"string offset {at:#x} is outside main.bin")
    return binary[at:end].decode("latin-1")


def conditions(bank, action):
    out = []
    for it in items(bank.get(action, TRIGGERS)):
        raw = bytes.fromhex(it)
        if len(raw) != 24:
            raise Refused("missing", action["key"], "a condition is not 24 bytes")
        kind, _, key, value = struct.unpack(">IIQQ", raw)
        if kind not in CONDITION_KINDS:
            raise Refused("condition kind", action["key"], f"kind {kind}")
        name = ""
        if key or kind in GROUP_KINDS:
            name = bank.names.get(key)
            if name is None:
                raise Refused("missing", action["key"], f"condition key {key:016X} names no record")
        out.append([kind, name, value])
    return out


def events_of(value, binary, owner):
    """The 16-byte Sk8::Challenge::tChallengePresentationEvent items of a field (None: no field) as {"kind", "args", "flags"}; the
    two string offsets read `binary` (0 = "")."""
    out = []
    for it in items(value):
        kind, a, b, flags = struct.unpack(">4I", bytes.fromhex(it))
        out.append({"kind": kind, "args": [cstr(binary, a, owner), cstr(binary, b, owner)], "flags": flags})
    return out


def decode_events(bank, state, binary, owner):
    return events_of(bank.get(state, EVENTS), binary, owner)


def stategraph(bank, graph, binary):
    name = graph["key"]
    states = {}
    for h in ref_keys(graph, bank.get(graph, STATES), STATES):
        state = bank.by.get((STATE, h))
        if state is None:
            if h in DANGLING_STATES.get(name, ()):
                continue
            raise Refused("state", name, f"names state {h:016X}, which the bank does not hold")
        sname = state["fields"].get(NAME, {}).get("data")
        if sname is None:
            raise Refused("missing", name, f"state {state['key']} has no Name")
        if sname in states:
            raise Refused("key collision", name, f"two states named {sname}")
        where, handlers = f"{name}/{sname}", {}
        for hh in ref_keys(state, bank.get(state, HANDLERS), HANDLERS):
            handler = bank.by.get((HANDLER, hh))
            if handler is None or BLOB not in handler["fields"] or NAME not in handler["fields"]:
                raise Refused("missing", where, f"handler {hh:016X} has no record, code or event name")
            size, offset = struct.unpack(">II", bytes.fromhex(handler["fields"][BLOB]["data"]))
            if offset + size > len(binary):
                raise Refused("missing", where, f"handler {hh:016X} code runs past main.bin")
            event = handler["fields"][NAME]["data"]
            if event in handlers:
                raise Refused("key collision", where, f"event {event} is handled twice")
            handlers[event] = handler_name(handler)
        states[sname] = {"handlers": handlers, "events": decode_events(bank, state, binary, where)}
    if states and "initial" not in states:
        raise Refused("state", name, "the graph has states but no initial one")
    return {"initial": "initial" if states else "", "states": states}


def reward(bank, record):
    name, out = record["key"], {}
    for label, field in (("category", CATEGORY), ("item", ITEM), ("title", TITLE), ("description", DESCRIPTION), ("icon", ICON)):
        value = bank.get(record, field)
        if value is None:
            raise Refused("missing", name, f"no {label} in the record chain")
        out[label] = u32(value) if label == "category" else value["data"]
    out["raw"] = []
    for field in (RAW_A, RAW_B, RAW_C):
        value = bank.get(record, field)
        out["raw"].append(0 if value is None else int(flag(value)) if value["type"].endswith("Bool") else u32(value))
    return out


def unique(table, key, owner):
    if key in table:
        raise Refused("key collision", key, f"two {owner} records of this name")
    return key


def progression(db):
    """The main bank's actions, groups, stategraphs and rewards (see the module docstring for their shape)."""
    bank = Bank(db.collections("progressionbanks/main"), getattr(db, "names", ()))
    binary = db.bin("progressionbanks/main")
    out = {"actions": [], "groups": {}, "stategraphs": {}, "rewards": {}}
    graph_names = {key_hash(g["key"]): g["key"] for g in bank.of(GRAPH)}
    seen = set()
    for a in bank.of(ACTION):
        name = unique(seen, a["key"], "action")
        seen.add(name)
        target = bank.get(a, STATEGRAPH)
        graph = ""
        if target:
            half = struct.unpack(">QQ", bytes.fromhex(target["data"]))
            if half[0] or half[1]:
                graph = graph_names.get(half[0] or half[1])
                if graph is None:
                    raise Refused("missing", name, f"stategraph {(half[0] or half[1]):016X} is not in the bank")
        out["actions"].append({"name": name, "key": key_hash(a["key"]), "auto": flag(bank.get(a, AUTO)),
                               "conditions": conditions(bank, a), "stategraph": graph})
    for g in bank.of(GROUP):
        members = []
        for h in ref_keys(g, bank.get(g, GRAPHS), GRAPHS):
            if h not in bank.names:
                raise Refused("missing", g["key"], f"member {h:016X} names no challenge row")
            members.append(bank.names[h])
        out["groups"][unique(out["groups"], g["key"], "group")] = members
    for g in bank.of(GRAPH):
        out["stategraphs"][unique(out["stategraphs"], g["key"], "stategraph")] = stategraph(bank, g, binary)
    for r in bank.of(REWARD):
        out["rewards"][unique(out["rewards"], r["key"], "reward")] = reward(bank, r)
    return out


def handler_chunks(db):
    """{h_<key>: compiled Lua chunk} for every handler record of the main bank that names an event, in record order (246 on the
    disc). `default` and `main` name none and are abstract. The chunk of a record is binary[offset:offset+size] of its blob."""
    bank = Bank(db.collections("progressionbanks/main"), getattr(db, "names", ()))
    binary = db.bin("progressionbanks/main")
    out = {}
    for h in bank.of(HANDLER):
        if NAME not in h["fields"]:
            continue
        name = unique(out, handler_name(h), "handler")
        if BLOB not in h["fields"]:
            raise Refused("missing", name, "the handler record has no code")
        size, offset = struct.unpack(">II", bytes.fromhex(h["fields"][BLOB]["data"]))
        if offset + size > len(binary):
            raise Refused("missing", name, "the handler code runs past main.bin")
        out[name] = binary[offset:offset + size]
    return out


def handler_function(name, lua):
    """The handler as `function <name>(self, message, vars, sharedvars, globalvars) ... end`. unluac prints retail's chunk as that
    function already (all 246 do, under their own key); a chunk that is only a body is wrapped. A different function name or
    argument list, or no output at all, is a failed decompile."""
    lua = lua.strip()
    head = f"function {name}({HANDLER_ARGS})"
    if not lua:
        raise Refused("decompile", name, "unluac printed nothing")
    if lua.startswith(head + "\n") and lua.endswith("\nend"):
        return lua
    if re.match(r"function\b", lua):
        raise Refused("decompile", name, f"the chunk defines another function than {head}")
    body = "\n".join("  " + line if line else line for line in lua.split("\n"))
    return f"{head}\n{body}\nend"


def handler_functions(chunks):
    """{name: handler_function text} for handler_chunks()' chunks, decompiled in one unluac batch (SKATE_JAVA, SKATE_UNLUAC)."""
    try:
        done = unluac_batch.run(list(chunks.values()), os.environ.get("SKATE_JAVA", "java"), os.environ.get("SKATE_UNLUAC"))
    except (KeyError, OSError) as e:
        raise Refused("decompile", "progression handlers", f"{NO_JAVA} ({e!r})")
    out = {}
    for name, chunk in chunks.items():
        stdout, code = done[chunk]
        if code:
            raise Refused("decompile", name, f"unluac exited {code}")
        out[name] = handler_function(name, unluac_batch.text(stdout))
    return out


def progression_lua(functions):
    """scripts/progression.lua: a header, then the handler functions in the order given, blank line between."""
    head = ("-- Skate 3 career progression handlers, decompiled by unluac from the player's own disc.\n"
            "-- Retail code: this file belongs in the export folder only, never in the repo.\n"
            f"-- {len(functions)} handlers, one function per record, named h_<key> as career.json stategraphs name them.\n")
    return head + "\n" + "\n\n".join(functions.values()) + "\n"


def graph_scripts(db, graphs=GRAPH_SCRIPTS):
    """{"scripts/graphs/<graph>.lua": text}: the challenge stategraphs' handlers decompiled by skate3_online.graphs_lua."""
    if not graphs:
        return {}
    try:
        lua = graphs_lua(db, list(graphs))
    except SystemExit as e:        # graphs_lua exits with "unluac failed on <graph>"
        raise Refused("decompile", str(e.code).rsplit(" ", 1)[-1], str(e.code))
    except (OSError, KeyError, ValueError) as e:
        raise Refused("missing", ",".join(graphs), f"a challenge stategraph file is not on the disc ({e!r})")
    if lua is None:
        raise Refused("decompile", ",".join(graphs), NO_JAVA)
    return {f"scripts/graphs/{g}.lua": lua[g] for g in graphs}


def reward_sites(rows, prog, functions):
    """{reward: [site]} (see the module docstring). A graph site is a handler a state of the graph lists whose Lua calls
    UnlockProgressionReward with a literal name; a call with a computed name is refused, so no site is missed unseen."""
    sites = {}

    def add(reward, site):
        if site not in sites.setdefault(reward, []):
            sites[reward].append(site)
    for row, r in rows.items():
        for reward in r["own"]["rewards"] + r["kill"]["rewards"]:
            add(reward, f"row:{row}")
    for graph, g in prog["stategraphs"].items():
        for state in g["states"].values():
            for h in state["handlers"].values():
                if h not in functions:
                    raise Refused("missing", graph, f"handler {h} was not decompiled")
                called = REWARD_LITERAL.findall(functions[h])
                if functions[h].count(REWARD_CALL) != len(called):
                    raise Refused("decompile", h, "UnlockProgressionReward is called with a computed reward name")
                for _, reward in called:
                    add(reward, f"graph:{graph}")
    return {k: sorted(v) for k, v in sorted(sites.items())}


def norm(name):
    """A class or field key in the one spelling the constants use: a name the converter resolved comes back as
    `Hash_<16 hex>` (the converter spells a key by its name whenever the name table holds it)."""
    return name if name.startswith("Hash_") else "Hash_%016X" % hash64(name)


def normalised(records):
    return [dict(r, **{"class": norm(r["class"]), "fields": {norm(k): v for k, v in r["fields"].items()}}) for r in records]


def name_table(db, extra=()):
    """{hash64: name} of every name the exporter can resolve: the database's (CareerDb: with the record-name reports and the
    stategraph files), the engine's world names (WORLD_MAPS), the sign-up indicator keys, the roster keys and the world
    no table holds, the skatercollections world records' keys, and `extra`."""
    names = (list(getattr(db, "names", ())) + list(WORLD_MAPS) + list(ROSTER_KEYS.values()) + list(INDICATORS) + list(EXTRA_WORLDS)
             + list(extra))
    for source in (db, getattr(db, "base", None)):      # a DLC package sees the disc's stategraph names through .base
        names += getattr(source, "graph_names", ())
    # The world records' own keys (skatercollections): the converter names them from the vault's strings, the table does not.
    names += [r["key"] for r in db.collections("skatercollections") if norm(r["class"]) == WORLD_CLASS and not r["key"].startswith("Hash_")]
    return {hash64(n): n for n in names}


def report_names(archive):
    """The record names a DLC package's own `*_summaryreport.txt` files under data/db/progressionbanks and challengebanks
    list, one `/<class>.class/<record>.xml` line each (the reports CareerDb reads from db.big for the disc)."""
    out = []
    for e in getattr(archive, "entries", ()):
        low = e.path.lower()
        if low.endswith("_summaryreport.txt") and low.startswith(("data/db/progressionbanks/", "data/db/challengebanks/")):
            out += re.findall(r"/([^/\n]+)\.xml", archive.read(e).decode("latin-1"))
    return out


def ref_key(value):
    """The record key a RefSpec (class, key, 0) or ClassRefSpec (key, 0) names, 0 for none."""
    a, b = struct.unpack(">QQ", bytes.fromhex(value["data"])[:16])
    return b or a


def lookup(names, key, owner, what):
    if not key:
        return None
    name = names.get(key)
    if name is None:
        raise Refused("missing", owner, f"{what} {key:016X} names no record")
    return name


def f32(value):
    return struct.unpack(">f", bytes.fromhex(value["data"])[:4])[0]


def string_of(binary, value, owner):
    """A tLocationID: a u32 offset into the .bin's strings (0 = none)."""
    return cstr(binary, u32(value), owner) if value else ""


def reward_list(binary, value, names, owner, label):
    """A tObservatoryProgressionReward {u32 cash, u32 board sales, u32 .bin offset, u32 count}: the offset leads to count
    24-byte RefSpecs {u64 class, u64 key, u64 0}; a key of 0 is an empty reference the game skips (rewards 1, 0x827CDEA0)."""
    out = {"cash": 0, "boards": 0, "rewards": []}
    if not value:
        return out
    cash, boards, at, count = struct.unpack(">4I", bytes.fromhex(value["data"])[:16])
    out["cash"], out["boards"] = cash, boards
    for i in range(count if at else 0):
        if at + 24 * (i + 1) > len(binary):
            raise Refused("missing", owner, f"{label} reward list runs past the bank .bin")
        _, key, _ = struct.unpack_from(">QQQ", binary, at + 24 * i)
        if key:
            out["rewards"].append(lookup(names, key, owner, f"{label} reward"))
    return out


def ref_names(value, names, owner, what):
    """The names a RefSpec array lists (items {class, key, 0}); empty references are skipped."""
    return [lookup(names, ref_key({"data": it}), owner, what) for it in items(value) if ref_key({"data": it})]


def merged(by, record):
    """A framework-parented record's fields, nearest first (Attrib's chain), spelled by hash."""
    out, seen = {}, 0
    while record is not None and seen < 64:
        for k, v in record["fields"].items():
            out.setdefault(norm(k), v)
        record = by.get(key_hash(record["parent"])) if record["parent"] else None
        seen += 1
    return out


def local_data(db, framework, row, names, with_file=False):
    """(the row's own local-data fields, their chain, the row's .bin) or None without local data. The chain is the row's record, then its
    parent records, nearest first: a parent that is another record of the row's own file (a lesson's `skateschool`, stage1_school 3: "local
    @skateschool") before one in the framework. Offsets in a row's own fields point into the row's own .bin; a template's are not read as
    locators (skate3_online.skate_rules). With `with_file` a fourth value is the set of fields held by any record of the row's file (own
    or a same-file parent): their offsets all point into the row's .bin."""
    rel = f"challenge_local_data/{row}"
    try:
        records = db.collections(rel)
    except (OSError, ValueError, KeyError):
        return None
    record = next((r for r in records if norm(r["class"]) == LOCAL and r["key"] in (row, norm(row))), None)
    if record is None:
        return None
    own = {norm(k): v for k, v in record["fields"].items()}
    in_file = {key_hash(r["key"]): r for r in records if norm(r["class"]) == LOCAL}
    chain, file_fields, step, seen = {}, set(), record, 0
    while step is not None and seen < 64:
        mine = in_file.get(key_hash(step["key"])) is step
        for k, v in step["fields"].items():
            chain.setdefault(norm(k), v)
            if mine:
                file_fields.add(norm(k))
        parent = key_hash(step["parent"]) if step["parent"] else 0
        step = (in_file.get(parent) or framework.get(parent)) if parent else None
        seen += 1
    got = own, chain, db.bin(rel)
    return got + (file_fields,) if with_file else got


def exits(own, binary, names, row):
    """A teleport row's TeleportExitLocation: tWorldLocationID items {u64 field hash, u64 world, u64 0, u32 locator offset, u32 0}."""
    out = []
    for it in items(own.get(EXIT_LOCATION)):
        raw = bytes.fromhex(it)
        out.append({"world": world_name(names, struct.unpack(">Q", raw[8:16])[0], row, "exit World"), "location": cstr(binary, struct.unpack(">I", raw[24:28])[0], row)})
    return out


def end_teleport(bank, g, binary, row):
    """(locator, world) EndTeleport sends the player to when the row ends, else (None, None): the flag EndTeleport is set and
    its tWorldLocationID (Hash_89ADD2C42D6D58AE) names a locator (stage1_univ 2.1: ftl_univ_02, photo_univ_08, film_univ_05 do;
    ftl_univ_03 has none). The world is the row's own for all but vert_skp5_01 (stage 2)."""
    flagged, where = bank.get(g, END_TELEPORT), bank.get(g, END_LOCATION)
    if not (flagged and flag(flagged) and where):
        return None, None
    raw = bytes.fromhex(where["data"])
    locator = cstr(binary, struct.unpack(">I", raw[24:28])[0], row)
    return (locator, world_name(bank.names, struct.unpack(">Q", raw[8:16])[0], row, "EndTeleport World")) if locator else (None, None)


def spelled(names, key):
    """The name of a record key, else the spelling the converter gives a key it cannot name (`Hash_<16 hex>`). Only for a sign-up
    indicator, which draws as the default marker when the name table lacks it; a world goes through world_name."""
    return "" if not key else names.get(key, "Hash_%016X" % key)


def world_name(names, key, owner, what="World"):
    """A world's name: the name table's, else `Hash_<16 hex>` for a key in UNNAMED_WORLDS, else the export refuses. `owner` is
    the row the key came from."""
    if not key:
        return ""
    if key in names:
        return names[key]
    if key in UNNAMED_WORLDS:
        return "Hash_%016X" % key
    raise Refused("missing", owner, f"{what} {key:016X} names no world and is not one retail leaves unnamed (UNNAMED_WORLDS)")


def indicator_name(names, value):
    return spelled(names, ref_key(value)) if value else ""


def career_rows(db, members=(), pack=None, base=None):
    """The rows the career can reach (see the module docstring) of the challenge banks `db` holds. `members` are the
    names of the progression groups' members. For a DLC package (a DlcDb), `pack` is its id and `base` the disc's db: the
    package's records follow the disc's, its own copy of `default` (rebased offsets, same fields) is dropped so the
    disc's stays the chain's root, and only the package's own rows come back."""
    records = normalised(db.collections("challengebanks/main"))
    head = len(base.collections("challengebanks/main")) if base is not None else 0
    records = records[:head] + [r for r in records[head:] if r["key"] != "default"]
    mine = {id(r) for r in records[head:]}
    # skate3_online.rebase moves a package's string and blob offsets by its place in the combined .bin, but not the list
    # offset of a tObservatoryProgressionReward (BIN_WORDS leaves the type): add that here, to the package's own values only.
    moved = (mine, getattr(db, "delta", 0)) if base is not None else (set(), 0)
    names = name_table(db, report_names(getattr(db, "archive", None)))
    bank = Bank(records, ())
    bank.names = {**names, **bank.names}
    binary = db.bin("challengebanks/main")
    framework = {}
    for k, r in Template(db, local_parents=True).framework.items():
        framework[key_hash(k)] = r
    challenges = bank.of(CHALLENGE)
    parents = {key_hash(r["parent"]) for r in challenges if r["parent"]}
    # A package names its templates dlc_<...> (order 9); two of them, dlc_dwgh_anywhere_photos / _films, no row inherits from,
    # so the structural test cannot tell them from rows. INFERRED (order 9: its row tables leave the templates out).
    leaves = [r for r in challenges if key_hash(r["key"]) not in parents
              and (base is None or (id(r) in mine and not r["key"].startswith("dlc_")))]
    glob = {}
    for ch in leaves:
        ref = bank.get(ch, GLOBAL_REF)
        g = bank.by.get(("Hash_%016X" % struct.unpack(">Q", bytes.fromhex(ref["data"])[:8])[0], ref_key(ref))) if ref else None
        if g is None:
            raise Refused("missing", ch["key"], "its challenge_global_data record is not in the bank")
        glob[ch["key"]] = g
    number = lambda g, f: u32(bank.get(g, f)) if bank.get(g, f) else 0   # noqa: E731
    wanted = {k for k, g in glob.items() if number(g, GLOBAL_TYPE) in GLOBAL_KINDS or k in members}
    # A lot lists its activities (rewards 3, 0x827CDBDC): they are reachable through it.
    todo = list(wanted)
    while todo:
        lot = todo.pop()
        for lot_row in ref_names(bank.get(glob[lot], LOT_ROWS), bank.names, lot, "lot activity"):
            if lot_row in glob and lot_row not in wanted:
                wanted.add(lot_row)
                todo.append(lot_row)
    out = {}
    for ch in leaves:
        name = ch["key"]
        if name in wanted:
            out[name] = row_of(db, bank, framework, ch, glob[name], binary, pack, moved)
    return out


def rewards_of(bank, g, field, moved):
    """A row's Own or Kill reward value, its list offset moved to the combined .bin when a package's record holds it."""
    holder = next((r for r in bank.chain(g) if field in r["fields"]), None)
    if holder is None:
        return None
    value, (own_ids, delta) = holder["fields"][field], moved
    if id(holder) in own_ids and delta:
        words = list(struct.unpack(">4I", bytes.fromhex(value["data"])[:16]))
        words[2] += delta if words[2] else 0
        value = dict(value, data=struct.pack(">4I", *words).hex().upper())
    return value


def row_of(db, bank, framework, ch, g, binary, pack, moved=(set(), 0)):
    name, names = ch["key"], bank.names
    if name.startswith("Hash_"):
        raise Refused("missing", name, "the row's key names no string")

    def text_of(f):
        v = bank.get(g, f)
        return v["data"] if v else ""
    world = bank.get(g, WORLD)
    kind = bank.get(g, KIND)
    anywhere = bool(kind) and u32(kind) in ANYWHERE
    if not (world and ref_key(world)) and not anywhere:
        raise Refused("missing", name, "the row names no World")
    graph = bank.get(ch, STATEGRAPH)
    indicator = bank.get(g, INDICATOR)
    map_start = bank.get(g, MAP_START)
    lot = bank.get(g, LOT)
    killed = bank.get(g, KILLED_IT)
    half = bank.get(g, SIGNUP_HALF)
    row = {"key": key_hash(name), "title": text_of(ROW_TITLE), "description": text_of(ROW_DESCRIPTION),
           "kind": u32(kind) if kind else 0,
           "graph": lookup(names, ref_key(graph), name, "stategraph") or "" if graph else "",
           "world": world_name(names, ref_key(world), name) if world else "",
           "location": string_of(binary, bank.get(g, LOCATION), name),
           "map_start": string_of(binary, map_start, name) or None}
    local = local_data(db, framework, name, names)
    ending = end_teleport(bank, g, binary, name)
    start, offboard, ways = None, False, []
    if local:
        own, chain, lbin = local
        start = string_of(lbin, own.get(START_LOCATION), name) or None
        offboard = flag(chain.get(SPAWN_OFFBOARD))
        ways = exits(own, lbin, names, name)
    row.update({"start": start, "offboard": offboard,
                "indicator": indicator_name(names, indicator),
                "own": reward_list(binary, rewards_of(bank, g, OWN, moved), names, name, "Own"),
                "kill": reward_list(binary, rewards_of(bank, g, KILL, moved), names, name, "Kill"),
                "online_bonus": u32(bank.get(g, BONUS)) if bank.get(g, BONUS) else 0,
                "killable": row["kind"] == DEATH_RACE or bool(items(killed)),
                "lot": lookup(names, ref_key(lot), name, "lot") if lot else None,
                "lot_rows": ref_names(bank.get(g, LOT_ROWS), names, name, "lot activity"),
                "disabled": flag(bank.get(g, DISABLED)),
                "global": u32(bank.get(g, GLOBAL_TYPE)) if bank.get(g, GLOBAL_TYPE) else 0,
                "signup_half": f32(half) if half else 0.0,
                "end_teleport": ending[0], "end_world": ending[1], "pack": pack, "exits": ways})
    return row


def collections_bank(db):
    """skatercollections as a Bank over the one-spelling records (lookups by class and key hash, parent chains)."""
    return Bank(normalised(db.collections("skatercollections")), ()), db.bin("skatercollections")


def fe_locations(db):
    """schoolstart and gamestart: {"world", "location"} of the skatercollections fe_locations records of those names
    (boot 17-19: the new career starts at gamestart, a skate.School launch at schoolstart), through the record's parents."""
    sc, binary = collections_bank(db)
    names = name_table(db)
    out = {}
    for key in ("schoolstart", "gamestart"):
        r = sc.by.get((FE_LOCATIONS, hash64(key)))
        if r is None:
            raise Refused("missing", key, "no fe_locations record of this name")
        world, where = sc.get(r, WORLD), sc.get(r, FE_LOCATION)
        location = string_of(binary, where, key)
        if not (world and ref_key(world) and location):
            raise Refused("missing", key, "the fe_location has no world or no locator")
        out[key] = {"world": lookup(names, ref_key(world), key, "world"), "location": location}
    return out


def characters(db):
    """([{"key", "recipe", "name", "category", "unlock"}], all_characters_key): the Free Play roster, in record order
    (freeplay 3.2-3.3). A character is a valid characters_marquee record whose parent is pro_skaters ("pro") or ip
    ("other"); the props, ambient skaters, the tutorial skater and the community templates have other parents. `name` is
    the display string id. `unlock` is the string the store tests (category 1 CHARACTERS, FNV of the string): the
    recipe. INFERRED (freeplay 3.2 says the record's name string; unlocks.md A.5 and rewards.md section 15 give the CHARACTERS
    reward items as the recipes: giovanni_reda, security_03, z_legend, deerman_of_darkwoods)."""
    sc, _ = collections_bank(db)
    names = name_table(db)
    pro, other = hash64("pro_skaters"), hash64("ip")
    out = []
    for r in sc.of(MARQUEE):
        parent = key_hash(r["parent"]) if r["parent"] else 0
        if parent not in (pro, other) or not flag(sc.get(r, VALID)):
            continue
        key = r["key"] if not r["key"].startswith("Hash_") else names.get(key_hash(r["key"]))
        if key is None:
            raise Refused("missing", r["key"], "the roster key names no string")
        recipe, display = sc.get(r, RECIPE), sc.get(r, DISPLAY)
        if not (recipe and recipe["data"]):
            raise Refused("missing", key, "the character has no recipe")
        out.append({"key": key, "recipe": recipe["data"], "name": display["data"] if display else "",
                    "category": "pro" if parent == pro else "other", "unlock": recipe["data"]})
    return out, ALL_CHARACTERS_KEY


def world_maps(db, worlds, maps=None):
    """{world: "maps/<Name>.skate" or None}: a world has an engine map when WORLD_MAPS (tools/online_challenge_map.py: the
    online challenge map's table of retail worlds to the engine's maps) holds it, else when a parent does (the
    progression park worlds are children of dist_startpark); else none."""
    maps = WORLD_MAPS if maps is None else maps
    sc, _ = collections_bank(db)
    names = name_table(db, maps)
    parent = {key_hash(r["key"]): r["parent"] for r in sc.of(WORLD_CLASS)}
    out = {}
    for world in sorted(worlds):
        name, seen = world, 0
        while name not in maps and seen < 16:
            up = parent.get(hash64(name))
            name = names.get(key_hash(up)) if up else None
            seen += 1
            if name is None:
                break
        out[world] = f"maps/{maps[name]}.skate" if name in maps else None
    return out


def strings(career, language):
    """(table, missing): table is {id: text} for every string in `career` (values, not keys) that the language table holds
    except '#'-prefixed ones, which carry their own text (online_challenge_map.Builder.text); missing lists the ids the
    row titles and descriptions, reward titles and descriptions and character names name that the table lacks."""
    ids = set()

    def walk(x):
        if isinstance(x, dict):
            for v in x.values():
                walk(v)
        elif isinstance(x, list):
            for v in x:
                walk(v)
        elif isinstance(x, str) and x and not x.startswith("#"):
            ids.add(x)
    walk(career)
    named = set()
    for part, fields in (("rows", ("title", "description")), ("rewards", ("title", "description"))):
        for r in career.get(part, {}).values():
            named |= {r.get(f, "") for f in fields}
    named |= {ch.get("name", "") for ch in career.get("characters", [])}
    return ({i: language[i] for i in sorted(ids) if i in language},
            sorted(i for i in named if i and not i.startswith("#") and i not in language))


def write(out, parts, strings=None, scripts=None):
    """Write out/career.json {"version": 1, **parts}, out/strings.json when `strings` (a function of the built parts
    giving {id: text}) is given, and each of `scripts` ({path under out: text}, LF). A part is its value or a zero-argument
    function giving it; every part is built before anything is written, so a Refused leaves the folder as it was. A script is its
    text, a JSON value, or a Path (a file copied byte for byte: the speech WAVs). Each file lands under a temp name and is renamed
    over the old one."""
    if "version" in parts:
        raise ValueError("version is written by write()")
    built = {k: v() if callable(v) else v for k, v in parts.items()}
    files = {"career.json": {"version": 1, **built}}
    if strings is not None:
        files["strings.json"] = strings(built)
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    tmps = {name: out / (name + ".tmp") for name in [*files, *(scripts or {})]}
    try:
        for name, value in {**files, **(scripts or {})}.items():
            tmps[name].parent.mkdir(parents=True, exist_ok=True)
            if isinstance(value, Path):
                shutil.copyfile(value, tmps[name])
            else:
                tmps[name].write_bytes((value if isinstance(value, str) else json.dumps(value, indent=1, ensure_ascii=False)).encode("utf-8"))
        for name in tmps:
            os.replace(tmps[name], out / name)
    finally:
        for tmp in tmps.values():
            tmp.unlink(missing_ok=True)


def export(db, out, dlcs=(), language=None, places=None, speech=None, scenes=None, cast=None, screens=None, ai=None):
    """Everything this exporter has. `dlcs` are skate3_online.DlcDb packages (their rows carry the pack id, the stem of
    their bank file); `language` is {id: text}: without it no strings.json is written. `places` is a DiscPlaces (default: the
    db's own disc); a db with no disc has nothing to place and gets empty locators, volumes and indicators. The Lua is
    decompiled first, so a failed decompile leaves `out` as it was. `speech` is a function (row files, work folder) giving
    (speech.json, {"speech/<wav>": file}, summary), speech_export for a disc (without it no speech/ is written); it runs before
    anything is written too. `scenes` is the same kind of function (work folder) giving ({path under out: dict, text or Path}, summary)
    (scene_export for a disc: scenes/tutorial.json, the cameras, the actor banks and the beds; without it no scenes/ is written). `cast` is the
    same kind of function giving ({"cast/<key>.glb": Path}, summary) (cast_part for a disc: the stage-1 cast, tools/skate3_career_cast.py;
    without it no cast/ is written); with both parts every actor track of the scenes must have its model (check_cast). `screens` (screens_part
    for a disc: screens/runtime/<movie>.json and the files they name) and `ai` (ai_part: ai/<row>.json and ai/tricks.json) are the same kind of
    function of a work folder; each runs before anything is written. Returns a summary."""
    prog = progression(db)
    functions = handler_functions(handler_chunks(db))
    graph_lua = graph_scripts(db)
    rows = career_rows(db, {m for g in prog["groups"].values() for m in g})
    text = dict(language or {})
    for d in dlcs:
        for name, row in career_rows(d, pack=d.banks[0].stem, base=db).items():
            if name in rows:
                raise Refused("key collision", name, "a package row is named like a disc row")
            rows[name] = row
        text = {**getattr(d, "language", {}), **text} if language is not None else text
    fe = fe_locations(db)
    roster, all_key = characters(db)
    check_activity_keys(rows, STAGE_ONE)
    worlds = world_maps(db, named_worlds(rows, fe, prog))      # an Anywhere row has no world
    places = places or (DiscPlaces(db.disc) if getattr(db, "disc", None) else None)
    locators, volumes, indicators = stage_places(db, rows, prog, fe, worlds, places, STAGE_ONE) if places else ({}, {}, {})
    career = {**prog, "rows": rows, "fe_locations": fe, "worlds": worlds, "locators": locators, "volumes": volumes,
              "indicators": indicators, "characters": roster, "all_characters_key": all_key,
              "reward_sites": reward_sites(rows, prog, functions)}
    files, sources = row_files(db, rows, locators, volumes, STAGE_ONE)
    scripts = {"scripts/progression.lua": progression_lua(functions), **graph_lua}
    summary = {"rows": len(rows), "strings": 0, "missing": [], "handlers": len(functions), "scripts": len(scripts),
               "reward_sites": len(career["reward_sites"]), "locators": len(locators), "volumes": len(volumes),
               "indicators": sum(map(len, indicators.values())), "row_files": len(files),
               "objectives": {r: sum(len(g["objectives"]) for field in f["groups"].values() for g in field) for r, f in files.items()}}
    scripts.update({f"rows/{r}.json": f for r, f in files.items()})
    if sources:
        scripts["scripts/objectives.json"] = sources       # the snippets' Lua text by offset: retail text, --out only

    def strings_part(built):
        table, summary["missing"] = strings(built, text)
        summary["strings"] = len(table)
        return table
    with tempfile.TemporaryDirectory(prefix="skate3-speech-") as work:      # the decoded takes wait here until write() copies them
        summary["speech"] = None
        if speech is not None:
            speech_json, wavs, summary["speech"] = speech(files, work)
            scripts["speech/speech.json"] = speech_json
            scripts.update(wavs)
        summary["scenes"] = None
        if scenes is not None:
            scene_files, summary["scenes"] = scenes(Path(work) / "scenes")
            scripts.update(scene_files)
        summary["cast"] = None
        if cast is not None:
            cast_files, summary["cast"] = cast(Path(work) / "cast-work")
            if "scenes/tutorial.json" in scripts:
                check_cast(scripts["scenes/tutorial.json"], cast_files)
            scripts.update(cast_files)
        summary["screens"] = None
        if screens is not None:
            screen_paths, summary["screens"] = screens(Path(work) / "screens-work")
            scripts.update(screen_paths)
        summary["ai"] = None
        if ai is not None:
            ai_files, summary["ai"] = ai(Path(work) / "ai-work")
            scripts.update(ai_files)
        write(out, career, strings=strings_part if language is not None else None, scripts=scripts)
    return summary


# ---------------------------------------------------------------- the stage-1 cast (cast/)
def cast_part(disc, assets, work):
    """({"cast/<key>.glb": Path in work}, summary): tools/skate3_career_cast.py's models (spec 15.2), one converted at a time. A model that
    cannot be built, or whose joints are not its bank's bones, refuses ("conversion"): nothing is written. Retail models: --out only."""
    import skate3_career_cast as cast_module
    try:
        return cast_module.cast_export(disc, assets, work)
    except cast_module.CastFailed as e:
        raise Refused("conversion", e.key, e.detail)


def check_cast(tutorial, files):
    """Every actor track of the scenes (every character but `current_player`, the player's own skater) names a model the cast has: the cutscene
    player loads cast/<character>.glb for it (stage1_nis 5), so a track with none would play without its actor."""
    missing = sorted({t["character"] for s in tutorial["scenes"].values() for t in s["tracks"]
                      if t["character"] != "current_player" and f"cast/{t['character']}.glb" not in files})
    if missing:
        raise Refused("missing", ", ".join(missing), "a scene track names a character with no cast/<key>.glb")


# ---------------------------------------------------------------- the menu movies (screens/)
SCREEN_SOURCE = "data/fe/source/"
SCREEN_KIND = "skate3-retail-screen"        # prepare_retail_screens.KIND: the same manifest format, read by the same loader
# The 13 movies the career opens, by the name the loader uses, with their path under SCREEN_SOURCE (the disc's fedata.big / fedynamic.big).
# Screen numbers are the exe's front-end screen ids (presentation 1.5 and its table at 211-240 and 729-740).
SCREEN_MOVIES = {
    "endchallengeoptions": "screens/hudComponents/endchallengeoptions",     # 33: the Won dialog (presentation 1.5)
    "endchallenge_small": "screens/hudComponents/endchallenge_small",       # 34: the Won dialog for Hall of Meat
    "challengefailed_small": "screens/hudComponents/challengefailed_small",  # 36: the Lost dialog
    "photoselect": "screens/challenge/photoselect",                          # 89: Pick a photo (OpenPhotoSelection 0x828544e0)
    "skate101": "screens/tutorials/skate101",                                # 37: Coach Frank Says... (Show101 0x827d0040)
    "unlockoverlay": "screens/unlock/unlockoverlay",                         # 109: Rewards (reward chain 0x827cf570)
    "unlock_milestone": "screens/unlock/unlock_milestone",                   # 111: Milestone Unlocked! (0x827cd150)
    "unlock_teammate": "screens/unlock/unlock_teammate",                     # 112: New Teammate! (0x827cd150)
    "challenge_signup": "screens/challenge/challenge_signup",                # 23: the sign-up poster (presentation 2.2)
    "hudtutorial": "screens/hud2/hudtutorial",                               # 50: Show / HideTutorialCalloutHUD (spec 12; stage1_school 380)
    "spawnskater": "screens/cas/spawnskater",                                # Free Play's spawn screen, mode 200 (freeplay 2.4)
    "freeskate_options": "screens/main/freeskate_options",                   # the Free Play settings screen (freeplay 5.1)
    "sk8popup": "screens/popup/sk8popup",                                    # the yes/no popups (stage1_school 382)
}


def screen_files(cache, names):
    """{"screens/<path>": Path in cache} for each movie of `names`: its manifest runtime/<name>.json and every file prepare_core_menu.runtime_files
    finds the manifest naming (the textures, fonts and button glyphs, each checked for size). A movie with no manifest is "missing"; a manifest
    that is not the format, names an unresolved font, or names a file that is absent, the wrong size or outside the cache is "conversion"."""
    import prepare_core_menu
    cache = Path(cache)
    files = {}
    for name in names:
        manifest = f"runtime/{name}.json"
        if not (cache / manifest).is_file():
            raise Refused("missing", name, "no manifest was prepared for the movie")
        try:
            found = prepare_core_menu.runtime_files(cache, manifest, SCREEN_KIND)
        except (ValueError, KeyError, OSError) as e:
            raise Refused("conversion", name, str(e))
        for rel in found:
            files["screens/" + rel] = cache / rel
    return files


def screens_part(disc, assets, work):
    """({"screens/<path>": Path in work}, summary): the 13 SCREEN_MOVIES, each through prepare_core_menu.prepare as prepare_retail_screens.prepare
    does it (fedynamic.big included, the controls merged in, the same font fallbacks and button glyphs), into work/screens-cache. Needs the
    install's private/stock/skater-collections.json (read only): the font mappings. A movie that cannot be merged refuses ("conversion").
    Retail screens: --out only."""
    import prepare_core_menu as screen
    import prepare_retail_screens as retail
    from vendor.skate3_ui.project import extract_project
    collections = Path(assets) / "private/stock/skater-collections.json"
    if not collections.is_file():
        raise Refused("missing", "skater-collections.json", f"{collections} is not there (the screens need the install's stock collections)")
    cache = Path(work) / "screens-cache"
    cache.mkdir(parents=True, exist_ok=True)
    original = screen.extract_project
    screen.extract_project = functools.partial(extract_project, include_dynamic=True)     # as prepare_retail_screens.prepare: fedynamic.big too
    try:
        for name, path in SCREEN_MOVIES.items():
            manifest = f"runtime/{name}.json"
            try:
                result = screen.prepare(Path(disc), cache, collections, SCREEN_SOURCE + path, manifest, SCREEN_KIND)
                retail._blank_unresolved(result)
                retail._debug_fallback(result)
                retail.shadow_foreground(result, lambda family: screen.AssetCache(cache).font_asset(family))
                result["buttons"] = retail._glyphs(Path(disc), cache)
                (cache / manifest).write_bytes((json.dumps(result, separators=(",", ":")) + "\n").encode("utf-8"))    # LF, as every export file
            except (ValueError, KeyError, OSError, IndexError, StopIteration, struct.error) as e:
                raise Refused("conversion", name, f"{type(e).__name__}: {e}")
    finally:
        screen.extract_project = original
    files = screen_files(cache, SCREEN_MOVIES)
    return files, {"movies": len(SCREEN_MOVIES), "files": len(files), "bytes": sum(p.stat().st_size for p in files.values())}


# ---------------------------------------------------------------- the Follow the Leader paths (ai/)
AI_ROWS = ("ftl_univ_02", "ftl_univ_03")      # the two stage-1 Follow the Leader rows (stage1_univ section 5): Reda's AI recordings


def ai_part(disc, work):
    """({"ai/<row>.json": text, "ai/tricks.json": text}, summary): tools/skate3_ai_paths.py's decode of AI_ROWS from the disc (its install_files, so
    the files are what `skate3_ai_paths.py --rows ftl_univ_02,ftl_univ_03 --install DIR` writes). A row with no .rx2 or local-data record is "missing"; a
    file that does not decode, branches to a path or node the file lacks, or names a trick past the TU3 table is "conversion". Retail data: --out only."""
    import skate3_ai_paths as ai
    try:
        outs, _ = ai.export_rows(Path(disc), None, AI_ROWS, ai.trick_table())
        for row, out in outs.items():
            coverage = out["coverage"]
            if coverage["anomalies"] or coverage["branch_targets_missing"]:
                raise Refused("conversion", row, f"anomalies {coverage['anomalies'][:3]}, missing branch targets {coverage['branch_targets_missing'][:3]}")
        files = ai.install_files(outs)
    except KeyError as e:
        raise Refused("missing", ", ".join(AI_ROWS), str(e.args[0] if e.args else e))
    except (ValueError, OSError) as e:
        raise Refused("conversion", ", ".join(AI_ROWS), str(e))
    ids = json.loads(files["ai/tricks.json"])
    return files, {"rows": len(outs), "paths": sum(len(o["paths"]) for o in outs.values()), "tricks": len(ids)}


# ---------------------------------------------------------------- locators, volumes, ribbons, the stage-1 key check
def activity_key(key):
    """A row's u64 key as retail's vars.kActivityID holds it: converted to a double (call 0x82F57EE0), then frsp to a single, stored
    with stfs (0x82D057C0; stage1_teleport 1.1). `message.ActivityKey == vars.kActivityID` compares two such floats (INFERRED for
    the message side), so two keys that round alike are one key to the teleport and sign-up handlers (spec 18.27)."""
    return struct.unpack(">f", struct.pack(">f", float(key)))[0]


def check_activity_keys(rows, stage=STAGE_ONE):
    """Refuse when two stage rows' keys are one f32 (spec 18.27). A stage row the export lacks is not compared."""
    seen = {}
    for name in stage:
        if name not in rows:
            continue
        value = activity_key(rows[name]["key"])
        if value in seen:
            raise Refused("key collision", name, f"its key and {seen[value]}'s are the same 32-bit float {value!r} (spec 18.27)")
        seen[value] = name


def single(x):
    """x, a float read from a single, as the shortest decimal that parses back to the same single (0.42, not 0.41999998688697815)."""
    packed = struct.pack(">f", x)
    for digits in range(1, 10):
        text = float("%.*g" % (digits, x))
        if struct.pack(">f", text) == packed:
            return text
    return x


def matrix_of(m):
    return [[single(v) for v in row] for row in m]


def locator_entry(map_, v):
    return {"map": map_, "matrix": matrix_of(v["matrix"]), "slots": [matrix_of(m) for m in v.get("slots", [])]}


def volume_entry(map_, v):
    out = {"map": map_, "min": [single(x) for x in v["min"]], "max": [single(x) for x in v["max"]]}
    if "id" in v:
        out["id"] = v["id"]
    return out


class DiscPlaces:
    """The three placement sources of the disc: a row's own mission (missions.big), a world's stream (worldDIST_<Name>.big) and the
    global locators (data/content/global_locators/<group>/<DIST_District>/*.rx2). Each is read entry by entry, never whole."""

    def __init__(self, disc):
        self.disc, self._missions, self._globals = Path(disc), None, None

    def mission(self, row):
        """(locators, volumes, splines) of missions/<row>/cSim_Global.xsf, None when the row has no mission folder."""
        if self._missions is None:
            self._missions = big(self.disc, "data/content/missions.big")
        if not any(f"/missions/{row}/".lower() in e.path.lower() for e in self._missions.entries):
            return None
        splines = {}
        locators, volumes = mission_placement(self._missions, row, splines)
        return locators, volumes, splines

    def world(self, map_name):
        """(locators, volumes) of the world stream of the engine map `map_name`, ({}, {}) when the disc has none."""
        rel = f"data/content/worldDIST_{map_name}.big"
        return world_placement(big(self.disc, rel)) if (self.disc / rel).is_file() else ({}, {})

    def globals(self):
        """{lower-case name: [(engine map of the file's district or None, {"matrix", "slots"})]}."""
        if self._globals is None:
            self._globals = {}
            for p in sorted((self.disc / "data/content/global_locators").rglob("*.rx2")):
                district = WORLD_MAPS.get(p.parent.name.lower())
                for name, v in global_locators(p.read_bytes()).items():
                    self._globals.setdefault(name, []).append((district, v))
        return self._globals


class Placed:
    """Locators and volumes, as stage_places collects them: `out` is what career.json gets; `placed[map]` indexes (lower-case) the
    names a mission or a kept world stream put there, so a name is looked up on the map it is asked in."""

    def __init__(self, worlds, places):
        self.worlds, self.places, self.out = worlds, places, {"locators": {}, "volumes": {}}
        self.placed, self.splines, self.streams = {}, {}, {}

    def map_of(self, world, owner):
        map_ = self.worlds.get(world)
        if not map_:
            raise Refused("locator", owner, f"world {world!r} has no engine map")
        return map_

    def put(self, kind, name, entry):
        have = self.out[kind].get(name)
        if have is not None and have != entry:
            raise Refused("key collision", name, f"the {kind[:-1]} is placed twice, differently")
        self.out[kind][name] = entry

    def add(self, kind, name, v, map_):
        self.put(kind, name, (locator_entry if kind == "locators" else volume_entry)(map_, v))
        self.placed.setdefault(map_, {"locators": {}, "volumes": {}})[kind][name.lower()] = name

    def load_mission(self, row, world):
        map_ = self.map_of(world, row)
        found = self.places.mission(row)
        if found is None:
            return
        locators, volumes, self.splines[row] = found
        for name, v in locators.items():
            self.add("locators", name, v, map_)
        for name, v in volumes.items():
            self.add("volumes", name, v, map_)

    def stream(self, map_):
        name = Path(map_).stem
        if name not in self.streams:
            self.streams[name] = self.places.world(name)
        return self.streams[name]

    def keep_world(self, world, owner):
        map_ = self.map_of(world, owner)
        locators, volumes = self.stream(map_)
        for name, v in locators.items():
            self.add("locators", name, v, map_)
        for name, v in volumes.items():
            self.add("volumes", name, v, map_)

    def resolve(self, kind, name, world, owner, expect_id=None):
        """The locator or volume `name` on the map of `world`: one a mission or a kept world stream already placed there, else (locators)
        the global locators of that map's district, else the world stream. Not found, or a volume of another id, refuses."""
        map_ = self.map_of(world, owner)
        low = name.lower()
        source = self.placed.get(map_, {"locators": {}, "volumes": {}})[kind].get(low)
        if source is not None:
            entry = self.out[kind][source]
        else:
            entry = None
            if kind == "locators":
                mine = [v for district, v in self.places.globals().get(low, []) if district and "maps/" + district + ".skate" == map_]
                if len({json.dumps(v, sort_keys=True) for v in mine}) > 1:
                    raise Refused("key collision", name, f"{owner}: two global locator files of {map_} place it differently")
                entry = locator_entry(map_, mine[0]) if mine else None
            if entry is None:      # last, because a world stream of a big world (Downtown, University) is slow to read
                streamed = {n.lower(): v for n, v in self.stream(map_)[0 if kind == "locators" else 1].items()}
                entry = (locator_entry if kind == "locators" else volume_entry)(map_, streamed[low]) if low in streamed else None
        if entry is None:
            raise Refused("locator", name, f"{owner}: the {kind[:-1]} is in no mission, world stream or global locator file of {map_}")
        if expect_id is not None and entry.get("id") != expect_id:
            raise Refused("locator", name, f"{owner}: the volume's id is {entry.get('id')}, its local data names {expect_id}")
        self.put(kind, name, entry)


def ribbon_item(objective, raw_hex, binary, owner):
    """One tVisualEditorData item (stage1_traces 3.1; consumer 0x82d4f720): (kind, locator or None, type key, style key, spline
    instance id, spline name). Kind >= 2 does nothing in retail and the ribbon parameter (+4, copied to the ribbon node) is 0 in
    every stage-1 item, so a future item with either refuses rather than losing it."""
    raw = bytes.fromhex(raw_hex)
    if len(raw) != 80:
        raise Refused("missing", owner, f"{objective}: a VisualIndicators item is {len(raw)} bytes, not 80")
    kind, param, name_at, _, instance, locator_at = struct.unpack(">IIIIQI", raw[:28])
    if kind not in (0, 1):
        raise Refused("missing", owner, f"{objective}: a VisualIndicators item of kind {kind}, which retail ignores")
    if param:
        raise Refused("missing", owner, f"{objective}: a ribbon parameter {param}, which the export does not carry")
    type_key, = struct.unpack(">Q", raw[40:48])
    style_key, = struct.unpack(">Q", raw[64:72])
    locator = cstr(binary, locator_at, owner) or None if kind == 0 else None
    return kind, locator, type_key, style_key, instance, cstr(binary, name_at, owner)


def named_worlds(rows, fe, prog):
    """Every world a row, an fe_location, a teleport exit or a progression teleport event names."""
    events = {e["args"][1] for g in prog["stategraphs"].values() for st in g["states"].values() for e in st["events"]
              if e["kind"] == TELEPORT_EVENT}
    return ({r["world"] for r in rows.values()} | {r["end_world"] for r in rows.values()} | {v["world"] for v in fe.values()}
            | {e["world"] for r in rows.values() for e in r["exits"]} | events) - {None, ""}


def field_items(v):
    """The items of a field that is an array or a single value (a tVisualEditorData / tTriggerVolumeInstanceID / tLocationID)."""
    return [] if not v else items(v) if "array" in v else [v["data"]]


def volume_name(text):
    """A tTriggerVolumeInstanceID's string: DIST_<District>|<volume name>|0x<id>."""
    parts = text.split("|")
    return parts[1] if len(parts) > 1 else parts[0]


def stage_places(db, rows, prog, fe, worlds, places, stage=STAGE_ONE):
    """(locators, volumes, indicators) for the stage rows `rows` holds (see the module docstring). `worlds` is world_maps' answer
    and `places` a DiscPlaces (or its stand-in): it gives each stage row's mission, a world's stream and the global locators.
    Every name a stage row gives (Location, StartLocation, MapStartLocation, EndTeleport, teleport exits, the locators and
    volumes its local data and its own VisualIndicators name), an fe_location and a progression teleport event must resolve, else
    "locator". The one exception is the five kind 0 locators of UNPLACED_INDICATORS (objectives their row never lists: tut_sksc_02_3 /
    _5 / _11, tut_sksc_03_3 / _6): their items export "locator": null. Any other unplaced kind 0 locator refuses. The kind 1 items of
    unlisted objectives are exported like the rest (tut_sksc_01 has 23 splines, two of them from unlisted objectives)."""
    mine = [r for r in stage if r in rows]
    bank = Bank(normalised(db.collections("challengebanks/main")), ())
    cbin = db.bin("challengebanks/main")
    names = name_table(db, RIBBON_NAMES)
    placed = Placed(worlds, places)
    for row in mine:
        placed.load_mission(row, rows[row]["world"])
        if rows[row]["world"] in WHOLE_WORLDS:
            placed.keep_world(rows[row]["world"], row)
    # todo: the names that must resolve, (kind, name, world, owner, volume id); soft: the five UNPLACED_INDICATORS locators, which may stay
    # unplaced; ribbons: each row's indicator items
    todo, soft, ribbons = [], [], {}
    for row in mine:
        r = rows[row]
        for name in (r["location"], r["start"], r["map_start"]):
            if name:
                todo.append(("locators", name, r["world"], row, None))
        if r["end_teleport"]:
            todo.append(("locators", r["end_teleport"], r["end_world"], row, None))
        todo += [("locators", e["location"], e["world"], row, None) for e in r["exits"] if e["location"]]
        got = []
        local = local_data(db, {}, row, names)
        if local:
            own, _, lbin = local
            for v in own.values():
                for x in (field_items(v) if v.get("type") in (VISUAL_TYPE, VOLUME_TYPE, LOCATION_TYPE) else []):
                    if v["type"] == VISUAL_TYPE:
                        got.append(("", x, lbin))
                    elif v["type"] == LOCATION_TYPE:
                        name = cstr(lbin, int(x[:8], 16), row)
                        if name:
                            todo.append(("locators", name, r["world"], row, None))
                    else:
                        name = volume_name(cstr(lbin, int(x[:8], 16), row))
                        if name:
                            todo.append(("volumes", name, r["world"], row, int(x[16:32], 16)))
        objectives = [o for o in bank.of(OBJECTIVE) if o["parent"] and key_hash(o["parent"]) == key_hash(row)]
        for o in sorted(objectives, key=lambda o: (int(m.group(1)) if (m := re.search(r"_(\d+)$", o["key"])) else -1, o["key"])):
            got += [(o["key"], x, cbin) for x in field_items(o["fields"].get(VISUAL))]
        ribbons[row] = []
        for objective, x, binary in got:
            kind, locator, type_key, style_key, instance, spline_name = ribbon_item(objective, x, binary, row)
            if locator:
                (soft if (row, objective, locator) in UNPLACED_INDICATORS else todo).append(("locators", locator, r["world"], row, None))
            ribbons[row].append((objective, kind, locator, type_key, style_key, instance, spline_name))
    for key, v in fe.items():
        todo.append(("locators", v["location"], v["world"], key, None))
    for graph, g in prog["stategraphs"].items():
        for state, st in g["states"].items():
            for e in st["events"]:
                if e["kind"] == TELEPORT_EVENT and e["args"][0] and tuple(e["args"][:2]) not in UNPLACED_EVENTS:
                    todo.append(("locators", e["args"][0], e["args"][1], f"{graph}/{state}", None))
    for kind, name, world, owner, expect_id in todo:
        placed.resolve(kind, name, world, owner, expect_id)
    unplaced = set()       # the listed (row, locator) pairs that no file places
    for kind, name, world, owner, expect_id in soft:
        try:
            placed.resolve(kind, name, world, owner, expect_id)
        except Refused as e:
            if e.reason != "locator":
                raise
            unplaced.add((owner, name))
    indicators = {}
    for row in mine:
        out = []
        for objective, kind, locator, type_key, style_key, instance, spline_name in ribbons[row]:
            spline = placed.splines.get(row, {}).get(instance) if kind == 1 else None
            if kind == 1 and spline is None:
                raise Refused("locator", spline_name or "%016X" % instance, f"{row}: {objective or 'its VisualIndicators'} names a spline "
                              f"({instance:016X}) the row's mission stream does not hold")
            out.append({"objective": objective, "kind": kind, "locator": None if (row, locator) in unplaced else locator,
                        "type": spelled(names, type_key),
                        "style": spelled(names, style_key),
                        "spline": [[single(x) for x in p] for p in spline["points"]] if spline else [],
                        "flags": [spline["flags"]] if spline else []})
        indicators[row] = out
    return ({k: placed.out["locators"][k] for k in sorted(placed.out["locators"])},
            {k: placed.out["volumes"][k] for k in sorted(placed.out["volumes"])}, indicators)


# ---------------------------------------------------------------- objectives, hints, failure records, presentation groups, speech
# rows/<row>.json for each stage-1 row: the shape skate3_modes::ModeData reads for one row (a rules.json row: title, online, intro,
# objectives, challenge_type, groups, natives, trigger_volumes, trick_groups, flow; tools/skate3_online.py objective_entry) plus the
# story's extensions, below. Not tested in play.
#
# Objective lists of challenge_global_data: Objectives (the lessons, MakeObjectives), ObjectivesOwned / ObjectivesKilledIt (the follow, film
# and photo rows, MakeObjectivesFromGroup). An item names an objective (a group of one) or an objective group.
OBJECTIVE_FIELDS = ("Objectives", "ObjectivesOwned", "ObjectivesKilledIt")
OBJECTIVES, OBJECTIVES_OWNED, OBJECTIVES_KILLED_IT = (norm(n) for n in OBJECTIVE_FIELDS)
FLOW = norm("ObjectiveFlow")                 # tObjectiveFlow {u32 type, u32 count}, group +0x60 / +0x64, applied by 0x82c6d168 (stage1_school 2)
INLINE_FAILURES = norm("FailureObjectives")  # Hash_40D40861DD9A6CC6: the inline failure snippets, read at 0x82c7395c (stage1_traces 4.2)
OBJECTIVE_DEFINITION, ACTIVATION, MASTER = norm("ObjectiveDefinition"), norm("ActivationMode"), norm("MasterObjective")
POINT_REQUIREMENT = norm("PointRequirement")
# The objective ctor 0x82c726e8 reads three RefSpec lists of challenge_failure_objective records (stage1_traces 4.2): Hash_B69AF085F7A6D275
# (from 0x82c72c54: failures, run by 0x82c75490), Hash_ECAEB8E619A68AF3 (from 0x82c731a8: hints, run by 0x82c755b8) and the inline
# FailureObjectives. Their names are in no table. Each record has a snippet (ObjectiveDefinition) and its own presentation group
# (PresentationEvents, Hash_BD96B775B855E129).
HINTS, FAILURES = "Hash_ECAEB8E619A68AF3", "Hash_B69AF085F7A6D275"
FAILURE_OBJECTIVE, FAILURE_EVENTS = "Hash_3159635DA17FF60E", "Hash_BD96B775B855E129"
# An objective's own groups (ctor 0x82c726e8): intro +0xf78, complete +0xf7c, fail +0xf80 (stage1_school 2).
OBJ_INTRO, OBJ_COMPLETE, OBJ_FAIL = "Hash_E73C9CF0F75A6FCD", "Hash_D404BA3319E2C389", "Hash_607EC72F932997C1"
SIGNUP_EVENTS = "Hash_96A46E5F831631C6"      # global data: the sign-up presentation events (ActivityHasSignupPresentationEvents 0x8255caa8)
# A row's presentation groups by the name this file gives them: the local-data tChallengePresentationEvent lists (presentation.md 1.1).
LOCAL_GROUPS = (("Intro", "IntroPresentationEvents"), ("Restart", "RestartPresentationEvents"), ("Outro", "OutroPresentationEvents"),
                ("Goto", "GotoPresentationEvents"))
SPEECH_TYPE = "Sk8::Challenge::tSpeechInfo"     # {u32 character, u16 id}: PlayNPCSpeechUsingAttribute 0x82843860 (stage1_traces 1.1)
HOST_CHARACTER, CHARACTER_OVERRIDE = "HostCharacter", "AudioCharacterOverride"
# The tSpeechInfo fields the career graphs read by name (self.<name> in graphs/*.lua, the stage1_traces 1.6 table). A field this list lacks
# keeps its Hash_<16 hex> spelling (stage1_traces 1.6 does the same for film_univ_05's seven).
SPEECH_FIELDS = (
    "AudioBeginReplay", "AudioDefineChallengeWinLowTime", "AudioEliminated", "AudioLineTimerExpired", "AudioLoseTimerExpired",
    "AudioLostProChallenge", "AudioOutOfBoundsWarning", "AudioPhotoPickAShot", "AudioPhotographerSetupPlayerSpeech",
    "AudioPlayerAcceptChallenge", "AudioPlayerBailed", "AudioPlayerCompleteAnObjective", "AudioPlayerDeclineChallenge",
    "AudioPlayerFailedAttemptOneObj", "AudioPlayerFailedChallenge", "AudioPlayerLeftChallenge", "AudioPlayerNotEnoughPoint",
    "AudioPlayerQuitChallenge", "AudioPlayerRestartChallenge", "AudioPlayerSucceedChallenge", "AudioPlayerWonChallenge",
    "AudioReinforcePlayerOnObjectiveOnFail", "AudioResultsScreen", "AudioTimeLimitLowSpeech", "AudioWipeout", "AudioWonProChallenge",
    "SignupSpeech")
SPEECH_NAMES = {hash64(n): n for n in SPEECH_FIELDS}
FRAMEWORK_BIN = "challenge_local_data_framework"


# The decimal volume ids the lessons' snippets name that no disc file places (spec 18.20: tut_sksc_02's hintvolume_07, _08, _11, _12, _23, _24;
# MEASURED: the id is in no cSim_Global.xsf of the row's mission nor in the SkateSchool stream, and the snippet text names the volume). Each is
# a hint's condition, so retail never fires that hint (INFERRED: IsInVolume finds no volume). They export null; any other unplaced id refuses.
_HINT_VOLUME = "an id of tut_sksc_02_hintvolume_%s in a hint snippet; no file places the volume (spec 18.20, stage1_school 5)"
UNPLACED_VOLUMES = {
    ("tut_sksc_02", "2422885606196998"): _HINT_VOLUME % "07",
    ("tut_sksc_02", "2422889901164294"): _HINT_VOLUME % "08",
    ("tut_sksc_02", "2442646750725894"): _HINT_VOLUME % "11",
    ("tut_sksc_02", "2442642455758598"): _HINT_VOLUME % "12",
    ("tut_sksc_02", "2442693995366150"): _HINT_VOLUME % "23",
    ("tut_sksc_02", "2442698290333446"): _HINT_VOLUME % "24",
}


def unplaced_checked(row, volume_id, by_id):
    """The career volume of a decimal id, None for a UNPLACED_VOLUMES id, else the export refuses ("locator")."""
    found = by_id.get(volume_id)
    if found is None and (row, volume_id) not in UNPLACED_VOLUMES:
        raise Refused("locator", volume_id, f"{row}: a snippet names this trigger volume id and no placed volume has it (not one of UNPLACED_VOLUMES)")
    return found


def native_calls(*snippets):
    """Every ["native", name, args] node of the compiled snippets' trees (skate3_online.snippet_tree), arguments' nested calls too."""
    found, todo = [], [s["tree"] for s in snippets if s]
    while todo:
        node = todo.pop()
        if isinstance(node, list):
            if node[:1] == ["native"]:
                found.append(node)
            todo.extend(node)
    return found


class StoryObjectives(Objectives):
    """skate3_online.Objectives with what the story rows read of an objective: its hints (the ECAE list), failure records (the B69A
    list), the inline failure snippets and the presentation groups (stage1_traces 4.1-4.7, spec 12.2). `failures` is the B69A list;
    skate3_online's own `failures` (the inline FailureObjectives) is `inline_failures`."""

    def records(self, f, field, owner):
        """(snippets, keys, event lists) of a RefSpec list of failure records: the snippet of each (None when its ObjectiveDefinition is
        empty, so the list keeps its positions), its record name and its PresentationEvents."""
        snippets, keys, groups = [], [], []
        for item in items(f.get(field)):
            r = self.ref(item)
            if r is None:
                raise Refused("missing", owner, f"a failure record of the {field} list is not in the challenge bank")
            rf = self.chain(r)
            try:
                snippets.append(self.definitions(rf[OBJECTIVE_DEFINITION])[0] if OBJECTIVE_DEFINITION in rf else None)
            except ValueError as e:
                raise Refused("decompile", r["key"], f"the snippet of {owner}'s failure record is outside the port's Lua subset ({e})")
            keys.append(r["key"])
            groups.append(events_of(rf.get(FAILURE_EVENTS), self.bin, r["key"]))
        return snippets, keys, groups

    def objective(self, r, binary, locators, volumes):
        try:
            out = super().objective(r, binary, locators, volumes)
        except ValueError as e:
            raise Refused("decompile", r["key"], f"a snippet is outside the port's Lua subset ({e})")
        f = self.chain(r)
        out["inline_failures"] = out.pop("failures")
        out["hints"], out["hint_keys"], hint_groups = self.records(f, HINTS, r["key"])
        out["failures"], out["failure_keys"], failure_groups = self.records(f, FAILURES, r["key"])
        out["groups"] = {"intro": events_of(f.get(OBJ_INTRO), binary, r["key"]), "complete": events_of(f.get(OBJ_COMPLETE), binary, r["key"]),
                         "fail": events_of(f.get(OBJ_FAIL), binary, r["key"]), "hints": hint_groups, "failures": failure_groups}
        return out


def framework_bin(db, row):
    try:
        return db.bin(FRAMEWORK_BIN)
    except (OSError, KeyError, ValueError) as e:
        raise Refused("missing", row, f"a template's event list needs the framework strings, which the database lacks ({e!r})")


def presentation_of(db, row, gf, local, cbin):
    """{name: [event]}: the row's presentation groups that its data holds (Intro, Restart, Outro, Goto from the local data, SignUp from the
    global data). A list the row's own record holds reads the row's .bin; one a template holds reads the framework's."""
    out = {}
    if local:
        _, chain, lbin, in_file = local
        for name, field in LOCAL_GROUPS:
            v = chain.get(norm(field))
            if v is None:
                continue
            binary = lbin if norm(field) in in_file else framework_bin(db, row) if items(v) else b""
            out[name] = events_of(v, binary, row)
    if SIGNUP_EVENTS in gf:
        out["SignUp"] = events_of(gf[SIGNUP_EVENTS], cbin, row)
    return out


def speech_of(gf, local):
    """{"calls": {field: [character, id]}, "HostCharacter", "AudioCharacterOverride"}: the tSpeechInfo fields of the row's global and
    local data chains (a name from SPEECH_FIELDS, else Hash_<16 hex>), and the two characters (None when the chain sets neither)."""
    chain = local[1] if local else {}
    calls = {}
    for fields in (gf, chain):
        for k, v in fields.items():
            if v.get("type") == SPEECH_TYPE and "array" not in v:
                character, ident = struct.unpack(">IH", bytes.fromhex(v["data"])[:6])
                calls[SPEECH_NAMES.get(key_hash(k), k)] = [character, ident]
    host, override = chain.get(norm(HOST_CHARACTER)), chain.get(norm(CHARACTER_OVERRIDE))
    return {"calls": dict(sorted(calls.items())), HOST_CHARACTER: u32(host) if host else None,
            CHARACTER_OVERRIDE: u32(override) if override else None}


def row_files(db, rows, locators, volumes, stage=STAGE_ONE):
    """({row: file}, sources) for the stage rows `rows` holds. A file is a rules.json row of skate3_modes::ModeData plus the story's keys:
    title (string id), online, intro (the Intro group's NIS scene or null), objectives (the Objectives list's count), challenge_type, flow
    ([type, count] or null), groups {Objectives | ObjectivesOwned | ObjectivesKilledIt: [{"PointRequirement", "objectives": [objective]}]},
    natives, trigger_volumes {decimal volume id: the career volume plus its name, or null when no file places it (spec 18.20)}, trick_groups,
    trick_group_members {group: [scorable names]} (rules.json keeps these beside its rows; a row file carries its own), points_objective (the
    global points objective), presentation, speech and, when the local data sets it, unique_trick_group. An objective is
    skate3_online.Objectives.objective's (key, activation, triggers, fail_first, count_kind, master, requires_highlight, display_in_hud,
    bonus_time, obj, points_valid, ScoringBoundary) with: hints / hint_keys (the ECAE list in list order), failures / failure_keys (the B69A
    list), inline_failures (FailureObjectives), and groups {intro, complete, fail, hints [[event]], failures [[event]]}; an event is
    {"kind", "args", "flags"}. `sources` is {snippet offset: its Lua text} (scripts/objectives.json, the oracle's input). Retail text:
    it goes only to --out."""
    mine = [r for r in stage if r in rows]
    if not mine:
        return {}, {}
    bank = Bank(normalised(db.collections("challengebanks/main")), ())
    cbin = db.bin("challengebanks/main")
    objectives = StoryObjectives(db, getattr(db, "disc", None))
    names = name_table(db)
    framework = {key_hash(k): r for k, r in Template(db, local_parents=True).framework.items()}
    by_id = {str(v["id"]): dict(v, name=n) for n, v in volumes.items() if "id" in v}
    points = objectives.objective(objectives.by[(OBJECTIVE, GLOBAL_POINTS)], cbin, locators, volumes) if (OBJECTIVE, GLOBAL_POINTS) in objectives.by \
        else None
    files = {}
    for row in mine:
        g = bank.by.get((GLOBAL_DATA, key_hash(row)))
        if g is None:
            raise Refused("missing", row, "its challenge_global_data record is not in the bank")
        gf = {}
        for r in bank.chain(g):
            for k, v in r["fields"].items():
                gf.setdefault(k, v)
        groups = {}
        for name in OBJECTIVE_FIELDS:
            groups[name] = objectives.groups(gf, name, locators, volumes)
            if any(x is None or None in x["objectives"] for x in groups[name]):
                raise Refused("missing", row, f"an objective of the {name} list is not in the challenge bank")
        every = [o for field in groups.values() for x in field for o in x["objectives"]]
        snippets = [s for o in every + ([points] if points else []) for s in [o["obj"], o["points_valid"], *o["hints"], *o["failures"], *o["inline_failures"]]]
        calls = native_calls(*snippets)
        ids = {n[2][0][1] for n in calls if n[1] in TRIGGER_NATIVES and n[2] and n[2][0][0] == "str"}
        tricks = {n[2][0][1] for n in calls if n[1] in TRICK_GROUP_NATIVES and n[2] and n[2][0][0] == "str"}
        members = trick_groups(db, tricks)
        if sorted(members) != sorted(tricks):
            raise Refused("missing", row, f"a snippet names trick groups no skatercollections record holds: {sorted(tricks - set(members))}")
        local = local_data(db, framework, row, names, with_file=True)
        presentation = presentation_of(db, row, gf, local, cbin)
        scene = next((e["args"][0] for e in presentation.get("Intro", []) if e["kind"] == 5), None)
        flow = gf.get(FLOW)
        out = {"title": rows[row]["title"], "online": AVAILABLE_ONLINE in gf and gf[AVAILABLE_ONLINE]["data"][:2] != "00",
               "intro": scene or None, "objectives": len(items(gf.get(OBJECTIVES))), "challenge_type": rows[row]["kind"],
               "flow": list(struct.unpack(">2I", bytes.fromhex(flow["data"][:16]))) if flow else None,
               "groups": groups, "natives": sorted({n for s in snippets if s for n in s["natives"]}),
               "trigger_volumes": {i: unplaced_checked(row, i, by_id) for i in sorted(ids)}, "trick_groups": sorted(tricks), "trick_group_members": members,
               "points_objective": points,
               "presentation": presentation, "speech": speech_of(gf, local)}
        track = local[1].get(norm("TrackUniqueTrickGroup")) if local else None
        if track and int(track["data"][16:32], 16):
            out["unique_trick_group"] = "Hash_" + track["data"][16:32]
        files[row] = out
    return files, {k: objectives.sources[k] for k in sorted(objectives.sources, key=int)}


# ---------------------------------------------------------------- in-challenge speech (speech/)
# The tables are Task 1's (crates/skate-game/src/skate3_career/speech_tables.json): the id -> event pairs of the three bank tables
# (0x8224f1b8 main cast, 0x8224f400 cameraman, 0x8224ef30 living world) and the 104 character names (0x8257b43c..).
SPEECH_TABLES = TOOLS.parent / "crates/skate-game/src/skate3_career/speech_tables.json"
UNKNOWN, CAMERAMAN, REDA, SKATE_COACH, NONE_CHARACTER = 0x27, 0x28, 0x1E, 0x1F, 0x68     # eSk8Characters (stage1_traces 1.1)
SCRIPTED_FROM = 5000                  # 0x824a4f30: an id from 0x1388 up takes the scripted path (nis.big), the rest the banks
SPEECH_EVENTS = (2, 3)                # presentation event kinds Speech and SpeechNoWait (presentation.md "Event types")
# The end-dialog lines use PlayNPCSpeechUsingAttribute, the plain form: Won / FailedAttemptOneObj / Failed / Bailed / LoseTimer
# (objectives_shared.lua L1175-1190, photo_skate2.lua L1104-1119). Every other field goes through PlayNPCSpeechUsingOverrideAttribute
# (0x828438c8 -> 0x828437d0), which swaps AudioCharacterOverride in when the character is 0x27 Unknown. A field no graph reads
# (SignupSpeech, the unnamed ones) takes the override form: INFERRED (it only differs for Unknown, and then the extra bank is a spare).
PLAIN_SPEECH = ("AudioPlayerWonChallenge", "AudioPlayerFailedAttemptOneObj", "AudioPlayerFailedChallenge", "AudioPlayerBailed",
                "AudioLoseTimerExpired")
ELF_SHIFT = 23                        # 0x8296ea10: h = (h << 4) + c; g = h & 0xF0000000; h ^= g ^ (g >> 23)
# Cast bits of the main cast's second set (stage1_traces 1.5: Reda 8, SkateCoach 0x10); the first set has none of them.
CAST_SECOND_SET = {REDA: 0x08, SKATE_COACH: 0x10}


@functools.cache
def speech_tables():
    return json.loads(SPEECH_TABLES.read_text(encoding="utf-8"))


def elf_hash(name):
    """The 32-bit hash the speech handler gives a character name (0x8296ea10, stage1_traces 1.1)."""
    h = 0
    for ch in name.encode("ascii"):
        h = ((h << 4) + ch) & 0xFFFFFFFF
        g = h & 0xF0000000
        h ^= g ^ (g >> ELF_SHIFT)
    return h


def speaker(name, tables=None):
    """A character name's index: a lower_bound over the 104 hashes with NO equality test (0x8257c418-0x8257c44c), so a name the
    table lacks gets the character of the next larger hash ("Slappy" is Vacant10, 62; stage1_traces 1.4). A hash above every
    listed one has no node (the retail lookup would read end()): refused."""
    by_hash = sorted((elf_hash(n), i) for n, i in (tables or speech_tables())["characters"])
    h = elf_hash(name)
    for key, index in by_hash:
        if key >= h:
            return index
    raise Refused("missing", name, "the name hashes above every character name, so retail's lower_bound has no node (0x8257c44c)")


def atoi(text):
    """The leading integer of a string, 0 when there is none (0x82f4dec0)."""
    found = re.match(r"\s*([+-]?\d+)", str(text))
    return int(found.group(1)) if found else 0


def event_lists(row):
    """Every presentation event list of a row file: the row's groups and each objective's own (intro, complete, fail, hints, failures)."""
    yield from row.get("presentation", {}).values()
    for field in row.get("groups", {}).values():
        for group in field:
            for objective in group["objectives"]:
                g = objective["groups"]
                yield from (g["intro"], g["complete"], g["fail"])
                yield from g["hints"]
                yield from g["failures"]


def row_speech_lines(row, tables=None):
    """{(character, line)} a row file names, a line of 0 left out (id 0 is in no table, stage1_traces 1.5): its speech fields
    (rows/<row>.json "speech") and its Speech / SpeechNoWait events (character by name, line by atoi). Unknown (0x27) is replaced by
    the row's AudioCharacterOverride unless that is None (0x68), in the override form (PLAIN_SPEECH fields excepted; events always
    use it: 0x8257c458-0x8257c4b8)."""
    override = row["speech"].get("AudioCharacterOverride")

    def effective(character, plain):
        return override if character == UNKNOWN and not plain and override not in (None, NONE_CHARACTER) else character

    out = {(effective(ch, field in PLAIN_SPEECH), line) for field, (ch, line) in row["speech"]["calls"].items() if line}
    for events in event_lists(row):
        for e in events:
            if e["kind"] in SPEECH_EVENTS and atoi(e["args"][1]):
                out.add((effective(speaker(e["args"][0], tables), False), atoi(e["args"][1])))
    return out


def speech_banks(character, line):
    """The bank tables a bank line (id < 5000) of `character` is looked up in (stage1_traces 1.2): the cameraman bank for 0x28; the
    main cast for Reda and the coach (their aud_characteristics inherit class 0 from `default`; the table writer was not traced, so
    INFERRED); Unknown's speaker is "a nearby world character" (INFERRED) so the main cast and the living world. The other
    characters' paths (Cuz, Cyrus 0x824aa148, class-1 speakers 0x824ab590) are not ported: refused."""
    if character == CAMERAMAN:
        return ("cameraman",)
    if character == UNKNOWN:
        return ("maincast", "livingworld")
    if character in (REDA, SKATE_COACH):
        return ("maincast",)
    raise Refused("missing", f"character {character}", f"line {line}: this character's bank path is not ported (stage1_traces 1.2)")


def table_event(tables, bank, line):
    """The event the bank's id table gives a line, None when the table lacks it (the line plays nothing)."""
    return {i: e for i, e in tables[bank]["pairs"]}.get(line)


def speech_plan(files, tables=None):
    """What the rows' speech needs: {"scripted": [(character, line)], "banks": {bank: [event]}}, both sorted."""
    tables = tables or speech_tables()
    scripted, banks = set(), {}
    for row in files.values():
        for character, line in row_speech_lines(row, tables):
            if line >= SCRIPTED_FROM:
                scripted.add((character, line))
                continue
            for bank in speech_banks(character, line):
                event = table_event(tables, bank, line)
                if event is not None:
                    banks.setdefault(bank, set()).add(event)
    return {"scripted": sorted(scripted), "banks": {b: sorted(banks[b]) for b in sorted(banks)}}


def speech_missing(what, action):
    """Run `action()`; a missing or damaged disc file becomes a refusal naming `what`."""
    try:
        return action()
    except (OSError, ValueError, KeyError) as e:
        raise Refused("missing", what, f"{type(e).__name__}: {e}")


def speech_layout(disc, files, work):
    """(speech, jobs) for the rows' speech, reading no audio. `speech` is speech.json:
    {"scripted": {"<character>_<id>": [wav]}, "banks": {bank: {"<event>": [{"masks": [..], "files": [wav]}]}},
    "conditions": {bank: {"<event>": [type]}}}, a wav being the file name under speech/ (`<sample>_<take>.wav`, a scripted variant's
    `<name>_0.wav`). Every leaf of a named event is kept, with the masks the game matches against the speaker (resolve_line); a
    leaf whose sample is not in the bank has no files (retail finds nothing to play). `jobs` is [(wav names, produce)]; produce(command)
    yields each WAV, one take at a time."""
    tables = speech_tables()
    plan = speech_plan(files, tables)
    folder = Path(disc) / "data/audio/english"
    jobs, taken = [], {}

    def claim(names, owner):
        for name in names:
            if taken.setdefault(name, owner) != owner:
                raise Refused("key collision", name, f"two banks' samples make the wav name: {taken[name]} and {owner}")

    scripted = {}
    if plan["scripted"]:
        lines = speech_missing("nis.big", lambda: ea_speech.ScriptedLines(folder / "nis.big", work))
        variants = speech_missing("nis.big", lines.variants)
        for key in plan["scripted"]:
            names = []
            for stem in variants.get(key, []):
                name = f"{stem}_0.wav"
                claim([name], f"nis.big {stem}")
                jobs.append(([name], lambda command, stem=stem: [lines.wav(stem, command)]))
                names.append(name)
            scripted[f"{key[0]}_{key[1]}"] = names
    banks, conditions, entries = {}, {}, 0
    for bank, events in plan["banks"].items():
        reader = speech_missing(f"{bank}speech.big", lambda: ea_speech.SpeechBank(folder / f"{bank}speech.big", bank, work))
        leaves, types, samples = speech_missing(f"{bank} events", lambda: (reader.events(), reader.conditions(), reader.samples()))
        decoded = {}
        banks[bank], conditions[bank] = {}, {}
        for event in events:
            if event not in leaves:
                raise Refused("missing", f"{bank} event {event:#x}", "the id table names it and the bank's .evt has no such event")
            out = []
            for masks, sid in leaves[event]:
                sample = samples.get(sid)
                if sample is not None and sample not in decoded:
                    count = len(speech_missing(sample, lambda: reader.takes_of(sample)))
                    decoded[sample] = [f"{sample}_{n}.wav" for n in range(count)]
                    claim(decoded[sample], f"{bank} {sample}")
                    jobs.append((decoded[sample], lambda command, reader=reader, sample=sample: reader.wavs(sample, command)))
                out.append({"masks": list(masks), "files": decoded.get(sample, [])})
            banks[bank][str(event)] = out
            conditions[bank][str(event)] = types[event]
            entries += len(out)
    return {"scripted": scripted, "banks": banks, "conditions": conditions}, jobs


def speech_export(disc, files, work, vgmstream=None):
    """(speech, {"speech/<wav>": file in `work`}, summary): speech_layout, then every take decoded by vgmstream (SKATE_VGMSTREAM) into
    `work`/speech, one at a time. Retail audio: the files go only to --out. The summary counts the WAVs, their bytes, the scripted
    lines asked for (and those with no file), the bank entries (leaves) and the leaves whose sample the bank lacks."""
    try:
        command = ea_speech.vgmstream_command(vgmstream)
    except RuntimeError as e:
        raise Refused("missing", "vgmstream", str(e))
    speech, jobs = speech_layout(disc, files, work)
    folder = Path(work) / "speech"
    folder.mkdir(parents=True, exist_ok=True)
    wavs, total = {}, 0
    for names, produce in jobs:
        done = 0
        try:
            for data in produce(command):
                path = folder / names[done]
                path.write_bytes(data)
                wavs[f"speech/{names[done]}"] = path
                total += len(data)
                done += 1
        except (RuntimeError, OSError, ValueError, KeyError) as e:
            raise Refused("missing", names[done] if done < len(names) else names[-1], f"decoding failed: {e}")
        if done != len(names):
            raise Refused("missing", names[0], f"{done} of its {len(names)} takes decoded")
    leaves = [leaf for bank in speech["banks"].values() for event in bank.values() for leaf in event]
    summary = {"wavs": len(wavs), "bytes": total, "scripted": len(speech["scripted"]),
               "scripted_missing": sorted(k for k, v in speech["scripted"].items() if not v), "bank_entries": len(leaves),
               "bank_events": sum(len(b) for b in speech["banks"].values()), "leaves_without_sample": sum(1 for x in leaves if not x["files"])}
    return speech, wavs, summary


def condition_values(bank, character, host, argument):
    """{condition type: value} the game matches a leaf's masks against, for the types it is known to set (INFERRED, stage1_traces
    1.5; a type not listed matches every leaf): cameraman type 1 is the speaker mask from the host (0x824acbc0: Reda 0x1E -> 2,
    SkateCoach 0x1F -> 4, else 1) and type 2 the caller's argument when it passes one (0x8007); main cast type 1 is the first cast
    set (none of the cast bits belong to Reda or the coach) and type 2 the second set's bit."""
    if bank == "cameraman":
        values = {1: 2 if host == REDA else 4 if host == SKATE_COACH else 1}
        if argument is not None:
            values[2] = argument
        return values
    if bank == "maincast" and character in CAST_SECOND_SET:
        return {1: 0, 2: CAST_SECOND_SET[character]}
    return {}


def resolve_line(speech, character, line, host=NONE_CHARACTER, override=None, plain=False, argument=None):
    """The reference reading of stage1_traces 1.2 / 1.5 over an exported speech.json (the Rust speech.rs ports it): the wav names a
    request (character, line) can play, in leaf order; [] when nothing plays. A line of 0, an id in no table and a scripted line
    with no file play nothing. A leaf matches when each of its masks is 0 or shares a bit with its condition (INFERRED, 1.5)."""
    if not line:
        return []
    if character == UNKNOWN and not plain and override not in (None, NONE_CHARACTER):
        character = override
    if line >= SCRIPTED_FROM:
        return list(speech["scripted"].get(f"{character}_{line}", []))
    tables, out = speech_tables(), []
    for bank in speech_banks(character, line):
        event = table_event(tables, bank, line)
        leaves = speech["banks"].get(bank, {}).get(str(event)) if event is not None else None
        if leaves is None:
            continue
        values = condition_values(bank, character, host, argument)
        types = speech["conditions"][bank][str(event)]
        for leaf in leaves:
            if all(m == 0 or m & values[t] for m, t in zip(leaf["masks"], types) if t in values):
                out += leaf["files"]
    return out


# ---------------------------------------------------------------- the stage-1 scenes (scenes/)
# The NIS stage 1 plays or holds (stage1_nis 1): the five tutorial intros, the photo and film intros, the two Story scenes of the
# University and Create_start_intro (held in the save for stage 2, spec 14). The inventory's other two rows are FE movies, not
# scenes; 01_Reda_Welcome (no 99_6003 stream, rendered into intro_movie, INFERRED stage1_nis 1) and 06_Coach_Frank_Intro (only the
# GPMM demo plays it) are never played.
SCENES = ("tut_sksc_01", "tut_sksc_02", "tut_sksc_06", "tut_sksc_03", "tut_sksc_04", "photo_univ_08", "film_univ_05",
          "02_Reda_Passes_Baton", "03_Reda_Leaves_Shingo", "Create_start_intro")
NO_SUBTITLES = ("Create_start_intro",)       # stage1_nis 3.5: the disc has no .sub for it (only Create_basic, Create_funbox ...)
POST_BANK = 99                                # AUDIO_POST id N plays `99_N_*` (spec 15.3); a post with no 99_ stream plays nothing (spec 18.2)
SUBTITLES_BIG = "data/fe/subtitles/subtitles.big"
LANGUAGE_BIN = "data/fe/languages/english/language_english_global_skate3ng.bin"
# The camera effect columns (spec 15.7, brief Task 8). Each is the ICE parameters of one effect in column order after the time; a
# parameter's track gives its key times. The rows are the take's own keys (+ both sides of a cut); the cutscene player loads them and, for now,
# draws none but the brightness dip (spec 18.4: the BRIGHTNESS_* dip scales scene exposure, DOF and SHADOW_* keys are exported, not drawn;
# NEAR_CLIP / CLIPPING_FAR likewise); AVATAR_EYE is exported and ignored, the camera keeping its world eye (spec 18.3).
# Not retail (user 10-20): the player lines up rows with straight lines. Retail interpolates between keys by Hermite curves where the
# parameter has a cubic link (old_tutorial.ice_value, the exe's 0x828408a8). Of these columns only the dof ones have one (CUBIC_FOCUS for
# FOCAL_DISTANCE / APERTURE, CUBIC_RAWFOCUS for NEAR_FOCUS / FAR_FOCUS / BLUR_*, in the exe's ICE table): their rows approximate the curve.
# Brightness, shadow, clip and eye have no cubic link (tangent = the key's delta, i.e. a line, or a step), so their rows are exact. The 17
# camera columns are not affected (sampled at 60 Hz).
EFFECTS = {
    "brightness": ("BRIGHTNESS_OVERRIDE", "BRIGHTNESS_NEAR", "BRIGHTNESS_FAR"),
    "dof": ("FOCAL_DISTANCE", "APERTURE", "NEAR_FOCUS", "FAR_FOCUS", "BLUR_FALLOFF", "BLUR_INTENSITY", "RAWFOCUS_OVERRIDE", "RAWFOCUS_RELATIVE"),
    "shadow": ("SHADOW_OVERRIDE", "SHADOW_SCALE", "SHADOW_X_OFFSET", "SHADOW_Y_OFFSET", "SHADOW_Z_OFFSET", "SHADOW_BIAS", "SHADOW_PRECISION",
               "SHADOW_PRECISION_OVERRIDE"),
    "near_clip": ("NEAR_CLIP",),
    "far_clip": ("CLIPPING_FAR",),
    "avatar_eye": ("AVATAR_EYE",),
}


def disc_entry(archive, path):
    """The bytes of one entry of a .big (case-insensitive path); a missing entry is a refusal, not a SystemExit."""
    entry = next((e for e in archive.entries if e.path.lower() == path.lower()), None)
    if entry is None:
        raise Refused("missing", path, f"not in {getattr(archive, 'path', 'the archive')}")
    return archive.read(entry)


def parse_scene_records(data, names):
    """{scene: {"hash", "kind", "origin", "duration", "tracks": [{"character", "package", "anim", "clip"}]}} for `names` out of
    scene_000.bin ('scen', stage1_nis 2 / 3.1; the layout old_tutorial.scenes reads): 0x68-byte records from 0x10 (hash u32 BE,
    kind u8 at 4, origin and duration as f32 BE from 8, track count and first track u16 at 0x24, name at 0x28), then 180-byte tracks
    (character 0:32, package 32:48, anim 48:112, clip 112:176). A name the file lacks, or twice, is refused."""
    if len(data) < 16 or data[:4] != b"scen":
        raise Refused("missing", "scene_000.bin", "not a 'scen' file")
    count = struct.unpack_from(">I", data, 8)[0]
    tracks_at = 0x10 + count * 0x68
    if tracks_at > len(data):
        raise Refused("missing", "scene_000.bin", "the record table runs past the file")
    text = lambda at, a, b: data[at + a:at + b].split(b"\0")[0].decode("latin-1")      # noqa: E731
    out = {}
    for i in range(count):
        o = 0x10 + i * 0x68
        name = text(o, 0x28, 0x68)
        if name not in names:
            continue
        if name in out:
            raise Refused("key collision", name, "two scene records carry the name")
        f = struct.unpack_from(">7f", data, o + 8)
        n, first = struct.unpack_from(">HH", data, o + 0x24)
        tracks = []
        for t in range(first, first + n):
            r = tracks_at + 180 * t
            if r + 180 > len(data):
                raise Refused("missing", name, "a track runs past the end of scene_000.bin")
            tracks.append({"character": text(r, 0, 32), "package": text(r, 32, 48), "anim": text(r, 48, 112), "clip": text(r, 112, 176)})
        out[name] = {"hash": struct.unpack_from(">I", data, o)[0], "kind": data[o + 4], "origin": list(f[0:3]), "duration": f[6], "tracks": tracks}
    for name in names:
        if name not in out:
            raise Refused("missing", name, "scene_000.bin has no record of that name")
    return out


def camera_segments(ice, takes):
    """([(take, a, b, shift)], the edit take or None): the stretches of take time [a, b] the scene's camera plays, at scene time t + shift
    (old_tutorial.scene_camera: one take plays as it is; with take-switch keys the edit's shots play their takes)."""
    edit = next((t for t in takes if any(t["values"]["CONTAINS_SUBTAKE"])), None)
    if edit is None:
        return [(takes[0], 0.0, takes[0]["duration"], 0.0)], None
    return [(takes[n], start, start + t1 - t0, t0 - start) for n, start, t0, t1 in ice["edit_shots"](edit, takes)], edit


def segment_rows(ice, take, a, b, shift, names, tracks):
    """[[scene time, value, ...]] of `names` over take seconds [a, b]: one row at the start, one at each key time of `tracks` (two where a
    value jumps there: the limit from before, then the value after, as ice_rows does for a camera cut) and the limit at the end."""
    value, dur = ice["ice_value"], take["duration"]
    inner = {x for track in tracks for x in take["keys"][track][1][1:-1] if a < x * dur < b}
    end, rows = b / dur, []
    for u in sorted({a / dur, end} | inner):
        after = [round(float(value(take, n, u, False)), 5) for n in names]
        before = [round(float(value(take, n, u, True)), 5) for n in names] if u in inner else after
        at = round(u * dur + shift, 5)
        if u == end or (u in inner and before != after):
            rows.append([at, *before])
        if u != end:
            rows.append([at, *after])
    return rows


def effect_keys(ice, segments, effects=None):
    """{effect: [[t, value, ...], ...]} over the camera's stretches (camera_segments), for each of `effects` (default EFFECTS). A row that
    repeats both neighbours' values is dropped (a constant run is its two ends); the stretches before the first and after the last row
    that changes the picture are dropped too, so outside the keys the picture is the shot's own (the player returns 1.0 there, spec 15.7).
    "Changes the picture" is: any value off its ICE default; for the brightness a row with the override off, or near and far both 0, adds
    nothing (INFERRED (spec 15.7: the dip of 03_Reda_Leaves_Shingo spans 39.33-43.84 s although its override is on from 0, stage1_nis 3.2; plan
    Task 27: the player returns 1.0 outside the keys): 0 is the identity of an additive brightness). The player draws only the brightness dip;
    DOF, shadow, clip and AVATAR_EYE rows are exported and not drawn (spec 18.4, 18.3)."""
    params = ice["ICE"]
    out = {}
    for key, names in (EFFECTS if effects is None else effects).items():
        missing = [n for n in names if n not in params]
        if missing:
            raise Refused("missing", missing[0], "not a parameter of the exe's ICE table")
        tracks = sorted({params[n]["track"] for n in names})
        rows = []
        for take, a, b, shift in segments:
            rows += segment_rows(ice, take, a, b, shift, names, tracks)
        rows = [r for i, r in enumerate(rows) if not (0 < i < len(rows) - 1 and r[1:] == rows[i - 1][1:] == rows[i + 1][1:])]
        defaults = [round(float(params[n]["default"]), 5) for n in names]
        if key == "brightness":
            plain = lambda v: v[0] == 0 or (v[1] == 0 and v[2] == 0)                    # noqa: E731
        else:
            plain = lambda v: v == defaults                                              # noqa: E731
        live = [i for i, r in enumerate(rows) if not plain(r[1:])]
        out[key] = rows[max(live[0] - 1, 0):live[-1] + 2] if live else []
    return out


def audio_keys(ice, takes, edit):
    """([(time, post id)], [(time, speech id, character)]) the takes key with command 2 (play): skate3_online.scene_posts for the
    AUDIO_POST_ID keys, the same reading for AUDIO_SPEECH_ID / AUDIO_SPEECH_CHARACTER (a key's time is where it starts). An id keyed in
    several takes plays once, at its earliest time (as old_tutorial.post_ids: an edit's sub-takes repeat the bed). In an edit
    (`edit`) the take's time is not the scene's, and every key of the stage-1 scenes is at 0: a later one is refused rather than
    placed by a guess."""
    posts, speech = {}, {}
    for at, ident in online.scene_posts(ice, takes):
        posts[ident] = min(at, posts.get(ident, at))
    track = ice["ICE"]["AUDIO_SPEECH_CMD"]["track"]
    for t in takes:
        v = t["values"]
        times = t["keys"][track][1] if track < len(t["keys"]) else []
        for j, cmd in enumerate(v.get("AUDIO_SPEECH_CMD", [])):
            if cmd == 2 and j < len(v.get("AUDIO_SPEECH_ID", [])) and j < len(v.get("AUDIO_SPEECH_CHARACTER", [])):
                at = round(times[j] * t["duration"], 3) if j < len(times) else 0.0
                key = (v["AUDIO_SPEECH_ID"][j], v["AUDIO_SPEECH_CHARACTER"][j])
                speech[key] = min(at, speech.get(key, at))
    if edit and any(at for at in [*posts.values(), *speech.values()]):
        raise Refused("missing", "audio key", "an edit keys audio after its start; its time on the edit's clock is not decoded")
    return sorted((at, i) for i, at in posts.items()), sorted((at, i, ch) for (i, ch), at in speech.items())


def fold_wav(data):
    """A vgmstream WAV with three or more channels folded to stereo by the retail fold-down (skate3_online.fold_stereo, the one the online
    beds use; the 5-channel NIS beds are L C R Ls Rs); one or two channels are returned as they are. Only 16-bit PCM is read."""
    import io
    import wave
    import numpy as np
    try:
        with wave.open(io.BytesIO(data)) as w:
            channels, width, rate, raw = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.readframes(w.getnframes())
    except (wave.Error, EOFError) as e:
        raise Refused("missing", "wav", f"not a WAV: {e}")
    if width != 2:
        raise Refused("missing", "wav", f"{8 * width}-bit samples; only 16-bit PCM is read")
    if channels < 3:
        return data
    mixed = online.fold_stereo(np.frombuffer(raw, "<i2").reshape(-1, channels).astype(np.float32))
    out = io.BytesIO()
    with wave.open(out, "wb") as w:
        w.setnchannels(mixed.shape[1])
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(np.clip(np.round(mixed), -32768, 32767).astype("<i2").tobytes())
    return out.getvalue()


def subtitle_cues(raw, values, duration):
    """[[start, end, text]] of one English .sub, read as prepare_retail_screens.data reads it (8-byte records: u16 LE time in 1/10 s,
    pad, u32 LE string key; the blanking key CLEAR; the 0xFFFF terminator; `_text` for the font's apostrophe, ellipsis and the `^`
    line break). A cue runs from its time to the next record's (a blank ends it); a last cue with no blank runs to `duration`, the
    scene's length (INFERRED (spec 15.5: a cue that outlasts its scene is cut when the scene ends; no stage-1 .sub lacks a closing blank key):
    nothing blanks it, the scene's end does). A key no language table holds is refused. Retail text:
    --out only."""
    import prepare_retail_screens as retail
    records = []
    for at in range(0, len(raw) - 7, 8):
        time, _, key = struct.unpack_from("<HHI", raw, at)
        if time == retail.END:
            break
        records.append((time / 10, key))
    cues = []
    for i, (start, key) in enumerate(records):
        if key == retail.CLEAR:
            continue
        if key not in values:
            raise Refused("missing", f"string {key:#x}", "a .sub cue names a key the English language table lacks")
        cues.append([start, records[i + 1][0] if i + 1 < len(records) else max(duration, start), retail._text(values[key])])
    return cues


@functools.lru_cache(maxsize=2)
def ice_table_of(disc):
    """The exe's ICE parameter table (skate3_online.ice_table over default.xex), once per disc: skate3_online.ice_state caches by its identity."""
    try:
        return online.ice_table(online.Image(online.xex_image(Path(disc) / "default.xex")))
    except (OSError, AssertionError, StopIteration, KeyError) as e:
        raise Refused("missing", "default.xex", f"the ICE table could not be read: {type(e).__name__}: {e}")


def scene_takes(ice, records, scene):
    """A scene's ICE takes, decoded in file order (skate3_online.intro_cameras' loop)."""
    if scene["hash"] not in records:
        raise Refused("missing", scene["name"], "ice_000.bin has no record of its hash")
    e, at, stop, n = records[scene["hash"]]
    takes = []
    for _ in range(n):
        takes.append(ice["ice_take"](e[at:stop]))
        at = (at + takes[-1]["size"] + 3) & ~3
    return takes


def scene_stream(variants, character, ident, scene):
    """The one nis.big stream `<character>_<ident>_*` (ScriptedLines.variants) a scene's audio key names, or None where the disc has none (no
    99_6774 / 99_7762: those scenes play their speech stream; a post with no stream plays nothing, spec 18.2). Several streams under one id are refused: nothing here picks among them."""
    stems = variants.get((character, ident), [])
    if len(stems) > 1:
        raise Refused("key collision", f"{character}_{ident}", f"{scene}: {len(stems)} streams carry the id and nothing picks one")
    return stems[0] if stems else None


def scene_layout(disc, work):
    """(tutorial, files, jobs, summary) for SCENES, decoding no audio. `tutorial` is {"scenes": {scene: {kind, origin, duration, tracks,
    audio, camera, transition, subtitles}}} in cutscene.rs's `Scenes` format (what it reads, plus the brief's additions the reader
    ignores until Task 27): a track is {character, package, anim, clip} (`clip1` is Skate 1's Clip1Meta struct, so the clip's name is
    `clip`); an audio entry {audio, kind "post"|"speech", id, character, at, seconds}; subtitles [[start, end, text]]. `files` maps a
    path under the export to a Path in `work`: scenes/scene/camera/<scene>.json (skate3_online.intro_cameras' camera, camera_only for
    every scene so the actor scenes are kept, plus "effects" and "effect_columns"), scenes/scene/anim/<bank>.abin (scene.big's
    banks as they are, and the Female1 take beside each Male1 one). `jobs` is [(scenes/nis/<stream>.wav, produce)]; produce(command)
    gives the folded WAV. Retail data: --out only."""
    disc, work = Path(disc), Path(work)
    work.mkdir(parents=True, exist_ok=True)
    boot = online.big(disc, "data/big/miscboot.big")
    records = parse_scene_records(disc_entry(boot, "data/scene/scene_000.bin"), SCENES)
    table = ice_table_of(str(disc))
    # The cameras are the online exporter's; its actor-skip rule stays (camera_only keeps every scene), and its own post.big beds
    # (none of these ids: post.big holds only the 99_91xx beds) and its printed notes are not used.
    notes = StringIO()
    with redirect_stdout(notes):
        cameras = online.intro_cameras(disc, table, set(SCENES), work / "cam", camera_only=SCENES)
    for name in SCENES:
        if name not in cameras:
            raise Refused("missing", name, "skate3_online.intro_cameras did not export its camera: " + (notes.getvalue().strip().splitlines() or ["?"])[-1])
    ice, full, _, _ = online.ice_state(disc, table)
    ice_by_hash = online.ice_records(full)
    scene_big = online.big(disc, "data/big/scene.big")
    banks = {e.path.lower(): e for e in scene_big.entries if e.path.lower().startswith("data/scene/anim/")}
    import prepare_retail_screens as retail
    values = retail._keyed(disc_entry(boot, LANGUAGE_BIN))
    sub_archive = online.big(disc, SUBTITLES_BIG)
    subs = {Path(e.path).stem.lower(): e for e in sub_archive.entries if "/subtitles_english/" in e.path.lower()}
    lines = speech_missing("nis.big", lambda: ea_speech.ScriptedLines(disc / "data/audio/english/nis.big", work))
    variants = speech_missing("nis.big", lines.variants)
    scenes, files, jobs, missing_beds, copied = {}, {}, [], [], {}
    (work / "anim").mkdir(exist_ok=True)

    def bank_file(anim):
        if anim not in copied:
            entry = banks.get("data/" + anim.lower())
            if entry is None:
                return False
            path = work / "anim" / Path(anim).name
            path.write_bytes(scene_big.read(entry))       # one bank at a time
            copied[anim] = path
            files["scenes/" + anim] = path
        return True

    for name in SCENES:
        record = records[name]
        takes = scene_takes(ice, ice_by_hash, {**record, "name": name})
        segments, edit = camera_segments(ice, takes)
        camera_path = work / "cam" / f"scene/camera/{name}.json"
        camera = json.loads(camera_path.read_text(encoding="utf-8"))
        camera["effects"] = effect_keys(ice, segments)
        camera["effect_columns"] = {k: ["t", *v] for k, v in EFFECTS.items()}
        camera_path.write_bytes((json.dumps(camera, separators=(",", ":")) + "\n").encode("utf-8"))
        files[f"scenes/scene/camera/{name}.json"] = camera_path
        for track in record["tracks"]:
            if not bank_file(track["anim"]):
                raise Refused("missing", track["anim"], f"scene.big has no such bank (a track of {name})")
            female = track["anim"].replace("_Male1.abin", "_Female1.abin")
            if female != track["anim"]:
                bank_file(female)          # the reader swaps in the female take when it exists; Skate 3's player banks are _Male1 / _Female1
        used = [seg[0] for seg in segments] + ([edit] if edit else [])
        posts, speech = audio_keys(ice, used, edit is not None)
        audio = []
        for kind, character, ident, at in ([("post", POST_BANK, i, t) for t, i in posts] + [("speech", ch, i, t) for t, i, ch in speech]):
            stem = scene_stream(variants, character, ident, name)
            if stem is None:
                missing_beds.append((name, kind, character, ident))       # spec 18.2: a post with no 99_ stream plays nothing (no 99_6774 / 99_7762: the speech stream is the scene's audio)
                continue
            take = lines.take(stem)
            rel = f"nis/{stem}.wav"
            if all(rel != job[0] for job in jobs):
                jobs.append((rel, lambda command, stem=stem: fold_wav(lines.wav(stem, command))))
            audio.append({"audio": rel, "kind": kind, "id": ident, "character": character, "at": at, "seconds": round(take.frames / take.hertz, 3)})
        if not audio:
            raise Refused("missing", name, "none of its audio keys names a stream on the disc")
        if name in NO_SUBTITLES:
            cues = []
        else:
            entry = subs.get(name.lower())
            if entry is None:
                raise Refused("missing", f"{name}.sub", "no English subtitle file carries the scene's name")
            cues = subtitle_cues(sub_archive.read(entry), values, record["duration"])
        scenes[name] = {"kind": cameras[name]["kind"], "origin": record["origin"], "duration": record["duration"], "tracks": record["tracks"],
                        "audio": audio, "camera": f"scene/camera/{name}.json", "transition": cameras[name]["transition"], "subtitles": cues}
    summary = {"scenes": len(scenes), "actors": {n: len(s["tracks"]) for n, s in scenes.items()}, "cues": {n: len(s["subtitles"]) for n, s in scenes.items()},
               "audio_files": len(jobs), "audio_bytes": 0, "anim_banks": len(copied), "anim_bytes": sum(p.stat().st_size for p in copied.values()),
               "missing_beds": missing_beds}
    return {"scenes": scenes}, files, jobs, summary


def scene_export(disc, work, vgmstream=None):
    """(files, summary): scene_layout, then every stream decoded by vgmstream (SKATE_VGMSTREAM), one at a time, into work/nis and folded to
    stereo. `files` maps scenes/tutorial.json to the dict and every other path under the export to a Path in `work`. Retail data."""
    try:
        command = ea_speech.vgmstream_command(vgmstream)
    except RuntimeError as e:
        raise Refused("missing", "vgmstream", str(e))
    tutorial, files, jobs, summary = scene_layout(disc, work)
    files = {"scenes/tutorial.json": tutorial, **files}
    for rel, produce in jobs:
        try:
            data = produce(command)
        except (RuntimeError, OSError, ValueError, KeyError) as e:
            raise Refused("missing", rel, f"decoding failed: {e}")
        path = Path(work) / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        files["scenes/" + rel] = path
        summary["audio_bytes"] += len(data)
    return files, summary


# ---------------------------------------------------------------- the DLC folder
STFS_MAGIC = (b"CON ", b"LIVE", b"PIRS")
# skate3_online.dlc_packages skips a package its reader cannot open and prints a line. Retail's "Time is Money" unlock package
# (about 120 KB) has no database and is skipped that way; a challenge package is megabytes. Skipping a big one would drop rows.
SKIP_LINE = re.compile(r"skipped DLC package (.+) \((\d+) bytes\)")
SMALLEST_REAL_PACKAGE = 1 << 20


def stfs_end(header):
    """Where an STFS package's last block ends, from its header: the file size a complete package has (MEASURED: it equals the
    size of all nine retail packages, bar zero padding on one). The 4 KiB blocks sit after the header, one hash block per 0xAA
    data blocks, one per 0xAA of those, and one more above that; the volume descriptor holds the block count."""
    header_size = struct.unpack(">I", header[0x340:0x344])[0]
    vd = header[0x379:0x379 + 0x24]
    total = struct.unpack(">I", vd[0x1C:0x20])[0]
    sex = (~vd[2]) & 1
    last = max(total - 1, 0)
    at = (((last + 0xAA) // 0xAA) << sex) + last
    if last >= 0xAA:
        at += ((last + 0x70E4) // 0x70E4) << sex
    if last >= 0x70E4:
        at += 1 << sex
    return (at << 12) + ((header_size + 0xFFF) & ~0xFFF) + 0x1000


def load_dlcs(folder, db, work, say=print):
    """The DLC packages under `folder` as DlcDbs (skate3_online.dlc_packages). A folder that does not exist, and a package that
    is cut short or damaged, refuse (spec 2: a partial file is refused, not skipped). A folder that yields no package says so
    in one line and exports the disc rows alone."""
    folder = Path(folder)
    if not folder.is_dir():
        raise Refused("missing", str(folder), "the DLC folder does not exist")
    for pkg in sorted(q for q in folder.rglob("*") if q.is_file()):
        with open(pkg, "rb") as f:
            head = f.read(0x1000)
        if head[:4] not in STFS_MAGIC:
            continue
        if len(head) < 0x1000:
            raise Refused("missing", pkg.name, f"the package is {pkg.stat().st_size} bytes, shorter than its header")
        need = stfs_end(head)
        if pkg.stat().st_size < need:
            raise Refused("missing", pkg.name, f"the package is {pkg.stat().st_size} bytes of the {need} its header counts: cut short")
    seen = StringIO()
    with redirect_stdout(seen):
        packages = dlc_packages(folder, db, work)
    for line in seen.getvalue().splitlines():
        say(line)
        found = SKIP_LINE.search(line)
        if found and int(found.group(2)) >= SMALLEST_REAL_PACKAGE:
            raise Refused("missing", found.group(1), f"the reader skipped a {found.group(2)} byte package, which holds a database")
    if not packages:
        say(f"no DLC package with a challenge database under {folder}: exporting the disc rows only")
    return packages


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--disc", type=Path, required=True, help="extracted Skate 3 disc folder")
    p.add_argument("--dlc", type=Path, help="folder of the Skate 3 DLC packages (their rows and strings join the export)")
    p.add_argument("--assets", type=Path, required=True, help="the install's assets folder, read only (the language table)")
    p.add_argument("--out", type=Path, required=True, help="folder that receives career.json, strings.json, scripts/, rows/, speech/, scenes/, cast/, screens/ and ai/ "
                   "(the scripts need SKATE_JAVA and SKATE_UNLUAC, the speech and the scene beds SKATE_VGMSTREAM)")
    a = p.parse_args(argv)
    language = whole_json(a.assets / "private/challenge-map/runtime/challengemap.json")["language"]
    with tempfile.TemporaryDirectory(prefix="skate3-career-") as work:
        try:
            db = CareerDb(a.disc, work)
            dlcs = load_dlcs(a.dlc, db, Path(work) / "dlc") if a.dlc else []
            summary = export(db, a.out, dlcs, language, speech=lambda files, work: speech_export(a.disc, files, work),
                             scenes=lambda work: scene_export(a.disc, work), cast=lambda work: cast_part(a.disc, a.assets, work),
                             screens=lambda work: screens_part(a.disc, a.assets, work), ai=lambda work: ai_part(a.disc, work))
        except Refused as e:
            print(f"refused ({e})", file=sys.stderr)
            return 2
    print(f"wrote {a.out / 'career.json'} ({summary['rows']} rows, {summary['reward_sites']} rewards with sites) and "
          f"{a.out / 'strings.json'} ({summary['strings']} strings); {summary['scripts']} scripts under {a.out / 'scripts'} "
          f"({summary['handlers']} handlers); {summary['locators']} locators, {summary['volumes']} volumes and "
          f"{summary['indicators']} indicator items for the stage-1 rows; {summary['row_files']} row files under {a.out / 'rows'} "
          f"(objectives: {', '.join(f'{r} {n}' for r, n in summary['objectives'].items())})")
    sp = summary["speech"]
    print(f"speech: {sp['wavs']} WAVs ({sp['bytes']} bytes) under {a.out / 'speech'}; {sp['scripted']} scripted lines "
          f"({len(sp['scripted_missing'])} with no file: {', '.join(sp['scripted_missing']) or 'none'}), {sp['bank_entries']} bank entries "
          f"over {sp['bank_events']} events ({sp['leaves_without_sample']} without a sample in the bank)")
    sc = summary["scenes"]
    print(f"scenes: {sc['scenes']} under {a.out / 'scenes'} (actors: {', '.join(f'{n} {k}' for n, k in sc['actors'].items())}); {sc['audio_files']} audio files "
          f"({sc['audio_bytes']} bytes) under {a.out / 'scenes/nis'}; {sc['anim_banks']} actor banks ({sc['anim_bytes']} bytes); subtitle cues: "
          f"{', '.join(f'{n} {k}' for n, k in sc['cues'].items())}; ids with no stream: {sc['missing_beds'] or 'none'}")
    cs = summary["cast"]
    joints = ", ".join(f"{k} {n} ({cs['bytes'][k]} bytes)" for k, n in cs["joints"].items())
    print(f"cast: {cs['models']} models ({cs['total_bytes']} bytes) under {a.out / 'cast'}; joints: {joints}")
    sr = summary["screens"]
    print(f"screens: {sr['movies']} movies ({sr['files']} files, {sr['bytes']} bytes) under {a.out / 'screens'}")
    ai = summary["ai"]
    print(f"ai: {ai['rows']} rows ({ai['paths']} paths, {ai['tricks']} trick ids) under {a.out / 'ai'}")
    if summary["missing"]:
        print(f"{len(summary['missing'])} ids named by rows, rewards and characters are not in the language table", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
