//! Manual session markers. Automatic bail recovery owns a different checkpoint.
mod effect;
mod hud;
mod noise;
mod state;
mod validation;
use crate::{
    app::SimulationSet,
    input::ControllerInput,
    map_transition::CurrentMap,
    physics::{GamePhysics, SkaterRuntime},
};
use bevy::prelude::*;
use skate_core::physics::{board::BodyId, skeleton_animation_record::AnimationPartTransform};

/// Skate 3's phonelist (the panel, 826682B0) sounds: `cellphone_activate` as LB opens it, and its hidden song
/// shortcuts' `next_song` (sk8_menu 0xCE, Volume 0.75) and `restart_song` (0xCE, Volume 1.0).
const CELLPHONE_ACTIVATE: u64 = 0x47FE_75BF_61F1_9941;
const NEXT_SONG: u64 = 0xDCBE_D0AD_AEC7_1C38;
const RESTART_SONG: u64 = 0x9BBF_7BCB_DB63_49B1;

#[derive(Clone, Copy, Debug)]
struct Marker {
    transform: AnimationPartTransform,
    on_board: bool,
    foot_forward: bool,
    generation: u64,
}

#[derive(Resource, Default)]
pub(crate) struct SessionMarker {
    marker: Option<Marker>,
    hold: state::Hold,
    generation: u64,
    pub visible: bool,
    /// The panel opened on this LB press (826682B0 mode 3), so its song shortcuts work.
    phone_open: bool,
    pub can_place: bool,
    pub can_return: bool,
    /// Not retail (user 10-25): the B row reads Player List where retail's reads Swap Camera ([`swap_camera_available`]).
    pub player_list: bool,
    /// LB+B asked for the Player List; core_menu opens the Start menu's Spectate list on it and clears this.
    pub open_player_list: bool,
    /// The skater's state lets the marker be set (grounded on the board, or standing).
    pub allowed: bool,
    pub progress: f32,
    /// The online out-of-bounds static (Effects.StartEffect(eChallengeEffect.OOB)), set by online_boundary every
    /// gameplay frame; the noise pass shows `progress + oob`, as retail sums its sources.
    pub oob: f32,
    /// Markers placed / returned to since startup; the Skate 2 tutorial watches these change.
    pub sets: u64,
    pub uses: u64,
    /// Online activities' SessionMarker.SetSessionMarker (0x82d4e3d8 -> 0x82d55ae8): place the marker where
    /// the skater is on the next update, without the placement-validity check (online_boundary).
    pub request_set: bool,
    /// The skater spawned (a map loaded) or travelled (`SkaterRuntime::travel_and_mark`): place the marker where it
    /// lands, forced and not counted in `sets` (user 10-11: "When you spawn in your marker should be placed where you
    /// spawn"; retail's travel and world spawn post cMsgSetSessionMarker, map-load spawn itself is UNTRACED).
    spawn: bool,
    /// Retail's set- and use-disabled lists (Lua SessionMarker.Disable/EnableSetSessionMarker 0x82D55710 /
    /// 0x82D557A0, Disable/EnableUseSessionMarker 0x82D55828 / 0x82D558B8): each caller is in a list at most once,
    /// and while either list holds anyone, 8289BA10 / 8289BAE0 refuse the place / the return. Kept across a map
    /// load: retail holds them in the activity manager ([dc+0x184]), not in the marker.
    set_disabled: std::collections::BTreeSet<&'static str>,
    use_disabled: std::collections::BTreeSet<&'static str>,
    blocked_until_release: bool,
    ui_time: f64,
    last_batch: u64,
}
impl SessionMarker {
    /// SessionMarker.DisableSetSessionMarker (`true`) / EnableSetSessionMarker (`false`), for `by`.
    pub(crate) fn disable_set(&mut self, by: &'static str, disabled: bool) {
        if disabled { self.set_disabled.insert(by) } else { self.set_disabled.remove(by) };
    }
    /// SessionMarker.DisableUseSessionMarker (`true`) / EnableUseSessionMarker (`false`), for `by`.
    pub(crate) fn disable_use(&mut self, by: &'static str, disabled: bool) {
        if disabled { self.use_disabled.insert(by) } else { self.use_disabled.remove(by) };
    }
}

pub(crate) struct SessionMarkerPlugin;
impl Plugin for SessionMarkerPlugin {
    fn build(&self, app: &mut App) {
        let root = &app.world().resource::<crate::config::Config>().asset_root;
        match validation::Validation::load(root) {
            Ok(settings) => {
                app.insert_resource(settings);
            }
            Err(e) => {
                error!("Session marker validation data: {e}");
                return;
            }
        }
        app.insert_resource(SessionMarker { spawn: true, ..default() })
            .add_message::<crate::skate_audio::FrontEndSoundRequest>()
            .add_systems(
                PreUpdate,
                suspend.after(crate::map_transition::MapTransitionSet),
            )
            .add_systems(
                FixedUpdate,
                (update, player_list)
                    .chain()
                    .after(SimulationSet::Input)
                    .before(SimulationSet::Controls)
                    .run_if(crate::graphics_menu::gameplay_active)
                    // Not over Edit Skater: its LB / RB step the clothing grids by game.
                    .run_if(crate::customiser::closed),
            );
        hud::install(app);
        effect::install(app);
    }
}

fn suspend(
    mut marker: ResMut<SessionMarker>,
    map: Res<CurrentMap>,
    menu: Res<crate::graphics_menu::Menu>,
    replay: Res<crate::replay::Replay>,
    time: Res<Time<Real>>,
    matched: Option<Res<crate::match_core::Match>>,
) {
    if marker.generation != map.generation {
        *marker = SessionMarker {
            generation: map.generation,
            blocked_until_release: true,
            sets: marker.sets,
            uses: marker.uses,
            spawn: true,
            set_disabled: std::mem::take(&mut marker.set_disabled),
            use_disabled: std::mem::take(&mut marker.use_disabled),
            ..default()
        };
    }
    // A match's Lua with Pause.DisablePauseMenuAndBackpack in force blocks the markers too, as a Spot Battle does
    // (match_core::Match::markers_blocked).
    if !crate::graphics_menu::gameplay_active(Some(menu)) || replay.active || matched.is_some_and(|m| m.markers_blocked()) {
        marker.hold.cancel();
        marker.visible = false;
        marker.progress = 0.;
        marker.oob = 0.;
        marker.blocked_until_release = true;
        marker.ui_time = 0.;
    } else if !marker.blocked_until_release {
        // PlayerUI runs on the UI clock, independently of physics substeps.
        marker.ui_time += time.delta_secs_f64();
    }
}

fn update(
    vehicles: Res<crate::modding::vehicles::Vehicles>,
    mut session: ResMut<SessionMarker>,
    input: Res<ControllerInput>,
    map: Res<CurrentMap>,
    physics: Res<GamePhysics>,
    mut skater: ResMut<SkaterRuntime>,
    validation: Res<validation::Validation>,
    replay: Res<crate::replay::Replay>,
    mut audio: MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
    matched: Option<Res<crate::match_core::Match>>,
    mut music: Option<ResMut<crate::music::Music>>,
    menu: Option<Res<crate::graphics_menu::Menu>>,
    network: Option<Res<crate::multiplayer::Multiplayer>>,
    challenge: Option<Res<crate::challenge_map::ChallengeMap>>,
    mut rules: Local<Option<std::collections::BTreeMap<String, [bool; 3]>>>,
    mut traffic: Option<ResMut<crate::traffic::Traffic>>,
) {
    let (set_off, use_off) = matched.as_ref().map_or((false, false), |m| m.marker_lists());
    session.disable_set("match", set_off);
    session.disable_use("match", use_off);
    // TU3 0x8289BA10 / 0x8289BAE0: the running challenge's AllowSessionMarkersToBeSet / ToBeUsed (and ToBeUsedInWipeout,
    // can_return below) through its parents; no value reads false. Only while a Skate 3 match runs: retail's activity
    // getter (dc+0x148 vfunc 0xC) is not traced to its writer, but every freeskate_* record reads false for both, so in
    // free skate (markers work there) it can't be one. No rules (online.json before a Rebuild): nothing changes.
    let rules = rules.get_or_insert_with(|| challenge.as_deref().map(|c| c.marker_rules()).unwrap_or_default());
    let activity = matched
        .as_ref()
        .filter(|m| m.phase != crate::match_core::Phase::Idle && crate::skate2_hud::map_game(&map) == 3)
        .and_then(|m| rules.get(&m.row.to_ascii_lowercase()))
        .copied();
    session.disable_set("challenge", activity.is_some_and(|[set, _, _]| !set));
    session.disable_use("challenge", activity.is_some_and(|[_, used, _]| !used));
    if matched.is_some_and(|m| m.markers_blocked()) {
        session.blocked_until_release = true;
        return;
    }
    if vehicles.occupied() || replay.active {
        // An online auto-set must not land wherever the vehicle or replay ends.
        session.request_set = false;
        session.blocked_until_release |= vehicles.occupied();
        return;
    }
    session.spawn |= std::mem::take(&mut skater.mark_after_travel);
    let (modifier, set, held) = input.session_marker_actions();
    let p = &skater.player_input.physical;
    let on_board = p.state.category_12 != 500;
    let deck = physics.board.part_transforms()[BodyId::Deck.index()];
    let mut transform = effective_root(
        skater.animated_skeleton.roots.animation_to_world,
        skater.player_input.processed.flags_2476,
    );
    // 82591DA8: the skeleton branch off board (State75) or in a bail (State59), the deck branch otherwise.
    if on_board && !skater.player_state.state_flags[59 - 52] {
        for i in 0..3 {
            transform[i][..3].copy_from_slice(&deck.basis.columns[i]);
        }
        transform[3] = [
            deck.translation.x,
            deck.translation.y + 0.2,
            deck.translation.z,
            0.,
        ];
        //82591E30: above .5m/s, project normalized velocity onto world Up.
        //The cross products are deliberately not normalized a second time.
        let velocity = Vec3::from_slice(&p.skateboard.vector_80.map(f32::from_bits)[..3]);
        if velocity.length_squared() > 0.25 {
            let right = Vec3::Y.cross(velocity.normalize());
            let forward = right.cross(Vec3::Y);
            if forward.length_squared() > 0.9 {
                transform[0] = right.extend(0.).to_array();
                transform[1] = Vec3::Y.extend(0.).to_array();
                transform[2] = forward.extend(0.).to_array();
            }
        }
    }
    // Once the teleport has landed (its probe grounds it), so the marker is where the skater stands.
    if session.spawn && skater.player_input.pending_teleport().is_none() {
        session.spawn = false;
        session.request_set = false;
        session.marker = Some(Marker {
            transform,
            on_board,
            foot_forward: skater.animation.foot_forward(),
            generation: map.generation,
        });
    }
    if std::mem::take(&mut session.request_set) {
        session.marker = Some(Marker {
            transform,
            on_board,
            foot_forward: skater.animation.foot_forward(),
            generation: map.generation,
        });
        session.sets += 1;
    }
    if session.blocked_until_release {
        if !modifier {
            session.blocked_until_release = false;
        }
        return;
    }
    let state = p.state.state_16;
    let state_allowed = (p.state.category_12 == 100
        && p.collision.wheel_count_0 >= 2
        && state != 104
        && deck.basis.columns[1][1] > 0.71)
        || (p.state.category_12 == 500 && state == 500);
    // 8289B358: the ground and clearance check (82BFAFF0) runs only online (TheSimulation IsOnline, 0x8285A870);
    // offline retail places the marker without it.
    let online = network.is_some_and(|n| n.active());
    session.can_place = modifier
        && state_allowed
        && p.surface_default_mode != 8
        && session.set_disabled.is_empty()
        && (!online || validation.check(physics.world(), transform[3]));
    session.can_return = session
        .marker
        .is_some_and(|m| m.generation == map.generation)
        && !matches!(state, 104 | 502)
        && session.use_disabled.is_empty()
        // 0x8289BAE0: in a wipeout (category 300) only if the challenge allows it.
        && !(p.state.category_12 == 300 && activity.is_some_and(|[_, _, wipeout]| !wipeout));
    // 8289B358's other gates, not ported (what they read has no counterpart here):
    // - flow state [0x830670CC] != 5 clears both (8289B374); `update` runs only under gameplay_active.
    // - +0x154B (8289BC50: a type-6 message naming HighAlert_Area / NoSkateZone_Area, 82C0E890) allows the place only
    //   online, in game mode [0x830B7AE8] 3, or with [[dc+0xDC]+0x20C]+0x34 / +0x35: the port has no such areas.
    // - game mode 1 with the sign-up manager (dc+0xDC+0x160, 82D53140) in state 4-6 / 8-13 clears both: the port has no
    //   sign-up flow. UNTRACED: what modes 1 / 3 and those states are.
    // - [[dc+0x124]+0x177D], a camera override FE states set (8288B708) and clear (8288B9E8), clears both. UNTRACED:
    //   which FE states; the port's menus over play stop `update` instead.
    // - 8289BAE0 refuses the return while AI.IsLocalPlayerBeingChased ([0x830854B8] vfunc 4, 0x8284D380; no security
    //   AI here), or with ChallengeHOMManager [[dc+0xDC]+0x200]+0x472 clear (its ctor sets it, 0x82568C44; no other
    //   D-form store to it found, UNTRACED beyond that).
    // LB opens the panel with `cellphone_activate` if 82668BD8 passes (the game mode, then the skater's state, as
    // Skate 2's 0x8258B770; ponytail: the skater part is `state_allowed`); closing is silent. Under the Skate 2 HUD
    // skate2_hud voices it in Skate 2's. Not ported, UNTRACED: 82668BD8 first refuses on [[0x830CFE0C]+0x1C] (825D60C8,
    // +0x321) and [[0x830670A0]+0xC] vfunc 4; modes 4 / 5 (online), 0 and 3 then pass; 1 / 7 also refuse while the
    // activity's KilledItHUD (none: OwnedItHUD) shows or its countdown runs, or the sign-up manager is busy; others refuse.
    let opened = modifier && !session.visible && state_allowed;
    session.phone_open = modifier && (opened || session.phone_open && session.visible);
    session.visible = modifier;
    session.allowed = state_allowed;
    if opened {
        audio.write(crate::skate_audio::FrontEndSoundRequest(CELLPHONE_ACTIVATE));
    }
    // A retained Pad publication must not turn one .pressed into repeated sets.
    if set && session.last_batch != input.consumed_batches {
        info!(
            "Session marker set: allowed {} (category {}, state {state}, wheels {}, deck up {:.2}), surface mode {}, placeable {}",
            state_allowed,
            p.state.category_12,
            p.collision.wheel_count_0,
            deck.basis.columns[1][1],
            p.surface_default_mode,
            session.can_place
        );
        if session.can_place {
            session.marker = Some(Marker {
                transform,
                on_board,
                foot_forward: skater.animation.foot_forward(),
                generation: map.generation,
            });
            session.sets += 1;
            audio.write(crate::skate_audio::FrontEndSoundRequest(0x0d6c_88a3_b91c_828f));
        } else {
            audio.write(crate::skate_audio::FrontEndSoundRequest(0x66b3_afe3_b602_918c));
        }
    }
    // The open panel's song shortcuts (826682B0 codes 0x4000 / 0x8000 in mode 3), gated by 8249C078: in Playlist Mode,
    // Music Volume (settings +0x88) != 0 and no console music playing (82D15AC0; ponytail: My Music is not treated as
    // console music, so it skips too). Each plays its sound, then sets the music system's flag (8249C168 reads them).
    let (next, restart) = input.track_actions();
    let live = menu.is_some_and(|m| m.audio().volumes[crate::music::MUSIC] != 0);
    if session.phone_open
        && live
        && session.last_batch != input.consumed_batches
        && let Some(music) = music.as_deref_mut()
    {
        // Each Apt key is its own event (825EC0E8), so both can come in one poll; 8249C168 takes restart and leaves
        // next for its following update, as music::sequence does.
        if restart {
            audio.write(crate::skate_audio::FrontEndSoundRequest(RESTART_SONG));
            music.restart = true;
        }
        if next {
            audio.write(crate::skate_audio::FrontEndSoundRequest(NEXT_SONG));
            music.skip = true;
        }
    }
    session.last_batch = input.consumed_batches;
    let ready = p.state.flag_69 == 0
        && state != 702
        && !(physics.board_wiping_out && p.skeleton.teleport_pending_604 != 0)
        && skater.player_input.pending_teleport().is_none();
    let distance = session.marker.map_or(0., |m| {
        // 82DB6EC0 -> 82BE1AE8 publishes animation-to-world at output+368;
        // UpdateSessionMarker reads its translation at output+416.
        Vec3::from_slice(&skater.animated_skeleton.roots.animation_to_world[3][..3])
            .distance(Vec3::from_slice(&m.transform[3][..3]))
    });
    let usable = session.can_return;
    while session.ui_time >= 1. / 60. {
        session.ui_time -= 1. / 60.;
        let step = session.hold.update(held, usable, distance, ready);
        session.progress = step.progress;
        if step.relocate {
            if let Some(target) = session.marker {
                match skater.player_input.request_teleport(target.transform) {
                    Ok(()) => {
                        skater.animation.restore_foot_forward(target.foot_forward);
                        skater
                            .teleport_state
                            .request_manual(target.transform, target.on_board);
                        session.uses += 1;
                        audio.write(crate::skate_audio::FrontEndSoundRequest(0x7f13_5f9f_d28f_7f21));
                        // Bug 10-25 #24: as retail, the car the marker was set on goes (Sim::clear_at).
                        if let Some(sim) = traffic.as_mut().and_then(|t| t.sim.as_mut()) {
                            sim.clear_at(Vec3::from_slice(&target.transform[3][..3]));
                        }
                    }
                    Err(e) => {
                        warn!("Session marker return rejected: {e}");
                        session.hold.cancel();
                    }
                }
            }
        }
    }
}

/// SwapCameraAvailable 0x82669090, whose B row reads "Swap Camera" (cameraman-spec.md Part 2): online, session type 22
/// (online free skate: no challenge lobby and no match running) and 2+ players, ourselves included. Elsewhere B reads
/// Edit Park / the greyed Object Dropper.
pub(crate) fn swap_camera_available(online: bool, lobby: bool, match_running: bool, players: usize) -> bool {
    online && !lobby && !match_running && players >= 2
}

/// Not retail (user 10-25): where retail's B row reads Swap Camera and each press moves the camera to the next player
/// (0x826682b0), it reads Player List, and LB+B opens the Start menu's Spectate list. Under any panel (Skate 3's, or the
/// Skate 2 / Skate 1 one Match Map shows): the press is the pad's, not the movie's.
fn player_list(
    mut session: ResMut<SessionMarker>,
    mut input: ResMut<ControllerInput>,
    mut published: ResMut<crate::input::PublishedTickInput>,
    network: Option<Res<crate::multiplayer::Multiplayer>>,
    roster: Option<Res<crate::online_roster::OnlineRoster>>,
    matched: Option<Res<crate::match_core::Match>>,
) {
    let roster = roster.as_deref();
    session.player_list = swap_camera_available(
        network.is_some_and(|n| n.active()),
        roster.is_some_and(|r| r.state.challenge.is_some()),
        matched.is_some_and(|m| m.phase != crate::match_core::Phase::Idle),
        roster.map_or(0, |r| r.members.len()),
    );
    open_player_list(&mut session, &mut input, &mut published);
}

/// LB+B on the Player List row asks for the list. The press never reaches the skater: this tick goes out neutral (it
/// was published in SimulationSet::Input, before this) and menu_release holds every tick after until the pad is up.
fn open_player_list(session: &mut SessionMarker, input: &mut ControllerInput, published: &mut crate::input::PublishedTickInput) {
    if session.visible && session.player_list && !input.gated() && input.b_pressed() {
        session.open_player_list = true;
        input.menu_release = true;
        published.0 = input.tick_input();
    }
}

/// Bug 10-25 #29. 82591E30's skeleton branch saves PhysOut Skeleton+432, which 82BE2100 fills from GetEffectiveRoot
/// 82BE3650: Skeleton+11920 (the animation root) with every lane of X and Z negated while Processed2476 bit2 (the
/// animation mirror) is set. The return (Flow::Teleport 825582D0 -> 82706F50 -> 82DB8998) heads the board along the
/// saved Z, so saving the raw root faced a mirrored skater backwards.
pub(crate) fn effective_root(mut root: AnimationPartTransform, flags_2476: u32) -> AnimationPartTransform {
    if flags_2476 & 4 != 0 {
        for axis in [0, 2] {
            root[axis] = root[axis].map(|v| -v);
        }
    }
    root
}

#[cfg(test)]
mod tests {
    /// 82BE3650 on a mirrored skater: right and forward reversed lane for lane, up and position kept; unmirrored,
    /// the root as it is.
    #[test]
    fn an_on_foot_marker_saves_the_effective_root() {
        let root = [[0., 0., -1., 0.], [0., 1., 0., 0.], [1., 0., 0., 0.], [3., 4., 5., 1.]];
        assert_eq!(super::effective_root(root, !4), root);
        assert_eq!(
            super::effective_root(root, 4),
            [[0., 0., 1., 0.], [0., 1., 0., 0.], [-1., 0., 0., 0.], [3., 4., 5., 1.]]
        );
    }

    #[test]
    fn the_phonelist_sounds_are_the_vaults_records() {
        use crate::core_menu_runtime::fe_sound_id;
        assert_eq!(
            (super::CELLPHONE_ACTIVATE, super::NEXT_SONG, super::RESTART_SONG),
            (fe_sound_id("cellphone_activate"), fe_sound_id("next_song"), fe_sound_id("restart_song"))
        );
    }

    /// 0x82D55710 / 0x82D557A0: a caller is in the list once, so one Enable undoes any number of its Disables, and
    /// another caller's Disable still holds.
    #[test]
    fn a_disabled_list_holds_each_caller_once() {
        let mut marker = super::SessionMarker::default();
        marker.disable_set("ots", true);
        marker.disable_set("ots", true);
        marker.disable_set("film", true);
        marker.disable_set("ots", false);
        assert_eq!(marker.set_disabled.iter().copied().collect::<Vec<_>>(), ["film"]);
        marker.disable_set("film", false);
        assert!(marker.set_disabled.is_empty());
        marker.disable_use("photo", true);
        assert!(marker.set_disabled.is_empty() && !marker.use_disabled.is_empty(), "the two lists are separate");
    }

    /// SwapCameraAvailable 0x82669090: online free skate (no challenge lobby, no match) with 2+ players; else no.
    #[test]
    fn the_player_list_row_shows_where_retails_swap_camera_does() {
        use super::swap_camera_available as available;
        assert!(available(true, false, false, 2));
        assert!(available(true, false, false, 10));
        assert!(!available(false, false, false, 2), "offline");
        assert!(!available(true, true, false, 2), "a challenge lobby");
        assert!(!available(true, false, true, 2), "a match running");
        assert!(!available(true, false, false, 1), "alone in the session");
    }

    /// User 10-25: LB+B on the Player List row asks for the list; neither that tick nor the held buttons after it reach
    /// the skater. On the greyed Object Dropper row B reaches the skater as before.
    #[test]
    fn lb_b_opens_the_player_list_without_reaching_the_skater() {
        use crate::input::{ControllerInput, PublishedTickInput};
        let pad = |buttons| skate_core::input::xbox::XboxState { buttons, triggers: [0; 2], left: [0; 2], right: [0; 2] };
        for player_list in [false, true] {
            let mut input = ControllerInput::default();
            // LB held first: Pad ignores an edge for 3 polls after the last (82966F30).
            for _ in 0..4 {
                input.sample_raw_for_test(pad(0x0100));
            }
            input.sample_raw_for_test(pad(0x0100 | 0x2000)); // LB + B
            let mut published = PublishedTickInput(input.tick_input());
            assert_eq!(published.0.actions().values()[17], 1., "B (pad slot 13) is published");
            let mut session = super::SessionMarker { visible: true, player_list, ..Default::default() };
            super::open_player_list(&mut session, &mut input, &mut published);
            assert_eq!(session.open_player_list, player_list);
            if player_list {
                assert_eq!(*published.0.actions().values(), [0.0; 18], "the press tick goes out neutral");
                input.sample_raw_for_test(pad(0x0100 | 0x2000));
                assert_eq!(*input.tick_input().actions().values(), [0.0; 18], "held, they stay off");
            } else {
                assert_eq!(published.0.actions().values()[17], 1., "Object Dropper: B is the skater's");
            }
        }
    }
}
