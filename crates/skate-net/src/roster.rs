//! Display names and host session state as owner-authenticated application records, and the
//! join/leave/host/kick/privacy events the online event log shows.
use crate::lobby::Session;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PLAYER_KEY: &str = "@player";
pub const SESSION_KEY: &str = "@session";
/// A guest's latest proposal ([`Propose`]); the host decides on it.
pub const PROPOSE_KEY: &str = "@propose";
/// A player's result in the running match ([`Score`]).
pub const SCORE_KEY: &str = "@score";
/// A player's match events ([`Events`]).
pub const EVENTS_KEY: &str = "@ev";
/// A spectator's `@watch`: the actor whose camera it asks to show (8 bytes little-endian), then the one its camera still
/// shows while it waits for that one's samples (8 more), empty when it shows its own. Those players send it everything
/// at the nearest rate, however far or crowded (lobby; user 10-25: the player you watch is always smooth).
pub const WATCH_KEY: &str = "@watch";
/// Not retail (user 10-29): `[1]` while a player's voice chat is on with a microphone open, `[0]` (or nothing) else, so
/// the others show their grey mic before they talk (retail's "has voice", micoff); before it a player showed none until
/// heard, the same as one with voice off.
pub const VOICE_KEY: &str = "@voice";
/// Not retail (user 10-25, the creator tag): this session's challenge, 16 random bytes every player publishes once. The
/// creator's game signs each player's own challenge (CREATOR_KEYS), so a proof seen in one session never passes in another.
pub const NONCE_KEY: &str = "@nonce";
/// The creator's signed answers to the challenges ([`NONCE_KEY`]), 14 players a record (each fits MAX_APP_VALUE): the
/// crypto lives in the game (creator.rs); this layer only carries the bytes.
pub const CREATOR_KEYS: [&str; 4] = ["@cr0", "@cr1", "@cr2", "@cr3"];
/// Players in one match: retail challenges hold 10.
pub const GAME_SLOTS: usize = 10;
pub const NAME_MAX: usize = 32;
pub const KICKED_MAX: usize = 8;
/// A member missing this long has left (Session::migrate rebuilds membership within about 1 s).
pub const LEAVE_GRACE_MS: u64 = 3_000;
/// Members already present this soon after joining are the baseline, not "joined" lines.
pub const BASELINE_MS: u64 = 2_000;
/// At most CHURN_LINES "joined" / "left" lines per CHURN_MS; more is someone cycling accounts, and is not shown.
pub const CHURN_LINES: usize = 20;
pub const CHURN_MS: u64 = 60_000;

pub fn truncate(name: &str, max: usize) -> &str {
    let end = (0..=max.min(name.len()))
        .rev()
        .find(|&i| name.is_char_boundary(i))
        .unwrap_or(0);
    &name[..end]
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub name: String,
    pub steam: u64,
}
impl Player {
    /// u64 SteamID (LE, 0 for Direct) then the UTF-8 name (at most NAME_MAX bytes).
    pub fn encode(&self) -> Vec<u8> {
        let mut b = self.steam.to_le_bytes().to_vec();
        b.extend_from_slice(truncate(&self.name, NAME_MAX).as_bytes());
        b
    }
    pub fn decode(bytes: &[u8]) -> Option<Player> {
        if bytes.len() < 8 || bytes.len() > 8 + NAME_MAX {
            return None;
        }
        let steam = u64::from_le_bytes(bytes[..8].try_into().ok()?);
        Some(Player { name: shown_name(std::str::from_utf8(&bytes[8..]).ok()?), steam })
    }
}

/// A name as the HUD and lists may show it (a peer's `@player`, a Steam friend's or a recent player's name): no control
/// characters (newlines) or blank names; C4: chat's allow-list too (no invisible, bidi or private-use characters), and
/// no reserved names, which read "The Legend".
pub fn shown_name(name: &str) -> String {
    let name: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let name = crate::chat::scrub(&name);
    match name.trim() {
        n if !crate::chat::visible(n) || reserved(n) => "The Legend".to_owned(),
        trimmed => trimmed.to_owned(),
    }
}

/// A name that reads as the game or its staff ("Host", "SERVER", "H0st has disconnected.", "Ｓｋａｔｅ ３"): its first
/// word, or the whole name, folded (NFKC, so mathematical, circled and full-width letters read as plain ones; case;
/// small capitals; common Cyrillic/Greek lookalikes and digits). User 10-25: nor a name that fakes one of the marks
/// the game puts after a name: "Creator" anywhere in it (leetspeak too), or "Host", "Co-Host" or "Friend" as any word
/// ("Ann - Host"). The real marks are drawn in their own colours, and a name only ever in the plain one.
// ponytail: a hand-picked lookalike map, not TR39 skeletons; the colour is what no name can copy.
fn reserved(name: &str) -> bool {
    const WORDS: [&str; 9] = ["host", "server", "admin", "administrator", "system", "moderator", "skate3", "official", "notice"];
    const MARKS: [&str; 3] = ["host", "cohost", "friend"];
    let name: String = icu_normalizer::ComposingNormalizerBorrowed::new_nfkc().normalize(name).into_owned();
    let fold = |c: char| -> char {
        let c = char::from_u32(match c as u32 { n @ 0xFF01..=0xFF5E => n - 0xFEE0, n => n }).unwrap_or(c);
        match c.to_lowercase().next().unwrap_or(c) {
            'а' | 'α' | 'ᴀ' => 'a', 'е' | 'ε' | 'ᴇ' => 'e', 'о' | 'ο' | '0' | 'ᴏ' => 'o', 'р' | 'ρ' | 'ᴘ' => 'p',
            'с' | 'ϲ' | 'ᴄ' => 'c', 'у' | 'γ' | 'ʏ' => 'y', 'х' | 'χ' => 'x',
            'і' | 'ι' | 'ӏ' | 'l' | '1' | '|' | '!' | 'ɪ' | 'ʟ' => 'i', 'ѕ' | '5' | '$' | 'ꜱ' => 's',
            'ј' | 'ᴊ' => 'j', 'ԁ' | 'ᴅ' => 'd', 'һ' | 'н' | 'η' | 'ʜ' => 'h', 'к' | 'κ' | 'ᴋ' => 'k', 'м' | 'μ' | 'ᴍ' => 'm',
            'т' | 'τ' | 'ᴛ' => 't', 'ν' | 'ᴠ' => 'v', 'в' | 'β' | 'ʙ' => 'b', 'п' => 'n', 'ʀ' => 'r', 'ɴ' => 'n', 'ᴜ' => 'u',
            'ᴡ' => 'w', 'ᴢ' => 'z', 'ɢ' => 'g', 'ғ' => 'f', c => c,
        }
    };
    let skeleton = |s: &str| s.chars().map(fold).filter(|c| c.is_alphanumeric()).collect::<String>();
    let (first, whole) = (skeleton(name.split_whitespace().next().unwrap_or("")), skeleton(&name));
    let leet = whole.replace('3', "e").replace('4', "a").replace('7', "t");
    let words = name.split(|c: char| c.is_whitespace() || c.is_ascii_punctuation() && c != '$' && c != '!' && c != '|');
    WORDS.iter().map(|w| skeleton(w)).any(|w| first == w || whole == w || (w == skeleton("skate3") && whole.starts_with(&w)))
        || leet.contains("creator")
        || words.map(skeleton).any(|w| MARKS.contains(&w.as_str()))
}
/// One kick. `seq` is unique and increasing, so a repeat kick of the same actor or the same
/// display name is still a new entry. `steam` (0 in Direct sessions) lets a new host re-ban.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kick {
    #[serde(rename = "q")]
    pub seq: u64,
    #[serde(rename = "a")]
    pub actor: u64,
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "s")]
    pub steam: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub private: bool,
    pub kicked: Vec<Kick>,
    /// The challenge lobby the host created or matched from the Online Challenge Map; `None` is
    /// Online Freeskate. Older peers omit it.
    #[serde(default, rename = "c", skip_serializing_if = "Option::is_none")]
    pub challenge: Option<ChallengeLobby>,
    /// User 10-29: the Online Freeskate location row the host picked (its string id), so a player who joins by
    /// invite, Join Game or the server browser spawns on their own slot there, not on the map's single-player spawn.
    /// Older peers omit it.
    #[serde(default, rename = "loc", skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The host's difficulty index (retail "All players in Session use this Difficulty"). Older peers omit it.
    #[serde(default, rename = "d", skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u8>,
    /// The host's physics (user 10-28; spec 2026-10-20 P4 / section 7): 1 Skate 2 for everyone in the session, sent
    /// only on New San Vanelona; omitted is Skate 3 (older peers omit it too).
    #[serde(default, rename = "ph", skip_serializing_if = "Option::is_none")]
    pub physics: Option<u8>,
    /// Host Actions > Disable Challenge Proposals.
    #[serde(default, rename = "np", skip_serializing_if = "std::ops::Not::not")]
    pub no_proposals: bool,
    /// Not retail (user 10-25): Host Actions > Player Collisions. Off by default; older peers omit it (off).
    #[serde(default, rename = "pc", skip_serializing_if = "std::ops::Not::not")]
    pub collisions: bool,
    /// A guest's proposal the host is deciding on.
    #[serde(default, rename = "p", skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
    /// The session is moving: everyone travels to the target and joins the host's new lobby (tagged with `token`).
    #[serde(default, rename = "mv", skip_serializing_if = "Option::is_none")]
    pub moving: Option<Move>,
    /// The match the host started on the lobby's row (Start Challenge).
    #[serde(default, rename = "gm", skip_serializing_if = "Option::is_none")]
    pub game: Option<Game>,
    /// The host's weather and time of day for everyone. Older peers omit it (guests then show Clear and Day).
    #[serde(default, rename = "w", skip_serializing_if = "Option::is_none")]
    pub world: Option<World>,
    /// Actors the host made co-host: they move the session, kick and set the world through the host ([`CoHost`]).
    #[serde(default, rename = "co", skip_serializing_if = "Vec::is_empty")]
    pub cohosts: Vec<u64>,
    /// A team lobby's two teams (decision T1: the host assigns teams). A player in neither is not assigned yet (and
    /// counts as the first team).
    #[serde(default, rename = "ta", skip_serializing_if = "Vec::is_empty")]
    pub team_a: Vec<u64>,
    #[serde(default, rename = "tb", skip_serializing_if = "Vec::is_empty")]
    pub team_b: Vec<u64>,
    /// A guest's Switch Team proposal the host is deciding on.
    #[serde(default, rename = "sw", skip_serializing_if = "Option::is_none")]
    pub switching: Option<u64>,
    /// Host only, never sent: the co-hosts' real endpoints, so they are co-hosts again in a moved session's lobby
    /// (actor ids are new there).
    #[serde(skip)]
    pub cohost_endpoints: Vec<u64>,
}
/// A co-host's latest request ([`CoHost`]); the host acts on it only if the sender is a co-host.
pub const COHOST_KEY: &str = "@cohost";
/// Co-hosts at once: keeps `@session` within one application value (a knob).
pub const COHOSTS_MAX: usize = 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoHostAsk {
    /// Kick this actor (never the host or another co-host).
    #[serde(rename = "k")]
    Kick(u64),
    /// Settings > World: weather and time of day indices.
    #[serde(rename = "w")]
    World(u8, u8),
}
/// `nonce` makes a repeat of the same request a new record.
// ponytail: one record per co-host, so a second request within a network tick replaces the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoHost {
    #[serde(rename = "k")]
    pub nonce: u64,
    #[serde(rename = "r")]
    pub ask: CoHostAsk,
}
impl CoHost {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("co-host request")
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}
/// Settings > World as the host set it for the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct World {
    /// The game's Weather index (0 Clear).
    #[serde(rename = "w")]
    pub weather: u8,
    /// The game's Time of Day index (0 Day).
    #[serde(rename = "t")]
    pub time: u8,
    /// The host's clock, minutes since midnight, when sent; guests in a cycle follow it.
    #[serde(rename = "m")]
    pub minute: u16,
}
/// A match: `mode` is the game's mode name, `slots` the players in it in spawn-slot order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    #[serde(rename = "k")]
    pub token: u64,
    #[serde(rename = "o")]
    pub mode: String,
    #[serde(rename = "s")]
    pub slots: Vec<u64>,
    /// The run is over: everyone shows the results.
    #[serde(default, rename = "r", skip_serializing_if = "std::ops::Not::not")]
    pub results: bool,
    /// A team match: each slot's team (1 or 2, Online.GetTeam's numbering); empty when not a team match.
    #[serde(default, rename = "t", skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<u8>,
}
/// A player's `@score` in match `token`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Score {
    #[serde(rename = "k")]
    pub token: u64,
    #[serde(rename = "v")]
    pub value: i64,
    #[serde(rename = "f")]
    pub finished: bool,
    /// The round sync points (Online.OnlineChallengeRoundSyncPoint) this player's mode has reached.
    #[serde(default, rename = "r", skip_serializing_if = "is_zero")]
    pub round: u32,
    /// Per other player (actor): the first of its `@ev` events this player has not taken yet. A sender keeps every
    /// event until each player still in the match has taken it ([`Events`]).
    #[serde(default, rename = "t", skip_serializing_if = "Vec::is_empty")]
    pub taken: Vec<(u64, u32)>,
    /// This player's mode stopped on an error: the others take it as having left the match (it stays in the session).
    #[serde(default, rename = "q", skip_serializing_if = "std::ops::Not::not")]
    pub quit: bool,
}
fn is_zero(n: &u32) -> bool {
    *n == 0
}
impl Score {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("score")
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}
/// A player's match events not yet taken by everyone (a mode's own JSON: a S.K.A.T.E. attempt, a vote), oldest
/// first; `first` numbers `items[0]`, so readers take each event once, in order. The sender drops an event only once
/// every other player's [`Score::taken`] is past it, so none is lost (retail session messages are reliable).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Events {
    #[serde(rename = "k")]
    pub token: u64,
    #[serde(rename = "f")]
    pub first: u32,
    #[serde(rename = "i")]
    pub items: Vec<String>,
}
impl Events {
    /// As many events as fit one app record, oldest first: the newer ones wait for the next record (after the
    /// readers' acknowledgements let the oldest go), so `first` always numbers what was sent.
    pub fn encode(&self) -> Vec<u8> {
        self.fitted().1
    }
    /// How many of `items` fit one record, and that record.
    pub fn fitted(&self) -> (usize, Vec<u8>) {
        let mut out = self.clone();
        let mut bytes = serde_json::to_vec(&out).expect("events");
        while bytes.len() > crate::lobby::MAX_APP_VALUE && !out.items.is_empty() {
            out.items.pop();
            bytes = serde_json::to_vec(&out).expect("events");
        }
        (out.items.len(), bytes)
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}
/// A player's `@ready`: whether it can play the lobby's row `row` (its mode's data and the row's map are here). The
/// host starts a match with the players that can; one that cannot never joins it (retail: a player who cannot load a
/// challenge is not in it), and the host waits for a player that has not answered for this row yet.
pub const READY_KEY: &str = "@ready";
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    #[serde(rename = "r")]
    pub row: String,
    #[serde(rename = "ok")]
    pub ok: bool,
}
impl Ready {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("ready")
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(bytes).ok().filter(|r| r.row.len() <= TITLE_MAX)
    }
}
/// An Online Challenge Map row: `tab` and `id` (a challenge id or a location's string id) find it again on each
/// player's own map; `map` and `name` (a string id) are for the event log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    #[serde(rename = "t")]
    pub tab: u8,
    #[serde(rename = "i")]
    pub id: String,
    #[serde(rename = "m")]
    pub map: String,
    #[serde(rename = "n")]
    pub name: String,
}
impl Target {
    /// Each text cut to TITLE_MAX bytes.
    pub fn new(tab: usize, id: &str, map: &str, name: &str) -> Self {
        let cut = |s: &str| truncate(s, TITLE_MAX).to_owned();
        Target { tab: tab.min(u8::MAX as usize) as u8, id: cut(id), map: cut(map), name: cut(name) }
    }
    fn valid(&self) -> bool {
        [&self.id, &self.map, &self.name].iter().all(|s| text(s, TITLE_MAX))
    }
}
/// The open proposal everyone votes on (retail SignUp_Poster). `token` is the proposer's `@propose` nonce: an
/// `@accept` names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    #[serde(rename = "b")]
    pub by: String,
    #[serde(rename = "a")]
    pub actor: u64,
    #[serde(rename = "g")]
    pub target: Target,
    #[serde(rename = "k")]
    pub token: u64,
}
/// A player's accept of the open proposal (retail session message subtype 1, TU3 0x82dd0b30: the host relays it).
pub const ACCEPT_KEY: &str = "@accept";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accept {
    #[serde(rename = "k")]
    pub token: u64,
}
impl Accept {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("accept")
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}
/// A guest's latest Switch Team proposal (retail ID_ONLINE_HUB_SWITCH_TEAM); `nonce` makes a repeat a new record. The
/// host has the final say.
pub const SWITCH_KEY: &str = "@switch";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    #[serde(rename = "k")]
    pub nonce: u64,
}
impl Switch {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("switch")
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    #[serde(rename = "k")]
    pub token: u64,
    #[serde(rename = "g")]
    pub target: Target,
}
/// A guest's `@propose` record. `nonce` makes a repeat of the same target a new record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Propose {
    #[serde(rename = "k")]
    pub nonce: u64,
    #[serde(rename = "g")]
    pub target: Target,
}
impl Propose {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("proposal")
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(bytes).ok().filter(|p| p.target.valid())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeLobby {
    /// String ID shown as the roster title (the challenge type, e.g. Spot Battle).
    #[serde(rename = "t")]
    pub title: String,
    /// Picked from the Team tab: "Online Team Challenge".
    #[serde(rename = "m")]
    pub team: bool,
    /// The challenge row the lobby is on (tab and challenge id only), the row a match is played on.
    #[serde(default, rename = "r", skip_serializing_if = "Option::is_none")]
    pub row: Option<Target>,
}
pub const TITLE_MAX: usize = 64;
/// A host's (or proposer's) string as received: at most `max` bytes and nothing chat's allow-list drops, so no line
/// breaks or the `\u{1f}` the move log splits on reach a log or the screen.
fn text(s: &str, max: usize) -> bool {
    s.len() <= max && crate::chat::scrub(s) == s
}
impl SessionState {
    /// The oldest kicks give way until the record fits one application value (a proposal and a move can be in it).
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(self).expect("session state");
        let mut trimmed = self.clone();
        while bytes.len() > crate::lobby::MAX_APP_VALUE && !trimmed.kicked.is_empty() {
            trimmed.kicked.remove(0);
            bytes = serde_json::to_vec(&trimmed).expect("session state");
        }
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let s: Self = serde_json::from_slice(bytes).ok()?;
        (s.kicked.len() <= KICKED_MAX
            && s.kicked.iter().all(|k| text(&k.name, NAME_MAX))
            && s.challenge.as_ref().is_none_or(|c| text(&c.title, TITLE_MAX) && c.row.as_ref().is_none_or(Target::valid))
            && s.location.as_deref().is_none_or(|l| text(l, TITLE_MAX))
            && s.proposal.as_ref().is_none_or(|p| text(&p.by, NAME_MAX) && p.target.valid())
            && s.moving.as_ref().is_none_or(|m| m.target.valid())
            && s.game.as_ref().is_none_or(|g| text(&g.mode, TITLE_MAX) && g.slots.len() <= GAME_SLOTS)
            && s.cohosts.len() <= COHOSTS_MAX
            && s.team_a.len() + s.team_b.len() <= GAME_SLOTS
            && s.game.as_ref().is_none_or(|g| g.teams.is_empty() || g.teams.len() == g.slots.len()))
            .then_some(s)
    }
    /// A player's team in a team lobby: 1 or 2 (Online.GetTeam's numbering).
    pub fn team(&self, actor: u64) -> u8 {
        if self.team_b.contains(&actor) { 2 } else { 1 }
    }
    pub fn is_cohost(&self, actor: u64) -> bool {
        self.cohosts.contains(&actor)
    }
    /// `steam` must be the endpoint the host actually banned (`Session::kick`'s result in a Steam
    /// session, else 0), never the kicked player's own `@player` value: use `roster::kick`.
    pub fn record_kick(&mut self, actor: u64, name: &str, steam: u64) {
        let seq = self.kicked.iter().map(|k| k.seq).max().unwrap_or(0).saturating_add(1); // ponytail: a host-sent u64::MAX seq repeats MAX (not a panic in dev)
        self.kicked.push(Kick {
            seq,
            actor,
            name: truncate(name, NAME_MAX).to_owned(),
            steam,
        });
        let over = self.kicked.len().saturating_sub(KICKED_MAX);
        self.kicked.drain(..over);
    }
}
/// Host only: kick `actor` and record it in `state` (the caller republishes `@session`).
/// The recorded SteamID is the endpoint `Session::kick` actually banned, so a spoofed `@player`
/// steam value can neither dodge nor frame anyone. `steam_endpoints` is true for Steam
/// sessions, where endpoints are SteamIDs; Direct endpoints record 0. `None` if nothing was kicked.
pub fn kick(
    session: &mut Session,
    state: &mut SessionState,
    actor: u64,
    steam_endpoints: bool,
) -> Option<String> {
    let name = members(session)
        .into_iter()
        .find(|m| m.actor == actor)
        .map(|m| m.name)
        .unwrap_or_default();
    let endpoint = session.kick(actor)?;
    state.record_kick(actor, &name, if steam_endpoints { endpoint } else { 0 });
    Some(name)
}
/// A new host re-bans the Steam endpoints kicked earlier in the session (Steam endpoints are
/// SteamIDs; Direct kicks carry steam 0 and cannot be carried over).
pub fn seed_bans(session: &mut Session, state: &SessionState) {
    for k in state.kicked.iter().filter(|k| k.steam != 0) {
        session.ban(k.steam);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub actor: u64,
    pub name: String,
    pub steam: u64,
    pub host: bool,
}

/// Host first, then by actor id. Actors without a valid `@player` record are skipped.
pub fn members(session: &Session) -> Vec<Member> {
    let host = session.host_actor();
    let mut out: Vec<Member> = session
        .actors
        .iter()
        .filter_map(|(&actor, a)| {
            let p = Player::decode(&a.application.get(PLAYER_KEY)?.value)?;
            Some(Member {
                actor,
                name: p.name,
                steam: p.steam,
                host: Some(actor) == host,
            })
        })
        .collect();
    out.sort_by_key(|m| (!m.host, m.actor));
    out
}
/// The host actor's `@session`; `None` while the host or its record is not known yet
/// (right after a migration or a reconnect), so callers do not diff against a default.
pub fn session_state(session: &Session) -> Option<SessionState> {
    session
        .host_actor()
        .and_then(|h| session.actors.get(&h))
        .and_then(|a| a.application.get(SESSION_KEY))
        .and_then(|r| SessionState::decode(&r.value))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RosterEvent {
    Joined(String),
    Left(String),
    /// A new host after the old one left (retail "New Host is: {0:s}").
    NewHost(String),
    /// The host handed the session over while staying (Make Host; retail's event 25, "{0:s} is now the host").
    AdminTransfer(String),
    /// The host vanished for good (retail "Host has disconnected. Finding a New Host.").
    HostLeft,
    Kicked(String),
    Privacy { by: String, private: bool },
}

#[derive(Default)]
pub struct Tracker {
    known: BTreeMap<u64, Member>,
    gone: BTreeMap<u64, (Member, u64)>,
    host: Option<u64>,
    /// Last `@session` seen; the first one is adopted silently as the baseline.
    state: Option<SessionState>,
    baseline_until: u64,
    /// When the connection to the host went down (`pause`); the clocks stop until it is back.
    paused: Option<u64>,
    /// "New Host is" held while the old host may still return (Make Host): (old host actor, new host name).
    new_host: Option<(u64, String)>,
    /// When the recent Joined / Left lines were shown (Q8: join-leave spam is capped at CHURN_LINES per CHURN_MS).
    churn: std::collections::VecDeque<u64>,
}
impl Tracker {
    /// A new session: the next members are the baseline.
    pub fn reset(&mut self, now_ms: u64) {
        *self = Tracker {
            baseline_until: now_ms + BASELINE_MS,
            ..Tracker::default()
        };
    }
    /// Called instead of `update` while the client is not connected to a host (joining, or a
    /// migration rebuilding membership). The baseline and leave windows count from the moment the
    /// connection is back, so a slow Steam handshake neither announces everyone already there
    /// nor turns a migration into "left" and "joined" lines.
    pub fn pause(&mut self, now_ms: u64) {
        self.paused.get_or_insert(now_ms);
    }
    /// `state` is `None` while the host's `@session` is unknown (migration, reconnect): kick and
    /// privacy are then not diffed, so the event log stays quiet.
    pub fn update(
        &mut self,
        members: &[Member],
        state: Option<&SessionState>,
        now_ms: u64,
    ) -> Vec<RosterEvent> {
        let mut events = Vec::new();
        if let Some(at) = self.paused.take() {
            let gap = now_ms.saturating_sub(at);
            if self.baseline_until > at {
                self.baseline_until += gap;
            }
            for (_, since) in self.gone.values_mut() {
                *since += gap;
            }
        }
        let baseline = now_ms < self.baseline_until;
        let mut joined = Vec::new();
        for m in members {
            let returning = self.gone.remove(&m.actor).is_some();
            // The old host is back (Make Host): the hand-off is the one "is now the host" line.
            if returning && self.new_host.as_ref().is_some_and(|(old, _)| *old == m.actor) {
                let (_, name) = self.new_host.take().unwrap();
                events.push(RosterEvent::AdminTransfer(name));
            }
            match self.known.get_mut(&m.actor) {
                Some(known) if known == m => {}
                Some(known) => *known = m.clone(),
                None => {
                    self.known.insert(m.actor, m.clone());
                    if !returning && !baseline {
                        joined.push(m);
                    }
                }
            }
        }
        let present: Vec<u64> = members.iter().map(|m| m.actor).collect();
        let missing: Vec<u64> = self
            .known
            .keys()
            .copied()
            .filter(|a| !present.contains(a))
            .collect();
        for actor in missing {
            let m = self.known.remove(&actor).unwrap();
            self.gone.insert(actor, (m, now_ms));
        }
        // Audit 10-24: a quick rejoin is a new actor with the same SteamID: neither "joined" nor, later, "left".
        for m in joined {
            let old = self.gone.iter().find(|(_, (g, _))| m.steam != 0 && g.steam == m.steam && !g.host).map(|(&a, _)| a);
            match old {
                Some(old) => _ = self.gone.remove(&old),
                None => events.push(RosterEvent::Joined(m.name.clone())),
            }
        }
        // Kicks are keyed by actor id and a unique seq, never by display name.
        // New entries are found by comparing entries, not seq, so a new host whose @session
        // restarted its seq at 1 is still diffed correctly.
        let before = self.state.as_ref();
        if let (Some(before), Some(state)) = (before, state) {
            for k in state.kicked.iter().filter(|k| !before.kicked.contains(k)) {
                events.push(RosterEvent::Kicked(k.name.clone()));
                self.gone.remove(&k.actor);
            }
        }
        // ponytail: a kicked actor that rejoins with the same id and later leaves logs no "left".
        let kicked = state.or(self.state.as_ref());
        let expired: Vec<u64> = self
            .gone
            .iter()
            .filter(|(_, (_, at))| now_ms.saturating_sub(*at) >= LEAVE_GRACE_MS)
            .map(|(&a, _)| a)
            .collect();
        for actor in expired {
            let (m, _) = self.gone.remove(&actor).unwrap();
            if kicked.is_some_and(|s| s.kicked.iter().any(|k| k.actor == actor)) {
                continue;
            }
            if !m.host {
                events.push(RosterEvent::Left(m.name));
                continue;
            }
            // It was the host when it vanished: retail order, then the new host if already known.
            events.push(RosterEvent::HostLeft);
            if self.new_host.as_ref().is_some_and(|(old, _)| *old == actor) {
                let (_, name) = self.new_host.take().unwrap();
                events.push(RosterEvent::NewHost(name));
            }
        }
        if let Some(h) = members.iter().find(|m| m.host) {
            if let Some(old) = self.host.filter(|&old| old != h.actor && !baseline) {
                if self.gone.contains_key(&old) {
                    self.new_host = Some((old, h.name.clone()));
                } else {
                    // The old host is still here: a hand-off (Make Host).
                    events.push(RosterEvent::AdminTransfer(h.name.clone()));
                }
            }
            self.host = Some(h.actor);
        }
        if let Some(state) = state {
            if before.is_some_and(|b| b.private != state.private) {
                let by = members
                    .iter()
                    .find(|m| m.host)
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                events.push(RosterEvent::Privacy {
                    by,
                    private: state.private,
                });
            }
            if before != Some(state) {
                self.state = Some(state.clone());
            }
        }
        // ponytail: lines past the cap are dropped, not summarised; add a "and N more" line if players miss them.
        while self.churn.front().is_some_and(|&t| now_ms.saturating_sub(t) >= CHURN_MS) {
            self.churn.pop_front();
        }
        events.retain(|e| {
            if !matches!(e, RosterEvent::Joined(_) | RosterEvent::Left(_)) {
                return true;
            }
            let shown = self.churn.len() < CHURN_LINES;
            if shown {
                self.churn.push_back(now_ms);
            }
            shown
        });
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// User 10-29: the host's location row goes over the wire, and a host's oversized one is refused.
    #[test]
    fn the_hosts_location_rides_the_session_state() {
        let state = SessionState { location: Some("#Mega Compound".into()), ..Default::default() };
        assert_eq!(SessionState::decode(&state.encode()), Some(state));
        let long = SessionState { location: Some("x".repeat(TITLE_MAX + 1)), ..Default::default() };
        assert_eq!(SessionState::decode(&long.encode()), None);
    }
    #[test]
    fn a_host_s_session_strings_cannot_carry_line_breaks_or_separators() {
        let ok = Target::new(0, "spot_1", "University", "ID_SPOT");
        let mut s = SessionState { game: Some(Game { token: 1, mode: "skate".into(), slots: vec![], results: false, teams: vec![] }), ..Default::default() };
        s.proposal = Some(Proposal { by: "Host".into(), actor: 1, target: ok.clone(), token: 1 });
        assert!(SessionState::decode(&s.encode()).is_some());
        for bad in ["a\nREPORT_PANIC x", "a\u{1f}b", "a\u{202e}b"] {
            let mut t = s.clone();
            t.game.as_mut().unwrap().mode = bad.into();
            assert_eq!(SessionState::decode(&t.encode()), None, "mode {bad:?}");
            let mut t = s.clone();
            t.proposal.as_mut().unwrap().by = bad.into();
            assert_eq!(SessionState::decode(&t.encode()), None, "by {bad:?}");
            let mut t = s.clone();
            t.proposal.as_mut().unwrap().target.map = bad.into();
            assert_eq!(SessionState::decode(&t.encode()), None, "map {bad:?}");
            let mut t = s.clone();
            t.challenge = Some(ChallengeLobby { title: bad.into(), team: false, row: None });
            assert_eq!(SessionState::decode(&t.encode()), None, "title {bad:?}");
            let mut t = s.clone();
            t.kicked.push(Kick { seq: 1, actor: 2, name: bad.into(), steam: 0 });
            assert_eq!(SessionState::decode(&t.encode()), None, "kick {bad:?}");
            assert_eq!(Propose::decode(&Propose { nonce: 1, target: Target { id: bad.into(), ..ok.clone() } }.encode()), None);
        }
        let mut t = s.clone();
        t.kicked.push(Kick { seq: u64::MAX, actor: 2, name: "x".into(), steam: 0 });
        t.record_kick(3, "y", 0); // a host's u64::MAX seq: no overflow panic
    }
    #[test]
    fn names_truncate_on_char_boundaries() {
        assert_eq!(truncate("abc", 32), "abc");
        let s = "ééééééééééééééééé"; // 17 x 2 bytes = 34
        assert_eq!(truncate(s, 32).len(), 32);
        let emoji = format!("{}😀", "a".repeat(30)); // 30 + 4 bytes
        assert_eq!(truncate(&emoji, 32), "a".repeat(30));
        let p = Player {
            name: "x".repeat(40),
            steam: 7,
        };
        assert_eq!(
            Player::decode(&p.encode()),
            Some(Player {
                name: "x".repeat(32),
                steam: 7
            })
        );
        assert_eq!(Player::decode(&[1, 2]), None, "too short");
        let named = |name: &str| Player::decode(&Player { name: name.into(), steam: 0 }.encode()).unwrap().name;
        assert_eq!(named("\n\n\n"), "The Legend", "blank after control characters");
        assert_eq!(named(""), "The Legend");
        assert_eq!(named(" A\nB\t"), "A B");
        // C4: invisible names, bidi tricks and private use are gone; reserved names read as the game's own.
        for blank in ["\u{200B}\u{200B}", "\u{3164}", "\u{2800}", "\u{115F}\u{1160}", "\u{202E}\u{FEFF}", "\u{E000}"] {
            assert_eq!(named(blank), "The Legend", "{blank:?}");
        }
        assert_eq!(named("Ann\u{202E}aie\u{200B}"), "Annaie");
        assert_eq!(named(&format!("Z{}", "\u{301}".repeat(10))), format!("Z{}", "\u{301}".repeat(crate::chat::MAX_MARKS)));
        for fake in ["Host", "host has disconnected.", "SERVER", "H0st", "Нost", "Ｈｏｓｔ", "Admin", "System:", "Skate 3", "skate3 official", "Moderator", "  Notice  "] {
            assert_eq!(named(fake), "The Legend", "{fake:?}");
        }
        for fine in ["Hostile Hank", "Skate Rat", "Sk8 3000", "Ann", "Ghost", "Даша", "健太"] {
            assert_eq!(named(fine), fine);
        }
        // User 10-25: no name fakes a mark the game draws after names (the creator tag, the host and friend marks).
        for fake in [
            "bob - Creator", "CREATOR", "xXcreatorXx", "C R E A T O R", "Cr3at0r", "𝐂𝐫𝐞𝐚𝐭𝐨𝐫 bob", "ⓒⓡⓔⓐⓣⓞⓡ", "ᴄʀᴇᴀᴛᴏʀ",
            "Ｃｒｅａｔｏｒ", "Сreator", "bob - Host", "bob-host", "bob (Friend)", "bob - Co-Host", "bob 𝐇𝐨𝐬𝐭",
        ] {
            assert_eq!(named(fake), "The Legend", "{fake:?}");
        }
        for fine in ["Hostile Hank", "Ghostface", "Friendly Fred", "Create", "Reactor Joe"] {
            assert_eq!(named(fine), fine);
        }
        assert_eq!(
            Player::decode(&[0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xfe]),
            None,
            "invalid UTF-8"
        );
    }
    #[test]
    fn session_state_round_trips_and_caps_kicks() {
        let mut s = SessionState::default();
        for i in 0..12 {
            s.record_kick(i, &format!("p{i}"), 0);
        }
        assert_eq!(s.kicked.len(), KICKED_MAX);
        assert_eq!(s.kicked.last().map(|k| k.name.as_str()), Some("p11"));
        // Worst case names (JSON escapes) still fit one application value.
        for i in 0..12 {
            s.record_kick(u64::MAX - i, &"\u{0}".repeat(32), u64::MAX);
        }
        assert!(s.encode().len() <= crate::lobby::MAX_APP_VALUE);
        let mut s = SessionState::default();
        s.record_kick(1, "p", 0);
        s.private = true;
        assert_eq!(SessionState::decode(&s.encode()), Some(s.clone()));
        assert!(s.encode().len() <= crate::lobby::MAX_APP_VALUE);
        // The challenge lobby round-trips; a record without it (older peer) is freeskate.
        s.challenge = Some(ChallengeLobby { title: "ID_MAP_FILTER_CHALLENGETYPES_SPOTBATTLE".into(), team: true, row: None });
        assert_eq!(SessionState::decode(&s.encode()), Some(s.clone()));
        assert_eq!(SessionState::decode(br#"{"private":false,"kicked":[]}"#), Some(SessionState::default()));
        s.challenge = Some(ChallengeLobby { title: "x".repeat(TITLE_MAX + 1), team: false, row: None });
        assert_eq!(SessionState::decode(&s.encode()), None);
    }
    /// User 10-25: players don't collide unless the host turns it on; an older peer's state (no "pc") reads off.
    #[test]
    fn player_collisions_are_off_unless_the_host_turns_them_on() {
        assert!(!SessionState::default().collisions);
        let on = SessionState { collisions: true, ..Default::default() };
        assert!(SessionState::decode(&on.encode()).unwrap().collisions);
        let old = SessionState::default().encode();
        assert!(!String::from_utf8_lossy(&old).contains("\"pc\"") && !SessionState::decode(&old).unwrap().collisions);
    }
    #[test]
    fn moves_proposals_and_difficulty_round_trip_and_fit() {
        let target = Target::new(2, &"i".repeat(80), "University", "ID_ONLINE_FS_LOCATION_UNIV_MEGA");
        assert_eq!(target.id.len(), TITLE_MAX, "cut to fit");
        let mut s = SessionState { difficulty: Some(1), physics: Some(1), no_proposals: true, ..Default::default() };
        let row = Some(Target::new(3, &"r".repeat(TITLE_MAX), "", ""));
        s.challenge = Some(ChallengeLobby { title: "t".repeat(TITLE_MAX), team: true, row });
        s.proposal = Some(Proposal { by: "\"".repeat(NAME_MAX), actor: 7, target: target.clone(), token: u64::MAX });
        s.moving = Some(Move { token: u64::MAX, target: target.clone() });
        for i in 0..KICKED_MAX as u64 {
            s.record_kick(i, &"\"".repeat(NAME_MAX), u64::MAX);
        }
        let bytes = s.encode();
        assert!(bytes.len() <= crate::lobby::MAX_APP_VALUE, "{} bytes: old kicks give way", bytes.len());
        let back = SessionState::decode(&bytes).unwrap();
        assert_eq!((back.difficulty, back.no_proposals, &back.proposal, &back.moving), (s.difficulty, true, &s.proposal, &s.moving));
        assert_eq!(back.physics, Some(1), "the host's Skate 2 physics");
        assert_eq!(back.kicked.last(), s.kicked.last(), "the newest kick is kept");
        // A move ends a match (online_moves::start_move), and no match starts while moving: a match's worst case is
        // with a proposal.
        s.moving = None;
        s.game = Some(Game { token: u64::MAX, mode: "skate".into(), slots: vec![u64::MAX; GAME_SLOTS], results: true, teams: vec![] });
        let bytes = s.encode();
        assert!(bytes.len() <= crate::lobby::MAX_APP_VALUE, "{} bytes", bytes.len());
        let back = SessionState::decode(&bytes).unwrap();
        assert_eq!((&back.game, &back.challenge, &back.proposal), (&s.game, &s.challenge, &s.proposal));
        assert_eq!(back.kicked.last(), s.kicked.last(), "a match still leaves room for the newest kick");
        // A team lobby's worst case (every player moved to the second team, a switch and a proposal open, a team match
        // on): the kicks give way, everything else fits. Team lobbies are on Skate 3 maps, where the host sends no
        // physics (online_moves::physics: only Skate 2 on New San Vanelona).
        let mut team = s.clone();
        team.physics = None;
        team.team_b = vec![u64::MAX; GAME_SLOTS - 1];
        team.team_a = vec![u64::MAX - 1];
        team.switching = Some(u64::MAX);
        team.game.as_mut().unwrap().teams = vec![2; GAME_SLOTS];
        let bytes = team.encode();
        assert!(bytes.len() <= crate::lobby::MAX_APP_VALUE, "{} bytes", bytes.len());
        let back = SessionState::decode(&bytes).unwrap();
        assert_eq!((&back.game, &back.team_b, &back.team_a, back.switching), (&team.game, &team.team_b, &team.team_a, team.switching));
        assert_eq!(back.proposal, team.proposal);
        assert_eq!((back.team(u64::MAX), back.team(u64::MAX - 1), back.team(3)), (2, 1, 1));
        let mut crowded = team.clone();
        crowded.team_a.push(5);
        assert_eq!(SessionState::decode(&serde_json::to_vec(&crowded).unwrap()), None, "more players than a lobby holds");
        let mut odd = team.clone();
        odd.game.as_mut().unwrap().teams.pop();
        assert_eq!(SessionState::decode(&serde_json::to_vec(&odd).unwrap()), None, "a team per slot");
        let accept = Accept { token: u64::MAX };
        assert_eq!(Accept::decode(&accept.encode()), Some(accept));
        assert_eq!(Switch::decode(&Switch { nonce: 4 }.encode()), Some(Switch { nonce: 4 }));
        let mut many = s.clone();
        many.game.as_mut().unwrap().slots.push(1);
        assert_eq!(SessionState::decode(&serde_json::to_vec(&many).unwrap()), None, "more players than a match holds");
        let score = Score { token: 9, value: -3, finished: true, ..Default::default() };
        assert_eq!(Score::decode(&score.encode()), Some(score.clone()));
        assert_eq!(Score::decode(br#"{"k":9,"v":-3,"f":true}"#), Some(score), "no round, nothing taken");
        let acked = Score { round: 2, taken: vec![(u64::MAX, u32::MAX); GAME_SLOTS], ..Default::default() };
        assert_eq!(Score::decode(&acked.encode()), Some(acked.clone()));
        assert!(acked.encode().len() <= crate::lobby::MAX_APP_VALUE);
        // C5: the oldest events go first; the newer ones wait for the next record (never dropped by the encoder).
        let events = Events { token: 9, first: 4, items: (0..40).map(|i| format!("{i:0>60}")).collect() };
        let (sent, bytes) = events.fitted();
        let back = Events::decode(&bytes).unwrap();
        assert!(bytes.len() <= crate::lobby::MAX_APP_VALUE && sent < 40);
        assert_eq!((back.first, back.items.as_slice()), (4, &events.items[..sent]), "the oldest kept, numbered from first");
        let ready = Ready { row: "dr_dwtn_01".into(), ok: true };
        assert_eq!(Ready::decode(&ready.encode()), Some(ready));
        // A record from before these fields: no difficulty, proposals allowed, nothing pending.
        assert_eq!(SessionState::decode(br#"{"private":true,"kicked":[]}"#), Some(SessionState { private: true, ..Default::default() }));
        let propose = Propose { nonce: 3, target };
        assert_eq!(Propose::decode(&propose.encode()), Some(propose));
        assert_eq!(Propose::decode(br#"{"k":1,"g":{"t":0,"i":"x","m":"y","n":"z"}}"#).map(|p| p.target.id), Some("x".into()));
        let long = format!(r#"{{"k":1,"g":{{"t":0,"i":"{}","m":"y","n":"z"}}}}"#, "x".repeat(TITLE_MAX + 1));
        assert_eq!(Propose::decode(long.as_bytes()), None);
    }
    #[test]
    fn join_leave_spam_is_capped() {
        let mut t = Tracker::default();
        t.reset(0);
        let host = m(1, "Host", true);
        t.update(&[host.clone()], None, 0);
        let mut lines = 0;
        for i in 0..100u64 {
            // A fresh account every 100 ms, each gone the next tick.
            let now = 3_000 + i * 100;
            lines += t.update(&[host.clone(), m(100 + i, "spam", false)], None, now).len();
            lines += t.update(&[host.clone()], None, now + 50).len();
        }
        assert!(lines <= CHURN_LINES, "{lines} lines");
        let later = 3_000 + 100 * 100 + CHURN_MS;
        let after = t.update(&[host.clone(), m(999, "Real", false)], None, later);
        assert!(after.len() <= CHURN_LINES && after[0] == RosterEvent::Joined("Real".into()), "{after:?}");
    }
    fn m(actor: u64, name: &str, host: bool) -> Member {
        Member {
            actor,
            name: name.into(),
            steam: 0,
            host,
        }
    }
    /// Audit 10-24: a quick leave and rejoin (a new actor id, the same SteamID, within LEAVE_GRACE_MS) read "X joined"
    /// then "X left" while X was still there. It is quiet now; a real leave still reads "left".
    #[test]
    fn a_quick_rejoin_under_a_new_actor_logs_no_joined_or_left() {
        let x = |actor| Member { actor, name: "X".into(), steam: 77, host: false };
        let host = m(1, "Host", true);
        let mut t = Tracker::default();
        t.reset(0);
        t.update(&[host.clone(), x(2)], None, 100);
        // The old actor replaced by a new one in the same tick (a rejoin from the same endpoint).
        assert!(t.update(&[host.clone(), x(3)], None, 5_000).is_empty());
        assert!(t.update(&[host.clone(), x(3)], None, 9_000).is_empty(), "no late \"left\"");
        // Gone first, back a second later.
        assert!(t.update(&[host.clone()], None, 10_000).is_empty());
        assert!(t.update(&[host.clone(), x(4)], None, 11_000).is_empty());
        assert!(t.update(&[host.clone(), x(4)], None, 15_000).is_empty());
        t.update(&[host.clone()], None, 16_000);
        assert_eq!(t.update(&[host.clone()], None, 20_000), vec![RosterEvent::Left("X".into())]);
        // Direct players carry no SteamID: matched by nothing.
        t.update(&[host.clone(), m(5, "D", false)], None, 21_000);
        t.update(&[host.clone()], None, 21_500);
        assert_eq!(t.update(&[host.clone(), m(6, "D", false)], None, 22_000), vec![RosterEvent::Joined("D".into())]);
    }
    #[test]
    fn tracker_reports_joins_leaves_kicks_and_privacy() {
        let mut t = Tracker::default();
        let open = SessionState::default();
        t.reset(0);
        assert!(
            t.update(
                &[m(1, "Host", true), m(2, "A", false)],
                Some(&open),
                100
            )
            .is_empty(),
            "baseline at join"
        );
        assert_eq!(
            t.update(
                &[m(1, "Host", true), m(2, "A", false), m(3, "B", false)],
                Some(&open),
                5_000
            ),
            vec![RosterEvent::Joined("B".into())]
        );
        // B vanishes: Left only after the grace period.
        assert!(
            t.update(
                &[m(1, "Host", true), m(2, "A", false)],
                Some(&open),
                6_000
            )
            .is_empty()
        );
        assert_eq!(
            t.update(
                &[m(1, "Host", true), m(2, "A", false)],
                Some(&open),
                9_500
            ),
            vec![RosterEvent::Left("B".into())]
        );
        // A kicked: Kicked, never Left.
        let mut kicked = SessionState::default();
        kicked.record_kick(2, "A", 0);
        assert_eq!(
            t.update(&[m(1, "Host", true)], Some(&kicked), 10_000),
            vec![RosterEvent::Kicked("A".into())]
        );
        assert!(
            t.update(&[m(1, "Host", true)], Some(&kicked), 20_000)
                .is_empty()
        );
        let private = SessionState {
            private: true,
            ..kicked.clone()
        };
        assert_eq!(
            t.update(&[m(1, "Host", true)], Some(&private), 21_000),
            vec![RosterEvent::Privacy {
                by: "Host".into(),
                private: true
            }]
        );
    }
    #[test]
    fn migration_rebuild_is_one_new_host_line() {
        let mut t = Tracker::default();
        let s = SessionState::default();
        t.reset(0);
        t.update(
            &[m(1, "Old", true), m(2, "A", false), m(3, "B", false)],
            Some(&s),
            3_000,
        );
        // Session::migrate drops remote actors; they come back within the grace period with a new host.
        assert!(t.update(&[m(2, "A", false)], None, 3_100).is_empty());
        // The old host may still come back (Make Host): "New Host is" waits for it.
        assert!(t.update(&[m(2, "A", true), m(3, "B", false)], Some(&s), 3_900).is_empty());
        // It never does: retail's host-left line, then the new host.
        assert_eq!(
            t.update(&[m(2, "A", true), m(3, "B", false)], Some(&s), 7_000),
            vec![RosterEvent::HostLeft, RosterEvent::NewHost("A".into())]
        );
    }
    #[test]
    fn make_host_is_one_admin_transfer_line_when_the_old_host_returns() {
        let mut t = Tracker::default();
        let s = SessionState::default();
        t.reset(0);
        t.update(&[m(1, "Old", true), m(2, "A", false)], Some(&s), 3_000);
        assert!(t.update(&[m(2, "A", false)], None, 3_100).is_empty());
        assert!(t.update(&[m(2, "A", true)], Some(&s), 3_300).is_empty());
        // Item 30: retail's hand-off line (0x825e5f78 event 25, ID_ONLINE_EVENT_ADMIN_TRANSFER), not "New Host is".
        assert_eq!(
            t.update(&[m(1, "Old", false), m(2, "A", true)], Some(&s), 3_600),
            vec![RosterEvent::AdminTransfer("A".into())]
        );
        assert!(t.update(&[m(1, "Old", false), m(2, "A", true)], Some(&s), 9_000).is_empty());
        // A hand-off the old host never drops out of is the same line.
        assert_eq!(
            t.update(&[m(1, "Old", true), m(2, "A", false)], Some(&s), 9_500),
            vec![RosterEvent::AdminTransfer("Old".into())]
        );
    }
    /// I1: a Steam join whose handshake takes longer than the baseline window. The tracker is
    /// paused until the connection is up, so players already there are not announced.
    #[test]
    fn a_slow_join_handshake_announces_nobody_already_there() {
        let mut t = Tracker::default();
        let s = SessionState::default();
        t.reset(0);
        // Not connected for 4 s: only ourselves (no @player from anyone else yet).
        for now in [0, 1_000, 2_500, 4_000] {
            t.pause(now);
        }
        assert!(
            t.update(&[m(1, "Host", true), m(9, "Me", false)], None, 4_100).is_empty(),
            "connected: the baseline starts now"
        );
        // Other records arrive over the next second.
        assert!(t
            .update(
                &[m(1, "Host", true), m(2, "A", false), m(3, "B", false), m(9, "Me", false)],
                Some(&s),
                5_200
            )
            .is_empty());
        // A real join after the baseline is still announced.
        assert_eq!(
            t.update(
                &[m(1, "Host", true), m(2, "A", false), m(3, "B", false), m(4, "C", false), m(9, "Me", false)],
                Some(&s),
                7_000
            ),
            vec![RosterEvent::Joined("C".into())]
        );
    }
    /// I1: a migration whose route to the new host takes longer than LEAVE_GRACE_MS.
    #[test]
    fn a_slow_migration_reconnect_is_not_left_and_joined_lines() {
        let mut t = Tracker::default();
        let s = SessionState::default();
        t.reset(0);
        let all = [m(1, "Old", true), m(2, "A", false), m(3, "B", false), m(9, "Me", false)];
        t.update(&all, Some(&s), 3_000);
        // Old host gone; migrate cleared every remote actor and we wait 5 s for A's ROSTER.
        assert!(t.update(&[m(9, "Me", false)], None, 3_100).is_empty());
        for now in [3_200, 6_000, 8_200] {
            t.pause(now);
        }
        assert!(t.update(&[m(2, "A", true), m(9, "Me", false)], None, 8_300).is_empty());
        assert!(t
            .update(&[m(2, "A", true), m(3, "B", false), m(9, "Me", false)], Some(&s), 8_800)
            .is_empty());
        // Only the old host is really gone: retail's two lines, no "left"/"joined" for A or B.
        assert_eq!(
            t.update(&[m(2, "A", true), m(3, "B", false), m(9, "Me", false)], Some(&s), 11_500),
            vec![RosterEvent::HostLeft, RosterEvent::NewHost("A".into())]
        );
    }
    #[test]
    fn migration_in_a_private_session_with_past_kicks_is_one_line() {
        let mut t = Tracker::default();
        let mut s = SessionState {
            private: true,
            ..Default::default()
        };
        s.record_kick(9, "Z", 0);
        t.reset(0);
        t.update(
            &[m(1, "Old", true), m(2, "A", false), m(3, "B", false)],
            Some(&s),
            100,
        );
        // Host link gone: no host member and no @session yet.
        assert!(t.update(&[m(2, "A", false)], None, 3_000).is_empty());
        // "New Host is" waits for the old host's leave window (it might be a Make Host).
        assert!(t.update(&[m(2, "A", true), m(3, "B", false)], None, 3_900).is_empty());
        // The new host's inherited @session is identical: nothing to announce.
        assert!(
            t.update(&[m(2, "A", true), m(3, "B", false)], Some(&s), 4_500)
                .is_empty()
        );
        // ...but a later change is still announced.
        let public = SessionState {
            private: false,
            ..s
        };
        assert_eq!(
            t.update(&[m(2, "A", true), m(3, "B", false)], Some(&public), 5_000),
            vec![RosterEvent::Privacy {
                by: "A".into(),
                private: false
            }]
        );
        assert_eq!(
            t.update(&[m(2, "A", true), m(3, "B", false)], Some(&public), 6_000),
            vec![RosterEvent::HostLeft, RosterEvent::NewHost("A".into())]
        );
    }
    #[test]
    fn duplicate_names_are_kicked_and_left_by_actor() {
        let mut t = Tracker::default();
        let host = m(1, "Host", true);
        let (a, b) = (m(2, "The Legend", false), m(3, "The Legend", false));
        let mut s = SessionState::default();
        t.reset(0);
        t.update(&[host.clone(), a.clone(), b.clone()], Some(&s), 100);
        // Kick the first Legend: one Kicked line, and no Left for it.
        s.record_kick(2, "The Legend", 0);
        assert_eq!(
            t.update(&[host.clone(), b.clone()], Some(&s), 5_000),
            vec![RosterEvent::Kicked("The Legend".into())]
        );
        assert!(t.update(&[host.clone(), b.clone()], Some(&s), 9_000).is_empty());
        // The other Legend leaving still logs.
        assert!(t.update(&[host.clone()], Some(&s), 10_000).is_empty());
        assert_eq!(
            t.update(&[host.clone()], Some(&s), 13_500),
            vec![RosterEvent::Left("The Legend".into())]
        );
        // Kicking a same-named player again logs again, as does a repeat kick of one actor.
        s.record_kick(4, "The Legend", 0);
        assert_eq!(
            t.update(&[host.clone()], Some(&s), 14_000),
            vec![RosterEvent::Kicked("The Legend".into())]
        );
        s.record_kick(4, "The Legend", 0);
        assert_eq!(
            t.update(&[host], Some(&s), 15_000),
            vec![RosterEvent::Kicked("The Legend".into())]
        );
    }
    #[test]
    fn a_new_hosts_restarted_seq_still_logs_new_kicks() {
        let mut t = Tracker::default();
        let mut old = SessionState::default();
        old.record_kick(8, "Y", 0);
        old.record_kick(9, "Z", 0);
        t.reset(0);
        t.update(&[m(1, "Host", true)], Some(&old), 100);
        // A new host that never saw the old @session publishes seq 1 again.
        let mut fresh = SessionState::default();
        fresh.record_kick(7, "W", 0);
        assert_eq!(
            t.update(&[m(1, "Host", true)], Some(&fresh), 5_000),
            vec![RosterEvent::Kicked("W".into())]
        );
    }
    #[test]
    fn spoofed_player_steam_id_does_not_change_the_recorded_ban() {
        use crate::lobby::{Info, Session};
        let info = |id| Info {
            id,
            map: 1,
            rig: 2,
            physics: 3,
            appearance: 4,
        };
        let (mut host, mut client) = (Session::new(7, info(10), None), Session::new(7, info(20), Some(1)));
        // The client lies about its SteamID (an innocent's, 555); its endpoint is really 2.
        client.publish_application(
            PLAYER_KEY,
            Player {
                name: "Liar".into(),
                steam: 555,
            }
            .encode(),
            0,
        );
        let mut now = 0;
        for _ in 0..20 {
            now += 50;
            for p in host.service(now) {
                if p.peer == 2 {
                    client.receive(1, &p.data, now);
                }
            }
            for p in client.service(now) {
                if p.peer == 1 {
                    host.receive(2, &p.data, now);
                }
            }
        }
        assert_eq!(members(&host).iter().find(|m| m.actor == 20).unwrap().steam, 555);
        assert_eq!(host.endpoint(20), Some(2), "Make Host resolves the real endpoint");
        let mut state = SessionState::default();
        assert_eq!(kick(&mut host, &mut state, 20, true), Some("Liar".into()));
        assert_eq!(state.kicked[0].steam, 2, "the real endpoint, not the spoofed 555");
        let mut next = Session::new(7, info(30), None);
        seed_bans(&mut next, &state);
        assert!(next.is_banned(2) && !next.is_banned(555));
        assert_eq!(kick(&mut host, &mut state, 20, true), None, "already gone: nothing recorded");
        assert_eq!(state.kicked.len(), 1);
    }
    #[test]
    fn kicked_steam_ids_seed_a_new_hosts_bans() {
        use crate::lobby::{Info, Session};
        let mut s = SessionState::default();
        s.record_kick(5, "A", 111);
        s.record_kick(6, "B", 0);
        let mut host = Session::new(
            1,
            Info {
                id: 9,
                map: 0,
                rig: 0,
                physics: 0,
                appearance: 0,
            },
            None,
        );
        seed_bans(&mut host, &s);
        assert!(host.is_banned(111));
        assert!(!host.is_banned(0));
    }
    #[test]
    fn members_read_player_records_host_first() {
        use crate::lobby::{Info, Session};
        let mut s = Session::new(
            1,
            Info {
                id: 9,
                map: 0,
                rig: 0,
                physics: 0,
                appearance: 0,
            },
            None,
        );
        assert!(members(&s).is_empty(), "no @player yet");
        s.publish_application(
            PLAYER_KEY,
            Player {
                name: "Me".into(),
                steam: 5,
            }
            .encode(),
            0,
        );
        assert_eq!(
            members(&s),
            vec![Member {
                actor: 9,
                name: "Me".into(),
                steam: 5,
                host: true
            }]
        );
    }
}
