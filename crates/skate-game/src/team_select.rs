//! Edit Skaters: retail FE slot 63 TeamManagement (`TeamSkater_Selection/team_management`) and slot 68 ImportSkater
//! (`import_skater`) on the boot screens' APT runtime (boot_screens.rs), with retail's natives answered here
//! (research.md, .superpowers/sdd/2026-10-05-team-select). Bevy-independent: [`Manager`] is the two screens'
//! native controllers (input, OnLoaded, OnMenuHilight/Select, the keyboard and popups 263/291/292/293/302; never 303, Q3) and
//! says what the host must do through [`Out`]; the plugin at the bottom hosts it over the crossbar.
//!
//! Import Skater's community list is the player's Steam friends' published skaters (decision Q2): [`FriendSkaters`]
//! is the data source the Steam relay implements; every downloaded look passes [`sanitize_friend`].
use crate::{
    apt_movie::Movie,
    apt_scene::{Draw, Texture},
    apt_vm::{Value, Vm},
    boot_flow::{Key, TextEntry, Typed},
    boot_screens::Screen,
    core_menu_runtime::{array, fe_sound_id},
    team_data::{Team, MEMBERS},
};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Card photo asset names (retail's `FrontEndState_TeamManagement_Profile_%d.Texture`).
pub(crate) fn photo_name(member: usize) -> String {
    format!("frontendstate_teammanagement_profile_{member}")
}

// ---------------------------------------------------------------------------------------------------------------
// Friends' Skaters: the data source

/// One Steam friend for Import Skater's "Friends' Skaters" list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FriendRow {
    /// Their Steam persona name (any script; drawn through text_fallback).
    pub name: String,
    /// SteamID64.
    pub id: u64,
    pub state: FriendState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FriendState {
    /// Their published skater is being looked up or downloaded.
    Loading,
    /// Downloaded: `fetch` returns it.
    Ready,
    /// They have no published skater: the row shows greyed.
    None,
}
/// The friends list behind Import Skater (decision Q2/Q2a). The Steam relay implements it: `list` is every friend
/// (offline ones too) in any order, cheap enough to call every frame; `fetch` is the friend's published boss look as
/// downloaded (the customiser profile JSON bytes of their settings/character.json), or None while it is not here.
/// The bytes are untrusted: [`sanitize_friend`] checks them before anything uses them.
pub(crate) trait FriendSkaters: Send {
    fn list(&self) -> Vec<FriendRow>;
    fn fetch(&mut self, id: u64) -> Option<Vec<u8>>;
    /// Our own skater is hidden from friends until the player accepts the Steam Workshop legal agreement.
    fn needs_agreement(&self) -> bool {
        false
    }
}
/// The shared source: the Steam relay inserts `FriendSource(Arc::new(Mutex::new(its source)))`.
pub(crate) type Friends = std::sync::Arc<std::sync::Mutex<dyn FriendSkaters>>;
/// Until the Steam relay provides one: no friends (the list shows only Preset Skaters).
pub(crate) struct NoFriends;
impl FriendSkaters for NoFriends {
    fn list(&self) -> Vec<FriendRow> {
        Vec::new()
    }
    fn fetch(&mut self, _: u64) -> Option<Vec<u8>> {
        None
    }
}
/// Test and render source: fixed rows and looks.
#[cfg(test)]
pub(crate) struct FakeFriends {
    pub rows: Vec<FriendRow>,
    pub looks: BTreeMap<u64, Vec<u8>>,
    pub needs_agreement: bool,
}
#[cfg(test)]
impl FriendSkaters for FakeFriends {
    fn list(&self) -> Vec<FriendRow> {
        self.rows.clone()
    }
    fn fetch(&mut self, id: u64) -> Option<Vec<u8>> {
        self.looks.get(&id).cloned()
    }
    fn needs_agreement(&self) -> bool {
        self.needs_agreement
    }
}

/// Longest friend name kept (Steam allows 32 characters).
const FRIEND_NAME_MAX: usize = 32;
/// The list order: alphabetical ignoring case and accents ([`sort_key`]), ties by the exact name, then the id. Names
/// are untrusted: control characters are dropped, and they are composed (NFC) so an accent a client sent decomposed
/// draws as the font's own precomposed letter.
pub(crate) fn sorted_friends(mut rows: Vec<FriendRow>) -> Vec<FriendRow> {
    for row in &mut rows {
        let clean: String = row.name.chars().filter(|c| !c.is_control()).collect();
        row.name = crate::text_fallback::nfc(&clean).chars().take(FRIEND_NAME_MAX).collect();
    }
    rows.sort_by_cached_key(|r| (sort_key(&r.name), r.name.clone(), r.id));
    rows
}
/// A collation-ish key: lowercased, decomposed (NFD), every combining mark dropped, and the Latin letters that have no
/// decomposition folded to their base ("Łukasz" sorts with L, "ß" as "ss").
// ponytail: a fold table, not full UCA collation; add icu_collator if a language's order ever matters.
pub(crate) fn sort_key(name: &str) -> String {
    let nfd = icu_normalizer::DecomposingNormalizerBorrowed::new_nfd();
    let ccc = icu_normalizer::properties::CanonicalCombiningClassMapBorrowed::new();
    let mut key = String::new();
    for c in nfd.normalize(&name.to_lowercase()).chars().filter(|&c| ccc.get_u8(c) == 0) {
        match c {
            'ø' => key.push('o'),
            'ł' => key.push('l'),
            'đ' | 'ð' => key.push('d'),
            'ı' => key.push('i'),
            'ħ' => key.push('h'),
            'ŧ' => key.push('t'),
            'ŋ' => key.push('n'),
            'þ' => key.push_str("th"),
            'ß' => key.push_str("ss"),
            'æ' => key.push_str("ae"),
            'œ' => key.push_str("oe"),
            c => key.push(c),
        }
    }
    key
}

/// The look fields the engine reads (customiser_parts, cas_pose, the board): a friend's look keeps these alone.
const LOOK_KEYS: [&str; 19] = [
    "gender", "selections", "morphs", "colours", "skin_tint", "hair_tint", "skin", "brow", "beard", "hair_choice", "truck",
    "wheel", "stance", "style", "posture", "gestures", "tattoos", "graphics", "grip",
];

/// A friend's downloaded look through the network appearance trust boundary: the peer outfit size cap before anything
/// parses it, then only the [`LOOK_KEYS`] (never a model file, an image or an unknown field), each worn item only its
/// asset and material ids, creator designs dropped (multiplayer::appearance::read_look's decoding). The host then
/// resolves it with `Parts::resolve` (strict validation, stock-logo sanitize) before use.
pub(crate) fn sanitize_friend(bytes: &[u8]) -> Result<Json, String> {
    use crate::multiplayer::appearance::{read_look, Look, MAX_OUTFIT};
    if bytes.len() > MAX_OUTFIT {
        return Err("Over the outfit size cap".into());
    }
    let Json::Object(mut profile) = serde_json::from_slice(bytes).map_err(|e| e.to_string())? else {
        return Err("Not a skater".into());
    };
    profile.retain(|key, _| LOOK_KEYS.contains(&key.as_str()));
    let item = |worn: &Json| match (worn["asset_id"].as_str(), worn["material_id"].as_str()) {
        (Some(asset), Some(material)) => serde_json::json!({ "asset_id": asset, "material_id": material }),
        _ => Json::Null,
    };
    if let Some(Json::Object(selections)) = profile.get_mut("selections") {
        for worn in selections.values_mut() {
            *worn = item(worn);
        }
    }
    if let Some(hair) = profile.get_mut("hair_choice") {
        *hair = item(hair);
    }
    let wrapped = serde_json::to_vec(&Look::Outfit(Json::Object(profile))).map_err(|e| e.to_string())?;
    match read_look(&wrapped)? {
        Look::Outfit(mut profile) => {
            crate::custom_graphic::without_designs(&mut profile);
            Ok(profile)
        }
        _ => Err("Not a skater".into()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The pipeline's team data (tools/prepare_boot.py `_team`: runtime/team.json)

#[derive(serde::Deserialize, Clone)]
pub(crate) struct Logo {
    /// Vault key (16 hex digits).
    pub key: String,
    /// Texture asset name.
    pub texture: String,
}
#[derive(serde::Deserialize, Clone)]
pub(crate) struct Preset {
    /// ID_TEAMMATE_PRESET_xx.
    pub name: String,
    pub profile: Json,
}
#[derive(serde::Deserialize, Clone, Debug)]
pub(crate) struct PhotoSetup {
    pub camera: [f32; 3],
    pub target: [f32; 3],
    pub colour: [f32; 4],
    pub background: Option<String>,
    pub width: u32,
    pub height: u32,
    /// The editor clip the skater holds (cac_body: CAS_Single_Pose_N); none in older boot data.
    #[serde(default)]
    pub pose: Option<String>,
    /// The lens (degrees, vertical; metres): the vault's defaults when older boot data has none.
    #[serde(default = "default_fov")]
    pub fov: f32,
    #[serde(default = "default_near")]
    pub near: f32,
    #[serde(default = "default_far")]
    pub far: f32,
    /// The render location's sun in the camera's right/up/forward basis, and its 9 SH rows (none in older data).
    #[serde(default)]
    pub sun: Option<[f32; 3]>,
    #[serde(default)]
    pub sh: Option<Vec<[f32; 3]>>,
}
fn default_fov() -> f32 {
    45.
}
fn default_near() -> f32 {
    0.01
}
fn default_far() -> f32 {
    200.
}
#[derive(serde::Deserialize, Clone)]
pub(crate) struct TeamFile {
    pub logos: Vec<Logo>,
    pub presets: Vec<Preset>,
    pub photos: BTreeMap<String, PhotoSetup>,
    pub thumbnails: BTreeMap<String, Texture>,
    /// The photo backgrounds' art, hashed ([`TeamFile::read`]): part of every photo's cache name, so re-running the
    /// boot stage with changed art redraws them.
    #[serde(skip)]
    pub fingerprint: String,
}
impl TeamFile {
    /// runtime/team.json under the boot data, with its [`TeamFile::fingerprint`].
    pub(crate) fn read(root: &std::path::Path) -> Result<Self, String> {
        let path = root.join("runtime/team.json");
        let json: Json = serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())?;
        if json["format"] != "skate3-boot" || json["version"] != 1 {
            return Err("Unsupported team data".into());
        }
        let mut team: Self = serde_json::from_value(json).map_err(|e| e.to_string())?;
        let mut hash = blake3::Hasher::new();
        for texture in team.photos.values().filter_map(|s| team.thumbnails.get(s.background.as_deref()?)) {
            hash.update(&std::fs::read(root.join(&texture.rgba)).unwrap_or_default());
        }
        team.fingerprint = hash.finalize().to_hex()[..32].to_owned();
        Ok(team)
    }
}

/// Everything the screens need from the prepared boot data (SKATE_BOOT_ROOT / assets/private/boot).
pub(crate) struct Data {
    pub team_management: Json,
    pub import_skater: Json,
    pub popup: Json,
    pub team: TeamFile,
}
impl Data {
    pub(crate) fn load(root: &std::path::Path) -> Result<Self, String> {
        let read = |name: &str| -> Result<Json, String> {
            let path = root.join(format!("runtime/{name}.json"));
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())
        };
        let team = TeamFile::read(root)?;
        Ok(Self { team_management: read("team_management")?, import_skater: read("import_skater")?, popup: read("sk8popup")?, team })
    }
    pub(crate) fn text(&self, id: &str) -> String {
        self.team_management["language"][id].as_str().map_or_else(|| id.to_owned(), str::to_owned)
    }
    /// The default team for a new or migrated career: the first logo, presets 01-04 as the four teammates.
    pub(crate) fn defaults(&self) -> (String, String, Vec<(String, Json)>) {
        let logo = self.team.logos.first().map(|l| l.key.clone()).unwrap_or_default();
        let teammates = self.team.presets.iter().take(MEMBERS - 1).map(|p| (self.text(&p.name), p.profile.clone())).collect();
        (self.text("ID_WELCOME_NEW_TEAM_NAME"), logo, teammates)
    }
    /// The logo list, the saved one appended when it is not among them (826409d0).
    pub(crate) fn logos(&self, saved: &str) -> Vec<Logo> {
        let mut logos = self.team.logos.clone();
        if !saved.is_empty() && !logos.iter().any(|l| l.key == saved) {
            logos.push(Logo { key: saved.to_owned(), texture: "missingtexture".into() });
        }
        logos
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The natives the two movies call

/// A row of Import Skater's list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// What the picker shows (a string id, `#` and tokens, or an apt_text::literal name).
    pub label: String,
    pub header: bool,
    pub enabled: bool,
}

/// `TeamManagement.*`, `ImportSkaters.*`, the image slots and the menu reports, for one loaded screen.
#[derive(Default, Clone)]
pub(crate) struct Natives {
    pub count: usize,
    pub team_name: String,
    /// The team logo's texture asset name.
    pub logo: String,
    pub names: Vec<String>,
    /// The selected card ([0x83083a04]).
    pub selected: usize,
    /// Slot 63 mode (+68): 0 teammates, 1 logo grid, 2 new-career logos.
    pub mode: u8,
    pub logos: Vec<String>,
    /// Loadable images by lowercase asset name: the pipeline's thumbnails, the card photos and the previews.
    pub images: BTreeMap<String, Texture>,
    pub rows: Vec<Row>,
    /// Import Skater: row -> its preview photo's asset name, once rendered.
    pub previews: BTreeMap<usize, String>,
    /// Image slot -> the asset name it shows (re-pointed when that image is re-rendered).
    loaded: BTreeMap<String, String>,
    slots: u32,
    /// `Game.OnMenuHilight(menu, index)` and `TeamManagement.SelectLogo(index, preview)` since the last take.
    pub hilight: Option<(i64, i64)>,
    pub logo_pick: Option<(usize, bool)>,
}
impl Natives {
    /// The movies' native calls this screen answers (None: not one of them).
    pub(crate) fn call(&mut self, movie: &mut Movie, vm: &mut Vm, native: &str, method: &str, args: &[Value]) -> Result<Option<Value>, String> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let index = |i: usize| arg(i).number().max(0.) as usize;
        let text = |s: &str| Value::Text(s.to_owned());
        Ok(Some(match (native, method) {
            // [count, team name, logo asset, photo asset per card]: the boot's first-member screen reads [2] and [3].
            ("TeamManagement", "GetTeamData") => {
                // The movie shows "#" + the name: the name goes as the rest of an apt_text::literal.
                let mut values = vec![Value::Number(self.count as f64), text(&unprefixed_literal(&self.team_name)), text(&self.logo)];
                values.extend((0..MEMBERS).map(|k| text(&if k < self.count { photo_name(k) } else { String::new() })));
                array(vm, values)?
            }
            ("TeamManagement", "GetSelectedTeammateData") => {
                let name = self.names.get(self.selected).map_or_else(String::new, |n| unprefixed_literal(n));
                array(vm, [Value::Number(self.selected as f64), Value::Text(name)])?
            }
            // 0x825c39e8: the three options for an unlocked card in mode 0, else none (Select Skater, the engine's
            // fourth, is added after: Manager::set_state).
            ("TeamManagement", "GetEditSkaterOptions") => {
                if self.mode == 0 && self.selected < self.count {
                    let options = ["ID_CAC_EDITSKATER_TITLE", "ID_TEAM_MANAGEMENT_CHANGE_NAME", "ID_TEAM_MANAGEMENT_IMPORT_SKATER"];
                    array(vm, std::iter::once(Value::Number(3.)).chain(options.map(text)))?
                } else {
                    array(vm, [Value::Number(0.)])?
                }
            }
            ("TeamManagement", "GetNumTeamLogos") => Value::Number(self.logos.len() as f64),
            ("TeamManagement", "GetLogoAssetID") => text(self.logos.get(index(0)).map_or("missingtexture", String::as_str)),
            ("TeamManagement", "SelectLogo") => {
                self.logo_pick = Some((index(0), arg(1) == Value::Bool(true)));
                Value::Undefined
            }
            ("TeamManagement", _) => Value::Undefined,
            ("ImportSkaters", "GetSkaters") => {
                let labels: Vec<Value> = self.rows.iter().map(|r| Value::Text(r.label.clone())).collect();
                let headers: Vec<Value> = self.rows.iter().map(|r| Value::Bool(r.header)).collect();
                let (labels, headers) = (array(vm, labels)?, array(vm, headers)?);
                array(vm, [Value::Number(self.rows.len() as f64), labels, headers])?
            }
            ("ImportSkaters", "IsPreviewPhotoLoaded") => Value::Bool(self.previews.contains_key(&index(0))),
            ("ImportSkaters", "GetPreviewPhotoAssetId") => text(self.previews.get(&index(0)).map_or("", String::as_str)),
            ("ImportSkaters", _) => Value::Undefined,
            ("Game", "OnMenuHilight") => {
                self.hilight = Some((arg(0).number() as i64, arg(1).number() as i64));
                Value::Undefined
            }
            // Headers and friends with no published skater are greyed (the picker skips them).
            ("Game", "OnMenuGetItemEnabled") if !self.rows.is_empty() => {
                Value::Bool(self.rows.get(index(1)).is_some_and(|r| r.enabled && !r.header))
            }
            ("Game", "ImageRenderer_ReserveNextFreeIndex") => {
                self.slots += 1;
                Value::Number(self.slots as f64)
            }
            // (asset name, slot): the image_renderer's LoadAsset / LoadAssetByName / Load.
            ("Game", "ImageRenderer_LoadAsset" | "ImageRenderer_LoadAssetByName" | "ImageRenderer_Load") => {
                let name = args.iter().find_map(|a| matches!(a, Value::Text(_)).then(|| a.text().to_lowercase())).unwrap_or_default();
                let slot = args.iter().find_map(|a| matches!(a, Value::Number(_)).then(|| a.number().to_string())).unwrap_or_default();
                match self.images.get(&name) {
                    Some(texture) => movie.images.insert(slot.clone(), texture.clone()),
                    None => movie.images.remove(&slot),
                };
                self.loaded.insert(slot, name);
                Value::Undefined
            }
            ("Game", "ImageRenderer_Unload" | "ImageRenderer_FreeImageIndex") => {
                let slot = arg(0).number().to_string();
                movie.images.remove(&slot);
                self.loaded.remove(&slot);
                Value::Undefined
            }
            ("Game", m) if m.starts_with("ImageRenderer_") => Value::Undefined,
            _ => return Ok(None),
        }))
    }
    /// A (re-)rendered image: stored, and shown at once in every slot that loaded it.
    pub(crate) fn set_image(&mut self, movie: &mut Movie, name: &str, texture: Texture) {
        let name = name.to_lowercase();
        for (slot, loaded) in &self.loaded {
            if *loaded == name {
                movie.images.insert(slot.clone(), texture.clone());
            }
        }
        self.images.insert(name, texture);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The controller

/// Select Skater's label: the game's own string ("Select Skater", the demo's skater picker header), there being no
/// retail option to word it.
const SELECT_SKATER: &str = "ID_DEMO_SELECT_SKATER_HEADER";

/// A name for a movie that puts the "#" in front itself (team_management's `"#" + name`).
fn unprefixed_literal(name: &str) -> String {
    crate::apt_text::literal(name)[1..].to_owned()
}

/// What the host does for the screens.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Out {
    Sound(u64),
    /// Edit Skater: the CAS on member k (0 = the boss); call [`Manager::reenter`] when it closes.
    EditSkater(usize),
    /// Select Skater (the engine's): the player skates as member k from now on, its look on the skater in place (no
    /// reload); the team, saved, remembers it.
    SkateAs(usize),
    /// Render member k's card photo from its saved look; answer with [`Manager::photo_ready`].
    Photo(usize),
    /// Render an Import Skater preview of `look` (sanitized, not yet resolved) with member k's photo setup
    /// (`team_profile_0k`, 0x82653858); answer with [`Manager::preview_ready`].
    Preview { key: String, member: usize, look: Json },
    /// Import: resolve `look` (Parts::resolve) and write it as member k's look; answer with [`Manager::import_applied`],
    /// or [`Manager::import_failed`] if it is refused (popup 292, still on Import Skater).
    Apply { member: usize, look: Json },
    /// The screen popped (the team saved first if it changed).
    Closed,
    /// Open the Steam Workshop legal agreement in Steam (the presence helper's `steam://` path).
    OpenAgreement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Popup {
    /// 291 / 293: confirm the import of row `.0`.
    Confirm(usize),
    /// 292.
    Error,
    /// 302: the team name is empty; its OK opens the rename keyboard.
    EmptyTeam,
    /// 263: invalid characters; its OK reopens the keyboard.
    Invalid,
    /// Engine notice (Q2, not retail): our skater is hidden from friends until the Workshop agreement is accepted.
    Agreement,
}
impl Popup {
    fn text(self, mode: u8) -> (&'static str, &'static [&'static str]) {
        match self {
            Popup::Confirm(_) if mode == 0 => ("ID_IMPORT_SKATER_CONFIRM_IMPORT", &["ID_COMMON_NO", "ID_COMMON_YES"]),
            Popup::Confirm(_) => ("ID_CONFIRM_USE_PRESET", &["ID_COMMON_NO", "ID_COMMON_YES"]),
            Popup::Error => ("ID_IMPORT_SKATER_ERROR", &["ID_COMMON_OK"]),
            Popup::EmptyTeam => ("ID_TEAM_MANAGEMENT_EMPTY_TEAM_NAME", &["ID_COMMON_OK"]),
            Popup::Invalid => ("ID_VIRTUALKEYBOARD_ALPHA_NUMERIC_STRING_ERROR", &["ID_COMMON_OK"]),
            Popup::Agreement => {
                ("#Your skater is hidden from friends until you accept the Steam Workshop agreement.", &["ID_COMMON_OK", "#Open in Steam"])
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Naming {
    Team,
    Member(usize),
}
pub(crate) struct Keyboard {
    pub entry: TextEntry,
    pub header: String,
    pub purpose: Naming,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Header,
    Friend(u64),
    Preset(usize),
}
struct Importer {
    screen: Screen,
    member: usize,
    /// Slot 68 +784: 0 from Edit Skaters, 1 a new teammate, 2 the new career's first member.
    mode: u8,
    sources: Vec<Source>,
    friends: Vec<FriendRow>,
    hilight: usize,
}

/// FE slots 63 and 68 with the keyboard and popups over them.
pub(crate) struct Manager {
    pub data: Data,
    pub team: Team,
    pub settings: PathBuf,
    friends: Friends,
    natives: Natives,
    screen: Option<Screen>,
    import: Option<Importer>,
    popup: Option<(Popup, crate::popup_runtime::Runtime)>,
    pub keyboard: Option<Keyboard>,
    /// Import Skater opened on its own (the boot's first member): its close is the manager's.
    standalone: bool,
    /// The Workshop agreement notice has been shown (once a session).
    agreement_shown: bool,
    /// +73: B was pressed; pop on the next update (popup 303 never shows, user ruling Q3: photos come from the cache
    /// or finish drawing after the screen is gone).
    exit: bool,
    /// +72: the team changed.
    dirty: bool,
    /// The CAS (Edit Skater) has the screen.
    pub away: bool,
    /// Card photos being rendered (the photo queue).
    pending: BTreeSet<usize>,
    /// Card photos asked for again while being rendered (the look changed under them): redrawn when they land.
    again: BTreeSet<usize>,
    /// Import Skater previews rendered or under way.
    previews: BTreeMap<String, Option<Texture>>,
    /// Previews being rendered (slot 68 +792, the preview request: A and B wait for it).
    rendering: BTreeSet<String>,
    /// Photo job key ([`Manager::job_key`]) -> its member (None: an Import Skater preview), for the photos the host
    /// is drawing.
    photo_keys: BTreeMap<String, Option<usize>>,
    /// This manager's photo jobs are named `<id>:<asset name>`: one another manager left behind (the boot's, when it
    /// ends mid-draw) is never taken for this one's.
    id: u64,
    /// ALL_VALID_ALPHANUMERIC_CHARACTERS (keyboard type-0 validation).
    valid: String,
    out: Vec<Out>,
}

/// The card's options picker (UpdateMenu's mMenuPicker).
const OPTIONS: &str = "mTeamManagement.mContent.mPlayerCards.mSkaterInfo.mMenuPicker";
const SCREEN_TEAMMATES: f64 = 10.;
const SCREEN_TEAMMATE_INFO: f64 = 11.;
const SCREEN_LOGOS: f64 = 20.;
const SCREEN_TEAM_LOGO: f64 = 21.;
const SCREEN_NEW_CAREER_LOGOS: f64 = 30.;
const IMPORT_SKATERS: f64 = 10.;
const IMPORT_PREVIEW: f64 = 11.;

impl Manager {
    pub(crate) fn new(data: Data, team: Team, settings: PathBuf, friends: Friends) -> Self {
        let valid = data.text("ALL_VALID_ALPHANUMERIC_CHARACTERS");
        let mut natives = Natives::default();
        natives.images = data.team.thumbnails.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
        Self {
            data,
            team,
            settings,
            friends,
            natives,
            screen: None,
            import: None,
            popup: None,
            keyboard: None,
            standalone: false,
            agreement_shown: false,
            exit: false,
            dirty: false,
            away: false,
            pending: BTreeSet::new(),
            again: BTreeSet::new(),
            previews: BTreeMap::new(),
            rendering: BTreeSet::new(),
            photo_keys: BTreeMap::new(),
            id: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            valid,
            out: Vec::new(),
        }
    }
    /// The photo job key of one of this manager's images.
    fn job_key(&self, name: &str) -> String {
        format!("{}:{name}", self.id)
    }
    pub(crate) fn take(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }
    fn sound(&mut self, key: &str) {
        self.out.push(Out::Sound(fe_sound_id(key)));
    }
    fn logos(&self) -> Vec<Logo> {
        self.data.logos(&self.team.logo)
    }
    /// The natives' view of the team (before a screen loads and after anything changes).
    fn sync(&mut self) {
        let logos = self.logos();
        let n = &mut self.natives;
        n.count = self.team.count();
        n.team_name = self.team.name.clone();
        n.names = (0..n.count).map(|k| self.team.member_name(k).to_owned()).collect();
        n.logo = logos.iter().find(|l| l.key == self.team.logo).map_or_else(|| "missingtexture".into(), |l| l.texture.clone());
        n.logos = logos.into_iter().map(|l| l.texture).collect();
        // The loaded screen's copy: the team fields only (its image slots stay as the movie made them).
        if let Some(screen) = self.screen.as_mut() {
            let t = &mut screen.bindings.team;
            (t.count, t.team_name, t.names, t.logo, t.logos) = (n.count, n.team_name.clone(), n.names.clone(), n.logo.clone(), n.logos.clone());
            (t.selected, t.mode) = (n.selected, n.mode);
        }
    }
    fn set_state(&mut self, state: f64) -> Result<(), String> {
        let Some(screen) = self.screen.as_mut() else { return Ok(()) };
        screen.set_state(state)?;
        // Select Skater (the engine's: retail's player is always the boss) on a card the player does not skate as: a
        // fourth item after the movie's three (UpdateMenu, from states 10 and 11). The picker keeps its three-row
        // window and scrolls to it, as retail's longer pickers do: a fourth row would lie over the button bar.
        let selected = self.natives.selected;
        if [SCREEN_TEAMMATES, SCREEN_TEAMMATE_INFO].contains(&state)
            && self.natives.mode == 0
            && selected < self.team.count()
            && selected != self.team.active
        {
            screen.call_clip(OPTIONS, "AddItem", vec![Value::Text(SELECT_SKATER.into())])?;
            screen.call_clip(OPTIONS, "Update", Vec::new())?;
        }
        self.drain_sounds();
        Ok(())
    }
    fn drain_sounds(&mut self) {
        for screen in self.screen.iter_mut().chain(self.import.as_mut().map(|i| &mut i.screen)) {
            self.out.extend(screen.take_sounds().into_iter().map(Out::Sound));
        }
    }

    /// Edit Skaters from the crossbar (slot 63 mode 0).
    pub(crate) fn open(&mut self) -> Result<(), String> {
        self.standalone = false;
        self.enter(0)
    }
    /// The new career's logo picker (slot 63 mode 2, state 30): A commits the logo and closes.
    pub(crate) fn open_new_career_logos(&mut self) -> Result<(), String> {
        self.standalone = false;
        self.enter(2)
    }
    /// Import Skater alone (the boot's first member: mode 2, member 0).
    pub(crate) fn open_import(&mut self, member: usize, mode: u8) -> Result<(), String> {
        self.standalone = true;
        self.exit = false;
        self.start_import(member, mode)
    }
    /// OnEnter 0x82651900 + OnLoaded 0x82651a48: refresh every photo, mode 0 (2 for a new career), load, then
    /// SetState 10 and 11 (30 for a new career); an empty team name gets popup 302.
    fn enter(&mut self, mode: u8) -> Result<(), String> {
        self.away = false;
        self.exit = false;
        self.natives.mode = mode;
        if mode != 2 {
            for k in 0..self.team.count() {
                self.queue_photo(k);
            }
        }
        let json = &self.data.team_management;
        let mut screen = Screen::load(json, 0., &self.team.boss, Vec::new())?;
        screen.bindings.team.images = self.natives.images.clone();
        self.screen = Some(screen);
        self.sync();
        if mode == 2 {
            self.set_state(SCREEN_NEW_CAREER_LOGOS)?;
        } else {
            self.set_state(SCREEN_TEAMMATES)?;
            self.set_state(SCREEN_TEAMMATE_INFO)?;
            if self.team.name.is_empty() {
                self.open_popup(Popup::EmptyTeam)?;
            }
        }
        Ok(())
    }
    /// Back from the CAS (Edit Skater): slot 63 enters again.
    pub(crate) fn reenter(&mut self) -> Result<(), String> {
        self.enter(0)
    }
    fn queue_photo(&mut self, member: usize) {
        if self.pending.insert(member) {
            self.out.push(Out::Photo(member));
        } else {
            self.again.insert(member);
        }
    }
    /// Member k's card photo is rendered (one asked for again meanwhile is queued anew).
    pub(crate) fn photo_ready(&mut self, member: usize, texture: Option<Texture>) {
        self.pending.remove(&member);
        if self.again.remove(&member) {
            self.queue_photo(member);
        }
        let Some(texture) = texture else { return };
        // Each look has its own cache file, so the canvas (which caches textures by path) shows a changed look's.
        self.natives.images.insert(photo_name(member), texture.clone());
        if let Some(screen) = self.screen.as_mut() {
            let movie = &mut screen.bindings.movie;
            screen.bindings.team.set_image(movie, &photo_name(member), texture);
        }
    }
    /// An Import Skater preview is rendered.
    pub(crate) fn preview_ready(&mut self, key: &str, texture: Option<Texture>) -> Result<(), String> {
        self.previews.insert(key.to_owned(), texture.clone());
        self.rendering.remove(key);
        let Some(import) = self.import.as_mut() else { return Ok(()) };
        if let Some(texture) = texture {
            let movie = &mut import.screen.bindings.movie;
            import.screen.bindings.team.set_image(movie, key, texture);
        }
        let shown = preview_key(import.sources.get(import.hilight).copied(), import.member);
        self.show_preview(shown.as_deref() == Some(key))
    }
    /// The natives as the team stands (the boot's first-member screen answers from them).
    pub(crate) fn natives(&mut self) -> Natives {
        self.sync();
        self.natives.clone()
    }
    pub(crate) fn image(&self, name: &str) -> Option<&Texture> {
        self.natives.images.get(name)
    }
    pub(crate) fn request_photo(&mut self, member: usize) {
        self.queue_photo(member);
    }
    pub(crate) fn is_open(&self) -> bool {
        self.screen.is_some() || self.import.is_some() || self.away
    }

    fn open_popup(&mut self, popup: Popup) -> Result<(), String> {
        let mode = self.import.as_ref().map_or(0, |i| i.mode);
        let (text, options) = popup.text(mode);
        let cancelable = matches!(popup, Popup::Confirm(_));
        let runtime = crate::popup_runtime::Runtime::load(&self.data.popup, text, options, cancelable)?;
        self.popup = Some((popup, runtime));
        // Popup OnEnter (0x825EF528).
        self.sound("core_popup");
        Ok(())
    }
    fn open_keyboard(&mut self, purpose: Naming) {
        let (header, default) = match purpose {
            Naming::Team => ("ID_TEAM_MANAGEMENT_ENTER_NEW_TEAM_NAME", self.team.name.clone()),
            Naming::Member(k) => ("ID_TEAM_MANAGEMENT_ENTER_NEW_TEAMMATE_NAME", self.team.member_name(k).to_owned()),
        };
        self.keyboard = Some(Keyboard { entry: TextEntry::new(&default, crate::boot_flow::TEAM_NAME_MAX), header: self.data.text(header), purpose });
    }
    /// Text typed on the PC keyboard while the on-screen keyboard is open.
    pub(crate) fn type_text(&mut self, text: &str) {
        if let (Some(keyboard), None) = (self.keyboard.as_mut(), self.popup.as_ref()) {
            keyboard.entry.type_text(text);
        }
    }
    fn save(&mut self) {
        if let Err(error) = self.team.save(&self.settings) {
            bevy::log::warn!("Team save: {error}");
        }
    }

    /// A key, routed as the front end does: popup, keyboard, Import Skater, else slot 63's native input.
    pub(crate) fn input(&mut self, key: Key) -> Result<(), String> {
        if self.away {
            return Ok(());
        }
        if let Some((popup, runtime)) = self.popup.as_mut() {
            let popup = *popup;
            let Some(apt) = key.apt() else { return Ok(()) };
            let picked = runtime.input(apt)?;
            self.out.extend(runtime.bindings.sounds.drain(..).map(Out::Sound));
            if let Some(choice) = picked {
                self.popup = None;
                self.popup_choice(popup, choice)?;
            }
            return Ok(());
        }
        if let Some(keyboard) = self.keyboard.as_mut() {
            match keyboard.entry.input(key) {
                Some(Typed::Done) => {
                    let (text, purpose) = (keyboard.entry.text.clone(), keyboard.purpose);
                    // Keyboard type-0 validation (0x82639590): popup 263, whose OK returns to the keyboard.
                    if !text.chars().all(|c| self.valid.contains(c)) {
                        return self.open_popup(Popup::Invalid);
                    }
                    self.keyboard = None;
                    self.named(purpose, &text)?;
                }
                Some(Typed::Cancel) => self.keyboard = None,
                None => {}
            }
            return Ok(());
        }
        if self.import.is_some() {
            return self.import_input(key);
        }
        if self.screen.is_none() {
            return Ok(());
        }
        // 0x82651ee0: mode 2 sends every key to the movie; otherwise B, Y and X are the native's.
        if self.natives.mode != 2 {
            match key {
                Key::B => {
                    self.sound("core_b_button");
                    if self.natives.mode == 1 {
                        self.natives.mode = 0;
                        self.sync();
                        self.set_state(SCREEN_TEAMMATES)?;
                        return self.set_state(SCREEN_TEAMMATE_INFO);
                    }
                    self.exit = true;
                    return Ok(());
                }
                Key::Y if self.natives.mode == 0 => {
                    self.sound("core_y_button");
                    self.natives.mode = 1;
                    self.sync();
                    return self.set_state(SCREEN_LOGOS);
                }
                Key::X if self.natives.mode == 0 => {
                    self.open_keyboard(Naming::Team);
                    return Ok(());
                }
                Key::X | Key::Y | Key::Start | Key::StartReleased => return Ok(()),
                _ => {}
            }
        }
        let Some(apt) = key.apt() else { return Ok(()) };
        let screen = self.screen.as_mut().unwrap();
        screen.bindings.team.hilight = None;
        screen.bindings.team.logo_pick = None;
        let picked = screen.input_pad(apt)?;
        let (hilight, logo) = (screen.bindings.team.hilight.take(), screen.bindings.team.logo_pick.take());
        self.drain_sounds();
        // OnMenuHilight 0x82651e50 (menu 0, the cards): wrapped into the unlocked cards, then SetState 11.
        if let Some((0, index)) = hilight {
            let count = self.team.count() as i64;
            self.natives.selected = index.rem_euclid(count.max(1)) as usize;
            self.sync();
            self.set_state(SCREEN_TEAMMATE_INFO)?;
        }
        if let Some((index, preview)) = logo {
            self.select_logo(index, preview)?;
        }
        if let Some(option) = picked {
            self.option(option)?;
        }
        Ok(())
    }
    /// SelectLogo: from the crossbar it sets the logo, re-renders every photo and stays on the grid (SetState 21); a
    /// new career previews it while moving and A commits it and closes.
    fn select_logo(&mut self, index: usize, preview: bool) -> Result<(), String> {
        let Some(logo) = self.logos().get(index).cloned() else { return Ok(()) };
        self.team.logo = logo.key;
        self.sync();
        if self.natives.mode == 2 {
            self.set_state(SCREEN_TEAM_LOGO)?;
            if !preview {
                self.save();
                self.close();
            }
            return Ok(());
        }
        self.dirty = true;
        for k in 0..self.team.count() {
            self.queue_photo(k);
        }
        self.set_state(SCREEN_TEAM_LOGO)
    }
    /// OnMenuSelect 0x82651c70 (menu 1, the options).
    fn option(&mut self, option: usize) -> Result<(), String> {
        let member = self.natives.selected;
        match option {
            0 => {
                self.screen = None;
                self.away = true;
                self.out.push(Out::EditSkater(member));
            }
            1 => self.open_keyboard(Naming::Member(member)),
            2 => self.start_import(member, 0)?,
            // Select Skater: saved at once (the player's look file follows it from the next start), and the card's
            // options drop it.
            3 if member != self.team.active => {
                self.team.active = member;
                self.save();
                self.sync();
                self.out.push(Out::SkateAs(member));
                self.set_state(SCREEN_TEAMMATE_INFO)?;
            }
            _ => {}
        }
        Ok(())
    }
    /// A name from the keyboard.
    fn named(&mut self, purpose: Naming, text: &str) -> Result<(), String> {
        match purpose {
            // 0x826521e0 (an unchanged name does nothing).
            Naming::Team if text == self.team.name => {}
            Naming::Team => {
                self.team.name = text.chars().take(crate::team_data::NAME_MAX).collect();
                self.dirty = true;
                self.sync();
                self.set_state(SCREEN_TEAM_LOGO)?;
            }
            // 0x826522a0: the name, a new photo, dirty -- unless it is unchanged (its strcmp: nothing at all).
            Naming::Member(k) if text == self.team.member_name(k) => {}
            Naming::Member(k) => {
                self.team.set_member_name(k, text);
                self.dirty = true;
                self.queue_photo(k);
                self.sync();
                self.set_state(SCREEN_TEAMMATE_INFO)?;
            }
        }
        Ok(())
    }
    fn popup_choice(&mut self, popup: Popup, choice: usize) -> Result<(), String> {
        match popup {
            Popup::Confirm(row) if choice == 1 => self.import_row(row),
            Popup::Confirm(_) | Popup::Error => Ok(()),
            Popup::EmptyTeam => {
                self.open_keyboard(Naming::Team);
                Ok(())
            }
            Popup::Invalid => Ok(()),
            Popup::Agreement => {
                if choice == 1 {
                    self.out.push(Out::OpenAgreement);
                }
                Ok(())
            }
        }
    }
    fn close(&mut self) {
        if self.dirty {
            self.save();
            self.dirty = false;
        }
        (self.screen, self.import, self.popup, self.keyboard) = (None, None, None, None);
        self.out.push(Out::Closed);
    }

    /// One authored frame (16 ms): the movies, the exit, and the friends list.
    pub(crate) fn advance(&mut self) -> Result<(), String> {
        if let Some(screen) = self.screen.as_mut() {
            screen.advance()?;
        }
        if let Some(import) = self.import.as_mut() {
            import.screen.advance()?;
        }
        if let Some((_, popup)) = self.popup.as_mut() {
            popup.advance()?;
        }
        self.drain_sounds();
        // Update 0x82651b40: with B pressed, save if dirty and pop. Retail first waits for its photo queue under popup
        // 303; here photos still drawing finish into their cache after the pop (Q3: 303 never shows).
        if self.exit && self.import.is_none() && self.screen.is_some() && self.popup.is_none() {
            self.close();
        }
        self.refresh_friends()?;
        Ok(())
    }

    // -- Import Skater (slot 68) --

    fn rows(friends: &[FriendRow], presets: &[Preset]) -> (Vec<Source>, Vec<Row>) {
        let mut sources = Vec::new();
        let mut rows = Vec::new();
        // 0x825c3e20: each non-empty list in list-id order (Community 2 before Presets 5), its header then its rows.
        if !friends.is_empty() {
            sources.push(Source::Header);
            rows.push(Row { label: "#Friends' Skaters".into(), header: true, enabled: false });
            for f in friends {
                sources.push(Source::Friend(f.id));
                rows.push(Row { label: crate::apt_text::literal(&f.name), header: false, enabled: f.state != FriendState::None });
            }
        }
        if !presets.is_empty() {
            sources.push(Source::Header);
            rows.push(Row { label: "ID_PRESET_LISTING_TITLE".into(), header: true, enabled: false });
            for (i, p) in presets.iter().enumerate() {
                sources.push(Source::Preset(i));
                rows.push(Row { label: p.name.clone(), header: false, enabled: true });
            }
        }
        (sources, rows)
    }
    fn start_import(&mut self, member: usize, mode: u8) -> Result<(), String> {
        let friends = sorted_friends(self.friends.lock().map(|f| f.list()).unwrap_or_default());
        let (sources, rows) = Self::rows(&friends, &self.data.team.presets);
        let mut screen = Screen::load(&self.data.import_skater, 0., &self.team.boss, Vec::new())?;
        screen.bindings.team.images = self.natives.images.clone();
        // The picker's first row (menu_picker.Update: NextValueCalc(0), past headers and greyed rows).
        let hilight = rows.iter().position(|r| r.enabled && !r.header).unwrap_or(0);
        screen.bindings.team.rows = rows;
        self.import = Some(Importer { screen, member, mode, sources, friends, hilight });
        let import = self.import.as_mut().unwrap();
        import.screen.set_state(IMPORT_SKATERS)?;
        if let Some((_, row)) = import.screen.bindings.team.hilight.take() {
            import.hilight = row.max(0) as usize;
        }
        self.drain_sounds();
        self.request_preview()?;
        self.show_preview(true)?;
        if !self.agreement_shown && self.friends.lock().is_ok_and(|f| f.needs_agreement()) {
            self.agreement_shown = true;
            self.open_popup(Popup::Agreement)?;
        }
        Ok(())
    }
    /// The hilighted row's preview: rendered once per look (a friend's once it has downloaded).
    fn request_preview(&mut self) -> Result<(), String> {
        let Some(import) = self.import.as_ref() else { return Ok(()) };
        let source = import.sources.get(import.hilight).copied();
        let member = import.member;
        let Some(key) = preview_key(source, member) else { return Ok(()) };
        if self.previews.contains_key(&key) {
            return Ok(());
        }
        let look = match source {
            Some(Source::Preset(i)) => Some(self.data.team.presets[i].profile.clone()),
            Some(Source::Friend(id)) => match self.friends.lock().ok().and_then(|mut f| f.fetch(id)).map(|b| sanitize_friend(&b)) {
                Some(Ok(look)) => Some(look),
                Some(Err(error)) => {
                    bevy::log::warn!("Friend skater {id}: {error}");
                    self.previews.insert(key.clone(), None);
                    None
                }
                None => None,
            },
            _ => None,
        };
        if let Some(look) = look {
            self.previews.insert(key.clone(), None);
            self.rendering.insert(key.clone());
            self.out.push(Out::Preview { key, member, look });
        }
        Ok(())
    }
    /// SetState 11 with the natives saying whether the hilighted row's photo is there (`refresh`: only then).
    fn show_preview(&mut self, refresh: bool) -> Result<(), String> {
        let Some(import) = self.import.as_mut() else { return Ok(()) };
        let row = import.hilight;
        let key = preview_key(import.sources.get(row).copied(), import.member);
        let ready = key.as_ref().and_then(|k| self.previews.get(k)).is_some_and(Option::is_some);
        let natives = &mut import.screen.bindings.team;
        natives.previews.clear();
        if let (true, Some(key)) = (ready, key) {
            natives.previews.insert(row, key.to_lowercase());
        }
        if refresh {
            import.screen.set_state(IMPORT_PREVIEW)?;
        }
        self.drain_sounds();
        Ok(())
    }
    fn import_input(&mut self, key: Key) -> Result<(), String> {
        // +792: while the hilighted row's preview renders, A (0x826536e0) and B (0x82654318) wait for it. A friend
        // still downloading has nothing to confirm yet either; B stays live then (a stalled download must not trap
        // the player: it has no photo request under way).
        let import = self.import.as_ref().unwrap();
        let source = import.sources.get(import.hilight).copied();
        let key_of = preview_key(source, import.member);
        let rendering = key_of.as_ref().is_some_and(|k| self.rendering.contains(k));
        let downloading = match source {
            Some(Source::Friend(id)) => {
                import.friends.iter().any(|f| f.id == id && f.state == FriendState::Loading)
                    || key_of.as_ref().is_some_and(|k| !self.previews.contains_key(k))
            }
            _ => false,
        };
        if (rendering && matches!(key, Key::A | Key::B)) || (downloading && key == Key::A) {
            return Ok(());
        }
        if key == Key::B {
            self.sound("core_b_button");
            return self.leave_import();
        }
        let Some(apt) = key.apt() else { return Ok(()) };
        let import = self.import.as_mut().unwrap();
        import.screen.bindings.team.hilight = None;
        let picked = import.screen.input(apt)?;
        let hilight = import.screen.bindings.team.hilight.take();
        self.drain_sounds();
        if let Some((_, index)) = hilight {
            let import = self.import.as_mut().unwrap();
            import.hilight = (index.max(0) as usize).min(import.sources.len().saturating_sub(1));
            self.request_preview()?;
            self.show_preview(true)?;
        }
        if let Some(row) = picked {
            let import = self.import.as_ref().unwrap();
            if matches!(import.sources.get(row), Some(Source::Friend(_) | Source::Preset(_))) {
                // 0x82654ea8's confirm: 291 from Edit Skaters, 293 for a new teammate or first member.
                self.open_popup(Popup::Confirm(row))?;
            }
        }
        Ok(())
    }
    /// Popup Yes: the row's look goes to the member; the host resolves and writes it, then Import Skater pops
    /// ([`Self::import_applied`]) or shows popup 292 over itself ([`Self::import_failed`]).
    fn import_row(&mut self, row: usize) -> Result<(), String> {
        let Some(import) = self.import.as_ref() else { return Ok(()) };
        let member = import.member;
        let look = match import.sources.get(row) {
            Some(Source::Preset(i)) => Ok(self.data.team.presets[*i].profile.clone()),
            Some(Source::Friend(id)) => {
                let id = *id;
                let bytes = self.friends.lock().ok().and_then(|mut f| f.fetch(id));
                bytes.ok_or_else(|| "not downloaded".to_owned()).and_then(|b| sanitize_friend(&b))
            }
            _ => return Ok(()),
        };
        match look {
            Ok(look) => {
                self.out.push(Out::Apply { member, look });
                Ok(())
            }
            Err(error) => {
                bevy::log::warn!("Import Skater: {error}");
                self.open_popup(Popup::Error)
            }
        }
    }
    /// The host wrote the imported look: Import Skater pops (slot 63 enters again and redraws the member's photo).
    pub(crate) fn import_applied(&mut self) -> Result<(), String> {
        if self.import.is_none() {
            return Ok(());
        }
        self.leave_import()
    }
    /// The host could not use an imported look (Parts::resolve refused it): popup 292 over Import Skater.
    pub(crate) fn import_failed(&mut self) -> Result<(), String> {
        self.open_popup(Popup::Error)
    }
    fn leave_import(&mut self) -> Result<(), String> {
        self.import = None;
        if self.standalone {
            self.close();
            return Ok(());
        }
        self.reenter()
    }
    /// The friends list changes (a download finishes, a friend publishes): the rows follow, the hilight stays. A state
    /// change only re-greys the rows (menu_picker.Update); a new or removed friend rebuilds the list (UpdateSkaters,
    /// whose SetNumOptions resets the picker) and then puts the picker back on the hilighted row (SetValue + Update).
    fn refresh_friends(&mut self) -> Result<(), String> {
        const PICKER: &str = "mImportSkater.mContent.mMenuPicker";
        const SCROLLBAR: &str = "mImportSkater.mContent.mScrollbar";
        let Some(import) = self.import.as_ref() else { return Ok(()) };
        let friends = sorted_friends(self.friends.lock().map(|f| f.list()).unwrap_or_default());
        let hilighted = import.sources.get(import.hilight).copied();
        let waiting = matches!(hilighted, Some(Source::Friend(_)))
            && preview_key(hilighted, import.member).is_some_and(|k| !self.previews.contains_key(&k));
        if friends == import.friends {
            if waiting {
                self.request_preview()?;
            }
            return Ok(());
        }
        let same_rows =
            friends.len() == import.friends.len() && friends.iter().zip(&import.friends).all(|(a, b)| (a.id, &a.name) == (b.id, &b.name));
        let (sources, rows) = Self::rows(&friends, &self.data.team.presets);
        let import = self.import.as_mut().unwrap();
        import.hilight = hilighted.and_then(|h| sources.iter().position(|s| *s == h)).unwrap_or(import.hilight.min(sources.len().saturating_sub(1)));
        (import.sources, import.friends) = (sources, friends);
        import.screen.bindings.team.rows = rows;
        if !same_rows {
            import.screen.set_state(IMPORT_SKATERS)?;
            import.screen.call_clip(PICKER, "SetValue", vec![Value::Number(import.hilight as f64)])?;
        }
        import.screen.bindings.team.hilight = None;
        import.screen.call_clip(PICKER, "Update", Vec::new())?;
        // Update lands on the row (or the next live one, if it was greyed) and reports it.
        if let Some((_, row)) = import.screen.bindings.team.hilight.take() {
            import.hilight = row.max(0) as usize;
        }
        import.screen.call_clip(SCROLLBAR, "SetValue", vec![Value::Number(import.hilight as f64)])?;
        self.request_preview()?;
        self.show_preview(true)
    }

    /// The frame bottom to top: the screen, the keyboard, the popup.
    pub(crate) fn draws(&self, font: Option<&crate::apt_text::Font>) -> Vec<Draw> {
        let mut out = Vec::new();
        if self.away {
            return out;
        }
        let screen = self.import.as_ref().map(|i| &i.screen).or(self.screen.as_ref());
        if let Some(Ok(draws)) = screen.map(Screen::draws) {
            out.extend(draws);
        }
        if let (Some(keyboard), Some(font)) = (self.keyboard.as_ref(), font) {
            out.extend(crate::boot::keyboard_panel(font, &keyboard.entry, &keyboard.header, &self.data.team_management["buttons"]));
        }
        if let Some((_, popup)) = &self.popup {
            if let Ok(draws) = crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes) {
                out.extend(draws);
            }
        }
        out
    }
}
/// A preview's photo name: per look and per member (each member's setup has its own background).
fn preview_key(source: Option<Source>, member: usize) -> Option<String> {
    match source? {
        Source::Preset(i) => Some(format!("import_preset_{i}_{member}")),
        Source::Friend(id) => Some(format!("import_friend_{id}_{member}")),
        Source::Header => None,
    }
}


// ---------------------------------------------------------------------------------------------------------------
// The crossbar host

/// core_menu's Edit Skaters row (crossbar item 25 -> FE slot 63).
#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct TeamUi {
    /// core_menu's pick: open Edit Skaters.
    pub request: bool,
    /// Open: it owns input and the crossbar hides.
    pub modal: bool,
    /// It closed this frame; core_menu ignores that press.
    pub closed_this_frame: bool,
    /// A teammate is in the CAS (on the player's skater).
    editing: bool,
    /// The player's look is back: its selections, and since when (the skater wears it a frame or two later).
    restored: Option<(Json, std::time::Instant)>,
}
impl TeamUi {
    /// Whether the online look must not be sent now (decision L2: only the player's skater is published, the boss or
    /// the member Select Skater picked): while another member is in the CAS, then until the skater wears the player's
    /// look again (or a few seconds pass), as the Graphic Creator's.
    pub(crate) fn holds_publish(&mut self, applied: &Json) -> bool {
        if self.editing {
            return true;
        }
        let Some((boss, since)) = &self.restored else { return false };
        if applied["selections"] == *boss || since.elapsed() > std::time::Duration::from_secs(3) {
            self.restored = None;
            return false;
        }
        true
    }
}
/// While the CAS edits another member: the player's profile path and look, and custom character, to put back.
pub(crate) struct Swap {
    path: PathBuf,
    look: Json,
    model: Option<String>,
}
impl Swap {
    /// The teammate is edited on the stock skater: the player's custom character comes off for now, never as the saved
    /// selection (CustomModels::wear, not request_stock). Returns the one to put back.
    fn stock(models: &mut crate::custom_models::CustomModels) -> Option<String> {
        let worn = models.active.clone();
        models.wear(None);
        worn
    }
    /// The CAS closed: the player's profile and character come back.
    pub(crate) fn restore(self, customiser: &mut crate::customiser::Customiser, models: &mut crate::custom_models::CustomModels) {
        customiser.swap_profile(self.path, self.look);
        models.wear(self.model);
    }
}
/// Import Skater's friends: the Steam relay replaces the default (no friends).
#[derive(bevy::prelude::Resource)]
pub(crate) struct FriendSource(pub Friends);
impl Default for FriendSource {
    fn default() -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(NoFriends)))
    }
}

/// Edit Skaters' canvas layer (between the retail screens' 47 and the boot's 50).
const LAYER: usize = 48;
const FRAME: f32 = 1. / 60.;

struct Host {
    manager: Manager,
    canvas: crate::apt_canvas::Canvas,
    /// The prepared boot data's folder (the canvas's root).
    root: PathBuf,
    font: Option<crate::apt_text::Font>,
    accumulated: f32,
    /// While the CAS edits a teammate: what to put back.
    swapped: Option<Swap>,
}
#[derive(bevy::prelude::Resource, Default)]
struct TeamScreens(Option<Host>);

pub(crate) struct TeamSelectPlugin;
impl bevy::prelude::Plugin for TeamSelectPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        use bevy::prelude::*;
        app.init_resource::<TeamUi>()
            .init_resource::<FriendSource>()
            .init_resource::<TeamScreens>()
            .add_plugins(crate::team_photo::PhotoPlugin)
            .add_systems(PreUpdate, interact.after(crate::customiser::navigation).before(crate::core_menu::interact))
            .add_systems(Update, (render.after(crate::graphics_menu::interact), prewarm.after(render)));
    }
}

/// The settings folder (beside the assets) and the prepared boot data (SKATE_BOOT_ROOT for tests).
fn places(config: &crate::config::Config) -> (PathBuf, PathBuf) {
    let settings = crate::saves::settings_dir(&config.asset_root);  // Not retail (user 10-13)
    let root = std::env::var_os("SKATE_BOOT_ROOT").map(PathBuf::from).unwrap_or_else(|| config.asset_root.join("private/boot"));
    (settings, root)
}

/// The team as saved (migrated from an older career on first use), with the game data's defaults.
pub(crate) fn load_team(data: &Data, settings: &std::path::Path) -> Team {
    let (team_name, logo, teammates) = data.defaults();
    let boss = data.text("ID_DEFAULT_PLAYER_NAME");
    let defaults = crate::team_data::Defaults { team_name: &team_name, boss: &boss, logo: &logo, teammates };
    Team::load(settings, &defaults)
}

/// The setup's background art from the pipeline's thumbnails (raw RGBA under the boot root).
fn background(team: &TeamFile, setup: &PhotoSetup, root: &std::path::Path) -> Option<(PathBuf, [u32; 2])> {
    let texture = team.thumbnails.get(setup.background.as_deref()?)?;
    Some((root.join(&texture.rgba), [texture.width, texture.height]))
}

/// The cards' photo cache (retail keeps each member's photo with the team, profile +284): beside the team's files.
pub(crate) fn photo_cache(settings: &std::path::Path) -> PathBuf {
    settings.join("team-photos")
}

/// The cards' cache file of member k's look as saved, in `team_profile_0k` (no file is touched: the prewarm thread
/// names them too). `library` is the clothing library's fingerprint (Parts::fingerprint).
fn card_file(team: &TeamFile, root: &std::path::Path, settings: &std::path::Path, member: usize, look: &Json, library: &str) -> Option<PathBuf> {
    let setup = team.photos.get(&format!("team_profile_{member:02}"))?;
    let background = background(team, setup, root);
    Some(crate::team_photo::cache_file(&photo_cache(settings), look, setup, background.as_ref(), &format!("{library}{}", team.fingerprint)))
}

/// Member k's card photo of a look (as saved) in `team_profile_0k`, drawn into (or found in) the cards' cache.
fn card_job(
    team: &TeamFile,
    root: &std::path::Path,
    settings: &std::path::Path,
    member: usize,
    look: Json,
    parts: &crate::customiser_parts::Parts,
    key: String,
) -> Option<crate::team_photo::PhotoJob> {
    let file = card_file(team, root, settings, member, &look, &parts.fingerprint)?;
    let setup = team.photos.get(&format!("team_profile_{member:02}"))?.clone();
    let background = background(team, &setup, root);
    let profile = parts.resolve(&look).ok()?;
    Some(crate::team_photo::PhotoJob { key, profile, setup, background, file })
}

/// Member k's look as its card shows it, as saved (the player's, the boss or the member Select Skater picked, is the
/// live look: the customiser holds its file).
fn member_look(member: usize, settings: &std::path::Path, customiser: &crate::customiser::Customiser) -> Option<Json> {
    if plays(customiser, settings, member) { Some(customiser.draft.clone()) } else { crate::team_data::read_look(settings, member) }
}
/// Member k is the player's skater: its look file is the customiser's profile.
fn plays(customiser: &crate::customiser::Customiser, settings: &std::path::Path, member: usize) -> bool {
    customiser.profile_path() == crate::team_data::look_path(settings, member)
}
/// Member k's look as saved, resolved; the default look when it has none (or it is refused, and kept aside: B3), as
/// the CAS shows it. Leaving the CAS with that default unchanged writes nothing (Customiser::leave).
pub(crate) fn member_profile(settings: &std::path::Path, member: usize, parts: &crate::customiser_parts::Parts) -> Json {
    let path = crate::team_data::look_path(settings, member);
    let look = crate::team_data::read_look(settings, member).and_then(|l| {
        parts.resolve(&l).map_err(|e| crate::customiser::refused(&path, &e, parts)).ok()
    });
    look.or_else(|| parts.library.defaults.get("male").cloned()).unwrap_or_default()
}

/// What the prewarm thread found: the team data and the members (with their look) whose card photo is not cached.
struct Found {
    team: TeamFile,
    missing: Vec<(usize, Json)>,
}
#[derive(Default)]
struct Prewarm {
    /// Edit Skaters, the CAS and the boot's teammate editing were all closed last frame (looks only change in them).
    quiet: bool,
    /// A look may have changed since the last look through the cache.
    dirty: bool,
    task: Option<std::sync::mpsc::Receiver<Found>>,
}

/// Card photos drawn ahead of time. Each time the looks may have changed (at start, and whenever Edit Skaters or the
/// CAS closes) a thread names every member's cache file and finds the ones not drawn; those are drawn (one at a time,
/// as the screen would), so the screen opens with all of them. When every member's is in, pictures of looks no member
/// wears any more are deleted (retail keeps one photo a member, too), with failed writes' leftovers and old previews.
/// No file is touched on the main thread but the drawing's own.
fn prewarm(
    mut state: bevy::prelude::Local<Prewarm>,
    screens: bevy::prelude::Res<TeamScreens>,
    ui: bevy::prelude::Res<TeamUi>,
    (customiser, parts, config): (
        bevy::prelude::Res<crate::customiser::Customiser>,
        bevy::prelude::Res<crate::customiser_parts::Parts>,
        bevy::prelude::Res<crate::config::Config>,
    ),
    mut photos: bevy::prelude::ResMut<crate::team_photo::Photos>,
) {
    const KEY: &str = "prewarm:";
    // Ours are not the screen's: they are only files.
    // perf 10-18: only with one of ours there (the Photos untouched otherwise), and the places only when used.
    // SKATE_PREWARM_LAZY=0: every frame, as before.
    static LAZY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*LAZY.get_or_init(|| std::env::var_os("SKATE_PREWARM_LAZY").is_none_or(|v| v != "0")) || photos.finished.iter().any(|d| d.key.starts_with(KEY)) {
        photos.finished.retain(|d| !d.key.starts_with(KEY));
    }
    match state.task.as_ref().map(|t| t.try_recv()) {
        Some(Ok(found)) => {
            state.task = None;
            let (settings, root) = places(&config);
            for (k, look) in found.missing {
                if let Some(job) = card_job(&found.team, &root, &settings, k, look, &parts, format!("{KEY}{k}")) {
                    photos.request(job);
                }
            }
        }
        Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => state.task = None,
        _ => {}
    }
    let quiet = !customiser.open && !ui.editing && !parts.library.models.is_empty() && !screens.0.as_ref().is_some_and(|h| h.manager.is_open());
    state.dirty |= quiet && !state.quiet;
    state.quiet = quiet;
    if !(quiet && state.dirty && state.task.is_none()) {
        return;
    }
    state.dirty = false;
    let (settings, root) = places(&config);
    let (draft, library, player) = (customiser.draft.clone(), parts.fingerprint.clone(), customiser.profile_path().to_path_buf());
    let (send, receive) = std::sync::mpsc::channel();
    state.task = Some(receive);
    std::thread::spawn(move || {
        let cache = photo_cache(&settings);
        crate::team_photo::sweep(&cache, None);
        crate::team_photo::sweep(&crate::team_photo::preview_folder(), Some(std::time::Duration::from_secs(24 * 3600)));
        let Ok(team) = TeamFile::read(&root) else { return };
        let mut keep = BTreeSet::new();
        let mut missing = Vec::new();
        for k in 0..MEMBERS {
            let mine = crate::team_data::look_path(&settings, k) == player;
            let Some(look) = (if mine { Some(draft.clone()) } else { crate::team_data::read_look(&settings, k) }) else { continue };
            let Some(file) = card_file(&team, &root, &settings, k, &look, &library) else { continue };
            let setup = &team.photos[&format!("team_profile_{k:02}")];
            if !crate::team_photo::cached(&file, setup) {
                missing.push((k, look));
            }
            keep.insert(file);
        }
        if missing.is_empty() {
            for entry in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
                let path = entry.path();
                if !keep.contains(&path) && path.extension().is_some_and(|e| e == "rgba") {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let _ = send.send(Found { team, missing });
    });
}

#[allow(clippy::too_many_arguments)]
fn interact(
    mut commands: bevy::prelude::Commands,
    mut ui: bevy::prelude::ResMut<TeamUi>,
    mut screens: bevy::prelude::ResMut<TeamScreens>,
    friends: bevy::prelude::Res<FriendSource>,
    (nav, config): (bevy::prelude::Res<crate::customiser::Navigation>, bevy::prelude::Res<crate::config::Config>),
    mut typing: bevy::prelude::MessageReader<bevy::input::keyboard::KeyboardInput>,
    window: bevy::prelude::Single<&bevy::prelude::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>,
    (mut images, mut composites): (
        bevy::prelude::ResMut<bevy::prelude::Assets<bevy::prelude::Image>>,
        bevy::prelude::ResMut<bevy::prelude::Assets<crate::scoring_hud::HudComposite>>,
    ),
    outputs: bevy::prelude::Query<bevy::prelude::Entity, bevy::prelude::With<bevy::prelude::IsDefaultUiCamera>>,
) {
    use bevy::prelude::*;
    ui.closed_this_frame = false;
    if std::mem::take(&mut ui.request) {
        let (settings, root) = places(&config);
        if screens.0.is_none() {
            let data = match Data::load(&root) {
                Ok(data) => data,
                Err(error) => {
                    error!("Edit Skaters unavailable ({error}); prepare it with the launcher's boot stage");
                    ui.closed_this_frame = true;
                    return;
                }
            };
            let Ok(output) = outputs.single() else { return };
            let canvas = crate::apt_canvas::Canvas::new(&mut commands, &mut images, &mut composites, &window, root.clone(), LAYER, -20);
            commands.spawn((
                MaterialNode(canvas.composite.clone()),
                crate::apt_canvas::CompositeOf(canvas.camera()),
                UiTargetCamera(output),
                // Over the crossbar (10), under the CAS (20+).
                GlobalZIndex(11),
                Pickable::IGNORE,
                Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
            ));
            let font = crate::apt_text::TextAssets::load(&data.team_management)
                .ok()
                .and_then(|t| t.font_named("Futura Std Medium").and_then(|id| t.fonts.get(&id).cloned()));
            let team = load_team(&data, &settings);
            let manager = Manager::new(data, team, settings.clone(), friends.0.clone());
            screens.0 = Some(Host { manager, canvas, root, font, accumulated: 0., swapped: None });
        }
        let host = screens.0.as_mut().unwrap();
        host.manager.team = load_team(&host.manager.data, &settings);
        if let Err(error) = host.manager.open() {
            error!("Edit Skaters: {error}");
            ui.closed_this_frame = true;
            return;
        }
    }
    let Some(host) = screens.0.as_mut() else { return };
    if !host.manager.is_open() {
        ui.modal = false;
        return;
    }
    ui.modal = true;
    if host.manager.away {
        typing.clear();
        return;
    }
    let keyboard = host.manager.keyboard.is_some();
    const BUTTONS: [(u16, Key); 9] = [
        (0x1, Key::Up),
        (0x2, Key::Down),
        (0x4, Key::Left),
        (0x8, Key::Right),
        (0x10, Key::Start),
        (0x1000, Key::A),
        (0x2000, Key::B),
        (0x4000, Key::X),
        (0x8000, Key::Y),
    ];
    let mut pressed: Vec<Key> = BUTTONS.into_iter().filter(|(bit, _)| nav.pressed & bit != 0).map(|(_, key)| key).collect();
    let mut typed = String::new();
    for event in typing.read() {
        if !event.state.is_pressed() {
            continue;
        }
        // Typing: letters type, Enter is Done, Escape cancels, Backspace deletes. Otherwise Enter is A, Escape and
        // Backspace are B, and X / Y are the X and Y buttons.
        let key = match (keyboard, event.key_code) {
            (_, KeyCode::Enter | KeyCode::NumpadEnter) if !event.repeat => Some(if keyboard { Key::Start } else { Key::A }),
            (_, KeyCode::Escape) if !event.repeat => Some(Key::B),
            (true, KeyCode::Backspace) => Some(Key::X),
            (false, KeyCode::Backspace) if !event.repeat => Some(Key::B),
            (false, KeyCode::KeyX) if !event.repeat => Some(Key::X),
            (false, KeyCode::KeyY) if !event.repeat => Some(Key::Y),
            (true, _) => {
                if let Some(text) = &event.text {
                    typed.push_str(text);
                }
                None
            }
            _ => None,
        };
        pressed.extend(key);
    }
    host.manager.type_text(&typed);
    for key in pressed {
        if let Err(error) = host.manager.input(key) {
            error!("Edit Skaters stopped: {error}");
            screens.0 = None;
            (ui.modal, ui.closed_this_frame) = (false, true);
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render(
    mut commands: bevy::prelude::Commands,
    mut ui: bevy::prelude::ResMut<TeamUi>,
    mut screens: bevy::prelude::ResMut<TeamScreens>,
    real: bevy::prelude::Res<bevy::prelude::Time<bevy::prelude::Real>>,
    (mut customiser, mut custom_models, parts): (
        bevy::prelude::ResMut<crate::customiser::Customiser>,
        bevy::prelude::ResMut<crate::custom_models::CustomModels>,
        bevy::prelude::Res<crate::customiser_parts::Parts>,
    ),
    mut photos: bevy::prelude::ResMut<crate::team_photo::Photos>,
    mut sounds: bevy::prelude::MessageWriter<crate::skate_audio::FrontEndSoundRequest>,
    window: bevy::prelude::Single<&bevy::prelude::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>,
    (mut images, mut composites, mut meshes, mut materials): (
        bevy::prelude::ResMut<bevy::prelude::Assets<bevy::prelude::Image>>,
        bevy::prelude::ResMut<bevy::prelude::Assets<crate::scoring_hud::HudComposite>>,
        bevy::prelude::ResMut<bevy::prelude::Assets<bevy::prelude::Mesh>>,
        bevy::prelude::ResMut<bevy::prelude::Assets<crate::scoring_hud::HudMaterial>>,
    ),
    mut cameras: bevy::prelude::Query<&mut bevy::prelude::Camera>,
    mut presence: Option<bevy::prelude::ResMut<crate::presence::Presence>>,
) {
    use bevy::prelude::*;
    let Some(host) = screens.0.as_mut() else { return };
    host.canvas.resize(&window, &mut images, &mut composites);
    collect_photos(&mut host.manager, &mut photos);
    if host.manager.is_open() && !host.manager.away {
        host.accumulated = (host.accumulated + real.delta_secs()).min(4. * FRAME);
        while host.accumulated >= FRAME {
            host.accumulated -= FRAME;
            if let Err(error) = host.manager.advance() {
                error!("Edit Skaters stopped: {error}");
                screens.0 = None;
                (ui.modal, ui.closed_this_frame) = (false, true);
                return;
            }
        }
    }
    let mut services = Services {
        customiser: &mut customiser,
        custom_models: &mut custom_models,
        parts: &parts,
        photos: &mut photos,
        root: &host.root,
        presence: presence.as_deref_mut(),
    };
    for out in host.manager.take() {
        match serve(&mut host.manager, out, &mut services, &mut host.swapped) {
            Served::Sound(id) => {
                sounds.write(crate::skate_audio::FrontEndSoundRequest(id));
            }
            Served::Closed => (ui.modal, ui.closed_this_frame) = (false, true),
            Served::Done => {}
        }
    }
    // The CAS closed: the boss's profile comes back and slot 63 enters again. (After the outs: the frame that opens
    // the CAS has served its EditSkater by now, so `open` is already set.)
    if host.manager.away && !customiser.open {
        if let Some(swap) = host.swapped.take() {
            swap.restore(&mut customiser, &mut custom_models);
            ui.restored = Some((customiser.draft["selections"].clone(), std::time::Instant::now()));
        }
        if let Err(error) = host.manager.reenter() {
            error!("Edit Skaters: {error}");
        }
    }
    ui.editing = host.swapped.is_some();
    let shown = host.manager.is_open() && !host.manager.away;
    host.canvas.set_active(shown, &mut cameras);
    let draws = if shown { host.manager.draws(host.font.as_ref()) } else { Vec::new() };
    host.canvas.draw(&draws, &mut commands, &mut images, &mut meshes, &mut materials);
}


/// What a host needs to carry out the manager's [`Out`]s (the crossbar plugin and the boot share it).
pub(crate) struct Services<'a> {
    pub customiser: &'a mut crate::customiser::Customiser,
    pub custom_models: &'a mut crate::custom_models::CustomModels,
    pub parts: &'a crate::customiser_parts::Parts,
    pub photos: &'a mut crate::team_photo::Photos,
    /// The prepared boot data's folder.
    pub root: &'a std::path::Path,
    /// The Steam presence helper (None: no Steam).
    pub presence: Option<&'a mut crate::presence::Presence>,
}
/// What is left for the host after [`serve`].
pub(crate) enum Served {
    Sound(u64),
    Closed,
    Done,
}
/// Finished photos back to the manager (only its own: [`Manager::job_key`]).
pub(crate) fn collect_photos(manager: &mut Manager, photos: &mut crate::team_photo::Photos) {
    photos.finished.retain(|done| {
        let Some(member) = manager.photo_keys.remove(&done.key) else { return true };
        let texture = done.file.clone().map(|(path, [width, height])| Texture { width, height, rgba: path.to_string_lossy().into_owned(), frame: None });
        let result = match member {
            Some(k) => {
                manager.photo_ready(k, texture);
                Ok(())
            }
            None => manager.preview_ready(done.key.split_once(':').map_or(&*done.key, |(_, name)| name), texture),
        };
        if let Err(error) = result {
            bevy::log::error!("Edit Skaters: {error}");
        }
        false
    });
}
/// Carry out one [`Out`]: the CAS (the teammate's profile swapped in, `swapped` keeps the boss's), photos, imports.
pub(crate) fn serve(manager: &mut Manager, out: Out, s: &mut Services, swapped: &mut Option<Swap>) -> Served {
    let settings = manager.settings.clone();
    match out {
        Out::Sound(id) => return Served::Sound(id),
        Out::Closed => return Served::Closed,
        Out::OpenAgreement => {
            let page = skate_net::presence::Order::Open(skate_net::presence::Page::WorkshopAgreement);
            if !s.presence.as_mut().is_some_and(|p| p.order(page)) {
                bevy::log::warn!("Steam is not running: accept the Workshop agreement at steamcommunity.com/sharedfiles/workshoplegalagreement");
            }
        }
        Out::EditSkater(member) => {
            if !plays(s.customiser, &settings, member) {
                let model = Swap::stock(s.custom_models);
                let look = member_profile(&settings, member, s.parts);
                let (path, look) = s.customiser.swap_profile(crate::team_data::look_path(&settings, member), look);
                *swapped = Some(Swap { path, look, model });
            } else {
                // The player's own skater: editing it makes it the stock one (the crossbar's own row).
                s.custom_models.request_stock();
            }
            s.customiser.begin();
        }
        // The member's look goes on the player's skater (customiser_parts redresses it in place) and its file becomes
        // the customiser's profile, so the CAS, online and Party Play follow. A worn custom character stays on (it is
        // the player's choice over any look, as over the boss's).
        Out::SkateAs(member) => {
            let look = member_profile(&settings, member, s.parts);
            s.customiser.swap_profile(crate::team_data::look_path(&settings, member), look);
            s.customiser.enabled = true;
        }
        Out::Photo(member) => {
            let look = member_look(member, &settings, s.customiser);
            let key = manager.job_key(&photo_name(member));
            match look.and_then(|look| card_job(&manager.data.team, s.root, &settings, member, look, s.parts, key)) {
                Some(job) => {
                    manager.photo_keys.insert(job.key.clone(), Some(member));
                    s.photos.request(job);
                    // A cached photo is finished at once: the card shows it this frame.
                    collect_photos(manager, s.photos);
                }
                None => manager.photo_ready(member, None),
            }
        }
        Out::Preview { key, member, look } => {
            let setup = manager.data.team.photos.get(&format!("team_profile_{member:02}")).cloned();
            match (s.parts.resolve(&look), setup) {
                (Ok(profile), Some(setup)) => {
                    let background = background(&manager.data.team, &setup, s.root);
                    let inputs = format!("{}{}", s.parts.fingerprint, manager.data.team.fingerprint);
                    let file = crate::team_photo::cache_file(&crate::team_photo::preview_folder(), &look, &setup, background.as_ref(), &inputs);
                    let job = manager.job_key(&key);
                    manager.photo_keys.insert(job.clone(), None);
                    s.photos.request(crate::team_photo::PhotoJob { key: job, profile, setup, background, file });
                    collect_photos(manager, s.photos);
                }
                _ => {
                    let _ = manager.preview_ready(&key, None);
                }
            }
        }
        Out::Apply { member, look } => match s.parts.resolve(&look) {
            Ok(profile) => {
                // The player's skater: the live look changes too (and is what online sends, L2).
                if plays(s.customiser, &settings, member) {
                    s.customiser.draft = profile.clone();
                    s.customiser.redraw = true;
                }
                if let Err(error) = crate::team_data::write_look(&settings, member, &profile) {
                    bevy::log::warn!("Import Skater: {error}");
                }
                if let Err(error) = manager.import_applied() {
                    bevy::log::error!("Edit Skaters: {error}");
                }
            }
            Err(error) => {
                bevy::log::warn!("Import Skater: the look was refused ({error})");
                let _ = manager.import_failed();
            }
        },
    }
    Served::Done
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::team_data::tests::dir;

    pub(crate) fn friends() -> FakeFriends {
        let look = br#"{"gender":"female","selections":{},"model":"C:/evil.glb","graphics":{"front":{"design":{"layers":[]}}}}"#.to_vec();
        let row = |name: &str, id, state| FriendRow { name: name.into(), id, state };
        FakeFriends {
            rows: vec![
                row("zeke", 1, FriendState::Ready),
                row("Влад", 2, FriendState::Ready),
                row("スケーター", 3, FriendState::Loading),
                row("Zoë", 4, FriendState::None),
                row("🛹 Shredder", 5, FriendState::Ready),
                row("Émile", 6, FriendState::Ready),
                row("anna", 7, FriendState::Ready),
            ],
            looks: [(1, look.clone()), (2, look.clone()), (5, look.clone()), (6, b"[1,2]".to_vec()), (7, look)].into(),
            needs_agreement: false,
        }
    }

    #[test]
    fn friends_sort_case_and_accent_insensitive() {
        let names: Vec<String> = sorted_friends(friends().list()).into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["anna", "Émile", "zeke", "Zoë", "Влад", "スケーター", "🛹 Shredder"]);
        let dirty = sorted_friends(vec![FriendRow { name: "a\u{7}b".repeat(20), id: 9, state: FriendState::Ready }]);
        assert_eq!(dirty[0].name.chars().count(), FRIEND_NAME_MAX);
        assert!(!dirty[0].name.contains('\u{7}'));
    }

    /// Letters with no decomposition sort with their base letter, and a decomposed accent is composed (NFC).
    #[test]
    fn friends_sort_with_their_base_letters() {
        let row = |name: &str, id| FriendRow { name: name.into(), id, state: FriendState::Ready };
        let rows = vec![row("zeke", 1), row("ßtar", 2), row("Øystein", 3), row("Łukasz", 4), row("anna", 5), row("Mi\u{20D7}ke", 6), row("Zoe\u{308}", 7)];
        let names: Vec<String> = sorted_friends(rows).into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["anna", "Łukasz", "Mi\u{20D7}ke", "Øystein", "ßtar", "zeke", "Zoë"]);
        assert_eq!(sort_key("Æsir Œuvre"), "aesir oeuvre");
    }

    #[test]
    fn friend_looks_pass_the_network_sanitizer() {
        let look = sanitize_friend(&friends().looks[&1]).unwrap();
        assert_eq!(look["gender"], "female");
        assert!(look.get("model").is_none(), "no model file reference survives");
        assert!(look["graphics"].get("front").is_none(), "creator designs are dropped");
        assert!(sanitize_friend(b"[1,2]").is_err());
        assert!(sanitize_friend(&vec![b' '; 40000]).is_err());
        let huge = format!(r#"{{"gender":"male","pad":"{}"}}"#, "x".repeat(40000));
        assert!(sanitize_friend(huge.as_bytes()).is_err(), "over the outfit size cap");
        // Over the cap is refused before it is parsed: even bytes that are not JSON at all.
        assert_eq!(sanitize_friend(&vec![b'{'; crate::multiplayer::appearance::MAX_OUTFIT + 1]), Err("Over the outfit size cap".into()));
        // Only the known look fields survive, and a worn item keeps only its ids.
        let odd = br#"{"gender":"male","notes":"hi","native":"pro","selections":{"Hat":{"asset_id":"a","material_id":"m","url":"x"},"Feet":{"asset_id":"b"}},"hair_choice":{"asset_id":"h","material_id":"c","x":1}}"#;
        let look = sanitize_friend(odd).unwrap();
        let mut keys: Vec<&String> = look.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["gender", "hair_choice", "selections"]);
        assert_eq!(look["selections"]["Hat"], serde_json::json!({"asset_id": "a", "material_id": "m"}));
        assert!(look["selections"]["Feet"].is_null(), "an item with no material is dropped (Parts::resolve skips nulls)");
        assert_eq!(look["hair_choice"], serde_json::json!({"asset_id": "h", "material_id": "c"}));
    }

    #[test]
    fn import_rows_are_friends_then_presets_with_headers() {
        let presets: Vec<Preset> = (1..=2).map(|i| Preset { name: format!("ID_TEAMMATE_PRESET_0{i}"), profile: Json::Null }).collect();
        let friends = sorted_friends(friends().list());
        let (sources, rows) = Manager::rows(&friends, &presets);
        assert_eq!(rows[0], Row { label: "#Friends' Skaters".into(), header: true, enabled: false });
        assert_eq!(rows[1].label, crate::apt_text::literal("anna"));
        let zoe = rows.iter().find(|r| r.label == crate::apt_text::literal("Zoë")).unwrap();
        assert!(!zoe.enabled, "no published skater: greyed");
        assert_eq!(rows[8], Row { label: "ID_PRESET_LISTING_TITLE".into(), header: true, enabled: false });
        assert_eq!(sources[9], Source::Preset(0));
        // No friends: only the presets, no empty header.
        let (_, rows) = Manager::rows(&[], &presets);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].label, "ID_PRESET_LISTING_TITLE");
    }

    /// A teammate's edit takes the player's custom character off without saving that, and puts it back after.
    #[test]
    fn a_teammate_edit_keeps_the_players_character() {
        let mut models = crate::custom_models::CustomModels::default();
        models.active = Some("pro".into());
        let worn = Swap::stock(&mut models);
        assert_eq!(worn.as_deref(), Some("pro"));
        assert_eq!(models.requested(), Some(None), "the stock skater for the edit");
        models.active = None;
        models.wear(worn);
        assert_eq!(models.requested(), Some(Some("pro".into())), "the player's character back");
        // A player on the stock skater: nothing to take off or put back.
        let mut stock = crate::custom_models::CustomModels::default();
        assert_eq!(Swap::stock(&mut stock), None);
        assert_eq!(stock.requested(), None);
    }

    /// The online look holds while a teammate is in the CAS, then until the boss's look is worn again.
    #[test]
    fn a_teammate_in_the_cas_is_never_published() {
        let boss = serde_json::json!({"selections": {"Hat": {"asset_id": "cap", "material_id": "red"}}});
        let teammate = serde_json::json!({"selections": {}});
        let mut ui = TeamUi::default();
        assert!(!ui.holds_publish(&boss));
        ui.editing = true;
        assert!(ui.holds_publish(&teammate));
        (ui.editing, ui.restored) = (false, Some((boss["selections"].clone(), std::time::Instant::now())));
        assert!(ui.holds_publish(&teammate), "the skater still wears the teammate");
        assert!(!ui.holds_publish(&boss), "the boss is back on");
        assert!(!ui.holds_publish(&teammate), "and the hold is over");
        ui.restored = Some((boss["selections"].clone(), std::time::Instant::now() - std::time::Duration::from_secs(4)));
        assert!(!ui.holds_publish(&teammate), "a look that never comes back stops holding");
    }

    /// The prepared boot data (SKATE_BOOT_ROOT), else the test is skipped.
    fn root() -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("SKATE_BOOT_ROOT")?);
        root.join("runtime/team.json").is_file().then_some(root)
    }
    fn manager(name: &str) -> Option<(Manager, PathBuf, PathBuf)> {
        manager_with(name, std::sync::Arc::new(std::sync::Mutex::new(friends())))
    }
    fn manager_with(name: &str, friends: Friends) -> Option<(Manager, PathBuf, PathBuf)> {
        let root = root()?;
        let data = Data::load(&root).unwrap();
        let settings = dir(name);
        let team = load_team(&data, &settings);
        Some((Manager::new(data, team, settings.clone(), friends), root, settings))
    }
    /// Every preview asked for, drawn (a background art stands in); returns their keys and members.
    fn fake_previews(m: &mut Manager) -> Vec<(String, usize)> {
        let mut drawn = Vec::new();
        for out in m.take() {
            if let Out::Preview { key, member, .. } = out {
                let texture = m.data.team.thumbnails.get(&format!("team_profile_background_{member}")).cloned();
                m.preview_ready(&key, texture).unwrap();
                drawn.push((key, member));
            }
        }
        drawn
    }
    fn frames(m: &mut Manager, n: u32) {
        for _ in 0..n {
            m.advance().unwrap();
        }
    }
    fn snap(m: &Manager, root: &std::path::Path, name: &str) {
        if let Ok(out) = std::env::var("SKATE_BOOT_OUT") {
            let font = crate::apt_text::TextAssets::load(&m.data.team_management)
                .ok()
                .and_then(|t| t.font_named("Futura Std Medium").and_then(|id| t.fonts.get(&id).cloned()));
            let draws = m.draws(font.as_ref());
            crate::core_menu_runtime::tests::raster_draws(&draws, root, &std::path::Path::new(&out).join(format!("{name}.png")));
        }
    }
    fn texts(screen: &Screen) -> Vec<String> {
        let mut out: Vec<String> = (screen.bindings.movie.instances.keys())
            .filter_map(|&id| match screen.vm.get(id, "_displayText") {
                Value::Text(t) if !t.is_empty() => Some(t),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }
    /// Stand-in photos for the renders: each member's card shows a background art (the engine draws the real ones).
    fn fake_photos(m: &mut Manager) {
        for out in m.take() {
            if let Out::Photo(k) = out {
                let background = format!("team_profile_background_{}", k.min(4));
                let texture = m.data.team.thumbnails.get(&background).cloned();
                m.photo_ready(k, texture);
            }
        }
    }

    /// The cards with the engine's drawn photos (SKATE_PHOTO_OUT from team_photo's GPU test: photo-k.rgba), the boss
    /// then teammate 1 selected, rendered as team-photos-cards*.png into SKATE_BOOT_OUT.
    #[test]
    fn the_cards_show_the_drawn_photos() {
        let Some(photos) = std::env::var_os("SKATE_PHOTO_OUT").map(PathBuf::from) else { return };
        let Some((mut m, root, settings)) = manager("drawn") else { return };
        m.open().unwrap();
        for k in 0..5 {
            let rgba = photos.join(format!("photo-{k}.rgba"));
            assert!(rgba.is_file(), "{} (run the GPU test first)", rgba.display());
            m.photo_ready(k, Some(Texture { width: 128, height: 256, rgba: rgba.to_string_lossy().into_owned(), frame: None }));
        }
        frames(&mut m, 40);
        snap(&m, &root, "team-photos-cards");
        m.input(Key::Right).unwrap();
        frames(&mut m, 20);
        snap(&m, &root, "team-photos-cards-teammate");
        let _ = std::fs::remove_dir_all(settings);
    }

    /// A photo another manager left among the finished ones (the boot's, ended mid-draw: the same asset names) is never
    /// taken for this one's card; this one's own is, and the other stays inert.
    #[test]
    fn a_photo_another_manager_left_is_not_taken() {
        let Some((boot, _, boot_settings)) = manager("left-boot") else { return };
        let Some((mut m, _, settings)) = manager("left-screen") else { return };
        let done = |key: String, file: &str| crate::team_photo::PhotoDone { key, file: Some((PathBuf::from(file), [128, 256])) };
        let mut photos = crate::team_photo::Photos::default();
        photos.finished.push(done(boot.job_key(&photo_name(0)), "stale.rgba"));
        m.queue_photo(0);
        m.photo_keys.insert(m.job_key(&photo_name(0)), Some(0));
        photos.finished.push(done(m.job_key(&photo_name(0)), "fresh.rgba"));
        collect_photos(&mut m, &mut photos);
        assert_eq!(m.image(&photo_name(0)).map(|t| t.rgba.as_str()), Some("fresh.rgba"));
        assert_eq!(photos.finished.len(), 1, "the boot's is left alone");
        let _ = std::fs::remove_dir_all(boot_settings);
        let _ = std::fs::remove_dir_all(settings);
    }

    /// Slot 63 from the crossbar: cards, hilight wrap, options, rename, the logo grid, B without popup 303 and the save.
    #[test]
    fn edit_skaters_runs_as_retail() {
        let Some((mut m, root, settings)) = manager("screen") else { return };
        m.open().unwrap();
        // OnEnter queues every member's photo.
        let photos: Vec<Out> = m.take().into_iter().filter(|o| matches!(o, Out::Photo(_))).collect();
        assert_eq!(photos, (0..5).map(Out::Photo).collect::<Vec<_>>());
        // A photo asked for again while it draws (an import landing meanwhile) is drawn again once it is in.
        m.request_photo(1);
        assert!(m.take().is_empty(), "one at a time");
        for k in 0..5 {
            let background = format!("team_profile_background_{k}");
            let texture = m.data.team.thumbnails.get(&background).cloned();
            m.photo_ready(k, texture);
        }
        assert_eq!(m.take(), [Out::Photo(1)], "the member asked for again");
        m.photo_ready(1, m.data.team.thumbnails.get("team_profile_background_1").cloned());
        frames(&mut m, 40);
        snap(&m, &root, "team-0-cards");
        let shown = texts(m.screen.as_ref().unwrap());
        for want in ["The Legend", "Edit Skater", "Change Name", "Import Skater", "New Team"] {
            assert!(shown.iter().any(|t| t.contains(want)), "{want} missing from {shown:?}");
        }
        // D-pad Left from the boss wraps to the last card.
        m.input(Key::Left).unwrap();
        assert_eq!(m.natives.selected, 4);
        m.input(Key::Right).unwrap();
        m.input(Key::Right).unwrap();
        assert_eq!(m.natives.selected, 1);
        frames(&mut m, 20);
        snap(&m, &root, "team-1-teammate-selected");
        assert!(texts(m.screen.as_ref().unwrap()).iter().any(|t| t == "Skater 01"));
        // Change Name: option 1 (the picker's Down, then A).
        m.input(Key::Down).unwrap();
        m.input(Key::A).unwrap();
        let keyboard = m.keyboard.as_ref().expect("Change Name opens the keyboard");
        assert_eq!((keyboard.entry.text.as_str(), keyboard.header.as_str()), ("Skater 01", "Enter Skater's Name"));
        snap(&m, &root, "team-2-change-name-keyboard");
        m.keyboard.as_mut().unwrap().entry.text = "Kid".into();
        m.input(Key::Start).unwrap();
        assert_eq!(m.team.member_name(1), "Kid");
        assert!(m.take().contains(&Out::Photo(1)), "a new name queues a new photo");
        m.photo_ready(1, None);
        // The same name again: nothing (no photo, the team not changed by it).
        let dirty = std::mem::take(&mut m.dirty);
        m.named(Naming::Member(1), "Kid").unwrap();
        assert!(!m.dirty && !m.take().contains(&Out::Photo(1)), "an unchanged name does nothing");
        m.dirty = dirty;
        // A redrawn photo shows the new look's file; the old one stays in the cache (a look worn again needs no drawing).
        let folder = std::env::temp_dir().join(format!("skate-team-photos-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = |n: u32| {
            let path = folder.join(format!("test-photo-{n}.rgba"));
            std::fs::write(&path, [0; 4]).unwrap();
            Texture { width: 1, height: 1, rgba: path.to_string_lossy().into_owned(), frame: None }
        };
        let (first, second) = (file(1), file(2));
        m.photo_ready(4, Some(first.clone()));
        m.photo_ready(4, Some(second.clone()));
        assert_eq!(m.image(&photo_name(4)).map(|t| t.rgba.as_str()), Some(second.rgba.as_str()));
        assert!(std::path::Path::new(&first.rgba).exists());
        let _ = std::fs::remove_dir_all(&folder);
        m.photo_ready(4, m.data.team.thumbnails.get("team_profile_background_4").cloned());
        // Invalid characters: popup 263, back to the keyboard.
        m.input(Key::X).unwrap();
        m.keyboard.as_mut().unwrap().entry.text = "Bad~Team".into();
        m.input(Key::Start).unwrap();
        assert!(matches!(m.popup, Some((Popup::Invalid, _))));
        frames(&mut m, 30);
        snap(&m, &root, "team-3-popup-263");
        m.input(Key::A).unwrap();
        assert!(m.popup.is_none() && m.keyboard.is_some());
        m.keyboard.as_mut().unwrap().entry.text = "Bones Brigade".into();
        m.input(Key::Start).unwrap();
        assert_eq!(m.team.name, "Bones Brigade");
        frames(&mut m, 10);
        // Y: the logo grid; moving and A sets the logo and stays; B returns to the cards.
        let before = m.team.logo.clone();
        m.input(Key::Y).unwrap();
        frames(&mut m, 30);
        snap(&m, &root, "team-4-logo-grid");
        m.input(Key::Right).unwrap();
        m.input(Key::Down).unwrap();
        m.take();
        m.input(Key::A).unwrap();
        assert_eq!(m.team.logo, m.data.team.logos[4].key);
        assert_ne!(m.team.logo, before);
        assert_eq!(m.take().iter().filter(|o| matches!(o, Out::Photo(_))).count(), 5, "a new logo redraws every photo");
        frames(&mut m, 20);
        snap(&m, &root, "team-5-logo-picked");
        m.input(Key::B).unwrap();
        assert_eq!(m.natives.mode, 0);
        // B with photos still drawing: no popup 303 (Q3), the save and the pop on the next update; the photos finish
        // into their cache after.
        assert!(!m.pending.is_empty(), "photos still drawing");
        m.input(Key::B).unwrap();
        assert!(m.popup.is_none(), "popup 303 never shows");
        frames(&mut m, 1);
        assert!(m.take().contains(&Out::Closed));
        assert!(m.popup.is_none() && m.screen.is_none());
        for k in 0..5 {
            m.photo_ready(k, None);
        }
        let saved = Team::load(&settings, &crate::team_data::tests::defaults());
        assert_eq!((saved.name.as_str(), saved.member_name(1), saved.logo.as_str()), ("Bones Brigade", "Kid", m.data.team.logos[4].key.as_str()));
        let _ = std::fs::remove_dir_all(settings);
    }

    /// Select Skater (the engine's): offered on every card but the player's; picking it saves the team at once and the
    /// host puts the member's look on the player's skater, its file the customiser's profile. Edit Skater on that
    /// member is then the player's own edit, and on the boss a swap that is put back.
    #[test]
    fn select_skater_puts_the_member_on_the_player() {
        use crate::team_data::look_path;
        let Some((mut m, root, settings)) = manager("select") else { return };
        m.open().unwrap();
        fake_photos(&mut m);
        frames(&mut m, 40);
        let offered = |m: &mut Manager| m.screen.as_mut().unwrap().call_clip(OPTIONS, "GetNumItems", Vec::new()).unwrap().number() == 4.;
        assert!(!offered(&mut m), "the player skates as the boss");
        m.input(Key::Right).unwrap();
        frames(&mut m, 20);
        assert!(offered(&mut m), "teammate 1 can be selected");
        let shown = |m: &Manager| texts(m.screen.as_ref().unwrap()).iter().any(|t| t.contains("Select Skater"));
        assert!(!shown(&m), "three rows show, as retail's");
        snap(&m, &root, "team-6-select-skater");
        for _ in 0..3 {
            m.input(Key::Down).unwrap();
        }
        frames(&mut m, 10);
        assert!(shown(&m), "the picker scrolls to it");
        snap(&m, &root, "team-7-select-skater-hilit");
        m.take();
        m.input(Key::A).unwrap();
        assert!(m.take().contains(&Out::SkateAs(1)));
        assert_eq!(m.team.active, 1);
        assert_eq!(crate::team_data::active_look_path(&settings), look_path(&settings, 1), "saved at once");
        frames(&mut m, 20);
        assert!(!offered(&mut m) && !shown(&m), "not offered on the member the player skates as");
        snap(&m, &root, "team-8-selected");
        m.input(Key::Left).unwrap();
        frames(&mut m, 20);
        assert!(offered(&mut m), "the boss can be selected back");

        // An empty library resolves no look (no head): every member wears the default, told apart by the files.
        let mut parts = crate::customiser_parts::Parts::default();
        parts.library.defaults.insert("male".into(), serde_json::json!({"gender": "male", "selections": {}}));
        let mut customiser = crate::customiser::Customiser::for_test(&parts, Vec::new(), "male");
        customiser.set_profile_path(look_path(&settings, 0));
        (customiser.open, customiser.enabled, customiser.draft) = (false, false, serde_json::json!({"boss": true}));
        let mut models = crate::custom_models::CustomModels::default();
        models.active = Some("pro".into());
        let mut photos = crate::team_photo::Photos::default();
        let mut swapped = None;
        let mut services =
            Services { customiser: &mut customiser, custom_models: &mut models, parts: &parts, photos: &mut photos, root: &root, presence: None };
        serve(&mut m, Out::SkateAs(1), &mut services, &mut swapped);
        assert_eq!(services.customiser.profile_path(), look_path(&settings, 1));
        assert_eq!(services.customiser.draft, member_profile(&settings, 1, &parts));
        assert!(services.customiser.enabled, "the member's look is worn");
        assert!(swapped.is_none() && services.custom_models.requested().is_none(), "the player's character stays on");
        serve(&mut m, Out::EditSkater(1), &mut services, &mut swapped);
        assert!(swapped.is_none(), "member 1 is the player's own skater now");
        serve(&mut m, Out::EditSkater(0), &mut services, &mut swapped);
        assert_eq!(services.customiser.profile_path(), look_path(&settings, 0), "the boss is edited as a teammate");
        swapped.take().unwrap().restore(services.customiser, services.custom_models);
        assert_eq!(services.customiser.profile_path(), look_path(&settings, 1), "and the player's look comes back");
        assert_eq!(services.customiser.draft, member_profile(&settings, 1, &parts));
        let _ = std::fs::remove_dir_all(settings);
    }

    /// Import Skater from Edit Skaters: friends (sorted, the greyed one skipped) then presets; popup 291; Yes applies.
    #[test]
    fn import_skater_lists_friends_and_presets() {
        let Some((mut m, root, settings)) = manager("import") else { return };
        m.open().unwrap();
        fake_photos(&mut m);
        m.input(Key::Right).unwrap();
        m.input(Key::Down).unwrap();
        m.input(Key::Down).unwrap();
        m.input(Key::A).unwrap();
        let import = m.import.as_ref().expect("option 2 opens Import Skater");
        assert_eq!(import.member, 1);
        assert_eq!(import.hilight, 1, "the first friend, past the header");
        // anna's preview is asked for (her download is here), sanitized, in member 1's photo setup.
        let preview = m.take().into_iter().find_map(|o| if let Out::Preview { key, member, look } = o { Some((key, member, look)) } else { None });
        let (key, member, look) = preview.expect("a preview of the hilighted friend");
        assert_eq!((key.as_str(), member), ("import_friend_7_1", 1));
        assert!(look.get("model").is_none());
        // While it draws (+792), A and B wait.
        m.input(Key::A).unwrap();
        m.input(Key::B).unwrap();
        assert!(m.popup.is_none() && m.import.is_some(), "A and B wait for the preview");
        let texture = m.data.team.thumbnails.get("team_profile_background_1").cloned();
        m.preview_ready(&key, texture).unwrap();
        frames(&mut m, 40);
        snap(&m, &root, "team-7-import-skater");
        let shown = texts(&m.import.as_ref().unwrap().screen);
        for want in ["Friends' Skaters", "anna", "Влад", "スケーター", "🛹 Shredder", "Preset Skaters"] {
            assert!(shown.iter().any(|t| t.contains(want)), "{want} missing from {shown:?}");
        }
        // Down past zeke reaches Zoë's greyed row and skips it.
        for _ in 0..3 {
            m.input(Key::Down).unwrap();
        }
        let import = m.import.as_ref().unwrap();
        assert_ne!(import.sources[import.hilight], Source::Friend(4), "Zoë has no published skater");
        fake_previews(&mut m);
        frames(&mut m, 10);
        snap(&m, &root, "team-8-import-moved");
        // A on a row: popup 291 (from Edit Skaters); Yes applies it to member 1 and returns to the cards.
        let row = m.import.as_ref().unwrap().hilight;
        m.input(Key::A).unwrap();
        assert!(matches!(m.popup, Some((Popup::Confirm(r), _)) if r == row));
        frames(&mut m, 30);
        snap(&m, &root, "team-9-popup-291");
        m.input(Key::Down).unwrap();
        m.input(Key::A).unwrap();
        let outs = m.take();
        assert!(outs.iter().any(|o| matches!(o, Out::Apply { member: 1, .. })), "{outs:?}");
        // The host refuses the look: popup 292 over Import Skater itself.
        m.import_failed().unwrap();
        assert!(matches!(m.popup, Some((Popup::Error, _))) && m.import.is_some(), "292 on slot 68");
        frames(&mut m, 30);
        snap(&m, &root, "team-9b-popup-292");
        m.input(Key::A).unwrap();
        // It writes it: back on slot 63, the member's photo redrawn.
        m.import_applied().unwrap();
        assert!(m.import.is_none() && m.screen.is_some(), "back on slot 63");
        assert!(m.take().contains(&Out::Photo(1)), "the imported look's portrait");
        let _ = std::fs::remove_dir_all(settings);
    }

    /// The boot's Preset Skaters (mode 2): popup 293, and the new career's logo picker (state 30).
    #[test]
    fn new_career_screens() {
        let Some((mut m, root, settings)) = manager("career") else { return };
        m.open_new_career_logos().unwrap();
        frames(&mut m, 40);
        snap(&m, &root, "team-10-new-career-logos");
        m.input(Key::Right).unwrap();
        assert_eq!(m.team.logo, m.data.team.logos[1].key, "moving previews the logo");
        m.input(Key::A).unwrap();
        assert!(m.take().contains(&Out::Closed), "A commits and closes");
        m.open_import(0, 2).unwrap();
        assert!(fake_previews(&mut m).iter().all(|(k, member)| k.ends_with("_0") && *member == 0), "member 0's setup");
        frames(&mut m, 40);
        m.input(Key::A).unwrap();
        assert!(matches!(m.popup, Some((Popup::Confirm(_), _))));
        frames(&mut m, 30);
        snap(&m, &root, "team-11-popup-293");
        // B is No; the list scrolls down to the presets (9 rows show: the friends fill the first page).
        m.input(Key::B).unwrap();
        assert!(m.popup.is_none() && m.import.is_some());
        for _ in 0..9 {
            m.input(Key::Down).unwrap();
            fake_previews(&mut m);
        }
        frames(&mut m, 20);
        snap(&m, &root, "team-12-import-presets");
        let import = m.import.as_ref().unwrap();
        assert!(matches!(import.sources[import.hilight], Source::Preset(_)), "{:?}", import.sources[import.hilight]);
        assert!(texts(&import.screen).iter().any(|t| t == "Skater 01"), "{:?}", texts(&import.screen));
        let _ = std::fs::remove_dir_all(settings);
    }

    /// A download landing while Import Skater is open (I7): a state change keeps the movie's rows and its hilight; a new
    /// friend rebuilds the list with the hilight on the same friend.
    #[test]
    fn friend_downloads_keep_the_rows_and_the_hilight() {
        const PICKER: &str = "mImportSkater.mContent.mMenuPicker";
        let shared = std::sync::Arc::new(std::sync::Mutex::new(friends()));
        let Some((mut m, _, settings)) = manager_with("refresh", shared.clone()) else { return };
        m.open_import(1, 0).unwrap();
        fake_previews(&mut m);
        m.input(Key::Down).unwrap();
        m.input(Key::Down).unwrap();
        fake_previews(&mut m);
        frames(&mut m, 5);
        let picker = |m: &mut Manager, method: &str| m.import.as_mut().unwrap().screen.call_clip(PICKER, method, Vec::new()).unwrap().number();
        let (rows, at) = (picker(&mut m, "GetNumItems"), m.import.as_ref().unwrap().hilight);
        assert_eq!(m.import.as_ref().unwrap().sources[at], Source::Friend(1), "on zeke");
        assert_eq!(picker(&mut m, "GetValue"), at as f64);
        // スケーター's skater finishes downloading.
        shared.lock().unwrap().rows.iter_mut().find(|r| r.id == 3).unwrap().state = FriendState::Ready;
        frames(&mut m, 2);
        assert_eq!(picker(&mut m, "GetNumItems"), rows, "no row added");
        assert_eq!(m.import.as_ref().unwrap().hilight, at);
        assert_eq!(picker(&mut m, "GetValue"), at as f64, "the movie's hilight stays");
        assert!(m.import.as_ref().unwrap().screen.bindings.team.rows.iter().all(|r| r.label != crate::apt_text::literal("スケーター") || r.enabled));
        // A friend publishes: one more row, the hilight follows zeke.
        shared.lock().unwrap().rows.push(FriendRow { name: "bob".into(), id: 8, state: FriendState::Ready });
        frames(&mut m, 2);
        assert_eq!(picker(&mut m, "GetNumItems"), rows + 1.);
        let import = m.import.as_ref().unwrap();
        assert_eq!(import.sources[import.hilight], Source::Friend(1), "still on zeke");
        let at = import.hilight;
        assert_eq!(picker(&mut m, "GetValue"), at as f64, "the movie's hilight follows");
        let _ = std::fs::remove_dir_all(settings);
    }

    /// The Workshop agreement notice: once a session on Import Skater; "Open in Steam" asks the helper for the page.
    #[test]
    fn the_workshop_agreement_notice_shows_once() {
        let mut source = friends();
        source.needs_agreement = true;
        let Some((mut m, root, settings)) = manager_with("agreement", std::sync::Arc::new(std::sync::Mutex::new(source))) else { return };
        m.open_import(1, 0).unwrap();
        assert!(matches!(m.popup, Some((Popup::Agreement, _))));
        frames(&mut m, 30);
        snap(&m, &root, "team-13-workshop-agreement");
        m.take();
        m.input(Key::Down).unwrap();
        m.input(Key::A).unwrap();
        assert!(m.popup.is_none());
        assert!(m.take().contains(&Out::OpenAgreement), "Open in Steam");
        m.input(Key::B).unwrap();
        fake_previews(&mut m);
        m.input(Key::B).unwrap();
        m.open_import(1, 0).unwrap();
        assert!(m.popup.is_none(), "once a session");
        let _ = std::fs::remove_dir_all(settings);
    }
}
