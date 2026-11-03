//! Contest spectators: the large crowds of `Sk8::Challenge::cChallengeCrowdManager` and the
//! living world's spectator waypoints. Neither reaches anything the port draws: the crowd
//! manager's `cCrowdPositionAndGenerationState` (line-tested positions in the crowd's trigger
//! volume) feeds the presentation's `ICrowdBuffer` crowd renderer, and the spectator waypoints
//! are `LivingWorld::WaypointMan`'s forced spectator spawns (`PendingSpectatorActivation`); none
//! of those is ported, so these natives only take their arguments as the natives do.

use super::{
    lua::{Lua, LuaResult, NativeCall, NativeFn, Value},
    system::World,
};

type R = LuaResult<Vec<Value>>;

/// `lua_touserdata(1)` of a native that asserts its argument is light userdata (a Vault struct).
fn vault_struct(lua: &Lua<World>, call: &NativeCall, native: &str) -> LuaResult<()> {
    match call.arg(0) {
        Value::LightUserData(_) => Ok(()),
        _ => Err(lua.error(format!("{native}: expected light userdata"))),
    }
}

/// `Crowd.LoadLargeCrowd(tLargeCrowd)` (`0x828932c0`): `cChallengeCrowdManager::LoadCrowd(activity,
/// crowd)` (`0x82db1be8`) resets the crowd buffer, adds a crowd generator for the crowd's trigger
/// volume, density and generator type (`SetupSpawnPoints` `0x82db0438`) and resubmits every
/// generator's positions.
fn load_large_crowd(lua: &mut Lua<World>, _: &mut World, call: &NativeCall) -> R {
    vault_struct(lua, call, "Crowd.LoadLargeCrowd")?;
    Ok(Vec::new())
}

/// `Crowd.UnloadLargeCrowd(tLargeCrowd)` (`0x828933c0`): `UnLoadCrowd(activity, crowd)`
/// (`0x82da8028`) deletes the activity's generators on the crowd's trigger volume and dispatches
/// the remaining positions.
fn unload_large_crowd(lua: &mut Lua<World>, _: &mut World, call: &NativeCall) -> R {
    vault_struct(lua, call, "Crowd.UnloadLargeCrowd")?;
    Ok(Vec::new())
}

/// `LivingWorld.ActivateSpectatorWaypoint(tSpectatorSpawnInfo)` (`0x82893da8`): a waypoint group
/// found by the info's waypoint GUID (`ILWManager::GetWaypointByGUID`) that takes spectators gets
/// `WaypointMan::ActivateSpectatorWaypoint` (`0x82ce9180`): a census-managed cull, then the
/// group's share (`ForceSpawnPercentage`) of forced spectator spawns of the info's living world
/// category group, queued. A null info does nothing.
fn activate_spectator_waypoint(_: &mut Lua<World>, _: &mut World, _: &NativeCall) -> R {
    Ok(Vec::new())
}

/// `LivingWorld.DeactivateSpectatorWaypoint(tSpectatorSpawnInfo, forceCull = false)`
/// (`0x8288e488`): the group found by the info's waypoint GUID gets
/// `WaypointMan::DeactivateSpectatorWaypoint(group, true)` (`0x82ce8ed8`; the native reads
/// `forceCull` and passes `true`), which drops the group's queued activation and releases its
/// spectators. A null info does nothing.
fn deactivate_spectator_waypoint(_: &mut Lua<World>, _: &mut World, _: &NativeCall) -> R {
    Ok(Vec::new())
}

pub(crate) fn natives() -> Vec<(&'static str, &'static str, NativeFn<World>)> {
    vec![
        ("Crowd", "LoadLargeCrowd", load_large_crowd),
        ("Crowd", "UnloadLargeCrowd", unload_large_crowd),
        ("LivingWorld", "ActivateSpectatorWaypoint", activate_spectator_waypoint),
        ("LivingWorld", "DeactivateSpectatorWaypoint", deactivate_spectator_waypoint),
    ]
}
