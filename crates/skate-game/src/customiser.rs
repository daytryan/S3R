//! Compact character menus over resident retail parts.
use crate::customiser_parts::{Library, Parts};
use bevy::{input::mouse::MouseWheel, prelude::*};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone, Default, Deserialize)]
pub(crate) struct Entry {
    pub label: String,
    #[serde(default)]
    pub children: Vec<Entry>,
    #[serde(default)]
    pub options: Vec<Entry>,
    pub patch: Option<Value>,
    pub scalar: Option<String>,
    note: Option<String>,
    pub gender: Option<String>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub initial: Option<f64>,
    pub step: Option<f64>,
    /// Menu swatch colour when it differs from what the patch applies (hair).
    pub swatch: Option<[f32; 3]>,
}
#[derive(Resource, Default)]
pub(crate) struct Navigation {
    pub pressed: u16,
    /// The pad's D-pad alone (no stick, no repeat; LB held masks it: LB + D-pad is the session marker's), just
    /// pressed, bits as `pressed`. Not retail (user 10-13): the proposal tab answers on it while skating.
    pub dpad: u16,
    dpad_previous: u16,
    /// LT (1) / RT (2) just pressed, past XInput's trigger threshold (30).
    pub triggers: u8,
    triggers_previous: u8,
    preview_turn: f32,
    previous: u16,
    held_for: f32,
    repeat_at: f32,
}
/// Edit Skater is not open (a run condition).
pub(crate) fn closed(customiser: Option<Res<Customiser>>) -> bool {
    customiser.is_none_or(|c| !c.open)
}
#[derive(Resource)]
pub(crate) struct Customiser {
    pub open: bool,
    pub enabled: bool,
    just_opened: bool,
    preview_yaw: f32,
    index: Entry,
    path: Vec<usize>,
    selected: usize,
    page_size: usize,
    search: String,
    pub draft: Value,
    settings: PathBuf,
    pub status: String,
    /// The look as `settings` holds it (B3, release review 10-18): leaving with the draft unchanged writes nothing, so
    /// defaults shown for a look that didn't load never replace it.
    saved: Value,
    /// Why the saved look didn't load, for the status line when the CAS next opens.
    problem: Option<String>,
    pub redraw: bool,
    /// The original Create-a-Skater screen (create_skater.rs) owns input and
    /// presentation; this panel stays hidden.
    pub original: bool,
    /// The original screen is missing or broken: this panel stands in for it. While false, an open customiser
    /// whose original screen is still loading shows nothing (10-11: this panel flashed up for that second).
    pub fallback: bool,
    /// Set by the original screen: preview framing (height, distance, yaw) and
    /// the clothing slot removed to show tattoos.
    pub view: Option<(f32, f32, f32)>,
    pub strip: Option<&'static str>,
    /// Set by the original screen too: the page's Skate 2 (camera, body state), shown in the Skate 2 rooms
    /// (cas_room::skate2_shot).
    pub skate2: Option<(&'static str, &'static str)>,
    /// Set by the original screen too: Grip Color is up (cas_pose turns the board over in the Skate 2 room).
    pub grip: bool,
}
impl Customiser {
    /// This panel is up and takes input.
    fn classic(&self) -> bool {
        self.open && !self.original && self.fallback
    }
    pub(crate) fn begin(&mut self) {
        self.open = true;
        self.enabled = true;
        self.just_opened = true;
        self.preview_yaw = 0.;
        self.path.clear();
        self.search.clear();
        self.selected = 0;
        self.status = self.problem.take().unwrap_or_default();
        self.redraw = true;
    }
    fn page(&self) -> &Entry {
        let mut page = &self.index;
        for &i in &self.path {
            page = &page.children[i];
        }
        page
    }
    fn visible(&self) -> Vec<usize> {
        let gender = self.draft["gender"].as_str().unwrap_or("male");
        self.page()
            .children
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.gender
                    .as_deref()
                    .is_none_or(|g| g == gender || g == "unisex")
            })
            .filter(|(_, e)| {
                self.search.is_empty()
                    || e.label.to_lowercase().contains(&self.search.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect()
    }
    pub fn preview_camera(&self) -> (f32, f32, f32) {
        let (height, distance, yaw) = self.view.unwrap_or_else(|| self.preview_framing());
        (height, distance, yaw + self.preview_yaw)
    }
    fn preview_framing(&self) -> (f32, f32, f32) {
        let mut page = &self.index;
        let mut labels = vec![];
        for &i in &self.path {
            page = &page.children[i];
            labels.push(page.label.as_str());
        }
        if labels.contains(&"Upper body") || labels.contains(&"Lower body") {
            let upper = labels.contains(&"Upper body");
            let side = self.draft["tattoos"][if upper { "Arm" } else { "Leg" }]["side"]
                .as_u64()
                .unwrap_or(0);
            let yaw = match side {
                0 => -0.65,
                1 => 0.65,
                3 => std::f32::consts::PI,
                _ => 0.,
            };
            return (if upper { 1.15 } else { 0.5 }, 2.2, yaw);
        }
        if labels.iter().any(|s| {
            [
                "Face",
                "Face presets",
                "Hair",
                "Hair colour",
                "Facial hair",
                "Skin tone",
            ]
            .contains(s)
        }) {
            return (1.48, 1.65, 0.);
        }
        if labels.contains(&"Board") {
            return (0.25, 2.0, 0.6);
        }
        (0.95, 3.2, 0.)
    }
    pub fn preview(&self, parts: &Parts) -> Value {
        let mut profile = self.draft.clone();
        if !self.open {
            return profile;
        }
        let mut page = &self.index;
        let mut zone = self.strip;
        let mut tattoos = false;
        for &i in &self.path {
            page = &page.children[i];
            if page.label == "Tattoos" {
                tattoos = true;
            }
            if tattoos && page.label == "Upper body" {
                zone = Some("OuterTorso");
            }
            if tattoos && page.label == "Lower body" {
                zone = Some("Pants");
            }
        }
        if let Some(slot) = zone {
            let gender = profile["gender"].as_str().unwrap_or("male");
            let candidate = parts
                .library
                .models
                .iter()
                .filter(|(_, p)| {
                    p.slot == slot
                        && p.flag("Gender") == gender
                        && (if slot == "OuterTorso" {
                            p.flag("TopType").is_empty()
                        } else {
                            p.flag("IsTattooViewingChoice").eq_ignore_ascii_case("true")
                        })
                })
                .min_by_key(|(id, _)| *id);
            if let Some((id, p)) = candidate {
                profile["selections"][slot] = json!({"asset_id":id,"material_id":p.materials[0]});
                if let Ok(resolved) = parts.resolve(&profile) {
                    return resolved;
                }
            }
        }
        profile
    }
    fn preload_entries(&self) -> Vec<&Entry> {
        if !self.open {
            return vec![];
        }
        let visible = self.visible();
        let start = self.selected / self.page_size * self.page_size;
        visible
            .iter()
            .skip(start)
            .take(self.page_size)
            .flat_map(|&i| {
                let e = &self.page().children[i];
                let current = option_index(e, &self.draft).unwrap_or(0);
                e.options
                    .iter()
                    .enumerate()
                    .filter(move |(i, _)| *i < 4 || i.abs_diff(current) <= 2)
                    .map(|(_, v)| v)
                    .chain(std::iter::once(e))
            })
            .collect()
    }
    pub fn preload_outfits(&self, parts: &Parts) -> Vec<Value> {
        self.preload_entries()
            .into_iter()
            .filter_map(|e| e.patch.as_ref())
            .filter_map(|patch| {
                let mut profile = self.draft.clone();
                if let Some(gender) = patch["gender"].as_str() {
                    profile = parts.library.defaults.get(gender)?.clone();
                } else {
                    merge(&mut profile, patch);
                    if let Some(hair) = patch["selections"].get("Hair") {
                        profile["hair_choice"] = hair.clone();
                    }
                }
                parts.resolve(&profile).ok()
            })
            .collect()
    }
}
#[derive(Component)]
struct Root;
#[derive(Component)]
struct Row(usize);
#[derive(Component)]
struct Adjust(usize, i32);
const BACK: usize = usize::MAX;
const NEXT: usize = usize::MAX - 1;
const PREV: usize = usize::MAX - 2;
const RESET: usize = usize::MAX - 3;
pub(crate) struct CustomiserPlugin;
impl Plugin for CustomiserPlugin {
    fn build(&self, app: &mut App) {
        crate::customiser_material::register(app);
        app.add_plugins(crate::graphic_tile::TilePlugin);
        app.init_resource::<Navigation>()
            .add_observer(crate::customiser_parts::respawned)
            .add_systems(Startup, crate::customiser_parts::setup)
            .add_systems(
                PreUpdate,
                // After this frame's key events: run before them, a press reached `pressed` a frame after
                // `just_pressed`, and readers checking both took it twice.
                navigation.after(bevy::input::InputSystems).before(crate::graphics_menu::MenuInput),
            )
            .add_systems(
                PreUpdate,
                (preferences, hold_pad)
                    .after(crate::map_transition::MapTransitionSet)
                    .before(crate::input::poll_controllers),
            )
            .add_systems(PostStartup, setup)
            .add_systems(
                Update,
                (
                    interact,
                    rotate_preview,
                    crate::customiser_parts::update,
                    draw,
                )
                    .chain()
                    .after(crate::graphics_menu::interact)
                    .before(crate::app::FrameSet::Animation),
            );
    }
}
pub(crate) fn navigation(
    mut nav: ResMut<Navigation>,
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut capabilities: Local<[crate::input::platform::CapabilityCache; 4]>,
    active: Option<Res<crate::input::ActivePad>>,
) {
    let pad = crate::input::platform::active_packet(active.and_then(|a| a.0), &mut capabilities);
    // LT/RT turn the skater ("Rotate Skater"); XInput's trigger threshold (30)
    // keeps a resting trigger from drifting the preview.
    let trigger = |v: u8| ((v as f32 - 30.) / 225.).clamp(0., 1.).sqrt();
    nav.preview_turn = pad.as_ref().map_or(0., |p| {
        trigger(p.state.triggers[1]) - trigger(p.state.triggers[0])
    });
    let (dpad, lb) = pad.as_ref().map_or((0, false), |p| (p.state.buttons & 15, p.state.buttons & 0x0100 != 0));
    nav.dpad = if lb { 0 } else { dpad & !nav.dpad_previous };
    nav.dpad_previous = dpad;
    let triggers = pad.as_ref().map_or(0, |p| u8::from(p.state.triggers[0] > 30) | u8::from(p.state.triggers[1] > 30) << 1);
    nav.triggers = triggers & !nav.triggers_previous;
    nav.triggers_previous = triggers;
    let mut current = pad.map_or(0, |p| {
        p.state.buttons
            | if p.state.left[1] > 16000 {
                1
            } else if p.state.left[1] < -16000 {
                2
            } else {
                0
            }
            | if p.state.left[0] > 16000 {
                8
            } else if p.state.left[0] < -16000 {
                4
            } else {
                0
            }
    });
    for (key, bit) in [
        (KeyCode::ArrowUp, 1),
        (KeyCode::ArrowDown, 2),
        (KeyCode::ArrowLeft, 4),
        (KeyCode::ArrowRight, 8),
    ] {
        if keys.pressed(key) {
            current |= bit;
        }
    }
    nav.pressed = current & !nav.previous;
    let (delay, rate) = nav_repeat();
    if current & 15 != 0 && current & 15 == nav.previous & 15 {
        nav.held_for += time.delta_secs();
        if nav.held_for >= nav.repeat_at {
            nav.pressed |= current & 15;
            nav.repeat_at += rate;
        }
    } else {
        nav.held_for = 0.;
        nav.repeat_at = delay;
    }
    nav.previous = current;
}
/// perf 10-18: held-direction repeat (first repeat, then every), port-chosen; was 0.35 / 0.09 s, which felt sluggish.
/// SKATE_NAV_REPEAT=delay,rate overrides (0.35,0.09 restores the old feel).
fn nav_repeat() -> (f32, f32) {
    static REPEAT: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();
    *REPEAT.get_or_init(|| parse_repeat(std::env::var("SKATE_NAV_REPEAT").ok().as_deref()))
}
fn parse_repeat(value: Option<&str>) -> (f32, f32) {
    let parsed = value.and_then(|v| v.split_once(',')).and_then(|(d, r)| Some((d.trim().parse().ok()?, r.trim().parse().ok()?)));
    parsed.filter(|&(d, r): &(f32, f32)| d > 0. && r > 0.).unwrap_or((0.25, 0.06))
}
// ponytail: tuned by feel (~1 turn per 0.9s at full trigger); adjust if retail capture says otherwise.
const PREVIEW_TURN_RATE: f32 = 7.0;
fn rotate_preview(
    mut state: ResMut<Customiser>,
    nav: Res<Navigation>,
    time: Res<Time<Real>>,
    creator: Option<Res<crate::graphic_creator::Creator>>,
) {
    // The Graphic Creator's board uses LT / RT (fine, move layer) everywhere but its Add page. The open colour
    // wheel leaves them turning the skater; its brightness is the right stick (user 10-13).
    let owned = creator.is_some_and(|c| c.owns_triggers());
    if state.open && nav.preview_turn != 0. && !owned {
        // Real time keeps inspection responsive while gameplay is paused.
        state.preview_yaw = (state.preview_yaw
            + nav.preview_turn * PREVIEW_TURN_RATE * time.delta_secs().min(0.1))
        .rem_euclid(std::f32::consts::TAU);
    }
}

fn page(label: impl Into<String>, children: Vec<Entry>) -> Entry {
    Entry {
        label: label.into(),
        children,
        ..default()
    }
}
fn choice(label: impl Into<String>, patch: Value) -> Entry {
    Entry {
        label: label.into(),
        patch: Some(patch),
        ..default()
    }
}
fn tidy(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|x| x.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn menu(lib: &Library, extras: Vec<Entry>) -> Entry {
    let model_page = |title: &str, slot: &str, types: &[&str]| {
        let mut entries = vec![];
        if [
            "Hat",
            "Hair",
            "Sock",
            "Glasses",
            "Jewellery",
            "WristItem",
            "Accessory",
        ]
        .contains(&slot)
            || crate::customiser_parts::ACCESSORY_SLOTS.iter().any(|(_, s)| *s == slot)
        {
            entries.push(choice("None", json!({"selections":{slot:Value::Null}})));
        }
        if title == "T-shirts" {
            for (id, p) in &lib.models {
                if p.slot == "OuterTorso" && p.flag("TopType").is_empty() {
                    entries.push(Entry { gender:Some(p.flag("Gender").into()),
                        ..choice("No top",json!({"selections":{"OuterTorso":{"asset_id":id,"material_id":p.materials[0]}}})) });
                }
            }
        }
        let mut models: Vec<_> = lib
            .models
            .iter()
            .filter(|(_, p)| {
                p.slot == slot
                    && (slot != "Hair" || p.flag("HairModelType") == "full")
                    && (types.is_empty() || types.contains(&p.flag("TopType")))
            })
            .collect();
        models.sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.0.cmp(b.0)));
        for (id, p) in models {
            let options: Vec<_> = p
                .materials
                .iter()
                .flat_map(|mid| {
                    let Some(m)=lib.materials.get(mid) else {return vec![];};
                    let mut options=vec![choice(&m.name,json!({"selections":{slot:{"asset_id":id,"material_id":mid}},"colours":{slot:Value::Null}}))];
                    if m.flag("IsColourizable").eq_ignore_ascii_case("true") {
                        options.extend(lib.colours.iter().map(|c| choice(&c.name,
                            json!({"selections":{slot:{"asset_id":id,"material_id":mid}},"colours":{slot:c.rgb}}))));
                    }
                    options
                })
                .collect();
            if options.is_empty() {
                continue;
            }
            let gender = p.flag("Gender");
            entries.push(Entry {
                label: if slot == "Hair" {
                    tidy(p.flag("HairStyle").replace("moosed", "moussed").as_str())
                } else {
                    p.name.clone()
                },
                options,
                gender: if gender.is_empty() {
                    None
                } else {
                    Some(gender.into())
                },
                ..default()
            });
        }
        if ["SkateBoard", "SkateTruck", "SkateWheel"].contains(&slot) {
            let mut groups: std::collections::BTreeMap<String, (String, Vec<(Option<bool>, Entry)>)> = default();
            for e in entries {
                for option in e.options {
                    let (key, base, worn) = wear_pair(&option.label);
                    let group = groups.entry(key).or_insert_with(|| (base.to_owned(), Vec::new()));
                    group.1.push((worn, option));
                }
            }
            entries = groups
                .into_values()
                .map(|(label, members)| {
                    // Skate 1 marks only the clean print: its unmarked sibling is the worn one.
                    let paired = members.iter().any(|(w, _)| *w == Some(false));
                    let mut options: Vec<Entry> = members
                        .into_iter()
                        .map(|(w, mut option)| {
                            let worn = w.unwrap_or(paired);
                            option.label = if worn { "Worn" } else { "New" }.into();
                            option
                        })
                        .collect();
                    // New first: the grid tile and the first style.
                    options.sort_by_key(|o| o.label == "Worn");
                    Entry {
                        label,
                        options,
                        ..default()
                    }
                })
                .collect();
        }
        page(title, entries)
    };
    let morph = |title: &str, name: &str| Entry {
        label: title.into(),
        scalar: Some(format!("morphs.{name}")),
        minimum: Some(0.),
        maximum: Some(0.5),
        initial: Some(if name == "fat" || name == "thin" {
            0.
        } else {
            0.25
        }),
        step: Some(0.025),
        ..default()
    };
    let mut body = page(
        "Body",
        vec![
            page(
                "Gender",
                vec![
                    choice("Male", json!({"gender":"male"})),
                    choice("Female", json!({"gender":"female"})),
                ],
            ),
            page(
                "Skin tone",
                ["light", "dark"]
                    .map(|s| choice(tidy(s), json!({"skin":s})))
                    .to_vec(),
            ),
            model_page("Hair", "Hair", &[]),
            page(
                "Build",
                vec![morph("Weight", "fat"), morph("Definition", "thin")],
            ),
            page(
                "Face",
                vec![
                    page(
                        "Brows",
                        vec![
                            morph("Depth", "local_brows_depth"),
                            morph("Height", "local_brows_height"),
                            morph("Angle", "local_brows_rotation"),
                        ],
                    ),
                    page(
                        "Eyes",
                        vec![
                            morph("Height", "local_eye_height"),
                            morph("Angle", "local_eye_rotation"),
                            morph("Width", "local_eye_width"),
                        ],
                    ),
                    page(
                        "Nose",
                        vec![
                            morph("Curve", "local_nose_curve"),
                            morph("Height", "local_nose_height"),
                            morph("Length", "local_nose_length"),
                            morph("Width", "local_nose_width"),
                        ],
                    ),
                    page(
                        "Mouth",
                        vec![
                            morph("Corners", "local_mouth_corner"),
                            morph("Height", "local_mouth_height"),
                            morph("Fullness", "local_mouth_lipsize"),
                            morph("Width", "local_mouth_width"),
                        ],
                    ),
                    page(
                        "Jaw & chin",
                        vec![
                            morph("Definition", "local_jaw_chiseled"),
                            morph("Depth", "local_jaw_depth"),
                            morph("Chin length", "local_chin_length"),
                        ],
                    ),
                ],
            ),
            page("Facial hair", {
                let mut names: Vec<_> = lib
                    .materials
                    .values()
                    .map(|m| m.flag("FacialHairStyle"))
                    .filter(|s| !s.is_empty())
                    .collect();
                names.sort();
                names.dedup();
                names
                    .into_iter()
                    .map(|s| Entry {
                        gender: Some("male".into()),
                        ..choice(
                            match s {
                                "none" => "Clean shaven".into(),
                                "billygoat" => "Long goatee".into(),
                                "soulpatch" => "Soul patch".into(),
                                "magnum" => "Thick moustache".into(),
                                _ => tidy(s),
                            },
                            json!({"beard":s}),
                        )
                    })
                    .collect()
            }),
        ],
    );
    let mut tattoos: Vec<_> = lib.tattoos.iter().collect();
    tattoos.sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.0.cmp(b.0)));
    body.children.push(page(
        "Tattoos",
        [("Upper body", "Arm"), ("Lower body", "Leg")]
            .into_iter()
            .map(|(name, slot)| {
                let mut designs =
                    vec![choice("None", json!({"tattoos":{slot:{"id":Value::Null}}}))];
                designs.extend(
                    tattoos
                        .iter()
                        .map(|(id, t)| choice(&t.name, json!({"tattoos":{slot:{"id":id}}}))),
                );
                page(
                    name,
                    vec![
                        page("Design", designs),
                        page("Placement", {
                            let mut positions = vec![
                                choice(
                                    if slot == "Arm" {
                                        "Left arm"
                                    } else {
                                        "Left leg"
                                    },
                                    json!({"tattoos":{slot:{"side":0}}}),
                                ),
                                choice(
                                    if slot == "Arm" {
                                        "Right arm"
                                    } else {
                                        "Right leg"
                                    },
                                    json!({"tattoos":{slot:{"side":1}}}),
                                ),
                            ];
                            if slot == "Arm" {
                                positions.extend([
                                    choice("Chest", json!({"tattoos":{slot:{"side":2}}})),
                                    choice("Back", json!({"tattoos":{slot:{"side":3}}})),
                                ]);
                            }
                            positions
                        }),
                    ],
                )
            })
            .collect(),
    ));
    let clothes = page(
        "Clothes",
        vec![
            model_page("Hats", "Hat", &[]),
            model_page(
                "T-shirts",
                "OuterTorso",
                &["tshirt", "ls_tshirt", "tanktop"],
            ),
            model_page("Shirts", "OuterTorso", &["buttonshirt"]),
            model_page("Hoodies", "OuterTorso", &["hoody"]),
            model_page("Jackets", "OuterTorso", &["jacket"]),
            model_page("Sweaters", "OuterTorso", &["sweater"]),
            model_page("Pants & shorts", "Pants", &[]),
            model_page("Shoes", "Feet", &[]),
            model_page("Socks", "Sock", &[]),
            page(
                "Accessories",
                vec![
                    model_page("Glasses", "Glasses", &[]),
                    model_page("Necklaces", "Jewellery", &[]),
                    model_page("Wristwear", "WristItem", &[]),
                    // Retail's Accessory slot holds rings only (Accessories > Rings). The ported
                    // kinds each have their own slot and tab (customiser_parts::ACCESSORY_SLOTS).
                    model_page("Rings", "Accessory", &[]),
                    model_page("Belts", "Belt", &[]),
                    model_page("Chains", "Chain", &[]),
                    model_page("Backpacks", "Backpack", &[]),
                    model_page("Other", "AccessoryOther", &[]),
                    model_page("Gloves", "Gloves", &[]),
                ],
            ),
        ],
    );
    let board = page(
        "Board",
        vec![
            model_page("Deck", "SkateBoard", &[]),
            model_page("Trucks", "SkateTruck", &[]),
            model_page("Wheels", "SkateWheel", &[]),
        ],
    );
    // ponytail: the retail top level (Body Mods, Merchandise, Edit Style, Trucks,
    // Wheels, per the owned string table); board tuning gets its own pages.
    let truck = page(
        "Trucks adjustment",
        vec![Entry {
            label: "Truck tightness".into(),
            scalar: Some("truck".into()),
            minimum: Some(0.),
            maximum: Some(1.),
            initial: Some(0.7),
            step: Some(0.1),
            ..default()
        }],
    );
    let wheel = page(
        "Wheels adjustment",
        vec![Entry {
            label: "Wheel hardness".into(),
            scalar: Some("wheel".into()),
            minimum: Some(0.),
            maximum: Some(1.),
            initial: Some(0.7),
            step: Some(0.1),
            ..default()
        }],
    );
    let mut clothes = clothes;
    clothes.children.push(board);
    let mut root = page(
        "Your skater",
        vec![
            body,
            clothes,
            page(
                "Style",
                vec![page(
                    "Posture",
                    ["Default", "Stiff", "Slouch", "Buff"]
                        .iter()
                        .enumerate()
                        .map(|(i, s)| choice(*s, json!({"posture":i})))
                        .collect(),
                )],
            ),
            truck,
            wheel,
        ],
    );
    for e in extras {
        if e.label == "Style" {
            root.children[2].children.extend(e.children);
        } else if let Some(i) = root.children[0]
            .children
            .iter()
            .position(|p| p.label == e.label)
        {
            root.children[0].children[i] = e;
        } else {
            root.children[0].children.push(e);
        }
    }
    root
}
/// B3 (release review 10-18): a saved look the parts refuse (an item no longer installed, a hand edit) is kept aside
/// as `<name>.bad-<unix>`, never overwritten by the defaults shown in its place. The player's line.
/// With no parts library (a broken install) every look is refused: it is left in place and not saved over instead.
pub(crate) fn refused(path: &std::path::Path, error: &str, parts: &Parts) -> String {
    if parts.library.models.is_empty() {
        error!("Saved look {}: refused ({error}) with no parts library; left as it is and not saved over", path.display());
        crate::saves::block(path);
        return "Your saved look couldn't be loaded: the skater parts are missing. Run the launcher's setup again".into();
    }
    match crate::saves::set_aside(path) {
        Ok(bad) => {
            error!("Saved look {}: refused ({error}); kept as {}", path.display(), bad.display());
            format!("Your saved look couldn't be loaded; it is kept as {}", bad.file_name().unwrap_or_default().to_string_lossy())
        }
        Err(e) => {
            error!("Saved look {}: refused ({error}) and could not be kept aside ({e})", path.display());
            "Your saved look couldn't be loaded".into()
        }
    }
}
fn setup(mut commands: Commands, config: Res<crate::config::Config>, parts: Res<Parts>) {
    // The look of the team member the player skates as (Edit Skaters' Select Skater): the boss's character.json
    // unless a teammate was selected.
    let settings = crate::team_data::active_look_path(
        &crate::saves::settings_dir(&config.asset_root),  // Not retail (user 10-13)
    );
    let mut draft = parts
        .library
        .defaults
        .get("male")
        .cloned()
        .unwrap_or(json!({"selections":{},"morphs":{}}));
    let mut enabled = false;
    let status = String::new();
    let (look, mut problem) = match crate::saves::load_json::<Value>(&settings) {
        crate::saves::Loaded::Loaded(look) => (Some(look), None),
        crate::saves::Loaded::Restored(look, _) => (Some(look), Some("Your saved look was damaged; it was restored from its backup".to_owned())),
        loaded => (None, loaded.problem()),
    };
    if let Some(look) = look {
        match parts.resolve(&look) {
            Ok(p) => {
                draft = p;
                enabled = true;
            }
            Err(e) => problem = Some(refused(&settings, &e, &parts)),
        }
    }
    let extras = std::fs::read(
        crate::customiser_parts::asset_directory(&config.asset_root).join("extra-menu.json"),
    )
    .ok()
    .and_then(|b| serde_json::from_slice(&b).ok())
    .unwrap_or_default();
    commands.insert_resource(Customiser {
        open: false,
        enabled,
        just_opened: false,
        preview_yaw: 0.,
        index: menu(&parts.library, extras),
        path: vec![],
        selected: 0,
        page_size: 7,
        search: String::new(),
        saved: draft.clone(),
        problem,
        draft,
        settings,
        status,
        redraw: true,
        original: false,
        fallback: true,
        view: None,
        strip: None,
        skate2: None,
        grip: false,
    });
    commands.spawn((
        Root,
        GlobalZIndex(20),
        Node {
            display: Display::None,
            position_type: PositionType::Absolute,
            left: px(24),
            top: percent(4),
            width: percent(38),
            min_width: px(370),
            max_width: px(520),
            height: percent(92),
            padding: UiRect::all(px(22)),
            flex_direction: FlexDirection::Column,
            row_gap: px(10),
            border_radius: BorderRadius::all(px(18)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.025, 0.035, 0.048, 0.98)),
    ));
}
fn merge(target: &mut Value, patch: &Value) {
    if let (Some(a), Some(b)) = (target.as_object_mut(), patch.as_object()) {
        for (k, v) in b {
            merge(a.entry(k.clone()).or_insert(Value::Null), v);
        }
    } else {
        *target = patch.clone();
    }
}
fn scalar(profile: &Value, key: &str, initial: f64) -> f64 {
    profile
        .pointer(&format!("/{}", key.replace('.', "/")))
        .and_then(Value::as_f64)
        .unwrap_or(initial)
}
/// Prints whose names do not show their pair: Skate 1 rider decks that are a second worn copy of
/// the team deck, and Skate 3's misspelt worn Hom.
const WEAR_ALIASES: [(&str, &str); 4] = [
    ("Board Blacklabel Freeway Wh 01", "Board Blacklabel Wh 01"),
    ("Board Mystery Sammy Wh 01", "Board Mystery Wh 01"),
    ("Board Zero Mike Bl 01", "Board Zero Bl 01"),
    ("Homl (Worn)", "Hom (Worn)"),
];
/// A board part's new and worn prints are one item: Skate 3 names them "X" / "X (Worn)" (plain
/// decks "White 1" / "White Dirty 1 (Worn)"), the Skate 1 port "Board ... Bk02C" (clean) /
/// "Board ... Bk 02" (worn). Returns the pairing key (spacing ignored), the display name and
/// Some(worn) when the name itself says which it is.
fn wear_pair(name: &str) -> (String, &str, Option<bool>) {
    let name = WEAR_ALIASES.iter().find(|a| a.0 == name).map_or(name, |a| a.1);
    let key = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(base) = name.strip_suffix(" (Worn)") {
        return (key(&base.replace(" Dirty ", " ")), base, Some(true));
    }
    let b = name.as_bytes();
    if b.len() > 2 && b[b.len() - 1] == b'C' && b[b.len() - 2].is_ascii_digit() {
        let base = &name[..name.len() - 1];
        return (key(base), base, Some(false));
    }
    (key(name), name, None)
}
fn option_index(entry: &Entry, profile: &Value) -> Option<usize> {
    entry.options.iter().position(|o| {
        o.patch.as_ref().is_some_and(|p| {
            p.as_object().is_some_and(|fields| {
                fields.iter().all(|(key, value)| {
                    if key == "selections" {
                        value.as_object().is_some_and(|selections| {
                            selections.iter().all(|(slot, v)| {
                                (if slot == "Hair" {
                                    profile
                                        .get("hair_choice")
                                        .unwrap_or(&profile["selections"][slot])
                                } else {
                                    &profile["selections"][slot]
                                }) == v
                            })
                        })
                    } else {
                        matches_patch(&profile[key], value)
                    }
                })
            })
        })
    })
}

/// Whether an open customiser pauses (`Some(true)`) or runs (`Some(false)`) the world; `None` leaves it to the others.
/// The camera and the room are placed from the presentation's frames (camera::present, cas_room::stage), so one
/// opened before the world's first tick (the Skate 1 tutorial's frontend, the first launch's CAS) runs it until a
/// frame exists, then holds the skater still. Online (user 10-16) it never pauses: the session runs on underneath, as
/// the online crossbar's does, and the skater is held by its pad instead (`hold_pad`).
fn world_pause(open: bool, framed: bool, online: bool) -> Option<bool> {
    open.then_some(framed && !online)
}

/// Online, an open CAS takes the pad from the skater (the world runs: `world_pause`), as online_chat's typing bar does.
fn hold_pad(state: Res<Customiser>, net: Option<Res<crate::multiplayer::Multiplayer>>, input: Option<ResMut<crate::input::ControllerInput>>) {
    let hold = state.open && net.is_some_and(|n| n.active());
    if let Some(mut input) = input.filter(|i| i.cas != hold) {
        input.cas = hold;
    }
}

fn interact(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    nav: Res<Navigation>,
    mut wheel: MessageReader<MouseWheel>,
    mut typing: MessageReader<bevy::input::keyboard::KeyboardInput>,
    mut state: ResMut<Customiser>,
    parts: Res<Parts>,
    mut pause: ResMut<crate::graphics_menu::Menu>,
    buttons: Query<(&Interaction, &Row)>,
    adjustments: Query<(&Interaction, &Adjust)>,
    mut time: ResMut<Time<Virtual>>,
    history: Option<Res<crate::presentation::Presentation>>,
    net: Option<Res<crate::multiplayer::Multiplayer>>,
) {
    // Either screen holds the skater still, however it was opened (the Skate 2 tutorial opens it outside the
    // pause menu). Closing clears pause.open, and graphics_menu::interact unpauses.
    match world_pause(state.open, history.is_none_or(|h| h.pair().is_some()), net.is_some_and(|n| n.active())) {
        Some(true) => time.pause(),
        Some(false) => time.unpause(),
        None => {}
    }
    if !state.classic() {
        wheel.clear();
        typing.clear();
        return;
    }
    if state.just_opened {
        state.just_opened = false;
        wheel.clear();
        typing.clear();
        return;
    }
    if state.page().children.iter().all(|e| e.children.is_empty()) {
        for event in typing.read() {
            if event.state == bevy::input::ButtonState::Pressed
                && !keys.pressed(KeyCode::ControlLeft)
                && !keys.pressed(KeyCode::ControlRight)
            {
                if let Some(text) = &event.text {
                    for c in text
                        .chars()
                        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
                    {
                        if state.search.chars().count() < 32 {
                            state.search.push(c);
                            state.selected = 0;
                            state.redraw = true;
                        }
                    }
                }
            }
        }
    } else {
        typing.clear();
    }
    if !state.search.is_empty() {
        if keys.just_pressed(KeyCode::Backspace) {
            state.search.pop();
            state.selected = 0;
            state.redraw = true;
            return;
        }
        if keys.just_pressed(KeyCode::Escape) || nav.pressed & 0x2000 != 0 {
            state.search.clear();
            state.selected = 0;
            state.redraw = true;
            return;
        }
    }
    let visible = state.visible();
    let count = visible.len();
    let mut action = None;
    let mut back = keys.just_pressed(KeyCode::Escape)
        || keys.just_pressed(KeyCode::Backspace)
        || nav.pressed & 0x2000 != 0;
    let mut movement = 0;
    if keys.just_pressed(KeyCode::ArrowUp) || nav.pressed & 1 != 0 {
        movement -= 1;
    }
    if keys.just_pressed(KeyCode::ArrowDown) || nav.pressed & 2 != 0 {
        movement += 1;
    }
    for scroll in wheel.read() {
        if scroll.y > 0. {
            movement -= 1;
        } else if scroll.y < 0. {
            movement += 1;
        }
    }
    if keys.just_pressed(KeyCode::PageDown) {
        movement += state.page_size as i32;
    }
    if keys.just_pressed(KeyCode::PageUp) {
        movement -= state.page_size as i32;
    }
    if count > 0 {
        if movement != 0 {
            state.selected = (state.selected as i32 + movement).rem_euclid(count as i32) as usize;
            state.redraw = true;
        }
        if keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0 {
            action = Some((visible[state.selected], 0));
        }
        if keys.just_pressed(KeyCode::ArrowRight) || nav.pressed & 8 != 0 {
            action = Some((visible[state.selected], 1));
        }
        if keys.just_pressed(KeyCode::ArrowLeft) || nav.pressed & 4 != 0 {
            action = Some((visible[state.selected], -1));
        }
    }
    if mouse.just_pressed(MouseButton::Left) {
        for (interaction, row) in &buttons {
            if *interaction == Interaction::Pressed {
                match row.0 {
                    BACK => back = true,
                    NEXT => {
                        if count > 0 {
                            state.selected = (state.selected + state.page_size).min(count - 1);
                            state.redraw = true;
                        }
                    }
                    PREV => {
                        state.selected = state.selected.saturating_sub(state.page_size);
                        state.redraw = true;
                    }
                    RESET => {
                        let g = state.draft["gender"].as_str().unwrap_or("male");
                        if let Some(p) = parts.library.defaults.get(g) {
                            state.draft = p.clone();
                            state.redraw = true;
                        }
                    }
                    i => {
                        state.selected = visible.iter().position(|v| *v == i).unwrap_or(0);
                        action = Some((i, 0));
                    }
                }
            }
        }
    }
    if mouse.just_pressed(MouseButton::Left) {
        for (interaction, adjust) in &adjustments {
            if *interaction == Interaction::Pressed {
                state.selected = visible.iter().position(|i| *i == adjust.0).unwrap_or(0);
                action = Some((adjust.0, adjust.1));
            }
        }
    }
    if back {
        state.leave();
        if !state.open {
            pause.cas_closed();
        }
        return;
    }
    if let Some((i, direction)) = action {
        state.step(&parts, i, direction);
    }
}
/// Whether `profile` already has everything `patch` sets.
pub(crate) fn patch_selected(profile: &Value, patch: &Value) -> bool {
    option_index(
        &Entry {
            options: vec![Entry {
                patch: Some(patch.clone()),
                ..default()
            }],
            ..default()
        },
        profile,
    )
    .is_some()
}
pub(crate) fn scalar_of(profile: &Value, key: &str, initial: f64) -> f64 {
    scalar(profile, key, initial)
}
impl Customiser {
    /// Test hook: a closed customiser holding `draft`.
    #[cfg(test)]
    pub(crate) fn for_draft(draft: Value) -> Self {
        Self {
            open: false,
            enabled: true,
            just_opened: false,
            preview_yaw: 0.,
            index: Entry::default(),
            path: vec![],
            selected: 0,
            page_size: 6,
            search: String::new(),
            draft,
            settings: PathBuf::new(),
            status: String::new(),
            saved: Value::Null,
            problem: None,
            redraw: false,
            original: true,
            fallback: false,
            view: None,
            strip: None,
            skate2: None,
            grip: false,
        }
    }
    /// Test hook: where the profile and the files beside it live.
    #[cfg(test)]
    pub(crate) fn set_profile_path(&mut self, path: PathBuf) {
        self.settings = path;
    }
    #[cfg(test)]
    pub(crate) fn for_test(parts: &Parts, extras: Vec<Entry>, gender: &str) -> Self {
        Self {
            open: true,
            enabled: true,
            just_opened: false,
            preview_yaw: 0.,
            index: menu(&parts.library, extras),
            path: vec![],
            selected: 0,
            page_size: 7,
            search: String::new(),
            draft: parts.library.defaults[gender].clone(),
            settings: PathBuf::new(),
            status: String::new(),
            saved: Value::Null,
            problem: None,
            redraw: false,
            original: true,
            fallback: false,
            view: None,
            strip: None,
            skate2: None,
            grip: false,
        }
    }
    /// The menu entry at a label path from the root (e.g. ["Body", "Hair"]).
    pub(crate) fn entry(&self, labels: &[&str]) -> Option<&Entry> {
        labels.iter().try_fold(&self.index, |e, l| {
            e.children.iter().find(|c| c.label == *l)
        })
    }
    pub(crate) fn gender(&self) -> &str {
        self.draft["gender"].as_str().unwrap_or("male")
    }
    /// Children shown for the skater's gender.
    pub(crate) fn for_gender<'a>(&self, entry: &'a Entry) -> Vec<&'a Entry> {
        let gender = self.gender();
        entry
            .children
            .iter()
            .filter(|e| {
                e.gender
                    .as_deref()
                    .is_none_or(|g| g == gender || g == "unisex")
            })
            .collect()
    }
    /// Set a scalar (e.g. "morphs.fat", "truck", "graphics.front.size") and re-resolve.
    pub(crate) fn set_scalar(&mut self, parts: &Parts, key: &str, value: f64) {
        let mut profile = self.draft.clone();
        *key.split('.').fold(&mut profile, |v, k| &mut v[k]) = json!(value);
        self.commit(parts, profile);
    }
    /// The saved skater profile (My Graphics lives beside it).
    pub(crate) fn profile_path(&self) -> &std::path::Path {
        &self.settings
    }
    /// Edit Skaters: edit another team member (its profile file and resolved look) in place of the player's;
    /// returns what it replaced, to swap back when the CAS closes (team_select.rs).
    pub(crate) fn swap_profile(&mut self, path: PathBuf, look: Value) -> (PathBuf, Value) {
        self.redraw = true;
        self.saved = look.clone();
        (std::mem::replace(&mut self.settings, path), std::mem::replace(&mut self.draft, look))
    }
    pub(crate) fn save_and_close(&mut self) {
        self.path.clear();
        self.leave();
    }
    /// Close keeping the saved profile on disk as it was.
    pub(crate) fn close_without_saving(&mut self) {
        self.path.clear();
        self.search.clear();
        self.open = false;
        self.redraw = true;
    }
    /// Back one level; at the root, save the profile and close (callers then
    /// resume skating).
    pub(crate) fn leave(&mut self) {
        self.search.clear();
        if let Some(i) = self.path.pop() {
            self.selected = self.visible().iter().position(|v| *v == i).unwrap_or(0);
        } else if self.draft == self.saved {
            self.open = false;  // nothing changed: nothing written (B3)
        } else {
            let save = (|| -> Result<(), String> {
                crate::saves::write_atomic(
                    &self.settings,
                    &serde_json::to_vec_pretty(&self.draft).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())
            })();
            match save {
                Ok(()) => {
                    crate::autosave_tag::saving();
                    self.saved = self.draft.clone();
                    self.open = false
                }
                Err(e) => self.status = format!("Could not save: {e}"),
            }
        }
        self.redraw = true;
    }
    /// Activate child `i` of the page: open it, or apply/adjust its value
    /// (`direction` 0 = select, +-1 = next/previous option or slider step).
    pub(crate) fn step(&mut self, parts: &Parts, i: usize, direction: i32) {
        let Some(entry) = self.page().children.get(i).cloned() else {
            return;
        };
        if !entry.children.is_empty() {
            self.search.clear();
            self.path.push(i);
            self.selected = 0;
            self.status.clear();
        } else if let Some(note) = entry.note {
            self.status = note;
        } else if let Some(key) = entry.scalar {
            let mut profile = self.draft.clone();
            let amount = if direction < 0 { -1. } else { 1. };
            let val = (scalar(&profile, &key, entry.initial.unwrap_or(0.))
                + amount * entry.step.unwrap_or(0.1))
            .clamp(entry.minimum.unwrap_or(0.), entry.maximum.unwrap_or(1.));
            if let Some((p, c)) = key.split_once('.') {
                profile[p][c] = json!(val);
            } else {
                profile[&key] = json!(val);
            }
            self.commit(parts, profile);
        } else {
            let patch = if entry.options.is_empty() {
                entry.patch
            } else {
                let index = match option_index(&entry, &self.draft) {
                    Some(i) => (i as i32 + if direction == 0 { 1 } else { direction })
                        .rem_euclid(entry.options.len() as i32)
                        as usize,
                    None => 0,
                };
                entry.options[index].patch.clone()
            };
            self.apply(parts, patch.as_ref());
        }
        self.redraw = true;
    }
    /// Apply one authored choice (a child's or option's patch).
    pub(crate) fn apply(&mut self, parts: &Parts, patch: Option<&Value>) {
        let mut profile = self.draft.clone();
        if let Some(patch) = patch {
            if let Some(gender) = patch["gender"].as_str() {
                if let Some(default) = parts.library.defaults.get(gender) {
                    profile = default.clone();
                }
            } else {
                // Rings go under gloves: gloves take them off, and with gloves on a ring is not put on.
                let wears = |p: &Value, slot: &str| !p["selections"][slot].is_null();
                if wears(patch, "Accessory") && wears(&profile, "Gloves") && !wears(patch, "Gloves") {
                    return;
                }
                merge(&mut profile, patch);
                if wears(patch, "Gloves") && profile["selections"].is_object() {
                    profile["selections"]["Accessory"] = Value::Null;
                }
                if let Some(hair) = patch["selections"].get("Hair") {
                    profile["hair_choice"] = hair.clone();
                }
            }
        }
        self.commit(parts, profile);
    }
    fn commit(&mut self, parts: &Parts, profile: Value) {
        match parts.resolve(&profile) {
            Ok(p) => {
                self.draft = p;
                self.status.clear();
            }
            Err(e) => self.status = e,
        }
        self.redraw = true;
    }
}
fn preferences(
    state: Res<Customiser>,
    models: Res<crate::custom_models::CustomModels>,
    mut physics: ResMut<crate::physics::GamePhysics>,
    mut skater: ResMut<crate::physics::SkaterRuntime>,
) {
    apply_preferences(&state.draft, &mut physics, &mut skater.animation);
    if let Some(style) = models.native_style() {
        skater.animation.motion.playback_context.pro_skater =
            skate_core::animation::skeleton_input::name::encode(style.as_bytes());
        skater.animation.motion.animation.posture.set_profile(0);
        physics.set_gesture_preferences(None);
    }
}
pub(crate) fn apply_preferences(
    profile: &Value,
    physics: &mut crate::physics::GamePhysics,
    animation: &mut crate::skater_animation::SkaterAnimation,
) {
    physics.set_equipment_preferences(
        scalar(profile, "truck", 0.7) as f32,
        scalar(profile, "wheel", 0.7) as f32,
    );
    physics.set_gesture_preferences(profile["gestures"].as_object().map(|g| {
        // ResetGestureSet824FA730 marks all 37 entries available and selects
        // the first four in table order for Up,Down,Left,Right.
        std::array::from_fn(|i| {
            g.get(&i.to_string())
                .and_then(Value::as_u64)
                .unwrap_or(i as u64) as u32
        })
    }));
    animation.set_customisation(
        scalar(profile, "stance", 1.) as u32,
        scalar(profile, "style", 0.) as u32,
    );
    animation
        .motion
        .animation
        .posture
        .set_profile(scalar(profile, "posture", 0.) as u32);
}
fn draw(
    mut commands: Commands,
    mut state: ResMut<Customiser>,
    window: Single<&Window>,
    mut root: Single<(Entity, &mut Node), With<Root>>,
) {
    // Writing a Node marks it changed and reruns UI layout; only write a new value.
    let display = if state.classic() {
        Display::Flex
    } else {
        Display::None
    };
    if root.1.display != display {
        root.1.display = display;
    }
    if !state.classic() {
        return;
    }
    let size = (((window.height() * 0.92 - 255.) / 90.).floor() as usize).clamp(1, 10);
    if size != state.page_size {
        state.page_size = size;
        state.redraw = true;
    }
    if !state.redraw {
        return;
    }
    state.redraw = false;
    commands.entity(root.0).despawn_children();
    let page = state.page();
    let visible = state.visible();
    let start = state.selected / state.page_size * state.page_size;
    let subtitle = if !state.search.is_empty() {
        format!("Search: {}", state.search)
    } else if state.path.is_empty() {
        "Make it yours".to_owned()
    } else {
        let mut p = &state.index;
        let mut crumbs = vec![];
        for &i in &state.path {
            crumbs.push(p.label.clone());
            p = &p.children[i];
        }
        crumbs.join(" / ")
    };
    commands.entity(root.0).with_children(|p| {
        p.spawn((
            Text::new(subtitle),
            TextFont {
                font_size: 13.,
                ..default()
            },
            TextColor(Color::srgb(0.48, 0.62, 0.68)),
        ));
        p.spawn((
            Text::new(&page.label),
            TextFont {
                font_size: 29.,
                ..default()
            },
            TextColor(Color::WHITE),
        ));
        p.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(6),
            flex_grow: 1.,
            min_height: px(0),
            ..default()
        })
        .with_children(|list| {
            if visible.is_empty() {
                list.spawn((
                    Text::new("No matches. Esc clears your search."),
                    TextFont {
                        font_size: 16.,
                        ..default()
                    },
                    TextColor(Color::WHITE),
                ));
            }
            for (pos, &i) in visible.iter().enumerate().skip(start).take(state.page_size) {
                let e = &page.children[i];
                let active = option_index(e, &state.draft);
                let detail = if let Some(key) = &e.scalar {
                    let v = scalar(&state.draft, key, e.initial.unwrap_or(0.));
                    format!(
                        "{}%   − / +",
                        ((v - e.minimum.unwrap_or(0.))
                            / (e.maximum.unwrap_or(1.) - e.minimum.unwrap_or(0.))
                            * 100.)
                            .round() as i32
                    )
                } else if !e.options.is_empty() {
                    let index = active.unwrap_or(0);
                    format!(
                        "{}  ·  {}/{}",
                        e.options[index].label,
                        index + 1,
                        e.options.len()
                    )
                } else if !e.children.is_empty() {
                    String::new()
                } else if e.patch.is_some() {
                    if e.patch
                        .as_ref()
                        .is_some_and(|p| matches_patch(&state.draft, p))
                    {
                        "Selected".into()
                    } else {
                        "Select".into()
                    }
                } else {
                    String::new()
                };
                let adjustable = e.scalar.is_some() || e.options.len() > 1;
                let width = (window.width() * 0.38).clamp(370., 520.)
                    - 44.
                    - 24.
                    - if adjustable { 68. } else { 0. };
                let label = format!(
                    "{}{}",
                    e.label,
                    if !e.children.is_empty() { "  ›" } else { "" }
                );
                let label_size =
                    (width / (label.chars().count().max(1) as f32 * 0.64)).clamp(16., 18.);
                let detail_size =
                    (width / (detail.chars().count().max(1) as f32 * 0.64)).clamp(10., 12.);
                list.spawn((
                    Node {
                        width: percent(100),
                        height: px(84),
                        flex_shrink: 0.,
                        align_items: AlignItems::Stretch,
                        border_radius: BorderRadius::all(px(9)),
                        ..default()
                    },
                    BackgroundColor(if pos == state.selected {
                        Color::srgb(0.08, 0.27, 0.29)
                    } else {
                        Color::srgb(0.045, 0.065, 0.085)
                    }),
                ))
                .with_children(|r| {
                    if adjustable {
                        adjust_button(r, i, -1, "−");
                    }
                    r.spawn((
                        Button,
                        Row(i),
                        Node {
                            flex_grow: 1.,
                            min_width: px(0),
                            padding: UiRect::axes(px(12), px(7)),
                            flex_direction: FlexDirection::Column,
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                    ))
                    .with_children(|text| {
                        text.spawn((
                            Text::new(label),
                            TextFont {
                                font_size: label_size,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                        if !detail.is_empty() {
                            text.spawn((
                                Text::new(detail),
                                TextFont {
                                    font_size: detail_size,
                                    ..default()
                                },
                                TextColor(Color::srgb(0.60, 0.76, 0.78)),
                            ));
                        }
                    });
                    if adjustable {
                        adjust_button(r, i, 1, "+");
                    }
                });
            }
        });
        if !state.status.is_empty() {
            p.spawn((
                Text::new(&state.status),
                TextFont {
                    font_size: 13.,
                    ..default()
                },
                TextColor(Color::srgb(1., 0.72, 0.4)),
            ));
        }
        p.spawn(Node {
            width: percent(100),
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|footer| {
            for (id, title) in [
                (
                    BACK,
                    if state.path.is_empty() {
                        "Done"
                    } else {
                        "‹ Back"
                    },
                ),
                (PREV, "‹"),
                (NEXT, "›"),
            ] {
                if id != BACK && visible.len() <= state.page_size {
                    continue;
                }
                footer
                    .spawn((
                        Button,
                        Row(id),
                        Node {
                            padding: UiRect::axes(px(14), px(8)),
                            border_radius: BorderRadius::all(px(8)),
                            ..default()
                        },
                        BackgroundColor(if id == BACK {
                            Color::srgb(0.12, 0.39, 0.38)
                        } else {
                            Color::srgb(0.07, 0.10, 0.13)
                        }),
                    ))
                    .with_children(|r| {
                        r.spawn((
                            Text::new(title),
                            TextFont {
                                font_size: 17.,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                    });
            }
            if visible.len() > state.page_size {
                footer.spawn((
                    Text::new(format!(
                        "{} / {}",
                        start / state.page_size + 1,
                        visible.len().div_ceil(state.page_size)
                    )),
                    TextFont {
                        font_size: 13.,
                        ..default()
                    },
                    TextColor(Color::srgb(0.6, 0.7, 0.75)),
                ));
            }
        });
        p.spawn((
            Text::new(if state.path.is_empty() {
                "Right stick: Rotate   •   Done saves and resumes."
            } else {
                "↑↓ Browse   ←→ Change   Right stick: Rotate
Type to search"
            }),
            TextFont {
                font_size: 12.,
                ..default()
            },
            TextColor(Color::srgb(0.48, 0.60, 0.66)),
        ));
    });
}

fn adjust_button(parent: &mut ChildSpawnerCommands, index: usize, direction: i32, label: &str) {
    parent
        .spawn((
            Button,
            Adjust(index, direction),
            Node {
                width: px(34),
                flex_shrink: 0.,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(label),
                TextFont {
                    font_size: 22.,
                    ..default()
                },
                TextColor(Color::srgb(0.7, 0.9, 0.9)),
            ));
        });
}
fn matches_patch(profile: &Value, patch: &Value) -> bool {
    if let Some(object) = patch.as_object() {
        object.iter().all(|(k, v)| matches_patch(&profile[k], v))
    } else {
        profile == patch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nav_repeat_defaults_and_overrides() {
        assert_eq!(parse_repeat(None), (0.25, 0.06));
        assert_eq!(parse_repeat(Some("0.35, 0.09")), (0.35, 0.09));
        assert_eq!(parse_repeat(Some("fast")), (0.25, 0.06));
        assert_eq!(parse_repeat(Some("0,0.1")), (0.25, 0.06));
    }
    /// B3 (release review 10-18): leaving the CAS with nothing changed writes nothing, so the defaults shown for a look
    /// that didn't load never replace it; a change is saved. With no parts library a refused look stays where it is
    /// and nothing saves over it.
    #[test]
    fn an_unchanged_look_is_never_written() {
        let dir = std::env::temp_dir().join(format!("s3r-cas-unchanged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("character.json");
        let mut c = Customiser::for_draft(json!({"n": 1}));
        c.set_profile_path(file.clone());
        c.saved = c.draft.clone();
        c.leave();
        assert!(!file.exists(), "unchanged: nothing written");
        c.draft["n"] = json!(2);
        c.leave();
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&file).unwrap()).unwrap(), json!({"n": 2}));
        std::fs::remove_file(&file).unwrap();
        c.leave();
        assert!(!file.exists(), "saved once: not again until it changes");
        // A teammate swapped in with its look as loaded is unchanged too.
        let mate = dir.join("teammate1.json");
        c.swap_profile(mate.clone(), json!({"n": 3}));
        c.leave();
        assert!(!mate.exists());
        let refused_look = dir.join("teammate2.json");
        std::fs::write(&refused_look, b"{\"n\":4}").unwrap();
        refused(&refused_look, "Item unavailable", &Parts::default());
        assert_eq!(std::fs::read(&refused_look).unwrap(), b"{\"n\":4}", "no library: left in place");
        assert!(crate::saves::write_atomic(&refused_look, b"{}").is_err(), "and not saved over");
        std::fs::remove_dir_all(&dir).unwrap();
    }
    /// S1 tutorial preview 3 bug 1: the frontend pauses the world from the map's first frame, so its customiser opens
    /// before any presentation frame; holding the world then left the camera off the skater and the room hidden.
    #[test]
    fn an_open_customiser_runs_the_world_until_it_has_a_frame_to_show() {
        assert_eq!(world_pause(true, false, false), Some(false), "no frame yet: run the world");
        assert_eq!(world_pause(true, true, false), Some(true), "framed: hold the skater still");
        assert_eq!(world_pause(false, false, false), None, "closed: left to the others");
        assert_eq!(world_pause(false, true, false), None);
        assert_eq!(world_pause(true, true, true), Some(false), "online: the session runs on (the pad holds the skater)");
    }
    /// Overnight 10-15 D: the Skate 1 frontend's customiser holds the world at physics tick 7 of a fresh launch
    /// (run4 log). The pose it holds, and the editor stand drawn over it, are whole from tick 1: a degenerate early
    /// pose is not why that customiser showed no skater.
    #[test]
    #[ignore = "needs SKATE3_ASSET_ROOT and the installed Skate 1 map"]
    fn s1_frontend_cas_pose_is_whole_from_the_first_ticks() {
        use crate::physics::mode_harness::{Harness, Spawn, asset_root, pads};
        let mut h = Harness::load("Skate_1_San_Vanelona", Spawn::Package);
        let names = h.skater.animation.evaluator.frames.bone_names.clone();
        let poser = crate::cas_pose::Poser::load(&asset_root(), "male", &names).unwrap();
        let bad = |m: &bevy::math::Mat4| !m.is_finite() || m.determinant().abs() < 1e-4;
        let report = |tag: &str, pose: &[bevy::math::Mat4]| {
            let worst: Vec<_> = pose.iter().enumerate().filter(|(_, m)| bad(m)).map(|(i, m)| format!("{}={:?}", names.get(i).map_or("?", |s| s.as_str()), m.to_cols_array())).collect();
            eprintln!("{tag}: {} bones, {} degenerate {:?}", pose.len(), worst.len(), worst.iter().take(6).collect::<Vec<_>>());
            assert!(worst.is_empty(), "{tag}: degenerate bones {worst:?}");
        };
        let initial: Vec<_> = h.skater.render_pose.iter().copied().map(crate::animation::native_matrix).collect();
        report("tick 0 render_pose", &initial);
        for t in 1..=240u64 {
            h.tick(pads::idle());
            if [1, 7, 8, 30, 240].contains(&t) {
                let render: Vec<_> = h.skater.render_pose.iter().copied().map(crate::animation::native_matrix).collect();
                let root = crate::animation::native_matrix(h.skater.animated_skeleton.roots.animation_to_world);
                eprintln!("tick {t}: root {:?} state {:?}", root.to_cols_array(), h.skater.player_state.current());
                assert!(root.is_finite() && (root.determinant() - 1.).abs() < 1e-3, "tick {t}: root {root:?}");
                report(&format!("tick {t} render_pose"), &render);
                let cas = poser.pose("CAS_STAND", 0.5, true, &h.skater.render_pose).unwrap();
                report(&format!("tick {t} CAS_STAND"), &cas);
            }
        }
    }
    #[test]
    fn new_and_worn_deck_prints_pair_up() {
        assert_eq!(wear_pair("Trim 119 (Worn)"), ("Trim119".into(), "Trim 119", Some(true)));
        assert_eq!(wear_pair("Trim 119"), ("Trim119".into(), "Trim 119", None));
        // Skate 1: the C print is clean, its unmarked sibling worn; spacing differs.
        assert_eq!(wear_pair("Board Alienworkshop Bk02C").0, wear_pair("Board Alienworkshop Bk 02").0);
        assert_eq!(wear_pair("Board Alienworkshop Bk02C").2, Some(false));
        assert_eq!(wear_pair("Hom").2, None);
        assert_eq!(wear_pair("White Dirty 1 (Worn)").0, wear_pair("White 1").0);
        assert_eq!(wear_pair("Homl (Worn)").0, wear_pair("Hom").0);
        assert_eq!(wear_pair("Board Zero Mike Bl 01").0, wear_pair("Board Zero Bl01C").0);
    }
    /// User 10-13: on the open colour wheel LT / RT still turn the skater; the right stick sets brightness.
    #[test]
    fn the_open_colour_wheel_leaves_lt_and_rt_to_rotate_skater() {
        use crate::colour_wheel::{ColourWheel, Open, controls};
        use bevy::ecs::system::RunSystemOnce;
        let rt = skate_core::input::xbox::XboxState { buttons: 0, triggers: [0, 255], left: [0; 2], right: [0, 32767] };
        let (stick, brightness) = controls(Some(&rt), &ButtonInput::<KeyCode>::default());
        let mut world = World::new();
        world.insert_resource(Customiser {
            open: true,
            enabled: true,
            just_opened: false,
            preview_yaw: 0.,
            index: Entry::default(),
            path: vec![],
            selected: 0,
            page_size: 6,
            search: String::new(),
            draft: json!({"selections":{},"morphs":{}}),
            settings: PathBuf::new(),
            status: String::new(),
            saved: Value::Null,
            problem: None,
            redraw: false,
            original: true,
            fallback: false,
            view: None,
            strip: None,
            skate2: None,
            grip: false,
        });
        let custom = crate::cas_menu::Custom {
            template: json!({"grip": {"rgb": [0, 0, 0], "solid": false}}),
            at: "/grip/rgb".into(),
            start: [0.5; 3],
            authored: [0.5; 3],
            default: json!({"grip": null}),
            hair: false,
        };
        let open = Open::new(world.resource::<Customiser>(), custom);
        world.insert_resource(ColourWheel(Some(open)));
        let mut time = Time::<Real>::default();
        time.update_with_duration(std::time::Duration::ZERO);
        time.update_with_duration(std::time::Duration::from_millis(50));
        world.insert_resource(time);
        // What navigation makes of RT held.
        world.insert_resource(Navigation { preview_turn: 1., ..default() });
        world.run_system_once(rotate_preview).unwrap();
        let turned = world.resource::<Customiser>().preview_yaw;
        assert!(turned > 0., "RT turns the skater with the wheel open");
        let mut wheel = world.resource_mut::<ColourWheel>();
        let open = wheel.0.as_mut().unwrap();
        let before = open.wheel.value;
        open.steer(Vec2::ZERO, 0., 0.05);
        open.steer(stick, brightness, 0.05);
        assert!(open.wheel.value > before, "right stick up brightens the wheel: {before} -> {}", open.wheel.value);
        let triggers_only = skate_core::input::xbox::XboxState { right: [0; 2], ..rt };
        assert_eq!(controls(Some(&triggers_only), &ButtonInput::<KeyCode>::default()).1, 0., "RT alone leaves brightness");
    }
    /// Opening Edit Skater showed this panel for the second the original screen took to load (10-11): it is only
    /// the fallback for a missing or broken original screen.
    #[test]
    fn the_panel_stays_hidden_while_the_original_screen_loads() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.spawn(Window::default());
        let root = world.spawn((Root, Node { display: Display::None, ..default() })).id();
        world.insert_resource(Customiser {
            open: true,
            enabled: true,
            just_opened: true,
            preview_yaw: 0.,
            index: Entry::default(),
            path: vec![],
            selected: 0,
            page_size: 6,
            search: String::new(),
            draft: json!({"selections":{},"morphs":{}}),
            settings: PathBuf::new(),
            status: String::new(),
            saved: Value::Null,
            problem: None,
            redraw: true,
            original: false,
            fallback: false,
            view: None,
            strip: None,
            skate2: None,
            grip: false,
        });
        world.run_system_once(draw).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::None);
        world.resource_mut::<Customiser>().fallback = true;
        world.run_system_once(draw).unwrap();
        assert_eq!(world.get::<Node>(root).unwrap().display, Display::Flex);
    }
    #[test]
    fn customiser_missing_library_keeps_stock_scene_visible() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<crate::customiser_parts::Parts>()
            .init_resource::<crate::custom_models::CustomModels>()
            .init_resource::<crate::animation::AnimationStatus>()
            .init_resource::<Assets<crate::customiser_material::SkaterMaterial>>()
            .init_resource::<Assets<Image>>();
        let world = app.world_mut();
        world.insert_resource(Customiser {
            open: true,
            enabled: true,
            just_opened: false,
            preview_yaw: 0.,
            index: Entry::default(),
            path: vec![],
            selected: 0,
            page_size: 6,
            search: String::new(),
            draft: json!({"selections":{},"morphs":{}}),
            settings: PathBuf::new(),
            status: String::new(),
            saved: Value::Null,
            problem: None,
            redraw: false,
            original: false,
            fallback: true,
            view: None,
            strip: None,
            skate2: None,
            grip: false,
        });
        let player = world.spawn(crate::world::PlayerRoot).id();
        let stock = world
            .spawn((SceneRoot(default()), Visibility::Inherited, ChildOf(player)))
            .id();
        world
            .run_system_once(crate::customiser_parts::update)
            .unwrap();
        assert_eq!(
            *world.get::<Visibility>(stock).unwrap(),
            Visibility::Inherited
        );
        assert!(
            world
                .resource::<Customiser>()
                .status
                .contains("unavailable")
        );
        assert!(world.resource::<Parts>().applied.is_null());
    }
    #[test]
    fn customiser_colour_choice_is_distinct_from_the_original() {
        let entry = Entry {
            options: vec![
                choice(
                    "Original",
                    json!({"selections":{"Feet":{"asset_id":"shoe","material_id":"new"}},"colours":{"Feet":null}}),
                ),
                choice(
                    "Red",
                    json!({"selections":{"Feet":{"asset_id":"shoe","material_id":"new"}},"colours":{"Feet":[1.,0.,0.]}}),
                ),
            ],
            ..default()
        };
        let mut p = json!({"selections":{"Feet":{"asset_id":"shoe","material_id":"new"}}});
        assert_eq!(option_index(&entry, &p), Some(0));
        merge(&mut p, entry.options[1].patch.as_ref().unwrap());
        assert_eq!(option_index(&entry, &p), Some(1));
        let material = json!({"name":"original","flags":{},"diffuse":"",
            "alpha":false,"tint":[0.4,0.5,0.6],"metallic":0.,"roughness":0.7});
        let library: Library = serde_json::from_value(json!({
            "models":{
                "shoe":{"slot":"Feet","name":"shoe","flags":{},"materials":["new"],"scene":""},
                "head":{"slot":"Rostral","name":"head","flags":{},"materials":["head"],"scene":""}},
            "materials":{"new":material.clone(),"head":material},"defaults":{},"morphs":[]
        }))
        .unwrap();
        let parts = Parts::for_test(library);
        p["selections"]["Rostral"] = json!({"asset_id":"head","material_id":"head"});
        assert!(parts.resolve(&p).is_ok());
        merge(&mut p, entry.options[0].patch.as_ref().unwrap());
        assert_eq!(option_index(&entry, &p), Some(0));
        assert!(
            parts.resolve(&p).is_ok(),
            "original material tint must be selectable"
        );
        let restored: Value = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        assert!(
            parts.resolve(&restored).is_ok(),
            "saved/online null reset must remain valid"
        );
        for invalid in [
            json!([2., 0., 0.]),
            json!([-0.1, 0., 0.]),
            json!([0., 1.]),
            json!("red"),
        ] {
            p["colours"]["Feet"] = invalid;
            assert_eq!(parts.resolve(&p).unwrap_err(), "Invalid clothing colour");
        }
    }
    /// User 10-30: a backpack took the gloves off. Each Accessories tab wears its own slot, and its None takes off
    /// only its own item.
    #[test]
    fn the_accessory_tabs_wear_their_items_together() {
        let parts = Parts::for_test(Parts::accessory_test_library());
        let mut c = Customiser::for_test(&parts, vec![], "male");
        let tabs = [("Rings", "ring"), ("Belts", "belt"), ("Chains", "chain"), ("Backpacks", "backpack"), ("Other", "cigarette"), ("Gloves", "gloves")];
        for (tab, item) in tabs {
            let page = c.entry(&["Clothes", "Accessories", tab]).unwrap_or_else(|| panic!("{tab}"));
            let patch = page.children.iter().find(|e| !e.options.is_empty()).and_then(|e| e.options[0].patch.clone()).unwrap();
            assert_eq!(patch["selections"].as_object().unwrap().values().next().unwrap()["asset_id"], item, "{tab}");
            merge(&mut c.draft, &patch);
        }
        let worn = |draft: &Value| -> Vec<String> {
            let resolved = parts.resolve(draft).unwrap();
            let mut ids: Vec<String> = resolved["selections"].as_object().unwrap().values().map(|v| v["asset_id"].as_str().unwrap().to_owned()).collect();
            ids.sort();
            ids
        };
        assert_eq!(worn(&c.draft), ["backpack", "belt", "chain", "cigarette", "gloves", "head", "ring"]);
        let none = c.entry(&["Clothes", "Accessories", "Gloves"]).and_then(|p| p.children.iter().find(|e| e.label == "None")).and_then(|e| e.patch.clone()).unwrap();
        merge(&mut c.draft, &none);
        assert_eq!(worn(&c.draft), ["backpack", "belt", "chain", "cigarette", "head", "ring"]);
        // Chosen in the menu: gloves take the ring off, and with gloves on no ring goes on.
        let tile = |tab: &str| c.entry(&["Clothes", "Accessories", tab]).and_then(|p| p.children.iter().find(|e| !e.options.is_empty())).and_then(|e| e.options[0].patch.clone()).unwrap();
        let (gloves, ring) = (tile("Gloves"), tile("Rings"));
        c.apply(&parts, Some(&gloves));
        assert_eq!(worn(&c.draft), ["backpack", "belt", "chain", "cigarette", "gloves", "head"]);
        c.apply(&parts, Some(&ring));
        assert_eq!(worn(&c.draft), ["backpack", "belt", "chain", "cigarette", "gloves", "head"], "no ring over gloves");
        c.apply(&parts, Some(&none));
        c.apply(&parts, Some(&ring));
        assert_eq!(worn(&c.draft), ["backpack", "belt", "chain", "cigarette", "head", "ring"], "gloves off: the ring goes on");
    }
    #[test]
    fn customiser_owned_menu_presets_tattoos_and_search() {
        let Ok(path) = std::env::var("SKATE_CAC_TEST_LIBRARY") else {
            return;
        };
        let lib: Library = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let extras: Vec<Entry> = serde_json::from_slice(
            &std::fs::read(std::path::Path::new(&path).with_file_name("extra-menu.json")).unwrap(),
        )
        .unwrap();
        let parts = Parts::for_test(lib);
        for gender in ["male", "female"] {
            let mut state = Customiser {
                open: true,
                enabled: true,
                just_opened: false,
                preview_yaw: 0.,
                index: menu(&parts.library, extras.clone()),
                path: vec![],
                selected: 0,
                page_size: 6,
                search: String::new(),
                draft: parts.library.defaults[gender].clone(),
                settings: PathBuf::new(),
                status: String::new(),
                saved: Value::Null,
                problem: None,
                redraw: true,
                original: false,
                fallback: true,
                view: None,
                strip: None,
                skate2: None,
                grip: false,
            };
            assert_eq!(state.visible().len(), 5);
            let body = 0;
            let tattoos = state.index.children[body]
                .children
                .iter()
                .position(|e| e.label == "Tattoos")
                .unwrap();
            let original = state.draft.clone();
            for (i, slot) in [(0, "OuterTorso"), (1, "Pants")] {
                state.path = vec![body, tattoos, i];
                let preview = state.preview(&parts);
                assert_ne!(preview["selections"][slot], original["selections"][slot]);
                assert_eq!(state.draft, original);
            }
            state.open = false;
            assert_eq!(state.preview(&parts), original);
            state.open = true;
            let faces = state.index.children[body]
                .children
                .iter()
                .position(|e| e.label == "Face presets")
                .unwrap();
            state.path = vec![body, faces];
            assert_eq!(state.visible().len(), 10);
            for i in state.visible() {
                let mut profile = state.draft.clone();
                merge(
                    &mut profile,
                    state.page().children[i].patch.as_ref().unwrap(),
                );
                assert!(parts.resolve(&profile).is_ok());
            }
            state.search = "Face 10".into();
            assert_eq!(state.visible().len(), 1);
            state.search = "No such item".into();
            assert_eq!(state.visible().len(), 0);
            // Selecting a new face/body profile must survive JSON save/reload.
            let encoded = serde_json::to_vec(&state.draft).unwrap();
            let restored: Value = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(
                parts.resolve(&restored).unwrap(),
                parts.resolve(&state.draft).unwrap()
            );
        }
    }

    /// Perf 10-12: what customiser_parts::update recomputes each frame Edit Skater is open, ahead of its
    /// "already applied" return (preview, resolve, the visible choices' outfits), at the root and on the
    /// busiest choice page. SKATE3_ASSET_ROOT = the install's assets;
    /// `cargo test ... open_frame_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires the installed character library"]
    fn open_frame_cost() {
        let root = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap());
        let dir = crate::customiser_parts::asset_directory(&root);
        let bytes = std::fs::read(dir.join("library-v3.json")).or_else(|_| std::fs::read(dir.join("library.json"))).unwrap();
        let parts = Parts::for_test(serde_json::from_slice::<Library>(&bytes).unwrap());
        let mut c = Customiser::for_test(&parts, vec![], "male");
        // The page whose visible rows carry the most patches.
        fn busiest(e: &Entry, path: &mut Vec<usize>, best: &mut (usize, Vec<usize>)) {
            let n = e.children.iter().filter(|c| c.patch.is_some() || !c.options.is_empty()).count();
            if n > best.0 {
                *best = (n, path.clone());
            }
            for (i, child) in e.children.iter().enumerate() {
                path.push(i);
                busiest(child, path, best);
                path.pop();
            }
        }
        let mut best = (0, Vec::new());
        busiest(&c.index, &mut Vec::new(), &mut best);
        for (name, path) in [("root", Vec::new()), ("busiest page", best.1.clone())] {
            c.path = path;
            c.selected = 0;
            let frames = 200;
            let t = std::time::Instant::now();
            let mut outfits = 0;
            for _ in 0..frames {
                let preview = c.preview(&parts);
                assert!(parts.resolve(&preview).is_ok());
                let preload = c.preload_outfits(&parts);
                outfits = preload.len();
                std::hint::black_box((preview, preload));
            }
            eprintln!("{name}: {:.1} us/frame, {outfits} outfits resolved a frame", t.elapsed().as_secs_f64() * 1e6 / frames as f64);
        }
    }
}
