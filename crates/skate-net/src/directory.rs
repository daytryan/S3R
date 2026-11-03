//! Bounded control messages between the game and the optional discovery helper.
use serde::{Deserialize, Serialize};
pub const NAMESPACE: &str = "skate3rust-free-skate-v7";
// Spacewar is shared with other games; ours carry this namespace (any version).
pub fn is_game_lobby(namespace: &str) -> bool {
    namespace.starts_with("skate3rust-free-skate-v")
}
/// The lobby key holding the host's exact game build. Browses (Quick Match too) filter on it and a join (an invite
/// too) checks it, so different builds never share a lobby.
pub const BUILD_KEY: &str = "sk8build";
/// The lobby key holding the host's ping location (Steam's text), from which a browser estimates its ping to the host.
pub const PING_KEY: &str = "ping";
/// The `sk8game` value of the one-player lobbies that count the players online on a build (the presence helper's):
/// not [`is_game_lobby`], so no browse lists them.
pub const PLAYERS: &str = "skate3rust-players-v1";
/// Not retail (user 10-25): the lobby keys a host's Host Actions set for the server browser: the session's name and
/// description, and "1" when it is left out of the browser (Quick Match and the Online Challenge Map still find it).
pub const NAME_KEY: &str = "name";
pub const DESCRIPTION_KEY: &str = "desc";
pub const UNLISTED_KEY: &str = "unlisted";
/// A session's name and description at most: (characters, bytes), as [`bounded`] keeps them.
pub const NAME: (usize, usize) = (24, 48);
pub const DESCRIPTION: (usize, usize) = (60, 120);
/// The longest control message the relay answers with (a page of the longest rows fits: tests).
pub const RESPONSE_MAX: usize = 4096;
/// The relay's last status when Steam itself went away (the presence helper reports that to the player too).
pub const STEAM_CLOSED: &str = "ERROR Steam closed. Open Steam, sign in, and retry.";
pub const PAGE_SIZE: usize = 5;
/// The most sessions one browse asks Steam for (user 10-27: 100; Steam answers 50 unless asked for more).
pub const BROWSE_MAX: usize = 100;
/// Online Challenge Map lobby keys (`tab`, `filter`, `challenge`; namespace v6+). A host
/// publishes them; a tagged browse matches each non-empty one exactly (empty = any).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tags {
    pub tab: String,
    pub filter: String,
    pub challenge: String,
    /// A session move's token: the moved session's players find their host's new lobby by it.
    #[serde(default)]
    pub session: String,
    /// P5: a private session's moved lobby. The relay lists it only to a browse carrying its `session` token, so
    /// strangers' browses and Quick Matches never find it while it waits public for its guests.
    #[serde(default)]
    pub hidden: bool,
}
/// The lobby key a hidden lobby carries ("1").
pub const HIDDEN_KEY: &str = "hidden";
/// Whether a lobby with these `hidden` / `session` lobby values is listed to a browse for `wanted` (its session tag).
pub fn listed(hidden: Option<&str>, session: Option<&str>, wanted: &str) -> bool {
    hidden != Some("1") || (!wanted.is_empty() && session == Some(wanted))
}
impl Tags {
    pub fn is_empty(&self) -> bool {
        self.keys().iter().all(|(_, v)| v.is_empty())
    }
    pub fn keys(&self) -> [(&'static str, &str); 4] {
        [("tab", &self.tab), ("filter", &self.filter), ("challenge", &self.challenge), ("session", &self.session)]
    }
    /// Steam lobby size: freeskate (and untagged) lobbies hold the protocol ceiling; a challenge what its Start
    /// Challenge allows (skate-game match_core::row_cap): retail's `max_players`, 6 (disc 0x82da3bf0), and 4 for a
    /// Skate 1 / 2 S.K.A.T.E.
    pub fn capacity(&self) -> usize {
        match (self.tab.as_str(), self.filter.as_str()) {
            ("" | "freeskate", _) => crate::lobby::MAX_PLAYERS,
            ("skate1" | "skate2", "ID_MAP_FILTER_CHALLENGETYPES_SKATE") => 4,
            _ => 6,
        }
    }
    fn valid(&self) -> bool {
        self.keys().iter().all(|(_, v)| v.len() <= 64 && !v.contains('\0'))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub command: Command,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    Browse {
        page: usize,
        map: u64,
        physics: u64,
        /// With non-empty `tags`: only v6 lobbies on the map labelled `label` whose tags match.
        #[serde(default)]
        label: String,
        #[serde(default)]
        tags: Tags,
    },
    Host {
        map: u64,
        physics: u64,
        label: String,
        #[serde(default)]
        tags: Tags,
    },
    Join {
        lobby: u64,
        map: u64,
        physics: u64,
    },
    Privacy {
        private: bool,
    },
    Owner {
        member: u64,
    },
}
/// Not retail (user 10-25): a host's server browser listing for the lobby it owns (Host Actions). The game sends it
/// unanswered (`DETAILS`), so it never waits behind a browse nor holds one, a privacy change or a join up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Details {
    pub name: String,
    pub description: String,
    /// In the server browser (a private session never is, whatever this says).
    pub listed: bool,
}
impl Default for Details {
    fn default() -> Self {
        Self { name: String::new(), description: String::new(), listed: true }
    }
}
pub fn details(text: &str) -> Option<Details> {
    if text.len() > 1200 {
        return None;
    }
    let d: Details = serde_json::from_str(text).ok()?;
    let fits = |t: &str, (chars, _): (usize, usize)| t.chars().count() <= chars && !t.contains('\0');
    (fits(&d.name, NAME) && fits(&d.description, DESCRIPTION)).then_some(d)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Row {
    pub id: u64,
    pub map: String,
    pub players: usize,
    pub capacity: usize,
    pub compatible: bool,
    /// Steam's round-trip estimate to the host in ms ([`PING_KEY`]); none until both ends have measured.
    #[serde(default)]
    pub ping: Option<u32>,
    /// The lobby's challenge-type filter (a string id) or else its tab; empty for none.
    #[serde(default)]
    pub mode: String,
    /// The host's name and description for the session ([`NAME_KEY`], [`DESCRIPTION_KEY`]); empty for none.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Event {
    Rows {
        page: usize,
        total: usize,
        rows: Vec<Row>,
    },
    Owner {
        lobby: u64,
        owner: u64,
        own: u64,
    },
    Error(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub request: u64,
    pub event: Event,
}
pub fn encode<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("Control serialization")
}
pub fn request(text: &str) -> Option<Request> {
    if text.len() > 1200 {
        return None;
    }
    let r: Request = serde_json::from_str(text).ok()?;
    match &r.command {
        Command::Browse { page, label, tags, .. }
            if *page > 100 || label.len() > 64 || label.contains('\0') || !tags.valid() =>
        {
            return None
        }
        Command::Host { label, tags, .. }
            if label.len() > 64 || label.contains('\0') || !tags.valid() =>
        {
            return None
        }
        Command::Owner { member } if *member == 0 => return None,
        _ => (),
    };
    Some(r)
}
pub fn response(text: &str) -> Option<Response> {
    if text.len() > RESPONSE_MAX {
        return None;
    }
    let r: Response = serde_json::from_str(text).ok()?;
    match &r.event {
        Event::Rows { rows, .. }
            if rows.len() > PAGE_SIZE
                || rows.iter().any(|r| {
                    r.map.len() > 64
                        || r.mode.len() > 64
                        || r.name.len() > NAME.1
                        || r.description.len() > DESCRIPTION.1
                        || r.capacity > crate::lobby::MAX_PLAYERS
                        || r.players > crate::lobby::MAX_PLAYERS
                }) =>
        {
            return None;
        }
        Event::Owner { lobby, owner, own } if *lobby == 0 || *owner == 0 || *own == 0 => {
            return None;
        }
        _ => (),
    };
    Some(r)
}

/// Keep labels readable and bounded in bytes, including Unicode map names. A stranger's lobby label is shown in the
/// browser, so invisible and direction-changing characters go too (online safety Q13).
pub fn label(text: &str) -> String {
    bounded(text, (32, 48))
}
/// [`label`]'s cleaning, keeping at most (characters, bytes) of `text`.
pub fn bounded(text: &str, (chars, bytes): (usize, usize)) -> String {
    let mut result = String::new();
    let shown = |c: &char| !c.is_control() && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}');
    for c in text.chars().filter(shown).take(chars) {
        if result.len() + c.len_utf8() > bytes {
            break;
        }
        result.push(c);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Q13: a stranger's lobby label can't hide text or flip it right to left in the browser.
    #[test]
    fn lobby_labels_drop_invisible_and_bidi_characters() {
        assert_eq!(label("San\u{202E}Van\u{200B}\u{2066}\u{FEFF}\u{7}elona"), "SanVanelona");
        assert_eq!(label("Zoë's Park"), "Zoë's Park");
    }
    #[test]
    fn unicode_labels_and_full_page_fit_control_datagram() {
        let name = label(&"\u{1f600}".repeat(80));
        assert!(name.len() <= 48);
        // A full 50-player freeskate lobby is a valid row (a 10 cap once dropped the whole page), and the longest one
        // still fits: a label, name and description of quotes (escaped to twice their length), the longest mode and ping.
        let quotes = label(&"\"".repeat(80));
        let (name, description) = (bounded(&"\"".repeat(80), NAME), bounded(&"\"".repeat(200), DESCRIPTION));
        assert_eq!((name.len(), description.len()), (NAME.0, DESCRIPTION.0));
        let rows = (u64::MAX - 4..=u64::MAX)
            .map(|id| Row {
                id,
                map: quotes.clone(),
                players: crate::lobby::MAX_PLAYERS,
                capacity: crate::lobby::MAX_PLAYERS,
                compatible: true,
                ping: Some(u32::MAX),
                mode: "M".repeat(64),
                name: name.clone(),
                description: description.clone(),
            })
            .collect();
        assert!(
            response(&encode(&Response {
                request: 1,
                event: Event::Rows {
                    page: 0,
                    total: 5,
                    rows
                }
            }))
            .is_some()
        );
        assert!(
            request(&encode(&Request {
                id: 1,
                command: Command::Host {
                    map: 1,
                    physics: 2,
                    label: name,
                    tags: Tags::default()
                }
            }))
            .is_some()
        );
    }
    #[test]
    fn rejects_invalid_owners_and_unbounded_requests() {
        assert!(
            response(&encode(&Response {
                request: 0,
                event: Event::Owner {
                    lobby: 1,
                    owner: 0,
                    own: 2
                }
            }))
            .is_none()
        );
        assert!(
            request(&encode(&Request {
                id: 1,
                command: Command::Browse {
                    page: 101,
                    map: 1,
                    physics: 2,
                    label: String::new(),
                    tags: Tags::default()
                }
            }))
            .is_none()
        );
        assert!(request(&"x".repeat(1201)).is_none());
    }
    #[test]
    fn lobby_tags_are_bounded_and_optional() {
        // A pre-v6 request (no label/tags) still parses, untagged.
        let old = request(r#"{"id":1,"command":{"Browse":{"page":0,"map":1,"physics":2}}}"#).unwrap();
        assert!(matches!(old.command, Command::Browse { tags, .. } if tags.is_empty()));
        let host = |challenge: String| {
            request(&encode(&Request {
                id: 1,
                command: Command::Host {
                    map: 1,
                    physics: 2,
                    label: "University".into(),
                    tags: Tags { tab: "versus".into(), challenge, ..Tags::default() },
                },
            }))
        };
        // Tags are their own keys, not squeezed into the 32-char label.
        assert!(host("freeskate/online_freeskate_team_join".into()).is_some());
        assert!(host("x".repeat(65)).is_none());
        assert!(host("a\0b".into()).is_none());
    }
    #[test]
    fn hidden_lobbies_list_only_to_their_token() {
        let old = request(r#"{"id":1,"command":{"Host":{"map":1,"physics":2,"label":"x","tags":{"tab":"a","filter":"","challenge":""}}}}"#).unwrap();
        assert!(matches!(old.command, Command::Host { tags, .. } if !tags.hidden), "an older game's host is not hidden");
        assert!(listed(None, None, ""), "an ordinary lobby");
        assert!(listed(None, Some("ab"), ""), "a public session's move stays findable");
        assert!(!listed(Some("1"), Some("ab"), ""), "a stranger's browse or Quick Match");
        assert!(!listed(Some("1"), Some("ab"), "cd"), "another move's token");
        assert!(!listed(Some("1"), None, ""));
        assert!(listed(Some("1"), Some("ab"), "ab"), "its own guests");
    }
    /// User 10-25: a host's name and description are bounded where they are sent and where they come back.
    #[test]
    fn session_details_are_bounded() {
        let details = |name: &str, description: &str| details(&encode(&Details { name: name.into(), description: description.into(), listed: false }));
        assert!(details("Ryan's Park", "Chill freeskate, all welcome").is_some());
        assert!(details(&"\u{1f600}".repeat(NAME.0), "").is_some(), "characters, not bytes");
        assert!(details(&"n".repeat(NAME.0 + 1), "").is_none());
        assert!(details("", &"d".repeat(DESCRIPTION.0 + 1)).is_none());
        assert!(details("a\0b", "").is_none());
        assert!(super::details(r#"{"name":"x"}"#).is_none(), "all three fields");
        assert!(Details::default().listed, "a session is listed until its host hides it");
        assert_eq!(bounded("ab\u{202E}cd\u{7}ef", (4, 48)), "abcd");
        assert_eq!(bounded(&"\u{e9}".repeat(10), (24, 9)), "\u{e9}".repeat(4), "never past the byte bound, never mid-character");
        let row = |name: String| Row {
            id: 1,
            map: "m".into(),
            players: 1,
            capacity: 6,
            compatible: true,
            ping: None,
            mode: String::new(),
            name,
            description: String::new(),
        };
        let rows = |name| encode(&Response { request: 1, event: Event::Rows { page: 0, total: 1, rows: vec![row(name)] } });
        assert!(response(&rows("n".repeat(NAME.1))).is_some());
        assert!(response(&rows("n".repeat(NAME.1 + 1))).is_none());
    }
    #[test]
    fn privacy_and_owner_requests_validate() {
        let ok = encode(&Request {
            id: 1,
            command: Command::Privacy { private: true },
        });
        assert!(request(&ok).is_some());
        assert!(
            request(&encode(&Request {
                id: 2,
                command: Command::Owner { member: 0 }
            }))
            .is_none()
        );
        assert!(
            request(&encode(&Request {
                id: 3,
                command: Command::Owner {
                    member: 76561198000000000
                }
            }))
            .is_some()
        );
    }
}
