//! Online crossbar picks (core_menu hands them over as [`OnlineRequest`]s), the Player List (`player_list`), the
//! Steam sign-in (retail's EA Nation sign-in, StateOnlineSignIn) and the retail popup (`popup/sk8popup`) for it,
//! Quit Session and Kick. Both draw on `apt_screen`'s online layer while the crossbar hides itself (ruling 9:
//! retail draws the popup over the crossbar).
//!
//! Sign-in, as TU3 does it (RequireSignIn 0x8261ce38): no online row is greyed while signed out; picking one starts
//! the sign-in at once behind a "Connecting to Steam..." popup (no options, B cancels, 30 s), success shows nothing
//! and replays the pick, failure or cancel shows an OK popup.
use crate::{
    apt_screen::{ONLINE, Screens},
    config::Config,
    core_menu_runtime::Key,
    customiser::Navigation,
    multiplayer::Multiplayer,
    online_moves::PROPOSAL_TIMEOUT,
    online_roster::OnlineRoster,
    online_session::{OnlineSession, Popup},
    player_list::{self, Context, Effect, PlayerAction, PlayerList},
    popup_runtime,
    presence::Presence,
    skate_audio::FrontEndSoundRequest,
};
use bevy::prelude::*;
use serde_json::Value as Json;
use skate_net::{
    presence::{Order, Page},
    roster::{Proposal, Propose},
};
use std::path::Path;

/// One authored frame (the popup advances on real time).
const FRAME: f32 = 1. / 60.;
/// C9: authored frames (0.5 s) a new popup or proposal tab ignores A / Enter / an answer for. Knob.
const POPUP_HOLD: u32 = 30;
/// perf 10-18: a confirm the player just asked for from the crossbar (Quit, Kick, Sign Out, Switch Team) holds only this
/// long; the stray press POPUP_HOLD guards against comes with unsolicited asks. SKATE_SHORT_HOLD=0: all 30.
const ASKED_HOLD: u32 = 8;
fn popup_hold(ask: &Ask) -> u32 {
    let asked = matches!(ask, Ask::Quit | Ask::Kick { .. } | Ask::SignOut | Ask::ConfirmSwitch | Ask::Restart(..));
    if asked && crate::config::perf_on("SKATE_SHORT_HOLD") { ASKED_HOLD } else { POPUP_HOLD }
}
/// A popup or tab shown `since` is still in its [`POPUP_HOLD`] at `now`.
fn held(since: f32, now: f32) -> bool {
    now - since < POPUP_HOLD as f32 * FRAME
}
const KICK_CONFIRM: &str = "ID_MESSENGER_KICK_CONFIRM";
/// Retail popup 38's timeout.
const SIGN_IN_TIMEOUT: f32 = 30.;
/// ponytail: Steam answers at once when it is open, which would flash the popup for a frame; retail's Blaze login took
/// seconds. Shortest time "Connecting to Steam..." stays up; a knob.
const SIGN_IN_SHOWN: f32 = 1.5;
/// No retail string fits (retail's popup 126 blames the EA server); here the Steam client is what must be running.
const SIGN_IN_FAILED: &str = "Could not connect to Steam. Steam must be open and signed in to access online features.";
const INVITE_SENT: &str = "Invite sent. To accept it, your friend first signs in to Online in S3R.";
/// The steam-relay folder is missing beside the exe; opening Steam would not help.
const HELPER_MISSING: &str = "Online play files are missing (steam-relay). Reinstall S3R to play online.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnlineAction {
    PlayerList,
    Contacts,
    HostActions,
    QuitSession,
    Friends,
    StartChallenge,
    /// An Own the Lot lobby's Select Activity (retail crossbar item 42): the picked activity starts.
    SelectActivity,
    /// A team lobby's Switch Team (ID_ONLINE_HUB_SWITCH_TEAM).
    SwitchTeam,
    /// Not retail (user 10-25): the server browser (server_browser).
    ServerBrowser,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKind {
    Session,
    Friends,
    Recent,
    Teammates,
    InviteFriend,
    /// The server browser's public sessions.
    Lobbies,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OnlineRequest {
    List(ListKind),
    TogglePrivacy,
    QuitSession,
    Kick { actor: u64, name: String },
    /// Options > Sign in to / Sign out of Steam (crossbar item 12).
    SignInOut,
    /// Host Actions > Allow / Disable Challenge Proposals.
    ToggleProposals,
    /// Not retail (user 10-25): Host Actions > Turn On / Off Player Collisions.
    ToggleCollisions,
    /// Not retail (user 10-25): Host Actions > Hide from / Show in Server Browser.
    ToggleListed,
    /// Not retail (user 10-25): Host Actions > Server Name and Description (session_details' keyboard).
    EditDetails,
    /// Not retail (user 10-26): Settings search's words, typed on the same keyboard from the open page's (core_menu).
    SearchSettings(String),
    /// An Online Challenge Map row picked in a session: the host moves the session there, a guest proposes it.
    /// With the players the row's lobby holds (online_session::capacity).
    Move(skate_net::roster::Target, usize),
    /// Host only: start the lobby's challenge (match_core).
    StartChallenge,
    /// Host only: start the Own the Lot lobby's activity (its index in skate3_otl::OtlLots' list for the lot).
    StartActivity(usize),
    /// Host only: make `actor` a co-host (`on`) or not.
    CoHost { actor: u64, on: bool },
    /// A retail notice (an Online Challenge Map row that cannot be played here: match_core::unavailable).
    Notice(&'static str),
    /// A team lobby's Switch Team (crossbar): ID_ONLINE_CONFIRM_SWITCH_TEAM first; the host switches at once, a guest
    /// proposes it to the host.
    SwitchTeam,
    /// Host only, a team lobby's Player List: move `actor` to the other team (decision T1).
    MoveTeam(u64),
    /// Not retail (user 10-27): a Game Settings pick that turns Performance mode on or off (core_menu): asked first;
    /// Yes puts it in and restarts the game.
    Restart(crate::graphics_menu::Setting, usize),
}

/// What a sign-in unlocks once Steam answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Then {
    /// Open the Online Challenge Map (core_menu does).
    ChallengeMap,
    Request(OnlineRequest),
    /// Steam join requests held while signed out.
    Join(Vec<String>),
    /// The Sign in row: close the crossbar (its row now reads Sign out).
    CloseCrossbar,
}

/// What a crossbar pick asks for; `choice` is the picker entry (Contacts, Host Actions).
pub(crate) fn request(action: OnlineAction, choice: Option<usize>) -> Option<OnlineRequest> {
    use OnlineAction::*;
    Some(match (action, choice) {
        (PlayerList, _) => OnlineRequest::List(ListKind::Session),
        (Friends, _) => OnlineRequest::List(ListKind::Friends),
        (ServerBrowser, _) => OnlineRequest::List(ListKind::Lobbies),
        (QuitSession, _) => OnlineRequest::QuitSession,
        (StartChallenge, _) => OnlineRequest::StartChallenge,
        (SwitchTeam, _) => OnlineRequest::SwitchTeam,
        (SelectActivity, Some(i)) => OnlineRequest::StartActivity(i),
        (Contacts, Some(0)) => OnlineRequest::List(ListKind::Teammates),
        (Contacts, Some(1)) => OnlineRequest::List(ListKind::Friends),
        (Contacts, Some(2)) => OnlineRequest::List(ListKind::Recent),
        (Contacts, Some(3)) => OnlineRequest::List(ListKind::InviteFriend),
        (HostActions, Some(0)) => OnlineRequest::ToggleProposals,
        (HostActions, Some(1)) => OnlineRequest::TogglePrivacy,
        (HostActions, Some(5)) => OnlineRequest::ToggleCollisions,
        (HostActions, Some(6)) => OnlineRequest::ToggleListed,
        (HostActions, Some(7)) => OnlineRequest::EditDetails,
        // Session Difficulty, Weather and Time of Day (2-4) open Game Settings (core_menu).
        _ => return None,
    })
}

enum Ask {
    /// Retail popup 63: No / Yes, B cancels; Yes leaves.
    Quit,
    Kick { actor: u64, name: String },
    /// "Connecting to Steam..." (retail popup 38): no options, B cancels. The attempt itself is `OnlineUi::connecting`.
    Connecting,
    /// One OK: sign-in cancelled or failed, signed out (retail popups 42, 126, 89).
    Notice(String),
    /// No / Yes (retail popup 33; in a session, ID_ONLINE_GAME_SIGNOUT_MESSAGE).
    SignOut,
    /// A Steam invite while signed out (retail popup 55): Yes / No.
    Invite(Vec<String>),
    /// Switch Team's question (ID_ONLINE_CONFIRM_SWITCH_TEAM): Yes / No.
    ConfirmSwitch,
    /// Host only: a guest's Switch Team proposal (Accept / Decline; B and PROPOSAL_TIMEOUT decline): (actor, name).
    Switch(u64, String),
    /// Quick Match's search (online_session `Popup::Matching`): retail popup 43, "Matching up with skaters...", no
    /// options, B cancels (flag 1; opened from 0x826f4f58 / 0x826f5048 / 0x826f5278).
    Matching,
    /// Not retail (user 10-27): a Game Settings pick that turns Performance mode on or off: No / Yes, B cancels; Yes
    /// puts it in and restarts the game ([`OnlineUi::restart`]).
    Restart(crate::graphics_menu::Setting, usize),
}

const ACCEPT_DECLINE: &[&str] = &["ID_COMMON_ACCEPT", "ID_COMMON_DECLINE"];


#[derive(Resource, Default)]
pub(crate) struct OnlineUi {
    pub requests: Vec<OnlineRequest>,
    /// The confirm popup or the Player List is open: it owns input and the crossbar hides.
    pub modal: bool,
    /// The popup or list closed this frame; core_menu ignores the press that closed it.
    pub closed_this_frame: bool,
    /// Quit Session was confirmed, or Skate with picked: core_menu closes the crossbar.
    pub close_crossbar: bool,
    popup: Option<(Ask, popup_runtime::Runtime)>,
    list: Option<PlayerList>,
    /// The sk8popup manifest, read on first use; `Some(None)`: unavailable (warned once).
    source: Option<Option<Json>>,
    accumulated: f32,
    /// Something else took the screen (a Steam join loading another map): close the list and popup.
    pub dismiss: bool,
    /// (session id, local actor) last seen: a session that ends or changes closes the list, popup and crossbar.
    session: Option<(u64, u64)>,
    /// (`Presence::generation`, `OnlineRoster::generation`, [`talkers`]) the open list's rows were built from.
    rows_from: (u64, u64, Vec<u64>),
    /// Signed in to Steam (retail: EA Nation). Off until the player asks for something online.
    pub signed_in: bool,
    /// A sign-in under way: (started, what it unlocks).
    connecting: Option<(f32, Then)>,
    /// A sign-in for the Online Challenge Map succeeded; core_menu opens it.
    pub open_challenge_map: bool,
    /// A pick that needs Steam while signed out (core_menu): sign in, then do this.
    pub sign_in_for: Option<Then>,
    /// When the open popup was asked (a Switch Team proposal expires).
    asked_at: f32,
    /// The open proposal (its token) and when this player first saw it: the poster's "Expires in:" counts from there.
    proposal_seen: Option<(u64, f32)>,
    /// Seconds the open proposal has left, as the poster shows them.
    proposal_left: f32,
    /// The proposal (token) this player answered: its tab does not come back.
    answered: u64,
    /// The open proposal's tab (crate::proposal_tab), while it waits for this player's answer.
    tab: Option<Tab>,
    /// The spectate legend's font and "Back" ([`legend`]), loaded once (None inside: sk8popup unavailable).
    back: Option<Option<(crate::apt_text::Font, String)>>,
    /// The server browser's sessions, while its list is open.
    browser: crate::server_browser::Browser,
    /// Host Actions > Server Name and Description's keyboard, or Settings search's, while open (over the crossbar; it
    /// owns input).
    keyboard: Option<crate::session_details::Keyboard>,
    /// Settings search's words at Done, for core_menu to open their page.
    pub searched: Option<String>,
    /// Performance Mode's restart was confirmed: core_menu puts the pick in and restarts the game.
    pub restart: Option<(crate::graphics_menu::Setting, usize)>,
}
/// Not retail (user 10-13): what the proposal tab draws.
struct Tab {
    proposal: Proposal,
    card: crate::challenge_map::Card,
    font: crate::apt_text::Font,
    /// Challenge Proposed!, Accept, Decline, Proposed by:
    words: [String; 4],
}
impl OnlineUi {
    fn close_all(&mut self, screens: Option<&mut Screens>) {
        self.popup = None;
        self.tab = None;
        self.keyboard = None;
        if let (Some(list), Some(screens)) = (self.list.take(), screens) {
            list.close(screens);
        }
        self.modal = false;
    }
}


/// The tab for `p`: its row's card from this player's own challenge map (the proposal's own name and map when the
/// row isn't installed here), with sk8popup's Futura and strings. None without sk8popup.
fn proposal_tab(ui: &mut OnlineUi, root: &Path, map: Option<&crate::challenge_map::ChallengeMap>, installed: &[String], p: &Proposal) -> Option<Tab> {
    let source = ui.source.get_or_insert_with(|| load_source(root)).as_ref()?;
    let text = crate::apt_text::TextAssets::load(source).map_err(|e| error!("Proposal tab: {e}")).ok()?;
    let font = text.fonts.get(&text.font_named("Futura Std Medium")?)?.clone();
    let language = &source["language"];
    let card = map.and_then(|m| m.proposal_card(installed, p.target.tab.into(), &p.target.id)).unwrap_or_else(|| crate::challenge_map::Card {
        title: steam_text(language, &p.target.name).trim_start_matches('#').to_owned(),
        place: p.target.map.replace('_', " "),
        ..Default::default()
    });
    let words = ["ID_ONLINE_OVERLAY_CHALLENGE_PROPOSED", "ID_COMMON_ACCEPT", "ID_COMMON_DECLINE", "ID_ONLINE_SIGNUP_PROPOSED_BY"]
        .map(|id| steam_text(language, id));
    Some(Tab { proposal: p.clone(), card, font, words })
}

pub(crate) struct OnlineMenuPlugin;
impl Plugin for OnlineMenuPlugin {
    fn build(&self, app: &mut App) {
        let root = app.world().resource::<Config>().asset_root.clone();
        player_list::load_blocked(player_list::blocked_path(&root));
        player_list::load_voice(player_list::voice_path(&root));
        app.init_resource::<OnlineUi>()
            .add_systems(PreUpdate, interact.after(crate::customiser::navigation).before(crate::core_menu::interact))
            .add_systems(Update, player_list::guard.after(crate::online_roster::track).before(crate::online_chat::exchange))
            .add_systems(Update, draw.before(crate::apt_screen::render));
    }
}

/// The retail kick question with the name filled. No '#': apt_text resolves a '#' text token by token and would
/// fold the '^' line break into a space. The name goes in after the line breaks, its `^` unbroken: no fake line.
fn kick_text(source: &Json, name: &str) -> String {
    source["language"][KICK_CONFIRM].as_str().unwrap_or(KICK_CONFIRM).replace('^', "\n").replace("{0:s}", &crate::apt_text::unbroken(name))
}

fn load_source(asset_root: &Path) -> Option<Json> {
    let path = asset_root.join("private/online-ui/runtime/sk8popup.json");
    let result = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()));
    result.map_err(|e| warn!("Online confirm popup unavailable ({}: {e}); Quit Session and Kick act without asking", path.display())).ok()
}

/// A retail string with "EA Nation" / "Xbox LIVE" read as Steam (spec branding); the id itself when missing.
pub(crate) fn steam_text(language: &Json, id: &str) -> String {
    let text = language[id].as_str().unwrap_or(id);
    text.replace("EA Nation", "Steam").replace("Xbox LIVE", "Steam").replace('^', "\n").trim_end().to_owned()
}

/// Opens the popup; hands `ask` back when the popup is unavailable, to act on without asking.
fn ask(ui: &mut OnlineUi, asset_root: &Path, ask: Ask) -> Result<(), Ask> {
    let in_session = ui.session.is_some();
    let Some(source) = ui.source.get_or_insert_with(|| load_source(asset_root)) else { return Err(ask) };
    let (description, options, cancelable) = question(&ask, source, in_session);
    match popup_runtime::Runtime::load(source, &description, options, cancelable) {
        Ok(mut runtime) => {
            // C9: these can come up under a player typing or skating (a Switch Team ask, a notice, an invite).
            runtime.hold(popup_hold(&ask));
            ui.popup = Some((ask, runtime));
            ui.modal = true;
            ui.accumulated = 0.;
            Ok(())
        }
        Err(error) => {
            error!("Online confirm popup: {error}");
            Err(ask)
        }
    }
}

const YES_NO: &[&str] = &["ID_COMMON_YES", "ID_COMMON_NO"];
const NO_YES: &[&str] = &["ID_COMMON_NO", "ID_COMMON_YES"];
/// The answer to Quit Session's question that leaves.
const QUIT_YES: usize = 1;
/// Ask::Restart's question; retail has no such setting, so no retail string.
const RESTART_TEXT: &str = "Performance Mode needs a restart.\nRestart the game now?";
/// What [`ask`] shows for `ask`: the description, the option ids in order, and whether B answers it.
fn question(ask: &Ask, source: &Json, in_session: bool) -> (String, &'static [&'static str], bool) {
    let language = &source["language"];
    match ask {
        Ask::Quit => (steam_text(language, "ID_ONLINE_CONFIRM_QUIT_ONLINE"), NO_YES, true),
        Ask::Restart(..) => (RESTART_TEXT.to_owned(), NO_YES, true),
        Ask::Kick { name, .. } => (kick_text(source, name), YES_NO, true),
        Ask::Connecting => (steam_text(language, "ID_ONLINE_SIGNING_IN"), &[][..], true),
        Ask::Matching => (steam_text(language, "ID_ONLINE_MATCHING_SKATERS"), &[][..], true),
        Ask::Notice(text) => (text.clone(), &["ID_COMMON_OK"][..], false),
        Ask::SignOut if in_session => (steam_text(language, "ID_ONLINE_GAME_SIGNOUT_MESSAGE"), NO_YES, false),
        Ask::SignOut => (steam_text(language, "ID_ONLINESETTINGS_SIGNOUT_CONFIRM_MESSAGE"), NO_YES, false),
        // Not retail (C9; retail popup 55 is Yes / No): No first, so a stray A never takes this player online.
        Ask::Invite(_) => (steam_text(language, "ID_ONLINE_SIGNIN_GAME_INVITE"), NO_YES, false),
        Ask::ConfirmSwitch => (steam_text(language, "ID_ONLINE_CONFIRM_SWITCH_TEAM"), YES_NO, true),
        Ask::Switch(_, name) => (
            format!(
                "{}\n{}",
                steam_text(language, "ID_ONLINE_EVENT_SWITCH_TEAM_PROPOSED"),
                steam_text(language, "ID_ONLINE_EVENT_SWITCH_TEAM_PROPOSED_PLAYER").replace("{0:s}", name)
            ),
            ACCEPT_DECLINE,
            true,
        ),
    }
}

/// An OK popup: retail ID_ONLINE_SIGNIN_CANCELLED, ID_ONLINE_DISCONNECT_BY_USER, or literal text; `{0:d}` filled
/// with `number`.
fn notice_with(ui: &mut OnlineUi, asset_root: &Path, id: &str, number: usize) {
    let source = ui.source.get_or_insert_with(|| load_source(asset_root));
    let text = if id.starts_with("ID_") { steam_text(source.as_ref().map_or(&Json::Null, |s| &s["language"]), id) } else { id.to_owned() };
    let text = text.replace("{0:d}", &number.to_string());
    if let Err(Ask::Notice(text)) = ask(ui, asset_root, Ask::Notice(text)) {
        warn!("Online notice: {text}");
    }
}
fn notice(ui: &mut OnlineUi, asset_root: &Path, id: &str) {
    notice_with(ui, asset_root, id, 0);
}

/// An Online Challenge Map row picked in a session. Spot battles and every other challenge hold 10 players
/// (`capacity`): a bigger freeskate session cannot go there.
fn request_move(
    ui: &mut OnlineUi,
    root: &Path,
    (target, capacity): (skate_net::roster::Target, usize),
    net: &mut Multiplayer,
    roster: &mut OnlineRoster,
) {
    let cohost = net.cohost(&roster.state);
    let state = &mut roster.state;
    if roster.members.len() > capacity {
        notice_with(ui, root, "ID_ONLNE_TOO_MANY_PLAYERS", capacity);
    } else if state.moving.is_some() {
        notice(ui, root, "ID_ONLINE_MOVING_LOCATION");
    } else if hosting(net) {
        // The host's own pick overrides an open proposal.
        crate::online_moves::start_move(state, target);
    } else if cohost {
        // The host moves the session on it (online_moves intake); the move's event line follows.
        net.publish_proposal(&Propose { nonce: crate::multiplayer::unique(), target });
    } else if state.no_proposals {
        notice(ui, root, "ID_ONLINE_CANNOT_PROPOSE");
    } else if state.proposal.is_some() {
        notice(ui, root, "ID_ONLINE_CANNOT_PROPOSE_BUSY");
    } else if net.publish_proposal(&Propose { nonce: crate::multiplayer::unique(), target }) {
        // ponytail: retail's overlay; the host's popup and everyone's event log follow.
        notice(ui, root, "ID_ONLINE_OVERLAY_CHALLENGE_PROPOSED");
    }
}

/// Signs in (unless signed in already), then does `then`. Retail RequireSignIn: no question first.
fn sign_in(ui: &mut OnlineUi, asset_root: &Path, presence: &mut Presence, then: Then, now: f32) {
    if ui.signed_in {
        return unlock(ui, presence, then);
    }
    if ui.connecting.is_some() {
        return;
    }
    presence.connect();
    ui.connecting = Some((now, then));
    if ui.popup.is_none() {
        // An unavailable popup still lets the attempt run, unseen.
        let _ = ask(ui, asset_root, Ask::Connecting);
    }
}

fn unlock(ui: &mut OnlineUi, presence: &mut Presence, then: Then) {
    match then {
        Then::ChallengeMap => ui.open_challenge_map = true,
        Then::Request(request) => ui.requests.push(request),
        Then::Join(connect) => presence.join_requests.extend(connect),
        Then::CloseCrossbar => ui.close_crossbar = true,
    }
}

/// How a sign-in attempt stands after `shown` seconds: `Some(true)` signed in, `Some(false)` failed, `None` still
/// connecting.
pub(crate) fn sign_in_result(shown: f32, available: bool, failed: bool) -> Option<bool> {
    if shown < SIGN_IN_SHOWN {
        None
    } else if available {
        Some(true)
    } else if failed || shown >= SIGN_IN_TIMEOUT {
        Some(false)
    } else {
        None
    }
}

/// Sign out (retail popup 33's Yes): leave any session, then retail popup 89.
fn sign_out(ui: &mut OnlineUi, root: &Path, net: &mut Multiplayer, session: &mut OnlineSession) {
    net.leave_steam();
    session.pending = None;
    session.cancel_browse();
    ui.signed_in = false;
    ui.close_crossbar = true;
    notice(ui, root, "ID_ONLINE_DISCONNECT_BY_USER");
}

fn confirm(ask: Ask, ui: &mut OnlineUi, net: &mut Multiplayer, roster: &mut OnlineRoster, session: &mut OnlineSession) {
    match ask {
        Ask::Quit => {
            net.leave();
            session.pending = None;
            // A search started from the Challenge Map in the session, and its row's area, end with it (as sign_out).
            session.cancel_browse();
            session.area = None;
            ui.close_crossbar = true;
        }
        // Multiplayer::kick records the kick in the session state; republish it. A co-host asks the host to.
        Ask::Kick { actor, .. } => {
            if !hosting(net) {
                if net.cohost(&roster.state) {
                    net.publish_cohost(skate_net::roster::CoHostAsk::Kick(actor));
                }
            } else if net.kick(&mut roster.state, actor).is_some() {
                net.publish_session(&roster.state);
                // The kick-confirm callback 0x8262cf80, on success: ShowCentralMessage(ID_CENTRALMESSAGE_PLAYERKICKED).
                roster.central = Some("ID_CENTRALMESSAGE_PLAYERKICKED".into());
            }
        }
        // Yes: the host switches at once (it has the final say); a guest proposes it, as retail's Switch Team does.
        Ask::ConfirmSwitch if roster.state.challenge.as_ref().is_some_and(|c| c.team) => {
            if hosting(net) {
                crate::online_moves::switch_team(&mut roster.state, roster.local_actor);
            } else if roster.state.switching.is_some() {
                ui.requests.push(OnlineRequest::Notice("ID_ONLINE_CANNOT_PROPOSE_BUSY"));
            } else if net.publish_switch() {
                // No log line: ID_ONLINE_EVENT_SWITCH_TEAM_YOU is not in TU3.
                ui.requests.push(OnlineRequest::Notice("ID_ONLINE_EVENT_SWITCH_TEAM_PROPOSED"));
            }
        }
        // Accept.
        Ask::Switch(actor, _) => {
            if roster.state.switching == Some(actor) && hosting(net) {
                crate::online_moves::switch_team(&mut roster.state, actor);
            }
        }
        Ask::Restart(setting, index) => ui.restart = Some((setting, index)),
        // Answered in interact (Switch Team outside a team lobby: nothing to switch).
        Ask::Connecting | Ask::Notice(_) | Ask::SignOut | Ask::Invite(_) | Ask::ConfirmSwitch | Ask::Matching => {}
    }
}

#[derive(Debug, PartialEq)]
enum Search {
    Show,
    NoMatch,
    Close,
    Keep,
}
/// What online_session's search popup (`popup`) needs from the open one (`open`): Matching waits for a free screen,
/// No Match replaces Matching, and a Matching the search dropped (a match joined, a session change) closes.
fn search(popup: Option<Popup>, open: Option<&Ask>) -> Search {
    let matching = matches!(open, Some(Ask::Matching));
    match popup {
        Some(Popup::Matching) if open.is_none() => Search::Show,
        Some(Popup::NoMatch) if open.is_none() || matching => Search::NoMatch,
        None if matching => Search::Close,
        _ => Search::Keep,
    }
}

/// We host a session (Kick and Make Host are host-only).
fn hosting(net: &Multiplayer) -> bool {
    let (online, _, host) = net.mod_identity();
    online && host
}
fn list_context(kind: ListKind, net: &Multiplayer, presence: &Presence, roster: &OnlineRoster) -> Context {
    Context {
        kind,
        host: hosting(net),
        steam_session: net.is_steam(),
        presence: presence.available,
        cohost: net.cohost(&roster.state),
        cohosts: roster.state.cohosts.clone(),
        teams: roster.state.challenge.as_ref().is_some_and(|c| c.team),
        shield: player_list::snapshot(),
    }
}
fn list_rows(kind: ListKind, roster: &OnlineRoster, presence: &Presence) -> Vec<player_list::Row> {
    let mut rows = player_list::rows(kind, &roster.members, roster.local_actor, &presence.friends, &roster.recent.players);
    player_list::mark_talkers(&mut rows, crate::voice::talking);
    rows
}
/// Voice chat: who in the session is talking (their rows carry a lit mic, so a change rebuilds the rows).
fn talkers(roster: &OnlineRoster) -> Vec<u64> {
    roster.members.iter().map(|m| m.actor).filter(|&actor| crate::voice::talking(actor)).collect()
}
/// The server browser's rows but our own session, its modes in sk8popup's retail strings.
fn lobby_rows(ui: &mut OnlineUi, root: &Path, room: Option<u64>) -> Vec<player_list::Row> {
    let language = ui.source.get_or_insert_with(|| load_source(root)).as_ref().map(|s| &s["language"]);
    ui.browser.rows(room, |id| language.filter(|l| l[id].is_string()).map(|l| steam_text(l, id)))
}

/// What a picked Player List action does. True: close the list and the crossbar (Skate with).
fn act(action: PlayerAction, person: player_list::Person, ui: &mut OnlineUi, net: &mut Multiplayer, presence: &mut Presence) -> bool {
    use PlayerAction::*;
    let order = match (action, person.actor) {
        (ViewSteamProfile, _) => Order::Open(Page::Profile(person.steam)),
        (SendFriendInvite, _) => Order::Open(Page::AddFriend(person.steam)),
        (SendMessage, _) => Order::Open(Page::Chat(person.steam)),
        // The session's own lobby: a private one advertises none (audit 10-24).
        (SendInvite, _) => Order::Invite {
            friend: person.steam,
            connect: net.room_id().map(|lobby| skate_net::presence::connect_string(lobby, &net.map_name)),
        },
        // The Task 3 join path (online_session) leaves, travels and joins.
        (SkateWith, _) => {
            presence.join_requests.extend(person.connect);
            return true;
        }
        // Asks ID_MESSENGER_KICK_CONFIRM first (ruling 7).
        (Kick, Some(actor)) => {
            ui.requests.push(OnlineRequest::Kick { actor, name: person.name });
            return false;
        }
        (MakeHost, Some(actor)) => {
            net.make_host(actor);
            return false;
        }
        (MakeCoHost | RemoveCoHost, Some(actor)) => {
            ui.requests.push(OnlineRequest::CoHost { actor, on: action == MakeCoHost });
            return false;
        }
        (SwitchTeam, Some(actor)) => {
            ui.requests.push(OnlineRequest::MoveTeam(actor));
            return false;
        }
        (HideGraphics | ShowGraphics, Some(actor)) => {
            crate::custom_graphic::toggle_hidden(actor);
            return false;
        }
        (Mute | Unmute | HidePlayer | ShowPlayer | Block | Unblock, _) => {
            player_list::pick(action, &person);
            return false;
        }
        // Greyed: no voice, skate profiles or teams.
        _ => return false,
    };
    presence.order(order);
    false
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn interact(
    mut ui: ResMut<OnlineUi>,
    config: Res<Config>,
    (keys, nav): (Res<ButtonInput<KeyCode>>, Res<Navigation>),
    mut net: ResMut<Multiplayer>,
    mut roster: ResMut<OnlineRoster>,
    mut session: ResMut<OnlineSession>,
    mut presence: ResMut<Presence>,
    mut screens: Option<ResMut<Screens>>,
    mut sounds: MessageWriter<FrontEndSoundRequest>,
    time: Res<Time<Real>>,
    mut typing: MessageReader<bevy::input::keyboard::KeyboardInput>,
    (menu, challenge, playable, lots, chat): (
        Res<crate::graphics_menu::Menu>,
        Option<Res<crate::challenge_map::ChallengeMap>>,
        Res<crate::match_core::Playable>,
        Option<Res<crate::skate3_otl::OtlLots>>,
        Option<Res<crate::online_chat::Chat>>,
    ),
) {
    let ui = &mut *ui;
    let now = time.elapsed_secs();
    let root = &config.asset_root;
    ui.closed_this_frame = false;
    // What the PC keyboard typed this frame (read every frame, so nothing typed before the keyboard opened arrives).
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let typed: String = (typing.read())
        .filter(|e| e.state == bevy::input::ButtonState::Pressed && !control)
        .filter_map(|e| e.text.as_deref())
        .flat_map(str::chars)
        .filter(|c| !c.is_control())
        .collect();
    let key = net.session().map(|s| (s.session, s.local));
    if key != ui.session {
        ui.session = key;
        // Kicked, host gone, Quit Session, a join: the list, the popup and the crossbar's tables are stale.
        ui.close_all(screens.as_deref_mut());
        ui.close_crossbar = true;
    } else if std::mem::take(&mut ui.dismiss) {
        ui.close_all(screens.as_deref_mut());
    }
    // Steam went away under us (closed, signed out): retail's lost-connection line.
    if ui.signed_in && !presence.available {
        ui.signed_in = false;
        roster.lines.push(("ID_ONLINE_DISCONNECT_FROM_EA_SERVER", String::new()));
        // Audit 10-24: a Steam session and its searches end with it (the session kept running signed out).
        if net.is_steam() {
            net.leave_steam();
        }
        // As sign_out: a queued pick would start the relay again, signed out.
        session.pending = None;
        session.cancel_browse();
    }
    if let Some((since, _)) = &ui.connecting
        && let Some(ok) = sign_in_result(now - since, presence.available, presence.error.is_some())
    {
        let (_, then) = ui.connecting.take().expect("checked above");
        if matches!(ui.popup, Some((Ask::Connecting, _))) {
            ui.popup = None;
            ui.closed_this_frame = true;
        }
        if ok {
            ui.signed_in = true;
            unlock(ui, &mut presence, then);
        } else {
            warn!("Steam sign-in failed: {}", presence.error.as_deref().unwrap_or("timed out"));
            let missing = presence.error.as_deref() == Some(crate::presence::NO_HELPER);
            notice(ui, root, if missing { HELPER_MISSING } else { SIGN_IN_FAILED });
        }
    }
    if let Some(then) = ui.sign_in_for.take() {
        sign_in(ui, root, &mut presence, then, now);
    }
    // A friend's "Join Game" or an accepted invite while signed out (retail popup 55); online_session waits.
    if !ui.signed_in && ui.connecting.is_none() && ui.popup.is_none() && !presence.join_requests.is_empty() {
        let connect = std::mem::take(&mut presence.join_requests);
        if let Err(Ask::Invite(connect)) = ask(ui, root, Ask::Invite(connect)) {
            sign_in(ui, root, &mut presence, Then::Join(connect), now);
        }
    }
    for request in std::mem::take(&mut ui.requests) {
        let asking = match request {
            // Retail gates every Contacts entry (and here the Friends extra) behind sign-in; Player List is in-session.
            OnlineRequest::List(kind) if kind != ListKind::Session && !ui.signed_in => {
                sign_in(ui, root, &mut presence, Then::Request(request), now);
                continue;
            }
            OnlineRequest::SignInOut if ui.signed_in => Ask::SignOut,
            OnlineRequest::SignInOut => {
                sign_in(ui, root, &mut presence, Then::CloseCrossbar, now);
                continue;
            }
            OnlineRequest::QuitSession => Ask::Quit,
            // Only the host and co-hosts may kick (the host checks a co-host's again).
            OnlineRequest::Kick { .. } if !hosting(&net) && !net.cohost(&roster.state) => continue,
            // online_roster publishes the change; online_moves keeps the list to players still here.
            OnlineRequest::CoHost { actor, on } => {
                let member = roster.members.iter().any(|m| m.actor == actor && !m.host);
                let cohosts = &mut roster.state.cohosts;
                if hosting(&net) && member {
                    if !on {
                        cohosts.retain(|a| *a != actor);
                    } else if cohosts.len() >= skate_net::roster::COHOSTS_MAX {
                        notice(ui, root, &format!("A session has at most {} co-hosts", skate_net::roster::COHOSTS_MAX));
                    } else if !cohosts.contains(&actor) {
                        cohosts.push(actor);
                    }
                }
                continue;
            }
            OnlineRequest::Kick { actor, name } => Ask::Kick { actor, name },
            OnlineRequest::TogglePrivacy => {
                let private = !roster.state.private;
                if net.set_private(private) {
                    roster.state.private = private;
                    net.publish_session(&roster.state);
                }
                continue;
            }
            // online_roster publishes the change.
            OnlineRequest::ToggleProposals => {
                if hosting(&net) {
                    roster.state.no_proposals = !roster.state.no_proposals;
                }
                continue;
            }
            OnlineRequest::ToggleCollisions => {
                if hosting(&net) {
                    roster.state.collisions = !roster.state.collisions;
                }
                continue;
            }
            // multiplayer publishes the listing to the lobby (Multiplayer::publish_details).
            OnlineRequest::ToggleListed => {
                if hosting(&net) {
                    net.details.listed = !net.details.listed;
                }
                continue;
            }
            OnlineRequest::EditDetails => {
                if hosting(&net) && net.is_steam() {
                    open_keyboard(ui, root, crate::session_details::Keyboard::open(crate::session_details::Field::Name, &net.details));
                }
                continue;
            }
            OnlineRequest::SearchSettings(words) => {
                open_keyboard(ui, root, crate::session_details::Keyboard::search(&words));
                continue;
            }
            OnlineRequest::Restart(setting, index) => Ask::Restart(setting, index),
            OnlineRequest::Move(target, capacity) => {
                request_move(ui, root, (target, capacity), &mut net, &mut roster);
                continue;
            }
            // online_roster publishes the match; match_core runs it.
            OnlineRequest::StartChallenge | OnlineRequest::StartActivity(_) => {
                let activity = match request {
                    OnlineRequest::StartActivity(i) => {
                        let lot = roster.state.challenge.as_ref().and_then(|c| c.row.as_ref()).map(|r| r.id.clone());
                        lots.as_deref().zip(lot).and_then(|(l, lot)| Some(l.0.get(&lot)?.get(i)?.0.clone()))
                    }
                    _ => None,
                };
                let players: Vec<u64> = roster.members.iter().map(|m| m.actor).collect();
                if hosting(&net) {
                    let cap = crate::match_core::lobby_cap(&roster.state);
                    let ready = crate::match_core::readies(&net);
                    match crate::match_core::start_game(&mut roster.state, &players, &ready, cap, &playable, activity.as_deref()) {
                        Ok(()) => {}
                        Err(Some((id, number))) => notice_with(ui, root, id, number),
                        Err(None) => warn!("Start Challenge: this lobby has no challenge to start"),
                    }
                }
                ui.close_crossbar = true;
                continue;
            }
            OnlineRequest::Notice(id) => {
                notice(ui, root, id);
                continue;
            }
            OnlineRequest::SwitchTeam => Ask::ConfirmSwitch,
            // online_roster publishes the change.
            OnlineRequest::MoveTeam(actor) => {
                if hosting(&net) && roster.state.challenge.as_ref().is_some_and(|c| c.team) && roster.members.iter().any(|m| m.actor == actor) {
                    crate::online_moves::switch_team(&mut roster.state, actor);
                }
                continue;
            }
            OnlineRequest::List(kind) => {
                match (&ui.list, screens.as_deref_mut()) {
                    (None, Some(screens)) => {
                        let rows = if kind == ListKind::Lobbies {
                            ui.browser.open(&mut net, now);
                            lobby_rows(ui, root, net.room_id())
                        } else {
                            list_rows(kind, &roster, &presence)
                        };
                        let context = list_context(kind, &net, &presence, &roster);
                        match PlayerList::open(screens, kind, rows, context) {
                            Ok(list) => {
                                ui.list = Some(list);
                                ui.rows_from = (presence.generation, roster.generation, talkers(&roster));
                            }
                            Err(error) => error!("Player List: {error}"),
                        }
                    }
                    (None, None) => warn!("Player List: no screen layers (no UI camera); not opened"),
                    (Some(_), _) => {}
                }
                continue;
            }
        };
        match ask(ui, root, asking) {
            // Unavailable popup: sign out without asking, as Quit and Kick act.
            Err(Ask::SignOut) => sign_out(ui, root, &mut net, &mut session),
            Err(asking) => confirm(asking, ui, &mut net, &mut roster, &mut session),
            Ok(()) => {}
        }
    }
    // Quick Match's search popups (online_session): popup 43 while it looks, popup 106 when nothing matched.
    match search(session.popup, ui.popup.as_ref().map(|(a, _)| a)) {
        // Without the popup (warned once) the search runs unseen.
        Search::Show => {
            let _ = ask(ui, root, Ask::Matching);
        }
        Search::NoMatch => {
            session.popup = None;
            notice(ui, root, "ID_SIDEKICK_ONLINE_QUICK_MATCH_NO_MATCH");
        }
        Search::Close => ui.popup = None,
        Search::Keep => {}
    }
    // As core_menu maps them.
    let pressed = [
        (nav.pressed & 1 != 0, Key::Up),
        (nav.pressed & 2 != 0, Key::Down),
        (nav.pressed & 4 != 0, Key::Left),
        (nav.pressed & 8 != 0, Key::Right),
        (keys.just_pressed(KeyCode::Enter) || nav.pressed & 0x1000 != 0, Key::Accept),
        (keys.just_pressed(KeyCode::Backspace) || nav.pressed & 0x2000 != 0, Key::Back),
    ];
    let mut pressed = pressed.into_iter().filter(|(down, _)| *down).map(|(_, key)| key);
    // Not retail (user 10-13): the open proposal as a corner tab (crate::proposal_tab), for everyone but its proposer,
    // over play; it counts down and closes once answered, passed, overridden or expired (online_moves::votes decides
    // on the host). Retail's full-screen SignUp_Poster stopped play.
    let (active, local, _) = net.mod_identity();
    let open = roster.state.proposal.clone().filter(|_| active);
    match (&open, ui.proposal_seen) {
        (Some(p), Some((token, _))) if token == p.token => {}
        (Some(p), _) => ui.proposal_seen = Some((p.token, now)),
        (None, _) => ui.proposal_seen = None,
    }
    ui.proposal_left = ui.proposal_seen.map_or(0., |(_, at)| PROPOSAL_TIMEOUT - (now - at));
    let free = ui.popup.is_none() && ui.list.is_none() && ui.keyboard.is_none() && !challenge.as_ref().is_some_and(|c| c.active()) && (!menu.open || menu.crossbar);
    // A muted or blocked player's proposal shows no tab here (no vote).
    match open.filter(|p| p.actor != local && ui.answered != p.token && ui.proposal_left > 0. && !player_list::quiet(p.actor)) {
        Some(p) if ui.tab.as_ref().is_some_and(|t| t.proposal == p) => {}
        Some(p) => {
            ui.tab = proposal_tab(ui, root, challenge.as_deref(), &menu.map_labels(), &p);
            // No font to draw with: no vote, as without retail's popup.
            if ui.tab.is_none() {
                ui.answered = p.token;
            }
        }
        None => ui.tab = None,
    }
    // D-pad Left accepts (a vote), Right declines, while play has the pad; arrow keys too unless chat is typing.
    let playing = ui.popup.is_none() && ui.list.is_none() && !menu.open && !challenge.as_ref().is_some_and(|c| c.active());
    let typing = chat.is_some_and(|c| c.bar.is_some());
    let left = nav.dpad & 4 != 0 || (!typing && keys.just_pressed(KeyCode::ArrowLeft));
    let right = nav.dpad & 8 != 0 || (!typing && keys.just_pressed(KeyCode::ArrowRight));
    // C9: not in the tab's first moments (a d-pad press meant for a trick).
    let fresh = ui.proposal_seen.is_some_and(|(_, at)| held(at, now));
    if playing && !fresh && (left || right) && let Some(tab) = ui.tab.take() {
        ui.answered = tab.proposal.token;
        if left && roster.state.proposal.as_ref() == Some(&tab.proposal) {
            net.publish_accept(tab.proposal.token);
        }
    }
    // The host is asked about a guest's Switch Team proposal the same way; it declines after PROPOSAL_TIMEOUT.
    let switching = roster.state.switching.filter(|_| hosting(&net));
    if let Some((Ask::Switch(actor, _), _)) = &ui.popup {
        let current = switching == Some(*actor);
        if !current || now - ui.asked_at > PROPOSAL_TIMEOUT {
            ui.popup = None;
            if current {
                roster.state.switching = None;
            }
        }
    } else if switching.is_some_and(player_list::quiet) {
        // A muted or blocked guest's ask is declined unasked.
        roster.state.switching = None;
    } else if let Some(actor) = switching.filter(|_| ui.popup.is_none() && free) {
        let name = player_list::shown_name(&roster.members, actor).unwrap_or_default();
        match ask(ui, root, Ask::Switch(actor, name)) {
            Ok(()) => ui.asked_at = now,
            // No popup to ask with: decline.
            Err(_) => roster.state.switching = None,
        }
    }
    // The open server browser reads its pages and looks again when due; its rows are compared each frame (refresh
    // rebuilds the list only when they changed).
    let lobbies = ui.list.as_ref().is_some_and(|l| l.kind() == ListKind::Lobbies).then(|| {
        ui.browser.step(&mut net, now);
        lobby_rows(ui, root, net.room_id())
    });
    // The keyboard owns input while open: the PC keyboard types (Backspace erases, Enter is Done, Esc cancels), the pad
    // works its keys (text_entry::input). The server details' closes when we stop hosting (the session migrated or
    // ended); Settings search's and the server browser's aren't the session's.
    use crate::session_details::Field;
    let browsing = lobbies.is_some();
    if ui.keyboard.as_ref().is_some_and(|k| matches!(k.field, Field::Name | Field::Description)) && !hosting(&net) {
        ui.keyboard = None;
        ui.closed_this_frame = true;
    }
    if let Some(keyboard) = ui.keyboard.as_mut() {
        use crate::{boot_flow::Key as Pad, text_entry::Typed};
        keyboard.entry.type_text(&typed);
        if keys.just_pressed(KeyCode::Backspace) {
            keyboard.entry.backspace();
        }
        let mut answer = (keys.just_pressed(KeyCode::Enter).then_some(Typed::Done)).or(keys.just_pressed(KeyCode::Escape).then_some(Typed::Cancel));
        for (bit, key) in [(1, Pad::Up), (2, Pad::Down), (4, Pad::Left), (8, Pad::Right), (0x10, Pad::Start), (0x1000, Pad::A), (0x2000, Pad::B), (0x4000, Pad::X), (0x8000, Pad::Y)] {
            if nav.pressed & bit != 0 {
                answer = answer.or(keyboard.entry.input(key));
            }
        }
        match answer {
            Some(Typed::Done) if keyboard.field == Field::Search => {
                ui.searched = Some(keyboard.entry.text.trim().to_owned());
                ui.keyboard = None;
            }
            Some(Typed::Done) if keyboard.field == Field::Servers => {
                ui.browser.search = keyboard.entry.text.trim().to_owned();
                ui.keyboard = None;
            }
            // The name's Done goes on to the description's keyboard.
            Some(Typed::Done) => ui.keyboard = keyboard.apply(&mut net.details),
            Some(Typed::Cancel) => ui.keyboard = None,
            None => {}
        }
        ui.closed_this_frame |= ui.keyboard.is_none();
    } else if let Some((asking, runtime)) = ui.popup.as_mut() {
        // With no options only B answers (popup_runtime reads A as the highlighted row).
        let busy = matches!(asking, Ask::Connecting | Ask::Matching);
        // The popup (over the list, for Kick) owns input.
        let answer = pressed.find_map(|key| {
            let input = runtime.input(key);
            // Nav, A, and B on a cancelable popup, as create_skater plays them.
            sounds.write_batch(runtime.bindings.sounds.drain(..).map(FrontEndSoundRequest));
            input.transpose().filter(|a| !busy || !matches!(a, Ok(i) if *i != popup_runtime::CANCEL))
        });
        if let Some(answer) = answer {
            let (asking, _) = ui.popup.take().expect("answered above");
            ui.closed_this_frame = true;
            match (asking, answer) {
                (Ask::Connecting, Ok(_)) => {
                    // Retail popup 42; the pick is dropped.
                    ui.connecting = None;
                    notice(ui, root, "ID_ONLINE_SIGNIN_CANCELLED");
                }
                (Ask::SignOut, Ok(1)) => sign_out(ui, root, &mut net, &mut session),
                // B (the only answer it takes).
                (Ask::Matching, Ok(_)) => session.cancel_matching(&mut net),
                (Ask::Invite(connect), Ok(1)) => sign_in(ui, root, &mut presence, Then::Join(connect), now),
                (asking @ Ask::Quit, Ok(QUIT_YES)) | (asking @ Ask::Restart(..), Ok(1)) => {
                    confirm(asking, ui, &mut net, &mut roster, &mut session)
                }
                (asking @ (Ask::Kick { .. } | Ask::ConfirmSwitch | Ask::Switch(..)), Ok(0)) => {
                    confirm(asking, ui, &mut net, &mut roster, &mut session)
                }
                // Decline or B.
                (Ask::Switch(actor, _), Ok(_)) => {
                    if roster.state.switching == Some(actor) {
                        roster.state.switching = None;
                    }
                }
                // No, OK, or B (popup_runtime::CANCEL).
                (_, Ok(_)) => {}
                (_, Err(error)) => error!("Online popup stopped: {error}"),
            }
        }
    } else if let (Some(list), Some(screens)) = (ui.list.as_mut(), screens.as_deref_mut()) {
        let kind = list.kind();
        // Rows are rebuilt (friends sorted) only when friends, members or recent players changed.
        let from = (presence.generation, roster.generation, talkers(&roster));
        let rows = lobbies.or_else(|| (from != ui.rows_from).then(|| list_rows(kind, &roster, &presence)));
        ui.rows_from = from;
        let effect = if list.refresh(screens, rows, list_context(kind, &net, &presence, &roster)) {
            pressed.find_map(|key| list.input(screens, key))
        } else {
            // The screen stopped (logged when it did).
            Some(Effect::Closed)
        };
        // (close the list, close the crossbar too)
        let (close, crossbar) = match effect {
            Some(Effect::Act(action, person)) => {
                // Audit 10-24 (user: offline stays private): a friend whose game is not signed in gets Spacewar from Steam.
                if action == PlayerAction::SendInvite {
                    roster.lines.push(("{0:s}", INVITE_SENT.into()));
                }
                let skate = act(action, person, ui, &mut net, &mut presence);
                (skate, skate)
            }
            Some(Effect::Volume(person, volume)) => {
                player_list::set_volume(&person, volume);
                (false, false)
            }
            Some(Effect::Closed) => (true, false),
            None => (false, false),
        };
        if close {
            ui.list.take().expect("open above").close(screens);
            ui.closed_this_frame = true;
            ui.close_crossbar |= crossbar;
        }
    }
    // Not retail (user 10-27): the server browser's Y Search, X Refresh and LB / RB Sort (F, R and Q / E on a PC
    // keyboard) at its list with nothing over it; after the keyboard above, so the Y that opens it types no space.
    let at_list = ui.list.as_ref().is_some_and(|l| l.kind() == ListKind::Lobbies && l.at_root());
    if browsing && at_list && ui.keyboard.is_none() && ui.popup.is_none() && !ui.closed_this_frame {
        let hit = |bit: u16, key: KeyCode| nav.pressed & bit != 0 || keys.just_pressed(key);
        if hit(0x8000, KeyCode::KeyF) {
            let words = ui.browser.search.clone();
            open_keyboard(ui, root, crate::session_details::Keyboard::servers(&words));
        } else if hit(0x4000, KeyCode::KeyR) {
            ui.browser.refresh(&mut net, now);
        } else if hit(0x0100, KeyCode::KeyQ) || hit(0x0200, KeyCode::KeyE) {
            ui.browser.sort = ui.browser.sort.step(hit(0x0200, KeyCode::KeyE));
        }
    }
    ui.modal = ui.popup.is_some() || ui.list.is_some() || ui.keyboard.is_some();
}

/// The legend's spot and look: its right edge (mirroring the event log's left margin, mcEventLog x 116, as the chat's
/// does), its row's top, its glyph and words' height, and its dark backer.
pub(crate) const LEGEND_RIGHT: f32 = 1280. - 116.;
pub(crate) const LEGEND_TOP: f32 = 720. - 96.;
pub(crate) const LEGEND_SIZE: f32 = 18.;
pub(crate) const LEGEND_BACKER: [f32; 4] = [0.024, 0.04, 0.055, 0.85];
/// Not retail (user 10-24; TU3's Swap Camera shows no legend): one button's legend bottom right, in the proposal tab's
/// look (the movie's `glyph` and a Futura on its dark backer): spectating's "B Back" (B leaves:
/// match_core::spectate_back), and Game Settings' "Y Search" (core_menu, user 10-26).
pub(crate) fn legend(font: &crate::apt_text::Font, glyph: &str, words: &str) -> Vec<crate::apt_scene::Draw> {
    use crate::graphic_board::{button, colour, label, quad};
    let width = LEGEND_SIZE * 4. / 3.;
    let x = LEGEND_RIGHT - width - 8. - font.width(words, LEGEND_SIZE);
    let backer = [x - 10., LEGEND_TOP - 7., LEGEND_RIGHT + 10., LEGEND_TOP + LEGEND_SIZE + 7.];
    let mut out = vec![quad("runtime/white.rgba", [1, 1], backer, LEGEND_BACKER)];
    out.push(button(glyph, [x, LEGEND_TOP - 1.], LEGEND_SIZE + 1.5));
    out.extend(label(font, words, [x + width + 8., LEGEND_TOP], LEGEND_SIZE, colour(0xEBF2F8, 1.)));
    out
}

/// The on-screen keyboard over the crossbar, drawn in the spectate legend's Futura (loaded once).
fn open_keyboard(ui: &mut OnlineUi, root: &Path, keyboard: crate::session_details::Keyboard) {
    if ui.back.is_none() {
        ui.back = Some(spectate_words(ui, root));
    }
    match &ui.back {
        Some(Some(_)) => ui.keyboard = Some(keyboard),
        _ => warn!("{}: no sk8popup font to draw the keyboard with", keyboard.title()),
    }
}

/// The spectate legend's font and "Back", from sk8popup (None without it).
fn spectate_words(ui: &mut OnlineUi, root: &Path) -> Option<(crate::apt_text::Font, String)> {
    let source = ui.source.get_or_insert_with(|| load_source(root)).as_ref()?;
    let text = crate::apt_text::TextAssets::load(source).map_err(|e| error!("Spectate legend: {e}")).ok()?;
    let font = text.fonts.get(&text.font_named("Futura Std Medium")?)?.clone();
    Some((font, steam_text(&source["language"], "ID_COMMON_BACK")))
}

/// Advances the popup on real time and hands its draws to the online layer.
fn draw(
    mut ui: ResMut<OnlineUi>,
    screens: Option<ResMut<Screens>>,
    real: Res<Time<Real>>,
    (config, spectate, menu): (Res<Config>, Option<Res<crate::match_core::Spectate>>, Option<Res<crate::graphics_menu::Menu>>),
) {
    let Some(mut screens) = screens else { return };
    let layer = screens.layer(ONLINE);
    let ui = &mut *ui;
    let mut draws = None;
    if let Some((_, runtime)) = ui.popup.as_mut() {
        ui.accumulated = (ui.accumulated + real.delta_secs()).min(4. * FRAME);
        let mut result = Ok(());
        while ui.accumulated >= FRAME && result.is_ok() {
            ui.accumulated -= FRAME;
            result = runtime.advance();
        }
        draws = Some(result.and_then(|()| crate::apt_scene::draw(&runtime.bindings.movie, &runtime.vm, &runtime.bindings.shapes)));
    }
    // Under the popups.
    match &ui.tab {
        Some(tab) => {
            let [header, accept, decline, by] = &tab.words;
            let words = [accept.as_str(), decline.as_str(), by.as_str()];
            layer.extra.insert("y-proposal", crate::proposal_tab::draws(&tab.font, &tab.card, &tab.proposal.by, ui.proposal_left, header, words));
        }
        None => {
            layer.extra.remove("y-proposal");
        }
    }
    // Not under a menu or popup: their B is their own.
    let watching = spectate.is_some_and(|s| s.0.is_some()) && !ui.modal && crate::graphics_menu::gameplay_active(menu);
    if watching && ui.back.is_none() {
        ui.back = Some(spectate_words(ui, &config.asset_root));
    }
    match ui.back.as_ref().and_then(Option::as_ref).filter(|_| watching) {
        Some((font, back)) => {
            layer.extra.insert("y-spectate", legend(font, "B", back));
        }
        None => {
            layer.extra.remove("y-spectate");
        }
    }
    // Over the crossbar (user 10-25): Host Actions' server name and description keyboard, in sk8popup's Futura.
    match (&ui.keyboard, ui.back.as_ref().and_then(Option::as_ref)) {
        (Some(keyboard), Some((font, _))) => {
            layer.extra.insert("z-keyboard", crate::session_details::draws(font, keyboard));
        }
        _ => {
            layer.extra.remove("z-keyboard");
        }
    }
    // Keys draw in order: after the name tags.
    match draws {
        Some(Ok(draws)) => {
            layer.extra.insert("z-popup", draws);
        }
        Some(Err(error)) => {
            error!("Online confirm popup stopped: {error}");
            ui.popup = None;
            ui.modal = ui.list.is_some() || ui.keyboard.is_some();
            layer.extra.remove("z-popup");
        }
        None => {
            layer.extra.remove("z-popup");
        }
    }
    // The graphics menu (open under the crossbar) hides the layer unless it is modal.
    layer.modal = ui.modal;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// User 10-24: the spectate legend, B and the installed "Back", bottom right inside the stage. SKATE3_ASSET_ROOT
    /// = the install's assets; SKATE_ONLINE_UI_SHOTS saves spectate-back.png.
    #[test]
    #[ignore = "needs INST\\assets"]
    fn spectating_shows_b_back_bottom_right() {
        let assets = std::path::PathBuf::from(std::env::var("SKATE3_ASSET_ROOT").expect("SKATE3_ASSET_ROOT"));
        let (font, back) = spectate_words(&mut OnlineUi::default(), &assets).expect("sk8popup");
        assert_eq!(back, "Back");
        let draws = legend(&font, "B", &back);
        assert!(draws.iter().any(|d| d.texture == "runtime/buttons/B.rgba"), "the B glyph");
        for v in draws.iter().flat_map(|d| &d.vertices) {
            assert!((640. ..1280.).contains(&v.position[0]) && (540. ..720.).contains(&v.position[1]), "bottom right: {:?}", v.position);
        }
        if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
            crate::core_menu_runtime::tests::raster_draws(&draws, &assets.join("private/online-ui"), &std::path::Path::new(&out).join("spectate-back.png"));
        }
    }

    #[test]
    fn only_asks_the_player_made_hold_briefly() {
        assert_eq!(popup_hold(&Ask::Quit), ASKED_HOLD);
        assert_eq!(popup_hold(&Ask::SignOut), ASKED_HOLD);
        assert_eq!(popup_hold(&Ask::Notice(String::new())), POPUP_HOLD);
        assert_eq!(popup_hold(&Ask::Switch(1, String::new())), POPUP_HOLD);
    }

    #[test]
    fn a_fresh_proposal_tab_holds_its_answer_for_half_a_second() {
        assert!(held(10., 10.) && held(10., 10.49));
        assert!(!held(10., 10.51) && !held(10., 30.));
    }

    #[test]
    fn crossbar_picks_become_requests() {
        use OnlineAction::*;
        assert_eq!(request(PlayerList, None), Some(OnlineRequest::List(ListKind::Session)));
        assert_eq!(request(Friends, None), Some(OnlineRequest::List(ListKind::Friends)));
        assert_eq!(request(Contacts, Some(2)), Some(OnlineRequest::List(ListKind::Recent)));
        assert_eq!(request(Contacts, Some(3)), Some(OnlineRequest::List(ListKind::InviteFriend)));
        assert_eq!(request(HostActions, Some(1)), Some(OnlineRequest::TogglePrivacy));
        assert_eq!(request(HostActions, Some(0)), Some(OnlineRequest::ToggleProposals));
        assert_eq!(request(HostActions, Some(2)), None, "Session Difficulty opens Game Settings");
        assert_eq!(request(HostActions, Some(5)), Some(OnlineRequest::ToggleCollisions));
        assert_eq!((request(HostActions, Some(6)), request(HostActions, Some(7))), (Some(OnlineRequest::ToggleListed), Some(OnlineRequest::EditDetails)));
        assert_eq!(request(QuitSession, None), Some(OnlineRequest::QuitSession));
    }

    #[test]
    fn sign_in_waits_then_succeeds_fails_or_times_out() {
        assert_eq!(sign_in_result(0.1, true, false), None, "Connecting stays up a moment even when Steam is open");
        assert_eq!(sign_in_result(SIGN_IN_SHOWN, true, false), Some(true));
        assert_eq!(sign_in_result(SIGN_IN_SHOWN, false, true), Some(false), "Steam not open");
        assert_eq!(sign_in_result(10., false, false), None, "still waiting for the helper");
        assert_eq!(sign_in_result(SIGN_IN_TIMEOUT, false, false), Some(false), "retail's 30 s timeout");
    }

    /// Retail's Quit Session question is popup 63 (0x8261C7AC: li r3,0x3F; record 0x83028E18):
    /// ID_ONLINE_CONFIRM_QUIT_ONLINE with No before Yes, and B cancels; its callback 0x8261D5A0 leaves on button 1.
    #[test]
    fn quit_session_asks_retails_popup_63_no_first() {
        let source = serde_json::json!({ "language": {
            "ID_ONLINE_CONFIRM_QUIT_ONLINE": "You are about to leave this online session. Do you want to continue?",
            "ID_SESSION_EVENT_MESSAGE": "Are you sure you want to leave this online session?",
        } });
        let (text, options, cancelable) = question(&Ask::Quit, &source, true);
        assert_eq!(text, "You are about to leave this online session. Do you want to continue?");
        assert_eq!((options, cancelable), (NO_YES, true));
        assert_eq!(QUIT_YES, options.iter().position(|o| *o == "ID_COMMON_YES").unwrap(), "Yes leaves");
    }

    #[test]
    fn retail_strings_read_steam() {
        let language = serde_json::json!({
            "ID_ONLINE_SIGNING_IN": "Connecting to EA Nation...",
            "ID_ONLINESETTINGS_SIGNOUT_CONFIRM_MESSAGE": "Are you sure you still want to sign out of EA Nation?  ",
        });
        assert_eq!(steam_text(&language, "ID_ONLINE_SIGNING_IN"), "Connecting to Steam...");
        assert_eq!(steam_text(&language, "ID_ONLINESETTINGS_SIGNOUT_CONFIRM_MESSAGE"), "Are you sure you still want to sign out of Steam?");
        assert_eq!(steam_text(&language, "ID_MISSING"), "ID_MISSING");
    }

    #[test]
    fn kick_question_names_the_player_on_two_lines() {
        let source = serde_json::json!({ "language": { KICK_CONFIRM: "Kick {0:s}?^No rejoining." } });
        assert_eq!(kick_text(&source, "#1 Bo"), "Kick #1 Bo?\nNo rejoining.");
        assert_eq!(kick_text(&source, "Bo^You are banned"), "Kick Bo\u{2060}^You are banned?\nNo rejoining.", "a name's ^ is no line break");
        assert_eq!(kick_text(&Json::Null, "Bo"), KICK_CONFIRM);
    }

    /// Item 30: Quick Match's popups. Matching waits for a free screen and No Match replaces it; a Matching the
    /// search dropped closes, and nothing else is touched.
    #[test]
    fn quick_match_popups_follow_the_search() {
        assert_eq!(search(Some(Popup::Matching), None), Search::Show);
        assert_eq!(search(Some(Popup::Matching), Some(&Ask::Matching)), Search::Keep);
        assert_eq!(search(Some(Popup::Matching), Some(&Ask::Quit)), Search::Keep, "waits for the open popup");
        assert_eq!(search(Some(Popup::NoMatch), Some(&Ask::Matching)), Search::NoMatch);
        assert_eq!(search(Some(Popup::NoMatch), None), Search::NoMatch);
        assert_eq!(search(Some(Popup::NoMatch), Some(&Ask::Quit)), Search::Keep);
        assert_eq!(search(None, Some(&Ask::Matching)), Search::Close, "matched, or the session changed");
        assert_eq!(search(None, Some(&Ask::Quit)), Search::Keep);
        assert_eq!(search(None, None), Search::Keep);
    }

    /// Retail popup 38 with Steam text: no options (no picker, no A legend), B cancels. PNGs to SKATE_ONLINE_UI_SHOTS.
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn connecting_popup_has_no_options_and_b_cancels() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let source: Json = serde_json::from_slice(&std::fs::read(root.join("runtime/sk8popup.json")).unwrap()).unwrap();
        let text = steam_text(&source["language"], "ID_ONLINE_SIGNING_IN");
        assert_eq!(text, "Connecting to Steam...");
        // Item 30: popup 43 (Matching, as popup 38) and the OK popups 106 / 64 / 67 / 74 / 104 / 110.
        let matching = steam_text(&source["language"], "ID_ONLINE_MATCHING_SKATERS");
        assert_eq!(matching, "Matching up with skaters...");
        let oks: Vec<String> = [
            "ID_SIDEKICK_ONLINE_QUICK_MATCH_NO_MATCH",
            "ID_ONLINE_KICKED_BY_GAME_HOST",
            "ID_ONLINE_GAME_LEFT",
            "ID_ONLINE_DISCONNECT_FROM_EA_SERVER",
            "ID_ONLINE_PLAYNOW_ERROR_STARTING",
            "ID_JOIN_ONLINE_GAME_FAILED_POPUP_MESSAGE",
        ]
        .iter()
        .map(|id| steam_text(&source["language"], id))
        .collect();
        assert!(oks.iter().all(|t| !t.starts_with("ID_") && !t.contains("EA Nation")), "{oks:?}");
        let mut popups = vec![
            ("connecting", text.as_str(), &[][..], true),
            ("matching", matching.as_str(), &[][..], true),
            ("signin-failed", SIGN_IN_FAILED, &["ID_COMMON_OK"][..], false),
        ];
        popups.extend(oks.iter().map(|t| ("notice", t.as_str(), &["ID_COMMON_OK"][..], false)));
        for (name, text, options, cancelable) in popups {
            let mut popup = popup_runtime::Runtime::load(&source, text, options, cancelable).unwrap();
            for _ in 0..40 {
                popup.advance().unwrap();
            }
            if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
                let draws = crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes).unwrap();
                crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join(format!("{name}.png")));
            }
            if cancelable {
                assert_eq!(popup.input(Key::Back).unwrap(), Some(popup_runtime::CANCEL));
            }
        }
    }

    /// The popup as interact opens it: the description is drawn as given (SKATE_ONLINE_UI_ROOT = private/online-ui).
    #[test]
    #[ignore = "needs INST\\assets\\private\\online-ui"]
    fn kick_popup_shows_the_question() {
        let root = std::path::PathBuf::from(std::env::var("SKATE_ONLINE_UI_ROOT").expect("SKATE_ONLINE_UI_ROOT"));
        let source: Json = serde_json::from_slice(&std::fs::read(root.join("runtime/sk8popup.json")).unwrap()).unwrap();
        let text = kick_text(&source, "Bo");
        let mut popup = popup_runtime::Runtime::load(&source, &text, &["ID_COMMON_YES", "ID_COMMON_NO"], true).unwrap();
        for _ in 0..40 {
            popup.advance().unwrap();
        }
        let shown: Vec<&String> = (popup.vm.objects.iter())
            .filter_map(|o| match o.fields.get("_displayText") {
                Some(crate::apt_vm::Value::Text(t)) => Some(t),
                _ => None,
            })
            .collect();
        assert!(shown.iter().any(|t| t.contains("kick Bo?\n")), "{shown:?}");
        if let Ok(out) = std::env::var("SKATE_ONLINE_UI_SHOTS") {
            let draws = crate::apt_scene::draw(&popup.bindings.movie, &popup.vm, &popup.bindings.shapes).unwrap();
            crate::core_menu_runtime::tests::raster_draws(&draws, &root, &std::path::Path::new(&out).join("kick-popup.png"));
        }
        assert_eq!(popup.input(Key::Back).unwrap(), Some(popup_runtime::CANCEL), "cancelable");
    }
}
