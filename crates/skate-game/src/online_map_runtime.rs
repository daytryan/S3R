//! Skate 3's Challenge Map (map/challengemap) in online mode: the retail Team /
//! Career / Freeskate / Versus tabs from online.json (online_map.rs), with the
//! info panel's Join Public / Create Private options for challenges and freeskate locations.
use crate::{
    apt_movie::Movie,
    apt_scene::{Draw, Shapes, Texture, Vertex},
    apt_vm::{Host, ObjectKind, Value, Vm},
    challenge_map_runtime::{
        HIDE_HEADER, INFO_MODE, LOAD_DEFAULT_MAP, MAP_ORIGIN, MARKER_SIZE, OWN_MAP_ORIGIN, REFRESH_BUTTONS, REFRESH_CURSOR,
        REFRESH_ITEMS, REFRESH_MAP, SHOW_HEADER, SHOW_HELPER, STUFF_MODE, WORLD_MAP,
    },
    core_menu_runtime::{array, boot_screen, common_call, drain, native_vm, screen_property_changed, Key, ScreenHost},
    online_map::{Challenge, Choice, Entry, Nav, OnlineMap, Pick},
};
use std::collections::{BTreeMap, BTreeSet};

/// The info panel's options, retail order (cOnlineActionProvider, not in a session).
const OPTIONS: [&str; 2] = ["ID_UNRANKED_GAME_TYPE_JOIN", "ID_UNRANKED_GAME_TYPE_CREATE"];
/// challengemap.SetState(100): mSuperTabs.SetSelectedTab(GetCurrentSuperCategoryIndex()).
const SELECT_TAB: f64 = 100.;

#[derive(Debug)]
pub(crate) enum OnlineOutcome {
    Pick(Pick),
    Close,
}

/// World (x, z) -> map1024 texels: u = a.x + b.z + c, v = d.x + e.z + f.
type Fit = [f32; 6];

/// The districts drawn on map1024; each is its own level with its own world space.
const DISTRICTS: [&str; 3] = ["DownTown", "Industrial", "University"];

/// Least-squares affine fit of one level's Locations markers (`marker`: destination id -> texel;
/// retail's challenge_map::MARKERS, or a ported map's) against their teleport destinations' translations.
pub(crate) fn fit(destinations: &[crate::teleport_menu::Destination], level: &str, marker: impl Fn(&str) -> Option<[f32; 2]>) -> Option<Fit> {
    let pairs: Vec<([f64; 2], [f64; 2])> = destinations
        .iter()
        .filter(|d| crate::map_library::same_name(&d.map, level))
        .filter_map(|d| {
            let [u, v] = marker(&d.id)?;
            let m = d.matrix?;
            Some(([m[3][0] as f64, m[3][2] as f64], [u as f64, v as f64]))
        })
        .collect();
    // One reject-and-refit pass: some teleport spots sit far from retail's marker.
    let first = solve(&pairs)?;
    let off = |([x, z], [u, v]): &([f64; 2], [f64; 2])| {
        let [x, z] = [*x as f32, *z as f32];
        ((first[0] * x + first[1] * z + first[2]) as f64 - u).hypot((first[3] * x + first[4] * z + first[5]) as f64 - v)
    };
    let mut offs: Vec<f64> = pairs.iter().map(off).collect();
    offs.sort_by(f64::total_cmp);
    let limit = (offs[offs.len() / 2] * 3.).max(1.);
    let kept: Vec<_> = pairs.iter().filter(|p| off(p) <= limit).copied().collect();
    solve(&kept).or(Some(first))
}

fn solve(pairs: &[([f64; 2], [f64; 2])]) -> Option<Fit> {
    if pairs.len() < 3 {
        return None;
    }
    // Normal equations (A^T A) p = A^T t, rows [x, z, 1].
    let mut ata = [[0f64; 3]; 3];
    let mut atb = [[0f64; 3]; 2];
    for ([x, z], t) in pairs {
        let r = [*x, *z, 1.];
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += r[i] * r[j];
            }
            for k in 0..2 {
                atb[k][i] += r[i] * t[k];
            }
        }
    }
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(ata);
    if d.abs() < 1e-9 {
        return None;
    }
    let mut out = [0f32; 6];
    for k in 0..2 {
        for c in 0..3 {
            let mut m = ata;
            (0..3).for_each(|r| m[r][c] = atb[k][r]);
            out[k * 3 + c] = (det(m) / d) as f32;
        }
    }
    Some(out)
}

/// A level drawn on its place's own map picture (challenge_map::Extra), with its pin and world -> picture fit.
#[derive(Clone)]
struct OwnMap {
    level: &'static str,
    image: String,
    pin: String,
    anchor: [f32; 2],
    fit: Option<Fit>,
}

#[derive(Clone)]
pub(crate) struct OnlineBindings {
    pub movie: Movie,
    pub shapes: Shapes,
    pub map: OnlineMap,
    pub nav: Nav,
    chosen: Option<usize>,
    /// The highlighted challenge's info panel is showing; `option` is its picker row.
    info: bool,
    option: usize,
    /// In a session: Some(hosting). The info panel then moves the session (host) or proposes it (guest).
    pub session: Option<bool>,
    thumbnails: BTreeMap<String, Texture>,
    /// Per district (DISTRICTS), world -> map1024.
    fits: Vec<(&'static str, Fit)>,
    /// Ported levels' own map pictures (the Skate 2 / Skate 1 tabs show San Vanelona, not Port Carverton).
    own: Vec<OwnMap>,
    /// Teleport destination id -> world [x, z], for freeskate spots without a start.
    teleports: BTreeMap<String, [f32; 2]>,
    slots: u32,
    random: u32,
    unanswered: BTreeSet<String>,
    pub intro_complete: bool,
    pub outro_complete: bool,
}
impl OnlineBindings {
    fn localize(&self, s: &str) -> String {
        self.movie.text_assets.localize(s)
    }
    fn entry(&self) -> Option<&Entry> {
        self.map.list(&self.nav).get(self.nav.highlighted)
    }
    fn challenge(&self) -> Option<&Challenge> {
        self.entry()?.challenge.as_ref()
    }
    fn options(&self) -> &'static [&'static str] {
        match self.session {
            None => &OPTIONS,
            Some(true) if self.challenge().is_some() => &["ID_ONLINE_MOVE_TO_CHALLENGE"],
            Some(true) => &["ID_ONLINE_MOVE_TO_LOCATION"],
            Some(false) => &["ID_ONLINE_SIGNUP_PROPOSE"],
        }
    }
    /// The folder the list shows, if any.
    fn folder(&self) -> Option<&Entry> {
        self.map.tabs.get(self.nav.tab)?.rows.get(self.nav.open?)
    }
    fn own_map(&self, level: &str) -> Option<&OwnMap> {
        self.own.iter().find(|o| crate::map_library::same_name(o.level, level))
    }
    /// The open tab's own map picture: the first of its rows on a ported level.
    fn tab_map(&self) -> Option<&OwnMap> {
        let rows = &self.map.tabs.get(self.nav.tab)?.rows;
        rows.iter().flat_map(|e| std::iter::once(e).chain(&e.rows)).find_map(|e| {
            self.own_map(e.challenge.as_ref().map(|c| &c.map).or(e.location.as_ref().map(|l| &l.map))?)
        })
    }
    fn map_texture(&self) -> String {
        self.challenge()
            .and_then(|c| c.map_image.clone())
            .or_else(|| self.tab_map().map(|o| o.image.clone()))
            .unwrap_or_else(|| WORLD_MAP.into())
    }
    /// The map showing marks its rows: Port Carverton, or the tab's own map (not a challenge's own picture).
    fn marked_map(&self) -> Option<String> {
        let image = self.map_texture();
        (image == WORLD_MAP || self.tab_map().is_some_and(|o| o.image == image)).then_some(image)
    }
    /// A row's level and world [x, z]: located challenges, and freeskate spots on a ported map
    /// (retail's world map marks challenges only).
    fn spot<'a>(&self, e: &'a Entry) -> Option<(&'a str, [f32; 2])> {
        if let Some(c) = &e.challenge {
            let [x, _, z] = c.position?;
            return Some((c.map.as_str(), [x, z]));
        }
        let l = e.location.as_ref().filter(|l| self.own_map(&l.map).is_some())?;
        let at = match &l.spawn.start {
            Some(s) => [s.matrix[3][0], s.matrix[3][2]],
            None => *self.teleports.get(l.teleport.as_ref()?)?,
        };
        Some((l.map.as_str(), at))
    }
    /// Marker centre on the stage: map1024's district fits, or a ported level's own map.
    fn texel(&self, e: &Entry) -> Option<[f32; 2]> {
        let (level, [x, z]) = self.spot(e)?;
        let (f, [ox, oy]) = match self.own_map(level) {
            Some(o) => (o.fit?, OWN_MAP_ORIGIN),
            None => (self.fits.iter().find(|(d, _)| crate::map_library::same_name(d, level))?.1, MAP_ORIGIN.map(|v| v as f32)),
        };
        Some([ox + f[0] * x + f[1] * z + f[2], oy + f[3] * x + f[4] * z + f[5]])
    }
    fn cursor(&self) -> Option<[f32; 2]> {
        self.marked_map()?;
        self.texel(self.entry()?)
    }
    fn texture(&self, file: &str) -> Option<&Texture> {
        self.thumbnails.get(file).or_else(|| self.map.icons.get(file))
    }
    /// "#" + localized objective; the online panel shows only the Own-it tier.
    fn objective(&self) -> Option<String> {
        self.challenge()?.objectives.first().map(|o| format!("#{}", self.localize(o)))
    }
}
impl ScreenHost for OnlineBindings {
    fn movie(&mut self) -> &mut Movie {
        &mut self.movie
    }
    fn random(&mut self) -> &mut u32 {
        &mut self.random
    }
}
impl Host for OnlineBindings {
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
        let text = |s: String| Value::Text(s);
        Ok(match (native.as_str(), method) {
            ("FEChallengeMap", "GetSuperCategories") => {
                let names: Vec<Value> = self.map.tabs.iter().map(|t| text(t.name.clone())).collect();
                array(vm, names)?
            }
            ("FEChallengeMap", "GetCurrentSuperCategoryIndex") => Value::Number(self.nav.tab as f64),
            // [count, selected, names, isHeader, iconState, secondIconState]
            ("FEChallengeMap", "GetStuffMenuItems") => {
                let list = self.map.list(&self.nav);
                let names: Vec<Value> =
                    list.iter().map(|e| text(format!("#{}", self.map.label(e, |s| self.localize(s))))).collect();
                let headers: Vec<Value> = list.iter().map(|e| Value::Bool(e.header)).collect();
                let n = list.len();
                let names = array(vm, names)?;
                let headers = array(vm, headers)?;
                let icons = array(vm, (0..n).map(|_| text("none".into())))?;
                let second = array(vm, (0..n).map(|_| text("none".into())))?;
                array(vm, [Value::Number(n as f64), Value::Number(self.nav.highlighted as f64), names, headers, icons, second])?
            }
            ("FEChallengeMap", "GetStuffTitleText") => {
                text(self.folder().map_or("#".into(), |f| format!("#{}", self.localize(&f.name))))
            }
            // Retail fetches a challenge's leaderboard here: "Retrieving Data...".
            ("FEChallengeMap", "GetMapHelperInfoString") => match self.entry() {
                Some(e) if e.challenge.is_some() => text("ID_MAP_LEADERBOARD_RETRIEVE".into()),
                Some(e) => text(e.help.clone().unwrap_or_else(|| "#".into())),
                None => text("#".into()),
            },
            ("FEChallengeMap", "IsSelectAvailable") => Value::Bool(true),
            ("FEChallengeMap", "GetDefaultMapTexture") => text(WORLD_MAP.into()),
            ("FEChallengeMap", "IsUsingDefaultMap") => Value::Bool(self.map_texture() == WORLD_MAP),
            // [file, is RX2, is a skatepark picture]
            ("FEChallengeMap", "GetMapTexture") => {
                let file = self.map_texture();
                array(vm, [text(file), Value::Bool(true), Value::Bool(false)])?
            }
            ("FEChallengeMap", "IsCursorVisible") => Value::Bool(self.cursor().is_some()),
            ("FEChallengeMap", "GetCursorCoordinates") => {
                let [x, y] = self.cursor().unwrap_or_default();
                array(vm, [Value::Number(x as f64), Value::Number(y as f64)])?
            }
            ("FEChallengeMap", "GetChallengeFriendsLeaderboardData") => array(vm, [Value::Number(0.), Value::Bool(false)])?,
            ("FEChallengeMap", "GetCurrentOnlineDifficultyName") => text("#".into()),
            ("FEChallengeMap", "GetChallengeTitle") => {
                text(format!("#{}", self.entry().map_or(String::new(), |e| self.localize(&e.name))))
            }
            ("FEChallengeMap", "HasOnlineDifficultySelector") => Value::Bool(false),
            ("FEChallengeMap", "GetChallengeComponentNumOptions") => Value::Number(self.options().len() as f64),
            ("FEChallengeMap", "GetChallengeComponentOption") => {
                let options = self.options();
                text(options.get(arg(0).number().max(0.) as usize).unwrap_or(&options[0]).to_string())
            }
            // (killed tier?): Own-it only online (capture 068-069).
            ("FEChallengeMap", "GetChallengeNumObjectives") => {
                let owned = !arg(0).truth() && self.objective().is_some();
                Value::Number(owned as u8 as f64)
            }
            ("FEChallengeMap", "GetChallengeObjectiveTitle") => text(self.objective().unwrap_or_else(|| "#".into())),
            ("CIC", "IsDescriptionEnabled") => Value::Bool(self.challenge().is_some()),
            ("CIC", "IsImageEnabled") => Value::Bool(false),
            ("CIC", "GetDescription") => {
                text(self.challenge().map_or("#".into(), |c| format!("#{}", self.localize(&c.description))))
            }
            ("Mission", "IsObjectivesEnabled") | ("Mission", "Online_SignUp_IsObjectivesEnabled") => {
                Value::Bool(self.objective().is_some())
            }
            ("Mission", "IsChallengeKillable") => Value::Bool(false),
            // InitRewards(killable, owned, killed, ownedBoardSales, killedBoardSales, coopReward):
            // online, only the coop slot ("Online Bonus:", capture 068) carries the bonus.
            ("Mission", "AreRewardsShownForChallenge") => Value::Bool(self.challenge().is_some()),
            ("Mission", "GetBoardSalesReward") => Value::Number(0.),
            ("Mission", "GetCoopBoardSaleBonusReward") | ("Mission", "Online_SignUp_GetBoardSalesReward") => {
                Value::Number(self.challenge().map_or(0, |c| c.bonus) as f64)
            }
            ("Mission", "ShowCareerTeammates") => Value::Bool(false),
            ("ScreenManager", "IntroComplete") => {
                self.intro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", "OutroComplete") => {
                self.outro_complete = true;
                Value::Undefined
            }
            ("ScreenManager", _) => Value::Undefined,
            // The info panel's option picker reports too; it moves the option, not the row.
            ("Game", "OnMenuHilight") if self.info => {
                self.option = arg(1).number().max(0.) as usize;
                Value::Undefined
            }
            ("Game", "OnMenuSelect") if self.info => Value::Undefined,
            ("Game", "OnMenuHilight") => {
                self.nav.highlighted = arg(1).number().max(0.) as usize;
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
            ("Game", "ImageRenderer_Load") => {
                let slot = arg(1).number().to_string();
                let path = arg(0).text().to_lowercase();
                let file = path.rsplit(['\\', '/']).next().unwrap_or_default().to_string();
                match self.texture(&file).cloned() {
                    Some(texture) => self.movie.images.insert(slot, texture),
                    None => self.movie.images.remove(&slot),
                };
                Value::Undefined
            }
            ("Game", "ImageRenderer_Unload") | ("Game", "ImageRenderer_FreeImageIndex") => {
                self.movie.images.remove(&arg(0).number().to_string());
                Value::Undefined
            }
            ("Game", _) => Value::Undefined,
            // Online_SignUp_* and the rest of the challenge natives: not shown on the map screen.
            ("FEChallengeMap" | "CIC" | "Mission", _) => {
                if self.unanswered.insert(format!("{native}.{method}")) {
                    bevy::log::warn!("online challenge map: unanswered {native}.{method}");
                }
                if method.starts_with("Online_SignUp_Get") { text("#".into()) } else { Value::Undefined }
            }
            _ => return Err(format!("Unimplemented online challenge map binding {native}.{method}")),
        })
    }
}

#[derive(Clone)]
pub(crate) struct OnlineRuntime {
    pub vm: Vm,
    pub bindings: OnlineBindings,
    screen: usize,
    /// Front-end sound ids the inputs since the last drain played.
    pub sounds: Vec<u64>,
}
// ponytail: plumbing duplicated from challenge_map_runtime; merge when the offline tabs get challenges
impl OnlineRuntime {
    /// `destinations` (teleport_menu) place challenge markers on the world map (see [`fit`]); `extra` gives
    /// the ported levels their own map pictures and pins.
    pub fn load(
        json: &serde_json::Value,
        map: OnlineMap,
        destinations: &[crate::teleport_menu::Destination],
        extra: &crate::challenge_map::Extra,
    ) -> Result<Self, String> {
        if map.tabs.is_empty() {
            return Err("Online challenge map needs tabs".into());
        }
        let mut vm = native_vm(&["FEChallengeMap", "CIC", "Mission"])?;
        let highlighted = Nav::first_selectable(&map.tabs[0].rows);
        let mut bindings = OnlineBindings {
            movie: Movie::load(json)?,
            shapes: serde::Deserialize::deserialize(&json["shapes"]).map_err(|e| e.to_string())?,
            map,
            nav: Nav { tab: 0, open: None, highlighted },
            chosen: None,
            info: false,
            option: 1,
            session: None,
            thumbnails: serde::Deserialize::deserialize(&json["thumbnails"]).map_err(|e| e.to_string())?,
            fits: DISTRICTS
                .iter()
                .filter_map(|&d| {
                    let retail = |id: &str| crate::challenge_map::MARKERS.iter().find(|(m, _)| *m == id).map(|(_, at)| *at);
                    Some((d, fit(destinations, d, retail)?))
                })
                .collect(),
            own: extra
                .own_maps()
                .into_iter()
                .map(|(level, p)| OwnMap {
                    level,
                    image: p.map.clone(),
                    pin: p.pin.clone(),
                    anchor: p.anchor,
                    fit: fit(destinations, level, |id| extra.marker(id)),
                })
                .collect(),
            teleports: destinations.iter().filter_map(|d| Some((d.id.clone(), d.matrix.map(|m| [m[3][0], m[3][2]])?))).collect(),
            slots: 0,
            random: 0x9e3779b9,
            unanswered: BTreeSet::new(),
            intro_complete: false,
            outro_complete: false,
        };
        let buttons: BTreeMap<String, Texture> = serde::Deserialize::deserialize(&json["buttons"]).unwrap_or_default();
        bindings.movie.images.extend(buttons.into_iter().map(|(name, texture)| (format!("button:{name}"), texture)));
        let screen = boot_screen(json, &mut vm, &mut bindings)?;
        let mut runtime = Self { vm, bindings, screen, sounds: Vec::new() };
        runtime.map()?;
        Ok(runtime)
    }
    pub fn intro_complete(&self) -> bool {
        self.bindings.intro_complete
    }
    pub fn outro_complete(&self) -> bool {
        self.bindings.outro_complete
    }
    /// Native calls the movie made that this host does not answer (logged once each).
    pub fn unanswered(&self) -> &BTreeSet<String> {
        &self.bindings.unanswered
    }
    fn states(&mut self, states: &[f64]) -> Result<(), String> {
        for &state in states {
            self.vm.begin_update();
            self.vm.call_method(self.screen, "SetState", vec![Value::Number(state)], &mut self.bindings)?;
            drain(&mut self.vm, &mut self.bindings)?;
        }
        Ok(())
    }
    fn map(&mut self) -> Result<(), String> {
        let root = self.bindings.movie.root;
        if let Value::Object(panel) = self.vm.get(root, "mcMapPanel") {
            self.vm.set(panel, "_visible", Value::Bool(false))?;
        }
        self.states(&[LOAD_DEFAULT_MAP, REFRESH_MAP, REFRESH_CURSOR])?;
        if let Value::Object(image) = self.vm.get(root, "mDefaultMapImage") {
            for (key, value) in [("_x", MAP_ORIGIN[0]), ("_y", MAP_ORIGIN[1])] {
                self.vm.set(image, key, Value::Number(value))?;
                screen_property_changed(&mut self.bindings.movie, &mut self.vm, image, key)?;
            }
        }
        Ok(())
    }
    pub fn show(&mut self) -> Result<(), String> {
        self.map()?;
        self.states(&[STUFF_MODE, HIDE_HEADER, SHOW_HELPER, REFRESH_ITEMS, REFRESH_BUTTONS])
    }
    pub fn close(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        let root = self.bindings.movie.root;
        self.vm.call_method(root, "gotoAndPlay", vec![Value::Text("outro".into())], &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    pub fn advance(&mut self) -> Result<(), String> {
        self.vm.begin_update();
        self.bindings.movie.advance(&mut self.vm)?;
        drain(&mut self.vm, &mut self.bindings)?;
        let roots: Vec<usize> =
            self.bindings.movie.instances.keys().copied().chain(self.bindings.movie.classes.values().copied()).collect();
        self.vm.collect(roots)
    }
    /// The movie's draws plus a marker per located row in the open list, just above
    /// the map image (Port Carverton, or the Skate 2 / Skate 1 tab's own map).
    pub fn draws(&self) -> Result<Vec<Draw>, String> {
        let b = &self.bindings;
        let mut draws = crate::apt_scene::draw(&b.movie, &self.vm, &b.shapes)?;
        // The info panel shows only the selected challenge's cursor (capture 068).
        let Some(image) = b.marked_map().filter(|_| !b.info) else { return Ok(draws) };
        let Some(at) = b.thumbnails.get(&image).and_then(|t| draws.iter().position(|d| d.texture == t.rgba)) else {
            return Ok(draws);
        };
        let own = b.tab_map();
        let quads: Vec<Draw> = b
            .map
            .list(&b.nav)
            .iter()
            .filter_map(|e| {
                // A challenge's type icon (1:1, centred); a freeskate spot the ported map's pin at its own size.
                let (icon, [ax, ay], [w, h]) = match &e.challenge {
                    Some(c) => (b.map.icons.get(&c.icon)?, [0.5, 0.5], [MARKER_SIZE; 2]),
                    None => {
                        let (pin, anchor) = own.map(|o| (&o.pin, o.anchor))?;
                        let icon = b.thumbnails.get(pin)?;
                        (icon, anchor, [icon.width as f32, icon.height as f32])
                    }
                };
                Some((icon, b.texel(e)?, [ax, ay], [w, h]))
            })
            .map(|(icon, [cx, cy], [ax, ay], [w, h])| {
                let (left, top) = (cx - ax * w, cy - ay * h);
                let corner = |u: f32, v: f32| Vertex { position: [left + u * w, top + v * h], uv: [u, v] };
                Draw {
                    texture: icon.rgba.clone(),
                    size: [icon.width, icon.height],
                    vertices: vec![corner(0., 0.), corner(1., 0.), corner(1., 1.), corner(0., 0.), corner(1., 1.), corner(0., 1.)],
                    multiply: [1.; 4],
                    add: [0.; 4],
                }
            })
            .collect();
        draws.splice(at + 1..at + 1, quads);
        Ok(draws)
    }
    fn forward(&mut self, key: Key) -> Result<(), String> {
        self.vm.begin_update();
        let args = vec![Value::Number(key.apt().1), Value::Number(0.)];
        self.vm.call_method(self.screen, "UpdateInput", args, &mut self.bindings)?;
        drain(&mut self.vm, &mut self.bindings)
    }
    pub fn input(&mut self, key: Key) -> Result<Option<OnlineOutcome>, String> {
        use crate::core_menu_runtime::{SOUND_A_BUTTON, SOUND_B_BUTTON, SOUND_NAV_DOWN, SOUND_NAV_UP};
        let before = (self.bindings.nav.tab, self.bindings.nav.highlighted, self.bindings.option);
        let outcome = self.step(key)?;
        let b = &self.bindings;
        let moved = (b.nav.highlighted, b.option) != (before.1, before.2);
        self.sounds.extend(match key {
            Key::Up if moved => Some(SOUND_NAV_UP),
            Key::Down if moved => Some(SOUND_NAV_DOWN),
            // ponytail: the movie plays no tab sound of its own; nav-down stands in.
            Key::Left | Key::Right if b.nav.tab != before.0 => Some(SOUND_NAV_DOWN),
            Key::Accept => Some(SOUND_A_BUTTON),
            Key::Back => Some(SOUND_B_BUTTON),
            _ => None,
        });
        Ok(outcome)
    }
    fn step(&mut self, key: Key) -> Result<Option<OnlineOutcome>, String> {
        let tab = self.bindings.nav.tab;
        if self.bindings.info {
            return match key {
                Key::Up | Key::Down => {
                    self.forward(key)?;
                    Ok(None)
                }
                Key::Accept => {
                    self.forward(key)?;
                    let choice = if self.bindings.option == 0 { Choice::JoinPublic } else { Choice::CreatePrivate };
                    let folder = self.bindings.folder().map(|f| f.name.clone()).unwrap_or_default();
                    let Some(entry) = self.bindings.entry().cloned() else { return Ok(None) };
                    Ok(match (entry.challenge, entry.location) {
                        (Some(challenge), _) => Some(Pick::Challenge { challenge, name: entry.name, choice, tab, folder }),
                        (None, Some(location)) => Some(Pick::Location { location, tab, name: entry.name, choice }),
                        (None, None) => None,
                    }
                    .map(OnlineOutcome::Pick))
                }
                Key::Back => {
                    self.bindings.info = false;
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
            Key::Left | Key::Right => {
                if self.bindings.nav.tab(&self.bindings.map, key == Key::Right) {
                    self.states(&[SELECT_TAB, HIDE_HEADER, REFRESH_ITEMS, SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR, REFRESH_BUTTONS])?;
                }
                Ok(None)
            }
            Key::Accept => {
                self.bindings.chosen = None;
                self.forward(key)?;
                let row = self.bindings.chosen.take().unwrap_or(self.bindings.nav.highlighted);
                self.bindings.nav.highlighted = row;
                let Some(entry) = self.bindings.entry().cloned() else { return Ok(None) };
                if entry.folder && self.bindings.nav.open.is_none() {
                    self.bindings.nav.open = Some(row);
                    self.bindings.nav.highlighted = Nav::first_selectable(&entry.rows);
                    self.states(&[SHOW_HEADER, REFRESH_ITEMS, SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR])?;
                    return Ok(None);
                }
                if let Some(command) = entry.command {
                    let folder = self.bindings.folder().map(|f| f.name.clone()).unwrap_or_default();
                    return Ok(Some(OnlineOutcome::Pick(Pick::Command { command, tab, folder })));
                }
                // Challenges and freeskate locations both ask Join Public / Create Private first.
                if entry.challenge.is_some() || entry.location.is_some() {
                    self.bindings.info = true;
                    self.bindings.option = 0;
                    self.states(&[INFO_MODE, REFRESH_BUTTONS])?;
                    // Retail opens the panel on Create Private Game (capture 068).
                    self.forward(Key::Down)?;
                }
                Ok(None)
            }
            Key::Back => {
                if self.bindings.nav.back() {
                    self.states(&[HIDE_HEADER, REFRESH_ITEMS, SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR])?;
                    Ok(None)
                } else {
                    Ok(Some(OnlineOutcome::Close))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the prepared challenge map and online.json"]
    fn online_tabs_lists_and_info_panel() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_CHALLENGE_MAP_ROOT").unwrap());
        let out = std::path::PathBuf::from(std::env::var("SKATE_CORE_MENU_OUT").unwrap_or_else(|_| ".".into()));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("runtime/challengemap.json")).unwrap()).unwrap();
        let installed: Vec<String> = ["DownTown", "Industrial", "University", "SkateSchool", "BlackBoxPark", "MaloofMoneyCup",
            "Danny_Way_Hawaiian_Dream", "Art_Gallery", "Sanatorium", "Maloof_Money_Cup_NYC", "StartPark", "DownTownSkatePark",
            "IndustrialSkatePark", "MegaPark", "Zen_Plaza", "Back_Lot_Park", "Downtown_Night_Park", "New_San_Vanelona",
            "Skate_1_San_Vanelona"].map(String::from).into();
        let map = crate::online_map::OnlineMap::load(&root, &installed).unwrap();
        let snap = |rt: &OnlineRuntime, name: &str| crate::core_menu_runtime::tests::raster_draws(&rt.draws().unwrap(), &root, &out.join(format!("{name}.png")));
        let run = |rt: &mut OnlineRuntime, n: usize| (0..n).for_each(|_| rt.advance().unwrap());
        let destinations = crate::teleport_menu::load(root.parent().unwrap().parent().unwrap()).unwrap();
        let extra = crate::challenge_map::Extra::load(&root).unwrap();
        let mut rt = OnlineRuntime::load(&json, map, &destinations, &extra).unwrap();
        assert_eq!(rt.bindings.fits.len(), 3, "world->map fit per district");
        // locations_port.py FIT: New San Vanelona texel = world (x, z) * 0.15197 + (152.3, 271.2).
        let nsv = rt.bindings.own_map("New_San_Vanelona").and_then(|o| o.fit).expect("New San Vanelona fit");
        for (got, want) in [nsv[0], nsv[2], nsv[4], nsv[5]].into_iter().zip([0.15197, 152.3, 0.15197, 271.2]) {
            assert!((got - want).abs() < 0.5f32.max(want * 0.01), "NSV fit {nsv:?}");
        }
        assert!(rt.bindings.own_map("Skate_1_San_Vanelona").and_then(|o| o.fit).is_some(), "Skate 1 fit");
        let mut offs = Vec::new();
        for (district, f) in &rt.bindings.fits {
            for (id, [u, v]) in crate::challenge_map::MARKERS {
                let found = destinations.iter().find(|d| d.id == id && crate::map_library::same_name(&d.map, district));
                let Some(m) = found.and_then(|d| d.matrix) else { continue };
                let (x, z) = (m[3][0], m[3][2]);
                let off = ((f[0] * x + f[1] * z + f[2] - u).powi(2) + (f[3] * x + f[4] * z + f[5] - v).powi(2)).sqrt();
                eprintln!("{district} {id}: fit off by {off:.1} texels");
                offs.push(off);
            }
        }
        // Teleport spots are not always where retail drew the marker; most land close.
        offs.sort_by(f32::total_cmp);
        assert!(offs[offs.len() / 2] < 12., "median fit error {:.1} texels", offs[offs.len() / 2]);
        run(&mut rt, 60);
        assert!(rt.intro_complete());
        rt.show().unwrap();
        run(&mut rt, 30);
        snap(&rt, "online-0-team");
        for (i, name) in ["online-1-career", "online-2-freeskate", "online-3-versus", "online-tab4-skate2", "online-tab5-skate1"]
            .into_iter()
            .enumerate()
        {
            assert!(rt.input(Key::Right).unwrap().is_none(), "tab {}", i + 1);
            run(&mut rt, 30);
            snap(&rt, name);
        }
        // Skate 2 > S.K.A.T.E.: its folder opens on the Versus Options + Quick Match rows, then the Skate 2 rows.
        rt.input(Key::Left).unwrap();
        let skate = rt.bindings.map.tabs[4].rows.iter().position(|e| e.name == "ID_MAP_FILTER_CHALLENGETYPES_SKATE").unwrap();
        rt.bindings.nav.highlighted = skate;
        rt.states(&[REFRESH_ITEMS]).unwrap();
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        snap(&rt, "online-tab4-skate2-skate");
        // Skate 2's tab shows New San Vanelona with Skate 2's own S.K.A.T.E. marker per game.
        assert_eq!(rt.bindings.map_texture(), "map_skate2_sanvan.rx2");
        let icon = &rt.bindings.map.icons["skate2_mini_skate"].rgba;
        let icons = rt.draws().unwrap().iter().filter(|d| &d.texture == icon).count();
        assert!(icons >= 5, "{icons} S.K.A.T.E. markers on New San Vanelona");
        rt.input(Key::Back).unwrap();
        // Skate 1's tab: Skate 1 San Vanelona with a pin per freeskate spot.
        rt.input(Key::Right).unwrap();
        rt.bindings.nav.highlighted = 0;
        rt.states(&[REFRESH_ITEMS]).unwrap();
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        snap(&rt, "online-tab5-skate1-freeskate");
        assert_eq!(rt.bindings.map_texture(), "map_skate1_sanvan.rx2");
        let pins = rt.draws().unwrap().iter().filter(|d| d.texture.contains("pin_skate1")).count();
        assert!(pins >= 10, "{pins} pins on Skate 1 San Vanelona");
        assert!(rt.bindings.cursor().is_some(), "cursor on the first Skate 1 spot");
        rt.input(Key::Back).unwrap();
        rt.input(Key::Left).unwrap();
        rt.input(Key::Left).unwrap();
        // Versus > Spot Battle (42): open, scroll to the end, open a challenge's info panel.
        // Not in a session the Versus root lists Ranked Quick/Custom Match first: 3 Downs reach Spot Battle.
        for _ in 0..3 { rt.input(Key::Down).unwrap(); }
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        snap(&rt, "online-4-spotbattle");
        for _ in 0..50 { rt.input(Key::Down).unwrap(); }
        run(&mut rt, 20);
        snap(&rt, "online-5-spotbattle-end");
        assert!(rt.input(Key::Accept).unwrap().is_none(), "A on a challenge opens its info panel");
        run(&mut rt, 30);
        snap(&rt, "online-6-info");
        assert_eq!(rt.bindings.option, 1, "panel opens on Create Private Game");
        // The movie's picker wraps: Down from Create Private reaches Join Public, then back.
        rt.input(Key::Down).unwrap();
        assert_eq!(rt.bindings.option, 0, "Down wraps to Join Public Game");
        rt.input(Key::Down).unwrap();
        match rt.input(Key::Accept).unwrap() {
            Some(OnlineOutcome::Pick(crate::online_map::Pick::Challenge { choice: crate::online_map::Choice::CreatePrivate, .. })) => {}
            other => panic!("expected Create Private Game, got {other:?}"),
        }
        // A located challenge with objectives and an Online Bonus (capture 068: Wayward Waterfall, Career).
        rt.input(Key::Back).unwrap();
        rt.input(Key::Back).unwrap();
        let tabs = &rt.bindings.map.tabs;
        let (tab, open, row) = (0..tabs.len())
            .flat_map(|t| tabs[t].rows.iter().enumerate().flat_map(move |(f, e)| e.rows.iter().enumerate().map(move |(r, c)| (t, f, r, c))))
            .find(|(_, _, _, e)| e.challenge.as_ref().is_some_and(|c| c.bonus > 0 && c.position.is_some() && !c.objectives.is_empty()))
            .map(|(t, f, r, _)| (t, f, r))
            .expect("a located challenge with a bonus");
        rt.bindings.nav = Nav { tab, open: Some(open), highlighted: row };
        rt.states(&[SELECT_TAB, SHOW_HEADER, REFRESH_ITEMS, SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR]).unwrap();
        run(&mut rt, 30);
        snap(&rt, "online-7-located");
        assert!(rt.bindings.cursor().is_some(), "cursor on the located challenge");
        rt.input(Key::Accept).unwrap();
        run(&mut rt, 30);
        snap(&rt, "online-8-located-info");
        let icon = &rt.bindings.map.icons[&rt.bindings.challenge().unwrap().icon].rgba;
        assert!(!rt.draws().unwrap().iter().any(|d| &d.texture == icon), "no row markers under the info panel");
        // A freeskate location opens the same panel, on Create Private Game.
        rt.input(Key::Back).unwrap();
        let tabs = &rt.bindings.map.tabs;
        let (tab, open, highlighted) = (0..tabs.len())
            .flat_map(|t| tabs[t].rows.iter().enumerate().map(move |(f, e)| (t, f, e)))
            .find_map(|(t, f, e)| Some((t, f, e.rows.iter().position(|r| r.location.is_some())?)))
            .expect("a freeskate location");
        rt.bindings.nav = Nav { tab, open: Some(open), highlighted };
        rt.states(&[SELECT_TAB, SHOW_HEADER, REFRESH_ITEMS, SHOW_HELPER, REFRESH_MAP, REFRESH_CURSOR]).unwrap();
        run(&mut rt, 30);
        assert!(rt.input(Key::Accept).unwrap().is_none(), "A on a location opens its panel");
        run(&mut rt, 30);
        snap(&rt, "online-9-location-info");
        match rt.input(Key::Accept).unwrap() {
            Some(OnlineOutcome::Pick(crate::online_map::Pick::Location { choice: crate::online_map::Choice::CreatePrivate, .. })) => {}
            other => panic!("expected Create Private Game at a location, got {other:?}"),
        }
        eprintln!("unanswered natives: {:?}", rt.unanswered());
    }
}
