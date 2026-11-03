//! The online session HUD of the game whose HUD shows ([`crate::skate2_hud::hud_game`]: the map's, or Settings > HUD
//! Style's), each on its own movies and layer: the event log (fed from [`OnlineRoster::lines`]), the roster (throughout
//! a lobby, giving way to a match's own leaderboard), name tags above remote skaters in the event log's text style, and
//! an online match's results.
//! - Skate 3: online-ui's `hud_online_eventlog2` and `leaderboardhud` (names-only layout) on [`ONLINE`].
//! - Skate 2: hud-skate2/online's `hud_online_eventlog2`, `leaderboardhud` (RankName), `online_chyron` (connection
//!   lost / kicked) and the results screens (`roundresults_lb`: roundresults with its results_lb list;
//!   `throwdownresults` for S.K.A.T.E. and 1-Up) on [`SKATE2_ONLINE`].
//! - Skate 1: hud-skate1/online's `hud_online_eventlog`, and the lobby's overlay over `onlinehud` on [`SKATE1_ONLINE`].
use crate::{
    apt_movie::Movie,
    apt_scene::{Draw, Vertex, apply},
    apt_screen::{ONLINE, SKATE1_ONLINE, SKATE2_ONLINE, Screen, Screens},
    apt_text::Font,
    apt_vm::Value,
    camera::GameplayCamera,
    graphics_menu::Menu,
    map_transition::CurrentMap,
    match_core::{DNF, Match, Mode, Phase, Standing},
    multiplayer::{Multiplayer, appearance::RemoteCharacter},
    online_roster::OnlineRoster,
    skate1_online_hud,
    skate2_hud::hud_game,
};
use bevy::prelude::*;
use serde_json::{Value as Json, json};
use skate_net::roster::Member;
use std::collections::BTreeMap;

/// Lines the movie shows (mcEventLine0-4).
pub(crate) const LINES: usize = 5;
/// Seconds a line stays before its outro.
pub(crate) const SHOW_FOR: f32 = 5.0;
/// The outro label runs frames 22 to the end; tune if the outro is cut.
pub(crate) const OUTRO_FOR: f32 = 0.6;
/// Metres above the skater root; a calibration knob.
const NAME_TAG_HEIGHT: f32 = 2.1;
const NAME_TAG_RANGE: f32 = 60.;
const EVENT_LOG: &str = "1-eventlog";
/// The event log's controller: the root's `var onlineEventLog = new hud_online_eventlog2(this)` (Skate 1's
/// `new hud_online_eventlog(this)`), not `screen`.
const CONTROLLER: &str = "onlineEventLog";
const ROSTER: &str = "2-roster";
/// Skate 1's lobby overlay, over its onlinehud (ROSTER).
const LOBBY: &str = "2-roster-overlay";
/// Skate 2's results screen, over its mode HUD.
const RESULTS: &str = "3-results";
/// The layers of Skate 1's, 2's and 3's online HUD.
const LAYERS: [u8; 3] = [SKATE1_ONLINE, SKATE2_ONLINE, ONLINE];
fn layer_of(game: u8) -> u8 {
    LAYERS[usize::from(game.clamp(1, 3)) - 1]
}
/// `leaderboardhud.playerStructItems`, `MAX_PLAYERS_AVAILABLE` (Skate 1's `NUM_ONLINE_PLAYERS` too).
const PLAYER_SLOTS: usize = 8;
const MAX_PLAYERS: usize = 8;
/// [`roster_grid`]: name_txt's font, height and colour, the highlighted row's colour (sampled from a render), and
/// the share of a row the text fills once rows shrink, the column's inner margin and a name that must fit a column
/// uncut; the last three are tuning knobs.
const ROSTER_FONT: i32 = 133;
const ROSTER_PX: f32 = 18.;
const ROSTER_NAME: [u8; 3] = [0xe4, 0xe0, 0xcc];
const ROSTER_LOCAL: [u8; 3] = [253, 243, 159];
/// Not retail (online safety C4): the host's and a Steam friend's name colour (chat and tags), the mark after it, and
/// chat text's own tint, so nothing a player types reads as theirs or as a game notice. Knobs.
const HOST_NAME: [u8; 3] = [255, 184, 77];
const FRIEND_NAME: [u8; 3] = [140, 222, 140];
const CHAT_TEXT: [u8; 3] = [196, 218, 240];
/// Q13: stage pixels a name draws at most (chat, tags), cut with "...". Knob.
const NAME_MAX: f32 = 220.;
/// Q8: join and leave lines the log shows a minute: per name, in all. Knobs.
const CHURN_EACH: usize = 2;
const CHURN_ALL: usize = 8;
const CHURN_FOR: f32 = 60.;
const ROSTER_FILL: f32 = 0.9;
const ROSTER_PAD: f32 = 6.;
const ROSTER_FIT: &str = "SkaterName";
/// `Layer::extra` key of the grid.
const GRID: &str = "roster";
/// Voice chat (user 10-26): the gap between a name and its mic.
const MIC_GAP: f32 = 4.;

struct Line {
    text: String,
    born: f32,
    /// 0 intro, 1 outro, 2 shown (the movie's Refresh: `gotoAndStop('shown')` for anything but 0 / 1).
    state: u8,
}

#[derive(Default)]
pub(crate) struct EventLog {
    /// Oldest first.
    lines: Vec<Line>,
}
impl EventLog {
    pub fn push(&mut self, text: String, now: f32) {
        self.lines.push(Line { text, born: now, state: 0 });
        if self.lines.len() > LINES {
            self.lines.remove(0);
        }
    }
    /// Starts outros and drops finished lines; true if anything changed.
    pub fn tick(&mut self, now: f32) -> bool {
        let before = self.lines.len();
        self.lines.retain(|l| now < l.born + SHOW_FOR + OUTRO_FOR);
        let mut changed = self.lines.len() != before;
        for line in self.lines.iter_mut().filter(|l| l.state != 1 && now >= l.born + SHOW_FOR) {
            line.state = 1;
            changed = true;
        }
        changed
    }
    /// `EventLog.GetLogEntries`: `[text, state, ...]`. A new line's intro is reported once, then it is shown.
    pub fn entries(&mut self) -> Vec<Json> {
        let out = self.lines.iter().flat_map(|l| [json!(l.text), json!(l.state)]).collect();
        for line in self.lines.iter_mut().filter(|l| l.state == 0) {
            line.state = 2;
        }
        out
    }
}

/// The retail string `id` (or `id` as text when the movie lacks it) with EA Nation / Xbox LIVE read as Steam and its
/// `{0:s}`, `{1:s}`... filled from `arg`'s '\u{1f}'-separated parts in one pass (a name's own `{1:s}` stays as
/// typed); only [`id_parts`] are localized, a name never is (one spelling a string id shows as written).
pub(crate) fn format(language: &BTreeMap<String, String>, id: &str, arg: &str) -> String {
    let text = language.get(id).map_or(id, String::as_str).replace("EA Nation", "Steam").replace("Xbox LIVE", "Steam");
    let parts: Vec<&str> = arg.split('\u{1f}').collect();
    let (mut out, mut rest) = (String::new(), text.as_str());
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let token = rest.find('}').map(|end| &rest[..=end]);
        let index = token.and_then(|t| t.strip_prefix('{')?.strip_suffix(":s}")?.parse::<usize>().ok()).filter(|&i| i < parts.len());
        match (index, token) {
            (Some(i), Some(token)) => {
                let part = parts[i];
                out.push_str(if id_parts(id).contains(&i) { language.get(part).map_or(part, String::as_str) } else { part });
                rest = &rest[token.len()..];
            }
            _ => {
                out.push('{');
                rest = &rest[1..];
            }
        }
    }
    out + rest
}
/// The parts of a line's `arg` that are string ids (a place, a challenge, a team); every other part is a name.
fn id_parts(id: &str) -> &'static [usize] {
    match id {
        "ID_ONLINE_EVENT_LOCATION_CHANGE" => &[0, 1],
        "ID_ONLINE_EVENT_SWITCHED_TEAMS" => &[1],
        _ => &[],
    }
}

/// What leaderboardhud asks, for the names-only roster (`lb.dis` SetupScreen / RefreshAll / UpdateHighlight;
/// UpdateVOIPIcons: `voip`, each name's voice::voip_status). `host`: the row with the yellow arrow.
pub(crate) fn roster_answers(names: &[String], local: usize, host: usize, title: &str, voip: &[i64]) -> Vec<(&'static str, Json)> {
    // More than MAX_PLAYERS_AVAILABLE: 8 blank rows keep the box at full size and roster_grid draws the names and mics.
    let (names, local, host, voip) = if names.len() > MAX_PLAYERS {
        (vec![String::new(); MAX_PLAYERS], usize::MAX, usize::MAX, &[][..])
    } else {
        (names.to_vec(), local, host, voip)
    };
    // [numPlayers, numTop, layout eLeaderBoardHUD_RankName (hides rank and score), variant (ID_COMMON_UNKNOWN hides
    // the variant row; 0 drew a "0" under the title), isTeams, isOnlineMission, showPlayers]
    let setup = json!([names.len(), 0, 11, "ID_COMMON_UNKNOWN", false, true, true]);
    // Per player: rank text, score (< 0 greys the row as logged off), score text, highlight, turn marker (the host's
    // yellow arrow in the lobby), team, row ("player_" + row), sort id.
    let data: Vec<Json> =
        (0..names.len()).flat_map(|i| [json!(""), json!(0), json!(""), json!(i == local), json!(i == host), json!(0), json!(i), json!(i)]).collect();
    debug_assert_eq!(data.len(), names.len() * PLAYER_SLOTS);
    vec![
        ("HUDComponents.Leaderboard_GetSetupDataBlock", setup),
        ("HUDComponents.Leaderboard_GetLeaderboardNameBlock", json!(names)),
        ("HUDComponents.Leaderboard_GetLeaderboardDataBlock", Json::Array(data)),
        ("Mission.GetTitle", json!(title)),
        ("FELeaderboard.GetVoipStatus", json!((0..names.len()).map(|i| voip.get(i).copied().unwrap_or(0)).collect::<Vec<_>>())),
    ]
}

/// What Skate 2's leaderboardhud asks for the roster: eLeaderBoardHUD_RankName (its 'lobby' frame: names, no score).
fn skate2_roster_answers(names: &[String], local: usize, title: &str) -> Vec<(&'static str, Json)> {
    // More than its 8 rows: the 8 longest names, unlit, keep the box at full height and as wide as retail makes it
    // (RefreshAll sizes it by its widest name) and skate2_roster_grid draws everyone over it.
    // ponytail: longest by characters, not by measured width.
    let (names, local) = if names.len() > MAX_PLAYERS {
        let mut widest = names.to_vec();
        widest.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
        widest.truncate(MAX_PLAYERS);
        (widest, usize::MAX)
    } else {
        (names.to_vec(), local)
    };
    let rows: Vec<_> = (names.into_iter().enumerate()).map(|(i, n)| (n, 0, String::new(), i == local)).collect();
    crate::skate2_contests::leaderboard_answers(title, true, 11, "ID_COMMON_UNKNOWN", &rows, None).to_vec()
}

/// s2fix R2 (10-28): Skate 2's roundresults_lb text table has no ID_MAP_FILTER_CHALLENGETYPES_BESTTRICK / _JAM (the
/// lobby's type names), so its title cell showed the raw id; its own ids for them are the online mission templates'.
fn skate2_type_name(kind: &str) -> &str {
    match kind {
        "ID_MAP_FILTER_CHALLENGETYPES_BESTTRICK" => "ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK",
        "ID_MAP_FILTER_CHALLENGETYPES_JAM" => "ID_MISSION_TEMPLATE_ONLINE_JAM",
        other => other,
    }
}

/// What Skate 2's results screen (roundresults with its results_lb list) asks for an online match: the list layout
/// (eResultsLBType), the titles each mode's Lua sets, and a row per player, best first ([rank, int, float, string,
/// time]; results_lb places a row by its rank).
pub(crate) fn results_answers(mode: Mode, standings: &[Standing], title: &str, kind: &str) -> Vec<(&'static str, Json)> {
    const RANK: &str = "ID_LEADERBOARD_HEADER_RANK";
    const NAME: &str = "ID_LEADERBOARD_NAME_HEADER";
    // 0 PlaceNameInt [rank, name, int], 3 DeathRace [rank, name, time], 5 BestTrickNonElimInt [rank, name, int],
    // 7 PlaceNameString [rank, name, string].
    let (layout, main, cols): (u8, &str, [&str; 3]) = match mode {
        // r_*_ol.lua results_screen, its round results.
        Mode::DeathRace | Mode::SpotRace => {
            (3, "ID_CHALLENGE_COMMON_RACE_TIMES", [RANK, "ID_LEADERBOARD_HEADER_SKATER", "ID_LEADERBOARD_HEADER_TIME"])
        }
        // The contests' final standings (ResultsScreenHALIDCol1-3).
        Mode::BestTrick | Mode::Jam => (5, "ID_CHALLENGE_COMMON_CONTEST_STANDINGS", ["ID_COMMON_RANK", "ID_COMMON_NAME", "ID_COMMON_SCORE"]),
        // Spot Battle and Hall of Meat (ol_hom_*.lua's leaderboard state) set no main title. S.K.A.T.E. and 1-Up have
        // throwdownresults ([`throwdown_answers`]).
        Mode::SpotBattle | Mode::HallOfMeat | Mode::Skate | Mode::OneUp => (0, "", [RANK, NAME, "ID_LEADERBOARD_POINTS_HEADER"]),
        // Skate 3's own modes show their own results (owned rows); this is the generic points layout.
        Mode::Domination | Mode::Contest | Mode::OwnTheSpot | Mode::OwnTheLot | Mode::Film | Mode::Photo => {
            (0, "", [RANK, NAME, "ID_LEADERBOARD_POINTS_HEADER"])
        }
    };
    let data: Vec<Json> = (standings.iter().enumerate())
        .flat_map(|(i, s)| {
            let time = match mode {
                Mode::DeathRace | Mode::SpotRace if !s.finished || s.score >= DNF => "ID_RACE_DNF".to_owned(),
                Mode::DeathRace | Mode::SpotRace => format!("#{}", mode.show(s.score)),
                _ => String::new(),
            };
            [json!(i + 1), json!(s.score), json!(0), json!(""), json!(time)]
        })
        .collect();
    let names: Vec<String> = standings.iter().map(|s| crate::apt_text::literal(&s.name)).collect();
    vec![
        ("HUDComponents.Results_GetNumPlayers", json!(standings.len())),
        ("HUDComponents.Results_GetLayoutType", json!(layout)),
        ("HUDComponents.Results_GetLeaderboardDataBlock", Json::Array(data)),
        ("HUDComponents.Results_GetLeaderboardNameBlock", json!(names)),
        ("HUDComponents.Results_GetLeaderboardMainTitle", json!(main)),
        ("HUDComponents.Results_GetLeaderboardColTitle(0)", json!(cols[0])),
        ("HUDComponents.Results_GetLeaderboardColTitle(1)", json!(cols[1])),
        ("HUDComponents.Results_GetLeaderboardColTitle(2)", json!(cols[2])),
        ("HUDComponents.Results_GetIsNavigatable", json!(false)),
        ("Mission.GetTitle", json!(title)),
        ("Mission.GetChallengeTypeName", json!(kind)),
        ("Mission.GetDescription", json!("")),
    ]
}

/// What Skate 2's S.K.A.T.E. / 1-Up results screen (challenge/throwdownresults) asks: GetEndMissionThrowDownType (1
/// 1-Up, 2 S.K.A.T.E.) and a row per player, best first ([rank, letters or rounds won, ...]: Results_SetPlayerData's
/// int, the frame its mcSkate row plays; the local player's rank highlights its row). The '1up' frame draws its mcSkate
/// rows at alpha 0 and the movie has no mcUp clips, so 1-Up's rounds aren't drawn: names in rank order, as on the disc.
// ponytail: FillData's gotoAndPlay lands on mcSkate's '0'-'5' labels only when given a string, so the count goes as
// one. No end options: the match core moves on by itself.
pub(crate) fn throwdown_answers(mode: Mode, standings: &[Standing], title: &str) -> Vec<(&'static str, Json)> {
    let me = standings.iter().position(|s| s.local);
    let data: Vec<Json> = (standings.iter().enumerate())
        .flat_map(|(i, s)| {
            let count = if mode == Mode::Skate { -s.score } else { s.score };
            [json!(i + 1), json!(count.clamp(0, 5).to_string()), json!(0), json!(""), json!("")]
        })
        .collect();
    let names: Vec<String> = standings.iter().map(|s| crate::apt_text::literal(&s.name)).collect();
    vec![
        ("Mission.GetEndMissionThrowDownType", json!(if mode == Mode::OneUp { 1 } else { 2 })),
        ("Mission.GetWinState", json!(me == Some(0))),
        ("Mission.GetTitle", json!(title)),
        ("Mission.GetNumEndMissionOptions", json!(0)),
        ("HUDComponents.Results_GetNumPlayers", json!(standings.len())),
        ("HUDComponents.Results_GetLeaderboardDataBlock", Json::Array(data)),
        ("HUDComponents.Results_GetLeaderboardNameBlock", json!(names)),
        ("HUDComponents.Results_GetSelfRank", json!(me.map_or(0, |i| i + 1))),
    ]
}

/// `text` in `font`, `height` px tall, centred on `center` (stage pixels) and sitting on it, tinted `multiply`: the
/// atlas glyphs as one draw, then any system-font glyph (a name in another script; text_fallback) as its own.
pub(crate) fn name_tag(font: &Font, text: &str, height: f32, center: [f32; 2], multiply: [f32; 4]) -> Vec<Draw> {
    let text = &*crate::text_fallback::nfc(text);
    let sx = font.scale[0] * height;
    let sy = font.scale[1] * height;
    let mut x = center[0] - font.width(text, height) / 2. + font.offset[0] * sx;
    let y = center[1] - height + font.offset[1] * sy;
    let mut out = vec![Draw { texture: font.texture.clone(), size: font.size, vertices: Vec::new(), multiply, add: [0.; 4] }];
    for c in text.chars() {
        if let Some((g, texture, size)) = font.place(c) {
            let quad = crate::apt_scene::glyph_quad(font, &g, size, [x, y], [sx, sy]);
            if texture == font.texture {
                out[0].vertices.extend(quad);
            } else if size[0] > 0 {
                out.push(Draw { texture: texture.to_owned(), size, vertices: quad.to_vec(), multiply, add: [0.; 4] });
            }
            x = g.x_advance.mul_add(sx, x);
        }
    }
    out
}

/// A name tag's passes in the event log line's style, as apt_scene draws the movie's text fields in order (Skate 2's
/// glow under its text): (font, colour, x offset, height) each; a shadow font is a black pass, then its foreground 1 px
/// right.
fn tag_passes(movie: &Movie) -> Vec<(&Font, [f32; 4], f32, f32)> {
    movie.characters.values().filter_map(|c| c.text.as_ref()).flat_map(|text| text_passes(movie, text)).collect()
}
/// One text field's passes (see [`tag_passes`]).
fn text_passes<'a>(movie: &'a Movie, text: &Json) -> Vec<(&'a Font, [f32; 4], f32, f32)> {
    let fonts = &movie.text_assets.fonts;
    let Some(font) = text["font_id"].as_i64().and_then(|id| fonts.get(&(id as i32))) else { return Vec::new() };
    let height = text["font_height"].as_f64().unwrap_or(18.) as f32;
    let argb = text["color_argb"].as_str().and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok()).unwrap_or(!0);
    let colour = [16, 8, 0, 24].map(|shift| ((argb >> shift) & 255) as f32 / 255.);
    match font.foreground.and_then(|f| fonts.get(&f)) {
        Some(fore) => vec![(font, [0., 0., 0., colour[3]], 0., height), (fore, colour, 1., height)],
        None => vec![(font, colour, 0., height)],
    }
}

/// `text` cut to `room` wide (measured by `width`) with `dots` on the end, if it is wider.
fn fit(width: &dyn Fn(&str) -> f32, text: &str, room: f32, dots: &str) -> String {
    let mut text = text.to_owned();
    if width(&text) > room {
        while !text.is_empty() && width(&format!("{text}{dots}")) > room {
            text.pop();
        }
        text.push_str(dots);
    }
    text
}
/// Voice chat: a grid name's mic, lit while they talk (voice::voip_status 2), else silent; `height` tall from `at`,
/// faded with the box.
fn grid_mic(status: i64, at: [f32; 2], height: f32, fade: f32) -> Draw {
    let mut mic = crate::graphic_board::button(if status == 2 { "MicOn" } else { "MicOff" }, at, height);
    mic.multiply[3] *= fade;
    mic
}
/// C4: `name` (cut to [`NAME_MAX`]) and its host / friend mark, and the colour that goes with it (None: the plain one).
/// User 10-25: the verified creator (creator.rs) reads "Name - Creator" (then " - Host" when hosting), all in the tag's
/// colour, which sweeps the rainbow (user 10-26; these draws are rebuilt every frame, so it moves at no cost).
fn marked(width: &dyn Fn(&str) -> f32, name: &str, host: bool, friend: bool, creator: bool) -> (String, Option<[u8; 3]>) {
    let name = fit(width, name, NAME_MAX, "...");
    match (creator, host, friend) {
        (true, true, _) => (format!("{name}{} - Host", crate::creator::MARK), Some(crate::creator::colour())),
        (true, false, _) => (format!("{name}{}", crate::creator::MARK), Some(crate::creator::colour())),
        (_, true, _) => (format!("{name} - Host"), Some(HOST_NAME)),
        (_, _, true) => (format!("{name} - Friend"), Some(FRIEND_NAME)),
        _ => (name, None),
    }
}

/// The names of a roster past the movie's 8 rows, in columns over its 8-row box (stage pixels): 2 columns up to 16
/// players, then 3 (17 rows at 50), the text shrinking with the rows and long names cut with "…". Hides the movie's
/// rows and their backers and draws the stripes in their place (black at the backers' dark / dim alpha). None: not
/// that movie.
fn roster_grid(screen: &mut Screen, names: &[String], local: usize, voip: &[i64]) -> Option<Vec<Draw>> {
    let hud = screen.clip("mLeaderboardHUD")?;
    let vm = &mut screen.vm;
    let clips = |vm: &crate::apt_vm::Vm, prefix: &str| -> Option<Vec<usize>> {
        (0..MAX_PLAYERS)
            .map(|i| match vm.get(hud, &format!("{prefix}{i}")) {
                Value::Object(o) => Some(o),
                _ => None,
            })
            .collect()
    };
    let (backers, rows) = (clips(&*vm, "playerBacker_")?, clips(&*vm, "player_")?);
    let n = |o: usize, key: &str| vm.get(o, key).number() as f32;
    // ponytail: offsets only; the names-only layout never scales or turns mLeaderboardHUD or its backers.
    let (first, last) = (backers[0], backers[MAX_PLAYERS - 1]);
    let [x0, y0] = [n(hud, "_x") + n(first, "_x"), n(hud, "_y") + n(first, "_y")];
    let [width, height] = [n(first, "_width"), n(hud, "_y") + n(last, "_y") + n(last, "_height") - y0];
    let fade = n(hud, "_alpha") / 100.;
    let stripes = [n(backers[0], "_alpha"), n(backers[1], "_alpha")].map(|a| a / 100. * fade);
    // ponytail: the rows' colour swatches go with them (8 colours for up to 50 players); add a swatch per cell if
    // players need telling apart by colour.
    for clip in backers.into_iter().chain(rows) {
        vm.set(clip, "_visible", Value::Bool(false)).ok()?;
    }
    let font = screen.bindings.movie.text_assets.fonts.get(&ROSTER_FONT)?;
    let columns = if names.len() <= 2 * MAX_PLAYERS { 2 } else { 3 };
    let rows = names.len().div_ceil(columns).max(MAX_PLAYERS);
    let (column, row) = (width / columns as f32, height / rows as f32);
    let room = column - 2. * ROSTER_PAD;
    // As big as the movie's names, then as the rows allow, then as a typical name fits its column.
    let px = ROSTER_PX.min(row * ROSTER_FILL).min(room / font.width(ROSTER_FIT, 1.));
    let mut draws: Vec<Draw> = (0..rows)
        .map(|r| {
            let top = y0 + r as f32 * row;
            let points = [[x0, top], [x0 + width, top], [x0 + width, top + row], [x0, top + row]];
            let uvs = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
            Draw {
                texture: "runtime/white.rgba".into(),
                size: [1, 1],
                vertices: [0, 1, 2, 0, 2, 3].map(|i| Vertex { position: points[i], uv: uvs[i] }).into(),
                multiply: [0., 0., 0., stripes[r % 2]],
                add: [0.; 4],
            }
        })
        .collect();
    let dots = if font.glyphs.contains_key(&0x2026) { "…" } else { "..." };
    for (i, name) in names.iter().enumerate() {
        // Down each column first. The movie's 18 px name sits at the top of its 24 px row: that cell, centred. A player
        // with voice (voice::voip_status 1, 2) has their mic after the name, in its room.
        let (left, top) = (x0 + (i / rows) as f32 * column + ROSTER_PAD, y0 + (i % rows) as f32 * row);
        let mic = voip.get(i).copied().filter(|status| matches!(status, 1 | 2));
        let text = fit(&|s| font.width(s, px), name, room - mic.map_or(0., |_| px * 4. / 3. + MIC_GAP), dots);
        let [r, g, b] = (if i == local { ROSTER_LOCAL } else { ROSTER_NAME }).map(|c| f32::from(c) / 255.);
        let (width, line) = (font.width(&text, px), top + (row - px * 4. / 3.) / 2. + px);
        draws.extend(name_tag(font, &text, px, [left + width / 2., line], [r, g, b, fade]));
        draws.extend(mic.map(|status| grid_mic(status, [left + width + MIC_GAP, line - px], px, fade)));
    }
    Some(draws)
}

/// Skate 2's roster past its 8 rows: [`roster_grid`]'s columns over leaderboardhud's 8-row box (mcStrip0-7, a strip
/// a row: GetYScale's 24 px a player), in the movie's own look: mcStrip0's art on every other row (mcStrip1/3/5's and
/// mcStrip7's is authored at alpha 0), name_txt's font and height, kFONT_COLOR_NORMAL / kFONT_COLOR_HIGHLIGHTED, names
/// right-aligned (its autoSize "right"). Hides the movie's strips and rows, their colour swatches with them. None:
/// not that movie.
fn skate2_roster_grid(screen: &mut Screen, names: &[String], local: usize, voip: &[i64]) -> Option<Vec<Draw>> {
    let hud = screen.clip("mcLeaderboardHUD")?;
    let field = screen.clip("mcLeaderboardHUD.playerName0.mLeaderboardData.name_txt")?;
    let class = screen.clip("_global.leaderboardhud")?;
    let (vm, movie) = (&screen.vm, &screen.bindings.movie);
    let clips = |prefix: &str| -> Option<Vec<usize>> {
        (0..MAX_PLAYERS)
            .map(|i| match vm.get(hud, &format!("{prefix}{i}")) {
                Value::Object(o) => Some(o),
                _ => None,
            })
            .collect()
    };
    let hidden: Vec<usize> = clips("mcStrip")?.into_iter().chain(clips("playerName")?).collect();
    // mcStrip0's art (the shape under it), where it draws and its alpha down from the root.
    let mut art = hidden[0];
    while movie.characters.get(&movie.instances.get(&art)?.character)?.type_name != "shape" {
        art = *movie.instances[&art].children.values().next()?;
    }
    let shape = screen.shapes.get(&movie.instances[&art].character)?.first()?;
    let (mut fade, mut clip) = (1., art);
    loop {
        let alpha = movie.instances.get(&clip).and_then(|i| i.placement.as_ref()).map_or(255, |p| p.color[0]);
        fade *= f32::from(alpha) / 255. * (vm.get(clip, "_alpha").number() as f32 / 100.).clamp(0., 1.);
        match vm.get(clip, "_parent") {
            Value::Object(parent) if parent != clip && movie.instances.contains_key(&parent) => clip = parent,
            _ => break,
        }
    }
    let m = screen.stage_matrix(art);
    let (lo, hi) = (shape.triangles.iter().flatten())
        .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), v| (std::array::from_fn(|i| lo[i].min(v.position[i])), std::array::from_fn(|i| hi[i].max(v.position[i]))));
    // One strip a row; PositionHUD flips and stretches each strip to the back's width.
    let pitch = hi[1] - lo[1];
    let ([left, y0], [right, _]) = (apply(m, lo), apply(m, [hi[0], lo[1]]));
    let (x0, width) = (left.min(right), (left - right).abs());
    let height = apply(m, [lo[0], lo[1] + MAX_PLAYERS as f32 * pitch])[1] - y0;
    let columns = if names.len() <= 2 * MAX_PLAYERS { 2 } else { 3 };
    let rows = names.len().div_ceil(columns).max(MAX_PLAYERS);
    let squeeze = MAX_PLAYERS as f32 / rows as f32;
    let mut draws: Vec<Draw> = (0..rows)
        .step_by(2)
        .map(|r| Draw {
            texture: shape.texture.rgba.clone(),
            size: [shape.texture.width, shape.texture.height],
            vertices: (shape.triangles.iter().flatten())
                .map(|v| {
                    let y = lo[1] + (r as f32 + (v.position[1] - lo[1]) / pitch) * pitch * squeeze;
                    Vertex { position: apply(m, [v.position[0], y]), uv: v.uv }
                })
                .collect(),
            multiply: [shape.color[0], shape.color[1], shape.color[2], shape.color[3] * fade],
            add: [0.; 4],
        })
        .collect();
    let passes = text_passes(movie, movie.characters.get(&movie.instances.get(&field)?.character)?.text.as_ref()?);
    let &(font, _, _, size) = passes.last()?;
    let colour = |key: &str| {
        let c = vm.get(class, key).number() as u32;
        [16, 8, 0].map(|shift| ((c >> shift) & 255) as f32 / 255.)
    };
    let (normal, lit) = (colour("kFONT_COLOR_NORMAL"), colour("kFONT_COLOR_HIGHLIGHTED"));
    let (column, row) = (width / columns as f32, height / rows as f32);
    let room = column - 2. * ROSTER_PAD;
    let px = size.min(row * ROSTER_FILL).min(room / font.width(ROSTER_FIT, 1.));
    let dots = if font.glyphs.contains_key(&0x2026) { "…" } else { "..." };
    for (i, name) in names.iter().enumerate() {
        // Down each column first, each name against its cell's right edge, a player with voice's mic before it.
        let (end, top) = (x0 + (i / rows + 1) as f32 * column - ROSTER_PAD, y0 + (i % rows) as f32 * row);
        let mic = voip.get(i).copied().filter(|status| matches!(status, 1 | 2));
        let text = fit(&|s| font.width(s, px), name, room - mic.map_or(0., |_| px * 4. / 3. + MIC_GAP), dots);
        let [r, g, b] = if i == local { lit } else { normal };
        let (width, line) = (font.width(&text, px), top + (row - px * 4. / 3.) / 2. + px);
        for &(pass, own, dx, h) in &passes {
            let tint = if std::ptr::eq(pass, font) { [r, g, b, own[3] * fade] } else { [own[0], own[1], own[2], own[3] * fade] };
            draws.extend(name_tag(pass, &text, px, [end - width / 2. + dx * px / h, line], tint));
        }
        draws.extend(mic.map(|status| grid_mic(status, [end - width - MIC_GAP - px * 4. / 3., line - px], px, fade)));
    }
    for clip in hidden {
        screen.vm.set(clip, "_visible", Value::Bool(false)).ok()?;
    }
    Some(draws)
}

/// Viewport pixels to stage pixels on the canvas's AutoMin projection: the 1280x720 stage is centred and the
/// window's extra width (or height) shows beyond it.
fn to_stage(p: Vec2, size: Vec2) -> Vec2 {
    let k = (1280. / size.x).max(720. / size.y);
    (p - size / 2.) * k + Vec2::new(640., 360.)
}

pub(crate) struct OnlineHudPlugin;
impl Plugin for OnlineHudPlugin {
    fn build(&self, app: &mut App) {
        // Before render, so a line, the roster and the tags show the frame they change.
        app.add_systems(
            Update,
            (
                results.before(event_log),
                results_quit.after(results),
                event_log,
                central,
                roster_hud,
                name_tags,
                observing.after(event_log),
                chat_feed.after(event_log).after(crate::online_chat::exchange),
            )
                .after(crate::online_roster::track)
                .before(crate::apt_screen::render),
        );
    }
}

#[derive(Default)]
struct LogScreen {
    log: EventLog,
    /// The layer the movie is open on.
    open: Option<u8>,
    /// The movie failed; not reopened until the session and its lines are over.
    failed: bool,
    /// The chyron: shown since, its outro started.
    chyron: Option<(f32, bool)>,
    /// The layer the chyron is open on.
    chyron_layer: u8,
    /// Skate 3's online_chyron is installed ([`S3_CHYRON`]), once looked up.
    s3_chyron: Option<bool>,
    churn: Churn,
}
/// Q8: the join and leave lines shown in the last [`CHURN_FOR`] s, so a player rejoining over and over (or a crowd
/// of them) can't fill the log.
#[derive(Default)]
struct Churn(std::collections::VecDeque<(String, f32)>);
impl Churn {
    fn allow(&mut self, name: &str, now: f32) -> bool {
        self.0.retain(|(_, at)| now - at < CHURN_FOR);
        let ok = self.0.len() < CHURN_ALL && self.0.iter().filter(|(n, _)| n == name).count() < CHURN_EACH;
        if ok {
            self.0.push_back((name.to_owned(), now));
        }
        ok
    }
}
fn churns(id: &str) -> bool {
    matches!(id, "ID_ONLINE_EVENT_JOIN_SKATER" | "ID_ONLINE_EVENT_JOIN_OBSERVER" | "ID_ONLINE_EVENT_LEAVE")
}

/// Skate 2's online_chyron, over the online HUD.
const CHYRON: &str = "4-chyron";
/// Seconds the chyron shows before its outro, and the outro's 15 frames.
// ponytail: the image's chyron timing isn't recovered; a knob.
const CHYRON_FOR: f32 = 6.;
const CHYRON_OUTRO: f32 = 0.3;

/// Skate 3's online_chyron (OnlineChyronHUD, screen table 0x8302F1A8 row 57), prepared by the online-movies stage
/// (tools/prepare_online_movies.py) to draw on [`ONLINE`].
const S3_CHYRON: &str = "private/online-movies/runtime/online_chyron.json";

/// What a game puts on its chyron (OnlineChyron_GetData's [title, body]) instead of the log or a popup.
/// Skate 2: being kicked, and losing the connection to an online game (the image's two OnlineChyron callers the engine
/// has events for). Skate 3 (`sub_8266B470`'s callers): being kicked (0x826E92CC) and the lost EA Nation (here Steam)
/// connection (0x826FA5B8; the body outside an online game, as `sub_82743FC0`'s test is not traced: UNTRACED).
fn chyron_of(game: u8, id: &str) -> Option<[&'static str; 2]> {
    match (game, id) {
        (2 | 3, "ID_ONLINE_KICKED_BY_GAME_HOST") => Some(["ID_ONLINE_CHYRON_HEADER_EANATION", "ID_ONLINE_CHYRON_LOSTCONNECTION"]),
        (2, "ID_ONLINE_GAME_LEFT") => Some(["ID_EANATION", "ID_ONLINE_DISCONNECT_FROM_EA_SERVER_ONLINEGAME"]),
        (3, "ID_ONLINE_DISCONNECT_FROM_EA_SERVER") => Some(["ID_EANATION", "ID_ONLINE_DISCONNECT_FROM_EA_SERVER"]),
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
enum Route {
    Log,
    Chyron,
    Popup,
}
/// Where a line shows. Skate 3 puts a session's end in an OK sk8popup (online_menu's notice; TU3 popup table
/// 0x83028050 rows 67 game left, 104 failed to join, 110 lobby full; 64 kicked and 74 Steam / EA Nation lost only
/// while its chyron movie is not installed), Skate 2 and 3 on their chyron ([`chyron_of`]; item 30b: the chyron's
/// callers are found, rows 64's and 74's are not). Skate 1's image is not traced for these (UNTRACED): its log keeps them.
fn route(game: u8, id: &str, s3_chyron: bool) -> Route {
    const POPUPS: [&str; 5] = [
        "ID_ONLINE_KICKED_BY_GAME_HOST",
        "ID_ONLINE_GAME_LEFT",
        "ID_ONLINE_DISCONNECT_FROM_EA_SERVER",
        "ID_ONLINE_PLAYNOW_ERROR_STARTING",
        "ID_JOIN_ONLINE_GAME_FAILED_POPUP_MESSAGE",
    ];
    match game {
        2 if chyron_of(2, id).is_some() => Route::Chyron,
        3 if s3_chyron && chyron_of(3, id).is_some() => Route::Chyron,
        3 if POPUPS.contains(&id) => Route::Popup,
        _ => Route::Log,
    }
}

/// online_chyron (`SetState`: Skate 2's 6, Skate 3's 7 (its error layout) set the text and play the intro; 4 the
/// outro): opened for a new message, closed after its outro. It outlives the session it reports the end of.
fn chyron(state: &mut LogScreen, screens: &mut Screens, new: Option<(u8, [&str; 2])>, now: f32, assets: &std::path::Path) {
    if let Some((game, _)) = new {
        let layer = if game == 3 { ONLINE } else { SKATE2_ONLINE };
        if state.chyron.is_some() && state.chyron_layer != layer {
            screens.layer(state.chyron_layer).close(CHYRON);
        }
        state.chyron_layer = layer;
    }
    let layer = screens.layer(state.chyron_layer);
    let result = (|| -> Result<(), String> {
        if let Some((game, [title, body])) = new {
            if layer.get(CHYRON).is_none() {
                if game == 3 {
                    layer.open.insert(CHYRON.to_owned(), Screen::load(&assets.join(S3_CHYRON), &[])?);
                } else {
                    layer.open(CHYRON, "online_chyron", &[])?;
                }
            }
            let screen = layer.get(CHYRON).ok_or("the screen stopped")?;
            let language = &screen.bindings.movie.text_assets.language;
            let data = json!([format!("#{}", format(language, title, "")), format!("#{}", format(language, body, ""))]);
            screen.answer("HUDComponents.OnlineChyron_GetData", data);
            screen.call("SetState", &[json!(if game == 3 { 7 } else { 6 })])?;
            state.chyron = Some((now, false));
            return Ok(());
        }
        match state.chyron {
            Some((since, _)) if now >= since + CHYRON_FOR + CHYRON_OUTRO => {
                layer.close(CHYRON);
                state.chyron = None;
            }
            Some((since, false)) if now >= since + CHYRON_FOR => {
                layer.get(CHYRON).ok_or("the screen stopped")?.call("SetState", &[json!(4)])?;
                state.chyron = Some((since, true));
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(error) = result {
        warn!("Online chyron: {error}");
        layer.close(CHYRON);
        state.chyron = None;
    }
}

/// The central message this player alone sees ([`OnlineRoster::central`]): skate3_modes' hudcentral on the online layer.
const CENTRAL: &str = "6-online-central";

/// HUD2.ShowCentralMessage(id, White, hook 0) as 0x82663f38 runs it: hudcentral's SetState 1 on load, then 4 with
/// Central_GetMessageData [text, colour 0] and Central_GetAudioHook 0 (skate3_modes HudDriver).
fn central_show(screen: &mut Screen, fresh: bool, id: &str) -> Result<(), String> {
    if fresh {
        screen.call("SetState", &[json!(1)])?;
    }
    let text = format(&screen.bindings.movie.text_assets.language, id, "");
    screen.answer("HUDComponents.Central_GetMessageData", json!([text, 0]));
    screen.answer("HUDComponents.Central_GetAudioHook", json!(0));
    screen.call("SetState", &[json!(4)]).map(drop)
}

/// Shows [`OnlineRoster::central`] (the host's kick went through, 0x8262cf80) and plays it out after
/// skate3_modes::CENTRAL_FOR (SetState 5); closed once over and out of the session.
fn central(
    mut shown: Local<Option<f32>>,
    mut roster: ResMut<OnlineRoster>,
    net: Res<Multiplayer>,
    config: Res<crate::config::Config>,
    screens: Option<ResMut<Screens>>,
    time: Res<Time<Real>>,
) {
    let new = roster.central.take();
    // Out of the session nothing holds it (a warning held as the session ended).
    roster.central_held &= net.active();
    let Some(mut screens) = screens else { return };
    let layer = screens.layer(ONLINE);
    let now = time.elapsed_secs();
    let result = (|| -> Result<(), String> {
        if let Some(id) = new {
            let fresh = layer.get(CENTRAL).is_none();
            if fresh {
                let path = config.asset_root.join(crate::skate3_modes::HUD_DIR).join("hudcentral.json");
                layer.open.insert(CENTRAL.to_owned(), Screen::load(&path, &[])?);
            }
            central_show(layer.get(CENTRAL).ok_or("the screen stopped")?, fresh, &id)?;
            *shown = Some(now);
        } else if !roster.central_held && shown.is_some_and(|at| f64::from(now - at) >= crate::skate3_modes::CENTRAL_FOR) {
            *shown = None;
            layer.get(CENTRAL).ok_or("the screen stopped")?.call("SetState", &[json!(5)])?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        warn!("Online central message: {error}");
        *shown = None;
        layer.close(CENTRAL);
    } else if shown.is_none() && !net.active() {
        layer.close(CENTRAL);
    }
}

fn event_log(
    mut state: Local<LogScreen>,
    mut roster: ResMut<OnlineRoster>,
    net: Res<Multiplayer>,
    menu: Res<Menu>,
    map: Res<CurrentMap>,
    screens: Option<ResMut<Screens>>,
    time: Res<Time<Real>>,
    ui: Option<ResMut<crate::online_menu::OnlineUi>>,
    config: Res<crate::config::Config>,
) {
    // Drained even without screens (no UI camera), so lines never pile up.
    let mut lines = std::mem::take(&mut roster.lines);
    let now = time.elapsed_secs();
    lines.retain(|(id, arg)| !churns(id) || state.churn.allow(arg, now));
    let game = hud_game(Some(&menu), &map);
    let s3_chyron = *state.s3_chyron.get_or_insert_with(|| config.asset_root.join(S3_CHYRON).is_file());
    let (popups, lines): (Vec<_>, Vec<_>) = lines.into_iter().partition(|(id, _)| route(game, id, s3_chyron) == Route::Popup);
    if let Some(mut ui) = ui {
        ui.requests.extend(popups.into_iter().map(|(id, _)| crate::online_menu::OnlineRequest::Notice(id)));
    }
    let Some(mut screens) = screens else { return };
    let state = &mut *state;
    let (shout, lines): (Vec<_>, Vec<_>) = lines.into_iter().partition(|(id, _)| route(game, id, s3_chyron) == Route::Chyron);
    // Ours (the Skate 2 physics notice): the game's box, else its central message.
    let own = roster.chyron.take();
    let boxed = game == 2 || game == 3 && s3_chyron;
    if let Some([_, body]) = own.as_ref().filter(|_| !boxed) {
        roster.central = Some(body.clone());
    }
    let own = own.filter(|_| boxed);
    if !shout.is_empty() || own.is_some() || state.chyron.is_some() {
        let new = own
            .as_ref()
            .map(|[title, body]| (game, [title.as_str(), body.as_str()]))
            .or_else(|| shout.last().and_then(|(id, _)| Some((game, chyron_of(game, id)?))));
        chyron(state, &mut screens, new, now, &config.asset_root);
    }
    // Another game's HUD now: the log moves to its movie.
    if let Some(open) = state.open.filter(|&l| l != layer_of(game)) {
        screens.layer(open).close(EVENT_LOG);
        state.open = None;
    }
    let layer = screens.layer(layer_of(game));
    // The log outlives the session: "The Host has kicked you out" arrives as we leave.
    let wanted = !lines.is_empty() || !state.log.lines.is_empty() || net.active();
    if state.failed || !wanted {
        state.failed &= wanted;
        if state.open.take().is_some() {
            layer.close(EVENT_LOG);
        }
        return;
    }
    let fail = |state: &mut LogScreen, error: String| {
        warn!("Online event log: {error}");
        state.failed = true;
        state.open = None;
        state.log = EventLog::default();
    };
    let mut changed = state.log.tick(now);
    if layer.get(EVENT_LOG).is_none() {
        if state.open.is_some() {
            // apt_screen closed it after an error (already logged).
            return fail(state, "the screen stopped".into());
        }
        let movie = if game == 1 { "hud_online_eventlog" } else { "hud_online_eventlog2" };
        // Skate 1's and 2's constructors read the log at once and leave Initialise (the rows) to the screen manager.
        let opened = (|| -> Result<(), String> {
            let screen = layer.open(EVENT_LOG, movie, &[("EventLog.GetNumEntries", json!(0)), ("EventLog.GetLogEntries", json!([]))])?;
            if game != 3 {
                screen.call_at(CONTROLLER, "Initialise", &[])?;
            }
            Ok(())
        })();
        if let Err(error) = opened {
            layer.close(EVENT_LOG);
            return fail(state, error);
        }
        state.open = Some(layer_of(game));
        changed = true;
    }
    let screen = layer.get(EVENT_LOG).expect("opened above");
    for (id, arg) in lines {
        // Skate 2's image logs a host change as ID_ONLINE_EVENT_HOST_CHANGE ("{0:s} is the new host.").
        let id = if game == 2 && id == "ID_ONLINE_EVENT_NEWHOST" { "ID_ONLINE_EVENT_HOST_CHANGE" } else { id };
        let language = &screen.bindings.movie.text_assets.language;
        // Skate 1's table has no observer line: its join line.
        let id = if id == "ID_ONLINE_EVENT_JOIN_OBSERVER" && !language.contains_key(id) { "ID_ONLINE_EVENT_JOIN_SKATER" } else { id };
        // Nor a Make Host line: its new-host line.
        let id = if id == "ID_ONLINE_EVENT_ADMIN_TRANSFER" && !language.contains_key(id) { "ID_ONLINE_EVENT_NEWHOST" } else { id };
        let text = format(language, id, &arg);
        // As written: a name word that spells a string id (or a `<B:` button tag) shows as typed.
        state.log.push(crate::apt_text::literal(&text), now);
        changed = true;
    }
    if !changed {
        return;
    }
    let entries = state.log.entries();
    screen.answer("EventLog.GetNumEntries", (entries.len() / 2).into());
    screen.answer("EventLog.GetLogEntries", Json::Array(entries));
    // Skate 1's Refresh (its Update); Skate 2's and 3's SetState 1 = Refresh.
    let refreshed = if game == 1 { screen.call_at(CONTROLLER, "Refresh", &[]) } else { screen.call_at(CONTROLLER, "SetState", &[1.into()]) };
    if let Err(error) = refreshed {
        layer.close(EVENT_LOG);
        fail(state, error);
    }
}

#[derive(Default)]
struct RosterScreen {
    /// The game whose roster movie is loaded.
    game: Option<u8>,
    /// The members and title the movie was last set up with.
    answered: Option<(Vec<Member>, String)>,
    /// The voice statuses its mics last showed (voice chat).
    voip: Vec<i64>,
    visible: bool,
    /// Skate 1's lobby overlay movie open.
    overlay: Option<&'static str>,
    /// The movie failed; not reloaded until the next session or HUD.
    failed: bool,
}

/// The roster, shown throughout a challenge or freeskate lobby (never over the Start menus, nor over a match's own
/// leaderboard; retail shows Online Freeskate's only while Back is held, not Skate 1's), set up again when the members or the title
/// change. Skate 2's and 3's leaderboardhud (`SetState`: 1 Show, 2 Hide, 3 SetupScreen, which ends with RefreshAll)
/// is loaded once per session (a load is 70-90 ms). Skate 1's is retail's eSk8HUDState.FreeSkate online: the session's
/// overlay over onlinehud (skate1_online_hud::lobby_overlay), its rows polled by the movie.
/// UNTRACED (Skate 1): that the Start menus leave the FreeSkate HUD state (the port hides it there, as before), and that
/// a challenge session's overlay is open before its first match (0x82594808 opens it as a mission starts; the
/// FreeSkate state sets it whether open or not).
#[allow(clippy::too_many_arguments)]
fn roster_hud(
    mut state: Local<RosterScreen>,
    roster: Res<OnlineRoster>,
    net: Res<Multiplayer>,
    menu: Res<Menu>,
    map: Res<CurrentMap>,
    m: Res<Match>,
    online1: Option<Res<crate::skate1_online::Skate1Online>>,
    screens: Option<ResMut<Screens>>,
    customiser: Option<Res<crate::customiser::Customiser>>,
) {
    let Some(mut screens) = screens else { return };
    // Rebuilt every frame the roster shows past 8 players.
    for layer in [ONLINE, SKATE2_ONLINE] {
        screens.layer(layer).extra.remove(GRID);
    }
    let state = &mut *state;
    let game = hud_game(Some(&menu), &map);
    if !net.active() || state.game.is_some_and(|g| g != game) {
        if let Some(loaded) = state.game {
            screens.layer(layer_of(loaded)).close(ROSTER);
            screens.layer(layer_of(loaded)).close(LOBBY);
        }
        *state = RosterScreen::default();
    }
    if !net.active() || state.failed {
        return;
    }
    let layer = screens.layer(layer_of(game));
    let lobby = roster.state.challenge.as_ref();
    // ponytail: names only; retail's lobby roster also shows team colours and the host crown (no teams yet).
    // Not over Edit Skater, where it covered the menu.
    let held = !menu.open && m.mode.is_none() && customiser.is_none_or(|c| !c.open);
    let title = lobby.map_or("ID_CROSSBAR_ONLINE_FREESKATE_MODE_TITLE", |c| c.title.as_str());
    let wanted = (roster.members.clone(), title.to_owned());
    let names = crate::player_list::shown_names(&roster.members);
    let local = roster.members.iter().position(|m| m.actor == roster.local_actor).unwrap_or(usize::MAX);
    // The yellow arrow (mTurn): the lobby fill 0x826E7D40 sets it on the host's row (entry+0x3A = id == 0x82743C40).
    // A challenge lobby's fill skips that slot (session flag 6, 0x826E7EFC..0x826E7F20), so the arrow stays there too
    // (user 10-27: hosting a Spot Battle lobby showed none).
    let host = roster.members.iter().position(|m| m.host).unwrap_or(usize::MAX);
    // Each row's voice::voip_status: a change alone redraws the mics (voip_icons).
    let voip: Vec<i64> = roster.members.iter().map(|m| crate::voice::voip(m.actor)).collect();
    let mut update = || -> Result<(), String> {
        if game == 1 {
            // eSk8HUDState.FreeSkate's overlay (skate1_online_hud::lobby_overlay) over onlinehud, which it shows; a
            // lobby on a mode without a Skate 1 overlay shows neither.
            let mode = lobby.and_then(|c| Mode::from_filter(&c.title));
            let overlay = if lobby.is_some() { mode.and_then(|m| skate1_online_hud::lobby_overlay(Some(m))) } else { skate1_online_hud::lobby_overlay(None) };
            let Some((movie, enter, exit)) = overlay else {
                for name in [ROSTER, LOBBY] {
                    layer.close(name);
                }
                (state.game, state.overlay, state.visible) = (Some(1), None, false);
                return Ok(());
            };
            if state.overlay != Some(movie) {
                for (name, movie) in [(ROSTER, "onlinehud"), (LOBBY, movie)] {
                    skate1_online_hud::bind(layer.open(name, movie, &[])?)?;
                }
                (state.game, state.overlay, state.visible) = (Some(1), Some(movie), false);
            }
            let title = online1.as_ref().and_then(|o| o.data.as_deref()).map(|d| skate1_online_hud::lobby_title(d, mode)).unwrap_or_default();
            let answers = skate1_online_hud::lobby_answers(&names, local, &title);
            for name in [ROSTER, LOBBY] {
                let screen = layer.get(name).ok_or("the screen stopped")?;
                for (key, value) in &answers {
                    screen.answer(key, value.clone());
                }
            }
            if held != state.visible {
                let overlay = layer.get(LOBBY).ok_or("the screen stopped")?;
                let set = [json!(if held { enter } else { exit })];
                // The Spot overlay's controller is `spotchal` (as skate1_online_hud's Hud::overlay calls it).
                match overlay.has("SetState") {
                    true => overlay.call("SetState", &set)?,
                    false => overlay.call_at("spotchal", "SetState", &set)?,
                };
                state.visible = held;
            }
            // ponytail: InkHUD's 'onlineobjective' (hudink) isn't converted; OnlineRaceHUD is onlinehud.
            let calls = layer.get(LOBBY).map(Screen::take_calls).unwrap_or_default();
            let online = layer.get(ROSTER).ok_or("the screen stopped")?;
            drop(online.take_calls());
            for (name, args) in calls {
                if name == "OnlineRaceHUD.Show" {
                    online.call("Show", &[args.first().cloned().unwrap_or(json!(false))])?;
                }
            }
            return Ok(());
        }
        // The movie localizes the title when it sets it; a name goes as apt_text::literal, never looked up.
        let answers = if game == 2 {
            skate2_roster_answers(&names, local, title)
        } else {
            // The verified creator's name in the creator tag's colour, as in the Player List.
            let shown: Vec<_> = (names.iter().zip(&roster.members))
                .map(|(n, m)| if crate::creator::is_creator(m.actor) { crate::apt_text::gold(n) } else { crate::apt_text::literal(n) })
                .collect();
            // Skate 3's lobby box reads "Challenge Lobby" in a challenge lobby (it sizes the box; the mode's own,
            // shorter title left long names hanging off it).
            let title = if lobby.is_some() { "ID_FREESKATE_CHALLENGE_LOBBY" } else { title };
            roster_answers(&shown, local, host, title, &voip)
        };
        // Skate 2's first RefreshAll after SetupScreen swaps each row to its 'lobby' frame, a new name field over the one
        // it filled; the game's next refresh (5) fills that.
        let setup: &[i32] = if game == 2 { &[3, 5] } else { &[3] };
        if state.game.is_none() {
            state.game = Some(game);
            let screen = layer.open(ROSTER, "leaderboardhud", &answers)?;
            for &s in setup {
                screen.call("SetState", &[s.into()])?;
            }
            screen.call("SetState", &[2.into()])?;
            (state.answered, state.voip) = (Some(wanted.clone()), voip.clone());
        }
        let screen = layer.get(ROSTER).ok_or("the screen stopped")?;
        // Hidden, a change waits for the next show.
        if held && state.answered.as_ref() != Some(&wanted) {
            for (key, value) in answers {
                screen.answer(key, value);
            }
            for &s in setup {
                screen.call("SetState", &[s.into()])?;
            }
            (state.answered, state.voip) = (Some(wanted.clone()), voip.clone());
        } else if held && state.voip != voip {
            voip_icons(screen, &answers)?;
            state.voip = voip.clone();
        }
        if held != state.visible {
            screen.call("SetState", &[if held { 1 } else { 2 }.into()])?;
            state.visible = held;
        }
        Ok(())
    };
    if let Err(error) = update() {
        warn!("Online roster: {error}");
        layer.close(ROSTER);
        layer.close(LOBBY);
        (state.game, state.failed) = (Some(game), true);
        return;
    }
    if game >= 2 && state.visible && names.len() > MAX_PLAYERS {
        let grid = if game == 3 { roster_grid } else { skate2_roster_grid };
        if let Some(draws) = layer.get(ROSTER).and_then(|screen| grid(screen, &names, local, &voip)) {
            layer.extra.insert(GRID, draws);
        }
    }
}

/// Voice chat (user 10-26): someone started or stopped talking: leaderboardhud's UpdateVOIPIcons (SetState 6, Skate 2's
/// and 3's) redraws the rows' mics alone from `answers`' GetVoipStatus.
pub(crate) fn voip_icons(screen: &mut Screen, answers: &[(&'static str, Json)]) -> Result<(), String> {
    if let Some((key, value)) = answers.iter().find(|(key, _)| *key == "FELeaderboard.GetVoipStatus") {
        screen.answer(key, value.clone());
    }
    screen.call("SetState", &[json!(6)]).map(drop)
}

/// An online match's results while the core holds them up: Skate 2's results screen (throwdownresults for S.K.A.T.E.
/// and 1-Up), or (Skate 1's HUD, or the screen failed) a line per player in the event log. Skate 3's are the modes'.
fn results(
    mut shown: Local<u64>,
    m: Res<Match>,
    mut roster: ResMut<OnlineRoster>,
    menu: Res<Menu>,
    map: Res<CurrentMap>,
    screens: Option<ResMut<Screens>>,
) {
    // A mode that owns its sequencing (Skate 1's) showed its own leaderboard.
    let Some(mode) = m.mode.filter(|_| m.token != 0 && !m.owned && matches!(m.phase, Phase::Results { .. })) else {
        if std::mem::take(&mut *shown) != 0
            && let Some(mut screens) = screens
        {
            screens.layer(SKATE2_ONLINE).close(RESULTS);
        }
        return;
    };
    if std::mem::replace(&mut *shown, m.token) == m.token || m.standings.is_empty() {
        return;
    }
    let game = hud_game(Some(&menu), &map);
    // Skate 3's modes show endchallenge_online themselves (skate3_modes.rs, as their Lua does).
    if game == 3 {
        return;
    }
    if game == 2 {
        let lobby = roster.state.challenge.as_ref();
        let kind = skate2_type_name(lobby.map_or("", |c| c.title.as_str()));
        let title = lobby.and_then(|c| c.row.as_ref()).map_or(kind, |r| r.name.as_str());
        let (movie, answers) = match mode {
            Mode::Skate | Mode::OneUp => ("throwdownresults", throwdown_answers(mode, &m.standings, title)),
            _ => ("roundresults_lb", results_answers(mode, &m.standings, title, kind)),
        };
        match screens.map(|mut s| s.layer(SKATE2_ONLINE).open(RESULTS, movie, &answers).map(drop)) {
            Some(Ok(())) => return,
            Some(Err(error)) => warn!("Online results: {error}"),
            None => {}
        }
    }
    // ponytail: Skate 1's modes are owned (their Lua shows its leaderboard); a non-owned row there, or a Skate 2
    // screen that failed, gets the standings as event-log lines.
    roster.lines.extend(standings_lines(mode, &m.standings).into_iter().map(|l| ("{0:s}", l)));
}

/// Skate 3's rule (user 10-28, s3rules S2 item 11): B on Skate 2's results screen asks "Quit Session?", as Skate 3's
/// results (skate3_modes::online_results: not under a popup, nor the B that just closed one). Every Skate 2 mode's
/// results share this screen. Its movies draw no Quit legend (S2 GAP-1: retail's own end screen is not exported).
fn results_quit(world: &mut World) {
    let open = world.get_resource_mut::<Screens>().is_some_and(|mut s| s.layer(SKATE2_ONLINE).open.contains_key(RESULTS));
    crate::skate3_modes::online_results(world, open);
}

/// The standings as event-log lines, winner first: as many as the log holds (audit 10-24: with 6 players the winner's
/// line was the one pushed out).
fn standings_lines(mode: Mode, standings: &[Standing]) -> Vec<String> {
    standings.iter().take(LINES).enumerate().map(|(rank, s)| format!("{}. {}  {}", rank + 1, s.name, mode.show(s.score))).collect()
}

/// A remote's name tag in each of `passes` (the event log line's style), centred on `at` and sitting on it, and while
/// they talk (voice chat, user 10-26) a lit mic just right of the name, as tall as its text.
fn tag(passes: &[(&Font, [f32; 4], f32, f32)], name: &str, member: &Member, [x, y]: [f32; 2], talking: bool) -> Vec<Draw> {
    let (mut draws, mut right, mut tall) = (Vec::new(), 0f32, 0f32);
    for &(font, colour, dx, height) in passes {
        let (text, mark) = marked(
            &|s| font.width(s, height),
            name,
            member.host,
            crate::player_list::friend(member.actor),
            crate::creator::is_creator(member.actor),
        );
        (right, tall) = (right.max(dx + font.width(&text, height) / 2.), tall.max(height));
        draws.extend(name_tag(font, &text, height, [x + dx, y], tinted(colour, mark)));
    }
    if talking {
        draws.push(crate::graphic_board::button("MicOn", [x + right + MIC_GAP, y - tall], tall));
    }
    draws
}

#[allow(clippy::too_many_arguments)]
fn name_tags(
    roster: Res<OnlineRoster>,
    net: Res<Multiplayer>,
    menu: Res<Menu>,
    map: Res<CurrentMap>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    screens: Option<ResMut<Screens>>,
    remotes: Query<(&RemoteCharacter, &GlobalTransform)>,
    cameras: Query<(&Camera, &GlobalTransform), With<GameplayCamera>>,
) {
    let Some(mut screens) = screens else { return };
    let here = layer_of(hud_game(Some(&menu), &map));
    for other in LAYERS.into_iter().filter(|&l| l != here) {
        screens.layer(other).extra.remove("tags");
    }
    let layer = screens.layer(here);
    let mut draws = Vec::new();
    let passes = layer.open.get(EVENT_LOG).map(|s| tag_passes(&s.bindings.movie)).unwrap_or_default();
    // User 10-30: none over Edit Skater's room, where the others aren't drawn (multiplayer::render::present).
    let tagged = net.active() && !customiser.is_some_and(|c| c.open);
    if let (true, false, Ok((camera, eye))) = (tagged, passes.is_empty(), cameras.single())
        && camera.is_active
        && let Some(size) = camera.logical_viewport_size()
    {
        let shown = crate::player_list::shown_names(&roster.members);
        for (remote, at) in &remotes {
            // C7: a hidden or blocked player has no tag. Q13: any other skater keeps theirs, whatever their look.
            let Some((member, name)) = roster.members.iter().zip(&shown).find(|(m, _)| m.actor == remote.0) else { continue };
            if crate::player_list::ghost(member.actor) {
                continue;
            }
            let head = at.translation() + Vec3::Y * NAME_TAG_HEIGHT;
            if head.distance(eye.translation()) > NAME_TAG_RANGE {
                continue;
            }
            // Err: behind the camera.
            let Ok(p) = camera.world_to_viewport(eye, head) else { continue };
            draws.extend(tag(&passes, name, member, to_stage(p, size).into(), crate::voice::talking(member.actor)));
        }
    }
    if draws.is_empty() {
        layer.extra.remove("tags");
    } else {
        layer.extra.insert("tags", draws);
    }
}

/// Not retail (user 10-11): the chat feed and its typing bar (online_chat) in the event log's own text passes (font,
/// colour, shadow), the sender's name in the roster's name colour (ours in its highlight), on the right edge of the
/// stage, two messages [`CHAT_GAP`] further apart than a message's own rows; the oldest lines that would rise past
/// [`CHAT_TOP`] (under the Skate 2 minimap) are left out. User 10-30: text alone, as the HUD's own; no box (the 10-29
/// box was out of place); in the right middle, clear of the HUD below it; while the bar is closed in a session, a faint
/// [`CHAT_HINT`] where it opens, as nothing else says you can type. The width keeps a 150-character line in its four rows.
/// Knobs.
const CHAT_PAD: f32 = 10.;
const CHAT_LEFT: f32 = 1280. - 16. - CHAT_PAD - CHAT_WIDTH;
const CHAT_BAR_Y: f32 = 460.;
const CHAT_TOP: f32 = 260.;
const CHAT_ROW: f32 = 20.;
const CHAT_GAP: f32 = 8.;
const CHAT_WIDTH: f32 = 420.;
/// The bar's label (drawn as a name, ours) and its caret, blinking at 2 Hz.
const CHAT_SAY: &str = "Say: ";
const CHAT_CARET: &str = "_";
const CHAT_HINT: &str = "Press Enter to chat";
const CHAT_HINT_ALPHA: f32 = 0.5;
/// The hint's text size against the chat's.
const CHAT_HINT_SCALE: f32 = 0.8;
/// `Layer::extra` key of the feed.
const CHAT: &str = "chat";

/// A text pass in `tint` (None: as it is); a shadow pass stays black.
fn tinted(colour: [f32; 4], tint: Option<[u8; 3]>) -> [f32; 4] {
    match tint {
        Some(tint) if colour[..3] != [0., 0., 0.] => {
            let [r, g, b] = tint.map(|c| f32::from(c) / 255.);
            [r, g, b, colour[3]]
        }
        _ => colour,
    }
}
/// A chat line's sender as the feed shows them: ours in the roster's highlight, the host's and a friend's marked; the
/// creator's gold, ours included (user 10-25).
fn chat_name(width: &dyn Fn(&str) -> f32, line: &crate::online_chat::Line) -> (String, [u8; 3]) {
    let (name, mark) = marked(width, &line.name, line.host && !line.local, line.friend && !line.local, line.creator);
    (name, if line.local && !line.creator { ROSTER_LOCAL } else { mark.unwrap_or(ROSTER_NAME) })
}

/// A row of the feed: spans (text, x, colour), its opacity and its baseline.
type ChatRow = (Vec<(String, f32, [u8; 3])>, f32, f32);
/// The feed's rows at `now`, top to bottom, the bar's (or, closed in a session, the hint's) last; empty with nothing to
/// show.
/// `hint`: the hint may show (not in Edit Skater); it shows for HINT_FOR s after joining.
fn chat_rows(width: &dyn Fn(&str) -> f32, chat: &crate::online_chat::Chat, now: f32, hint: bool) -> Vec<ChatRow> {
    let bar = chat.bar.as_ref().map(|bar| {
        let caret = if ((now - chat.opened) * 2.) as i64 % 2 == 0 { CHAT_CARET } else { "" };
        let first = width(CHAT_SAY);
        // The end of a long line, as a text field scrolls.
        let mut text = bar.text.clone();
        while !text.is_empty() && first + width(&format!("{text}{CHAT_CARET}")) > CHAT_WIDTH {
            text.remove(0);
        }
        (vec![(CHAT_SAY.to_owned(), CHAT_LEFT, ROSTER_LOCAL), (format!("{text}{caret}"), CHAT_LEFT + first, CHAT_TEXT)], 1.)
    });
    let mut rows = Vec::new();
    // Newest first, upwards from the bar's row: `y` is the baseline under the next message's last row.
    let mut y = CHAT_BAR_Y - CHAT_ROW - CHAT_GAP;
    for (line, alpha) in chat.visible(now).into_iter().rev() {
        let (name, colour) = chat_name(width, line);
        let name = format!("{name}: ");
        let first = width(&name);
        let wrapped = crate::online_chat::wrap(width, first, &line.text, CHAT_WIDTH);
        let n = wrapped.len();
        if y - n as f32 * CHAT_ROW < CHAT_TOP {
            break;
        }
        for (i, text) in wrapped.into_iter().enumerate().rev() {
            let mut spans = vec![(text, CHAT_LEFT + if i == 0 { first } else { 0. }, CHAT_TEXT)];
            if i == 0 {
                spans.insert(0, (name.clone(), CHAT_LEFT, colour));
            }
            rows.push((spans, alpha, y - (n - 1 - i) as f32 * CHAT_ROW));
        }
        y -= n as f32 * CHAT_ROW + CHAT_GAP;
    }
    rows.reverse();
    rows.extend(bar.map(|(spans, alpha)| (spans, alpha, CHAT_BAR_Y)));
    if hint && chat.live && chat.bar.is_none() && now < chat.hint_until {
        rows.push((vec![(CHAT_HINT.to_owned(), CHAT_LEFT, CHAT_TEXT)], CHAT_HINT_ALPHA, CHAT_BAR_Y));
    }
    rows
}

/// The feed's draws at `now` in `passes` (empty without them): its rows' text, nothing behind it.
fn chat_draws(passes: &[(&Font, [f32; 4], f32, f32)], chat: &crate::online_chat::Chat, now: f32, hint: bool) -> Vec<Draw> {
    let Some(&(font, _, _, height)) = passes.last() else { return Vec::new() };
    let width = |s: &str| font.width(s, height);
    let mut draws = Vec::new();
    for (spans, alpha, y) in &chat_rows(&width, chat, now, hint) {
        for (text, x, tint) in spans {
            let scale = if text == CHAT_HINT { CHAT_HINT_SCALE } else { 1. };
            for &(font, colour, dx, height) in passes {
                let height = height * scale;
                let [r, g, b, a] = tinted(colour, Some(*tint));
                let centre = x + font.width(text, height) / 2. + dx;
                draws.extend(name_tag(font, text, height, [centre, *y], [r, g, b, a * alpha]));
            }
        }
    }
    draws
}

fn chat_feed(
    chat: Res<crate::online_chat::Chat>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    menu: Res<Menu>,
    map: Res<CurrentMap>,
    screens: Option<ResMut<Screens>>,
    time: Res<Time<Real>>,
) {
    let Some(mut screens) = screens else { return };
    let here = layer_of(hud_game(Some(&menu), &map));
    for other in LAYERS.into_iter().filter(|&l| l != here) {
        screens.layer(other).extra.remove(CHAT);
    }
    let layer = screens.layer(here);
    // ponytail: drawn only while the event log's movie is up (it is throughout a session); no movie, no feed.
    let draws = match layer.open.get(EVENT_LOG) {
        Some(log) if chat.live || chat.bar.is_some() || !chat.lines.is_empty() => {
            let hint = customiser.is_none_or(|c| !c.open);
            chat_draws(&tag_passes(&log.bindings.movie), &chat, time.elapsed_secs(), hint)
        }
        _ => Vec::new(),
    };
    if draws.is_empty() {
        layer.extra.remove(CHAT);
    } else {
        layer.extra.insert(CHAT, draws);
    }
}

/// Skate 2's onlineobserving, over its online HUD; the retail string it shows.
const OBSERVING: &str = "5-observing";
const OBSERVING_PLAYER: &str = "ID_OBSERVING_PLAYER";
/// Where the Skate 1 / 3 "Observing" line sits on the 1280x720 stage.
// ponytail: Skate 3's hud2 onlineobserving movie is not on the disc data prepared here (and Skate 1 has none); the line
// is drawn in the event log's name-tag style at the top centre instead.
const OBSERVING_AT: [f32; 2] = [640., 96.];

/// onlineobserving's states: 3 sets the name (Observing_GetPlayerObserved, an apt_text literal) and shows it, 2 hides it.
fn observing_states(screen: &mut Screen, text: Option<&str>) -> Result<(), String> {
    match text {
        Some(text) => {
            screen.answer("HUDComponents.Observing_GetPlayerObserved", json!(crate::apt_text::literal(text)));
            screen.call("SetState", &[json!(3)]).map(drop)
        }
        None => screen.call("SetState", &[json!(2)]).map(drop),
    }
}

/// "Observing: <name>" (retail onlineobserving, OnlineObservingSetObservingPlayerName 0x821933f8) while the camera
/// follows another player's skater: a turn-based mode's waiting players, or a player who joined mid-match.
fn observing(
    mut shown: Local<Option<(u8, String)>>,
    m: Res<Match>,
    menu: Res<Menu>,
    map: Res<CurrentMap>,
    screens: Option<ResMut<Screens>>,
) {
    let Some(mut screens) = screens else { return };
    let watching = m.token != 0 && (m.observer || m.phase != Phase::Idle);
    let name = (m.watch.filter(|&w| watching && (m.observer || w != m.slot)))
        .and_then(|w| m.players.get(w))
        .filter(|n| !n.is_empty())
        .cloned();
    let want = name.map(|n| (layer_of(hud_game(Some(&menu), &map)), n));
    if *shown == want {
        return;
    }
    if let Some((layer, _)) = shown.take() {
        let layer = screens.layer(layer);
        layer.extra.remove("observing");
        if let Some(screen) = layer.get(OBSERVING)
            && let Err(e) = observing_states(screen, None)
        {
            warn!("Online observing: {e}");
            layer.close(OBSERVING);
        }
    }
    let Some((here, name)) = want.clone() else { return };
    let layer = screens.layer(here);
    if here == SKATE2_ONLINE {
        let result = (|| -> Result<(), String> {
            if layer.get(OBSERVING).is_none() {
                layer.open(OBSERVING, "onlineobserving", &[])?;
            }
            let screen = layer.get(OBSERVING).ok_or("the screen stopped")?;
            let text = format(&screen.bindings.movie.text_assets.language, OBSERVING_PLAYER, &name);
            observing_states(screen, Some(&text))
        })();
        if let Err(e) = result {
            warn!("Online observing: {e}");
            layer.close(OBSERVING);
        }
    } else {
        let Some(log) = layer.open.get(EVENT_LOG) else { return };
        let language = &log.bindings.movie.text_assets.language;
        let text = if language.contains_key(OBSERVING_PLAYER) { format(language, OBSERVING_PLAYER, &name) } else { name.clone() };
        let draws: Vec<Draw> = (tag_passes(&log.bindings.movie).into_iter())
            .flat_map(|(font, colour, dx, height)| name_tag(font, &text, height, [OBSERVING_AT[0] + dx, OBSERVING_AT[1]], colour))
            .collect();
        layer.extra.insert("observing", draws);
    }
    *shown = want;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[test]
    fn names_are_capped_and_the_host_and_friends_marked_outside_the_text() {
        let width = |s: &str| s.chars().count() as f32 * 10.;
        assert_eq!(fit(&width, "short", 100., "..."), "short");
        assert_eq!(fit(&width, &"W".repeat(40), 100., "..."), "WWWWWWW...");
        let long = "x".repeat(200);
        assert!(width(&marked(&width, &long, true, false, false).0) <= NAME_MAX + width(" - Host"), "the mark is never cut off");
        assert_eq!(marked(&width, "Ann", true, true, false), ("Ann - Host".into(), Some(HOST_NAME)));
        assert_eq!(marked(&width, "Ann", false, true, false), ("Ann - Friend".into(), Some(FRIEND_NAME)));
        assert_eq!(marked(&width, "Ann - Host", false, false, false), ("Ann - Host".into(), None), "a typed mark keeps the plain colour");
        // User 10-25: the verified creator, all in the tag's colour (a rainbow sweep, 10-26), the host mark after the tag.
        let sweep = |(text, colour): (String, Option<[u8; 3]>)| (text, colour.is_some_and(crate::creator::on_sweep));
        assert_eq!(sweep(marked(&width, "Ann", false, true, true)), ("Ann - Creator".into(), true));
        assert_eq!(sweep(marked(&width, "Ann", true, false, true)), ("Ann - Creator - Host".into(), true));
        assert!(width(&marked(&width, &long, true, false, true).0) <= NAME_MAX + width(" - Creator - Host"), "nor the tag");
        assert!(![HOST_NAME, FRIEND_NAME, ROSTER_NAME, ROSTER_LOCAL].into_iter().any(crate::creator::on_sweep), "no other name sweeps");
        let line = |local, host| crate::online_chat::Line { name: "Ann".into(), local, host, ..default() };
        assert_eq!(chat_name(&width, &line(false, true)).1, HOST_NAME);
        assert_eq!(chat_name(&width, &line(false, false)).1, ROSTER_NAME);
        assert_eq!(chat_name(&width, &line(true, true)), ("Ann".into(), ROSTER_LOCAL), "ours as ever");
        let creator = |local| crate::online_chat::Line { name: "Ann".into(), local, creator: true, ..default() };
        for local in [false, true] {
            let (name, colour) = chat_name(&width, &creator(local));
            assert!(name == "Ann - Creator" && crate::creator::on_sweep(colour), "the creator's own lines too: {colour:?}");
        }
        assert_eq!(tinted([0., 0., 0., 0.5], Some(HOST_NAME)), [0., 0., 0., 0.5], "the shadow stays black");
        assert_ne!(CHAT_TEXT, ROSTER_NAME);
    }

    #[test]
    fn join_and_leave_lines_are_capped_a_minute() {
        let mut churn = Churn::default();
        assert!(churn.allow("Ann", 0.) && churn.allow("Ann", 1.) && !churn.allow("Ann", 2.), "two per name");
        assert!((0..6).all(|i| churn.allow(&format!("p{i}"), 3.)) && !churn.allow("Bo", 4.), "eight in all");
        assert!(churn.allow("Ann", 61.), "a minute on");
        assert!(churns("ID_ONLINE_EVENT_LEAVE") && !churns("ID_ONLINE_EVENT_NEWHOST"));
    }
    #[test]
    fn event_log_states_follow_the_movie() {
        let mut log = EventLog::default();
        log.push("A joined.".into(), 0.);
        assert_eq!(log.entries(), vec![json!("A joined."), json!(0)], "new line plays intro");
        assert_eq!(log.entries(), vec![json!("A joined."), json!(2)], "reported once, then shown");
        assert!(!log.tick(1.));
        assert!(log.tick(SHOW_FOR + 0.01), "outro starts");
        assert_eq!(log.entries(), vec![json!("A joined."), json!(1)]);
        assert!(log.tick(SHOW_FOR + OUTRO_FOR + 0.02), "removed");
        assert!(log.entries().is_empty());
        for i in 0..7 { log.push(format!("l{i}"), 10.); }
        assert_eq!(log.entries().len(), 2 * LINES, "oldest dropped beyond 5");
    }
    /// Item 30: Skate 3 shows a session's end in a popup (TU3 popup rows 64/67/74/104/110), Skate 2 on its chyron;
    /// session events stay log lines, and Skate 1 keeps its log.
    #[test]
    fn session_ends_route_to_retail_popups() {
        for id in ["ID_ONLINE_KICKED_BY_GAME_HOST", "ID_ONLINE_GAME_LEFT"] {
            assert_eq!((route(3, id, false), route(2, id, true), route(1, id, true)), (Route::Popup, Route::Chyron, Route::Log), "{id}");
        }
        for id in ["ID_ONLINE_DISCONNECT_FROM_EA_SERVER", "ID_ONLINE_PLAYNOW_ERROR_STARTING", "ID_JOIN_ONLINE_GAME_FAILED_POPUP_MESSAGE"] {
            assert_eq!((route(3, id, false), route(2, id, true)), (Route::Popup, Route::Log), "{id}");
        }
        for id in ["ID_ONLINE_EVENT_HOSTLEFT", "ID_ONLINE_EVENT_JOIN_SKATER", "ID_ONLINE_EVENT_ADMIN_TRANSFER"] {
            assert_eq!(route(3, id, true), Route::Log, "{id}");
        }
        // Item 30b: with Skate 3's chyron installed, being kicked and losing Steam go to it (0x826E92CC, 0x826FA5B8);
        // the other session ends keep their popups.
        for id in ["ID_ONLINE_KICKED_BY_GAME_HOST", "ID_ONLINE_DISCONNECT_FROM_EA_SERVER"] {
            assert_eq!(route(3, id, true), Route::Chyron, "{id}");
        }
        for id in ["ID_ONLINE_GAME_LEFT", "ID_ONLINE_PLAYNOW_ERROR_STARTING", "ID_JOIN_ONLINE_GAME_FAILED_POPUP_MESSAGE"] {
            assert_eq!(route(3, id, true), Route::Popup, "{id}");
        }
    }

    /// Item 30: the kicker's "Player Kicked from Session!" (0x8262cf80) in the installed hudcentral, with the movie's
    /// own string. SKATE3_ASSET_ROOT = the install's assets.
    #[test]
    #[ignore = "requires the installed screens"]
    fn the_kick_central_message_shows_its_text() {
        let path = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").unwrap()).join(crate::skate3_modes::HUD_DIR).join("hudcentral.json");
        let mut screen = Screen::load(&path, &[]).unwrap();
        central_show(&mut screen, true, "ID_CENTRALMESSAGE_PLAYERKICKED").unwrap();
        for _ in 0..30 {
            screen.advance().unwrap();
        }
        let shown: Vec<String> = (screen.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(shown.iter().any(|t| t.contains("Player Kicked from Session!")), "{shown:?}");
        screen.call("SetState", &[json!(5)]).unwrap();
        for _ in 0..60 {
            screen.advance().unwrap();
        }
        screen.draws().unwrap();
    }
    #[test]
    fn format_fills_the_name() {
        let lang = BTreeMap::from([("ID_ONLINE_EVENT_JOIN_SKATER".to_string(), "{0:s} joined.".to_string())]);
        assert_eq!(format(&lang, "ID_ONLINE_EVENT_JOIN_SKATER", "Ann"), "Ann joined.");
        assert_eq!(format(&lang, "ID_MISSING", "x"), "ID_MISSING");
        let lang = BTreeMap::from([("ID_ONLINE_DISCONNECT_FROM_EA_SERVER".to_string(), "You have lost your connection to EA Nation.".to_string())]);
        assert_eq!(format(&lang, "ID_ONLINE_DISCONNECT_FROM_EA_SERVER", ""), "You have lost your connection to Steam.");
        // Two parts, the second a string id; literal text is filled too.
        let lang = BTreeMap::from([
            ("ID_ONLINE_EVENT_LOCATION_CHANGE".to_string(), "Moving to {0:s}: {1:s}".to_string()),
            ("ID_SPOT".to_string(), "Spot Battle".to_string()),
        ]);
        assert_eq!(format(&lang, "ID_ONLINE_EVENT_LOCATION_CHANGE", "DownTown\u{1f}ID_SPOT"), "Moving to DownTown: Spot Battle");
        assert_eq!(format(&lang, "{0:s} proposed a new location.", "Ann"), "Ann proposed a new location.");
        // A name is never looked up, nor scanned for another part's placeholder.
        let lang = BTreeMap::from([
            ("ID_ONLINE_EVENT_JOIN_SKATER".to_string(), "{0:s} joined.".to_string()),
            ("ID_ONLINE_EVENT_SWITCHED_TEAMS".to_string(), "{0:s} joined {1:s}".to_string()),
            ("ID_ONLINE_KICKED_BY_GAME_HOST".to_string(), "The host kicked you".to_string()),
            ("ID_TEAM".to_string(), "Team 1".to_string()),
        ]);
        assert_eq!(format(&lang, "ID_ONLINE_EVENT_JOIN_SKATER", "ID_ONLINE_KICKED_BY_GAME_HOST"), "ID_ONLINE_KICKED_BY_GAME_HOST joined.");
        assert_eq!(format(&lang, "ID_ONLINE_EVENT_SWITCHED_TEAMS", "{1:s}\u{1f}ID_TEAM"), "{1:s} joined Team 1");
    }
    #[test]
    fn roster_uses_the_names_only_layout() {
        let a = roster_answers(&["Host".into(), "Ann".into()], 1, 0, "Online Freeskate", &[2, 0]);
        let get = |k: &str| a.iter().find(|(key, _)| *key == k).unwrap().1.clone();
        assert_eq!(get("HUDComponents.Leaderboard_GetSetupDataBlock"), json!([2, 0, 11, "ID_COMMON_UNKNOWN", false, true, true]));
        assert_eq!(get("HUDComponents.Leaderboard_GetLeaderboardNameBlock"), json!(["Host", "Ann"]));
        let data = get("HUDComponents.Leaderboard_GetLeaderboardDataBlock");
        assert_eq!(data.as_array().unwrap().len(), 16, "8 slots per player");
        // User 10-27: the yellow arrow (mTurn, slot 4) is the host's (lobby fill 0x826E7D40: entry+0x3A = id == host);
        // our own row is the lit name (slot 3, entry+0x38 = id == local).
        assert_eq!((&data[3], &data[4]), (&json!(false), &json!(true)), "the host's row: the arrow");
        assert_eq!((&data[PLAYER_SLOTS + 3], &data[PLAYER_SLOTS + 4]), (&json!(true), &json!(false)), "ours: the lit name");
        assert_eq!(get("FELeaderboard.GetVoipStatus"), json!([2, 0]), "each row's voice::voip_status");
        let a = roster_answers(&["Host".into(), "Ann".into()], 1, usize::MAX, "t", &[0, 0]);
        let data = a.iter().find(|(key, _)| *key == "HUDComponents.Leaderboard_GetLeaderboardDataBlock").unwrap().1.clone();
        assert!((0..2).all(|i| data[i * PLAYER_SLOTS + 4] == json!(false)), "no host row (a challenge lobby): no arrow");
        // Ten players: 8 blank, unlit rows (roster_grid draws the names).
        let names: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        let a = roster_answers(&names, 9, 0, "t", &[2; 10]);
        let get = |k: &str| a.iter().find(|(key, _)| *key == k).unwrap().1.clone();
        assert_eq!(get("HUDComponents.Leaderboard_GetLeaderboardNameBlock"), json!(vec![""; 8]));
        assert_eq!(get("FELeaderboard.GetVoipStatus"), json!(vec![0; 8]), "the blank rows have no mics (the grid's do)");
        let data = get("HUDComponents.Leaderboard_GetLeaderboardDataBlock");
        assert!((0..8).all(|i| data[i * PLAYER_SLOTS + 3] == json!(false)));
        // Skate 2's: RankName; past 8, the 8 longest names, unlit (skate2_roster_grid draws everyone).
        let mut names = names;
        names[9] = "AVeryLongSteamNameIndeed".into();
        let a = skate2_roster_answers(&names, 1, "t");
        let get = |k: &str| a.iter().find(|(key, _)| *key == k).unwrap().1.clone();
        assert_eq!(get("HUDComponents.Leaderboard_GetLayoutType"), json!(11));
        let shown = get("HUDComponents.Leaderboard_GetLeaderboardNameBlock");
        assert_eq!((shown.as_array().unwrap().len(), &shown[0]), (8, &json!(crate::apt_text::literal("AVeryLongSteamNameIndeed"))));
        let data = get("HUDComponents.Leaderboard_GetLeaderboardDataBlock");
        assert!((0..8).all(|i| data[i * 5 + 3] == json!(false)));
    }

    /// Voice chat: a talking remote's tag has a lit mic just right of the name, on its line and as tall as its text; a
    /// silent one's has none (SKATE3_ASSET_ROOT: in each game's event log style; SKATE_ONLINE_HUD_SHOTS renders).
    #[test]
    fn a_talker_gets_a_mic_by_the_name_tag() {
        let ann = Member { actor: 7, name: "Ann".into(), steam: 0, host: false };
        let mic = "runtime/buttons/MicOn.rgba";
        let mics = |draws: &[Draw]| draws.iter().filter(|d| d.texture == mic).count();
        assert_eq!(mics(&tag(&[], "Ann", &ann, [640., 360.], true)), 1, "a talker's tag");
        assert_eq!(mics(&tag(&[], "Ann", &ann, [640., 360.], false)), 0, "a silent player's");
        let Some((assets, shots)) = assets() else { return };
        for game in [1, 2, 3] {
            let root = assets.join(folder(game));
            let movie = if game == 1 { "hud_online_eventlog" } else { "hud_online_eventlog2" };
            let s = Screen::load(&root.join(format!("runtime/{movie}.json")), &[]).unwrap();
            let passes = tag_passes(&s.bindings.movie);
            let draws = tag(&passes, "Ann", &ann, [640., 360.], true);
            let (lit, text): (Vec<&Draw>, Vec<&Draw>) = draws.iter().partition(|d| d.texture == mic);
            let [l, t, _, b] = bounds(lit[0]);
            let right = text.iter().map(|d| bounds(d)[2]).fold(f32::MIN, f32::max);
            assert!(l > right && l - right < 12., "game {game}: just right of the name ({right} -> {l})");
            let tall = passes.iter().map(|p| p.3).fold(0f32, f32::max);
            assert!((b - 360.).abs() < 0.01 && (b - t - tall).abs() < 0.01, "game {game}: on the tag's line, as tall ({t}..{b})");
            shoot(&draws, &root, shots.as_ref(), &format!("tag-mic{game}.png"));
        }
    }
    /// A draw's stage bounds: [left, top, right, bottom].
    fn bounds(d: &Draw) -> [f32; 4] {
        (d.vertices.iter())
            .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, v| [b[0].min(v.position[0]), b[1].min(v.position[1]), b[2].max(v.position[0]), b[3].max(v.position[1])])
    }

    /// The frame label a clip shows (the last one at or before its frame).
    fn label(s: &Screen, path: &str) -> Option<String> {
        let movie = &s.bindings.movie;
        let instance = movie.instances.get(&s.clip(path)?)?;
        let frames = &movie.characters.get(&instance.character)?.frames;
        (frames.iter().take(instance.frame + 1).rev())
            .find_map(|f| f.controls.iter().find(|c| c.type_name == "frame_label").and_then(|c| c.label.clone()))
    }

    /// Voice chat: the roster's mics as roster_hud drives them (SKATE_ONLINE_UI_ROOT = private/online-ui; Skate 2's in
    /// hud-skate2/online beside it): set up silent, then UpdateVOIPIcons alone (voip_icons: SetState 6) lights a
    /// talker's row (micon) and one with voice who stopped (micoff); past 8, the grid draws the same mics.
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn the_roster_refreshes_when_someone_starts_talking() {
        let root = PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let out = std::env::var("SKATE_ONLINE_UI_SHOTS").ok().map(PathBuf::from);
        let title = "ID_CROSSBAR_ONLINE_FREESKATE_MODE_TITLE";
        let names: Vec<String> = ["#Host", "#Ann", "#Bo"].map(String::from).to_vec();
        let mut s = Screen::load(&root.join("runtime/leaderboardhud.json"), &roster_answers(&names, 0, 0, title, &[0, 0, 0])).unwrap();
        for state in [3, 1] {
            s.call("SetState", &[json!(state)]).unwrap();
        }
        let shown = |s: &Screen| (0..3).map(|row| label(s, &format!("mLeaderboardHUD.player_{row}.voip"))).collect::<Vec<_>>();
        let frames = |labels: [&str; 3]| labels.map(|l| Some(l.to_owned())).to_vec();
        assert_eq!(shown(&s), frames(["invisible"; 3]));
        voip_icons(&mut s, &roster_answers(&names, 0, 0, title, &[1, 2, 0])).unwrap();
        for _ in 0..30 {
            s.advance().unwrap();
        }
        assert_eq!(shown(&s), frames(["micoff", "micon", "invisible"]));
        shoot(&s.draws().unwrap(), &root, out.as_ref(), "roster-voice.png");
        // Past 8: the grid's mics, lit for a talker, silent for one with voice.
        let many: Vec<String> = (0..12).map(|i| format!("Skater{i}")).collect();
        let mut voip = vec![0; 12];
        (voip[3], voip[9]) = (2, 1);
        for (key, value) in roster_answers(&many, 5, 0, title, &voip) {
            s.answer(key, value);
        }
        s.call("SetState", &[json!(3)]).unwrap();
        let grid = roster_grid(&mut s, &many, 5, &voip).expect("leaderboardhud's rows");
        let count = |name: &str| grid.iter().filter(|d| d.texture == format!("runtime/buttons/{name}.rgba")).count();
        assert_eq!((count("MicOn"), count("MicOff")), (1, 1));
        shoot(&s.draws().unwrap().into_iter().chain(grid).collect::<Vec<_>>(), &root, out.as_ref(), "roster-voice12.png");
        // Skate 2: its rows by position, the mic shown or hidden.
        let root2 = root.join("../hud-skate2/online");
        let names2: Vec<String> = ["Voip S2 Host", "Voip S2 Ann"].map(String::from).to_vec();
        crate::voice::hear("Voip S2 Host", 9201, 0);
        crate::voice::hear("Voip S2 Ann", 9202, 0);
        let mut s = Screen::load(&root2.join("runtime/leaderboardhud.json"), &skate2_roster_answers(&names2, 0, title)).unwrap();
        for state in [3, 5, 1] {
            s.call("SetState", &[json!(state)]).unwrap();
        }
        let ann = |s: &Screen| {
            let path = "mcLeaderboardHUD.playerName1.voip";
            (s.vm.get(s.clip(path).expect("Skate 2's row mic"), "_visible").truth(), label(s, path))
        };
        assert!(!ann(&s).0, "silent: hidden");
        crate::voice::hear("Voip S2 Ann", 9202, 2);
        voip_icons(&mut s, &skate2_roster_answers(&names2, 0, title)).unwrap();
        for _ in 0..30 {
            s.advance().unwrap();
        }
        assert_eq!(ann(&s), (true, Some("micon".into())));
        shoot(&s.draws().unwrap(), &root2, out.as_ref(), "roster2-voice.png");
    }

    fn standing(name: &str, score: i64, finished: bool, local: bool) -> Standing {
        Standing { name: name.into(), score, finished, local }
    }

    /// Each mode's results: the layout its Lua (or the closest data-backed one) uses and the cell its layout reads.
    #[test]
    fn results_follow_each_mode() {
        let get = |a: &[(&str, Json)], k: &str| a.iter().find(|(key, _)| *key == k).unwrap().1.clone();
        let race = [standing("Ann", 83_450, true, false), standing("Bo", DNF, true, true), standing("Cy", 0, false, false)];
        let a = results_answers(Mode::DeathRace, &race, "#Fabio Memorial Race", "ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE");
        assert_eq!(get(&a, "HUDComponents.Results_GetLayoutType"), json!(3));
        let data = get(&a, "HUDComponents.Results_GetLeaderboardDataBlock");
        assert_eq!(data.as_array().unwrap().len(), 15, "5 items per player");
        assert_eq!([&data[4], &data[9], &data[14]], [&json!("#1:23.45"), &json!("ID_RACE_DNF"), &json!("ID_RACE_DNF")]);
        assert_eq!(get(&a, "HUDComponents.Results_GetLeaderboardColTitle(2)"), json!("ID_LEADERBOARD_HEADER_TIME"));
        assert_eq!(get(&a, "HUDComponents.Results_GetLeaderboardNameBlock"), json!(["Ann", "Bo", "Cy"].map(crate::apt_text::literal)));
        let skate = [standing("Ann", 0, true, false), standing("Bo", -2, true, true), standing("Cy", -5, true, false)];
        let a = throwdown_answers(Mode::Skate, &skate, "t");
        let data = get(&a, "HUDComponents.Results_GetLeaderboardDataBlock");
        assert_eq!(get(&a, "Mission.GetEndMissionThrowDownType"), json!(2));
        assert_eq!([&data[1], &data[6], &data[11]], [&json!("0"), &json!("2"), &json!("5")]);
        assert_eq!((get(&a, "HUDComponents.Results_GetSelfRank"), get(&a, "Mission.GetWinState")), (json!(2), json!(false)));
        assert_eq!(get(&throwdown_answers(Mode::OneUp, &skate, "t"), "Mission.GetEndMissionThrowDownType"), json!(1));
        let a = results_answers(Mode::BestTrick, &skate, "t", "k");
        assert_eq!(get(&a, "HUDComponents.Results_GetLayoutType"), json!(5));
        assert_eq!(get(&a, "HUDComponents.Results_GetLeaderboardMainTitle"), json!("ID_CHALLENGE_COMMON_CONTEST_STANDINGS"));
        assert_eq!(get(&results_answers(Mode::SpotBattle, &skate, "t", "k"), "HUDComponents.Results_GetLayoutType"), json!(0));
    }

    /// Audit 10-24: the event-log fallback pushed a line per player, winner first, into a log of LINES: with 6 players
    /// the winner's line was the one pushed out.
    #[test]
    fn the_event_log_results_keep_the_winner() {
        let six: Vec<Standing> = (1..=6).map(|n| standing(&format!("P{n}"), 100 - n, true, false)).collect();
        let mut log = EventLog::default();
        for line in standings_lines(Mode::BestTrick, &six) {
            log.push(line, 0.);
        }
        assert!(log.lines[0].text.starts_with("1. P1"), "{:?}", log.lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    }

    /// The installed assets (SKATE3_ASSET_ROOT) and where renders go (SKATE_ONLINE_HUD_SHOTS), if set.
    fn assets() -> Option<(PathBuf, Option<PathBuf>)> {
        let root = PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").ok()?);
        Some((root, std::env::var("SKATE_ONLINE_HUD_SHOTS").ok().map(PathBuf::from)))
    }
    fn folder(game: u8) -> &'static str {
        ["private/hud-skate1/online", "private/hud-skate2/online", "private/online-ui"][usize::from(game) - 1]
    }
    /// roundresults' calls that do nothing in Flash either: results_lb never defines the InitData SetState(5) calls, and
    /// the movie has no mcTitle for ClampString (a main.apt global) to fill.
    pub(crate) fn results_noop(key: &str) -> bool {
        key.ends_with(".InitData") || key == "global.ClampString"
    }
    fn shoot(draws: &[Draw], root: &Path, shots: Option<&PathBuf>, name: &str) {
        if let Some(out) = shots {
            std::fs::create_dir_all(out).unwrap();
            crate::core_menu_runtime::tests::raster_draws(draws, root, &out.join(name));
        }
    }

    /// Each game's event log as event_log drives it (Initialise for Skate 1 / 2, then its refresh) with a joined line,
    /// through its intro, and a name tag in its style.
    #[test]
    fn each_game_event_log_shows_a_line() {
        let Some((assets, shots)) = assets() else { return };
        for game in [1, 2, 3] {
            let root = assets.join(folder(game));
            let movie = if game == 1 { "hud_online_eventlog" } else { "hud_online_eventlog2" };
            let mut s = Screen::load(&root.join(format!("runtime/{movie}.json")), &[
                ("EventLog.GetNumEntries", json!(0)),
                ("EventLog.GetLogEntries", json!([])),
            ])
            .unwrap();
            if game != 3 {
                s.call_at(CONTROLLER, "Initialise", &[]).unwrap();
            }
            let text = format(&s.bindings.movie.text_assets.language, "ID_ONLINE_EVENT_JOIN_SKATER", "Ann");
            assert!(!text.contains("ID_"), "game {game}: {text}");
            s.answer("EventLog.GetNumEntries", json!(1));
            s.answer("EventLog.GetLogEntries", json!([format!("#{text}"), 0]));
            let refreshed = if game == 1 { s.call_at(CONTROLLER, "Refresh", &[]) } else { s.call_at(CONTROLLER, "SetState", &[json!(1)]) };
            refreshed.unwrap();
            for _ in 0..30 {
                s.advance().unwrap();
            }
            let mut draws = s.draws().unwrap();
            assert!(!draws.is_empty(), "game {game}");
            assert!(s.take_calls().iter().any(|(k, _)| k == "EventLog.GetLogEntries"));
            let passes = tag_passes(&s.bindings.movie);
            assert!(!passes.is_empty(), "game {game}: no text style for name tags");
            for (font, colour, dx, height) in passes {
                draws.extend(name_tag(font, "Ann", height, [640. + dx, 360.], colour));
            }
            shoot(&draws, &root, shots.as_ref(), &format!("eventlog{game}.png"));
        }
    }

    /// Item 32: each game's chat feed and typing bar in its event log's text style: two lines (one ours, one wrapped)
    /// above the bar, the names in the roster colours, gone once faded, back with the bar. User 10-30: text alone (no
    /// box, open or closed), and a 150-character line, in the real font, still fits its four rows unshortened.
    #[test]
    fn each_game_chat_feed_draws_in_the_event_log_style() {
        use crate::online_chat::{Chat, Line};
        let Some((assets, shots)) = assets() else { return };
        for game in [1, 2, 3] {
            let root = assets.join(folder(game));
            let movie = if game == 1 { "hud_online_eventlog" } else { "hud_online_eventlog2" };
            let s = Screen::load(&root.join(format!("runtime/{movie}.json")), &[]).unwrap();
            let passes = tag_passes(&s.bindings.movie);
            let mut chat = Chat::default();
            chat.live = true;
            chat.hint_until = 30.;
            assert!(!chat_draws(&passes, &chat, 0., true).is_empty(), "game {game}: in a session, the hint");
            chat.live = false; // the hint aside: the lines and the bar alone below
            assert!(chat_draws(&passes, &chat, 0., true).is_empty(), "game {game}: nothing to show, nothing");
            let line = |name: &str, local, text: &str| Line { name: name.into(), local, text: text.into(), born: 0., ..default() };
            chat.push(line("Ann", false, "anyone up for a game of SKATE at the plaza? meet by the big stairs near the fountain and bring a board"));
            chat.push(line("Me", true, "omw"));
            let feed = chat_draws(&passes, &chat, 1., true);
            let tint = |name: [u8; 3]| feed.iter().any(|d| d.multiply[..3] == name.map(|c| f32::from(c) / 255.));
            assert!(tint(ROSTER_NAME) && tint(ROSTER_LOCAL), "game {game}: the names in the roster colours");
            let lowest = |draws: &[Draw]| draws.iter().flat_map(|d| d.vertices.iter().map(|v| v.position[1])).fold(0f32, f32::max);
            assert!(lowest(&feed) < CHAT_BAR_Y - CHAT_ROW / 2., "game {game}: above the bar's row");
            assert!(chat_draws(&passes, &chat, GONE, true).is_empty(), "game {game}: faded");
            chat.bar = Some(crate::text_entry::TextEntry::new("hello", skate_net::chat::MAX_CHARS));
            let mut open = chat_draws(&passes, &chat, GONE, true);
            assert!(lowest(&open) > CHAT_BAR_Y - CHAT_ROW / 2., "game {game}: the bar, with the lines again");
            let solid = |draws: &[Draw]| draws.iter().filter(|d| d.texture == crate::apt_canvas::SOLID).count();
            assert_eq!((solid(&feed), solid(&open)), (0, 0), "game {game}: no box behind the text");
            let (font, height) = passes.last().map(|p| (p.0, p.3)).unwrap();
            let width = |t: &str| font.width(t, height);
            let long = "meet me at the big gap by the fountain after this round, bring a board and your best tricks, we can film the whole session for a part later on tonight";
            assert_eq!(long.len(), skate_net::chat::MAX_CHARS, "a full line");
            let (name, _) = chat_name(&width, &line("Ann", false, long));
            let rows = crate::online_chat::wrap(&width, width(&format!("{name}: ")), long, CHAT_WIDTH);
            assert!(rows.len() <= crate::online_chat::LINE_ROWS && !rows.last().unwrap().ends_with("..."), "game {game}: {rows:?}");
            let mut draws = s.draws().unwrap();
            draws.append(&mut open);
            shoot(&draws, &root, shots.as_ref(), &format!("chat{game}.png"));
        }
    }
    const GONE: f32 = crate::online_chat::SHOW_FOR + crate::online_chat::FADE_FOR + 1.;

    /// User 10-29: the feed on the right edge; a message's rows CHAT_ROW apart and two messages CHAT_GAP further; the
    /// oldest lines left out rather than rising past CHAT_TOP. User 10-30: in the right middle; in a session a faint
    /// "Press Enter to chat" where the bar opens, which takes its place while open, under the lines, which stay put.
    #[test]
    fn the_chat_feed_spaces_messages_and_sits_on_the_right() {
        use crate::online_chat::{Chat, Line};
        let width = |s: &str| s.chars().count() as f32 * 10.;
        let texts = |rows: &[ChatRow]| rows.iter().map(|(spans, _, y)| (spans.iter().map(|s| s.0.as_str()).collect::<String>(), *y)).collect::<Vec<_>>();
        assert!(chat_rows(&width, &Chat::default(), 0., true).is_empty(), "out of a session: nothing");
        let mut chat = Chat::default();
        chat.live = true;
        chat.hint_until = 30.;
        let hint = chat_rows(&width, &chat, 0., true);
        assert_eq!(texts(&hint), [(CHAT_HINT.to_owned(), CHAT_BAR_Y)], "in a session with nothing to show: the hint");
        assert!(hint[0].1 < 1. && CHAT_BAR_Y < 720. * 0.7 && CHAT_TOP > 720. * 0.3, "faint, and in the right middle");
        assert!(chat_rows(&width, &chat, 30.5, true).is_empty(), "the hint lasts HINT_FOR s after joining");
        assert!(chat_rows(&width, &chat, 0., false).is_empty(), "and never over Edit Skater");
        let line = |text: &str| Line { name: "Ann".into(), text: text.into(), ..default() };
        chat.push(line("hi"));
        chat.push(line(&"word ".repeat(12)));
        let rows = chat_rows(&width, &chat, 1., true);
        let ys: Vec<f32> = rows.iter().map(|r| r.2).collect();
        assert_eq!(ys.len(), 4, "hi, two rows of the long one and the hint: {:?}", texts(&rows));
        assert_eq!([ys[2] - ys[1], ys[1] - ys[0]], [CHAT_ROW, CHAT_ROW + CHAT_GAP]);
        assert_eq!(ys[2], CHAT_BAR_Y - CHAT_ROW - CHAT_GAP, "the newest just above the bar's row");
        let xs = || rows.iter().flat_map(|r| r.0.iter().map(|(text, x, _)| (*x, *x + width(text))));
        assert!(xs().all(|(x0, x1)| x0 >= CHAT_LEFT && x1 <= CHAT_LEFT + CHAT_WIDTH && CHAT_LEFT + CHAT_WIDTH + CHAT_PAD == 1280. - 16.));
        chat.bar = Some(crate::text_entry::TextEntry::new("yo", skate_net::chat::MAX_CHARS));
        let open = chat_rows(&width, &chat, 1., true);
        assert_eq!(texts(&open).last(), Some(&("Say: yo_".to_owned(), CHAT_BAR_Y)));
        assert_eq!(open.len(), 4, "the bar in the hint's place: {:?}", texts(&open));
        assert_eq!(texts(&open[..3]), texts(&rows[..3]), "the lines don't move when the bar opens");
        for _ in 0..crate::online_chat::FEED {
            chat.push(line(&"x".repeat(skate_net::chat::MAX_CHARS)));
        }
        let rows = chat_rows(&width, &chat, 1., true);
        assert!(rows[0].2 - CHAT_ROW >= CHAT_TOP, "the top stays under the minimap: {:?}", texts(&rows));
        assert!(rows.len() < crate::online_chat::FEED * crate::online_chat::LINE_ROWS, "the oldest left out");
    }

    /// I2: Skate 2's onlineobserving as `observing` drives it: SetState 3 sets the name (and shows it), 2 hides it.
    #[test]
    fn skate2_observing_shows_the_watched_name() {
        let Some((assets, shots)) = assets() else { return };
        let root = assets.join(folder(2));
        let mut s = Screen::load(&root.join("runtime/onlineobserving.json"), &[]).unwrap();
        let text = format(&s.bindings.movie.text_assets.language, OBSERVING_PLAYER, "Ann");
        assert_eq!(text, "Observing: Ann");
        observing_states(&mut s, Some(&text)).unwrap();
        for _ in 0..30 {
            s.advance().unwrap();
        }
        let draws = s.draws().unwrap();
        assert_eq!((draws.len(), shown_texts(&s)), (1, vec![text.clone()]));
        shoot(&draws, &root, shots.as_ref(), "observing2.png");
        observing_states(&mut s, None).unwrap();
        for _ in 0..30 {
            s.advance().unwrap();
        }
        assert!(s.draws().unwrap().is_empty(), "hidden: {:?}", shown_texts(&s));
    }

    fn shown_texts(s: &Screen) -> Vec<String> {
        (s.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(Value::Text(t)) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// Skate 2's and Skate 1's rosters as roster_hud drives them.
    #[test]
    fn skate2_and_skate1_rosters_list_the_names() {
        let Some((assets, shots)) = assets() else { return };
        let names: Vec<String> = ["Host", "Ann", "Bo"].map(String::from).into();
        let root = assets.join(folder(2));
        let mut s = Screen::load(&root.join("runtime/leaderboardhud.json"), &skate2_roster_answers(&names, 1, "ID_CROSSBAR_ONLINE_FREESKATE_MODE_TITLE")).unwrap();
        for state in [3, 5, 2, 1] {
            s.call("SetState", &[json!(state)]).unwrap();
        }
        for _ in 0..60 {
            s.advance().unwrap();
        }
        assert!(!s.draws().unwrap().is_empty());
        assert!(s.bindings.unanswered.is_empty(), "leaderboardhud asks {:?}", s.bindings.unanswered);
        let named = |row: usize| s.vm.get(s.clip(&format!("mcLeaderboardHUD.playerName{row}.mLeaderboardData.name_txt")).unwrap(), "_displayText").text();
        assert_eq!([named(0), named(1), named(2)], ["Host", "Ann", "Bo"]);
        shoot(&s.draws().unwrap(), &root, shots.as_ref(), "roster2.png");
        // Skate 1's Online Freeskate: its overlay's kStateActive shows onlinehud's rows.
        let root = assets.join(folder(1));
        let load = |movie: &str| {
            let mut s = Screen::load(&root.join(format!("runtime/{movie}.json")), &[]).unwrap();
            skate1_online_hud::bind(&mut s).unwrap();
            for (key, value) in skate1_online_hud::lobby_answers(&names, 1, "Freeskate") {
                s.answer(&key, value);
            }
            s
        };
        let (mut overlay, mut s) = (load("hud_online_freeskate_overlay"), load("onlinehud"));
        overlay.call("SetState", &[json!(1)]).unwrap();
        assert!(overlay.take_calls().iter().any(|(c, a)| c == "OnlineRaceHUD.Show" && a == &[json!(true)]));
        s.call("Show", &[json!(true)]).unwrap();
        for _ in 0..30 {
            s.advance().unwrap();
        }
        let text = |path: &str| s.vm.get(s.clip(&format!("mcOverlay.raceStatus.{path}.labelText")).unwrap(), "text").text();
        assert_eq!(text("mcTextTitle"), "#Freeskate");
        assert_eq!([text("slot1.racerinfoposition"), text("slot1.racerinfoname"), text("slot3.racerinfoname")], ["#1", "#Host", "#Bo"]);
        assert!(!s.draws().unwrap().is_empty());
        shoot(&s.draws().unwrap(), &root, shots.as_ref(), "roster1.png");
    }

    /// Skate 2's roster past its 8 rows as roster_hud draws it: everyone in columns inside the movie's 8-row box (its
    /// strips' extent, 24 px a row), no two names overlapping, the movie's own strips and rows hidden. 8 is retail's.
    #[test]
    fn skate2_roster_grid_fits_the_box() {
        let Some((assets, shots)) = assets() else { return };
        let root = assets.join(folder(2));
        for count in [8_usize, 10, 16, 17, 25, 50] {
            // We are the sixth; one long name is cut.
            let mut names: Vec<String> = (0..count).map(|i| format!("Skater{i}")).collect();
            names[1] = "AVeryLongSteamNameIndeed".into();
            let title = "ID_CROSSBAR_ONLINE_FREESKATE_MODE_TITLE";
            let mut s = Screen::load(&root.join("runtime/leaderboardhud.json"), &skate2_roster_answers(&names, 5, title)).unwrap();
            for state in [3, 5, 2, 1] {
                s.call("SetState", &[json!(state)]).unwrap();
            }
            for _ in 0..60 {
                s.advance().unwrap();
            }
            let movie = s.draws().unwrap();
            if count == 8 {
                shoot(&movie, &root, shots.as_ref(), "roster2_8.png");
                continue;
            }
            let grid = skate2_roster_grid(&mut s, &names, 5, &[]).expect("leaderboardhud's strips");
            let rest = s.draws().unwrap();
            assert!(!s.vm.get(s.clip("mcLeaderboardHUD.mcStrip7").unwrap(), "_visible").truth());
            // The box: the strips hiding took away (the first draw it took is mcStrip0's art), 8 rows of 24 px.
            let bounds = |draws: &mut dyn Iterator<Item = &Draw>| {
                (draws.flat_map(|d| &d.vertices))
                    .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, v| [b[0].min(v.position[0]), b[1].min(v.position[1]), b[2].max(v.position[0]), b[3].max(v.position[1])])
            };
            let taken: Vec<&Draw> = movie.iter().filter(|d| !rest.contains(d)).collect();
            let [left, top, right, _] = bounds(&mut taken.iter().copied().filter(|d| d.texture == taken[0].texture));
            let [l, t, r, b] = bounds(&mut grid.iter());
            assert!(l >= left - 0.5 && r <= right + 0.5 && t >= top - 0.5 && b <= top + 8. * 24. + 0.5, "{count}: {:?} outside {:?}", [l, t, r, b], [left, top, right]);
            // One name draw each (name_txt's font has no glow pass), after a strip every other row.
            let rows = count.div_ceil(if count <= 16 { 2 } else { 3 }).max(8);
            assert_eq!(grid.len(), rows.div_ceil(2) + count);
            let boxes: Vec<[f32; 4]> = grid[rows.div_ceil(2)..].iter().map(|d| bounds(&mut std::iter::once(d))).collect();
            for (i, a) in boxes.iter().enumerate() {
                for c in &boxes[i + 1..] {
                    assert!(a[2] <= c[0] || c[2] <= a[0] || a[3] <= c[1] || c[3] <= a[1], "{count}: names overlap {a:?} {c:?}");
                }
            }
            shoot(&rest.into_iter().chain(grid).collect::<Vec<_>>(), &root, shots.as_ref(), &format!("roster2_{count}.png"));
        }
    }

    /// s2fix R2: the Best Trick / Jam lobby type names become ids roundresults_lb's table has; the rest stay.
    #[test]
    fn skate2_results_name_contests_with_their_own_ids() {
        assert_eq!(skate2_type_name("ID_MAP_FILTER_CHALLENGETYPES_BESTTRICK"), "ID_MISSION_TEMPLATE_ONLINE_BEST_TRICK");
        assert_eq!(skate2_type_name("ID_MAP_FILTER_CHALLENGETYPES_JAM"), "ID_MISSION_TEMPLATE_ONLINE_JAM");
        assert_eq!(skate2_type_name("ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE"), "ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE");
    }

    /// Skate 2's results screen for each mode with one, through its intro.
    #[test]
    fn skate2_results_screen_for_each_mode() {
        let Some((assets, shots)) = assets() else { return };
        let root = assets.join(folder(2));
        let scores = [standing("Ann", 1200, true, false), standing("Danny", 900, true, true), standing("Bo", 300, true, false)];
        let race = [standing("Ann", 83_450, true, false), standing("Danny", 91_020, true, true), standing("Bo", DNF, true, false)];
        let letters = [standing("Ann", 0, true, false), standing("Danny", -3, true, true), standing("Bo", -5, true, false)];
        let wins = [standing("Ann", 3, true, false), standing("Danny", 1, true, true), standing("Bo", 0, true, false)];
        for (mode, standings, kind) in [
            (Mode::DeathRace, &race, "ID_MAP_FILTER_CHALLENGETYPES_DEATHRACE"),
            (Mode::SpotBattle, &scores, "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE"),
            (Mode::BestTrick, &scores, "ID_MAP_FILTER_CHALLENGETYPES_STREET"),
            (Mode::HallOfMeat, &scores, "ID_MAP_FILTER_CHALLENGETYPES_HALLOFMEAT"),
            (Mode::Skate, &letters, "ID_MAP_FILTER_CHALLENGETYPES_SKATE"),
            (Mode::OneUp, &wins, "ID_MAP_FILTER_CHALLENGETYPES_1UP"),
        ] {
            let (movie, answers) = match mode {
                Mode::Skate | Mode::OneUp => ("throwdownresults", throwdown_answers(mode, standings, "#Projects Picnic")),
                _ => ("roundresults_lb", results_answers(mode, standings, "#Projects Picnic", kind)),
            };
            let mut s = Screen::load(&root.join(format!("runtime/{movie}.json")), &answers).unwrap();
            for _ in 0..90 {
                s.advance().unwrap();
            }
            assert!(!s.draws().unwrap().is_empty(), "{mode:?}");
            assert!(s.bindings.unanswered.iter().all(|k| results_noop(k)), "{mode:?}: {movie} asks {:?}", s.bindings.unanswered);
            shoot(&s.draws().unwrap(), &root, shots.as_ref(), &format!("results_{}.png", mode.name()));
        }
    }

    /// Skate 2's chyron as `chyron` drives it: kicked, through its intro, then its outro to the empty frame.
    #[test]
    fn skate2_chyron_shows_being_kicked() {
        assert_eq!(chyron_of(2, "ID_ONLINE_EVENT_JOIN_SKATER"), None, "joins stay in the log");
        let Some((assets, shots)) = assets() else { return };
        let root = assets.join(folder(2));
        let mut s = Screen::load(&root.join("runtime/online_chyron.json"), &[]).unwrap();
        let [title, body] = chyron_of(2, "ID_ONLINE_KICKED_BY_GAME_HOST").unwrap();
        let language = &s.bindings.movie.text_assets.language;
        let data = json!([format!("#{}", format(language, title, "")), format!("#{}", format(language, body, ""))]);
        assert_eq!(data[0], json!("#Steam"));
        s.answer("HUDComponents.OnlineChyron_GetData", data);
        s.call("SetState", &[json!(6)]).unwrap();
        for _ in 0..30 {
            s.advance().unwrap();
        }
        assert!(!s.draws().unwrap().is_empty());
        assert!(s.bindings.unanswered.is_empty(), "online_chyron asks {:?}", s.bindings.unanswered);
        shoot(&s.draws().unwrap(), &root, shots.as_ref(), "chyron.png");
        s.call("SetState", &[json!(4)]).unwrap();
        for _ in 0..(CHYRON_OUTRO * 60.) as usize {
            s.advance().unwrap();
        }
        assert!(s.draws().unwrap().is_empty(), "the outro ends on 'none'");
    }

    /// Item 30b: Skate 3's chyron as `chyron` drives it (the online-movies stage's movie): kicked and Steam lost, each
    /// SetState 7 through its intro with Steam's text, then its outro.
    #[test]
    fn skate3_chyron_shows_being_kicked_and_steam_lost() {
        let Some((assets, shots)) = assets() else { return };
        if !assets.join(S3_CHYRON).is_file() {
            eprintln!("online-movies stage not run on {}", assets.display());
            return;
        }
        for (id, shot) in [("ID_ONLINE_KICKED_BY_GAME_HOST", "chyron_s3_kicked.png"), ("ID_ONLINE_DISCONNECT_FROM_EA_SERVER", "chyron_s3_steam.png")] {
            let mut s = Screen::load(&assets.join(S3_CHYRON), &[]).unwrap();
            let [title, body] = chyron_of(3, id).unwrap();
            let language = &s.bindings.movie.text_assets.language;
            let data = json!([format!("#{}", format(language, title, "")), format!("#{}", format(language, body, ""))]);
            let text = data.to_string();
            assert!(!text.contains("#ID_") && !text.contains("EA Nation") && text.contains("Steam"), "{text}");
            s.answer("HUDComponents.OnlineChyron_GetData", data);
            s.call("SetState", &[json!(7)]).unwrap();
            for _ in 0..30 {
                s.advance().unwrap();
            }
            let shown = s.draws().unwrap();
            assert!(!shown.is_empty(), "{id}");
            assert!(s.bindings.unanswered.is_empty(), "online_chyron asks {:?}", s.bindings.unanswered);
            shoot(&shown, &assets.join(folder(3)), shots.as_ref(), shot);
            s.call("SetState", &[json!(4)]).unwrap();
            for _ in 0..(CHYRON_OUTRO * 60.) as usize {
                s.advance().unwrap();
            }
        }
    }

    /// A 21:9 window shows the 16:9 stage centred with extra width (AutoMin), not stretched.
    #[test]
    fn stage_point_follows_auto_min() {
        let size = Vec2::new(2560., 1080.);
        assert_eq!(to_stage(size / 2., size), Vec2::new(640., 360.));
        let k = 720. / 1080.;
        assert_eq!(to_stage(Vec2::ZERO, size), Vec2::new(640. - 1280. * k, 0.));
        assert_eq!(to_stage(Vec2::new(1920., 1080.), Vec2::new(1920., 1080.)), Vec2::new(1280., 720.));
    }

    /// The roster as roster_hud drives it: loaded hidden, then SetupScreen again after a join and Show.
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn roster_movie_lists_the_names() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let title = "ID_CROSSBAR_ONLINE_FREESKATE_MODE_TITLE";
        let answers = roster_answers(&["#Host".into(), "#Ann".into()], 1, 0, title, &[]);
        let mut s = crate::apt_screen::Screen::load(&root.join("runtime/leaderboardhud.json"), &answers).unwrap();
        s.call("SetState", &[json!(3)]).unwrap();
        s.call("SetState", &[json!(2)]).unwrap();
        for _ in 0..30 { s.advance().unwrap(); }
        assert!(s.draws().unwrap().is_empty(), "loaded hidden");
        // '#' keeps a name that starts with '#' literal.
        for (key, value) in roster_answers(&["#Host".into(), "#Ann".into(), "##1 Bo".into()], 1, 0, title, &[]) {
            s.answer(key, value);
        }
        s.call("SetState", &[json!(3)]).unwrap();
        s.call("SetState", &[json!(1)]).unwrap();
        for _ in 0..30 { s.advance().unwrap(); }
        assert!(!s.draws().unwrap().is_empty());
        // User 10-27: the host's row shows the bobbing yellow arrow (mTurn, RefreshAll's slot 4), no other row.
        let turn = |s: &crate::apt_screen::Screen, i: usize| {
            s.vm.get(s.clip(&format!("mLeaderboardHUD.player_{i}.mTurn")).unwrap(), "_visible").truth()
        };
        assert!(turn(&s, 0) && !turn(&s, 1) && !turn(&s, 2), "the arrow on the host's row alone");
        let out = std::env::var("SKATE_ONLINE_UI_SHOTS").ok();
        if let Some(out) = &out {
            crate::core_menu_runtime::tests::raster_draws(&s.draws().unwrap(), &root, &std::path::Path::new(out).join("roster.png"));
        }
        // Past 8: the same box, names in columns. We are the sixth; one long name is cut.
        for count in [12, 20, 50] {
            let mut names: Vec<String> = (0..count).map(|i| format!("Skater{i}")).collect();
            names[1] = "AVeryLongSteamNameIndeed".into();
            for (key, value) in roster_answers(&names, 5, 0, title, &[]) {
                s.answer(key, value);
            }
            s.call("SetState", &[json!(3)]).unwrap();
            for _ in 0..30 { s.advance().unwrap(); }
            let grid = roster_grid(&mut s, &names, 5, &[]).expect("leaderboardhud's rows");
            let backer = s.clip("mLeaderboardHUD.playerBacker_7").unwrap();
            assert!(!s.vm.get(backer, "_visible").truth(), "the movie's stripes give way to the grid's");
            assert_eq!(grid.len(), names.len().div_ceil(if count <= 16 { 2 } else { 3 }).max(8) + count);
            if let Some(out) = &out {
                let draws: Vec<_> = s.draws().unwrap().into_iter().chain(grid).collect();
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(out).join(format!("roster{count}.png")));
            }
        }
        // Back to 3: SetupScreen shows the movie's own rows again.
        for (key, value) in roster_answers(&["#Host".into(), "#Ann".into(), "#Bo".into()], 1, 0, title, &[]) {
            s.answer(key, value);
        }
        s.call("SetState", &[json!(3)]).unwrap();
        let shown = |s: &crate::apt_screen::Screen, i: usize| {
            s.vm.get(s.clip(&format!("mLeaderboardHUD.playerBacker_{i}")).unwrap(), "_visible").truth()
        };
        assert!(shown(&s, 2) && !shown(&s, 3));
    }
}
