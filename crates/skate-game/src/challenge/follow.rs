//! FollowTheLeader's state (moved from skate2_challenge.rs; Skate 1 binds the same manager).
use serde_json::{Value, json};

/// The leader's AI id, the warning row SetWarningInfo loaded, and the distance callbacks (id, metres, the player
/// within them) fired as playerradiusenter/exit while `updating`.
#[derive(Default)]
pub(crate) struct Follow {
    pub(crate) leader: i64,
    pub(crate) info: Vec<f64>,
    pub(crate) callbacks: Vec<(i64, f64, bool)>,
    pub(crate) updating: bool,
    pub(crate) distance: f64,
}

/// SetWarningInfo's row: [min, max, warning speaker, line, followee, recovery speaker, line, delay, repeat].
pub(crate) const GETTERS: [&str; 9] = [
    "GetMinDistance", "GetMaxDistance", "GetWarningCharacter", "GetWarningSpeechID", "GetWarningFollowee",
    "GetRecoveryCharacter", "GetRecoverySpeechID", "GetTimeBeforeWarningPlayed", "GetRepeatTime",
];

impl Follow {
    /// One leader-player distance (metres) while updating: the callbacks it crossed. The handlers compare Distance
    /// with the squared callback distance.
    pub(crate) fn crossings(&mut self, distance: f64) -> Vec<(&'static str, Value)> {
        if !self.updating {
            return Vec::new();
        }
        self.distance = distance;
        let mut out = Vec::new();
        for (_, metres, within) in &mut self.callbacks {
            if (distance <= *metres) != *within {
                *within = !*within;
                let event = if *within { "playerradiusenter" } else { "playerradiusexit" };
                out.push((event, json!({"PlayerID": self.leader, "Distance": *metres * *metres})));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A callback posts exit when the player falls out of its radius and enter when they come back, with the squared
    /// distance; nothing while updates are stopped.
    #[test]
    fn crossings_post_exit_and_enter_once_each_with_the_squared_distance() {
        let mut follow = Follow { leader: 7, callbacks: vec![(1, 10., true)], ..Default::default() };
        assert!(follow.crossings(20.).is_empty(), "not updating");
        follow.updating = true;
        assert!(follow.crossings(5.).is_empty());
        assert_eq!(follow.crossings(20.), [("playerradiusexit", json!({"PlayerID": 7, "Distance": 100.}))]);
        assert!(follow.crossings(30.).is_empty());
        assert_eq!(follow.crossings(10.), [("playerradiusenter", json!({"PlayerID": 7, "Distance": 100.}))]);
        assert_eq!(follow.distance, 10.);
    }
}
