//! Skate 3's Challenge Map (map/challengemap) on the shared APT runtime, showing
//! its Locations tab. The movie asks the native `FEChallengeMap` for tabs, rows,
//! helper text and map art, and is driven through challengemap.SetState(n) exactly
//! as the shipped native screen does. Bevy-independent: challenge_map.rs travels.
use crate::{
    apt_movie::Movie,
    apt_scene::{Draw, Shapes, Texture, Vertex},
    apt_vm::{Host, ObjectKind, Value, Vm},
    core_menu_runtime::{array, boot_screen, common_call, drain, native_vm, screen_property_changed, Key, ScreenHost},
};
use std::collections::BTreeMap;

/// challengemap.SetState values the native screen sends.
pub(crate) const REFRESH_ITEMS: f64 = 10.;
pub(crate) const INFO_MODE: f64 = 20.;
pub(crate) const STUFF_MODE: f64 = 21.;
pub(crate) const LOAD_DEFAULT_MAP: f64 = 33.;
pub(crate) const REFRESH_MAP: f64 = 34.;
pub(crate) const SHOW_HEADER: f64 = 40.;
pub(crate) const HIDE_HEADER: f64 = 41.;
pub(crate) const SHOW_HELPER: f64 = 55.;
pub(crate) const REFRESH_CURSOR: f64 = 110.;
pub(crate) const REFRESH_BUTTONS: f64 = 140.;
/// The Port Carverton overview (fe\source\images\map\).
pub(crate) const WORLD_MAP: &str = "map1024.rx2";
/// Retail tabs; this engine has no challenges, so the screen stays on Locations.
const TABS: [&str; 3] = ["ID_MAP_FILTER_CHALLENGE_ACTIVE", "ID_MAP_FILTER_CHALLENGE_COMPLETED", "ID_MAP_FILTER_LOCATION"];
const LOCATIONS_TAB: usize = 2;
/// Stage position of the world map's top-left corner on the native screen.
pub(crate) const MAP_ORIGIN: [f64; 2] = [289., -101.];
/// A place's own map (skate.School) shows in mMapImage at its authored placement.
pub(crate) const OWN_MAP_ORIGIN: [f32; 2] = [572., 120.5];
/// The native map renderer's location marker, drawn 1:1 (32 px) on the stage.
const MARKER: &str = "mini_teleport";
pub(crate) const MARKER_SIZE: f32 = 32.;

/// A Locations row: a district or park and the rows it lists.
#[derive(Clone, Debug)]
pub(crate) struct Place {
    /// String ID or "#literal".
    pub name: String,
    /// Helper text string ID.
    pub help: String,
    /// Its own map image file (fe/source/images/map); None: the world map.
    pub map: Option<String>,
    /// Its markers' pin art (a thumbnail) and the pin's anchor as fractions of its size;
    /// None: the native renderer's centred mini_teleport.
    pub pin: Option<(String, [f32; 2])>,
    pub rows: Vec<Row>,
}
/// A location, or a section header (skate.Park groups parks by name).
#[derive(Clone, Debug, Default)]
pub(crate) struct Row {
    /// Catalog name or string ID.
    pub name: String,
    pub header: bool,
    /// Marker centre in texels of the place's map.
    pub marker: Option<[f32; 2]>,
    /// skate.Park layout picture shown while highlighted.
    pub preview: Option<String>,
    /// Location details (photo file, "#description"): A opens the info panel
    /// (challenge_info map-screen mode) with a Teleport button, as retail.
    pub info: Option<(String, String)>,
}
impl Place {
    fn locations(&self) -> usize {
        self.rows.iter().filter(|r| !r.header).count()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// (place, spot) indices.
    Travel(usize, usize),
    Close,
}

#[derive(Clone)]
pub(crate) struct Bindings {
    pub movie: Movie,
    pub shapes: Shapes,
    places: Vec<Place>,
    /// None: the place list; Some(p): the spots of place p.
    open: Option<usize>,
    highlighted: usize,
    chosen: Option<usize>,
    /// The highlighted location's info panel is showing.
    info: bool,
    thumbnails: BTreeMap<String, Texture>,
    slots: u32,
    random: u32,
    pub intro_complete: bool,
    pub outro_complete: bool,
}
impl Bindings {
    /// (name, is header) per list row.
    fn rows(&self) -> Vec<(String, bool)> {
        let localize = |s: &str| self.movie.text_assets.localize(s);
        match self.open {
            Some(p) => self.places[p].rows.iter().map(|r| (format!("#{}", localize(&r.name)), r.header)).collect(),
            None => self.places.iter().map(|p| (format!("#{} ({})", localize(&p.name), p.locations()), false)).collect(),
        }
    }
    fn place(&self) -> Option<&Place> {
        self.places.get(self.open?)
    }
    fn row(&self) -> Option<&Row> {
        self.place()?.rows.get(self.highlighted)
    }
    fn marker_origin(&self) -> [f32; 2] {
        match self.place().and_then(|p| p.map.as_ref()) {
            Some(_) => OWN_MAP_ORIGIN,
            None => MAP_ORIGIN.map(|v| v as f32),
        }
    }
    /// Stage centre of the highlighted location's marker.
    fn cursor(&self) -> Option<[f32; 2]> {
        let [x, y] = self.row()?.marker?;
        let [ox, oy] = self.marker_origin();
        Some([ox + x, oy + y])
    }
    /// (file, is a skatepark picture) for challengemap.SetState(34).
    fn map_texture(&self) -> (String, bool) {
        if let Some(preview) = self.row().and_then(|r| r.preview.clone()) {
            return (preview, true);
        }
        (self.place().and_then(|p| p.map.clone()).unwrap_or_else(|| WORLD_MAP.into()), false)
    }
}
impl ScreenHost for Bindings {
    fn movie(&mut self) -> &mut Movie {
        &mut self.movie
    }
    fn random(&mut self) -> &mut u32 {
        &mut self.random
    }
}
impl Host for Bindings {
    fn property_changed(&mut self, vm: &mut Vm, object: usize, key: &str) -> Result<(), String> {
        screen_property_changed(&mut self.movie, vm, object, key)
    }
    fn call(&mut self, vm: &mut Vm, object: usize, method: &str, args: Vec<Value>) -> Result<Value, String> {
        if let Some(value) = common_call(self, vm, object, method, &args)? {
            return Ok(value);
        }
        let native = match vm.objects.get(object).map(|o| &o.kind) {
            Some(ObjectKind::Native(name)) => name.clone(),
            _ => "global".into(),
        };
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        Ok(match (native.as_str(), method) {
            ("FEChallengeMap", "GetSuperCategories") => array(vm, TABS.map(|t| Value::Text(t.into())))?,
            ("FEChallengeMap", "GetCurrentSuperCategoryIndex") => Value::Number(LOCATIONS_TAB as f64),
            // [count, selected, names, isHeader, iconState, secondIconState]
            ("FEChallengeMap", "GetStuffMenuItems") => {
                let rows = self.rows();
                let n = rows.len();
                let headers = array(vm, rows.iter().map(|r| Value::Bool(r.1)))?;
                let names = array(vm, rows.into_iter().map(|r| Value::Text(r.0)))?;
                let icons = array(vm, (0..n).map(|_| Value::Text("none".into())))?;
                let second = array(vm, (0..n).map(|_| Value::Text("none".into())))?;
                array(vm, [Value::Number(n as f64), Value::Number(self.highlighted as f64), names, headers, icons, second])?
            }
            ("FEChallengeMap", "GetStuffTitleText") => Value::Text(self.open.map_or("#".into(), |p| self.places[p].name.clone())),
            // A place's helper text; its locations have none.
            ("FEChallengeMap", "GetMapHelperInfoString") => match self.open {
                Some(_) => Value::Text("#".into()),
                None => Value::Text(self.places.get(self.highlighted).map_or("#".into(), |p| p.help.clone())),
            },
            ("FEChallengeMap", "IsSelectAvailable") => Value::Bool(true),
            ("FEChallengeMap", "GetDefaultMapTexture") => Value::Text(WORLD_MAP.into()),
            ("FEChallengeMap", "IsUsingDefaultMap") => Value::Bool(self.map_texture().0 == WORLD_MAP),
            // [file, is RX2, is a skatepark picture]
            ("FEChallengeMap", "GetMapTexture") => {
                let (file, picture) = self.map_texture();
                array(vm, [Value::Text(file), Value::Bool(true), Value::Bool(picture)])?
            }
            ("FEChallengeMap", "IsCursorVisible") => Value::Bool(self.cursor().is_some()),
            ("FEChallengeMap", "GetCursorCoordinates") => {
                let [x, y] = self.cursor().unwrap_or_default();
                array(vm, [Value::Number(x as f64), Value::Number(y as f64)])?
            }
            // [friend count, ...]: none, so the info panel shows "no data".
            ("FEChallengeMap", "GetChallengeFriendsLeaderboardData") => array(vm, [Value::Number(0.), Value::Bool(false)])?,
            ("FEChallengeMap", "GetCurrentOnlineDifficultyName") => Value::Text("#".into()),
            // The info panel (challenge_info map-screen mode): the location's name,
            // photo and description, and one option, Teleport.
            ("FEChallengeMap", "GetChallengeTitle") => {
                Value::Text(format!("#{}", self.row().map_or(String::new(), |r| self.movie.text_assets.localize(&r.name))))
            }
            ("FEChallengeMap", "HasOnlineDifficultySelector") => Value::Bool(false),
            ("FEChallengeMap", "GetChallengeComponentNumOptions") => Value::Number(1.),
            ("FEChallengeMap", "GetChallengeComponentOption") => Value::Text("ID_COMMON_TELEPORT".into()),
            ("CIC", "IsImageEnabled") | ("CIC", "IsDescriptionEnabled") => Value::Bool(self.row().is_some_and(|r| r.info.is_some())),
            ("CIC", "GetImage") => Value::Text(self.row().and_then(|r| r.info.as_ref()).map_or(String::new(), |i| i.0.clone())),
            ("CIC", "GetDescription") => Value::Text(self.row().and_then(|r| r.info.as_ref()).map_or("#".into(), |i| i.1.clone())),
            ("CIC", _) => Value::Undefined,
            // Locations have no objectives, rewards or teammates.
            ("Mission", _) => Value::Bool(false),
            ("ScreenManager", "IntroComplete") => {
                self.intro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", "OutroComplete") => {
                self.outro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", _) => Value::Undefined,
            // The info panel's Teleport menu reports too; it must not move the list row.
            ("Game", "OnMenuHilight") | ("Game", "OnMenuSelect") if self.info => Value::Undefined,
            ("Game", "OnMenuHilight") => {
                self.highlighted = arg(1).number().max(0.) as usize;
                Value::Undefined
            }
            ("Game", "OnMenuSelect") => {
                self.chosen = Some(arg(1).number().max(0.) as usize);
                Value::Undefined
            }
            ("Game", "OnMenuGetItemEnabled") => Value::Bool(true),
            ("Game", "ImageRenderer_ReserveNextFreeIndex") => {
                self.slots += 1;
                Value::Number(self.slots as f64)
            }
            // (path, slot): the movie prefixes "fe\source\images\map\".
            ("Game", "ImageRenderer_Load") => {
                let slot = arg(1).number().to_string();
                let path = arg(0).text().to_lowercase();
                let file = path.rsplit(['\\', '/']).next().unwrap_or_default();
                match self.thumbnails.get(file) {
                    Some(texture) => self.movie.images.insert(slot, texture.clone()),
                    None => self.movie.images.remove(&slot),
                };
                Value::Undefined
            }
            ("Game", "ImageRenderer_Unload") | ("Game", "ImageRenderer_FreeImageIndex") => {
                self.movie.images.remove(&arg(0).number().to_string());
                Value::Undefined
            }
            ("Game", _) => Value::Undefined,
            _ => return Err(format!("Unimplemented challenge map binding {native}.{method}")),
        })
    }
}

#[derive(Clone)]
pub(crate) struct Runtime {
    pub vm: Vm,
    pub bindings: Bindings,
    screen: usize,
    /// Front-end sound ids (`GlobalFEPlaySound`) the inputs since the last drain played.
    pub sounds: Vec<u64>,
}
impl Runtime {
    pub fn load(json: &serde_json::Value, places: Vec<Place>) -> Result<Self, String> {
        if places.is_empty() {
            return Err("Challenge map needs locations".into());
        }
        let mut vm = native_vm(&["FEChallengeMap", "CIC", "Mission"])?;
        let mut bindings = Bindings {
            movie: Movie::load(json)?,
            shapes: serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?,
            places,
            open: None,
            highlighted: 0,
            chosen: None,
            info: false,
            thumbnails: serde::Deserialize::deserialize(&json["thumbnails"]).map_err(|e| e.to_string())?,
            slots: 0,
            random: 0x9e3779b9,
            intro_complete: false,
            outro_complete: false,
        };
        let buttons: BTreeMap<String, Texture> = serde::Deserialize::deserialize(&json["buttons"]).unwrap_or_default();
        bindings.movie.images.extend(buttons.into_iter().map(|(name, texture)| (format!("button:{name}"), texture)));
        let screen = boot_screen(json, &mut vm, &mut bindings)?;
        let mut runtime = Self { vm, bindings, screen, sounds: Vec::new() };
        // Before the first frame is drawn: the intro plays over the map.
        runtime.map()?;
        Ok(runtime)
    }
    fn states(&mut self, states: &[f64]) -> Result<(), String> {
        for &state in states {
            self.vm.begin_update();
            self.vm.call_method(self.screen, "SetState", vec![Value::Number(state)], &mut self.bindings)?;
            drain(&mut self.vm, &mut self.bindings)?;
        }
        Ok(())
    }
    /// The world map, as the native screen shows it. mcMapPanel is the native
    /// MapRenderComponent's viewport, an authored blue placeholder the engine draws
    /// over; here the default map image beneath it is the map.
    /// ponytail: no live district render (zoom, challenge icons, cursor).
    fn map(&mut self) -> Result<(), String> {
        let root = self.bindings.movie.root;
        if let Value::Object(panel) = self.vm.get(root, "mcMapPanel") {
            self.vm.set(panel, "_visible", Value::Bool(false))?;
        }
        self.states(&[LOAD_DEFAULT_MAP, REFRESH_MAP, REFRESH_CURSOR])?;
        // The native screen shows the map 60 px above its authored placement, at
        // the same 1:1 scale (measured from the PS3 retail screen in RPCS3).
        if let Value::Object(image) = self.vm.get(root, "mDefaultMapImage") {
            for (key, value) in [("_x", MAP_ORIGIN[0]), ("_y", MAP_ORIGIN[1])] {
                self.vm.set(image, key, Value::Number(value))?;
                screen_property_changed(&mut self.bindings.movie, &mut self.vm, image, key)?;
            }
        }
        Ok(())
    }
    /// Populate the Locations list once the intro has run. The top-level list has
    /// no header.
    pub fn show(&mut self) -> Result<(), String> {
        self.map()?;
        // The helper first: the list sizes its scroll bar by whether it shows (mInfoComponentShown).
        self.states(&[STUFF_MODE, HIDE_HEADER, SHOW_HELPER, REFRESH_ITEMS, REFRESH_BUTTONS])
    }
    pub fn close(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        let root = self.bindings.movie.root;
        self.vm.call_method(root, "gotoAndPlay", vec![Value::Text("outro".into())], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// One authored frame (60 fps timeline).
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        drain(&mut self.vm, &mut self.bindings)?;
        let roots: Vec<usize> =
            self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)
    }
    /// The movie's draws plus the native map renderer's location markers, just
    /// above the map image they belong to (below the list, cursor and tabs).
    pub fn draws(&self) -> Result<Vec<Draw>, String> {
        let b = &self.bindings;
        let mut draws = crate::apt_scene::draw(&b.movie, &self.vm, &b.shapes)?;
        let Some(place) = b.place() else { return Ok(draws) };
        let (pin, [ax, ay]) = place.pin.as_ref().map_or((MARKER, [0.5, 0.5]), |(name, anchor)| (name.as_str(), *anchor));
        let Some(icon) = b.thumbnails.get(pin) else { return Ok(draws) };
        let map = place.map.as_deref().unwrap_or(WORLD_MAP);
        let Some(at) = b.thumbnails.get(map).and_then(|t| draws.iter().position(|d| d.texture == t.rgba)) else {
            return Ok(draws);
        };
        let [ox, oy] = b.marker_origin();
        // The retail marker draws 1:1 at 32 px; a ported pin at its own size.
        let (w, h) = if place.pin.is_some() { (icon.width as f32, icon.height as f32) } else { (MARKER_SIZE, MARKER_SIZE) };
        let quads = place.rows.iter().filter_map(|r| r.marker).map(|[x, y]| {
            let (left, top) = (ox + x - ax * w, oy + y - ay * h);
            let corner = |u: f32, v: f32| Vertex { position: [left + u * w, top + v * h], uv: [u, v] };
            Draw {
                texture: icon.rgba.clone(),
                size: [icon.width, icon.height],
                vertices: vec![corner(0., 0.), corner(1., 0.), corner(1., 1.), corner(0., 0.), corner(1., 1.), corner(0., 1.)],
                multiply: [1.; 4],
                add: [0.; 4],
            }
        });
        draws.splice(at + 1..at + 1, quads);
        Ok(draws)
    }
    fn forward(&mut self, key: Key) -> Result<(), String> {
        self.vm.begin_update();
        let args = vec![Value::Number(key.apt().1), Value::Number(0.)];
        self.vm.call_method(self.screen, "UpdateInput", args, &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    /// A opens a place's spots or travels to a spot; B steps back out, then closes.
    /// A spot with details opens its info panel first; A there is Teleport.
    /// One key, and the sound the screen's menu_picker plays for it, as the Start menu's
    /// sub-option picker: Up/Down `FE_SOUND_CORE_NAV_UP`/`_DOWN` when the highlight moves,
    /// A `FE_SOUND_CORE_A_BUTTON`, B the native `core_b_button`.
    pub fn input(&mut self, key: Key) -> Result<Option<Outcome>, String> {
        use crate::core_menu_runtime::{SOUND_A_BUTTON, SOUND_B_BUTTON, SOUND_NAV_DOWN, SOUND_NAV_UP};
        let before = self.bindings.highlighted;
        let outcome = self.step(key)?;
        let moved = self.bindings.highlighted != before;
        self.sounds.extend(match key {
            Key::Up if moved => Some(SOUND_NAV_UP),
            Key::Down if moved => Some(SOUND_NAV_DOWN),
            Key::Accept => Some(SOUND_A_BUTTON),
            Key::Back => Some(SOUND_B_BUTTON),
            _ => None,
        });
        Ok(outcome)
    }
    fn step(&mut self, key: Key) -> Result<Option<Outcome>, String> {
        if self.bindings.info {
            return match key {
                Key::Accept => {
                    self.forward(key)?;
                    Ok(self.bindings.open.map(|place| Outcome::Travel(place, self.bindings.highlighted)))
                }
                Key::Back => {
                    self.bindings.info = false;
                    // The info panel hid the helper; bring it back before the list sizes its scroll bar.
                    self.states(&[STUFF_MODE, SHOW_HEADER, SHOW_HELPER, REFRESH_ITEMS, REFRESH_MAP, REFRESH_CURSOR, REFRESH_BUTTONS])?;
                    Ok(None)
                }
                _ => Ok(None),
            };
        }
        match key {
            Key::Up | Key::Down => {
                self.forward(key)?;
                self.states(&[SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR])?;
                Ok(None)
            }
            // ponytail: Challenges/Completed stay empty tabs; LT/RT switch when the
            // engine has challenges to list.
            Key::Left | Key::Right => Ok(None),
            Key::Accept => {
                self.bindings.chosen = None;
                self.forward(key)?;
                let row = self.bindings.chosen.take().unwrap_or(self.bindings.highlighted);
                match self.bindings.open {
                    Some(place) if self.bindings.places[place].rows.get(row).is_some_and(|r| r.info.is_some()) => {
                        self.bindings.highlighted = row;
                        self.bindings.info = true;
                        self.states(&[INFO_MODE, REFRESH_BUTTONS])?;
                        Ok(None)
                    }
                    Some(place) if self.bindings.places[place].rows.get(row).is_some_and(|r| !r.header) => {
                        Ok(Some(Outcome::Travel(place, row)))
                    }
                    Some(_) => Ok(None),
                    None => {
                        let Some(first) = self.bindings.places.get(row).and_then(|p| p.rows.iter().position(|r| !r.header)) else {
                            return Ok(None);
                        };
                        self.bindings.open = Some(row);
                        self.bindings.highlighted = first;
                        self.states(&[SHOW_HEADER, REFRESH_ITEMS, SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR])?;
                        Ok(None)
                    }
                }
            }
            Key::Back => match self.bindings.open.take() {
                Some(place) => {
                    self.bindings.highlighted = place;
                    self.states(&[HIDE_HEADER, REFRESH_ITEMS, SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR])?;
                    Ok(None)
                }
                None => Ok(Some(Outcome::Close)),
            },
        }
    }
    #[cfg(test)]
    fn open_place(&self) -> Option<usize> {
        self.bindings.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Visual check against the owned, prepared screen (SKATE_CHALLENGE_MAP_ROOT =
    /// private/challenge-map beside private/teleports.json; PNGs in SKATE_CORE_MENU_OUT).
    #[test]
    #[ignore = "requires the prepared original challenge map"]
    fn locations_tab_browses_and_travels() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CHALLENGE_MAP_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/challengemap.json")).unwrap()).unwrap();
        let catalog = crate::teleport_menu::load(root.parent().unwrap().parent().unwrap()).unwrap();
        let installed = ["DownTown", "Industrial", "University", "SkateSchool", "StartPark", "DownTownSkatePark", "IndustrialSkatePark", "MegaPark", "MaloofMoneyCup", "BlackBoxPark"].map(String::from);
        let (places, _) = crate::challenge_map::places(&catalog, &installed, &Default::default());
        let snap = |rt: &Runtime, name: &str| {
            crate::core_menu_runtime::tests::raster_draws(&rt.draws().unwrap(), &root, &out.join(format!("{name}.png")));
        };
        let run = |rt: &mut Runtime, frames: usize| (0..frames).for_each(|_| rt.advance().unwrap());
        let markers = |rt: &Runtime| rt.draws().unwrap().iter().filter(|d| d.texture.contains(MARKER)).count();
        let mut rt = Runtime::load(&json, places).unwrap();
        // The native map viewport's blue placeholder never shows, not even during the intro.
        let placeholder = |rt: &Runtime| rt.draws().unwrap().iter().any(|d| d.multiply == [0., 0.6, 1., 1.]);
        for frame in 0..60 {
            assert!(!placeholder(&rt), "blue map placeholder on intro frame {frame}");
            rt.advance().unwrap();
        }
        assert!(rt.bindings.intro_complete);
        rt.show().unwrap();
        run(&mut rt, 30);
        snap(&rt, "challenge-map-0-locations");
        assert_eq!(markers(&rt), 0, "the place list shows no markers");
        assert_eq!(rt.bindings.cursor(), None);

        // Downtown: every location's marker, the cursor on the first.
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "A opens Downtown");
        run(&mut rt, 30);
        snap(&rt, "challenge-map-1-downtown");
        assert_eq!(markers(&rt), 14);
        assert_eq!(rt.bindings.cursor(), Some([289. + 403.1, -101. + 536.7]), "cursor on Aletown");
        for _ in 0..12 {
            rt.input(Key::Down).unwrap();
        }
        run(&mut rt, 20);
        snap(&rt, "challenge-map-2-scrolled");
        assert_eq!(rt.sounds.iter().filter(|&&s| s == crate::core_menu_runtime::SOUND_NAV_DOWN).count(), 12, "a nav sound per move");
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "A opens Tresoutta Plaza's info panel");
        assert_eq!(rt.input(Key::Accept).unwrap(), Some(Outcome::Travel(0, 12)), "13th spot: Tresoutta Plaza");
        rt.input(Key::Back).unwrap();
        assert_eq!(rt.input(Key::Back).unwrap(), None, "B returns to the place list");
        assert_eq!(rt.open_place(), None);

        // skate.School: its own map and marker.
        for _ in 0..3 {
            rt.input(Key::Down).unwrap();
        }
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        snap(&rt, "challenge-map-3-school");
        assert_eq!(rt.bindings.map_texture(), ("map_skateschool.rx2".into(), false));
        assert_eq!(markers(&rt), 1);
        rt.input(Key::Back).unwrap();

        // skate.Park: headers are skipped; each park shows its picture.
        rt.input(Key::Down).unwrap();
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        snap(&rt, "challenge-map-4-parks");
        assert_eq!(rt.bindings.highlighted, 1, "the first park, under its header");
        assert_eq!(rt.bindings.map_texture(), ("map_skatepark_downtown_full.rx2".into(), true));
        rt.input(Key::Down).unwrap();
        assert_eq!(rt.bindings.highlighted, 3, "Down skips the next header");
        assert_eq!(markers(&rt), 0);
        rt.input(Key::Back).unwrap();
        assert_eq!(rt.input(Key::Back).unwrap(), Some(Outcome::Close));
    }

    /// A location with details opens the retail info panel; A there teleports, B returns.
    #[test]
    #[ignore = "requires the prepared original challenge map"]
    fn location_details_open_the_info_panel() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CHALLENGE_MAP_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/challengemap.json")).unwrap()).unwrap();
        let catalog = crate::teleport_menu::load(root.parent().unwrap().parent().unwrap()).unwrap();
        let (places, _) = crate::challenge_map::places(&catalog, &["Sanatorium".to_string()], &Default::default());
        let snap = |rt: &Runtime, name: &str| match rt.draws() {
            Ok(draws) => crate::core_menu_runtime::tests::raster_draws(&draws, &root, &out.join(format!("{name}.png"))),
            Err(e) => eprintln!("{name}: {e}"),
        };
        let run = |rt: &mut Runtime, frames: usize| (0..frames).for_each(|_| rt.advance().unwrap());
        let mut rt = Runtime::load(&json, places).unwrap();
        run(&mut rt, 60);
        rt.show().unwrap();
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        snap(&rt, "challenge-map-5-sanatorium");
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "A opens the info panel");
        assert!(rt.bindings.info);
        run(&mut rt, 30);
        if let Value::Object(info) = rt.vm.get(rt.bindings.movie.root, "mcChallengeInfo") {
            for child in ["mHeader", "mRenderImage0", "mDescription", "mMenuPicker", "mDifficulty", "mObjectives"] {
                if let Value::Object(c) = rt.vm.get(info, child) {
                    let v = |k: &str| rt.vm.get(c, k).number();
                    eprintln!("{child}: x={} y={} h={} visible={:?}", v("_x"), v("_y"), v("_height"), rt.vm.get(c, "_visible"));
                }
            }
        }
        snap(&rt, "challenge-map-6-sanatorium-info");
        assert_eq!(rt.input(Key::Accept).unwrap(), Some(Outcome::Travel(0, 0)), "Teleport");
        rt.input(Key::Back).unwrap();
        assert!(!rt.bindings.info, "B returns to the list");
        run(&mut rt, 30);
        snap(&rt, "challenge-map-7-sanatorium-back");
        // Disc locations too: Aletown's retail photo and description.
        let (places, _) = crate::challenge_map::places(&catalog, &["DownTown".to_string()], &Default::default());
        let mut rt = Runtime::load(&json, places).unwrap();
        run(&mut rt, 60);
        rt.show().unwrap();
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        assert_eq!(rt.input(Key::Accept).unwrap(), None, "A opens Aletown's info panel");
        run(&mut rt, 30);
        snap(&rt, "challenge-map-8-aletown-info");
        assert_eq!(rt.input(Key::Accept).unwrap(), Some(Outcome::Travel(0, 0)), "Teleport");
    }

    /// The Xbox DLC places show their own map pictures with their location marked, and the
    /// DLC parks their own pictures (dlc_location_image.py installs them).
    #[test]
    #[ignore = "requires the prepared original challenge map with the DLC pictures"]
    fn dlc_places_show_their_own_maps_and_park_pictures() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CHALLENGE_MAP_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/challengemap.json")).unwrap()).unwrap();
        let catalog = crate::teleport_menu::load(root.parent().unwrap().parent().unwrap()).unwrap();
        let installed = ["Zen_Plaza", "Back_Lot_Park", "Downtown_Night_Park", "Maloof_Money_Cup_NYC", "Danny_Way_Hawaiian_Dream", "Art_Gallery"]
            .map(String::from);
        let (places, _) = crate::challenge_map::places(&catalog, &installed, &Default::default());
        let snap = |rt: &Runtime, name: &str| crate::core_menu_runtime::tests::raster_draws(&rt.draws().unwrap(), &root, &out.join(format!("{name}.png")));
        let run = |rt: &mut Runtime, frames: usize| (0..frames).for_each(|_| rt.advance().unwrap());
        let markers = |rt: &Runtime| rt.draws().unwrap().iter().filter(|d| d.texture.contains(MARKER)).count();
        let mut rt = Runtime::load(&json, places).unwrap();
        run(&mut rt, 60);
        rt.show().unwrap();
        // skate.Park first: each DLC park's picture under its header.
        rt.input(Key::Accept).unwrap();
        for (i, picture) in ["screen_zen_park_empty.rx2", "screen_backlot_park_empty.rx2", "screen_dtnight_park_empty.rx2"].into_iter().enumerate() {
            run(&mut rt, 30);
            snap(&rt, &format!("dlc-park-{i}"));
            assert_eq!(rt.bindings.map_texture(), (picture.into(), true));
            assert!(rt.draws().unwrap().iter().any(|d| d.texture == rt.bindings.thumbnails[picture].rgba), "{picture} drawn");
            rt.input(Key::Down).unwrap();
        }
        rt.input(Key::Back).unwrap();
        for (i, map) in ["dlc_maloof_map_512.rx2", "map_danny.rx2", "map_partypack.rx2"].into_iter().enumerate() {
            rt.input(Key::Down).unwrap();
            rt.input(Key::Accept).unwrap();
            run(&mut rt, 30);
            snap(&rt, &format!("dlc-map-{i}"));
            assert_eq!(rt.bindings.map_texture(), (map.into(), false));
            assert_eq!(markers(&rt), 1, "{map}: its location's marker");
            assert!(rt.bindings.cursor().is_some(), "{map}: cursor on the location");
            rt.input(Key::Back).unwrap();
        }
    }

    /// Ported Skate 2 / Skate 1 cities (runtime/locations-extra.json): each shows its own map
    /// with the game's pins, and every spot opens its photo and description.
    #[test]
    #[ignore = "requires the prepared original challenge map with the Skate 1/2 location port"]
    fn ported_cities_show_their_maps_pins_and_details() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CHALLENGE_MAP_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/challengemap.json")).unwrap()).unwrap();
        let catalog = crate::teleport_menu::load(root.parent().unwrap().parent().unwrap()).unwrap();
        let extra = crate::challenge_map::Extra::load(&root).unwrap();
        let installed = ["New_San_Vanelona", "Skate_2_Center", "Skate_2_Maloof", "Skate_1_San_Vanelona"].map(String::from);
        let (places, _) = crate::challenge_map::places(&catalog, &installed, &extra);
        let snap = |rt: &Runtime, name: &str| crate::core_menu_runtime::tests::raster_draws(&rt.draws().unwrap(), &root, &out.join(format!("{name}.png")));
        let run = |rt: &mut Runtime, frames: usize| (0..frames).for_each(|_| rt.advance().unwrap());
        let mut rt = Runtime::load(&json, places).unwrap();
        run(&mut rt, 60);
        rt.show().unwrap();
        for (index, (tag, pin)) in [("skate2", "pin_skate2"), ("skate1", "pin_skate1")].into_iter().enumerate() {
            rt.input(Key::Accept).unwrap();
            run(&mut rt, 30);
            snap(&rt, &format!("oldskate-{tag}-map"));
            let pins = rt.draws().unwrap().iter().filter(|d| d.texture.contains(pin)).count();
            assert!(pins > 10, "{tag}: {pins} pins");
            assert!(rt.bindings.cursor().is_some(), "{tag}: the first spot has a pin");
            assert_eq!(rt.input(Key::Accept).unwrap(), None, "{tag}: A opens the spot's info panel");
            assert!(rt.bindings.info);
            run(&mut rt, 30);
            snap(&rt, &format!("oldskate-{tag}-info"));
            rt.input(Key::Back).unwrap();
            rt.input(Key::Back).unwrap();
            assert_eq!(rt.open_place(), None);
            if index == 0 {
                rt.input(Key::Down).unwrap();
            }
        }
    }
}
