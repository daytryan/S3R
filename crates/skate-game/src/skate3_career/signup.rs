//! The sign-up manager (spec 6.2, career_flow Q6/Q8, gates.md): boxes round the active rows, the D-pad Up prompt,
//! the picker, the poster, and where a launch starts.

use bevy::math::Vec3;

use super::data::{CareerData, Row};
use super::rules::Rules;

/// Retail's ChallengeType values the sign-up code tests.
pub const OWN_THE_SPOT: u32 = 9;
pub const OWN_THE_LOT: u32 = 0x34;
pub const CREATE_A_SPOT: u32 = 0x23;
pub const ANYWHERE_PHOTO: u32 = 0x25;
pub const ANYWHERE_FILM: u32 = 0x27;
/// `Global` 3: a sign-up row (2 is a teleport, which runs its own graph, 12.5).
pub const SIGNUP: u32 = 3;
/// The picker shows at most this many rows.
pub const PICKER_MAX: usize = 4;
pub const PICKER_TITLE: &str = "ID_SIGNUP_MULTIPLIESIGNUP_SIGNUPHUD";
pub const PICKER_TEXT: &str = "ID_SIGNUP_MULTIPLIESIGNUP";

/// Field 8022BA64CB8E431D; 0 or less means 40.0 (0x821EB794).
#[must_use]
pub fn half_size(row: &Row) -> f32 {
    if row.signup_half <= 0.0 { 40.0 } else { row.signup_half }
}

/// A sign-up box: three equal extents round the row's Location (0x82BD2858; the equal extents are INFERRED, spec 6.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Box3 {
    pub row: String,
    pub min: Vec3,
    pub max: Vec3,
}

impl Box3 {
    #[must_use]
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
}

/// The locator's translation (row 3 of the export's matrices).
fn at(d: &CareerData, locator: &str) -> Option<Vec3> {
    d.locators.get(locator).map(|l| Vec3::from_array([l.matrix[3][0], l.matrix[3][1], l.matrix[3][2]]))
}

/// Every active sign-up row (Global 3) of `world` with a Location locator, except the hidden ones (0x82D529A8).
#[must_use]
pub fn boxes(rules: &Rules, d: &CareerData, world: &str, hidden: &dyn Fn(&str) -> bool) -> Vec<Box3> {
    rules
        .active
        .iter()
        .filter(|name| !hidden(name))
        .filter_map(|name| {
            let row = d.row(name).filter(|r| r.global == SIGNUP && r.world == world)?;
            let centre = at(d, &row.location)?;
            let half = Vec3::splat(half_size(row));
            Some(Box3 { row: name.clone(), min: centre - half, max: centre + half })
        })
        .collect()
}

/// The rows whose box holds `p`, in box order.
#[must_use]
pub fn in_range(boxes: &[Box3], p: Vec3) -> Vec<String> {
    boxes.iter().filter(|b| b.contains(p)).map(|b| b.row.clone()).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    None,
    /// The D-pad Up prompt for these rows.
    Ready(Vec<String>),
    /// In range, but the system is busy (a sequence, popup 306, a challenge).
    Waiting,
}

#[must_use]
pub fn prompt(in_range: &[String], ready: bool) -> Prompt {
    match (in_range.is_empty(), ready) {
        (true, _) => Prompt::None,
        (false, true) => Prompt::Ready(in_range.to_vec()),
        (false, false) => Prompt::Waiting,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    Begin(String),
    /// "Multiple Challenges" / "Please select a challenge:" with up to four rows.
    Picker(Vec<String>),
}

/// D-pad Up in the 'signup' input context (0x82D52F58). None with nothing in range.
#[must_use]
pub fn press(in_range: &[String]) -> Option<Press> {
    match in_range {
        [] => None,
        [one] => Some(Press::Begin(one.clone())),
        many => Some(Press::Picker(many.iter().take(PICKER_MAX).cloned().collect())),
    }
}

/// Every sign-up opens 23 SignUp_Poster except Own the Spot and Own the Lot (spec 6.2, correcting gates.md Q3).
#[must_use]
pub fn needs_poster(kind: u32) -> bool {
    !matches!(kind, OWN_THE_SPOT | OWN_THE_LOT)
}

/// The TeleportManager listener (0x82D55BF0) skips the sign-up teleport for these types. Its other two skips (field
/// 4263ED158A50063E and manager +0x3C / +0x1F4) are not exported or traced (gates.md Q3), so they never skip here.
#[must_use]
pub fn exempt_from_teleport(kind: u32) -> bool {
    matches!(kind, OWN_THE_LOT | OWN_THE_SPOT | CREATE_A_SPOT | ANYWHERE_PHOTO | ANYWHERE_FILM)
}

/// Where a row starts: its world's map when that is not the current one, and its MapStartLocation, else its Location
/// (0x82D54C90).
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub row: String,
    pub map: Option<String>,
    pub at: [[f32; 4]; 4],
}

/// None when the row or its start locator is not in the export.
#[must_use]
pub fn launch(d: &CareerData, row: &str, current_map: &str) -> Option<Launch> {
    let r = d.row(row)?;
    let locator = r.map_start.iter().chain(std::iter::once(&r.location)).find_map(|l| d.locators.get(l))?;
    let file = |p: &str| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().to_ascii_lowercase());
    let map = d.worlds.get(&r.world).cloned().flatten().filter(|m| file(m) != file(current_map));
    Some(Launch { row: row.to_owned(), map, at: locator.matrix })
}

/// The pending auto sign-up's state while its poster is up (spec 6.2): decline clears it for good; a mode exit drops
/// it without the decline's sound or anything on the rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Poster {
    pub row: Option<String>,
}

impl Poster {
    pub fn open(&mut self, row: &str) {
        self.row = Some(row.to_owned());
    }
    /// A: the row to start.
    pub fn accept(&mut self) -> Option<String> {
        self.row.take()
    }
    /// B (`challenge_decline`): the row stays active, so its marker stays.
    pub fn decline(&mut self) {
        self.row = None;
    }
    /// A mode exit (0x82D527F0): closed, nothing else.
    pub fn mode_exit(&mut self) {
        self.row = None;
    }
}

#[cfg(test)]
mod tests {
    use super::super::data::Locator;
    use super::*;

    fn row(world: &str, location: &str, global: u32, kind: u32, half: f32) -> Row {
        Row { world: world.into(), location: location.into(), global, kind, signup_half: half, ..Row::default() }
    }
    fn locator(map: &str, x: f32, y: f32, z: f32) -> Locator {
        let mut matrix = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        matrix[3] = [x, y, z, 1.0];
        Locator { map: map.into(), matrix, slots: Vec::new() }
    }
    fn data() -> CareerData {
        let mut d = CareerData::default();
        d.worlds.insert("dist_university".into(), Some("maps/University.skate".into()));
        d.worlds.insert("dist_skateschool".into(), Some("maps/SkateSchool.skate".into()));
        d.rows.insert("ftl_univ_02".into(), row("dist_university", "l_ftl", SIGNUP, 23, 3.0));
        d.rows.insert("film_univ_01".into(), row("dist_university", "l_film", SIGNUP, 2, 0.0));
        d.rows.insert("tele_world_to_school".into(), row("dist_university", "l_tele", 2, 0, 3.5));
        d.rows.insert("tut_sksc_01".into(), row("dist_skateschool", "l_tut", SIGNUP, 0, 4.0));
        d.locators.insert("l_ftl".into(), locator("maps/University.skate", 10.0, 0.0, 10.0));
        d.locators.insert("l_film".into(), locator("maps/University.skate", 12.0, 0.0, 10.0));
        d.locators.insert("l_tele".into(), locator("maps/University.skate", 0.0, 0.0, 0.0));
        d.locators.insert("l_tut".into(), locator("maps/SkateSchool.skate", 5.0, 1.0, 5.0));
        d
    }
    fn rules(active: &[&str]) -> Rules {
        Rules { active: active.iter().map(|s| (*s).to_owned()).collect(), ..Rules::default() }
    }

    #[test]
    fn the_box_follows_the_field() {
        for (field, half) in [(3.0, 3.0), (10.0, 10.0), (4.0, 4.0), (0.0, 40.0), (-1.0, 40.0)] {
            assert_eq!(half_size(&row("w", "l", SIGNUP, 0, field)), half, "{field}");
        }
    }

    #[test]
    fn teleports_get_no_box() {
        let d = data();
        let b = boxes(&rules(&["ftl_univ_02", "tele_world_to_school", "tut_sksc_01"]), &d, "dist_university", &|_| false);
        assert_eq!(b.iter().map(|b| b.row.as_str()).collect::<Vec<_>>(), ["ftl_univ_02"]);
        assert_eq!(b[0].min, Vec3::new(7.0, -3.0, 7.0));
        assert_eq!(b[0].max, Vec3::new(13.0, 3.0, 13.0));
        let hidden = boxes(&rules(&["ftl_univ_02"]), &d, "dist_university", &|r| r == "ftl_univ_02");
        assert!(hidden.is_empty());
    }

    #[test]
    fn one_row_in_range_begins_at_once_and_two_open_the_picker() {
        let d = data();
        let b = boxes(&rules(&["ftl_univ_02", "film_univ_01"]), &d, "dist_university", &|_| false);
        let one = in_range(&b, Vec3::new(8.0, 0.0, 10.0));
        assert_eq!(one, ["ftl_univ_02", "film_univ_01"], "film_univ_01 has the 40.0 box");
        let far = in_range(&b, Vec3::new(40.0, 0.0, 10.0));
        assert_eq!(press(&far), Some(Press::Begin("film_univ_01".into())));
        assert_eq!(press(&one), Some(Press::Picker(one.clone())));
        let five: Vec<String> = (0..5).map(|i| format!("r{i}")).collect();
        assert_eq!(press(&five), Some(Press::Picker(five[..4].to_vec())));
        assert_eq!(press(&[]), None);
        assert_eq!(prompt(&one, true), Prompt::Ready(one.clone()));
        assert_eq!(prompt(&one, false), Prompt::Waiting);
        assert_eq!(prompt(&[], true), Prompt::None);
    }

    #[test]
    fn an_ots_row_skips_the_poster() {
        assert!(!needs_poster(OWN_THE_SPOT));
        assert!(!needs_poster(OWN_THE_LOT));
        assert!(needs_poster(23), "Follow the Leader");
        assert!(needs_poster(0), "a tutorial");
        assert!(exempt_from_teleport(ANYWHERE_FILM));
        assert!(!exempt_from_teleport(23));
    }

    #[test]
    fn decline_clears_it_and_nothing_reissues_it() {
        let mut p = Poster::default();
        p.open("ftl_univ_02");
        p.decline();
        assert_eq!(p.accept(), None);
        p.open("ftl_univ_02");
        assert_eq!(p.accept().as_deref(), Some("ftl_univ_02"));
        assert_eq!(p.accept(), None);
    }

    #[test]
    fn a_pause_mid_popup_drops_the_poster_and_keeps_the_marker() {
        let d = data();
        let r = rules(&["ftl_univ_02"]);
        let mut p = Poster::default();
        p.open("ftl_univ_02");
        p.mode_exit();
        assert_eq!(p.row, None);
        assert_eq!(boxes(&r, &d, "dist_university", &|_| false).len(), 1, "the row stays active");
    }

    #[test]
    fn the_launch_loads_another_map_and_places_at_the_start() {
        let d = data();
        let l = launch(&d, "tut_sksc_01", "C:/x/maps/University.skate").unwrap();
        assert_eq!(l.map.as_deref(), Some("maps/SkateSchool.skate"));
        assert_eq!(l.at[3], [5.0, 1.0, 5.0, 1.0]);
        let here = launch(&d, "ftl_univ_02", "C:/x/maps/University.skate").unwrap();
        assert_eq!(here.map, None);
        assert_eq!(launch(&d, "nope", "x"), None);
    }
}
