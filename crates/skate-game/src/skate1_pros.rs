//! Skate 1 pro tutorials (P6): the spot gates P-Rod's script sets with Challenge.SetTriggerVisual. Everything else the
//! four pro tutorials need runs through the shared flow (docs/superpowers/plans/2026-10-14-skate1-p6-pros.md).
use crate::skate1_online::{Aabb, GateModel, ROOT as ONLINE_ROOT, gate_look, gate_transform};
use bevy::prelude::*;
use std::collections::BTreeMap;

const GATE_NAME: &str = "Skate 1 pro gate";

/// NIS.GetProInfo (RETAIL 0x8245eaa8), the pro intro's bio card (pro_bioinfo_overlay's GetInfo): [name, hometown,
/// stance] labels of a record in the pro table (104-byte rows built at 0x82327828: +16 ID_CHEAT_PROSKATERnn, +48
/// ID_CHEAT_PRODETAIL_LOCATIONn, +80 goofy). Career branch: the running mission picks the row (fp_u3_tutorial03 7,
/// 04 8, 01 4, 02 1, any other 5). UNTRACED: the online / party branches (mode 28: row 16 or 28; others "#Bad Pro").
pub(crate) fn pro_info(mission: Option<&str>) -> [&'static str; 3] {
    let (name, town, goofy) = match mission {
        Some("fp_u3_tutorial03") => ("ID_CHEAT_PROSKATER19", "ID_CHEAT_PRODETAIL_LOCATION13", false), // 7 danny_way
        Some("fp_u3_tutorial04") => ("ID_CHEAT_PROSKATER11", "ID_CHEAT_PRODETAIL_LOCATION3", true), // 8 chris_haslam
        Some("fp_u3_tutorial01") => ("ID_CHEAT_PROSKATER06", "ID_CHEAT_PRODETAIL_LOCATION5", false), // 4 jason_dill
        Some("fp_u3_tutorial02") => ("ID_CHEAT_PROSKATER17", "ID_CHEAT_PRODETAIL_LOCATION11", true), // 1 paul_rodriguez
        _ => ("ID_CHEAT_PROSKATER03", "ID_CHEAT_PRODETAIL_LOCATION2", true), // 5 mike_carroll
    };
    [name, town, if goofy { "ID_CHEAT_PRODETAIL_GOOFY" } else { "ID_CHEAT_PRODETAIL_REGULAR" }]
}

/// Challenge.SetTriggerVisual's map (RETAIL mgr+11340, mgr = [[0x831ab140]+188]+0x1EAC4): volume -> (eChallengeIndicators
/// kind, the volume's box; None when the tutorial data has no such volume).
#[derive(Resource, Default)]
pub(crate) struct ProGates {
    map: BTreeMap<String, (i64, Option<Aabb>)>,
    shown: Vec<(String, Entity)>,
    /// sg000: None = not looked for yet, Some(None) = no gates.json.
    look: Option<Option<(Handle<Mesh>, Handle<StandardMaterial>, GateModel)>>,
}

/// The VisualDirector pass draws at most four spot gates (RETAIL 0x826f6578: `cmpwi r23,4` before 0x826f6298).
const MAX_SPOT_GATES: usize = 4;

impl ProGates {
    /// Challenge.SetTriggerVisual(volume, kind) (RETAIL 0x827ae8d0): kind 0 -> 0x823df2e0 removes the volume's entry;
    /// any other kind -> 0x823df230 removes it and inserts {volume, kind, priority 0, active 1}.
    pub(crate) fn set(&mut self, volume: String, kind: i64, aabb: Option<Aabb>) {
        if kind == 0 {
            self.map.remove(&volume);
        } else {
            self.map.insert(volume, (kind, aabb));
        }
    }
    /// Challenge.ClearVisualIndicatorsByType(kind): every entry of that kind (RETAIL 0x823df410, list mgr+11344).
    /// ponytail: its twin 0x823df348 (the mission VisualIndicators rows, the lead's `Indicators`) is not touched; no
    /// script in the window clears a kind those rows use (they are kind 4, research 2.0).
    pub(crate) fn clear(&mut self, kind: i64) {
        self.map.retain(|_, (k, _)| *k != kind);
    }
    /// No challenge running. INFERRED: retail's map outlives the challenge, but fp_u3_tutorial02 clears both of its
    /// kinds on every way out (its `final` and `failed` states), so nothing observable differs.
    pub(crate) fn reset(&mut self) {
        self.map.clear();
    }
    /// The spot gates drawn: kinds 8-10 (SpotNextGates, SpotGates, SpotFinish) with a box, at most four (RETAIL
    /// 0x826f6578; 0x826f6298 returns 0 for a null volume and the counter only counts a built gate).
    /// UNTRACED (P6 U2): retail walks its map in its own key order; name order here. Only more than four gates would
    /// tell, and P-Rod shows one at a time.
    /// ponytail: kinds 4-7 and 11-12 are other builders (0x826f6098, 0x826f6120, 0x826f6400) no window script sets.
    fn spot_gates(&self) -> Vec<(&str, Aabb)> {
        self.map
            .iter()
            .filter(|(_, (k, _))| (8..=10).contains(k))
            .filter_map(|(v, (_, b))| Some((v.as_str(), (*b)?)))
            .take(MAX_SPOT_GATES)
            .collect()
    }
}

/// Spawns / despawns the drawn spot gates when the set changes.
/// - Placement INFERRED (P6 U1; RETAIL item = volume matrix + extents + flags, peer): the port fills the volume's box,
///   skate1_online's SpotGate rule (`gate_transform(.., spot)`).
/// - UNTRACED (P6 U1): next vs finish look (0x826f6298 sets bit 29 for kind 8, bit 30 for kind 10; reader untraced).
///   Fallback: both draw alike.
fn draw(world: &mut World) {
    world.resource_scope(|world, mut gates: Mut<ProGates>| {
        let wanted: Vec<(String, Aabb)> = gates.spot_gates().into_iter().map(|(v, b)| (v.to_owned(), b)).collect();
        if gates.shown.iter().map(|(v, _)| v).eq(wanted.iter().map(|(v, _)| v)) {
            return;
        }
        for (_, e) in std::mem::take(&mut gates.shown) {
            world.despawn(e);
        }
        if wanted.is_empty() {
            return;
        }
        if gates.look.is_none() {
            gates.look = Some(load(world));
        }
        let Some(Some((mesh, material, model))) = &gates.look else { return };
        let shown: Vec<(String, Entity)> = wanted
            .into_iter()
            .map(|(v, b)| {
                let at = gate_transform(&b, None, model, true);
                let e = world.spawn((Name::new(GATE_NAME), Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), at));
                (v, e.id())
            })
            .collect();
        gates.shown = shown;
    });
}

/// sg000 from skate1_online's gates.json (old_online1_gates.py).
/// - INFERRED: kinds 8-10 draw the SpotGate; the item 0x826f6298 builds is the visual treatment manager's sg000 pool's
///   (0x826b5c08, model table 0x8217b2c0).
/// - Without gates.json (an install from before the gate models) nothing is drawn, one warning (lead-approved 10-14).
fn load(world: &mut World) -> Option<(Handle<Mesh>, Handle<StandardMaterial>, GateModel)> {
    #[derive(serde::Deserialize)]
    struct Gates {
        models: BTreeMap<String, GateModel>,
    }
    let root = world.resource::<crate::config::Config>().asset_root.join(ONLINE_ROOT);
    let path = root.join("gates/gates.json");
    let gates: Gates = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        .inspect_err(|e| warn!("Skate 1 pro gates: {}: {e}", path.display()))
        .ok()?;
    let model = gates.models.into_iter().find(|(n, _)| n == "sg000")?.1;
    let (mesh, material) = gate_look(world, &root, &model)?;
    Some((mesh, material, model))
}

/// The sign-up indicator (overnight-1015 C3), old_indicator1-style export of visual_treatments' poi00 (staged).
const SIGNUP_MARKER: &str = "private/challenges-skate1/indicator/poi00.glb";

/// The sign-up markers drawn: (challenge, Location) -> entity.
#[derive(Resource, Default)]
struct SignupMarkers {
    shown: Vec<((String, Vec3), Entity)>,
}

/// Where a sign-up's marker stands. RETAIL: the availability rebuild 0x823d4238 asks 0x823def20 for each newly
/// available sign-up's indicator type (1-3) and 0x823defe8 inserts it at the challenge's Location lifted 2.0 m (types
/// 1-3 and 13).
/// UNTRACED: that type check (0x823def20); every kind-3 sign-up gets a marker here (review 10-15 minor 9).
fn marker_at(location: Vec3) -> Transform {
    Transform::from_translation(location + Vec3::Y * 2.)
}

/// poi_shader (VisualDirector type 1-3 builders 0x826f5e40 / 0x826f5ed8 / 0x826f5f78; VT row AB8E69C6F272D89E):
/// - INFERRED (texture by name adjacency, old_indicator1's rule; the glb's texture 1 is that white alpha strip): the
///   shape is ribbonbrushstoke_128x512's alpha (its _d diffuse is plain white), drawn unlit from both sides;
/// - UNTRACED: the colour by indicator type (the instance getters' hashes 0x971D... / 0xEA17... aren't in the row).
///   Fallback: field 0x5149's first float4 (0, 0, 1), the one the flagless default getter 0x826a5f50 reads (type 1);
/// - UNTRACED: the draw-on (UV1.y -37.5..0.5 under mTimer.x 0..1.87), any spin and the row's 40 m near / far.
///   Fallback: drawn whole and static at any distance.
fn marker_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(0., 0., 1.),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    }
}

/// One marker per available sign-up while no challenge runs. UNTRACED: when retail removes them; fallback: hidden
/// while a challenge runs (no sign-up can be offered then, SignUps::update).
fn signup_markers(
    mut commands: Commands,
    server: Res<AssetServer>,
    config: Res<crate::config::Config>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    state: Option<Res<crate::skate1_challenge::Skate1Challenge>>,
    mut markers: ResMut<SignupMarkers>,
) {
    let state = state.filter(|s| s.running().is_none());
    let wanted = || state.iter().flat_map(|s| s.signup_places());
    let mut now = wanted();
    let same = markers.shown.iter().all(|((name, at), _)| now.next().is_some_and(|(n, a)| n == name.as_str() && a == *at));
    if same && now.next().is_none() {
        return;
    }
    for (_, e) in std::mem::take(&mut markers.shown) {
        commands.entity(e).try_despawn();
    }
    if wanted().next().is_none() || !config.asset_root.join(SIGNUP_MARKER).is_file() {
        return;
    }
    let mesh = server.load(GltfAssetLabel::Primitive { mesh: 0, primitive: 0 }.from_asset(SIGNUP_MARKER));
    let mut material = marker_material();
    material.base_color_texture = Some(server.load(GltfAssetLabel::Texture(1).from_asset(SIGNUP_MARKER)));
    let material = materials.add(material);
    markers.shown = wanted()
        .map(|(name, at)| {
            let e = commands.spawn((Name::new("Skate 1 sign-up marker"), Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), marker_at(at)));
            ((name.to_owned(), at), e.id())
        })
        .collect();
}

pub(crate) struct Skate1ProsPlugin;
impl Plugin for Skate1ProsPlugin {
    fn build(&self, app: &mut App) {
        // After skate1_challenge's drive has applied this frame's requests, and in Update: a mesh spawned in PostUpdate
        // after bevy_pbr's check_entities_needing_specialization but before visibility is drawn with no
        // specialization tick, and bevy_pbr's unwrap panics (playtest 10-15: crash on the nollie lesson's first gate).
        app.init_resource::<ProGates>()
            .init_resource::<SignupMarkers>()
            .add_systems(Update, (draw, signup_markers).after(crate::skate1_challenge::drive));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(x: f32) -> Option<Aabb> {
        Some(Aabb { min: [x, 0., 0.], max: [x + 2., 3., 2.] })
    }

    /// RETAIL 0x827ae8d0: the same volume replaces its entry (0x823df230), kind 0 removes it (0x823df2e0);
    /// ClearVisualIndicatorsByType removes every entry of one kind (0x823df410).
    #[test]
    fn trigger_visuals_follow_the_retail_map() {
        let mut g = ProGates::default();
        g.set("gate_1".into(), 8, aabb(0.));
        g.set("gate_1".into(), 10, aabb(0.));
        g.set("gate_2".into(), 8, aabb(5.));
        assert_eq!(g.map.len(), 2);
        assert_eq!(g.map["gate_1"].0, 10, "replaced, not added");
        g.clear(8);
        assert_eq!(g.map.keys().collect::<Vec<_>>(), ["gate_1"]);
        g.set("gate_1".into(), 0, None);
        assert!(g.map.is_empty());
        g.set("gate_3".into(), 9, aabb(1.));
        g.reset();
        assert!(g.map.is_empty());
    }

    /// RETAIL 0x826f6578 (second loop): kinds 8-10 go to the spot gate builder 0x826f6298, which skips a volume it
    /// can't resolve; at most four count.
    #[test]
    fn at_most_four_boxed_spot_gates_draw() {
        let mut g = ProGates::default();
        for (i, kind) in [8, 9, 10, 8, 8, 4, 5].into_iter().enumerate() {
            g.set(format!("v{i}"), kind, aabb(i as f32));
        }
        g.set("a_no_box".into(), 8, None);
        let names: Vec<&str> = g.spot_gates().into_iter().map(|(v, _)| v).collect();
        assert_eq!(names, ["v0", "v1", "v2", "v3"]);
    }

    /// The drawn set follows the map: one sg000 per spot gate, gone when it is cleared.
    #[test]
    fn draw_spawns_and_despawns_with_the_map() {
        let model: GateModel = serde_json::from_value(serde_json::json!({
            "positions": [], "uvs": [], "indices": [], "texture": "",
            "min": [-7.336, 0.03, -7.336], "max": [7.336, 5.31, 7.336]
        }))
        .unwrap();
        let mut world = World::new();
        let mut g = ProGates { look: Some(Some((Handle::default(), Handle::default(), model))), ..default() };
        g.set("gate_1".into(), 8, aabb(0.));
        world.insert_resource(g);
        draw(&mut world);
        let gates = |w: &mut World| w.query::<&Name>().iter(w).filter(|n| n.as_str() == GATE_NAME).count();
        assert_eq!(gates(&mut world), 1);
        world.resource_mut::<ProGates>().clear(8);
        draw(&mut world);
        assert_eq!(gates(&mut world), 0);
    }

    /// The pros' places, scenes, standing entities and P-Rod's leader in P1's export (D1-D4). RETAIL values: scene
    /// lengths from scene.bin (research6 2), entity keys and models from the vault (research6 2), the competitor
    /// string 0x00111680 = "paul_rodriguez" (old_ai1's `Competitors` rule).
    /// overnight-1015 C3: the marker stands 2 m over the sign-up's Location (RETAIL 0x823defe8), drawn unlit.
    #[test]
    fn skate1_signup_marker_stands_two_metres_over_the_location() {
        assert_eq!(marker_at(Vec3::new(1., 2., 3.)).translation, Vec3::new(1., 4., 3.));
        let m = marker_material();
        assert!(m.unlit && m.double_sided && m.alpha_mode == AlphaMode::Blend);
    }

    /// overnight-1015 C2: the bio card's rows (RETAIL 0x8245eaa8 / 0x82327828; english.big: PAUL RODRIGUEZ,
    /// CHATSWORTH, CA, GOOFY; JASON DILL, NEW YORK, NY, REGULAR).
    #[test]
    fn skate1_pro_info_picks_the_running_pros_row() {
        assert_eq!(pro_info(Some("fp_u3_tutorial02")), ["ID_CHEAT_PROSKATER17", "ID_CHEAT_PRODETAIL_LOCATION11", "ID_CHEAT_PRODETAIL_GOOFY"]);
        assert_eq!(pro_info(Some("fp_u3_tutorial01")), ["ID_CHEAT_PROSKATER06", "ID_CHEAT_PRODETAIL_LOCATION5", "ID_CHEAT_PRODETAIL_REGULAR"]);
        assert_eq!(pro_info(Some("fp_u3_tutorial05")), pro_info(None), "Carroll is the fallthrough row");
        assert_eq!(pro_info(Some("fp_u3_tutorial04"))[0], "ID_CHEAT_PROSKATER11");
    }

    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT (P1 output, rerun after D1-D4)"]
    fn pro_tutorials_have_their_places_scenes_cast_and_rider() {
        let root = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"))
            .join("private/challenges-skate1");
        let read = |f: &str| -> serde_json::Value { serde_json::from_slice(&std::fs::read(root.join(f)).unwrap()).unwrap() };
        let (t, nis, cast, ai) = (read("tutorial.json"), read("nis/tutorial.json"), read("cast.json"), read("ai.json"));
        let locator = |n: &serde_json::Value| t["places"]["locators"].get(n.as_str().unwrap()).is_some();
        let volume = |n: &serde_json::Value| t["places"]["volumes"].get(n.as_str().unwrap()).is_some();
        for (name, key, model, scenes) in [
            ("fp_u3_tutorial02", "B033A0ADE0933A58", "Paul_rodriguez",
             &[("NIS_PRod_ProIntro_S01_0", 26.33), ("NIS_PRod_ProTutorial_S02_0", 48.73)][..]),
            ("fp_u3_tutorial01", "0DED833242F2F857", "jason_dill",
             &[("NIS_JasonDill_ProIntro_S01_0", 72.83), ("NIS_JasonDill_ProTutorial_S02_0", 46.0)][..]),
            ("fp_u3_tutorial05", "146D3B7D88385267", "mike_carroll",
             &[("NIS_MCarrol_ProIntro_S01_0", 34.87), ("NIS_MCarrol_ProTutorial_S01_0", 34.0)][..]),
            ("fp_u3_tutorial04", "5B87C6981CA2AE86", "Chris_haslam",
             &[("NIS_CHaslam_ProIntro2_S01_0", 46.23), ("NIS_CHaslam_ProTutorial_S01_0", 35.07)][..]),
        ] {
            let me = &t["pro_tutorials"][name]["self"];
            assert!(locator(&me["Location"]), "{name}: Location");
            assert!(locator(&me["BestTrickStartLocations"][0]), "{name}: start");
            assert!(volume(&me["OTSTriggerBoundary"]), "{name}: boundary");
            assert_eq!(me["GlobalSpawn"][0]["key"], key, "{name}: spawn row");
            assert_eq!(cast["entities"][key]["draw"], model, "{name}: standing pro");
            for &(scene, secs) in scenes {
                let d = nis["scenes"][scene]["duration"].as_f64().unwrap_or_else(|| panic!("{name}: {scene} missing"));
                assert!((d - secs).abs() < 0.01, "{scene}: {d}");
            }
        }
        // P-Rod: his gates and his leader's spawn (fp_u3_tutorial02.lua lines 7, 79, 179-181).
        let prod = &t["pro_tutorials"]["fp_u3_tutorial02"]["self"];
        assert!(prod["OTSSpotVolumes"].as_array().unwrap().iter().all(|v| volume(v)), "P-Rod's spot volumes");
        assert!(locator(&prod["BestTrickStartLocations"][1]), "the leader's spawn");
        assert_eq!(ai["riders"]["fp_u3_tutorial02"], serde_json::json!(["paul_rodriguez"]));
        assert!(ai["paths"].get("fp_u3_tutorial02").is_some() && ai["profiles"].get("paul_rodriguez").is_some());
        // Haslam's first spot volume (his script never reads OTSSpotVolumes; _02 is in no rx2 on the disc, RETAIL).
        assert!(volume(&t["pro_tutorials"]["fp_u3_tutorial04"]["self"]["OTSSpotVolumes"][0]));
    }
}
