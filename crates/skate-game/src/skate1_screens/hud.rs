//! Skate 1's HUD manager for the tutorial window (RETAIL; s1-screens ledger, research.md 2.0).
//! - HUD.ChangeHUDState queues (state, sub) FIFO (0x8234e720).
//! - The queue is applied at the end of the FE update (0x8234d488), after the active state's Update (vt+16, 16 ms per
//!   60 Hz tick).
//! - A new state runs the old one's Deactivate (vt+20), its own Activate (vt+12), then SetSubState. The same state runs
//!   SetSubState alone.
//! - SetSubState:
//!   - Film / Photo run Exit(old sub) first.
//!   - Then the state's own sub is stored and its timer zeroed.
//!   - Then Enter(sub) runs, unless the active type is 32 / 24 / 25 / 29.
//! Pure: it returns [`Act`]s, and skate1_screens.rs carries them out.
use super::{Event, Pad};
use std::collections::{BTreeMap, VecDeque};

pub(crate) const FREE_SKATE: u8 = 1;
pub(crate) const FILM: u8 = 15;
pub(crate) const PHOTO: u8 = 17;
pub(crate) const SKATE: u8 = 18;
pub(crate) const TUTORIAL: u8 = 19;
/// MissionBestTrickChallenge: Community Spirit (user 10-15). RETAIL object vtable 0x82169b50 (built by 0x82348b58).
pub(crate) const BEST_TRICK: u8 = 36;
pub(crate) const FOLLOW: u8 = 67;
/// The best-trick state's stored sub when none is set (+0x34 init 0x10).
const NO_SUB: i64 = 16;
/// The best-trick round leaderboard's hold before FE 49 (ms; Update 0x8236c6c8, 0xa8c).
const BOARD_AFTER: u32 = 2700;
/// Update's step per 60 Hz tick (ms).
const STEP: u32 = 16;
/// Active types (0x830a3f64) whose SetSubState skips Enter; MarkTutorialObjectiveComplete ignores them too.
const GUARDED: [u8; 4] = [32, 24, 25, 29];
/// Skate's plain Enter -> overlay SetState, subs 3..=24 (10 / 11 / 12 have their own arms; 0 there is unused).
const SKATE_SETS: [i64; 22] = [1, 2, 17, 3, 4, 18, 5, 0, 0, 0, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 24, 25];

/// The overlay movies 0x82471000 loads (idx film 1, photo 4, best trick 7, follow 11, skate 12, tutorial 15) and
/// hudframe+0's action overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Movie { Action, Film, Photo, Skate, Tutorial, Follow, BestTrick }

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Act {
    /// 0x82471000(idx): load this overlay (None: unload); hudloaded follows (at once when it is already loaded).
    Overlay(Option<Movie>),
    /// The movie's SetState(n).
    Set(Movie, i64),
    /// hudbase SetState 2 / 3 through 0x82471a98, gate resolved: the trick display shows.
    Trick(bool),
    /// hudbase SetState 4 through 0x82471cb0, gate resolved: the minimap shows.
    Minimap(bool),
    /// An FE push: 65 SignUpPoster, 73 TutorialComplete, 23 EndChallengeOptions, 24 MedicalBill, 74 PressA,
    /// 49 LeaderBoard.
    Fe(u8),
    Post(Event),
    /// The film menu's objective HUD for film i (hudobjective Setup('film')).
    FilmMenu(usize),
}

/// What the tables read from the game.
#[derive(Clone, Copy, Default)]
pub(crate) struct Ctx<'a> {
    pub key: &'a str,
    pub films: usize,
    pub film01_done: bool,
    pub spawn_rows: bool,
    /// S.K.A.T.E.: the current player's record +24 == 3 (meaning UNTRACED) and the local player's +28 == 1.
    pub kind3: bool,
    pub skate_won: bool,
    /// Best trick: (round, max rounds, the local player's record value == 1). INFERRED: that value is the overall
    /// rank (Contest.SetOverallStanding).
    pub best: (i64, i64, bool),
}

/// 0x82471a98. The trick display shows only when all of these hold:
/// - it is asked for;
/// - he_u1_film01 is done;
/// - the active type allows it: never in S.K.A.T.E. (4); in a tutorial (0) only with spawn rows and not in
///   fp_u3_tutorial03; any other type, always.
/// Off always hides. Mode 1 (career) assumed (UNTRACED: mode 6).
pub(crate) fn trick_shown(on: bool, kind: u8, c: &Ctx) -> bool {
    on && c.film01_done
        && match kind {
            4 => false,
            0 => c.spawn_rows && c.key != "fp_u3_tutorial03",
            _ => true,
        }
}
/// 0x82471cb0: the minimap shows when asked and he_u1_film01 is done (RETAIL; research.md's type gate was wrong).
pub(crate) fn minimap_shown(on: bool, c: &Ctx) -> bool {
    on && c.film01_done
}

#[derive(Default)]
pub(crate) struct Manager {
    queue: VecDeque<(u8, i64)>,
    pub(crate) state: Option<u8>,
    /// Each state object's own stored sub.
    subs: BTreeMap<u8, i64>,
    timer: u32,
    /// The active type 0x830a3f64 (Activate sets it).
    pub(crate) kind: u8,
    /// The overlay slot and whether its hudloaded has come.
    overlay: (Option<Movie>, bool),
    /// Film's reload flag: a SelectFilm that found the overlay unloaded comes back on OnLoaded.
    reload: bool,
    /// The film menu: (selected, count).
    film: (usize, usize),
    /// SetWinState (vt+116, INFERRED): what FE 23 / 73 show (Mission.GetWinState).
    pub(crate) won: bool,
}

impl Manager {
    pub(crate) fn change(&mut self, state: u8, sub: i64) {
        self.queue.push_back((state, sub));
    }

    /// The active state's sub (-1: none).
    pub(crate) fn sub(&self) -> i64 {
        self.state.and_then(|s| self.subs.get(&s).copied()).unwrap_or(-1)
    }

    /// The film menu's (selected, count) while it is up (Film sub 1).
    pub(crate) fn film_menu(&self) -> Option<(usize, usize)> {
        (self.state == Some(FILM) && self.sub() == 1).then_some(self.film)
    }

    /// One 60 Hz tick: the active state's Update, then the queue.
    pub(crate) fn tick(&mut self, c: &Ctx) -> Vec<Act> {
        let mut out = Vec::new();
        self.timer += STEP;
        self.update(c, &mut out);
        while let Some((state, sub)) = self.queue.pop_front() {
            if self.state != Some(state) {
                if let Some(old) = self.state {
                    self.deactivate(old, c, &mut out);
                }
                self.state = Some(state);
                self.activate(state, c, &mut out);
            }
            self.set_sub(sub, c, &mut out);
        }
        out
    }

    /// hudloaded for `movie`. Film's OnLoaded re-enters SelectFilm when its reload flag is set (RETAIL).
    pub(crate) fn loaded(&mut self, movie: Movie, c: &Ctx) -> Vec<Act> {
        let mut out = Vec::new();
        if self.overlay.0 == Some(movie) {
            self.overlay.1 = true;
        }
        if movie == Movie::Film && self.state == Some(FILM) && std::mem::take(&mut self.reload) {
            self.set_sub(1, c, &mut out);
        }
        out
    }

    /// cMsgFilmReplayAutoplayFinished (RETAIL handler 0x82466800, registered by 0x824a0cd0): with the active type Film
    /// (7, word 0x830a3f64), ChangeHUDState(MissionFilmChallenge 15, Options 6), whose Enter pushes FE 23.
    pub(crate) fn replay_finished(&mut self) {
        if self.kind == 7 {
            self.change(FILM, 6);
        }
    }

    /// HUD.MarkTutorialObjectiveComplete(n) (RETAIL 0x827b9c18): queues (Tutorial, 18 MarkObjectivesComplete), whose
    /// timer posts hudcomplete, and posts cMsgChallengeObjectiveComplete. The row's done flag is the lead's.
    pub(crate) fn mark_objective(&mut self, n: i64) -> Vec<Act> {
        if GUARDED.contains(&self.kind) {
            return Vec::new();
        }
        self.change(TUTORIAL, 18);
        vec![Act::Post(Event::ObjectiveComplete(n))]
    }

    /// Free skate's and the film menu's input (RB 50, Y 4): (taken, acts). UNTRACED, nothing: the film menu's B and
    /// Photo ViewPhotos' Y.
    pub(crate) fn press(&mut self, pad: Pad, c: &Ctx) -> (bool, Vec<Act>) {
        match (self.state, self.sub(), pad) {
            (Some(FREE_SKATE), _, Pad::Rb) if c.films > 0 => {
                self.change(FILM, 1);
                (true, Vec::new())
            }
            (Some(FILM), 1, Pad::Rb) if self.film.1 > 0 => {
                self.film.0 = (self.film.0 + 1) % self.film.1;
                self.timer = 0;
                (true, vec![Act::FilmMenu(self.film.0)])
            }
            (Some(FILM), 1, Pad::Y) => (true, vec![Act::Post(Event::StartFilm(self.film.0))]),
            _ => (false, Vec::new()),
        }
    }

    fn set_sub(&mut self, sub: i64, c: &Ctx, out: &mut Vec<Act>) {
        let Some(state) = self.state else { return };
        if matches!(state, FILM | PHOTO | BEST_TRICK) {
            self.exit(state, self.sub(), c, out);
        }
        self.subs.insert(state, sub);
        self.timer = 0;
        if !GUARDED.contains(&self.kind) {
            self.enter(state, sub, c, out);
        }
    }

    fn trick(&self, on: bool, c: &Ctx) -> Act {
        Act::Trick(trick_shown(on, self.kind, c))
    }

    fn load(&mut self, movie: Option<Movie>, out: &mut Vec<Act>) {
        if self.overlay.0 != movie {
            self.overlay = (movie, false);
        }
        out.push(Act::Overlay(movie));
    }

    fn activate(&mut self, state: u8, c: &Ctx, out: &mut Vec<Act>) {
        // UNTRACED, not ported:
        // - every Activate except Tutorial's first calls 0x8234e930(this, -1), an FE close whose target isn't traced;
        // - FreeSkate's also calls 0x8234f0e0.
        // A state the window never uses keeps the type and does nothing (UNTRACED).
        self.kind = match state {
            FREE_SKATE => 32,
            FILM => 7,
            PHOTO => 6,
            SKATE => 4,
            TUTORIAL => 0,
            FOLLOW => 23,
            BEST_TRICK => 8,
            _ => self.kind,
        };
        match state {
            FREE_SKATE => out.push(self.trick(true, c)),
            FILM => {
                self.film = (0, 0);
                out.push(Act::Minimap(minimap_shown(true, c)));
            }
            SKATE => out.extend([Act::Minimap(false), Act::Trick(false)]),
            TUTORIAL => out.push(Act::Minimap(false)),
            FOLLOW => out.push(Act::Minimap(minimap_shown(true, c))),
            // RETAIL 0x8236c5e0: mode 1 hides the minimap (0x82471cb0(0)). Its SetSubState(stored) when stored != 16
            // never runs here: Deactivate stores 16 and only a state change activates.
            BEST_TRICK => out.push(Act::Minimap(false)),
            _ => {}
        }
    }

    fn deactivate(&mut self, state: u8, c: &Ctx, out: &mut Vec<Act>) {
        let (trick, minimap) = (self.trick(true, c), Act::Minimap(minimap_shown(true, c)));
        match state {
            FILM => {
                self.exit(FILM, self.sub(), c, out);
                out.push(minimap);
                self.load(None, out);
            }
            PHOTO => {
                self.exit(PHOTO, self.sub(), c, out);
                self.load(None, out);
            }
            SKATE => {
                self.load(None, out);
                out.extend([minimap, trick]);
            }
            TUTORIAL => {
                self.load(None, out);
                out.extend([trick, minimap, Act::Post(Event::Indicator(false))]);
            }
            FOLLOW => {
                out.push(Act::Post(Event::Indicator(false)));
                self.load(None, out);
            }
            // RETAIL 0x8236c660: the overlay hides (SetState 3) and stays loaded; the minimap shows; sub := 16.
            BEST_TRICK => {
                out.extend([Act::Set(Movie::BestTrick, 3), minimap]);
                self.subs.insert(BEST_TRICK, NO_SUB);
            }
            _ => {} // FreeSkate's Deactivate: UNTRACED
        }
    }

    fn exit(&mut self, state: u8, sub: i64, c: &Ctx, out: &mut Vec<Act>) {
        match (state, sub) {
            (FILM, 1) => out.push(Act::Set(Movie::Action, 0)),
            (FILM, 5) => out.push(self.trick(true, c)),
            (PHOTO, 4) => out.push(Act::Set(Movie::Photo, 0)),
            (PHOTO, 7) => out.extend([Act::Set(Movie::Photo, 0), Act::Minimap(minimap_shown(true, c)), self.trick(true, c)]),
            // RETAIL 0x8236c840: leaving Countdown / Load sets the overlay's 21.
            (BEST_TRICK, 10 | 13) => out.push(Act::Set(Movie::BestTrick, 21)),
            _ => {}
        }
    }

    fn update(&mut self, c: &Ctx, out: &mut Vec<Act>) {
        let (Some(state), sub, t) = (self.state, self.sub(), self.timer) else { return };
        match (state, sub) {
            (TUTORIAL, 11 | 13) if t >= 2700 => self.queue.push_back((TUTORIAL, 17)),
            (TUTORIAL, 12) if t >= 750 => self.complete(out),
            (TUTORIAL, 18) if t >= 1000 => self.complete(out),
            (FILM, 1) if t >= 5000 => self.queue.push_back((FREE_SKATE, -1)),
            (FILM, 4) if t >= 2700 => self.set_sub(6, c, out),
            (PHOTO, 5) if t >= 4000 => self.set_sub(2, c, out),
            // RETAIL 0x8236c6c8: the round leaderboard's FE 49 (offline: 0x8234e580(mgr, 49, {-1, 1, 0}); its provider
            // state 0, the round board, unless the contest has one round), then the overlay hides.
            (BEST_TRICK, 2) if t >= BOARD_AFTER => {
                self.subs.insert(BEST_TRICK, NO_SUB);
                self.timer = 0;
                out.extend([Act::Fe(49), Act::Set(Movie::BestTrick, 3)]);
            }
            _ => {}
        }
    }

    /// hudcomplete, then the state parks at 26 so it posts once.
    fn complete(&mut self, out: &mut Vec<Act>) {
        out.push(Act::Post(Event::Complete));
        self.subs.insert(TUTORIAL, 26);
    }

    fn enter(&mut self, state: u8, sub: i64, c: &Ctx, out: &mut Vec<Act>) {
        use self::{Act::*, Movie as M};
        match (state, sub) {
            (TUTORIAL, 0) => self.load(Some(M::Tutorial), out),
            (TUTORIAL, 1) => {
                out.push(Fe(65));
                self.subs.insert(TUTORIAL, 26);
            }
            (TUTORIAL, 2) => out.extend([self.trick(true, c), Set(M::Tutorial, 1)]),
            (TUTORIAL, 3) => out.extend([Set(M::Tutorial, 3), Set(M::Tutorial, 7), Set(M::Tutorial, 9)]),
            // 5/6/7 -> 2/3/4 and 8/9/10 -> 5/6/7
            (TUTORIAL, 5..=10) => out.push(Set(M::Tutorial, sub - 3)),
            (TUTORIAL, 11) => {
                self.won = true;
                out.push(Set(M::Tutorial, if c.key == "fp_u2_tutorial05" { 15 } else { 13 }));
            }
            (TUTORIAL, 12) => {
                self.won = false;
                out.push(Set(M::Tutorial, 14));
            }
            (TUTORIAL, 13) => {
                self.won = true;
                out.push(Set(M::Tutorial, 15));
            }
            (TUTORIAL, 14) => out.push(Set(M::Tutorial, 8)),
            (TUTORIAL, 15) => out.extend([self.trick(true, c), Set(M::Tutorial, 11)]),
            (TUTORIAL, 16) => out.extend([Trick(false), Set(M::Tutorial, 12)]),
            (TUTORIAL, 17) => out.push(Fe(73)), // its timer := 0 is set_sub's
            (TUTORIAL, 18) => out.push(Set(M::Tutorial, 16)),
            (TUTORIAL, 19) => out.push(Set(M::Tutorial, 17)),
            (TUTORIAL, 20) => out.push(Set(M::Tutorial, 18)),
            (TUTORIAL, 21) => out.extend([Minimap(minimap_shown(true, c)), Post(Event::Indicator(true))]),
            (TUTORIAL, 22) => out.push(Post(Event::Indicator(false))),
            (TUTORIAL, 23) => {
                self.won = false;
                out.push(Fe(23));
            }
            (TUTORIAL, 24) => out.push(Set(M::Tutorial, 19)),
            (FILM, 0) => self.load(Some(M::Film), out),
            (FILM, 1) if self.overlay != (Some(M::Film), true) => {
                self.reload = true;
                self.set_sub(0, c, out);
            }
            (FILM, 1) => {
                self.film = (0, c.films);
                out.extend([Set(M::Action, 3), FilmMenu(0)]);
            }
            (FILM, 2) => out.push(Set(M::Film, 2)),
            (FILM, 3) => {
                self.won = true;
                out.push(Set(M::Film, 5));
            }
            (FILM, 4) => {
                self.won = false;
                out.extend([Set(M::Film, 4), Set(M::Action, 0)]);
            }
            (FILM, 5) => out.extend([Set(M::Action, 0), Set(M::Film, 3), Trick(false), Minimap(false)]),
            (FILM, 6) => {
                self.subs.insert(FILM, 2);
                out.extend([Set(M::Action, 0), Set(M::Film, 0), Fe(23)]);
            }
            (FILM, 7) => out.push(Set(M::Film, 0)),
            (FILM, 8) => out.extend([Minimap(minimap_shown(true, c)), Set(M::Film, 1), Set(M::Action, 0), Set(M::Film, 6)]),
            (FILM, 9) => out.push(Set(M::Film, 7)),
            (FILM, 10) => out.push(Set(M::Film, 8)),
            (PHOTO, 0) => self.load(Some(M::Photo), out),
            (PHOTO, 1) => {
                out.push(Fe(65));
                self.subs.insert(PHOTO, 12);
            }
            (PHOTO, 2) => out.push(Set(M::Photo, 1)),
            (PHOTO, 3) => {
                self.won = true;
                out.push(Set(M::Photo, 2));
            }
            (PHOTO, 4) => out.push(Set(M::Photo, 4)),
            (PHOTO, 5) => {
                self.won = false;
                out.push(Set(M::Photo, 3));
            }
            (PHOTO, 6) => out.push(Set(M::Photo, 5)),
            (PHOTO, 7) => out.extend([Set(M::Photo, 6), Minimap(false), Trick(false)]),
            (PHOTO, 8) => {
                self.subs.insert(PHOTO, 2);
                out.push(Fe(23));
            }
            (PHOTO, 9) => out.push(Set(M::Photo, 0)),
            (PHOTO, 10) => out.push(Fe(24)),
            (SKATE, 0) => self.load(Some(M::Skate), out),
            (SKATE, 1) => {
                self.subs.insert(SKATE, 26);
                out.extend([Set(M::Skate, 7), Set(M::Skate, 32)]);
            }
            (SKATE, 2) => {
                out.push(Fe(65));
                self.subs.insert(SKATE, 26);
            }
            (SKATE, 10) => {
                self.subs.insert(SKATE, 26);
                self.won = c.skate_won;
                out.extend([Set(M::Skate, 0), Fe(23)]); // its timer := 0 is set_sub's
            }
            (SKATE, 11) => out.push(Set(M::Skate, if c.skate_won { 22 } else { 23 })),
            (SKATE, 12) => out.push(Fe(74)),
            (SKATE, 14 | 16) if c.kind3 => out.push(Set(M::Skate, 31)),
            (SKATE, 22) if c.kind3 => out.push(Set(M::Skate, 30)),
            (SKATE, 3..=24) => out.push(Set(M::Skate, SKATE_SETS[sub as usize - 3])),
            (FOLLOW, 0) => self.load(Some(M::Follow), out),
            (FOLLOW, 1 | 3) => out.push(Set(M::Follow, 1)),
            (FOLLOW, 2) => out.extend([Set(M::Follow, 0), Fe(65)]),
            (FOLLOW, 4) => {
                self.won = true;
                out.extend([Set(M::Follow, 2), Post(Event::Indicator(false))]);
            }
            (FOLLOW, 5) => {
                self.won = false;
                out.extend([Set(M::Follow, 3), Post(Event::Indicator(false))]);
            }
            (FOLLOW, 6) => out.extend([Set(M::Follow, 0), Fe(23)]),
            (FOLLOW, 7) => out.extend([Set(M::Follow, 4), Minimap(minimap_shown(true, c)), Post(Event::Indicator(true))]),
            // RETAIL Enter 0x8236c898 (jump table 0x8236c8f0). UNTRACED, nothing: 7 TrickCallout (0x824263f0; no
            // called tricks in the data); 4 / 5 / 8 / 11 / 12 have no arm.
            (BEST_TRICK, 0 | 1 | 3 | 9 | 10) => out.push(Set(M::BestTrick, sub)),
            (BEST_TRICK, 2) => {
                let (round, rounds, first) = c.best;
                out.push(Set(M::BestTrick, if round != rounds { 20 } else if first { 22 } else { 23 }));
            }
            (BEST_TRICK, 6) => {
                out.push(Fe(65));
                self.subs.insert(BEST_TRICK, NO_SUB);
            }
            (BEST_TRICK, 13) => self.load(Some(M::BestTrick), out),
            (BEST_TRICK, 14) => out.push(Set(M::BestTrick, 21)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Act::*, *};

    /// User 10-15 (Community Spirit), RETAIL 0x82169b50: Load loads the overlay, the sign-up pushes FE 65, the round
    /// leaderboard shows 20 then FE 49 after 2.7 s, the last round's shows the result.
    #[test]
    fn the_best_trick_state_runs_its_rounds() {
        let mut m = Manager::default();
        let mut c = on();
        m.change(BEST_TRICK, 13);
        assert_eq!(m.tick(&c), [Minimap(false), Overlay(Some(Movie::BestTrick))]);
        assert_eq!(m.kind, 8);
        m.change(BEST_TRICK, 6);
        assert_eq!(m.tick(&c), [Set(Movie::BestTrick, 21), Fe(65)]);
        c.best = (1, 3, false);
        m.change(BEST_TRICK, 2);
        assert_eq!(m.tick(&c), [Set(Movie::BestTrick, 20)]);
        let acts: Vec<Act> = (0..170).flat_map(|_| m.tick(&c)).collect();
        assert_eq!(acts, [Fe(49), Set(Movie::BestTrick, 3)], "once, at 2.7 s");
        c.best = (3, 3, true);
        m.change(BEST_TRICK, 2);
        assert_eq!(m.tick(&c), [Set(Movie::BestTrick, 22)]);
        m.change(FREE_SKATE, 0);
        assert_eq!(m.tick(&c), [Set(Movie::BestTrick, 3), Minimap(true), Trick(true)]);
    }

    /// he_u1_film01 done and spawn rows: the gate passes, so every Trick / Minimap act shows what was asked.
    pub(super) fn on() -> Ctx<'static> {
        Ctx { key: "he_u1_tutorial01", film01_done: true, spawn_rows: true, ..Ctx::default() }
    }

    #[test]
    fn changes_wait_for_the_tick_and_apply_in_order() {
        let mut m = Manager::default();
        m.change(TUTORIAL, 0);
        m.change(TUTORIAL, 2);
        assert_eq!(m.state, None, "queued, not applied");
        // Activate (Minimap off, type 0), Enter 0 (load the overlay), then the same state: Enter 2 alone.
        assert_eq!(m.tick(&on()), [Minimap(false), Overlay(Some(Movie::Tutorial)), Trick(true), Set(Movie::Tutorial, 1)]);
        assert_eq!((m.state, m.sub(), m.kind), (Some(TUTORIAL), 2, 0));
    }

    #[test]
    fn a_new_state_deactivates_the_old_one_first() {
        let mut m = Manager::default();
        m.change(TUTORIAL, 4);
        m.tick(&on());
        m.change(FOLLOW, 7);
        assert_eq!(
            m.tick(&on()),
            [
                // Tutorial's Deactivate
                Overlay(None), Trick(true), Minimap(true), Post(Event::Indicator(false)),
                // FollowMe's Activate (vt 0x82169af0), then Enter 7 Countdown
                Minimap(true),
                Set(Movie::Follow, 4), Minimap(true), Post(Event::Indicator(true)),
            ]
        );
        assert_eq!(m.kind, 23);
    }

    #[test]
    fn film_and_photo_run_the_old_substates_exit_first() {
        let mut m = Manager::default();
        m.change(PHOTO, 7);
        assert_eq!(m.tick(&on()), [Set(Movie::Photo, 6), Minimap(false), Trick(false)]);
        m.change(PHOTO, 2);
        // Exit 7 (Photo 0, Minimap and Trick back), then Enter 2.
        assert_eq!(m.tick(&on()), [Set(Movie::Photo, 0), Minimap(true), Trick(true), Set(Movie::Photo, 1)]);
    }

    #[test]
    fn free_skate_guards_enter_and_objective_marks() {
        let mut m = Manager::default();
        m.change(FREE_SKATE, 3);
        assert_eq!(m.tick(&on()), [Trick(true)], "Activate only: type 32 skips Enter");
        assert_eq!(m.kind, 32);
        assert!(m.mark_objective(0).is_empty());
    }

    #[test]
    fn trick_and_minimap_gates() {
        let c = |film01_done, spawn_rows, key| Ctx { key, film01_done, spawn_rows, ..Ctx::default() };
        assert!(!trick_shown(true, 32, &c(false, true, "x")), "hidden until he_u1_film01");
        assert!(trick_shown(true, 32, &c(true, false, "x")));
        assert!(!trick_shown(false, 32, &c(true, true, "x")), "off always hides");
        assert!(!trick_shown(true, 4, &c(true, true, "x")), "never in S.K.A.T.E.");
        assert!(trick_shown(true, 0, &c(true, true, "he_u1_tutorial01")));
        assert!(!trick_shown(true, 0, &c(true, false, "he_u1_tutorial01")), "a tutorial without spawn rows");
        assert!(!trick_shown(true, 0, &c(true, true, "fp_u3_tutorial03")));
        assert!(minimap_shown(true, &c(true, false, "x")));
        assert!(!minimap_shown(true, &c(false, true, "x")) && !minimap_shown(false, &c(true, true, "x")));
    }

    /// Tutorial Enter (RETAIL table, ledger): a fresh manager in Tutorial sub 4 (Enter does nothing), then `sub`.
    fn tutorial(sub: i64, key: &str) -> (Vec<Act>, Manager) {
        let mut m = Manager::default();
        m.change(TUTORIAL, 4);
        m.tick(&on());
        m.change(TUTORIAL, sub);
        let acts = m.tick(&Ctx { key, ..on() });
        (acts, m)
    }

    #[test]
    fn tutorial_enters_follow_the_retail_table() {
        use Movie::Tutorial as T;
        let table: Vec<(i64, Vec<Act>)> = vec![
            (0, vec![Overlay(Some(T))]),
            (1, vec![Fe(65)]),
            (2, vec![Trick(true), Set(T, 1)]),
            (3, vec![Set(T, 3), Set(T, 7), Set(T, 9)]),
            (4, vec![]),
            (5, vec![Set(T, 2)]), (6, vec![Set(T, 3)]), (7, vec![Set(T, 4)]),
            (8, vec![Set(T, 5)]), (9, vec![Set(T, 6)]), (10, vec![Set(T, 7)]),
            (11, vec![Set(T, 13)]), (12, vec![Set(T, 14)]), (13, vec![Set(T, 15)]), (14, vec![Set(T, 8)]),
            (15, vec![Trick(true), Set(T, 11)]), (16, vec![Trick(false), Set(T, 12)]),
            (17, vec![Fe(73)]), (18, vec![Set(T, 16)]), (19, vec![Set(T, 17)]), (20, vec![Set(T, 18)]),
            (21, vec![Minimap(true), Post(Event::Indicator(true))]),
            (22, vec![Post(Event::Indicator(false))]),
            (23, vec![Fe(23)]),
            (24, vec![Set(T, 19)]),
            (25, vec![]),
        ];
        for (sub, acts) in table {
            assert_eq!(tutorial(sub, "he_u1_tutorial01").0, acts, "sub {sub}");
        }
        assert_eq!(tutorial(11, "fp_u2_tutorial05").0, [Set(T, 15)], "fp_u2_tutorial05's win shows TUTORIAL_COMPLETE");
        assert_eq!(tutorial(1, "x").1.sub(), 26, "the poster parks the state at 26");
        assert!(tutorial(11, "x").1.won && tutorial(13, "x").1.won);
        assert!(!tutorial(12, "x").1.won && !tutorial(23, "x").1.won);
    }

    #[test]
    fn tutorial_timers() {
        let c = on();
        // 11 / 13 (won): 2.7 s on, the lesson end (17 -> FE 73). 169 ticks of 16 ms are the first >= 2700.
        let (_, mut m) = tutorial(11, "x");
        assert!(!(0..168).any(|_| m.tick(&c).contains(&Fe(73))));
        assert!(m.tick(&c).contains(&Fe(73)));
        assert_eq!(m.sub(), 17);
        // 12 (lost) at 750 ms and 18 at 1000 ms: hudcomplete once, then the state parks at 26.
        for (sub, ticks) in [(12, 47), (18, 63)] {
            let (_, mut m) = tutorial(sub, "x");
            let done = |acts: Vec<Act>| acts.contains(&Post(Event::Complete));
            assert!(!(0..ticks - 1).any(|_| done(m.tick(&c))), "sub {sub} early");
            assert!(done(m.tick(&c)), "sub {sub}");
            assert_eq!(m.sub(), 26);
            assert!(!(0..200).any(|_| done(m.tick(&c))), "sub {sub} once");
        }
    }

    #[test]
    fn objective_marks_post_outside_free_skate() {
        let (_, mut m) = tutorial(4, "x");
        assert_eq!(m.mark_objective(2), [Post(Event::ObjectiveComplete(2))]);
        // Preview 3 bug 2: the mark queues MarkObjectivesComplete (SetState 16), whose timer answers hudcomplete.
        assert_eq!(m.tick(&on()), [Set(Movie::Tutorial, 16)]);
        assert_eq!(m.sub(), 18);
    }

    /// A fresh manager in `state` at `base` (whose Enter / Exit add nothing to the next tick), then `sub`.
    fn go(state: u8, base: i64, sub: i64, c: Ctx) -> (Vec<Act>, Manager) {
        let mut m = Manager::default();
        m.change(state, base);
        m.tick(&c);
        if state == FILM {
            m.loaded(Movie::Film, &c);
        }
        m.change(state, sub);
        let acts = m.tick(&c);
        (acts, m)
    }

    #[test]
    fn film_photo_skate_follow_tables() {
        use Movie::{Action as A, Film as F, Follow as W, Photo as P, Skate as S};
        let c = on();
        for (sub, acts) in [
            (2, vec![Set(F, 2)]), (3, vec![Set(F, 5)]), (4, vec![Set(F, 4), Set(A, 0)]),
            (5, vec![Set(A, 0), Set(F, 3), Trick(false), Minimap(false)]),
            (6, vec![Set(A, 0), Set(F, 0), Fe(23)]), (7, vec![Set(F, 0)]),
            (8, vec![Minimap(true), Set(F, 1), Set(A, 0), Set(F, 6)]), (9, vec![Set(F, 7)]), (10, vec![Set(F, 8)]),
        ] {
            assert_eq!(go(FILM, 0, sub, c).0, acts, "film {sub}");
        }
        assert_eq!(go(FILM, 0, 6, c).1.sub(), 2);
        for (sub, acts) in [
            (0, vec![Overlay(Some(P))]), (1, vec![Fe(65)]), (2, vec![Set(P, 1)]), (3, vec![Set(P, 2)]), (4, vec![Set(P, 4)]),
            (5, vec![Set(P, 3)]), (6, vec![Set(P, 5)]), (7, vec![Set(P, 6), Minimap(false), Trick(false)]),
            (8, vec![Fe(23)]), (9, vec![Set(P, 0)]), (10, vec![Fe(24)]),
        ] {
            assert_eq!(go(PHOTO, 9, sub, c).0, acts, "photo {sub}");
        }
        assert_eq!((go(PHOTO, 9, 1, c).1.sub(), go(PHOTO, 9, 8, c).1.sub()), (12, 2));
        let skate = |sub, kind3, skate_won| go(SKATE, 26, sub, Ctx { kind3, skate_won, ..c }).0;
        for (sub, set) in [(3, 1), (4, 2), (5, 17), (6, 3), (7, 4), (8, 18), (9, 5), (13, 6), (14, 8), (15, 9), (16, 10),
                           (17, 11), (18, 12), (19, 13), (20, 14), (21, 15), (22, 16), (23, 24), (24, 25)] {
            assert_eq!(skate(sub, false, false), [Set(S, set)], "skate {sub}");
        }
        assert_eq!(skate(0, false, false), [Overlay(Some(S))]);
        assert_eq!(skate(1, false, false), [Set(S, 7), Set(S, 32)]);
        assert_eq!(skate(2, false, false), [Fe(65)]);
        assert_eq!(skate(10, false, true), [Set(S, 0), Fe(23)]);
        assert!(go(SKATE, 26, 10, Ctx { skate_won: true, ..c }).1.won);
        assert_eq!((skate(11, false, true), skate(11, false, false)), (vec![Set(S, 22)], vec![Set(S, 23)]));
        assert_eq!(skate(12, false, false), [Fe(74)]);
        assert_eq!((skate(14, true, false), skate(16, true, false), skate(22, true, false)),
                   (vec![Set(S, 31)], vec![Set(S, 31)], vec![Set(S, 30)]));
        for (sub, acts) in [
            (0, vec![Overlay(Some(W))]), (1, vec![Set(W, 1)]), (2, vec![Set(W, 0), Fe(65)]), (3, vec![Set(W, 1)]),
            (4, vec![Set(W, 2), Post(Event::Indicator(false))]), (5, vec![Set(W, 3), Post(Event::Indicator(false))]),
            (6, vec![Set(W, 0), Fe(23)]), (7, vec![Set(W, 4), Minimap(true), Post(Event::Indicator(true))]),
        ] {
            assert_eq!(go(FOLLOW, 26, sub, c).0, acts, "follow {sub}");
        }
    }

    #[test]
    fn select_film_loads_the_overlay_first_and_comes_back() {
        let c = Ctx { films: 3, ..on() };
        let mut m = Manager::default();
        m.change(FILM, 1);
        // Activate, Enter 1 finds no overlay: reload flag, SetSubState(0) (Exit 1: Action 0; Enter 0: load).
        assert_eq!(m.tick(&c), [Minimap(true), Set(Movie::Action, 0), Overlay(Some(Movie::Film))]);
        assert_eq!(m.loaded(Movie::Film, &c), [Set(Movie::Action, 3), FilmMenu(0)]);
        assert!(m.loaded(Movie::Film, &c).is_empty(), "the flag is spent");
    }

    #[test]
    fn film_menu_input() {
        let c = Ctx { films: 3, ..on() };
        let mut m = Manager::default();
        assert_eq!(m.press(Pad::Rb, &c).0, false, "no HUD state yet");
        m.change(FREE_SKATE, -1);
        m.tick(&c);
        assert_eq!(m.press(Pad::Rb, &Ctx { films: 0, ..c }), (false, vec![]), "no film missions: RB isn't taken");
        assert_eq!(m.press(Pad::Rb, &c), (true, vec![]), "free skate RB queues Film 1");
        m.tick(&c);
        m.loaded(Movie::Film, &c);
        assert_eq!(m.press(Pad::Rb, &c), (true, vec![FilmMenu(1)]));
        m.press(Pad::Rb, &c);
        assert_eq!(m.press(Pad::Rb, &c), (true, vec![FilmMenu(0)]), "wraps");
        assert_eq!(m.film_menu(), Some((0, 3)));
        assert_eq!(m.press(Pad::Y, &c), (true, vec![Post(Event::StartFilm(0))]));
        assert_eq!(m.press(Pad::A, &c).0, false);
    }

    #[test]
    fn film_menu_times_out_unless_cycled() {
        let c = Ctx { films: 2, ..on() };
        let (_, mut m) = go(FILM, 0, 1, c);
        for _ in 0..300 {
            m.tick(&c);
        }
        m.press(Pad::Rb, &c); // RB restarts the 5 s
        for _ in 0..312 {
            m.tick(&c);
        }
        assert_eq!(m.state, Some(FILM));
        m.tick(&c); // 313 x 16 = 5008 >= 5000: queue (FreeSkate, -1)
        assert_eq!(m.state, Some(FREE_SKATE));
    }

    /// Bug 5 (preview 3): the film replay's end (cMsgFilmReplayAutoplayFinished, RETAIL 0x82466800) opens Options:
    /// Exit 5 (trick display back), then Enter 6 (FE 23). Outside a film (type != 7) it does nothing.
    #[test]
    fn film_replay_end_opens_the_options() {
        let c = on();
        let (_, mut m) = go(FILM, 0, 5, c);
        m.replay_finished();
        assert_eq!(m.tick(&c), [Trick(true), Set(Movie::Action, 0), Set(Movie::Film, 0), Fe(23)]);
        let (_, mut m) = go(PHOTO, 9, 7, c);
        m.replay_finished();
        assert!(m.tick(&c).is_empty(), "photo (type 6): no Options");
    }

    #[test]
    fn film_fail_and_photo_retry_timers() {
        let c = on();
        let (_, mut m) = go(FILM, 0, 4, c);
        assert!(!(0..168).any(|_| m.tick(&c).contains(&Fe(23))));
        assert_eq!(m.tick(&c), [Set(Movie::Action, 0), Set(Movie::Film, 0), Fe(23)], "2.7 s after Failed: Options");
        let (_, mut m) = go(PHOTO, 9, 5, c);
        assert!(!(0..249).any(|_| !m.tick(&c).is_empty()));
        assert_eq!(m.tick(&c), [Set(Movie::Photo, 1)], "4 s after Failed: Active again");
    }
}
