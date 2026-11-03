//! Discord Rich Presence: what the player is doing, the map, and online the mode and the session's player count.
//! Talks to the local Discord client over its IPC pipe (frames of op u32 LE, length u32 LE, JSON); without Discord
//! running it does nothing. A worker thread owns the pipe, so a slow or absent Discord never stalls a frame.
// ponytail: Windows named pipe only; Linux/macOS would open the unix socket $XDG_RUNTIME_DIR/discord-ipc-0 instead.
use crate::{match_core::Mode, multiplayer::Multiplayer, online_roster::OnlineRoster};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The Discord application ("Playing <app name>"); a public id, not a secret.
const APP_ID: &str = "1555758390033125387";
/// Art Assets key of the large image (uploaded in the Discord developer portal), and its hover text.
const LOGO: &str = "logo";
const TITLE: &str = "S3R";
/// The activity's one button (user 10-29): the project's Discord invite, the launcher's `DISCORD` (skate-launcher app.rs).
const DISCORD: &str = "https://discord.gg/PC2W9K4xan";
/// The worker re-sends the activity this often (reconnecting if Discord restarted); Discord allows 5 updates per 20 s.
const REFRESH: Duration = Duration::from_secs(15);
const MIN_GAP: Duration = Duration::from_secs(4);
/// Discord's replies echo the activity: a few kB. A knob.
const MAX_REPLY: usize = 64 * 1024;

/// What the presence shows: Discord's two text lines, and the party size bubble.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Status {
    pub details: String,
    pub state: String,
    pub party: Option<[usize; 2]>,
}

pub(crate) enum Doing<'a> {
    Offline(&'a str),
    Online { mode: &'a str, players: usize, capacity: usize, private: bool },
}

pub(crate) fn status(map: &str, doing: Doing) -> Status {
    let map = map.replace('_', " ");
    match doing {
        Doing::Offline(what) => Status { details: what.into(), state: map, party: None },
        // A private session shows neither where nor with how many.
        Doing::Online { mode, private: true, .. } => Status { details: mode.into(), state: "Private session".into(), party: None },
        Doing::Online { mode, players, capacity, .. } => {
            Status { details: mode.into(), state: map, party: Some([players, capacity.max(players)]) }
        }
    }
}

fn mode_text(mode: Mode) -> &'static str {
    match mode {
        Mode::DeathRace => "Death Race",
        Mode::HallOfMeat => "Hall of Meat",
        Mode::SpotBattle => "Spot Battle",
        Mode::BestTrick => "Best Trick",
        Mode::Jam => "Jam",
        Mode::Skate => "S.K.A.T.E.",
        Mode::OneUp => "1-Up",
        Mode::SpotRace => "Spot Race",
        Mode::Domination => "Domination",
        Mode::Contest => "Contest",
        Mode::OwnTheSpot => "Own the Spot",
        Mode::OwnTheLot => "Own the Lot",
        Mode::Film => "Film",
        Mode::Photo => "Photo",
    }
}

/// A challenge lobby's type (its Online Challenge Map folder filter).
fn challenge_text(title: &str, team: bool) -> &'static str {
    if let Some(mode) = Mode::from_filter(title) {
        return mode_text(mode);
    }
    match title.strip_prefix("ID_MAP_FILTER_CHALLENGETYPES_") {
        Some("DOMINATION") => "Domination",
        Some("STREET") => "Street Contest",
        Some("VERT") => "Vert Contest",
        _ if team => "Online Team Challenge",
        _ => "Online Challenge",
    }
}

pub(crate) struct DiscordPresencePlugin;
impl Plugin for DiscordPresencePlugin {
    fn build(&self, app: &mut App) {
        if crate::hero_clip::active(app) {
            return; // the launcher's hidden clip run must not touch the player's Discord status
        }
        let (tx, rx) = channel();
        if let Err(e) = std::thread::Builder::new().name("discord-presence".into()).spawn(move || worker(rx)) {
            warn!("Discord presence: {e}");
            return;
        }
        app.insert_resource(Discord { tx, sent: None, since: 0, on: true }).add_systems(Update, (switch, update).chain());
    }
}

#[derive(Resource)]
struct Discord {
    /// An activity, or None: clear it and let go of the pipe (Settings > Audio > Discord Status off).
    tx: Sender<Option<Value>>,
    sent: Option<Status>,
    /// Unix seconds the current activity began (Discord's elapsed timer).
    since: u64,
    /// Settings > Audio > Discord Status.
    on: bool,
}

/// Discord Status off: clear what Discord shows (once) and send nothing more; the pipe is never opened while off.
fn switch(mut discord: ResMut<Discord>, menu: Option<Res<crate::graphics_menu::Menu>>) {
    let on = menu.is_none_or(|m| m.discord_status());
    if !on && discord.sent.take().is_some() {
        let _ = discord.tx.send(None);
    }
    discord.on = on;
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut discord: ResMut<Discord>,
    net: Res<Multiplayer>,
    roster: Res<OnlineRoster>,
    map: Res<crate::map_transition::CurrentMap>,
    transition: Res<crate::map_transition::MapTransition>,
    m: Res<crate::match_core::Match>,
    customiser: Option<Res<crate::customiser::Customiser>>,
    lesson: Option<Res<crate::challenge::LessonMenu>>,
    time: Res<Time<Real>>,
    mut next: Local<f32>,
) {
    let now = time.elapsed_secs();
    // G1: no "Test world" status while the saved map loads behind the boot.
    if now < *next || !discord.on || transition.startup() {
        return;
    }
    *next = now + 1.;
    let s = &roster.state;
    let doing = if net.active() {
        let mode = match (&s.game, &s.challenge) {
            (Some(game), _) => Mode::parse(&game.mode).map_or("Online Challenge", mode_text),
            (None, Some(c)) => challenge_text(&c.title, c.team),
            (None, None) => "Online Freeskate",
        };
        let capacity = if s.challenge.is_some() { skate_net::roster::GAME_SLOTS } else { skate_net::lobby::MAX_PLAYERS };
        Doing::Online { mode, players: roster.members.len().max(1), capacity, private: s.private }
    } else if let Some(mode) = m.mode {
        Doing::Offline(mode_text(mode))
    } else if customiser.is_some_and(|c| c.open) {
        Doing::Offline("Customizing a skater")
    } else if lesson.is_some_and(|l| l.active) {
        Doing::Offline("Playing the tutorial")
    } else {
        Doing::Offline("Freeskating")
    };
    let status = status(&map.name, doing);
    if discord.sent.as_ref() == Some(&status) {
        return;
    }
    // The elapsed timer restarts when the activity or the map changes, not when players come and go.
    if discord.sent.as_ref().is_none_or(|old| (&old.details, &old.state) != (&status.details, &status.state)) {
        discord.since = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    }
    let mut activity = json!({
        "details": status.details,
        "state": status.state,
        "timestamps": { "start": discord.since },
        "assets": { "large_image": LOGO, "large_text": TITLE },
        "buttons": [{ "label": "Join the Discord", "url": DISCORD }],
    });
    if let (Some(size), Some(session)) = (status.party, net.session()) {
        activity["party"] = json!({ "id": format!("{:x}", session.session), "size": size });
    }
    let _ = discord.tx.send(Some(activity));
    discord.sent = Some(status);
}

fn frame(pipe: &mut File, op: u32, body: &Value) -> std::io::Result<()> {
    let body = body.to_string();
    let mut bytes = Vec::with_capacity(8 + body.len());
    bytes.extend(op.to_le_bytes());
    bytes.extend((body.len() as u32).to_le_bytes());
    bytes.extend(body.as_bytes());
    pipe.write_all(&bytes)
}

/// One reply frame; Discord answers every command, and an unread reply would fill the pipe.
fn reply(pipe: &mut File) -> std::io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    pipe.read_exact(&mut head)?;
    let op = u32::from_le_bytes(head[..4].try_into().unwrap());
    // Whatever owns the pipe name answers; a 4 GB length must not become a 4 GB allocation.
    let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
    if len > MAX_REPLY {
        return Err(std::io::Error::other("oversized Discord reply"));
    }
    let mut body = vec![0; len];
    pipe.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or_default()))
}

/// Discord listens on the first free of discord-ipc-0..9; the handshake answers READY (op 1) or closes (op 2).
fn connect() -> Option<File> {
    (0..10).find_map(|i| {
        let mut pipe = File::options().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok()?;
        frame(&mut pipe, 0, &json!({ "v": 1, "client_id": APP_ID })).ok()?;
        matches!(reply(&mut pipe), Ok((1, _))).then_some(pipe)
    })
}

fn worker(rx: Receiver<Option<Value>>) {
    let mut pipe: Option<File> = None;
    let mut activity: Option<Value> = None;
    let mut nonce = 0u64;
    loop {
        match rx.recv_timeout(REFRESH) {
            Ok(a) => activity = a,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Only the newest activity matters.
        if let Some(a) = rx.try_iter().last() {
            activity = a;
        }
        let Some(activity) = &activity else {
            // Switched off: clear what Discord shows, then close the pipe (never opened while off).
            if let Some(mut p) = pipe.take() {
                nonce += 1;
                let clear = json!({ "cmd": "SET_ACTIVITY", "args": { "pid": std::process::id() }, "nonce": nonce.to_string() });
                let _ = frame(&mut p, 1, &clear).and_then(|_| reply(&mut p));
            }
            continue;
        };
        if pipe.is_none() {
            pipe = connect();
        }
        let Some(p) = pipe.as_mut() else { continue };
        nonce += 1;
        let command = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": activity },
            "nonce": nonce.to_string(),
        });
        match frame(p, 1, &command).and_then(|_| reply(p)) {
            Ok(_) => std::thread::sleep(MIN_GAP),
            // Discord closed or restarted: reconnect on the next refresh.
            Err(_) => pipe = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 3.3b: Discord Status off clears the shown activity once and sends nothing more; On shows it again.
    #[test]
    fn discord_status_off_clears_and_stays_quiet() {
        let (tx, rx) = channel();
        let shown = status("University", Doing::Offline("Freeskating"));
        let mut menu = crate::graphics_menu::test_menu();
        menu.choose(crate::graphics_menu::Setting::DiscordStatus, 0);
        let mut app = App::new();
        app.insert_resource(Discord { tx, sent: Some(shown), since: 0, on: true })
            .insert_resource(menu)
            .add_systems(Update, switch);
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [None], "one clear, nothing else");
        assert!(!app.world().resource::<Discord>().on);
        app.world_mut().resource_mut::<crate::graphics_menu::Menu>().choose(crate::graphics_menu::Setting::DiscordStatus, 1);
        app.update();
        let d = app.world().resource::<Discord>();
        assert!(d.on && d.sent.is_none(), "on again: update sends the activity anew");
    }
    #[test]
    fn presence_shows_activity_map_and_party() {
        assert_eq!(status("New_San_Vanelona", Doing::Offline("Freeskating")),
                   Status { details: "Freeskating".into(), state: "New San Vanelona".into(), party: None });
        let online = |private| Doing::Online { mode: "Death Race", players: 4, capacity: 10, private };
        assert_eq!(status("University", online(false)).party, Some([4, 10]));
        let hidden = status("University", online(true));
        assert_eq!((hidden.state.as_str(), hidden.party), ("Private session", None), "a private session hides where and who");
        assert_eq!(challenge_text("ID_MAP_FILTER_CHALLENGETYPES_HOMBATTLE", false), "Hall of Meat");
        assert_eq!(challenge_text("ID_MAP_FILTER_CHALLENGETYPES_1UP", true), "1-Up");
        assert_eq!(challenge_text("ID_CROSSBAR_ONLINE_TEAM_CHALLENGE_MODE_TITLE", true), "Online Team Challenge");
    }
}
