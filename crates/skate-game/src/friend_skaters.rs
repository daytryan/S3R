//! Friends' Skaters over Steam (decisions Q2/Q2a): the real [`FriendSkaters`] behind Import Skater, and the
//! publisher of our own boss look. Both go through the `skate-steam-relay --presence` helper (presence.rs): the
//! helper keeps ONE friends-only Steam Workshop item per player with the skater as its metadata (ruling in
//! .superpowers/sdd/2026-10-05-team-select/steam-report.md), so offline friends' skaters are listed too.
//!
//! Published text: `{"name": <boss name>, "look": <character.json through sanitize_friend>}`, at most MAX_SKATER
//! bytes. Downloaded text gets the same cap and [`sanitize_friend`] before the UI sees it; anything else is "None".
use crate::team_select::{FriendRow, FriendSkaters, FriendSource, FriendState, Friends, sanitize_friend};
use bevy::prelude::*;
use serde_json::{Value, json};
use skate_net::presence::{MAX_SKATER, Order, Report};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A friend not answered by then is "None" (greyed); the helper's own lookup timeout is 15 s.
const TIMEOUT: Duration = Duration::from_secs(20);
/// While the list is open it is asked again this often (the helper caches answers as long).
const RELIST: Duration = Duration::from_secs(60);
/// How often the boss look and name are checked for a change to publish.
const PUBLISH_EVERY: Duration = Duration::from_secs(5);

/// A friend's skater, downloaded and sanitized.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Skater {
    /// Their boss's name (team_data::NAME_MAX characters, no control characters).
    pub name: String,
    /// The sanitized customiser profile JSON.
    pub look: Vec<u8>,
}

fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(crate::team_data::NAME_MAX).collect()
}

/// Our skater as published: None when the look is not a skater or does not fit one Workshop item's metadata.
pub(crate) fn encode(name: &str, look: &[u8]) -> Option<String> {
    let look = sanitize_friend(look).ok()?;
    let text = json!({ "name": clean_name(name), "look": look }).to_string();
    (text.len() <= MAX_SKATER).then_some(text)
}

/// A friend's published text through the trust boundary: the size cap, then the network-appearance sanitizer.
pub(crate) fn decode(text: &str) -> Option<Skater> {
    if text.len() > MAX_SKATER {
        return None;
    }
    let envelope: Value = serde_json::from_str(text).ok()?;
    let look = sanitize_friend(&serde_json::to_vec(envelope.get("look")?).ok()?).ok()?;
    let name = envelope.get("name").and_then(Value::as_str).unwrap_or_default();
    Some(Skater { name: clean_name(name), look: serde_json::to_vec(&look).ok()? })
}

/// The Steam source: fed by the helper's reports, read by Import Skater.
#[derive(Default)]
pub(crate) struct SteamFriendSkaters {
    /// Every friend (offline too), as the helper listed them.
    friends: Vec<(u64, String)>,
    /// A friend list arriving in pages.
    incoming: Vec<(u64, String)>,
    /// Answered friends: their skater, or None (nothing published, refused, or the lookup failed).
    skaters: HashMap<u64, Option<Skater>>,
    /// When the list was last asked for.
    asked: Option<Instant>,
    /// When Import Skater last read the list (it reads every frame while open).
    listed: Cell<Option<Instant>>,
    /// Friends to look up first (the hilighted row), once each per listing.
    wanted: Vec<u64>,
    prioritized: HashSet<u64>,
    /// Our item is hidden from friends until the Steam Workshop legal agreement is accepted.
    pub needs_agreement: bool,
}

impl SteamFriendSkaters {
    pub(crate) fn report(&mut self, report: Report) {
        match report {
            Report::SkaterFriends { first, total, friends } => {
                if first == 0 {
                    self.incoming.clear();
                }
                if first as usize != self.incoming.len() {
                    self.incoming.clear();
                    return;
                }
                self.incoming.extend(friends);
                if self.incoming.len() >= total as usize {
                    self.friends = std::mem::take(&mut self.incoming);
                }
            }
            Report::Skater { friend, skater } => {
                let skater = skater.as_deref().and_then(|text| {
                    let decoded = decode(text);
                    if decoded.is_none() {
                        warn!("A friend's skater was refused (malformed or oversized)");
                    }
                    decoded
                });
                self.skaters.insert(friend, skater);
            }
            Report::SkaterPublished { needs_agreement } => {
                if needs_agreement && !self.needs_agreement {
                    warn!("Your skater is hidden from friends until you accept the Steam Workshop legal agreement");
                }
                self.needs_agreement = needs_agreement;
            }
            _ => {}
        }
    }
    /// A friend's skater with its name (the trait gives the UI only the look).
    #[allow(dead_code)] // ponytail: for Import Skater if it takes the friend's skater name; unused until then.
    pub(crate) fn skater(&self, id: u64) -> Option<&Skater> {
        self.skaters.get(&id)?.as_ref()
    }
}

impl FriendSkaters for SteamFriendSkaters {
    fn list(&self) -> Vec<FriendRow> {
        let now = Instant::now();
        self.listed.set(Some(now));
        // Past RELIST a new listing is about to go out: show "looking up" rather than greying everyone.
        let timed_out = self.asked.is_some_and(|at| (TIMEOUT..RELIST).contains(&now.duration_since(at)));
        self.friends
            .iter()
            .map(|(id, name)| FriendRow {
                name: name.clone(),
                id: *id,
                state: match self.skaters.get(id) {
                    Some(Some(_)) => FriendState::Ready,
                    Some(None) => FriendState::None,
                    None if timed_out => FriendState::None,
                    None => FriendState::Loading,
                },
            })
            .collect()
    }
    fn fetch(&mut self, id: u64) -> Option<Vec<u8>> {
        match self.skaters.get(&id) {
            Some(skater) => skater.as_ref().map(|s| s.look.clone()),
            None => {
                if self.prioritized.insert(id) {
                    self.wanted.push(id);
                }
                None
            }
        }
    }
    fn needs_agreement(&self) -> bool {
        self.needs_agreement
    }
}

#[derive(Resource)]
struct SteamSkaters(Arc<Mutex<SteamFriendSkaters>>);

#[derive(Default)]
struct Publisher {
    checked: Option<Instant>,
    /// What the running helper was last given.
    sent: Option<String>,
    warned: bool,
}

/// Our skater to publish: the boss look (settings/character.json) and name (settings/team.json).
fn own_skater(settings: &Path) -> Option<Result<String, ()>> {
    let look = crate::saves::read(&crate::team_data::look_path(settings, 0)).ok()?;
    let team: Option<Value> = crate::saves::read(&crate::team_data::team_path(settings)).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let name = team.as_ref().and_then(|t| t["boss"].as_str()).unwrap_or_default();
    Some(encode(name, &look).ok_or(()))
}

fn sync(
    mut presence: ResMut<crate::presence::Presence>,
    source: Res<SteamSkaters>,
    config: Res<crate::config::Config>,
    mut publisher: Local<Publisher>,
) {
    let now = Instant::now();
    let Ok(mut s) = source.0.lock() else { return };
    for report in std::mem::take(&mut presence.skater_reports) {
        s.report(report);
    }
    let open = s.listed.get().is_some_and(|at| now.duration_since(at) < Duration::from_secs(1));
    if open {
        // The list is what asked for Steam: the helper starts now (3.3), and stops a while after it closes.
        presence.hold();
    }
    if !presence.available {
        // Steam or the helper went away: no friends; a restarted helper gets everything again.
        s.friends.clear();
        s.asked = None;
        publisher.sent = None;
        return;
    }
    if open && s.asked.is_none_or(|at| now.duration_since(at) >= RELIST) && presence.order(Order::SkaterList) {
        s.asked = Some(now);
        s.prioritized.clear();
    }
    for friend in std::mem::take(&mut s.wanted) {
        presence.order(Order::FetchSkater { friend });
    }
    drop(s);
    if publisher.checked.is_some_and(|at| now.duration_since(at) < PUBLISH_EVERY) {
        return;
    }
    publisher.checked = Some(now);
    let settings = crate::saves::settings_dir(&config.asset_root);  // Not retail (user 10-13)
    match own_skater(&settings) {
        Some(Ok(text)) => {
            // The helper compares with what Steam holds, so a restart does not re-upload an unchanged skater.
            if publisher.sent.as_ref() != Some(&text) && presence.order(Order::PublishSkater(text.clone())) {
                publisher.sent = Some(text);
            }
        }
        Some(Err(())) if !publisher.warned => {
            publisher.warned = true;
            warn!("Your skater is too large to share with Steam friends");
        }
        _ => {}
    }
}

/// Replaces Import Skater's default (no friends) with the Steam source and publishes our skater.
pub(crate) struct FriendSkatersPlugin;
impl Plugin for FriendSkatersPlugin {
    fn build(&self, app: &mut App) {
        let source = Arc::new(Mutex::new(SteamFriendSkaters::default()));
        let shared: Friends = source.clone();
        app.insert_resource(FriendSource(shared)).insert_resource(SteamSkaters(source)).add_systems(Update, sync);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_select::sorted_friends;
    use skate_net::directory::encode as wire;

    const LOOK: &str = r#"{"gender":"female","selections":{"Hair":{"asset_id":"a","material_id":"b"}},"morphs":{}}"#;

    /// Through the real protocol text, as the helper's datagrams arrive.
    fn feed(s: &mut SteamFriendSkaters, report: Report) {
        let text = wire(&report);
        s.report(skate_net::presence::report(&text).expect("protocol refused a report"));
    }
    fn friends(names: &[&str]) -> Report {
        Report::SkaterFriends {
            first: 0,
            total: names.len() as u32,
            friends: names.iter().enumerate().map(|(i, n)| (i as u64 + 1, n.to_string())).collect(),
        }
    }

    #[test]
    fn every_friend_listed_sorted_with_any_script() {
        let mut s = SteamFriendSkaters::default();
        let names = ["zed", "Ännchen", "anna", "Влад", "スケーター", "🛹 Shredder", "Bob"];
        feed(&mut s, friends(&names));
        // Nobody here is online; offline friends are listed all the same.
        let sorted: Vec<String> = sorted_friends(s.list()).into_iter().map(|f| f.name).collect();
        assert_eq!(sorted, ["anna", "Ännchen", "Bob", "zed", "Влад", "スケーター", "🛹 Shredder"]);
    }

    #[test]
    fn pages_assemble_and_a_lost_page_waits_for_the_next_listing() {
        let mut s = SteamFriendSkaters::default();
        let page = |first: u32, ids: std::ops::Range<u64>| Report::SkaterFriends {
            first,
            total: 150,
            friends: ids.map(|i| (i, format!("f{i}"))).collect(),
        };
        feed(&mut s, page(0, 0..100));
        assert!(s.list().is_empty(), "nothing until the last page");
        feed(&mut s, page(100, 100..150));
        assert_eq!(s.list().len(), 150);
        // A gap (a lost datagram) keeps the previous list.
        feed(&mut s, page(0, 0..100));
        s.report(Report::SkaterFriends { first: 120, total: 150, friends: vec![(1, "x".into())] });
        assert_eq!(s.list().len(), 150);
    }

    #[test]
    fn states_ready_none_loading_and_timeout() {
        let mut s = SteamFriendSkaters::default();
        feed(&mut s, friends(&["has", "none", "slow", "bad"]));
        s.asked = Some(Instant::now());
        let skater = encode("Влад 🛹", LOOK.as_bytes()).unwrap();
        feed(&mut s, Report::Skater { friend: 1, skater: Some(skater) });
        feed(&mut s, Report::Skater { friend: 2, skater: None });
        feed(&mut s, Report::Skater { friend: 4, skater: Some("{not json".into()) });
        let state = |s: &SteamFriendSkaters, id: u64| s.list().into_iter().find(|f| f.id == id).unwrap().state;
        assert_eq!(state(&s, 1), FriendState::Ready);
        assert_eq!(state(&s, 2), FriendState::None);
        assert_eq!(state(&s, 3), FriendState::Loading);
        assert_eq!(state(&s, 4), FriendState::None, "malformed is None");
        s.asked = Some(Instant::now() - TIMEOUT);
        assert_eq!(state(&s, 3), FriendState::None, "timed out");
        s.asked = Some(Instant::now() - RELIST);
        assert_eq!(state(&s, 3), FriendState::Loading, "a new listing is going out");
        // The friend's look and name survive the trip.
        assert_eq!(s.skater(1).unwrap().name, "Влад 🛹");
        let look: Value = serde_json::from_slice(&s.fetch(1).unwrap()).unwrap();
        assert_eq!(look["selections"]["Hair"]["asset_id"], "a");
        assert_eq!(s.fetch(2), None);
    }

    #[test]
    fn the_size_cap_and_the_sanitizer_hold() {
        let ok = encode("Me", LOOK.as_bytes()).unwrap();
        assert!(decode(&ok).is_some());
        // Exactly at the cap passes; one byte over is refused before parsing.
        let pad = |n: usize| format!("{ok:<n$}");
        assert!(decode(&pad(MAX_SKATER)).is_some());
        assert!(decode(&pad(MAX_SKATER + 1)).is_none());
        // Not a skater, no look, malformed.
        assert!(decode(r#"{"name":"x","look":[1,2]}"#).is_none());
        assert!(decode(r#"{"name":"x"}"#).is_none());
        assert!(decode("{").is_none());
        // Model and image references are dropped (custom models never travel), creator designs too.
        let sneaky = r#"{"name":"x","look":{"gender":"male","native":"C:/evil.glb","image":"x.png",
            "graphics":{"front":{"design":"abc"}}}}"#;
        let skater = decode(sneaky).unwrap();
        let look: Value = serde_json::from_slice(&skater.look).unwrap();
        assert!(look.get("native").is_none() && look.get("image").is_none());
        assert!(look["graphics"].get("front").is_none(), "designs dropped: {look}");
        // Names: control characters dropped, 16 characters kept, any script.
        let named = decode(&encode("a\u{7}bcdefghijklmnopqrstuvwxyz", LOOK.as_bytes()).unwrap()).unwrap();
        assert_eq!(named.name, "abcdefghijklmnop");
        // Our own oversized skater is not published.
        let worn: serde_json::Map<String, Value> =
            (0..100).map(|i| (format!("slot{i}"), json!({"asset_id": "0123456789abcdef", "material_id": "0123456789abcdef"}))).collect();
        let huge = json!({"gender": "male", "selections": worn}).to_string();
        assert!(sanitize_friend(huge.as_bytes()).is_ok(), "a valid look");
        assert!(encode("Me", huge.as_bytes()).is_none());
    }

    #[test]
    fn the_hilighted_friend_is_asked_for_once() {
        let mut s = SteamFriendSkaters::default();
        feed(&mut s, friends(&["a"]));
        assert_eq!(s.fetch(1), None);
        assert_eq!(s.fetch(1), None);
        assert_eq!(s.wanted, [1]);
    }

    #[test]
    fn own_skater_reads_the_boss_look_and_name() {
        let settings = crate::team_data::tests::dir("friend-skaters");
        assert!(own_skater(&settings).is_none(), "no boss look: nothing published");
        std::fs::write(settings.join("character.json"), LOOK).unwrap();
        std::fs::write(settings.join("team.json"), r#"{"version":1,"name":"T","logo":"0","boss":"Zoë","teammates":[]}"#).unwrap();
        let text = own_skater(&settings).unwrap().unwrap();
        let skater = decode(&text).unwrap();
        assert_eq!(skater.name, "Zoë");
        assert_eq!(own_skater(&settings).unwrap().unwrap(), text, "same files, same text (the helper dedupes)");
        let _ = std::fs::remove_dir_all(settings);
    }
}
