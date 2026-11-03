//! The world clock: Settings > World > Time of Day (Day, Night or Cycle). Online, the host's choice and hour apply
//! (online_moves). The lighting reads `WorldTime::hour` only.
use bevy::prelude::*;

/// 0..24: 12 is midday (the authored daylight), 0 midnight.
#[derive(Resource)]
pub(crate) struct WorldTime {
    pub hour: f32,
}
impl Default for WorldTime {
    fn default() -> Self {
        Self { hour: 12. }
    }
}

/// The hour "Night" holds.
pub(crate) const NIGHT_HOUR: f32 = 0.;
/// Real seconds for a whole day in "Cycle" (the user asked for 15-20 minutes). Knob.
pub(crate) const DAY_SECONDS: f32 = 18. * 60.;
/// Time of Day choices; graphics_menu stores the index.
pub(crate) const TIMES: [&str; 3] = ["Day", "Night", "Cycle"];
pub(crate) const CYCLE: u32 = 2;

/// Virtual time, so a paused single-player game stops the sun.
pub(crate) fn tick(
    time: Res<Time<Virtual>>,
    menu: Res<crate::graphics_menu::Menu>,
    mut clock: ResMut<WorldTime>,
    (map, retail, customiser): (
        Option<Res<crate::map_transition::CurrentMap>>,
        Option<Res<crate::retail_render::RetailScene>>,
        Option<Res<crate::customiser::Customiser>>,
    ),
) {
    // Not retail (user 10-19): the Halloween and Christmas modes are always night where they decorate (seasonal.rs,
    // U10; online every player has the session's weather, so all of them).
    let season = crate::seasonal::active(Some(&menu), map.as_deref(), retail.as_deref(), customiser.as_deref());
    let choice = if season.is_some() { 1 } else { menu.time_of_day() };
    let hour = next_hour(choice, clock.hour, time.delta_secs());
    if clock.hour != hour {
        clock.hour = hour;
    }
}

pub(crate) fn next_hour(choice: u32, hour: f32, seconds: f32) -> f32 {
    match choice {
        0 => 12.,
        1 => NIGHT_HOUR,
        _ => (hour + seconds * 24. / DAY_SECONDS).rem_euclid(24.),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn day_and_night_hold_and_the_cycle_wraps_in_eighteen_minutes() {
        use super::*;
        assert_eq!(next_hour(0, 5., 100.), 12.);
        assert_eq!(next_hour(1, 5., 100.), NIGHT_HOUR);
        assert!((next_hour(CYCLE, 12., DAY_SECONDS / 2.) - 0.).abs() < 1e-3);
        assert!((next_hour(CYCLE, 23., DAY_SECONDS / 12.) - 1.).abs() < 1e-3);
    }
}
