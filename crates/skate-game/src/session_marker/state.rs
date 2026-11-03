//! PlayerUI::UpdateSessionMarker 82898FC8. Time advances in native UI ticks.
#[derive(Default, Debug)]
pub(super) struct Hold {
    fired: bool,
    tail: u8,
}

#[derive(Default, Debug, PartialEq)]
pub(super) struct Step {
    pub progress: f32,
    pub relocate: bool,
}

impl Hold {
    pub fn cancel(&mut self) {
        *self = Self::default();
    }

    pub fn update(&mut self, held: bool, usable: bool, distance: f32, ready: bool) -> Step {
        let mut out = Step::default();
        if !held {
            self.fired = false;
        } else if !self.fired && usable && ready {
            // Not retail (owner 10-18: "as close to instant as possible"): retail ramps the static up over a hold,
            // 0.2 s up to 100 m, 1 s from 1000 m (`distance * 0.00089 + 0.111` between, 1/60 s per UI tick), and
            // relocates once it is full. Here the first ready tick relocates. Nothing waited on that ramp: the map's
            // whole collision is built at load (physics.rs, skate_world::collision_world), and a marker from an
            // earlier map generation is refused (can_return), so the ground under the marker always exists.
            //828992C8: within half a metre there is no effect or relocation.
            if distance > 0.5 && distance.is_finite() {
                self.fired = true;
                self.tail = 3;
                out.relocate = true;
            }
        }
        //828994F0 also executes in the relocation tick.
        if self.tail > 0 {
            out.progress = 1.;
            self.tail -= 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_cancels_and_completed_hold_does_not_repeat() {
        let mut hold = Hold::default();
        let mut count = 0;
        for _ in 0..120 {
            count += usize::from(hold.update(true, true, 20., true).relocate);
        }
        assert_eq!(count, 1);
        assert_eq!(hold.update(true, true, 20., true).progress, 0.);
        assert_eq!(hold.update(false, true, 20., true), Step::default());
        assert!(hold.update(true, true, 20., true).relocate, "a new press returns again");
    }
    #[test]
    fn unset_nearby_and_blocked_returns() {
        let mut hold = Hold::default();
        for _ in 0..100 {
            assert_eq!(hold.update(true, false, 10., true), Step::default());
        }
        for _ in 0..100 {
            assert_eq!(hold.update(true, true, 0.5, true), Step::default());
        }
        assert!(!hold.update(true, true, 10., false).relocate);
        assert!(hold.update(true, true, 10., true).relocate);
        assert_eq!(hold.update(false, true, 10., true).progress, 1.);
        assert_eq!(hold.update(false, true, 10., true).progress, 1.);
        assert_eq!(hold.update(false, true, 10., true).progress, 0.);
    }
    /// Not retail (owner 10-18): the first held tick relocates at any distance (retail: tick 13 at 100 m, 36 at
    /// 550 m, 61 at 1000 m), under the 3-tick static tail.
    #[test]
    fn the_return_is_instant_at_any_distance() {
        for distance in [1., 100., 550., 1000., 5000.] {
            let mut hold = Hold::default();
            assert_eq!(hold.update(true, true, distance, true), Step { progress: 1., relocate: true }, "{distance}");
        }
    }
}
