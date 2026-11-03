//! Retail Skate 3's VisualIndicators for challenge objectives (plan Task 24, spec 18.17, stage1_traces 3): each
//! objective's items from `career.json` `indicators`. An objective in status 2 Enabled or 4 Active adds its items
//! (SetStatus 0x82c73fc8 -> 0x82d4f720), any other status removes them (0x82d4f7e8). Kind 0 draws the indicator type
//! (`left_chevron`, `arrow_nonfade`, ...) in its style at its locator (0x82d4f328), as the Death Race draws its
//! chevrons; kind 1 is a ribbon along the spline's points (0xEB0021), flags 2 open, 8 closed; kind >= 2 draws nothing
//! (0x82d4f72c-0x82d4f740). This is the pure model: [`Indicators::visible`] is what the Bevy glue draws. Not tested in
//! play.
use crate::skate3_career::data::{Indicator, Locator};
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// The objective statuses whose indicators show (SetStatus 0x82c73fc8: Enabled / Active add them at 0x82d4f720).
pub const ENABLED: u8 = 2;
pub const ACTIVE: u8 = 4;
/// A spline flag: the ribbon closes its loop (gate_end_vi_01). INFERRED (stage1_traces 3.3: flags 2 on every open gate
/// frame, 8 on the one loop whose ends meet).
const CLOSED: u32 = 8;

/// One drawn item of a shown objective.
#[derive(Debug, PartialEq)]
pub enum Shown<'a> {
    /// Kind 1: [`crate::skate2_arrows::tube`] along these points. The type and style keys are not read for kind 1 (0x82d4f760-0x82d4f7b4);
    /// the look is the race ribbon's (spec 18.17).
    Ribbon { points: &'a [[f32; 3]], closed: bool },
    /// Kind 0: the indicator type's outline (indicators.json `shapes`) in its style, placed as the race places a
    /// chevron (`Spot::Locator`: the locator's matrix as is).
    Locator { shape: &'a str, style: &'a str, at: Transform },
}

/// VisualIndicators: the challenge's group and each objective's items.
#[derive(Default)]
pub struct Indicators {
    /// VisualIndicators.CreateGroup (stage1_school make_hud L21-30). Objectives add items only with an indicator
    /// manager (objective +0xf84, 0x82c6e380), so nothing shows before a group exists.
    group: Option<String>,
    items: BTreeMap<String, Vec<Indicator>>,
    shown: BTreeSet<String>,
}

impl Indicators {
    pub fn create_group(&mut self, name: &str) {
        self.group = Some(name.to_owned());
    }

    /// An objective's VisualIndicators: the items of `items` (a row's list) whose objective it is. A second add
    /// replaces the first (challenge_running re-adds every objective's, stage1_school L50-74).
    pub fn add(&mut self, objective: &str, items: &[Indicator]) {
        let own = items.iter().filter(|i| i.objective == objective).cloned().collect();
        self.items.insert(objective.to_owned(), own);
    }

    /// SetStatus 0x82c73fc8: Enabled / Active show the objective's items (0x82d4f720), any other status hides them
    /// (0x82d4f7e8).
    pub fn show(&mut self, objective: &str, status: u8) {
        if matches!(status, ENABLED | ACTIVE) {
            self.shown.insert(objective.to_owned());
        } else {
            self.shown.remove(objective);
        }
    }

    /// RemoveAllChallengeVisualIndicators: every objective's items; the group stays.
    pub fn remove_all(&mut self) {
        self.items.clear();
        self.shown.clear();
    }

    /// What shows: (objective, item) for each shown objective's items. An item whose locator or spline the data
    /// lacks draws nothing, as in the race.
    pub fn visible<'a>(&'a self, locators: &BTreeMap<String, Locator>) -> Vec<(&'a str, Shown<'a>)> {
        if self.group.is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for objective in &self.shown {
            for item in self.items.get(objective).into_iter().flatten() {
                let shown = match item.kind {
                    0 => item.locator.as_ref().and_then(|l| locators.get(l)).map(|l| Shown::Locator {
                        shape: &item.shape,
                        style: &item.style,
                        at: Transform::from_matrix(Mat4::from_cols_array_2d(&l.matrix)),
                    }),
                    1 if item.spline.len() > 1 => {
                        Some(Shown::Ribbon { points: &item.spline, closed: item.flags.iter().any(|f| f & CLOSED != 0) })
                    }
                    _ => None,
                };
                out.extend(shown.map(|s| (objective.as_str(), s)));
            }
        }
        out
    }
}

/// An online mode's VisualIndicators through skate2_arrows (as skate3_skate_game's show_indicators): `file` the mode's
/// indicators.json (asset path; its looks are "skate3:<type>:<style>", tools/skate3_online.py look_file), `items` each
/// shown item (look, locator matrix). No items give the list back, only when it is this mode's (the career and the Skate
/// 2 modes write it on their own). Writes on a change only: skate2_arrows respawns the markers when the list changes.
pub(crate) fn show(world: &mut World, file: &str, items: Vec<(String, [[f32; 4]; 4])>) {
    show_all(world, file, items, Vec::new());
}

/// [`show`] with the row's kind-1 outlines as ribbons (in a style of `file`): either keeps the file listed.
pub(crate) fn show_all(world: &mut World, file: &str, items: Vec<(String, [[f32; 4]; 4])>, ribbons: Vec<crate::skate2_arrows::Ribbon>) {
    let Some(mut arrows) = world.get_resource_mut::<crate::skate2_arrows::Arrows>() else { return };
    let ours = arrows.more.iter().any(|m| m == file);
    if items.is_empty() && ribbons.is_empty() {
        if ours {
            arrows.more.retain(|m| m != file);
            arrows.wanted.clear();
            arrows.ribbons.clear();
        }
        return;
    }
    if !ours {
        arrows.more.push(file.to_owned());
    }
    if arrows.wanted != items {
        arrows.wanted = items;
    }
    if arrows.ribbons != ribbons {
        arrows.ribbons = ribbons;
    }
}

/// G7: a row's VisualIndicators ([`crate::skate3_race::Chevron`]: tools/skate3_online.py chevrons) as [`show`] items:
/// each placed one in its own look "skate3:<type>:<style>"; one no mission places draws nothing.
pub(crate) fn looks(chevrons: &[crate::skate3_race::Chevron]) -> Vec<(String, [[f32; 4]; 4])> {
    chevrons.iter().filter_map(|c| Some((format!("skate3:{}:{}", c.look.ribbon, c.look.style), c.matrix?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skate2_arrows::tube;
    use bevy::mesh::{Indices, VertexAttributeValues};

    fn item(objective: &str, kind: u32, locator: Option<&str>, spline: Vec<[f32; 3]>, flags: Vec<u32>) -> Indicator {
        Indicator {
            objective: objective.into(),
            kind,
            locator: locator.map(Into::into),
            shape: "left_chevron".into(),
            style: "default".into(),
            spline,
            flags,
        }
    }

    const SQUARE: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]];

    fn positions(mesh: &Mesh) -> Vec<[f32; 3]> {
        match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(VertexAttributeValues::Float32x3(p)) => p.clone(),
            _ => panic!("no positions"),
        }
    }

    #[test]
    fn an_objective_shows_its_ribbons_only_in_state_2_or_4() {
        let items = [item("o_1", 1, None, SQUARE.to_vec(), vec![2]), item("o_2", 1, None, SQUARE.to_vec(), vec![2])];
        let mut ind = Indicators::default();
        ind.add("o_1", &items);
        ind.show("o_1", ACTIVE);
        assert!(ind.visible(&BTreeMap::new()).is_empty(), "no group, no indicator manager");
        ind.create_group("o");
        for status in 0..=6 {
            ind.show("o_1", status);
            let shown = ind.visible(&BTreeMap::new());
            assert_eq!(shown.len(), usize::from(status == 2 || status == 4), "status {status}");
            assert!(shown.iter().all(|(o, _)| *o == "o_1"), "only its own items");
        }
        ind.show("o_1", ENABLED);
        ind.remove_all();
        assert!(ind.visible(&BTreeMap::new()).is_empty());
    }

    #[test]
    fn flag_2_draws_open_and_8_closed() {
        let items = [item("o", 1, None, SQUARE.to_vec(), vec![2]), item("o", 1, None, SQUARE.to_vec(), vec![8])];
        let mut ind = Indicators::default();
        ind.create_group("g");
        ind.add("o", &items);
        ind.show("o", ENABLED);
        let closed: Vec<bool> = (ind.visible(&BTreeMap::new()).into_iter())
            .map(|(_, s)| match s {
                Shown::Ribbon { closed, .. } => closed,
                Shown::Locator { .. } => panic!("a ribbon"),
            })
            .collect();
        assert_eq!(closed, [false, true]);
        let open = tube(&SQUARE, false);
        assert_eq!((open.count_vertices(), open.indices().map_or(0, Indices::len)), (3 * 8, 3 * 18));
        let shut = tube(&SQUARE, true);
        assert_eq!((shut.count_vertices(), shut.indices().map_or(0, Indices::len)), (3 * 10, 3 * 24));
        let p = positions(&shut);
        assert_eq!((p[8], p[9]), (p[0], p[1]), "the loop meets itself");
    }

    /// G3 / G7 (online audit 10-28): a mode's looks go to skate2_arrows with its indicators.json while it shows them and
    /// the list goes back once it shows none; a list another owner wrote (the career's sign-ups) is left alone.
    #[test]
    fn a_mode_shows_its_looks_through_the_arrows_and_gives_them_back() {
        use crate::skate2_arrows::Arrows;
        let at = |x: f32| [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [x, 0., 0., 1.]];
        let mut world = World::new();
        world.insert_resource(Arrows { wanted: vec![("signup:a".into(), at(9.))], more: vec!["signup.json".into()], ..default() });
        show(&mut world, "dom.json", Vec::new());
        assert_eq!(world.resource::<Arrows>().wanted.len(), 1, "the career's list stays");
        let items = vec![("skate3:t:s".to_owned(), at(1.)), ("skate3:t:s".to_owned(), at(2.))];
        show(&mut world, "dom.json", items.clone());
        let arrows = world.resource::<Arrows>();
        assert_eq!((&arrows.wanted, arrows.more.iter().any(|m| m == "dom.json")), (&items, true));
        show(&mut world, "dom.json", Vec::new());
        let arrows = world.resource::<Arrows>();
        assert!(arrows.wanted.is_empty() && !arrows.more.iter().any(|m| m == "dom.json"), "given back");
        assert!(arrows.more.iter().any(|m| m == "signup.json"), "another file stays listed");
    }

    /// ots_indu_01_ol (Spot Battle online): its only VisualIndicator is the spot's outline, so the outline alone keeps
    /// the mode's looks file (its style) listed while it shows, and both go back together.
    #[test]
    fn an_outline_alone_keeps_the_looks_and_goes_back_with_them() {
        use crate::skate2_arrows::{Arrows, Ribbon};
        let mut world = World::new();
        world.init_resource::<Arrows>();
        let ribbon = Ribbon { glow: "o.png".into(), darken: "b.png".into(), points: vec![[0., 0., 0.], [1., 0., 0.], [1., 0., 1.]], closed: true };
        show_all(&mut world, "sb.json", Vec::new(), vec![ribbon.clone()]);
        let arrows = world.resource::<Arrows>();
        assert_eq!((arrows.ribbons.as_slice(), arrows.more.iter().any(|m| m == "sb.json")), (&[ribbon][..], true));
        show_all(&mut world, "sb.json", Vec::new(), Vec::new());
        let arrows = world.resource::<Arrows>();
        assert!(arrows.ribbons.is_empty() && !arrows.more.iter().any(|m| m == "sb.json"), "given back");
    }

    /// G7: each placed chevron is an item in its look; an unplaced one is none.
    #[test]
    fn a_rows_chevrons_are_items_in_their_look() {
        use crate::skate3_race::{Chevron, Look};
        let at = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [3., 4., 5., 1.]];
        let look = |ribbon: &str, style: &str| Look { ribbon: ribbon.into(), style: style.into() };
        let chevrons = [Chevron { matrix: Some(at), look: look("B094", "D7ED") }, Chevron { matrix: None, look: look("B094", "AAAA") }];
        assert_eq!(looks(&chevrons), vec![("skate3:B094:D7ED".to_owned(), at)]);
    }

    #[test]
    fn a_kind_0_item_sits_at_its_locator() {
        let mut matrix = Mat4::from_rotation_y(1.).to_cols_array_2d();
        matrix[3] = [5., 6., 7., 1.];
        let locators = BTreeMap::from([("vi_01".to_owned(), Locator { map: "m".into(), matrix, slots: vec![] })]);
        let items = [item("o", 0, Some("vi_01"), vec![], vec![]), item("o", 0, Some("missing"), vec![], vec![])];
        let mut ind = Indicators::default();
        ind.create_group("g");
        ind.add("o", &items);
        ind.show("o", ACTIVE);
        let shown = ind.visible(&locators);
        assert_eq!(shown.len(), 1, "a missing locator draws nothing");
        let Shown::Locator { shape, style, at } = &shown[0].1 else { panic!("a locator indicator") };
        assert_eq!((*shape, *style), ("left_chevron", "default"));
        assert!(at.translation.distance(Vec3::new(5., 6., 7.)) < 1e-5);
        assert!(at.rotation.angle_between(Quat::from_rotation_y(1.)) < 1e-4);
    }

    #[test]
    #[ignore = "requires the skate3-career export (SKATE3_CAREER_DIR)"]
    fn tut_sksc_01_builds_23_ribbons() {
        let dir = std::path::PathBuf::from(std::env::var_os("SKATE3_CAREER_DIR").expect("set SKATE3_CAREER_DIR"));
        let d = crate::skate3_career::load(&dir).unwrap_or_else(|why| panic!("{why}"));
        let items = &d.indicators["tut_sksc_01"];
        let mut ind = Indicators::default();
        ind.create_group("tut_sksc_01");
        for objective in items.iter().map(|i| i.objective.as_str()).collect::<BTreeSet<_>>() {
            ind.add(objective, items);
            ind.show(objective, ACTIVE);
        }
        let shown = ind.visible(&d.locators);
        let ribbons: Vec<(&[[f32; 3]], bool)> = (shown.iter())
            .filter_map(|(_, s)| match s {
                Shown::Ribbon { points, closed } => Some((*points, *closed)),
                Shown::Locator { .. } => None,
            })
            .collect();
        assert_eq!(ribbons.len(), 23);
        assert_eq!(ribbons.iter().filter(|r| r.1).count(), 1, "gate_end_vi_01 closes");
        for (points, closed) in ribbons {
            assert!(tube(points, closed).indices().is_some_and(|i| i.len() >= 6), "{} points", points.len());
        }
        // tut_sksc_01_0's six left_chevrons at tut_sksc_01_vi_01 .. _06.
        assert_eq!(shown.iter().filter(|(_, s)| matches!(s, Shown::Locator { shape: "left_chevron", .. })).count(), 6);
    }
}
