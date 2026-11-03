//! Resident retail parts: selection changes visibility/materials; morphs stay on GPU.
use crate::customiser_material::{SkaterMaterial, SkinStamp};
use bevy::{gltf::Gltf, mesh::morph::MorphWeights, prelude::*};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Deserialize)]
pub(crate) struct Part {
    pub slot: String,
    pub name: String,
    pub flags: HashMap<String, String>,
    pub materials: Vec<String>,
    pub scene: String,
}
impl Part {
    pub fn flag(&self, key: &str) -> &str {
        self.flags.get(key).map(String::as_str).unwrap_or("")
    }
    /// perf 10-19: this part's glTF, or with `label` one of its sub-assets (its scene), through [`load_gltf`]. A deck
    /// keeps its meshes' RAM copy: animation::taper reads their positions for the longboard.
    pub fn load<A: Asset>(&self, server: &AssetServer, label: Option<GltfAssetLabel>) -> Handle<A> {
        let path = label.map_or_else(|| self.scene.clone().into(), |label| label.from_asset(self.scene.clone()));
        load_gltf(server, path, self.slot == "SkateBoard")
    }
}
/// perf 10-19: SKATE_PARTS_GPU_ONLY: a skater glTF (a library part, the stock skater) whose meshes (unless
/// `keep_meshes`) and textures keep no RAM copy once uploaded, as multiplayer::render loads online characters. Nothing
/// reads their pixels: the only Image pixel readers are `mip` (stock logos and tattoos, loaded on their own), traffic's
/// paint (car models) and screenshot captures. Every loader of these files comes through here, as the asset server
/// keeps the first load's settings.
pub(crate) fn load_gltf<A: Asset>(server: &AssetServer, path: bevy::asset::AssetPath<'static>, keep_meshes: bool) -> Handle<A> {
    if !crate::config::perf_on("SKATE_PARTS_GPU_ONLY") {
        return server.load(path);
    }
    server.load_with_settings(path, move |s: &mut bevy::gltf::GltfLoaderSettings| gpu_only(s, keep_meshes))
}
fn gpu_only(s: &mut bevy::gltf::GltfLoaderSettings, keep_meshes: bool) {
    use bevy::asset::RenderAssetUsages;
    if !keep_meshes {
        s.load_meshes = RenderAssetUsages::RENDER_WORLD;
    }
    s.load_materials = RenderAssetUsages::RENDER_WORLD;
}
#[derive(Clone, Deserialize)]
pub(crate) struct Material {
    pub name: String,
    pub flags: HashMap<String, String>,
    diffuse: String,
    normal: Option<String>,
    rough: Option<String>,
    alpha: bool,
    #[serde(default)]
    opacity: Option<String>,
    pub tint: [f32; 3],
    metallic: f32,
    roughness: f32,
    #[serde(default)]
    lighting: Option<crate::retail_character::MaterialData>,
}
impl Material {
    pub fn flag(&self, key: &str) -> &str {
        self.flags.get(key).map(String::as_str).unwrap_or("")
    }
}
#[derive(Clone, Deserialize)]
pub(crate) struct Colour {
    pub name: String,
    pub rgb: [f32; 3],
}
#[derive(Clone, Deserialize)]
pub(crate) struct Tattoo {
    pub name: String,
    pub texture: String,
    pub bounds: [f32; 4],
}
/// A stock clothing graphic (logo_object): a brand or team logo.
#[derive(Clone, Deserialize)]
pub(crate) struct Graphic {
    pub name: String,
    /// "brand" or "team".
    pub kind: String,
    /// The decal (256x128 transparent art).
    pub texture: String,
    /// Its art box (cas.StampBorderConstraint, [top, bottom, left, right] V up), which retail
    /// keeps inside the Graphic Location; libraries made before it fit the whole decal.
    #[serde(default = "whole_decal")]
    pub bounds: [f32; 4],
}
fn whole_decal() -> [f32; 4] {
    crate::custom_graphic::CANVAS
}
/// The kinds of item the library files under retail's Accessory slot, which holds rings (cas.AccessoryType, set by
/// skate1.py / pro_clothing.py), and the slot each is worn in: its own, so a backpack doesn't take the gloves off
/// (user 10-30).
pub(crate) const ACCESSORY_SLOTS: [(&str, &str); 5] =
    [("belt", "Belt"), ("chain", "Chain"), ("backpack", "Backpack"), ("gloves", "Gloves"), ("other", "AccessoryOther")];
fn accessory_slots<'de, D: serde::Deserializer<'de>>(d: D) -> Result<HashMap<String, Part>, D::Error> {
    let mut models = HashMap::<String, Part>::deserialize(d)?;
    for part in models.values_mut().filter(|p| p.slot == "Accessory") {
        // pro_clothing.py types the wallet chain a belt, though it is worn with one.
        let kind = if part.name.ends_with("Wallet Chain") { "chain" } else { part.flag("AccessoryType") };
        let slot = ACCESSORY_SLOTS.iter().find(|(k, _)| *k == kind).map(|(_, slot)| *slot);
        if let Some(slot) = slot {
            part.slot = slot.to_owned();
        }
    }
    Ok(models)
}
#[derive(Default, Deserialize)]
pub(crate) struct Library {
    #[serde(deserialize_with = "accessory_slots")]
    pub models: HashMap<String, Part>,
    pub materials: HashMap<String, Material>,
    pub defaults: HashMap<String, Value>,
    pub morphs: Vec<String>,
    #[serde(default)]
    pub tattoos: HashMap<String, Tattoo>,
    #[serde(default)]
    pub graphics: HashMap<String, Graphic>,
    #[serde(default)]
    pub colours: Vec<Colour>,
}
#[derive(Resource, Default)]
pub(crate) struct Parts {
    pub library: Library,
    /// The library's set folder and file contents, hashed: changes whenever the customiser stage or a clothing
    /// script (dlc_clothing.py, old_skate_clothing.py) rewrites it (team photos are cached by it).
    pub fingerprint: String,
    /// Loaded on first preview, not all at startup.
    geometry: HashMap<String, Handle<Gltf>>,
    materials: HashMap<String, (Handle<SkaterMaterial>, Vec<Handle<Image>>)>,
    instances: HashMap<String, Entity>,
    tattoos: HashMap<String, Handle<Image>>,
    /// Stock graphics, loaded when first worn.
    graphics: HashMap<String, Handle<Image>>,
    pub designs: crate::custom_graphic::Designs,
    /// Where library paths are relative to.
    asset_root: std::path::PathBuf,
    pub applied: Value,
    /// The editor was open since the last `evict_previews`.
    browsed: bool,
    /// The local skater's grip (`grip_material`), on its deck's GRIP meshes.
    grip: Option<Handle<SkaterMaterial>>,
    /// A worn piece's scene was spawned again ([`respawned`]): `update` puts the outfit on again and rebinds.
    respawned: bool,
}
/// A deck's grip meshes: its upward faces, cut into a primitive of their own by character_glb.py.
pub(crate) const GRIP: &str = "Retail_Grip";
/// Hidden preview items kept spawned while Edit Skater is open (about two or three pages of the grid; see trim_previews).
const MAX_PREVIEWS: usize = 160;
/// A profile's Grip Color (cas_menu Node::Grip): the colour, and Solid (flat) rather than Realistic (the grip's
/// texture tinted). None: the authored grip.
pub(crate) fn grip_colour(profile: &Value) -> Option<([f32; 3], bool)> {
    let grip = &profile["grip"];
    let rgb = serde_json::from_value::<[f32; 3]>(grip["rgb"].clone()).ok()?;
    let solid = match &grip["solid"] {
        Value::Null => false,
        v => v.as_bool()?,
    };
    rgb.iter().all(|v| (0.0..=1.0).contains(v)).then_some((rgb, solid))
}
#[derive(Component)]
pub(crate) struct PartRoot(pub String);
pub(crate) fn asset_directory(assets: &std::path::Path) -> std::path::PathBuf {
    let base = assets.join("private/customisation");
    if std::fs::read(base.join("customiser-availability.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|v| v["version"] == 1 && v["status"] == "unavailable")
    {
        return base.join("unavailable");
    }
    if let Some(set) = std::fs::read(base.join("current.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["set"].as_str().map(str::to_owned))
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return base.join("sets").join(set);
    }
    base
}
impl Parts {
    #[cfg(test)]
    pub fn for_test(library: Library) -> Self {
        Self {
            library,
            ..default()
        }
    }
    /// A head and one item of each kind the library files under "Accessory", as the library file lists them.
    #[cfg(test)]
    pub fn accessory_test_library() -> Library {
        let item = |name: &str, kind: &str| {
            serde_json::json!({"slot": "Accessory", "name": name, "flags": {"Gender": "male", "AccessoryType": kind}, "materials": ["m"], "scene": ""})
        };
        serde_json::from_value(serde_json::json!({
            "models": {
                "head": {"slot": "Rostral", "name": "head", "flags": {}, "materials": ["m"], "scene": ""},
                "ring": item("Ring", ""), "belt": item("Belt", "belt"), "chain": item("Chris Cole's Wallet Chain", "belt"),
                "backpack": item("Mike's Backpack", "backpack"), "gloves": item("Danny Way Gloves", "gloves"),
                "cigarette": item("Dan Drehobl Cigarette", "other")},
            "materials": {"m": {"name": "m", "flags": {}, "diffuse": "", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1}},
            "defaults": {"male": {"gender": "male", "selections": {"Rostral": {"asset_id": "head", "material_id": "m"}}}},
            "morphs": []
        }))
        .unwrap()
    }
    /// A library whose files are read from `asset_root` (a test that loads the installed models).
    #[cfg(test)]
    pub fn for_assets(library: Library, asset_root: std::path::PathBuf) -> Self {
        Self {
            library,
            asset_root,
            ..default()
        }
    }
    /// Every item browsed in the editor stays spawned under the player (a hidden
    /// skinned scene re-propagated each frame) with its meshes and textures resident.
    /// After the editor closes, keep only what the applied outfit wears.
    fn evict_previews(&mut self, commands: &mut Commands) {
        let Self {
            applied,
            geometry,
            materials,
            instances,
            graphics,
            ..
        } = self;
        let worn: Vec<(&str, &str)> = applied["selections"]
            .as_object()
            .into_iter()
            .flat_map(|s| s.values())
            .filter_map(|v| Some((v["asset_id"].as_str()?, v["material_id"].as_str()?)))
            .collect();
        instances.retain(|id, entity| {
            let keep = worn.iter().any(|(asset, _)| asset == id);
            if !keep {
                commands.entity(*entity).despawn();
            }
            keep
        });
        geometry.retain(|id, _| worn.iter().any(|(asset, _)| asset == id));
        materials.retain(|mid, _| worn.iter().any(|(_, material)| material == mid));
        // Stock logos looked at in the editor (graphics_ready loads them again when a profile wears one).
        let logos: Vec<&str> = applied["graphics"].as_object().into_iter().flatten().filter_map(|(_, e)| crate::custom_graphic::logo(e)).collect();
        graphics.retain(|id, _| logos.contains(&id.as_str()));
        // The drafts the editor walked through leave their side textures behind: free the unworn ones.
        self.designs.forget_unworn(crate::custom_graphic::LOCAL_SIZE);
    }
    /// User 10-29: free the materials no skater wears any more (a player left): ours and `keep` (the shown skaters'
    /// pieces) stay. The other skaters' outfits warm this cache, and only our Edit Skater closing emptied it, so every
    /// outfit seen stayed in graphics memory for the whole session.
    pub(crate) fn forget_unworn_materials(&mut self, keep: &HashSet<&str>) {
        let Self { applied, materials, .. } = self;
        let worn: Vec<&str> = applied["selections"].as_object().into_iter().flat_map(|s| s.values()).filter_map(|v| v["material_id"].as_str()).collect();
        materials.retain(|mid, _| keep.contains(mid.as_str()) || worn.contains(&mid.as_str()));
    }
    /// While the editor is open: past MAX_PREVIEWS spawned items, free every one not in `keep` (the grid on
    /// screen and the preview) or worn, so one long browse through the 1,191 models does not pile them all up.
    /// ponytail: a whole trim at the cap, not per-item LRU; going back to an older page loads it again.
    fn trim_previews(&mut self, commands: &mut Commands, keep: &HashSet<(String, String)>) {
        if self.instances.len() <= MAX_PREVIEWS {
            return;
        }
        let Self { applied, geometry, materials, instances, .. } = self;
        let worn: Vec<(&str, &str)> = applied["selections"]
            .as_object()
            .into_iter()
            .flat_map(|s| s.values())
            .filter_map(|v| Some((v["asset_id"].as_str()?, v["material_id"].as_str()?)))
            .collect();
        let asset = |id: &str| worn.iter().any(|(a, _)| *a == id) || keep.iter().any(|(a, _)| a == id);
        instances.retain(|id, entity| {
            let kept = asset(id);
            if !kept {
                commands.entity(*entity).despawn();
            }
            kept
        });
        geometry.retain(|id, _| asset(id));
        materials.retain(|mid, _| worn.iter().any(|(_, m)| *m == mid) || keep.iter().any(|(_, m)| m == mid));
    }
    pub fn resolve(&self, profile: &Value) -> Result<Value, String> {
        // Online safety 10-26: a peer's look can be any JSON (multiplayer/appearance.rs read_look), and serde_json's
        // index writes below (`fill_style_defaults`, the Hair choice) panic on a non-object: every remote crashed.
        if !profile.is_object() || !matches!(profile.get("selections"), None | Some(Value::Null | Value::Object(_))) {
            return Err("Invalid outfit".into());
        }
        let lib = &self.library;
        if let Some(gender) = profile["gender"].as_str() {
            if !["male", "female"].contains(&gender) {
                return Err("Invalid body".into());
            }
        }
        for (name, max) in [
            ("truck", 1.),
            ("wheel", 1.),
            ("stance", 1.),
            ("style", 3.),
            ("posture", 3.),
            ("board_size", 3.),
        ] {
            if let Some(value) = profile.get(name) {
                if value.as_f64().is_none_or(|v| !(0.0..=max).contains(&v)) {
                    return Err(format!("Invalid {name}"));
                }
            }
        }
        if let Some(colours) = profile["colours"].as_object() {
            for value in colours.values() {
                // The menu uses null to clear a tint and restore the authored
                // material colour; profile_material already handles this reset.
                if value.is_null() {
                    continue;
                }
                let rgb = serde_json::from_value::<[f32; 3]>(value.clone())
                    .map_err(|_| "Invalid clothing colour")?;
                if rgb.iter().any(|v| !(0.0..=1.0).contains(v)) {
                    return Err("Invalid clothing colour".into());
                }
            }
        }
        for key in ["skin_tint", "hair_tint"] {
            // Null: the material's own (item 31's Default on Hair).
            if let Some(value) = profile.get(key).filter(|v| !v.is_null()) {
                let v = serde_json::from_value::<[f32; 3]>(value.clone())
                    .map_err(|_| "Invalid colour")?;
                if v.iter().any(|v| !(0.0..=1.0).contains(v)) {
                    return Err("Invalid colour".into());
                }
            }
        }
        // Null or absent: the authored grip.
        if !profile["grip"].is_null() && grip_colour(profile).is_none() {
            return Err("Invalid grip colour".into());
        }
        if let Some(weights) = profile["morphs"].as_object() {
            if weights.iter().any(|(key, v)| {
                !lib.morphs.contains(key) || v.as_f64().is_none_or(|v| !(0.0..=0.5).contains(&v))
            }) {
                return Err("Invalid face or body value".into());
            }
        }
        if let Some(gestures) = profile["gestures"].as_object() {
            if gestures.iter().any(|(key, v)| {
                !["0", "1", "2", "3"].contains(&key.as_str()) || v.as_u64().is_none_or(|v| v >= 37)
            }) {
                return Err("Invalid gesture".into());
            }
        }
        if let Some(zones) = profile["tattoos"].as_object() {
            for (zone, v) in zones {
                if !["Arm", "Leg"].contains(&zone.as_str())
                    || v["id"]
                        .as_str()
                        .is_some_and(|id| !lib.tattoos.contains_key(id))
                    || v["side"]
                        .as_u64()
                        .is_some_and(|v| v > if zone == "Arm" { 3 } else { 1 })
                {
                    return Err("Invalid tattoo".into());
                }
            }
        }

        let mut result = profile.clone();
        // Item 31's first preview (10-11) kept each colour row's Custom colour here: no longer anything's.
        if let Some(object) = result.as_object_mut() {
            object.remove("custom_colours");
        }
        // A save from before wore any of ACCESSORY_SLOTS' items under "Accessory": it moves to its own slot, with its
        // colour.
        let own = result["selections"]["Accessory"]["asset_id"].as_str().and_then(|id| lib.models.get(id)).map(|p| p.slot.clone());
        if let Some(slot) = own.filter(|slot| slot != "Accessory") {
            for key in ["selections", "colours"] {
                if let Some(map) = result.get_mut(key).and_then(Value::as_object_mut) {
                    if let Some(item) = map.remove("Accessory") {
                        map.insert(slot.clone(), item);
                    }
                }
            }
        }
        crate::custom_graphic::sanitize(&mut result, |id| lib.graphics.contains_key(id));
        fill_style_defaults(&mut result);
        let gender = profile["gender"].as_str().unwrap_or("male");
        if let Some(hair) = profile.get("hair_choice") {
            result["selections"]["Hair"] = hair.clone();
        }
        // Items a worn item's rule takes off (the RIG legs' shoes, a hood-up top's hair and hat) wait in "covered"
        // while it applies and come back after, unless the slot was chosen since (a null: removed on purpose).
        let covered = result.as_object_mut().and_then(|p| p.remove("covered"));
        if let (Some(Value::Object(covered)), Some(worn)) = (covered, result["selections"].as_object_mut()) {
            for (slot, item) in covered {
                let valid = item["asset_id"].as_str().and_then(|id| lib.models.get(id)).is_some_and(|p| {
                    p.slot == slot
                        && item["material_id"].as_str().is_some_and(|m| p.materials.iter().any(|s| s == m) && lib.materials.contains_key(m))
                });
                if valid && !worn.contains_key(&slot) {
                    worn.insert(slot, item);
                }
            }
        }
        let mut hidden = serde_json::Map::new();
        let mut hide = |s: &mut serde_json::Map<String, Value>, slot: &str| {
            if let Some(item) = s.remove(slot) {
                hidden.insert(slot.to_owned(), item);
            }
        };
        let selections = result["selections"]
            .as_object_mut()
            .ok_or("No outfit selected")?;
        selections.retain(|_, v| !v.is_null());
        for (slot, v) in selections.iter() {
            let id = v["asset_id"].as_str().ok_or("Invalid item")?;
            let part = lib.models.get(id).ok_or("Item unavailable")?;
            // A worn item's Gender flag only filters the editor's menus (and `choose` below): retail wears what a
            // recipe names, e.g. preset 03 (miscload.big SavedRecipe2.bin) is male in female-flagged "Socks Tiny"
            // e901c7651adf0e17.
            if part.slot != *slot {
                return Err("Choose an item for this body".into());
            }
            let mid = v["material_id"].as_str().ok_or("Invalid colour")?;
            if !part.materials.iter().any(|s| s == mid) || !lib.materials.contains_key(mid) {
                return Err("Colour unavailable".into());
            }
        }
        let model = |s: &serde_json::Map<String, Value>, slot: &str| {
            s.get(slot)
                .and_then(|v| v["asset_id"].as_str())
                .and_then(|id| lib.models.get(id))
        };
        let choose = |s: &mut serde_json::Map<String, Value>,
                      slot: &str,
                      key: &str,
                      value: &str|
         -> Result<(), String> {
            if value.is_empty() || value == "none" {
                s.remove(slot);
                return Ok(());
            }
            if model(s, slot).is_some_and(|p| p.flag(key) == value) {
                return Ok(());
            }
            let mut candidates: Vec<_> = lib
                .models
                .iter()
                .filter(|(_, p)| {
                    p.slot == slot
                        && ["", gender, "unisex"].contains(&p.flag("Gender"))
                        && p.flag(key) == value
                })
                .collect();
            candidates.sort_by_key(|(id, _)| *id);
            let (id, p) = candidates
                .first()
                .ok_or_else(|| format!("No matching {}", slot))?;
            let mid = p
                .materials
                .iter()
                .find(|id| {
                    lib.materials
                        .get(*id)
                        .is_some_and(|m| m.flag("IsDefault") == "yes")
                })
                .or_else(|| {
                    p.materials
                        .iter()
                        .find(|id| lib.materials.contains_key(*id))
                })
                .ok_or("No matching material")?;
            s.insert(
                slot.into(),
                serde_json::json!({"asset_id":id,"material_id":mid}),
            );
            Ok(())
        };
        if let Some(outer) = model(selections, "OuterTorso") {
            choose(
                selections,
                "Arm",
                "ArmModelType",
                outer.flag("ArmModelRequired"),
            )?;
            choose(
                selections,
                "InnerTorso",
                "InnerCutType",
                outer.flag("InnerCutTypeRequired"),
            )?;
            if outer.flag("SupportsWristItem") == "no" {
                hide(selections, "WristItem");
            }
            if outer.flag("IsNecklaceRemoved") == "yes" {
                hide(selections, "Jewellery");
            }
        }
        if let Some(pants) = model(selections, "Pants") {
            choose(
                selections,
                "Leg",
                "LegModelType",
                pants.flag("LegModelRequired"),
            )?;
        }
        if let Some(leg) = model(selections, "Leg") {
            let sock_style = selections
                .get("Sock")
                .and_then(|v| v["material_id"].as_str())
                .and_then(|id| lib.materials.get(id))
                .map(|m| m.flag("SockStyle"))
                .unwrap_or("none");
            let desired = if sock_style == "none" { "" } else { sock_style };
            if leg.flag("RequiresSockStyle") != desired
                && !(desired.is_empty() && leg.flag("RequiresSockStyle") == "none")
            {
                let mut matches: Vec<_> = lib
                    .models
                    .iter()
                    .filter(|(_, p)| {
                        p.slot == "Leg"
                            && ["", gender, "unisex"].contains(&p.flag("Gender"))
                            && p.flag("LegModelType") == leg.flag("LegModelType")
                            && (p.flag("RequiresSockStyle") == desired
                                || (desired.is_empty() && p.flag("RequiresSockStyle") == "none"))
                    })
                    .collect();
                matches.sort_by_key(|(id, _)| *id);
                // Tight pants over socks: retail (CACPartDB 0x82ddccb8) falls back to the unisex tight legs that need
                // "tiny" socks; the socks follow below.
                let tight = (leg.flag("LegModelType") == "legs_tight" && !desired.is_empty())
                    .then(|| {
                        lib.models
                            .iter()
                            .filter(|(_, p)| {
                                p.slot == "Leg"
                                    && p.flag("Gender") == "unisex"
                                    && p.flag("LegModelType") == "legs_tight"
                                    && p.flag("RequiresSockStyle") == "tiny"
                            })
                            .min_by_key(|(id, _)| *id)
                    })
                    .flatten();
                let (id, p) = matches.first().copied().or(tight).ok_or("Choose another sock length")?;
                selections.insert(
                    "Leg".into(),
                    serde_json::json!({"asset_id":id,"material_id":p.materials[0]}),
                );
            }
        }
        // Socks follow the legs (retail CASSimManager 0x82540d28): none without bare legs (long pants cover them),
        // else the length the legs are cut for (the first socks of that length when the worn ones differ), and
        // "tiny" socks take the legs off (the unisex tight legs are an empty mesh).
        let need = model(selections, "Leg").map(|leg| leg.flag("RequiresSockStyle")).filter(|s| !s.is_empty());
        let style = |mid: &str| lib.materials.get(mid).map_or("", |m| m.flag("SockStyle"));
        if let Some(worn) = selections.get("Sock").map(|s| s["material_id"].as_str().map_or("", style)) {
            if need != Some(worn) {
                let first = need.and_then(|need| {
                    lib.models
                        .iter()
                        .filter(|(_, p)| p.slot == "Sock")
                        .flat_map(|(id, p)| p.materials.iter().filter(move |m| style(m.as_str()) == need).map(move |m| (id, m)))
                        .min()
                });
                match first {
                    Some((id, mid)) => {
                        selections.insert("Sock".into(), serde_json::json!({"asset_id": id, "material_id": mid}));
                    }
                    None => hide(selections, "Sock"),
                }
            }
            if selections.get("Sock").and_then(|s| s["material_id"].as_str()).map(style) == Some("tiny") {
                selections.remove("Leg");
            }
        }
        if !selections.contains_key("Hat") {
            if let Some(hair) = model(selections, "Hair") {
                if hair.flag("HairModelType") != "full" {
                    let candidate = lib
                        .models
                        .iter()
                        .filter(|(_, p)| {
                            p.slot == "Hair"
                                && p.flag("HairStyle") == hair.flag("HairStyle")
                                && p.flag("Gender") == hair.flag("Gender")
                                && p.flag("HairModelType") == "full"
                        })
                        .min_by_key(|(id, _)| *id);
                    if let Some((id, p)) = candidate {
                        selections.insert(
                            "Hair".into(),
                            serde_json::json!({"asset_id":id,"material_id":p.materials[0]}),
                        );
                    }
                }
            }
        }
        if let Some(hat) = model(selections, "Hat") {
            let required = hat.flag("RequiresHairModelType");
            if required == "none" {
                hide(selections, "Hair");
            } else if let Some(hair) = model(selections, "Hair") {
                if !required.is_empty() && hair.flag("HairModelType") != required {
                    let matching = lib
                        .models
                        .iter()
                        .filter(|(_, p)| {
                            p.slot == "Hair"
                                && p.flag("HairStyle") == hair.flag("HairStyle")
                                && p.flag("HairModelType") == required
                                && ["", gender, "unisex"].contains(&p.flag("Gender"))
                        })
                        .min_by_key(|(id, _)| *id);
                    if let Some((id, p)) = matching {
                        selections.insert(
                            "Hair".into(),
                            serde_json::json!({"asset_id":id,"material_id":p.materials[0]}),
                        );
                    } else {
                        hide(selections, "Hair");
                    }
                }
            }
        }
        let removals: Vec<String> = selections
            .keys()
            .flat_map(|s| {
                model(selections, s).into_iter().flat_map(|p| {
                    p.flag("RequiresRemovalOfComponents")
                        .split(',')
                        .map(|s| s.trim().to_owned())
                })
            })
            .filter(|s| !s.is_empty())
            .collect();
        for s in removals {
            hide(selections, &s);
        }
        let shadow = selections
            .get("Hair")
            .and_then(|v| v["asset_id"].as_str())
            .and_then(|id| lib.models.get(id))
            .map(|p| {
                if p.flag("RequiresHairlineShadowStyle") == "on" {
                    "on"
                } else {
                    ""
                }
            })
            .unwrap_or("");
        let head = selections
            .get("Rostral")
            .and_then(|v| v["material_id"].as_str())
            .and_then(|id| lib.materials.get(id))
            .ok_or("Head unavailable")?;
        let tone = profile["skin"].as_str().unwrap_or(head.flag("SkinTone"));
        let beard = if gender == "female" {
            ""
        } else {
            profile["beard"]
                .as_str()
                .unwrap_or(head.flag("FacialHairStyle"))
        };
        // Brow style is a head material variant too (Edit Skater > Brow > Style).
        let brow = profile["brow"]
            .as_str()
            .unwrap_or(head.flag("EyebrowStyle"));
        for (slot, value) in selections.iter_mut() {
            let old_id = value["material_id"].as_str().unwrap();
            let old = &lib.materials[old_id];
            if old.flag("SkinTone").is_empty() {
                continue;
            }
            let part = &lib.models[value["asset_id"].as_str().unwrap()];
            let best = part
                .materials
                .iter()
                .filter_map(|id| lib.materials.get(id).map(|m| (id, m)))
                .filter(|(_, m)| {
                    m.flag("SkinTone") == tone
                        && (slot != "Rostral" || m.flag("FacialHairStyle") == beard)
                        && (slot != "Rostral" || brow.is_empty() || m.flag("EyebrowStyle") == brow)
                })
                .min_by_key(|(id, m)| {
                    let shadow_penalty =
                        if slot == "Rostral" && m.flag("HairlineShadowStyle") != shadow {
                            100
                        } else {
                            0
                        };
                    shadow_penalty
                        + if *id == old_id {
                            0
                        } else {
                            1 + old
                                .flags
                                .iter()
                                .filter(|(k, v)| {
                                    k.as_str() != "SkinTone"
                                        && k.as_str() != "FacialHairStyle"
                                        && k.as_str() != "EyebrowStyle"
                                        && m.flags.get(*k) != Some(*v)
                                })
                                .count()
                        }
                });
            let (id, _) = best.ok_or("This skin and facial hair combination is unavailable")?;
            value["material_id"] = Value::String(id.clone());
        }
        if !hidden.is_empty() {
            result["covered"] = Value::Object(hidden);
        }
        // Older designs (versions 1 and 2) become whole-side ones on the worn item's rectangles.
        let worn = result["selections"].clone();
        crate::custom_graphic::migrate(&mut result, |zone| self.side_rect(&worn, zone));
        Ok(result)
    }
    /// The rectangle of the worn item's graphic location `zone` (a top's side, the hat, the deck).
    pub(crate) fn side_rect(&self, selections: &Value, zone: &str) -> Option<[f32; 4]> {
        let slot = match zone {
            "deck" => "SkateBoard",
            "hat" => "Hat",
            _ => "OuterTorso",
        };
        let worn = &selections[slot];
        let id = worn["asset_id"].as_str()?;
        let logoable = self.library.materials.get(worn["material_id"].as_str()?)?.flag("IsLogoable").eq_ignore_ascii_case("true");
        let locations = crate::custom_graphic::locations(id, self.library.models.get(id)?, logoable);
        locations.into_iter().find(|l| l.0 == zone).map(|l| l.1)
    }
    /// Clone per outfit: remote colours and tattoos must never mutate another player.
    pub(crate) fn profile_material(
        &self,
        id: &str,
        mid: &str,
        profile: &Value,
        materials: &Assets<SkaterMaterial>,
    ) -> Option<SkaterMaterial> {
        let part = self.library.models.get(id)?;
        let source = self.library.materials.get(mid)?;
        let mut material = materials.get(&self.materials.get(mid)?.0)?.clone();
        let tint = if !source.flag("SkinTone").is_empty() {
            profile.get("skin_tint")
        } else if part.slot == "Hair" {
            profile.get("hair_tint")
        } else {
            profile["colours"].get(&part.slot)
        };
        let rgb = tint
            .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
            .unwrap_or(source.tint);
        material.base.base_color = Color::srgb(rgb[0], rgb[1], rgb[2]);
        let slot = if part.slot == "OuterTorso" && part.flag("TopType").is_empty() {
            "Arm"
        } else {
            &part.slot
        };
        let stamp = &profile["tattoos"][slot];
        let mut extension = SkinStamp {
            hair_opacity: material.extension.hair_opacity.clone(),
            retail: material.extension.retail.clone(),
            retail_mask: material.extension.retail_mask.clone(),
            enabled: if source.opacity.is_some() {
                Vec4::Y
            } else {
                Vec4::ZERO
            },
            ..default()
        };
        if extension.retail.tint.w > 0. {
            extension.retail.tint =
                Vec4::from_array(material.base.base_color.to_linear().to_f32_array());
        }
        use crate::custom_graphic::{self as graphic, Adjust, Side};
        // Retail's start table is per gender; tattoos use it too (cacstamp_skin's VS is the cloth one).
        let female = profile["gender"].as_str() == Some("female");
        if let (Some(image), Some((entry, _))) = (
            stamp["id"].as_str().and_then(|id| self.tattoos.get(id)),
            graphic::tattoo(slot, stamp["side"].as_u64().unwrap_or(0)),
        ) {
            extension.set_zone(0, image.clone(), graphic::stamp(entry, female, Adjust::of(stamp)));
        }
        // Graphic Locations: only customizable (IsLogoable) tops, hats and decks. A side with a
        // design shows its whole-side canvas (a stock logo on it is drawn into that canvas, on
        // top); a side with only a stock logo shows retail's own stamp.
        let locations = graphic::locations(id, part, source.flag("IsLogoable").eq_ignore_ascii_case("true"));
        if !locations.is_empty() {
            extension.enabled.z = 1.;
            for (index, (zone, rect)) in locations.into_iter().enumerate() {
                let entry = &profile["graphics"][zone];
                if let Some(text) = graphic::design(entry) {
                    let side = Side { design: text, rect, over: self.overlay(entry, zone, female) };
                    let image = self.designs.get(&side, graphic::LOCAL_SIZE).or_else(|| self.designs.get(&side, graphic::REMOTE_SIZE));
                    if let Some(image) = image {
                        extension.set_zone(index, image, graphic::side_stamp(rect));
                    }
                } else if let Some(image) = graphic::logo(entry).and_then(|id| self.graphics.get(id)) {
                    extension.set_zone(index, image.clone(), graphic::stamp(graphic::entry(zone), female, Adjust::of(entry)));
                }
            }
        }
        material.extension = extension;
        Some(material)
    }
    /// A deck's grip material (its GRIP meshes) from the deck's `profile_material`, so the deck's colour never
    /// reaches the grip. Realistic tints the grip's own texture as retail tints a coloured part
    /// (cacstamp_shift_defaultPS: diffuse squared times the colour, as `profile_material`); Solid is the flat colour
    /// over the grip's specular mask and normals. No Grip Color: the authored grip. None for a part not a deck.
    pub(crate) fn grip_material(&self, id: &str, mid: &str, deck: &SkaterMaterial, profile: &Value) -> Option<SkaterMaterial> {
        if self.library.models.get(id)?.slot != "SkateBoard" {
            return None;
        }
        let chosen = grip_colour(profile);
        let (rgb, solid) = chosen.unwrap_or((self.library.materials.get(mid)?.tint, false));
        let mut grip = deck.clone();
        grip.base.base_color = Color::srgb(rgb[0], rgb[1], rgb[2]);
        if grip.extension.retail.tint.w > 0. {
            grip.extension.retail.tint = Vec4::from_array(grip.base.base_color.to_linear().to_f32_array());
        }
        // customiser_stamp.wgsl: 1 Solid, 2 Realistic (the tape coloured, the screws not), 0 the authored grip.
        grip.extension.enabled.w = match (chosen, solid) {
            (None, _) => 0.,
            (Some(_), true) => 1.,
            (Some(_), false) => 2.,
        };
        Some(grip)
    }
    /// The stock logo drawn over a side's design: its library key and retail stamp.
    fn overlay(&self, entry: &Value, zone: &str, female: bool) -> Option<(u64, [Vec4; 2])> {
        use crate::custom_graphic::{self as graphic, Adjust};
        let id = graphic::logo(entry).filter(|id| self.library.graphics.contains_key(*id))?;
        Some((graphic::logo_key(id)?, graphic::stamp(graphic::entry(zone), female, Adjust::of(entry))))
    }
    /// Load the stock graphics and draw the sides a profile wears (`size`: the
    /// local skater's or another player's resolution), and mip its logos and tattoos once
    /// loaded. False until all are ready.
    pub(crate) fn graphics_ready(&mut self, profile: &Value, server: &AssetServer, images: &mut Assets<Image>, size: u32) -> bool {
        use crate::custom_graphic::{self as graphic, Side};
        let mut ready = true;
        let tattoos = profile["tattoos"].as_object().into_iter().flat_map(|t| t.values());
        for handle in tattoos.filter_map(|t| self.tattoos.get(t["id"].as_str()?)) {
            mip(images, handle);
        }
        let female = profile["gender"].as_str() == Some("female");
        for (zone, entry) in profile["graphics"].as_object().into_iter().flatten() {
            if let Some(g) = graphic::logo(entry).and_then(|id| Some((id, self.library.graphics.get(id)?))) {
                let handle = self.graphics.entry(g.0.to_owned()).or_insert_with(|| server.load(g.1.texture.clone()));
                ready &= server.is_loaded_with_dependencies(handle.id());
                mip(images, handle);
            }
            if let (Some(text), Some(rect)) = (graphic::design(entry), self.side_rect(&profile["selections"], zone)) {
                let side = Side { design: text, rect, over: self.overlay(entry, zone, female) };
                let (library, root) = (&self.library, &self.asset_root);
                ready &= self.designs.prepare(&side, size, images, |key| art_file(library, root, key));
            }
        }
        ready
    }
    /// A stock graphic's file on disk.
    pub(crate) fn graphic_file(&self, id: &str) -> Option<std::path::PathBuf> {
        self.library.graphics.get(id).map(|g| self.asset_root.join(&g.texture))
    }
    /// Draw a design on a canvas (`canvas` pixels: a side's, or the Graphic Creator's 2:1) to
    /// straight-alpha RGBA.
    pub(crate) fn draw_design(&mut self, doc: &crate::custom_graphic::Doc, canvas: (u32, u32)) -> Vec<u8> {
        let (library, root) = (&self.library, &self.asset_root);
        self.designs.draw(doc, canvas, None, |key| art_file(library, root, key))
    }
    pub(crate) fn material_ready(&self, mid: &str, server: &AssetServer) -> bool {
        self.materials.get(mid).is_some_and(|(_, images)| {
            images
                .iter()
                .all(|h| server.is_loaded_with_dependencies(h.id()))
        })
    }
    pub(crate) fn tattoos_ready(&self, profile: &Value, server: &AssetServer) -> bool {
        profile["tattoos"]
            .as_object()
            .into_iter()
            .flat_map(|v| v.values())
            .all(|v| {
                v["id"].as_str().is_none_or(|id| {
                    self.tattoos
                        .get(id)
                        .is_some_and(|h| server.is_loaded_with_dependencies(h.id()))
                })
            })
    }
    pub fn warm(&mut self, id: &str, server: &AssetServer, materials: &mut Assets<SkaterMaterial>) {
        if self.materials.contains_key(id) {
            return;
        }
        let Some(m) = self.library.materials.get(id) else {
            return;
        };
        let mut images = vec![];
        // perf 10-19: SKATE_PARTS_GPU_ONLY: the diffuse, normal, rough, opacity and mask maps are only sampled by the
        // skater shaders (no pixel reader takes them, see load_gltf; no stock logo or tattoo shares their files), so
        // they keep no RAM copy once uploaded.
        let gpu_only = crate::config::perf_on("SKATE_PARTS_GPU_ONLY");
        let mut load = |path: &String, linear: bool| {
            let handle = server.load_with_settings(
                path.clone(),
                move |s: &mut bevy::image::ImageLoaderSettings| {
                    s.is_srgb = !linear;
                    if gpu_only {
                        s.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
                    }
                },
            );
            images.push(handle.clone());
            handle
        };
        let diffuse = load(&m.diffuse, false);
        let normal = m.normal.as_ref().map(|p| load(p, true));
        let rough = m.rough.as_ref().map(|p| load(p, true));
        let opacity = m.opacity.as_ref().map(|p| load(p, true));
        let retail_mask = m
            .lighting
            .as_ref()
            .and_then(|l| l.specular.as_ref())
            .map(|p| load(p, true));
        let retail = m
            .lighting
            .as_ref()
            .filter(|l| l.params.len() == 9)
            .map(|l| crate::retail_character::CharacterParams {
                tint: Vec4::from_array(
                    Color::srgb(m.tint[0], m.tint[1], m.tint[2])
                        .to_linear()
                        .to_f32_array(),
                ),
                options: Vec4::new(
                    f32::from(normal.is_some()),
                    f32::from(retail_mask.is_some()),
                    -1.,
                    f32::from(l.is_hair()),
                ),
                rows: std::array::from_fn(|i| Vec4::from_array(l.params[i])),
                ..default()
            })
            .unwrap_or_default();
        let material = materials.add(SkaterMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(m.tint[0], m.tint[1], m.tint[2]),
                base_color_texture: Some(diffuse.clone()),
                normal_map_texture: normal.clone(),
                metallic_roughness_texture: rough,
                double_sided: m.opacity.is_some(),
                cull_mode: if m.opacity.is_some() {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                },
                metallic: m.metallic,
                perceptual_roughness: if m.rough.is_some() { 1. } else { m.roughness },
                alpha_mode: if m.alpha {
                    AlphaMode::Mask(0.5)
                } else {
                    AlphaMode::Opaque
                },
                ..default()
            },
            extension: SkinStamp {
                retail,
                retail_mask,
                hair_opacity: opacity,
                enabled: if m.opacity.is_some() {
                    Vec4::Y
                } else {
                    Vec4::ZERO
                },
                ..default()
            },
        });
        self.materials.insert(id.into(), (material, images));
    }
}
/// The file of a library image a design draws, by its 16 hex digit key: a stock graphic, else a
/// tattoo or decal (`GameArt`). None when this install lacks it.
fn art_file(library: &Library, root: &std::path::Path, key: u64) -> Option<std::path::PathBuf> {
    let id = crate::custom_graphic::logo_id(key);
    let texture = library.graphics.get(&id).map(|g| &g.texture).or_else(|| library.tattoos.get(&id).map(|t| &t.texture))?;
    Some(root.join(texture))
}
/// Give a loaded stock logo or tattoo retail's mip chain (custom_graphic::mips), once.
fn mip(images: &mut Assets<Image>, handle: &Handle<Image>) {
    let pending = |i: &Image| {
        let size = i.texture_descriptor.size;
        i.texture_descriptor.mip_level_count == 1 && i.data.as_ref().is_some_and(|d| d.len() == (size.width * size.height * 4) as usize)
    };
    if !images.get(handle).is_some_and(pending) {
        return;
    }
    let Some(image) = images.get_mut(handle) else { return };
    let size = image.texture_descriptor.size;
    let (data, levels) = crate::custom_graphic::mips(image.data.as_deref().unwrap_or_default(), size.width, size.height);
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
}
/// Stance (goofy), skating style and posture the game uses when a profile omits them.
pub(crate) const STYLE_DEFAULTS: [(&str, u64); 3] = [("stance", 1), ("style", 0), ("posture", 0)];
/// Written into profiles so the Edit Style rows show the value in effect.
fn fill_style_defaults(profile: &mut Value) {
    for (key, value) in STYLE_DEFAULTS {
        if profile.get(key).is_none() {
            profile[key] = value.into();
        }
    }
}
pub(crate) fn setup(
    mut commands: Commands,
    config: Res<crate::config::Config>,
    server: Res<AssetServer>,
) {
    let directory = asset_directory(&config.asset_root);
    let bytes = std::fs::read(directory.join("library-v3.json"))
        .or_else(|_| std::fs::read(directory.join("library.json")))
        .unwrap_or_default();
    let mut library = serde_json::from_slice::<Library>(&bytes).unwrap_or_default();
    let mut fingerprint = blake3::Hasher::new();
    fingerprint.update(directory.to_string_lossy().as_bytes());
    fingerprint.update(&bytes);
    library.defaults.values_mut().for_each(fill_style_defaults);
    let tattoos = library
        .tattoos
        .iter()
        .map(|(id, t)| {
            (
                id.clone(),
                server.load_with_settings(
                    t.texture.clone(),
                    |s: &mut bevy::image::ImageLoaderSettings| {
                        s.is_srgb = false;
                    },
                ),
            )
        })
        .collect();
    commands.insert_resource(Parts {
        tattoos,
        library,
        fingerprint: fingerprint.finalize().to_hex()[..32].to_owned(),
        asset_root: config.asset_root.clone(),
        ..default()
    });
}
pub(crate) fn update(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut parts: ResMut<Parts>,
    mut state: ResMut<crate::customiser::Customiser>,
    mut materials: ResMut<Assets<SkaterMaterial>>,
    root: Query<Entity, With<crate::world::PlayerRoot>>,
    parents: Query<&ChildOf>,
    mut scenes: Query<
        (Entity, Option<&PartRoot>, &mut Visibility),
        (
            With<SceneRoot>,
            Without<crate::custom_models::CustomModelRoot>,
        ),
    >,
    mut mesh_materials: Query<
        (Entity, Option<&mut MeshMaterial3d<SkaterMaterial>>, Option<&bevy::gltf::GltfMaterialName>),
        With<Mesh3d>,
    >,
    mut morphs: Query<(Entity, &mut MorphWeights)>,
    mut animation: ResMut<crate::animation::AnimationStatus>,
    custom_models: Res<crate::custom_models::CustomModels>,
    mut images: ResMut<Assets<Image>>,
) {
    if !state.enabled || custom_models.active.is_some() {
        return;
    }
    // Closed editor: preview() is the draft, and `applied` only holds a resolved
    // preview. Skips a JSON clone plus a full resolve on every gameplay frame.
    if !state.open && parts.applied == state.draft && !parts.respawned {
        // Once per close: multiplayer warms this cache too, and evicting every
        // frame would drop a remote outfit's textures before they finish loading.
        if parts.browsed {
            parts.browsed = false;
            parts.evict_previews(&mut commands);
        }
        return;
    }
    parts.browsed |= state.open;
    let Ok(root) = root.single() else {
        return;
    };
    let preview = state.preview(&parts);
    let Some(selections) = preview["selections"].as_object() else {
        return;
    };
    let desired: Vec<(String, String)> = selections
        .values()
        .filter_map(|v| {
            Some((
                v["asset_id"].as_str()?.into(),
                v["material_id"].as_str()?.into(),
            ))
        })
        .collect();
    // A missing/failed library is not a valid empty outfit. Never hide the
    // fallback skater until a complete replacement can actually be published.
    if desired.is_empty() || parts.resolve(&preview).is_err() {
        let message = "Character assets are unavailable. Update this copy's character assets; your skater is unchanged.";
        if state.status != message {
            state.status = message.into();
            state.redraw = true;
        }
        return;
    }
    if parts.applied == preview && !state.open && !parts.respawned {
        return;
    }
    // Resolve the visible choices ahead of selection, including skin, head,
    // sleeves, and sock dependencies. No conversion process runs in the game.
    let precache: HashSet<(String, String)> = state
        .preload_outfits(&parts)
        .iter()
        .flat_map(|p| {
            p["selections"]
                .as_object()
                .into_iter()
                .flat_map(|s| s.values())
        })
        .filter_map(|v| {
            Some((
                v["asset_id"].as_str()?.to_owned(),
                v["material_id"].as_str()?.to_owned(),
            ))
        })
        .chain(desired.iter().cloned())
        .collect();
    parts.trim_previews(&mut commands, &precache);
    for (_, mid) in &precache {
        parts.warm(mid, &server, &mut materials);
    }
    let precache: HashSet<String> = precache.into_iter().map(|(id, _)| id).collect();
    for id in precache {
        if parts.instances.contains_key(&id) {
            continue;
        }
        let Some((scene, deck)) = parts.library.models.get(&id).map(|p| (p.scene.clone(), p.slot == "SkateBoard")) else {
            continue;
        };
        // perf 10-19: through load_gltf (SKATE_PARTS_GPU_ONLY), as Part::load.
        let handle = parts
            .geometry
            .entry(id.clone())
            .or_insert_with(|| load_gltf(&server, scene.clone().into(), deck))
            .id();
        if server.is_loaded_with_dependencies(handle) {
            let entity = commands
                .spawn((
                    PartRoot(id.clone()),
                    Visibility::Hidden,
                    SceneRoot(load_gltf(&server, GltfAssetLabel::Scene(0).from_asset(scene), deck)),
                ))
                .id();
            commands.entity(root).add_child(entity);
            parts.instances.insert(id, entity);
        }
    }
    if parts.applied == preview && !parts.respawned {
        return;
    }
    let mut ready = true;
    for (id, mid) in &desired {
        parts.warm(mid, &server, &mut materials);
        if !parts.instances.contains_key(id) {
            ready = false;
        }
        let part = &parts.library.models[id];
        let slot = if part.slot == "OuterTorso" && part.flag("TopType").is_empty() {
            "Arm"
        } else {
            &part.slot
        };
        if let Some(tattoo) = state.draft["tattoos"][slot]["id"].as_str() {
            if parts
                .tattoos
                .get(tattoo)
                .is_none_or(|h| !server.is_loaded_with_dependencies(h.id()))
            {
                ready = false;
            }
        }
        if !parts.materials.get(mid).is_some_and(|(_, images)| {
            images
                .iter()
                .all(|h| server.is_loaded_with_dependencies(h.id()))
        }) {
            ready = false;
        }
        if let Some(&entity) = parts.instances.get(id) {
            if !mesh_materials
                .iter()
                .any(|(e, ..)| parents.iter_ancestors(e).any(|p| p == entity))
            {
                ready = false;
            }
        }
    }
    let size = crate::custom_graphic::LOCAL_SIZE;
    if !parts.graphics_ready(&state.draft, &server, &mut images, size) || !ready {
        return;
    }
    let wanted: HashSet<_> = desired.iter().map(|(id, _)| id.as_str()).collect();
    let mut rebind = false;
    for (entity, part, mut visible) in &mut scenes {
        if !parents.get(entity).is_ok_and(|p| p.parent() == root) {
            continue;
        }
        let target = if part.is_some_and(|p| wanted.contains(p.0.as_str())) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visible != target {
            *visible = target;
            rebind = true;
        }
    }
    for (id, mid) in &desired {
        let entity = parts.instances[id];
        let handle = parts.materials[mid].0.clone();
        let mut grip = handle.clone();
        if let Some(updated) = parts.profile_material(id, mid, &state.draft, &materials) {
            if let Some(material) = parts.grip_material(id, mid, &updated, &state.draft) {
                grip = parts.grip.get_or_insert_with(|| materials.add(material.clone())).clone();
                if let Some(old) = materials.get_mut(&grip) {
                    *old = material;
                }
            }
            if let Some(material) = materials.get_mut(&handle) {
                *material = updated;
            }
        }
        for (e, material, name) in &mut mesh_materials {
            if !parents.iter_ancestors(e).any(|p| p == entity) {
                continue;
            }
            let handle = if name.is_some_and(|n| n.0 == GRIP) { &grip } else { &handle };
            commands
                .entity(e)
                .insert(bevy::camera::visibility::RenderLayers::from_layers(&[
                    0, 28,
                ]));
            if let Some(mut material) = material {
                if material.0 != *handle {
                    material.0 = handle.clone();
                }
            } else {
                commands
                    .entity(e)
                    .remove::<MeshMaterial3d<StandardMaterial>>()
                    .insert(MeshMaterial3d(handle.clone()));
            }
        }
    }
    let weights: Vec<f32> = parts
        .library
        .morphs
        .iter()
        .enumerate()
        .map(|(i, n)| {
            state.draft["morphs"][n]
                .as_f64()
                .unwrap_or(if (2..19).contains(&i) { 0.25 } else { 0. })
                .clamp(0., 0.5) as f32
        })
        .collect();
    for (entity, mut morph) in &mut morphs {
        if parents
            .iter_ancestors(entity)
            .any(|p| parts.instances.values().any(|e| *e == p))
            && morph.weights().len() == weights.len()
            && morph.weights() != weights
        {
            morph.weights_mut().copy_from_slice(&weights);
        }
    }
    // A piece spawned again has new joints: bind them too.
    if std::mem::take(&mut parts.respawned) | rebind {
        *animation = default();
    }
    if parts.applied != preview {
        parts.applied = preview;
        state.status.clear();
        state.redraw = true;
    }
}
/// Bug 10-25 #7: Bevy spawns every instance of a scene again when its asset is replaced, i.e. when a part's glb loads a
/// second time: a Scene0-only loader ([`Part::load`]: team photos, Custom Graphic tiles, other players' outfits) asking
/// while this player's load of it runs, or loading it once its Gltf was freed. A worn piece then came back on the glTF's
/// own material (white) with new joints the skeleton never drove (the pants hung off the body) until the outfit changed.
/// Only a shown piece under the player counts: hidden previews get their material when they are put on.
pub(crate) fn respawned(
    ready: On<bevy::scene::SceneInstanceReady>,
    pieces: Query<(&ChildOf, &Visibility), With<PartRoot>>,
    players: Query<(), With<crate::world::PlayerRoot>>,
    mut parts: ResMut<Parts>,
) {
    if pieces.get(ready.entity).is_ok_and(|(parent, shown)| *shown != Visibility::Hidden && players.contains(parent.parent())) {
        parts.respawned = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// perf 10-19: a part's textures stay GPU-only; a deck's meshes keep their RAM copy for animation::taper.
    #[test]
    fn only_a_deck_keeps_its_meshes_in_ram() {
        use bevy::asset::RenderAssetUsages;
        let settings = |keep| {
            let mut s = bevy::gltf::GltfLoaderSettings::default();
            gpu_only(&mut s, keep);
            (s.load_meshes, s.load_materials)
        };
        assert_eq!(settings(false), (RenderAssetUsages::RENDER_WORLD, RenderAssetUsages::RENDER_WORLD));
        assert_eq!(settings(true), (RenderAssetUsages::default(), RenderAssetUsages::RENDER_WORLD));
    }
    #[test]
    fn owned_library_choices_resolve_without_conversion() {
        let Ok(path) = std::env::var("SKATE_CAC_TEST_LIBRARY") else {
            return;
        };
        let library: Library = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let parts = Parts {
            library,
            ..default()
        };
        let mut failures = vec![];
        for gender in ["male", "female"] {
            let base = &parts.library.defaults[gender];
            assert!(
                parts.resolve(base).is_ok(),
                "{gender}: {:?}",
                parts.resolve(base)
            );
            for (id, part) in &parts.library.models {
                if !["", gender, "unisex"].contains(&part.flag("Gender")) {
                    continue;
                }
                if ["Arm", "Leg", "Organ", "Rostral", "InnerTorso"].contains(&part.slot.as_str()) {
                    continue;
                }
                for mid in &part.materials {
                    let mut p = base.clone();
                    p["selections"][&part.slot] =
                        serde_json::json!({"asset_id":id,"material_id":mid});
                    if let Err(e) = parts.resolve(&p) {
                        failures.push(format!("{gender} {} {}: {e}", part.slot, part.name));
                    }
                }
            }
            for tone in ["light", "dark"] {
                let mut p = base.clone();
                p["skin"] = serde_json::json!(tone);
                if let Err(e) = parts.resolve(&p) {
                    failures.push(format!("{gender} {tone}: {e}"));
                }
            }
        }
        failures.sort();
        failures.dedup();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
#[cfg(test)]
mod online_tests {
    use super::*;
    /// A worn item's Gender flag only filters the editor's menus: retail's preset 03 (SavedRecipe2.bin) is a male
    /// skater in female-flagged "Socks Tiny", and resolves. An item in another slot's place still does not.
    #[test]
    fn a_recipe_wears_an_item_flagged_for_the_other_body() {
        let mut parts = Parts::default();
        let flags = HashMap::from([("Gender".into(), "female".into())]);
        parts.library.models.insert("socks".into(), Part { slot: "Sock".into(), name: "Socks Tiny".into(), flags, materials: vec!["m".into()], scene: String::new() });
        parts.library.materials.insert(
            "m".into(),
            Material { name: "m".into(), flags: HashMap::new(), diffuse: String::new(), normal: None, rough: None, alpha: false, opacity: None, tint: [1.; 3], metallic: 0., roughness: 0.5, lighting: None },
        );
        let wear = |slot: &str| serde_json::json!({"gender": "male", "selections": {slot: {"asset_id": "socks", "material_id": "m"}}});
        // ponytail: the fixture has no head, so resolve stops later at "Head unavailable"; only the body gate is under test.
        assert_ne!(parts.resolve(&wear("Sock")).err().as_deref(), Some("Choose an item for this body"));
        assert_eq!(parts.resolve(&wear("Hat")).unwrap_err(), "Choose an item for this body");
    }
    /// User 10-30: gloves and a backpack could not be worn together, as every item the library files under retail's
    /// Accessory slot (rings) was worn in it. Each kind has its own slot; a save from before moves its one item there,
    /// with its colour.
    #[test]
    fn every_accessory_kind_is_worn_with_the_others() {
        let parts = Parts::for_test(Parts::accessory_test_library());
        let items = ["ring", "belt", "chain", "backpack", "gloves", "cigarette"];
        let slot = |id: &str| parts.library.models[id].slot.clone();
        assert_eq!(items.map(slot), ["Accessory", "Belt", "Chain", "Backpack", "Gloves", "AccessoryOther"]);
        let worn = |id: &str| serde_json::json!({"asset_id": id, "material_id": "m"});
        let mut look = serde_json::json!({"gender": "male", "selections": {"Rostral": worn("head")}});
        for id in items {
            look["selections"][slot(id)] = worn(id);
        }
        let all = parts.resolve(&look).unwrap();
        assert_eq!(all["selections"].as_object().unwrap().len(), 7, "{all}");
        let old = serde_json::json!({"gender": "male", "selections": {"Rostral": worn("head"), "Accessory": worn("backpack")},
            "colours": {"Accessory": [0.1, 0.2, 0.3]}});
        let moved = parts.resolve(&old).unwrap();
        assert_eq!(moved["selections"]["Backpack"], worn("backpack"));
        assert_eq!(moved["colours"]["Backpack"], serde_json::json!([0.1, 0.2, 0.3]));
        assert!(moved["selections"].get("Accessory").is_none() && moved["colours"].get("Accessory").is_none(), "{moved}");
        let ring = serde_json::json!({"gender": "male", "selections": {"Rostral": worn("head"), "Accessory": worn("ring")}});
        assert_eq!(parts.resolve(&ring).unwrap()["selections"]["Accessory"], worn("ring"), "a ring stays");
    }
    #[test]
    fn online_appearance_materials_keep_players_colours_and_tattoos_independent() {
        let mut parts = Parts::default();
        parts.library.models.insert(
            "body".into(),
            Part {
                slot: "Arm".into(),
                name: "body".into(),
                flags: HashMap::from([("StampUVConstraintQ3".into(), "0,0,1,1".into())]),
                materials: vec!["skin".into()],
                scene: String::new(),
            },
        );
        parts.library.materials.insert(
            "skin".into(),
            Material {
                name: "skin".into(),
                flags: HashMap::from([("SkinTone".into(), "fair".into())]),
                diffuse: String::new(),
                normal: None,
                rough: None,
                alpha: false,
                opacity: None,
                tint: [1.; 3],
                metallic: 0.,
                roughness: 0.5,
                lighting: None,
            },
        );
        parts.library.tattoos.insert(
            "ink".into(),
            Tattoo {
                name: "ink".into(),
                texture: String::new(),
                bounds: [0., 0., 1., 1.],
            },
        );
        let mut images = Assets::<Image>::default();
        let image = images.add(Image::default());
        parts.tattoos.insert("ink".into(), image.clone());
        let mut materials = Assets::<SkaterMaterial>::default();
        let handle = materials.add(SkaterMaterial {
            base: StandardMaterial {
                normal_map_texture: Some(image.clone()),
                base_color_texture: Some(image.clone()),
                ..default()
            },
            extension: SkinStamp {
                retail_mask: Some(image.clone()),
                retail: crate::retail_character::CharacterParams {
                    tint: Vec4::ONE,
                    options: Vec4::new(1., 1., -1., 0.),
                    ..default()
                },
                ..default()
            },
        });
        parts
            .materials
            .insert("skin".into(), (handle.clone(), vec![]));
        let red = parts
            .profile_material(
                "body",
                "skin",
                &serde_json::json!({"skin_tint":[1,0,0],"tattoos":{"Arm":{"id":"ink","side":0}}}),
                &materials,
            )
            .unwrap();
        let blue = parts
            .profile_material(
                "body",
                "skin",
                &serde_json::json!({"skin_tint":[0,0,1]}),
                &materials,
            )
            .unwrap();
        assert_eq!(red.base.base_color, Color::srgb(1., 0., 0.));
        assert_eq!(blue.base.base_color, Color::srgb(0., 0., 1.));
        for material in [&red, &blue] {
            assert_eq!(material.base.normal_map_texture, Some(image.clone()));
            assert_eq!(material.base.base_color_texture, Some(image.clone()));
            assert_eq!(material.extension.retail_mask, Some(image.clone()));
            assert_eq!(
                material.extension.retail.options,
                Vec4::new(1., 1., -1., 0.)
            );
            assert_eq!(
                material.extension.retail.tint,
                Vec4::from_array(material.base.base_color.to_linear().to_f32_array())
            );
        }
        assert_eq!(red.extension.texture, Some(image));
        assert!(blue.extension.texture.is_none());
        assert_eq!(
            materials.get(&handle).unwrap().base.base_color,
            Color::WHITE
        );
        assert!(materials.get(&handle).unwrap().extension.texture.is_none());
    }

    /// Bug 7 + item 6: the deck's colour never reaches its grip; the grip wears its own Grip Color (Realistic:
    /// the grip's texture tinted by retail's rule; Solid: flat), and peers' values are checked.
    #[test]
    fn the_grip_wears_its_own_colour_and_never_the_decks() {
        let mut parts = Parts::default();
        let part = |slot: &str| Part { slot: slot.into(), name: slot.into(), flags: HashMap::new(), materials: vec!["m".into()], scene: String::new() };
        parts.library.models.insert("deck".into(), part("SkateBoard"));
        parts.library.models.insert("shirt".into(), part("OuterTorso"));
        parts.library.materials.insert(
            "m".into(),
            serde_json::from_value(serde_json::json!({"name": "m", "flags": {"IsColourizable": "True"}, "diffuse": "x.png", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1})).unwrap(),
        );
        let mut materials = Assets::<SkaterMaterial>::default();
        let retail = crate::retail_character::CharacterParams { tint: Vec4::ONE, light: Vec4::ONE, ..default() };
        let handle = materials.add(SkaterMaterial { base: default(), extension: SkinStamp { retail, ..default() } });
        parts.materials.insert("m".into(), (handle, vec![]));
        let profile = |grip: Value| serde_json::json!({"colours": {"SkateBoard": [1, 0, 0]}, "grip": grip});
        let grip = |p: &Value| {
            let deck = parts.profile_material("deck", "m", p, &materials).unwrap();
            (parts.grip_material("deck", "m", &deck, p).unwrap(), deck)
        };
        let (authored, deck) = grip(&profile(Value::Null));
        assert_eq!(deck.base.base_color, Color::srgb(1., 0., 0.));
        assert_eq!((authored.base.base_color, authored.extension.retail.tint, authored.extension.enabled.w), (Color::srgb(1., 1., 1.), Vec4::ONE, 0.));
        for solid in [false, true] {
            let (g, deck) = grip(&profile(serde_json::json!({"rgb": [0, 0, 1], "solid": solid})));
            assert_eq!(g.base.base_color, Color::srgb(0., 0., 1.));
            assert_eq!(g.extension.retail.tint, Vec4::from_array(Color::srgb(0., 0., 1.).to_linear().to_f32_array()));
            assert_eq!(g.extension.enabled.w, if solid { 1. } else { 2. }, "Solid, or Realistic's own shading");
            assert_eq!(deck.base.base_color, Color::srgb(1., 0., 0.), "the deck keeps its colour");
        }
        assert!(parts.grip_material("shirt", "m", &deck, &profile(Value::Null)).is_none());
        for (grip, ok) in [
            (Value::Null, true),
            (serde_json::json!({"rgb": [0.5, 0.5, 0.5]}), true),
            (serde_json::json!({"rgb": [2, 0, 0], "solid": true}), false),
            (serde_json::json!({"rgb": [0, 0, 0], "solid": 1}), false),
            (serde_json::json!({"solid": true}), false),
            (serde_json::json!("red"), false),
        ] {
            let result = parts.resolve(&serde_json::json!({"grip": grip}));
            assert_eq!(result.as_ref().err().map(String::as_str) == Some("Invalid grip colour"), !ok, "{grip}");
        }
    }

    #[test]
    fn the_board_size_is_kept_and_checked() {
        let parts = Parts::default();
        for (size, ok) in [(0., true), (3., true), (4., false), (-1., false)] {
            let result = parts.resolve(&serde_json::json!({"board_size": size}));
            // An empty outfit fails later ("No outfit selected"): only the size's own check is asked about here.
            assert_eq!(result.err().as_deref() == Some("Invalid board_size"), !ok, "{size}");
        }
        assert_eq!(crate::animation::BOARD_SIZES[1], ("Standard", 1., 1.), "the default is the stock deck");
    }

    /// Online safety 10-26: a peer's look can be any JSON, and every remote skater's outfit goes through `resolve`:
    /// no shape may panic it (a 12-byte `{"Outfit":5}` crashed every other player in the lobby).
    #[test]
    fn a_malformed_outfit_is_refused_never_a_panic() {
        use serde_json::json;
        let parts = Parts::default();
        let odd = [json!(5), json!("x"), json!([]), json!([1]), json!(true), json!(null), json!({}), json!({"a": 1})];
        let keys = [
            "selections", "hair_choice", "covered", "graphics", "tattoos", "colours", "morphs", "gestures", "gender",
            "grip", "skin", "beard", "brow", "stance", "skin_tint", "custom_colours",
        ];
        let mut profiles = odd.to_vec();
        for key in keys {
            for value in &odd {
                profiles.push(json!({ key: value }));
                profiles.push(json!({ key: value, "hair_choice": 1 }));
                profiles.push(json!({ "selections": { "Hair": value, "Head": value }, key: value }));
            }
        }
        for profile in &profiles {
            let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parts.resolve(profile)));
            assert!(resolved.is_ok(), "panicked on {profile}");
        }
        for bad in [json!(5), json!({"selections": [], "hair_choice": 1}), json!({"selections": "x"})] {
            assert_eq!(parts.resolve(&bad).err().as_deref(), Some("Invalid outfit"), "{bad}");
        }
    }
}

#[cfg(test)]
mod stock_hair_audit {
    use super::*;
    #[test]
    #[ignore = "requires prepared owned SKATE_CAC_TEST_LIBRARY"]
    fn stock_hair_keeps_authored_shading_and_textures_after_colour_restore() {
        let path = std::env::var("SKATE_CAC_TEST_LIBRARY").unwrap();
        let library: Library = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let pairs: Vec<_> = library
            .models
            .iter()
            .filter(|(_, p)| p.slot == "Hair")
            .flat_map(|(id, p)| p.materials.iter().map(move |mid| (id.clone(), mid.clone())))
            .collect();
        assert!(!pairs.is_empty());
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_asset::<Image>();
        let server = app.world().resource::<AssetServer>();
        let mut parts = Parts {
            library,
            ..default()
        };
        let mut materials = Assets::<SkaterMaterial>::default();
        let mut checked = HashSet::new();
        for (id, mid) in pairs {
            parts.warm(&mid, server, &mut materials);
            let handle = parts.materials[&mid].0.clone();
            let original = materials.get(&handle).unwrap().clone();
            assert_eq!(
                original.extension.retail.options.w, 1.,
                "{mid} must use hair lighting"
            );
            let dyed = parts
                .profile_material(
                    &id,
                    &mid,
                    &serde_json::json!({"hair_tint":[1,0,0]}),
                    &materials,
                )
                .unwrap();
            *materials.get_mut(&handle).unwrap() = dyed;
            let restored = parts
                .profile_material(&id, &mid, &serde_json::json!({}), &materials)
                .unwrap();
            assert_eq!(restored.base.base_color, original.base.base_color);
            assert_eq!(
                restored.extension.retail.tint,
                original.extension.retail.tint
            );
            assert_eq!(
                restored.extension.retail.options,
                original.extension.retail.options
            );
            assert_eq!(
                restored.base.base_color_texture,
                original.base.base_color_texture
            );
            assert_eq!(
                restored.base.normal_map_texture,
                original.base.normal_map_texture
            );
            assert_eq!(
                restored.extension.hair_opacity,
                original.extension.hair_opacity
            );
            assert_eq!(
                restored.extension.retail_mask,
                original.extension.retail_mask
            );
            assert_eq!(
                restored.base.perceptual_roughness,
                original.base.perceptual_roughness
            );
            *materials.get_mut(&handle).unwrap() = restored;
            checked.insert(mid);
        }
        eprintln!(
            "STOCK_HAIR_AUDIT verified {} authored materials",
            checked.len()
        );
    }
}

#[cfg(test)]
mod covered_tests {
    use super::*;
    /// An item a worn item's rule takes off comes back when that item goes (10-11: Isaac's RIG legs took the shoes
    /// off for good, a hood-up top the hair and hat). One taken off on purpose stays off.
    #[test]
    fn rule_removed_items_come_back_when_the_rule_goes() {
        let mut parts = Parts::default();
        for (id, slot, rule) in [
            ("head", "Rostral", ""),
            ("rig", "Pants", "Feet,Sock"),
            ("jeans", "Pants", ""),
            ("shoes", "Feet", ""),
            ("hoodup", "OuterTorso", "Hat,Hair"),
            ("tee", "OuterTorso", ""),
            ("hair", "Hair", ""),
            ("cap", "Hat", ""),
        ] {
            let flags = HashMap::from([("RequiresRemovalOfComponents".into(), rule.into())]);
            parts.library.models.insert(id.into(), Part { slot: slot.into(), name: id.into(), flags, materials: vec!["m".into()], scene: String::new() });
        }
        parts.library.materials.insert(
            "m".into(),
            Material { name: "m".into(), flags: HashMap::new(), diffuse: String::new(), normal: None, rough: None, alpha: false, opacity: None, tint: [1.; 3], metallic: 0., roughness: 0.5, lighting: None },
        );
        let worn = |id: &str| serde_json::json!({"asset_id": id, "material_id": "m"});
        // The editor's apply: the patch merged into the draft, then resolved.
        let wear = |profile: &Value, slot: &str, id: &str| {
            let mut p = profile.clone();
            p["selections"][slot] = worn(id);
            parts.resolve(&p).unwrap()
        };
        let start = parts
            .resolve(&serde_json::json!({"gender": "male", "selections": {
                "Rostral": worn("head"), "Pants": worn("jeans"), "Feet": worn("shoes"), "OuterTorso": worn("tee"),
                "Hair": worn("hair"), "Hat": worn("cap")}}))
            .unwrap();
        let rig = wear(&start, "Pants", "rig");
        assert!(rig["selections"].get("Feet").is_none(), "the RIG legs take the shoes off");
        assert_eq!(wear(&rig, "Pants", "jeans")["selections"]["Feet"], worn("shoes"), "other pants: the shoes are back");
        let hood = wear(&start, "OuterTorso", "hoodup");
        assert!(hood["selections"].get("Hair").is_none() && hood["selections"].get("Hat").is_none(), "hood up: no hair, no hat");
        let tee = wear(&hood, "OuterTorso", "tee");
        assert_eq!((&tee["selections"]["Hair"], &tee["selections"]["Hat"]), (&worn("hair"), &worn("cap")), "a tee: hair and hat are back");
        // Remove Hair (the menu's patch, as Customiser::apply records it) under the hood: it stays off after.
        let mut removed = hood.clone();
        removed["selections"]["Hair"] = Value::Null;
        removed["hair_choice"] = Value::Null;
        let removed = parts.resolve(&removed).unwrap();
        let tee = wear(&removed, "OuterTorso", "tee");
        assert!(tee["selections"].get("Hair").is_none(), "removed hair stays off");
        assert_eq!(tee["selections"]["Hat"], worn("cap"));
    }
}

#[cfg(test)]
mod sock_tests {
    use super::*;
    /// Retail's sock rules (CASSimManager 0x82540d28, CACPartDB 0x82ddccb8): long pants leave no bare legs, so no
    /// socks (they come back with shorts); tight pants over socks wear the "tiny" ones and no legs.
    #[test]
    fn socks_follow_the_legs_as_retail_dresses_them() {
        let mut parts = Parts::default();
        for (id, slot, flags) in [
            ("head", "Rostral", vec![]),
            ("jeans", "Pants", vec![]),
            ("shorts", "Pants", vec![("LegModelRequired", "knee")]),
            ("tights", "Pants", vec![("LegModelRequired", "legs_tight")]),
            ("knee", "Leg", vec![("LegModelType", "knee")]),
            ("knee_long", "Leg", vec![("LegModelType", "knee"), ("RequiresSockStyle", "long")]),
            ("tight", "Leg", vec![("LegModelType", "legs_tight")]),
            ("tight_socked", "Leg", vec![("LegModelType", "legs_tight"), ("RequiresSockStyle", "tiny"), ("Gender", "unisex")]),
            ("kneehigh", "Sock", vec![]),
            ("tiny", "Sock", vec![]),
        ] {
            let flags = flags.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
            let material = match id { "kneehigh" => "long", "tiny" => "tiny", _ => "m" };
            parts.library.models.insert(id.into(), Part { slot: slot.into(), name: id.into(), flags, materials: vec![material.into()], scene: String::new() });
        }
        for (mid, style) in [("m", ""), ("long", "long"), ("tiny", "tiny")] {
            let flags = if style.is_empty() { HashMap::new() } else { HashMap::from([("SockStyle".into(), style.into())]) };
            parts.library.materials.insert(
                mid.into(),
                Material { name: mid.into(), flags, diffuse: String::new(), normal: None, rough: None, alpha: false, opacity: None, tint: [1.; 3], metallic: 0., roughness: 0.5, lighting: None },
            );
        }
        let worn = |id: &str, mid: &str| serde_json::json!({"asset_id": id, "material_id": mid});
        let wear = |profile: &Value, slot: &str, item: Value| {
            let mut p = profile.clone();
            p["selections"][slot] = item;
            parts.resolve(&p).unwrap()
        };
        let shorts = parts
            .resolve(&serde_json::json!({"gender": "male", "selections": {
                "Rostral": worn("head", "m"), "Pants": worn("shorts", "m"), "Sock": worn("kneehigh", "long")}}))
            .unwrap();
        assert_eq!((&shorts["selections"]["Leg"], &shorts["selections"]["Sock"]), (&worn("knee_long", "m"), &worn("kneehigh", "long")));
        let jeans = wear(&shorts, "Pants", worn("jeans", "m"));
        assert!(jeans["selections"].get("Leg").is_none() && jeans["selections"].get("Sock").is_none(), "long pants: no legs, no socks");
        assert_eq!(wear(&jeans, "Pants", worn("shorts", "m"))["selections"]["Sock"], worn("kneehigh", "long"), "shorts: the socks are back");
        let tights = wear(&shorts, "Pants", worn("tights", "m"));
        assert!(tights["selections"].get("Leg").is_none(), "tiny socks take the legs off");
        assert_eq!(tights["selections"]["Sock"], worn("tiny", "tiny"), "tight pants: tiny socks");
        let mut bare = shorts.clone();
        bare["selections"]["Sock"] = Value::Null;
        let bare = wear(&parts.resolve(&bare).unwrap(), "Pants", worn("tights", "m"));
        assert_eq!(bare["selections"]["Leg"], worn("tight", "m"), "tight pants, no socks: the tight legs");
    }
}

#[cfg(test)]
mod respawn_tests {
    use super::*;
    use bevy::mesh::skinning::SkinnedMesh;
    /// A part's scene as its glb gives it: a joint and a skinned mesh on the glTF's own material.
    fn piece() -> Scene {
        let mut world = World::new();
        let joint = world.spawn((Name::new("HIPS"), Transform::default())).id();
        world.spawn((Mesh3d::default(), MeshMaterial3d::<StandardMaterial>::default(), SkinnedMesh { inverse_bindposes: default(), joints: vec![joint] }));
        Scene::new(world)
    }
    fn meshes(app: &mut App, root: Entity) -> Vec<(Entity, bool)> {
        let mut q = app.world_mut().query_filtered::<(Entity, Has<MeshMaterial3d<SkaterMaterial>>, Has<MeshMaterial3d<StandardMaterial>>), With<Mesh3d>>();
        let all: Vec<_> = q.iter(app.world()).collect();
        let w = app.world();
        all.into_iter()
            .filter(|(e, ..)| std::iter::successors(w.get::<ChildOf>(*e).map(|c| c.parent()), |p| w.get::<ChildOf>(*p).map(|c| c.parent())).any(|p| p == root))
            .map(|(e, skater, standard)| (e, skater && !standard))
            .collect()
    }
    /// Bug 10-25 #7: Bevy spawns every instance of a scene again when its asset is replaced (a second load of the
    /// part's glb: a Scene0-only loader, a team photo, a Custom Graphic tile or another player's outfit, asking while
    /// this player's load of it runs, or loading it again once its Gltf was freed). A worn piece came back on its glTF
    /// material (white) with new joints the skeleton never drove (pants hanging off the body) until the outfit changed.
    #[test]
    fn a_worn_piece_spawned_again_wears_its_material_and_is_bound_again() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), bevy::scene::ScenePlugin));
        app.init_asset::<SkaterMaterial>()
            .init_asset::<Image>()
            .init_resource::<crate::custom_models::CustomModels>()
            .init_resource::<crate::animation::AnimationStatus>()
            .register_type::<MeshMaterial3d<StandardMaterial>>()
            .add_observer(respawned)
            .add_systems(Update, update);
        let library: Library = serde_json::from_value(serde_json::json!({
            "models": {
                "head": {"slot": "Rostral", "name": "head", "flags": {}, "materials": ["m"], "scene": ""},
                "pants": {"slot": "Pants", "name": "pants", "flags": {}, "materials": ["m"], "scene": ""}},
            "materials": {"m": {"name": "m", "flags": {}, "diffuse": "", "alpha": false, "tint": [1, 1, 1], "metallic": 0, "roughness": 1}},
            "defaults": {}, "morphs": []
        }))
        .unwrap();
        let mut parts = Parts::for_test(library);
        let world = app.world_mut();
        let material = world.resource_mut::<Assets<SkaterMaterial>>().add(SkaterMaterial { base: default(), extension: default() });
        parts.materials.insert("m".into(), (material, vec![]));
        let player = world.spawn((crate::world::PlayerRoot, Transform::default(), Visibility::default())).id();
        let mut scenes = HashMap::new();
        for id in ["head", "pants"] {
            let scene = world.resource_mut::<Assets<Scene>>().add(piece());
            let entity = world.spawn((PartRoot(id.into()), Visibility::Hidden, SceneRoot(scene.clone()), ChildOf(player))).id();
            parts.instances.insert(id.into(), entity);
            scenes.insert(id, (scene, entity));
        }
        let worn = |id: &str| serde_json::json!({"asset_id": id, "material_id": "m"});
        let draft = parts.resolve(&serde_json::json!({"gender": "male", "selections": {"Rostral": worn("head"), "Pants": worn("pants")}})).unwrap();
        world.insert_resource(parts);
        world.insert_resource(crate::customiser::Customiser::for_draft(draft));
        let (scene, pants) = scenes["pants"].clone();
        for _ in 0..4 {
            app.update();
        }
        assert!(!app.world().resource::<Parts>().applied.is_null(), "the outfit is on");
        let first = meshes(&mut app, pants);
        assert!(!first.is_empty() && first.iter().all(|(_, skater)| *skater), "{first:?}");
        app.world_mut().resource_mut::<crate::animation::AnimationStatus>().ready = true; // animation::bind's
        // Past SceneSpawner's 3-frame debounce of a new scene's events, then the pants' glb loads again.
        for _ in 0..4 {
            app.update();
        }
        app.world_mut().resource_mut::<Assets<Scene>>().insert(scene.id(), piece()).unwrap();
        for _ in 0..5 {
            app.update();
        }
        let again = meshes(&mut app, pants);
        assert!(again.iter().all(|(e, _)| first.iter().all(|(f, _)| f != e)), "the pants were spawned again");
        assert!(!again.is_empty() && again.iter().all(|(_, skater)| *skater), "the pants wear their material again: {again:?}");
        assert!(!app.world().resource::<crate::animation::AnimationStatus>().ready, "the skeleton binds the new joints");
        assert_eq!(*app.world().get::<Visibility>(pants).unwrap(), Visibility::Inherited);
    }
}
#[cfg(test)]
mod reload_tests {
    //! Bug 10-25 #7 on the installed glbs (SKATE3_ASSET_ROOT; each returns at once without it).
    use super::*;
    const PANTS: &str = "d92b14a037b78985";
    fn app(assets: &std::path::Path) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin { file_path: assets.to_string_lossy().into_owned(), ..default() },
            bevy::image::ImagePlugin::default(),
            bevy::mesh::MeshPlugin,
            bevy::scene::ScenePlugin,
            bevy::gltf::GltfPlugin::default(),
        ));
        app.init_asset::<StandardMaterial>().init_asset::<AnimationClip>().register_type::<MeshMaterial3d<StandardMaterial>>();
        app.finish();
        app.cleanup();
        app
    }
    fn frames(app: &mut App, n: usize) {
        for _ in 0..n {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
    fn until(app: &mut App, done: impl Fn(&World) -> bool) {
        let t = std::time::Instant::now();
        while !done(app.world()) {
            frames(app, 1);
            assert!(t.elapsed().as_secs() < 60, "timed out");
        }
    }
    fn meshes_under(app: &mut App, root: Entity) -> Vec<Entity> {
        let mut q = app.world_mut().query_filtered::<Entity, With<Mesh3d>>();
        let all: Vec<Entity> = q.iter(app.world()).collect();
        let w = app.world();
        let mut out: Vec<Entity> = all
            .into_iter()
            .filter(|&e| std::iter::successors(w.get::<ChildOf>(e).map(|c| c.parent()), |p| w.get::<ChildOf>(*p).map(|c| c.parent())).any(|p| p == root))
            .collect();
        out.sort();
        out
    }
    fn pants(assets: &std::path::Path) -> (Library, String) {
        let library: Library = serde_json::from_slice(&std::fs::read(asset_directory(assets).join("library-v3.json")).unwrap()).unwrap();
        let scene = library.models[PANTS].scene.clone();
        (library, scene)
    }
    /// The online trigger: a part shown from its Scene0 alone (as Part::load gives another player's outfit, a team
    /// photo or a Custom Graphic tile) is spawned again, new entities on the glb's own material, the next time
    /// anything loads that glb (this player's Edit Skater does, for every item it previews or wears): Bevy freed the
    /// root Gltf after the first load, and loading it again replaces the Scene0 still shown.
    #[test]
    fn a_part_shown_from_its_scene_alone_is_spawned_again_when_its_glb_loads_again() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT").map(std::path::PathBuf::from) else { return };
        let (_, path) = pants(&assets);
        let mut app = app(&assets);
        let scene: Handle<Scene> = load_gltf(app.world().resource::<AssetServer>(), GltfAssetLabel::Scene(0).from_asset(path.clone()), false);
        let shown = app.world_mut().spawn(SceneRoot(scene)).id();
        until(&mut app, |w| w.get::<bevy::scene::SceneInstance>(shown).is_some_and(|i| w.resource::<bevy::scene::SceneSpawner>().instance_is_ready(**i)));
        frames(&mut app, 10);
        let before = meshes_under(&mut app, shown);
        let gltf: Handle<Gltf> = load_gltf(app.world().resource::<AssetServer>(), path.into(), false);
        until(&mut app, |w| w.resource::<AssetServer>().is_loaded_with_dependencies(gltf.id()));
        frames(&mut app, 10);
        let after = meshes_under(&mut app, shown);
        assert!(!before.is_empty() && before.iter().all(|e| !after.contains(e)), "spawned again: {before:?} -> {after:?}");
        assert!(after.iter().all(|&e| app.world().get::<MeshMaterial3d<StandardMaterial>>(e).is_some()), "on the glb's own material");
    }
    /// This player's default outfit, put on by `update` from the installed glbs; then the pants' glb loads a second
    /// time (AssetServer::reload replaces the same sub-assets as any second load). Without `respawned` the pants came
    /// back white (1 mesh off its SkaterMaterial) and the skeleton stayed bound to their old joints.
    #[test]
    fn a_worn_piece_its_glb_reloaded_under_is_dressed_and_bound_again() {
        let Some(assets) = std::env::var_os("SKATE3_ASSET_ROOT").map(std::path::PathBuf::from) else { return };
        let (library, path) = pants(&assets);
        let mut parts = Parts::for_assets(library, assets.clone());
        let draft = parts.resolve(&parts.library.defaults["male"]).unwrap();
        assert_eq!(draft["selections"]["Pants"]["asset_id"], PANTS);
        let mut app = app(&assets);
        app.init_asset::<SkaterMaterial>()
            .init_resource::<crate::custom_models::CustomModels>()
            .init_resource::<crate::animation::AnimationStatus>()
            .add_observer(respawned)
            .add_systems(Update, update);
        // The pieces' PNGs never finish loading in this bare app (the game's image path is not in it): stand-in
        // materials, which are not what is under test.
        for v in draft["selections"].as_object().unwrap().values() {
            let material = app.world_mut().resource_mut::<Assets<SkaterMaterial>>().add(SkaterMaterial { base: default(), extension: default() });
            parts.materials.insert(v["material_id"].as_str().unwrap().into(), (material, vec![]));
        }
        app.world_mut().spawn((crate::world::PlayerRoot, Transform::default(), Visibility::default()));
        app.world_mut().insert_resource(parts);
        app.world_mut().insert_resource(crate::customiser::Customiser::for_draft(draft));
        until(&mut app, |w| !w.resource::<Parts>().applied.is_null());
        frames(&mut app, 10);
        let pieces: Vec<(String, Entity)> = app.world().resource::<Parts>().instances.iter().map(|(id, e)| (id.clone(), *e)).collect();
        let before: Vec<Vec<Entity>> = pieces.iter().map(|(_, e)| meshes_under(&mut app, *e)).collect();
        app.world_mut().resource_mut::<crate::animation::AnimationStatus>().ready = true; // animation::bind's
        app.world().resource::<AssetServer>().reload(path);
        frames(&mut app, 60);
        for ((id, e), before) in pieces.iter().zip(&before) {
            let now = meshes_under(&mut app, *e);
            assert_eq!(now != *before, id == PANTS, "{id}: only the pants are spawned again");
            assert!(now.iter().all(|&m| app.world().get::<MeshMaterial3d<SkaterMaterial>>(m).is_some()), "{id} wears its material");
        }
        assert!(!app.world().resource::<crate::animation::AnimationStatus>().ready, "the skeleton binds the new joints");
    }
}
#[cfg(test)]
mod eviction_tests {
    use super::*;
    #[test]
    fn closing_the_editor_keeps_only_worn_previews() {
        let mut world = World::new();
        let (worn, browsed) = (world.spawn_empty().id(), world.spawn_empty().id());
        let mut parts = Parts {
            applied: serde_json::json!({"selections":{"Hat":{"asset_id":"cap","material_id":"red"}},"graphics":{"Front":{"logo":"worn_logo"}}}),
            ..default()
        };
        // Item 29: stock logos looked at in the editor (up to 233 of them, ~40 MB with mips) stayed resident for good.
        parts
            .graphics
            .extend(["worn_logo", "browsed_logo"].map(|g| (g.to_owned(), Handle::default())));
        parts
            .instances
            .extend([("cap".to_owned(), worn), ("beanie".to_owned(), browsed)]);
        parts
            .materials
            .extend(["red", "blue"].map(|m| (m.to_owned(), (Handle::default(), vec![]))));
        let mut queue = bevy::ecs::world::CommandQueue::default();
        parts.evict_previews(&mut Commands::new(&mut queue, &world));
        queue.apply(&mut world);
        assert!(world.get_entity(worn).is_ok() && world.get_entity(browsed).is_err());
        assert_eq!(parts.instances.keys().collect::<Vec<_>>(), ["cap"]);
        assert_eq!(parts.materials.keys().collect::<Vec<_>>(), ["red"]);
        assert_eq!(parts.graphics.keys().collect::<Vec<_>>(), ["worn_logo"]);
    }
    #[test]
    fn a_long_browse_keeps_only_the_screen_the_preview_and_the_worn() {
        let mut world = World::new();
        let mut parts = Parts {
            applied: serde_json::json!({"selections":{"Hat":{"asset_id":"cap","material_id":"red"}}}),
            ..default()
        };
        let keep = HashSet::from([("shoe".to_owned(), "white".to_owned())]);
        let mut queue = bevy::ecs::world::CommandQueue::default();
        for n in 0..MAX_PREVIEWS {
            parts.instances.insert(format!("old{n}"), world.spawn_empty().id());
        }
        parts.trim_previews(&mut Commands::new(&mut queue, &world), &keep);
        assert_eq!(parts.instances.len(), MAX_PREVIEWS, "under the cap nothing is freed");
        let (cap, shoe, old) = (world.spawn_empty().id(), world.spawn_empty().id(), parts.instances["old0"]);
        parts.instances.extend([("cap".to_owned(), cap), ("shoe".to_owned(), shoe)]);
        parts.materials.extend(["red", "white", "old"].map(|m| (m.to_owned(), (Handle::default(), vec![]))));
        parts.trim_previews(&mut Commands::new(&mut queue, &world), &keep);
        queue.apply(&mut world);
        let mut left: Vec<_> = parts.instances.keys().cloned().collect();
        left.sort();
        assert_eq!(left, ["cap", "shoe"]);
        assert!(world.get_entity(cap).is_ok() && world.get_entity(shoe).is_ok() && world.get_entity(old).is_err());
        let mut materials: Vec<_> = parts.materials.keys().cloned().collect();
        materials.sort();
        assert_eq!(materials, ["red", "white"]);
    }

    /// User 10-29: a left player's materials go; ours and the shown skaters' stay.
    #[test]
    fn a_left_players_materials_are_freed() {
        let mut parts = Parts { applied: serde_json::json!({"selections": {"Feet": {"asset_id": "shoe", "material_id": "ours"}}}), ..default() };
        parts.materials.extend(["ours", "theirs", "gone"].map(|m| (m.to_owned(), (Handle::default(), vec![]))));
        parts.forget_unworn_materials(&HashSet::from(["theirs"]));
        let mut left: Vec<_> = parts.materials.keys().cloned().collect();
        left.sort();
        assert_eq!(left, ["ours", "theirs"]);
    }
}
