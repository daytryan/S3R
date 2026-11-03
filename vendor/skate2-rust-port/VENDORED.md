# The vendored Skate 2 Rust port

Not tested in play.

This folder is its own cargo workspace, apart from the engine's. It holds the simulation crates of the MIT-licensed
Skate 2 Rust port and `skate2-physics-host`, the library that runs them for the game (see
`docs/superpowers/specs/2026-10-20-skate2-physics-switch-design.md`). The game links the library and runs the port on
a thread of its own (Amendment A: one program, `crates/skate-game/src/skate2_physics/sim.rs`); the host's binary is
only the headless test tool.

## The source

- Upstream: https://github.com/skate6743/skate2-rust-port (branch `main`).
- Commit: `f72f2c1302bf9cf176f842ace7b441d1824feba4` (v0.2.3).
- Copied: 2026-10-20, from a shallow clone of that commit.
- Crates copied: `crates/skate2-core` and `crates/skate2-formats`, 2541 files.
- License: MIT, Copyright (c) 2026 skate6743 and contributors. The text is `LICENSE` in this folder.
- Not copied: the port's `apps/`, `skate2-audio`, `skate2-symbols`, `skate2-update`, `skate2-xma`, and its real
  `skate2-net` (sockets and crypto).
- `crates/skate2-net` is our own offline stub. Its `src/port/{codec,frames,protocol,range}.rs` are the
  port's `crates/skate2-net/src/{codec,frames,protocol,range}.rs`, unchanged apart from LF line endings. Its `client` and `crypto` modules are offline
  stand-ins: no socket is opened and no crypto crate is used.
- `host/` is ours (the package `skate2-physics-host`), not the port's.

## The rule

The two copied crates are **copied unchanged except the lines marked `ENGINE HOOK (user 10-20)`**. Every such line is
listed in the Hooks table below, with the reason. A change that goes against native Skate 2 also carries
`Not native Skate 2 (user 10-20)` and its reason.

Two other changes:

- The stub's spike wording became `Offline stub`: the first line of `crates/skate2-net/src/lib.rs`, the `description`
  in its `Cargo.toml`, and the two runtime strings in `lib.rs` that said "spike" (`"offline stub: networking stubbed
  out"` and the crypto `Build::id`, `"offline stub"`).
- Line endings: every text file here is LF. The clone on this PC was checked out with CRLF (git's `core.autocrlf=true`),
  so the copies were converted from CRLF to LF. That is the only difference in the copied files.

To check the copies against a clone of the commit above (a clone made on this PC is CRLF, hence `--strip-trailing-cr`):

```
diff -r --strip-trailing-cr vendor/skate2-rust-port/crates/skate2-core    <clone>/crates/skate2-core
diff -r --strip-trailing-cr vendor/skate2-rust-port/crates/skate2-formats <clone>/crates/skate2-formats
cmp vendor/skate2-rust-port/crates/skate2-net/src/port/X.rs <(tr -d '\r' < <clone>/crates/skate2-net/src/X.rs)   # X: codec frames protocol range
```

Until the first hook lands both `diff -r` commands print nothing. After it, they print only the hooked files.

The workspace's release profile has `debug = 0` and `incremental = false`. The port's own has `debug = "line-tables-only"`
and `incremental = true`. The vendored workspace differs because disk and memory are short on this PC; it changes no
behaviour.

The port reads 203 variables starting with `SKATE2_` with plain `std::env::var`, and several change game state. Inside
the game it reads the game's own environment, so the game refuses the switch while any such variable is set, in any
case, and names it in a notice (`unclean_environment`; Task 14). The binary refuses to start: it answers
`Failed("environment: <name>")` and exits with code 2.

The port's own files (`saves\SKATER.P`, `online-config.json`, `config.json`) are never in the working folder, which
inside the game is the game's: `Host::boot` makes a fresh, empty folder in the system's temporary folder for each boot
and points the port there (the hook in the next table, `set_host_save_root`). See "Task 14: the working folder" below.

## Hooks

One row for each change to a vendored file, added by the task that makes it. Line numbers are those of the file after
the change.

| File:line | What | Why | Task |
|---|---|---|---|
| `crates/skate2-core/src/session/selected_session_career.rs:3545-3577` | `diagnostic_place_local_player(position, yaw) -> Option<u32>`: `diagnostic_travel`'s yaw matrix sent through the private `teleport_local_player` flow (spawn-position adjustment, stance, teleport), answering the teleport generation + 1. `diagnostic_set_board_velocity(velocity) -> bool`: the AI's `Actor::SetVelocityUnsafe` -> `Skateboard::SetVelocity` path (`actor_ai_control.rs`) on the local board. | The engine places the skater with a heading and a speed (design-inputs 5, items 1 and 3). `kbEnableAdjustSpawnPosition` stays native. Additive: only the host calls them. | 4 |
| `crates/skate2-core/src/session/selected_session_public_facade.rs:3722-3749` | `diagnostic_teleport_settled(expected) -> Option<bool>`: map travel's completion test (facade 3415-3424). `diagnostic_respawn_breadcrumbs() -> bool`: map travel's breadcrumb respawn at the published reset (3427-3437). | The host knows when a placement has settled, and a later bail does not go back to the trail from before the placement (design-inputs 5, items 2 and 4). Additive: only the host calls them. | 4 |
| `crates/skate2-core/src/session/selected_session_public_facade.rs:3812-3837` | `diagnostic_filtered_state() -> Option<FilteredPhysicsState>`: PhysOut's `Filtered.filtered_physics_state` after a completed frame. `diagnostic_deck_nose() -> Option<[f32; 3]>`: PhysOut's `SkateboardReckoning.deck_nose_dir`, beside the deck position `diagnostic_board_kinematics` reads. | `Frame.category` (the host's `frame.rs` gives the byte encoding), and the deck heading the placement test checks (`Host::deck_nose`); no public accessor read either. Additive, read only: only the host calls them. | 4 |
| `crates/skate2-core/src/session/selected_session_career.rs:3579-3598` | `diagnostic_release_spawn_pin() -> bool`: sets the Ground state's `player_provided_controls`, the flag a live input sets, so `ConsiderPinning` does not hold the board. **Not native Skate 2 (user 10-20)**: native Skate 2 never places a moving skater. | After a teleport, `ConsiderPinning` (Ground, `kMaxGroundPinFrames` 90, `kMaxGroundPinStartFrames` 30) gives a human's board the velocity back to the spot where its wheels first land, every tick, until the player touches the controls, which stops a placement's velocity on its touchdown (S1 finding for row 3). The host calls it only for a placement with a velocity, right after giving the board that velocity; a placement at rest keeps the native pin. Additive: only the host calls it. | 4 |
| `crates/skate2-core/src/session/selected_session_public_facade.rs:3750-3762` | `diagnostic_breadcrumbs() -> Option<(Vec<[f32; 3]>, [f32; 3])>`: the trail's crumb positions newest first (`try_service_breadcrumbs`, `SerializeBreadCrumbs`' order) and the wipeout spot (`last_known_actor_position`, the hips). | `Host::breadcrumbs`, which `a_placement_starts_the_respawn_trail` reads: the trail starts over at a placement. Additive, read only: only the host calls it. | 4 |
| `crates/skate2-core/src/rendering/skater/skater_presentation_selected_runtime.rs:169-174, 205, 258-260, 389-424, 614-619` | Two fields of `SelectedPresentationRuntime`: the presentation hierarchy's bone names (copied in `try_new`), and a copy of `local_to_world` (`root * hips`) with the first 32 of the final pose's matrices, `update_bone_transforms`' input (taken in `try_update` right after `local_to_world`). `host_pose() -> Option<([f32; 16], [[f32; 16]; 32])>`: `local_to_world` and presentation bones 0-31 in world space, where the palette draws them: each bone's skin matrix (global pose times inverse bind) then `local_to_world`, with its bind pose (the inverse of the palette's inverse bind) in front; bone 0, the trajectory, is the skater root (`GetSkaterRoot`), because the final pose's slot 0 holds the saved Hips transform; column 3 of every matrix set to 0, 0, 0, 1 (the upload reads columns 0-2 only). `None` until an update has published. `host_bone_names() -> Vec<&str>`. | The game draws its own skater from the port's bones (spike report section 5, plan call 8); the port exposes only per-mesh GPU skin rows. Additive: the copy is one 32-matrix array per update; only the host reads it. | 5 |
| `crates/skate2-core/src/session/selected_session_public_facade.rs:3839-3855` | `host_skater_pose() -> Option<([f32; 16], [[f32; 16]; 32])>` (`None` like `diagnostic_state_ids`) and `host_skater_bone_names() -> Vec<&str>`: the local skater's presentation runtime's `host_pose` and `host_bone_names`. | `Frame.root` and `Frame.bones` (the host's `frame.rs` gives the matrix layout) and `Host::bone_names`. Additive, read only: only the host calls them. | 5 |
| `crates/skate2-core/src/ui/trick_backend.rs:16-19, 63-78` | `TrickName.scorable_id: i32` (`TrickSettings::scorable_id`, -1 for an invalid trick; `index` is the Trick-vault key order, not the scorable). Six `PhysicsFrame` fields: `begun_all` and `converted_all` (every entry of Scoring2's `tricks_begun` and `tricks_converted_to`, where `begun` and `converted` keep the HUD's element [0]), `ended: Vec<(i32, u32)>` (`tricks_ended`: scorable id and `Scorable::score`), `active_ids` (`active_tricks`), `grinding` (`is_in_state_grinding`) and `completed_successfully` (`sequence_completed_successfully`). | `Frame.tricks` (world-objects 7): the engine scores by scorable id and needs every event of a tick, not the HUD's first. Additive: the HUD's own fields are unchanged; only the host reads the new ones. | 6 |
| `crates/skate2-core/src/physics/player/output/conditioning/scoring/hud_frame.rs:24-33, 35-36, 50-51, 162-172` | `scorable_id(settings)` (the id, -1 when not `is_valid`, as `try_mission_frame` reads an air's first trick) and `listed(list)` (a `Scorables` list's first `num_entries` elements). `name()` sets `scorable_id`: line 36 is the native invalid-trick line with `scorable_id: -1` added (the default 0 is a real id, `fspowerslide`). `try_scoring_hud_frame` fills the six new fields from the same Scoring2 output it already reads. | The same. Additive: no native value changes; the new lists use the same `name()` lookups as `begun` and `converted`. | 6 |
| `crates/skate2-core/src/session/selected_player_tick.rs:70-72, 371-372, 849-850` | `host_hud_frames: Vec<PhysicsFrame>` on `SelectedPlayerTick`, empty at creation, and a clone of each HUD frame pushed beside the native push to `scoring_hud_frames`. | The host takes the tick's frames without draining the native list, which a visual snapshot drains (Task 3's `Host::tick` takes one every tick). Gathered under the native condition (`scoring_hud_subscribed`, set by the first `try_visual_snapshot`). Additive: one `PhysicsFrame` clone per scoring update; the list grows until the host takes it, which `Host::tick` does every tick. | 6 |
| `crates/skate2-core/src/session/selected_session_public_facade.rs:3857-3895` | `take_scoring_hud_frames() -> Vec<PhysicsFrame>`: drains `host_hud_frames`. `host_end_line() -> bool`: sets the local player's scoring row's `reset_scoring` (`PlayersBackendSettings::try_set_scoring`), the flag `ChallengeManager.SetResetScoring(true, player)` (0x82882590) sets; the next scoring update pops it (`try_scoring_input`) and resets the module (the holder's line score without banking it, the sequence, the collectors, the line timer and the momentum meter); false without a local player. **Not native Skate 2 (user 10-20)**: native teleports keep the line. `host_right_patterns() -> Vec<(&str, Vec<[f32; 2]>)>`: every loaded right-stick pattern's name and points, in `controlled_right_pattern_points`' order, with no panic for a name. | `Frame.tricks` and `Host::place`'s `end_line` (spec 3.5: no score carries across an engine teleport; design-inputs 5). `host_right_patterns` is for `every_pattern_trick_reports_its_own_id`. Additive: the flag is the native one, set through the native settings row; only the host calls them. | 6 |
| `crates/skate2-core/src/math/native_rw_math_random.rs:75-101`, `crates/skate2-core/src/ai/crt_random.rs:15-24` | `global_next`'s generator hoisted to a module static `RANDOM` (the same `OnceLock<Mutex<RwMathRandom>>`, still clock-seeded unless asked), with `set_global_seed(seed)` (starts it over from `RwMathRandom::from_seed`) and `global_state()` (read only). `crt_random::set_seed(seed)` (`srand`: `holdrand` = seed) and `state()` (read only). | Repeatable scripted runs (world-objects 9). These two, with the shared random lock below, are all the process-wide random state: `RANDOM`, `HOLDRAND`, and `NativeRandomGate::production_shared`'s `locked` flag, which the facade's `set_rng_seed` also clears. In play nothing calls them, so play stays clock-seeded as native was. | 8 |
| `crates/skate2-core/src/session/living_world/selected_living_world_recipes.rs:172-177` | `set_exact_delivery(on)`: sets the `exact_delivery` field that `update_models` ORs with `traced_run()`. | Ambient loads are delivered at their due update instead of when a worker finishes, so a test run does not depend on decode speed (world-objects 9). Off in play. | 8 |
| `crates/skate2-core/src/session/players_backend_settings.rs:35-37, 77`, `players_backend_selection.rs:48-59`, `edit_skater_player_attributes.rs:51-60, 112-117` | `PlayersBackendSettings.host_gestures` (one `(player, [u32; 4])`, which `ResetPlayers` leaves alone) with `try_set_host_gestures` / `try_host_gestures`. `ActorAnimationConstruction::set_host_gestures` (writes `SkaterAnim::mGestures`) and `host_gestures` (read only). `apply_character_data` takes the host's slots after its own choice. **Not native Skate 2 (user 10-20)**: native forces every character but a created skater to `[0, 1, 2, 3]` at every `ReloadCharacterData`, which would undo the engine's slots; all 35 gestures are available (row 9 below). | The engine equips the local skater's four D-pad gestures (world-objects 8). The slots survive a character reload (`reload_character_data_for_spawn`, which `selected_session_freeskate.rs` 393 calls, goes through `apply_character_data`, so that file needs no edit) and a wipeout respawn. | 8 |
| `crates/skate2-core/src/animation/graphs/actor_graph_construction.rs:218-220, 254`, `animation/graphs/motion/generated_motion_character_gesture.rs:113-125, 271-272, 337-358, 372-380` | `ActorSkaterGraphState.host_gestures_started` (the last 64 `eAnimGestureNames` indexes whose gesture channel was created) and `ActorConstruction::host_take_started_gestures` (drains it). The callback keeps the slot's gesture index beside the cvar value the port reports in `state.gesture` and pushes it when `try_new_gesture_channel` answers `Created`. | The port's `state.gesture` is the debug cvar's value (34, 33, 32, 31 by default) for any slot, so it cannot say which gesture a D-pad press played; `each_slot_fires_its_gesture` reads this log instead. Read only: the port's own state is unchanged. | 8 |
| `crates/skate2-core/src/camera/frame/camera_selected_runtime.rs:129-140, 146-164`, `actors/skater/skater_manager_observers.rs:181-189`, `lib.rs:656-661` | `HostFreeLook` (whether the current camera shot takes the free stick, the free camera's heading and elevation offsets, the subject's look stick after the inverts, and the global condition's inverts) and `host_free_look` / `try_camera_free_look`, which read it. `lib.rs` exports `ProfileSettingsInput` and `HostFreeLook`. | `the_profile_reaches_the_camera` (Amendment D) checks the profile where the camera reads it. Read only. | 8 |
| `crates/skate2-core/src/session/selected_session_public_facade.rs:486-500, 3911-4019` | `ProfileSettingsInput` and `GESTURE_COUNT`. `set_local_gestures(slots) -> bool` (each under 35; settings row, host slot and animation), `local_gestures()`, `take_started_gestures()`, `diagnostic_reload_character_data()`, `set_rng_seed(seed: u64)` (folds the seed to 32 bits, seeds `RANDOM` and `HOLDRAND`, clears the shared random lock), `diagnostic_rng_state()`, `set_exact_delivery(on)`, `set_host_game_mode(mode) -> bool` (1, 3 or 4: sets the backend's game mode alone, **Not native Skate 2 (user 10-20)**: native `set_game_mode` is the game load and also recreates the tracker Prop and resets the session marker, which a game already running must not repeat; the Hall of Meat gate and the chase checks read the mode), `diagnostic_camera_free_look()`, `set_profile_settings(ProfileSettingsInput)` (writes the four camera fields of `audio.settings`, which `try_tick_once` pushes to the camera every tick, so they apply on the next tick and no reload touches them). | Task 8: gesture slots, seed, exact delivery, game mode and camera profile (world-objects 8 and 9, Amendment D). Only the engine's host calls them. | 8 |
| `crates/skate2-core/src/living_world/population/living_world_census.rs:38-43, 61`, `crates/skate2-core/src/session/living_world/selected_living_world_dmo.rs:785-789`, `crates/skate2-core/src/session/selected_session_public_facade.rs:3989-4014` | `CensusState.host_ambient_off` (false at creation). `CensusMan::Update`'s `banned`: the port's mode test `matches!(mode, 2 | 4 | 5)` becomes `living.census.host_ambient_off`, and `update_settings` copies it into `pedestrians_banned` and `vehicles_banned`. `set_ambient_census(enabled) -> bool` (sets the field and both flags at once; false before the world exists or on a poisoned lock) and `diagnostic_census_banned() -> Option<(bool, bool)>` (read only). **Not native Skate 2 (user 10-20)**: the host closes the gate at boot. | The ambient gate (Amendment C): no Skate 2 pedestrian or car in the sim. Banned, the census spawns none and culls every one it has (cull radius 0, `living_world_census_pedestrian.rs` 368-375, `living_world_census_vehicle.rs` 110-137), so the gate reuses the port's own ban and adds no new spawn path. `Host::boot` closes it before the boot's initial census (the world exists by then: `Host::gated_before_census`) and again after the boot; `SetAmbient` calls it. The gate reaches the props too: the next row. Additive apart from the mode test: only the host calls the two functions. | 22 |
| `crates/skate2-core/src/living_world/dmos/living_world_dmo_objects.rs:1490-1498, 1813-1817, 1974-1978` | The ambient gate reads `census.host_ambient_off` in three places. `try_evaluate_dynamic_object_census`: the cull radius is 0 when closed (so every object with `allow_cull` is culled, as the pedestrian and vehicle census do under their bans), else `evaluated.cull` as before. `try_spawn_dynamic_object_id` and `try_spawn_static`: return `Ok(None)` when closed, before anything is allocated. **Not native Skate 2 (user 10-20)**: Amendment C. | Fix round 1: the port's dynamic objects (DMOs: its props, 2 growing to 9-10 on NSV) ignore the bans, so with only the census gate they would stand in the sim as props the user never sees, in the game's own map. Every spawn of a dynamic object goes through those two functions (the census, a pedestrian's hand prop, the online DMOs), so closing both is the whole gate. Nothing is kept as an exception: the skater, its board and its bodies are not dynamic objects (`LWDynamicObjectMan`) and the skater sim reads none of them; the 1800-tick skating run and the 16 headless tests are unchanged with the props gone. Open, the guards are not taken and the radius is `evaluated.cull`: the open run's counts are identical tick for tick to before the hook. | 22 |
| `crates/skate2-formats/src/mount.rs:35-50, 229, 269, 340, 363` | `observe_misses(observer: fn(&str))` beside `observe_reads`, and `not_found(path)`, which runs the observer and answers `MountError::EntryNotFound`; the four places that answered it directly (`MountedBig::read_path`, `read_path_range`, `MountSet::read_path`, `read_path_range`) call it. | `the_stage_output_runs_the_disc_tests` counts the path reads that find nothing in the `skate2-physics` stage's copy of the disc (the support's `record_reads` logs them as `!notfound` lines); the port's own optional lookups swallow `EntryNotFound`, so a lost entry would otherwise go unseen. Additive, read only: the observer is unset in play. | 12 |
| `crates/skate2-core/src/session/edit_skater_profile.rs:20-36, 160-163`, `crates/skate2-core/src/ui/online_config.rs:52-55`, `crates/skate2-core/src/ui/game_config.rs:17-20` | `HOST_SAVE_ROOT` (a process-wide `RwLock<Option<PathBuf>>`, `None` at start) with `set_host_save_root` and `host_save_root`. `EditSkaterProfileStore::default_path`, `OnlineConfig::path` and `GameConfig::path` answer `<root>/SKATER.P`, `<root>/online-config.json` and `<root>/config.json` when it is set, before `SKATE2_SAVE_ROOT` and the working folder. | Amendment A: in one program the port's working folder is the game's. A stray `saves\SKATER.P` there failed the port's boot (`SKATER.P must contain 0x400004 bytes`, measured with the Task 12 host on the stage output), and a valid one would give the sim its saved skater and settings; a career autosave would write there. Every read and write of these files goes through the three functions (`default_path` has 27 callers), so the hook is the one choke point. `Host::boot` sets it to a fresh, empty folder per boot (`SaveFolder`, removed when the host ends) and never unsets it. Additive: unset, the port's rule is unchanged; only the host sets it. | 14 |

## Port deviations

Every row of design-inputs section 1 (`docs/superpowers/specs/skate2-physics-reference/design-inputs.md`): the port's
own deviations from native Skate 2. The mark on each row is one of "reverted (Task N)", "kept, `Not native Skate 2`"
or "not run by the host". A "reverted" row is final when its task has landed.

**A. Gameplay or sim**

| # | Deviation | Mark | Note |
|---|---|---|---|
| 1 | Hall of Meat bail slow motion removed (native `UpdateHoMSlowMotion` posts a one-tick `SetSimRate`) | removed; Task 7 deferred (it stays removed in test build 1) | The producer is rewritten from the port's own evidence and called before `record_replay_frame` (facade 3390) in Task 7R. Until then the host reports `TICK_SECONDS`. Game mode 3 turns the effect off, as native (Task 8: `mode_3_turns_the_hom_effect_off`; `Frame.hom` is now published from the tick's HUD frames). |
| 2 | HoM replay forced to full speed; cinematic freeze commands 1-3 skipped | not run by the host (replay presentation) | The engine shows its own replays; Skate 2's replay editor is a non-goal. Task 7 leaves the replay-reel changes as they are. |
| 3 | Party Play skaters pass through each other | not run by the host (Party Play) | The branch is game mode 2. The host takes modes 1, 3 and 4. |
| 4 | No pedestrians or traffic in game modes 2, 4, 5 | reverted (Task 22) | The `banned` mode test is now the settable ambient gate (`CensusState.host_ambient_off`, Hooks table), so the engine decides in every mode. The game always closes it (Amendment C: the user wants Skate 2's skating and tricks, not its city), so the census never runs, online included (spec section 7). **Not native Skate 2 (user 10-20)**: native Skate 2 has no such gate; with it open the census runs in every mode, as the original's did. |
| 5 | Online netcode replaced: replay frames, not lockstep; `isNetworkGame` kept clear | not run by the host (online) | The engine has its own online. `crates/skate2-net` here is the offline stub. |
| 6 | Online games skip replays and highlight reels | not run by the host (online presentation) | |
| 7 | Online sync points sent at once, with no NIS preload or streamer wait | not run by the host (online flow) | |
| 8 | Online, a pinned skater still gestures (retail behaviour, not the debug exe's) | not run by the host (online) | The host is offline. This is retail behaviour, so there is nothing to revert. |
| 9 | All 35 off-board gestures unlocked (skips the category-14 `IsUnlocked` check) | kept, `Not native Skate 2` | Native Skate 2 gates the gestures by its own career, which this engine does not run; a completed Skate 2 save has them all. The picker belongs to the engine (spec 3.1). Task 8 equips them through `set_local_gestures` (the Hooks table). |

**B. Front end, UI, rendering, save, online shell**

| # | Deviation | Mark | Note |
|---|---|---|---|
| 10 | Film Pack always unlocked | not run by the host (front end) | |
| 11 | Party Play lists the created skater | not run by the host (Party Play) | |
| 12 | Party Play players may share one skater | not run by the host (Party Play) | |
| 13 | No tips screen when a Party Play challenge switches | not run by the host (Party Play) | |
| 14 | Cutscene bloom transition 0.2 s instead of 1.0 s | not run by the host (rendering) | |
| 15 | Shorter intro (skips BootToPressStart) | not run by the host (front end) | |
| 16 | Main menu "Exit to Desktop" row and DLC notice | not run by the host (front end) | |
| 17 | "Xbox LIVE" renamed "Online" | not run by the host (front end) | In the port's `apps/`, which is not copied. |
| 18 | Unknown cheat codes do nothing | not run by the host (front end) | |
| 19 | Pause hub has no Debug row | not run by the host (front end) | The port's docs disagree about retail. |
| 20 | New Display and Online settings pages; `SK82_PC::DisplaySettings` save chunk | not run by the host (front end, save) | |
| 21 | Frame-rate cap replaces VSync; no 30 Hz front-end or NIS present mode | not run by the host (rendering) | |
| 22 | PostFX-off brightness boost, fisheye switch | not run by the host (rendering) | In the port's `apps/`, which is not copied. |
| 23 | PostFX forced on in cutscenes | not run by the host (rendering) | In the port's `apps/`, which is not copied. |
| 24 | Lens-reflection bloom ghosts removed | not run by the host (rendering) | In the port's `apps/`, which is not copied. |
| 25 | Non-16:9 windows get a wider field of view; UI art stretched | not run by the host (rendering) | In the port's `apps/`, which is not copied. |
| 26 | Rendering list "Deliberate deviations" (CSM formats, MSAA, anisotropy, ...) | not run by the host (rendering) | |
| 27 | PC presentation clock replaces `RealTimeController` | not run by the host (presentation only) | The host steps fixed 1/60 s ticks. The variable that restores the native clock, `SKATE2_NATIVE_PRESENT_CLOCK`, is refused by the clean-environment check. |
| 28 | MP4 and JPEG media export | not run by the host (app) | |
| 29 | Profile-name prompt, game-folder prompt, config files, updater | not run by the host (front end, app) | |
| 30 | Online additions: lobby browser, chat, nickname, word filter, build secret | not run by the host (online shell) | |
| 31 | New-profile flag cleared at career autosave, not at `FrontEndState_CaC::Shutdown` | not run by the host (career, save) | |

Ruled out as deviations in design-inputs 1: test-only unlocks, `SelectorGrindPolicy::ParkedForFirstSkatingTest`,
FreeSkate unlock gating, the Big Black bounce, big-air camera slow motion, controller deadzones, and the "PC policy"
physics comments. The last are number-handling policies: serial island order and no Xenon float or FPSCR parity, so
bit-level results can differ from a real Xbox (spec section 6).

## S1 findings

One heading for each row of spec section 12 that stage S1 checks. The task named under a heading writes its result
there. Until then the heading is open.

### Row 2: the ambient gate and the boot census

Answered by Task 22 (`tests/city.rs`: `the_ambient_gate_empties_the_city_on_the_first_tick`,
`the_ambient_gate_holds_on_nsv`, `the_gate_reaches_the_census`, and the control `the_open_gate_is_measured`) on the
disc, seed 0x5EED, exact delivery. Not tested in play. Amendment C replaced the question about other maps, online and
host-spawned cars and props (those are dropped); what remains is what the host's boot census does and whether the gate
holds.

- What appears before the gate: nothing that is a pedestrian or a vehicle. With the gate open (`Boot::ambient` true), the
  snapshot holds 0 pedestrians and 0 vehicles on the first tick and on every 60th tick of 1800 (the spike's 900-tick
  run, then 900 with the pad left alone). The host's boot has no NSV terrain (`session::boot` gives it an empty
  `WorldStreamCatalog` and the Sim collection only) and the census draws its targets from the terrain's layers
  (`census.target(..., painter, ...)`, `living_world_census_pedestrian.rs` 264-278): that is the likely reason, read
  from the code and not proven. So the two brief tests passed before the gate existed, and by themselves they do not
  show that the gate works; they stay as the guard for the day the terrain comes back (Task 9).
- What the gate does, shown where the census reads it: `the_gate_reaches_the_census` reads the census's `banned` flags
  (`Host::census_banned`). Before the gate they were `(false, false)` with `Boot::ambient` false (that test failed).
  With the gate, `Boot::ambient` false gives `(true, true)` after the boot and after a tick, `true` gives
  `(false, false)`, and `Host::set_ambient(false)` and `(true)` flip them at once and the next 8 census ticks keep them.
- The boot census: the gate is closed before it. `Host::boot` calls `set_ambient_census(false)` right after
  `try_begin_controlled`, when the world already exists (`Host::gated_before_census` is `true`), and the initial census
  runs only at the end of `try_poll_controlled_boot` (facade 1900). So there is no boot census to cull; had the gate
  come after the boot, the first ticks would have culled it (radius 0).
- Dynamic objects (the port's props, fix round 1), measured by `the_ambient_gate_holds_the_props_on_nsv`: with the census gate alone they were NOT absent while closed: the closed run had the same 2 props from the first second, 4 at tick 359, 9 from tick 479 on and 10 at tick 599 (that test failed). Their spawn checks `census.enable & 0x04` and not the bans, so the census gate did not stop them, and they would be invisible obstacles in the game's map. So the gate now closes them too (Hooks, `living_world_dmo_objects.rs`): closed, 0 on every one of 1800 ticks; open, the same counts as before (2 at 59, 4 at 359, 9 at 479, 10 at 599, 9 to the end). Closing an open gate on a running city culls them: 9 props at tick 700, none 2 ticks after `set_ambient(false)` and none after that (`closing_the_gate_culls_the_props`). Not kept as an exception: the skater, its board and their bodies are not dynamic objects.

### Row 3: a large board velocity right after placement

Answered by Task 4 (`a_placement_carries_its_speed` and probes): the skater does not bail, and with the host's ground
pin release (Hooks, `diagnostic_release_spawn_pin`, not native Skate 2) the speed carries past the touchdown. Not
tested in play.

- The placement's timeline: on the tick after `Place` the flow sends the teleport; on the next the physics state is
  TELEPORTING and the port's completion test passes; on the third (the arrival) the bodies are at the target, the
  board 0.16 m up with no wheel down (the spawn adjustment's 0.15 m above the ground plus the teleport's 0.1 m). The
  host gives the board its velocity after the arrival, so the fourth frame is the first that says settled.
- The heading: on the arrival the deck's nose is 0.011 rad from the placement's yaw; on the settling tick 0.027 rad,
  and about 0.029 at rest. From the arrival on, the idle stance turns the deck, natively: the boot spawn
  `F_WF_Waterfront` (yaw 0.3545) turns the same way, to 0.3619 on tick 0 and 0.3867 on tick 25.
  `a_placement_settles_at_its_target` checks the arrival against 0.02 rad and prints the settling tick's drift.
- What stopped the board: after a teleport, `ConsiderPinning` (Ground; `kMaxGroundPinFrames` 90,
  `kMaxGroundPinStartFrames` 30) takes the spot where the wheels first land (`valid_spawn_x/z`) and, on every tick
  until the player touches the controls (`player_provided_controls`), sets the board's velocity to the step back to
  that spot. Native Skate 2 does this so a respawned skater does not roll away; it never places a moving skater.
- Without the release (the hook as Task 4 first had it): 5 m/s along yaw 1.0 at the spike's wall holds 5.0-5.2 m/s
  while the board drops, then the tick its wheels land it falls from 5.11 to 0.15 m/s and the board stays put for
  the next 90 ticks. A probe at 10.14 m/s on the spike's approach did the same (9.9 to 0.08), and giving the velocity
  again on the ground, once or on every tick, did nothing, as the pin overwrites it.
- With the release: the same run, 4.997 m/s on the settling tick, 5.11 at the touchdown (7 ticks after settling),
  4.61 on the next tick (the wheels' landing), 4.68 thirty ticks after the touchdown and 4.67 sixty ticks after,
  filtered category ground throughout, no wipeout. `a_placement_carries_its_speed` asks for at least 4.0 m/s thirty
  ticks after the touchdown. About 80 ticks after settling, near the wall, the board slows (3.5, then 2.9 m/s, with two
  wheels up for a moment); not looked into.
- Also probed and not needed: the speed model's target speed taken again from the moving board
  (`ResetSpeedModel`, which `PhysicsGround.Enter` runs on the stopped bodies). Alone it changed nothing; with the
  release, every printed value was the same as with the release alone. Not kept.
- A placement at rest (zero velocity) keeps the native pin. A placed board that leaves the ground and lands again
  within 30 ticks of the teleport goes through `PhysicsGround.Enter`, which clears the release, so the pin could
  hold it there (read from the code; not seen on the flat run).

### Row 4: the camera without its renderer

Answered by Task 3 (`the_camera_runs_headless`): yes. The host has no renderer (its `HeadlessEffects` does nothing),
and `try_visual_snapshot` still gives the port's game camera after every completed tick. Over the spike's 900-tick run
on the disc, the camera moved on 898 of the 899 ticks after the first and stayed at most 5.26 m from the deck, the
farthest at spike tick 856, the respawn after the wall crash. The test asks for at least 800 ticks and 10 m. The frame
carries its position, its forward, right and up axes and `provisional_clip_from_world`. Not tested in play.

### Row 8: the nav arenas and the people

Open. Task 10. To measure: the arenas' size and load cost, how many people and guards then spawn, and whether a
guard's chase needs the skater on the nav mesh.

### Row 9: LivingWorld on a host boot with no NSV terrain

Open. Tasks 10 and 11. The question: do the LivingWorld services come up on a host boot with no NSV terrain, and does
`try_spawn` work without a registered locator? Expected handling: if not, the hooks bring them up.

### Row 10: road-state reads for a car that is not on the road network

Open. Task 11. To audit: which vehicle code reads road state for a host-driven car. Every such read is guarded.

### Row 11: the pedestrian skeleton's bone count

Open. Task 10. To measure: the bone count, which sets the message size. Expected handling: nearby people only,
compressed.

### Task 6: trick events (not a row of spec section 12)

Found by Task 6 (`every_pattern_trick_reports_its_own_id`, `the_basic_moves_report_their_ids`) on the disc. Not
tested in play.

- Pattern tricks: the port loads 269 right-stick patterns under 99 names (the matcher loads several sets, so names
  repeat). A pattern is named after its scorable, ignoring ASCII case: `Kickflip` is the scorable `kickflip`. 30
  scorables have one and were tested: ids 85-92, 95, 96, 99, 106-113, 116, 117, 120-128. Each was played as its first
  pattern's points, one per tick, at the Task 4 spot rolling at 4 m/s. Each began as its own id and no other, 15-20
  ticks after its first point, then went to air and landed. None failed.
- 239 ids have no pattern of their name: 0-84, 93-94, 97-98, 100-105, 114-115, 118-119, 129-268. They are the
  powerslides, manuals, reverts, grinds, slides, grabs, the flip variants (`kickflip2`, `latekickflip`, ...), the air
  statistics (129-133), the handplants and the rest, which other inputs or other scoring start. 191 patterns are named
  after no scorable: 78 `90_*`, 78 `N90_*`, 21 `L_*`, 10 `Fingerflip`, 4 `FS_Varial` and `BS_Varial`.
- The landing bump after a placement: the settling frame still has no wheel down (S1 finding for row 3). On the next
  tick the board lands, and the Motion graph rides the landing out in `OnBoard/OnGround/RidingIdle/Bumped` for 35
  ticks (`play` waited 36 neutral ticks every time). There a shallow antic does not anticipate: the first points of
  the kickflip and the heelflip (0.69 of full stick) go to `Trick/TrickFromSquare` and no trick begins, while the
  ollie's (0.97) anticipates. Played right after settling, `kickflip`, `heelflip`, `n_kickflip` and `n_heelflip`
  began nothing; after the bump all four began. The engine's mid-run switch-on (Task 19) places the skater the same
  way, so a flick in the first 36 ticks after settling can be lost, as natively after any landing bump.
- `interrupted` (`wipeout_interrupted_trick`) is native: it is set only when the wipeout cuts off a sequence-worthy
  activity (`module_dispatch.rs` `manage_collector`, `ground.rs` `activity_sequence_worthy`). The spike's own wall bail
  comes while coasting (wipeout at spike tick 595) and interrupts nothing. With its powerslide held into the wall
  (from spike tick 572), the bail ends the slide `(0, 12)` on tick 596 and sets `interrupted` on its wipeout tick, 597.
- The line: a trick line starts only when the momentum multiplier is above 1.01 after a sequence. The spike's ollie
  (10 points) starts none. Its kickflip (100 points, landed on tick 523) starts one: `line_score` 100, multiplier 1.5.
  A manual alone starts none. The wall crash ends the line natively (`line_score` 0 from spike tick 592).
- `host_end_line` before tick 575, in a nose manual after the kickflip: on tick 574 `line_score` 100 with the manual
  active; from tick 575 on, `line_score` 0 and nothing active (the reset also ends the active manual's scorable).
  Without the call, the line keeps its 100 until the manual is released at tick 590.
- The powerslide's scorable is active (`active [0]`, `sequence_begun`) from tick 289, the tick after its stick is
  first held, but begins only on tick 313; it ends `(0, 40)` on tick 361. The nose manual begins on tick 301, 31 ticks
  into the held stick, and ends `(2, 40)` on tick 340, 22 ticks after the release.

### Task 8: seed, gestures, game mode, profile (not a row of spec section 12)

Found by Task 8 (`the_same_seed_gives_the_same_run`, `each_slot_fires_its_gesture`, `mode_3_turns_the_hom_effect_off`,
`the_profile_reaches_the_camera`) on the disc. Not tested in play.

- The seed: `Boot::seed` is applied before the boot (`session::boot`), so the state right after it is the seed's, whatever
  ran before in the process. The port's city is off, and the skater's decks are bit for bit equal across seeded runs
  (and unseeded ones). Past the seed the draw count on `global_next` depends on timing, seed or not: about 4000 to
  8500 draws during the boot, then a burst of about 4000 more at an unrepeatable tick of the run (an ambient load
  finishing). The generators' later state is therefore not compared.
- The slots are Up, Down, Left and Right, in that order: with `[4, 5, 6, 34]`, D-pad up plays pumphi, down fonz, left
  bruce and right axe. No motion or action state names a gesture (the paths file has none), and the port's
  `state.gesture` is the debug cvar's number, so the test reads the host's gesture log.
- The camera: the angle reaches the port's camera (High about 1.3 m higher than Low behind the skater). Off the board,
  the right stick feeds the free camera through the inverts (`look_stick`): invert_x flips its x and invert_y its y. In
  Skate 2's camera data the free camera's heading offset is 0, so x changes nothing of the camera (the stick also turns
  the skater, which the camera follows, with or without the invert); y raises or lowers the camera, and invert_y
  reverses that. Port behaviour, not a hook.
- Game mode: 1 sets the Hall of Meat effect on, 3 and 4 off (`Physics::IsHallOfMeatEffectEnabled`).

### Task 12: the data set and the stage (not a row of spec section 12)

`tools/pipeline/skate2port/old_physics.py` (the stage `skate2-physics`) copies the entries listed in
`old_physics_entries.tsv` out of the user's Skate 2 disc into `assets/private/skate2-physics/` with a `manifest.json`;
`data::check` (called by `Host::boot` when the manifest is there) refuses a copy that lacks one, by name. Not tested in
play.

- The entry list (Step 0, `<S>\physics\merge_reads.py`): the read logs of the 22 disc tests of Tasks 3-8 and 22
  (`headless.rs` 16, `city.rs` 5; the pipe test reads in its child process and logs nothing, the in-process tests cover
  the same boot) hold 288 distinct `(archive, key)` reads in nine archives. The merge resolves the 6 hashed archives'
  hashes by djb2 against the disc's named tables and adds all 284 `cSim_*_high.xsf` cells of `worldbam.big` (6 of them
  were already read): 566 entries, 284 `cSim` lines, no duplicate `(archive, key)`, 132,067,710 stored bytes (132.1 MB).
  Fix round 1 (coverage): `livingworld.big`, `miscload.big`, `ropa2.big` and `db.big` are copied whole (every entry of
  their tables is listed, generated by `old_physics.whole_entries`), because the tests do not show which city recipes and
  models a long clock-seeded session loads: 1,996 entries, 284 `cSim` lines, 158,541,727 stored bytes (158.5 MB).
  Three older logs in the reads folder (`a_wipeout_respawns_at_the_last_placement`, `t6x_explore`,
  `the_open_gate_is_measured`) belong to tests that no longer exist; the merge skips them (they add no entry).
  Task 9 merges again with its own reads and re-runs the stage and `the_stage_output_runs_the_disc_tests`.
- The stage: reads the disc one range at a time (`IsoDisc` through `xdvdfs`, or an extracted disc folder, which is what
  the runner hands it), 1,996 entries and 158,541,727 bytes in 0.5 s, peak 120 MiB; the output is 159.0 MB on disk. Every
  entry is byte for byte the disc's (RefPack stays packed; checked against the spike's extracted set), each archive keeps
  its disc path and table kind, and its `.bh` when the disc has one (the entry's offsets are new, its stored and unpacked
  sizes are the disc's). The folder `draw/` is made empty for S3.
- The check: `Host::boot` on the stage output reads the manifest, opens each archive as the port does and finds every
  listed key with its listed size; the first one that fails is `DataError::Missing("<archive> <key>")`. The game gets
  `HostMessage::Missing` for it (the host binary sends it; the sim thread of Task 14 will). A bad manifest is
  `DataError::Manifest`, not a panic.
- `the_stage_output_runs_the_disc_tests`: the 16 headless, 5 city and 1 pipe tests, run as child processes with
  `S2HOST_TEST_DISC` set to the stage output, all pass (130 s), including `the_minimal_set_runs_the_same`, which
  compares the copy with the spike's minimal set; and no path read found nothing in the copy (0 `!notfound` lines over
  126,233 logged lines).
- Not checked: the `.bh` of an archive the port reads by name (all but anim, livingworld and ropa2) is written but the
  port never opens it.

### Task 14: the working folder (not a row of spec section 12)

Found by Task 14. Not tested in play. Amendment A puts the port in the game's process, so its working folder is the
game's.

- What the port reads and writes relative to the working folder (a grep of `crates/` for file system calls and
  `current_dir`, outside tests): `saves\SKATER.P` (`EditSkaterProfileStore::default_path`, 27 callers: read at the
  boot's prefix for the display, audio and new-profile settings, and at the player's mounted startup for the saved
  name and the created skater's recipe; written by the career, editor, extras, music player and settings saves, through
  a `.tmp` and a `.bak` beside it), `online-config.json` (`OnlineConfig::path`: read at the boot's prefix for Show Lobby
  Chat; written by the nickname prompt and the music player's options) and `config.json` (`GameConfig::path`: the
  launcher's game folder; nothing the host runs reads it). The rest of the file system calls are under the data folder
  (`data::mount`, and `data\startup.ini` under the session's data root) or behind `SKATE2_*` variables (the lean
  trace, the online frame dump), which the game refuses.
- Measured with the Task 12 host test binary (the port without the hook) on the stage output, from a fresh empty
  working folder: `boots_within_10_seconds` (2.5 s) and `the_spike_run_is_reproduced` (900+ ticks with a crash and a
  respawn) passed and left the folder empty, so a boot and those ticks write nothing. With a stray `saves\SKATER.P` (5
  bytes), `online-config.json` and `config.json` there, both failed at the boot: `Failed { stage: BootPrefix, detail:
  "Format(ProfileError(\"SKATER.P must contain 0x400004 bytes\"))" }`.
- The fix: the hook above. `Host::boot` points it at a fresh, empty folder of its own for each boot, in the system's
  temporary folder (`skate2-physics-saves-<pid>-<n>`), before anything else, and gives the session the same folder as
  its data root, so a stray `data\startup.ini` in the data folder is not read either (retail Skate 2, the disc and the
  stage's copy have none). The folder is removed when the host ends; the save root stays pointed at it, so a session
  the game abandoned never falls back to the working folder. Every boot starts from the port's first-use profile, as it
  did before from an empty working folder.
- Still to check with the hook (open: Task 14's build was stopped for low memory): the game's disc test
  `the_sim_runs_in_the_game` from a fresh working folder holding the stray files (scratchpad `t14/wf_engine.py`) must
  boot and tick and leave the folder unchanged; the test itself asserts the port's three files are outside the working
  folder.
- The host's own side of one sim at a time (Task 14 fix round 1; `host/`, ours, not a hook): `Host::boot_with` takes a
  cancel flag, checked before the data is opened, before the session starts and on every 5 ms poll of the port's boot
  (`session::boot`), and gives `HostError::Cancelled`; `Host::boot` is `boot_with` with a flag never set, so the binary
  and its disc tests are unchanged. `Host::warm_up` runs one unpublished tick (the next frame is tick 1 again); only the
  game's sim thread calls it, inside its boot, so the runs the host's disc tests pin (the spike run's anchors, the tick
  numbers) are unchanged.
