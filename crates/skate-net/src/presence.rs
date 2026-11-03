//! Game <-> Steam presence helper messages (skate-steam-relay --presence). Bounded like directory.rs.
use serde::{Deserialize, Serialize};
pub const CONNECT_PREFIX: &str = "sk8:";
pub const MAX_FRIENDS: usize = 200;
pub const MAX_REPORT: usize = 60_000;
const MAX_CONNECT: usize = 255; // k_cchMaxRichPresenceValueLength (256) includes the NUL
pub const MAX_NAME: usize = 128;
/// A published skater (Friends' Skaters): one Steam Workshop item's metadata, k_cchDeveloperMetadataMax (5000)
/// with its NUL. The hard cap on both sides of the helper.
pub const MAX_SKATER: usize = 4999;
/// Every Steam friend for Friends' Skaters (Steam's own friend limit is 2000).
pub const MAX_ALL_FRIENDS: usize = 2000;
/// Friends per `SkaterFriends` datagram: 100 x (id + 128-byte name without control characters, JSON-escaped)
/// stays under MAX_REPORT.
pub const SKATER_PAGE: usize = 100;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Friend {
    pub id: u64,
    pub name: String,
    pub online: bool,
    pub connect: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Report {
    Me { id: u64, name: String },
    Friends(Vec<Friend>),
    JoinRequested { connect: String },
    Error(String),
    /// Friends' Skaters: every friend (offline too), `first` = index of the first row, sent in SKATER_PAGE pages.
    SkaterFriends {
        first: u32,
        total: u32,
        friends: Vec<(u64, String)>,
    },
    /// A friend's published skater (the metadata text), or None: nothing published, or the lookup failed.
    Skater { friend: u64, skater: Option<String> },
    /// Our skater is on Steam; `needs_agreement`: hidden from friends until the Workshop legal agreement is accepted.
    SkaterPublished { needs_agreement: bool },
    /// The players signed in to online on our build, us included (directory::PLAYERS lobbies).
    Players(u32),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Page {
    Profile(u64),
    Chat(u64),
    AddFriend(u64),
    /// The Steam Workshop legal agreement (our skater is hidden from friends until it is accepted).
    WorkshopAgreement,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Order {
    SetConnect(Option<String>),
    /// `connect`: the session's own (audit 10-24: a private session advertises none, so Send Invite did nothing);
    /// None: the advertised one.
    Invite {
        friend: u64,
        #[serde(default)]
        connect: Option<String>,
    },
    Open(Page),
    /// Friends' Skaters: send every friend and look up their skaters.
    SkaterList,
    /// Look this friend up next.
    FetchSkater { friend: u64 },
    /// Publish (or update) our skater: the metadata text, at most MAX_SKATER bytes.
    PublishSkater(String),
}

pub fn connect_string(lobby: u64, map: &str) -> String {
    let head = format!("{CONNECT_PREFIX}{lobby}:");
    format!(
        "{head}{}",
        crate::roster::truncate(map, MAX_CONNECT - head.len())
    )
}
/// A friend's rich presence is data they control: digits only for the lobby, a short map name without control
/// characters, never an address (3.9).
pub fn parse_connect(s: &str) -> Option<(u64, &str)> {
    if s.len() > MAX_CONNECT {
        return None;
    }
    let (lobby, map) = s.strip_prefix(CONNECT_PREFIX)?.split_once(':')?;
    if lobby.is_empty() || lobby.len() > 20 || !lobby.bytes().all(|b| b.is_ascii_digit()) || map.chars().any(char::is_control) {
        return None;
    }
    let lobby = lobby.parse().ok().filter(|&l| l != 0)?;
    Some((lobby, map))
}
pub fn url(page: &Page) -> String {
    match page {
        Page::Profile(id) => format!("steam://url/SteamIDPage/{id}"),
        Page::Chat(id) => format!("steam://friends/message/{id}"),
        Page::AddFriend(id) => format!("steam://friends/add/{id}"),
        Page::WorkshopAgreement => "steam://openurl/https://steamcommunity.com/sharedfiles/workshoplegalagreement".into(),
    }
}
pub fn report(text: &str) -> Option<Report> {
    if text.len() > MAX_REPORT {
        return None;
    }
    let r: Report = serde_json::from_str(text).ok()?;
    let ok = match &r {
        Report::Me { name, .. } => name.len() <= MAX_NAME,
        Report::Friends(f) => {
            f.len() <= MAX_FRIENDS
                && f.iter().all(|f| {
                    f.name.len() <= MAX_NAME
                        && f.connect
                            .as_deref()
                            .is_none_or(|c| c.len() <= MAX_CONNECT && parse_connect(c).is_some())
                })
        }
        Report::JoinRequested { connect } => connect.len() <= MAX_CONNECT,
        Report::Error(e) => e.len() <= 512,
        Report::SkaterFriends {
            first,
            total,
            friends,
        } => {
            friends.len() <= SKATER_PAGE
                && *total as usize <= MAX_ALL_FRIENDS
                && *first as usize + friends.len() <= *total as usize
                && friends.iter().all(|(_, n)| n.len() <= MAX_NAME)
        }
        Report::Skater { skater, .. } => skater.as_ref().is_none_or(|s| s.len() <= MAX_SKATER),
        Report::SkaterPublished { .. } | Report::Players(_) => true,
    };
    ok.then_some(r)
}
/// Longest order text: a PublishSkater whose metadata is all escaped characters.
pub const MAX_ORDER: usize = 6 * MAX_SKATER + 64;
pub fn order(text: &str) -> Option<Order> {
    if text.len() > MAX_ORDER {
        return None;
    }
    let o: Order = serde_json::from_str(text).ok()?;
    // NUL would panic the helper's CString::new.
    let connect_ok = |c: &Option<String>| c.as_deref().is_none_or(|c| c.len() <= MAX_CONNECT && !c.contains('\0') && parse_connect(c).is_some());
    let ok = match &o {
        Order::SetConnect(c) => connect_ok(c),
        Order::Invite { friend, connect } => *friend != 0 && connect_ok(connect),
        Order::Open(Page::Profile(id) | Page::Chat(id) | Page::AddFriend(id)) => *id != 0,
        Order::Open(Page::WorkshopAgreement) => true,
        Order::SkaterList => true,
        Order::FetchSkater { friend } => *friend != 0,
        // The helper hands it to Steam as a C string (a NUL would panic it).
        Order::PublishSkater(s) => s.len() <= MAX_SKATER && !s.contains('\0'),
    };
    ok.then_some(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connect_strings_round_trip_and_reject_foreign() {
        let c = connect_string(109775241012345678, "University");
        assert_eq!(parse_connect(&c), Some((109775241012345678, "University")));
        assert_eq!(parse_connect("+connect 1.2.3.4"), None);
        assert_eq!(parse_connect("sk8:0:x"), None, "lobby 0");
        for bad in ["sk8:+5:x", "sk8:-5:x", "sk8: 5:x", "sk8::x", "sk8:5", "sk8:99999999999999999999:x", "sk8:5:a\nb", "sk8:5:a\u{7}"] {
            assert_eq!(parse_connect(bad), None, "{bad:?}");
        }
        assert_eq!(parse_connect(&format!("sk8:5:{}", "m".repeat(300))), None, "over Steam's limit");
        assert_eq!(parse_connect("sk8:5:1.2.3.4:5678"), Some((5, "1.2.3.4:5678")), "a map name, never an address");
        assert!(connect_string(1, &"m".repeat(400)).len() <= 255);
        assert!(order(&crate::directory::encode(&Order::SetConnect(Some("sk8:1:a\0b".into())))).is_none());
    }
    #[test]
    fn reports_and_orders_are_bounded() {
        let many = Report::Friends(
            (0..=MAX_FRIENDS as u64)
                .map(|id| Friend {
                    id,
                    name: "n".into(),
                    online: true,
                    connect: None,
                })
                .collect(),
        );
        assert!(
            report(&crate::directory::encode(&many)).is_none(),
            "too many friends"
        );
        let me = Report::Me {
            id: 1,
            name: "Rich".into(),
        };
        assert_eq!(report(&crate::directory::encode(&me)), Some(me));
        assert!(
            order(&crate::directory::encode(&Order::SetConnect(Some(
                "http://x".into()
            ))))
            .is_none(),
            "not sk8:"
        );
        assert!(order(&crate::directory::encode(&Order::Invite { friend: 0, connect: None })).is_none());
        // Audit 10-24: an invite carries the session's connect; an older game's (no field) still reads.
        let invite = Order::Invite { friend: 7, connect: Some(connect_string(5, "University")) };
        assert_eq!(order(&crate::directory::encode(&invite)), Some(invite));
        assert_eq!(order(r#"{"Invite":{"friend":7}}"#), Some(Order::Invite { friend: 7, connect: None }));
        assert!(order(&crate::directory::encode(&Order::Invite { friend: 7, connect: Some("+connect 1.2.3.4".into()) })).is_none());
        assert_eq!(url(&Page::Profile(7)), "steam://url/SteamIDPage/7");
        assert_eq!(url(&Page::Chat(7)), "steam://friends/message/7");
        assert_eq!(url(&Page::AddFriend(7)), "steam://friends/add/7");
        let agreement = Order::Open(Page::WorkshopAgreement);
        assert_eq!(order(&crate::directory::encode(&agreement)), Some(agreement));
        assert_eq!(url(&Page::WorkshopAgreement), "steam://openurl/https://steamcommunity.com/sharedfiles/workshoplegalagreement");
    }
    #[test]
    fn skater_messages_are_bounded_and_keep_any_script() {
        let names = ["Zoë", "Влад", "スケーター", "🛹 Shredder", "a\"b\\c"];
        let page = Report::SkaterFriends {
            first: 0,
            total: names.len() as u32,
            friends: names.iter().enumerate().map(|(i, n)| (i as u64 + 1, n.to_string())).collect(),
        };
        assert_eq!(report(&crate::directory::encode(&page)), Some(page), "names round-trip");
        // A full page of the longest names, every byte escaped (the helper drops control characters), fits one report.
        let worst = Report::SkaterFriends {
            first: 0,
            total: SKATER_PAGE as u32,
            friends: (0..SKATER_PAGE as u64).map(|i| (u64::MAX - i, "\"".repeat(MAX_NAME))).collect(),
        };
        assert!(report(&crate::directory::encode(&worst)).is_some());
        let rows = |n: usize, first: u32, total: u32| Report::SkaterFriends {
            first,
            total,
            friends: (0..n as u64).map(|i| (i, "x".into())).collect(),
        };
        assert!(report(&crate::directory::encode(&rows(SKATER_PAGE + 1, 0, 500))).is_none(), "page too big");
        assert!(report(&crate::directory::encode(&rows(10, 95, 100))).is_none(), "past total");
        assert!(report(&crate::directory::encode(&rows(0, 0, MAX_ALL_FRIENDS as u32 + 1))).is_none());
        let skater = |n: usize| Report::Skater { friend: 7, skater: Some("{".repeat(n)) };
        assert!(report(&crate::directory::encode(&skater(MAX_SKATER))).is_some());
        assert!(report(&crate::directory::encode(&skater(MAX_SKATER + 1))).is_none(), "size cap");
        let publish = |s: String| order(&crate::directory::encode(&Order::PublishSkater(s)));
        assert!(publish("\u{1}".repeat(MAX_SKATER)).is_some(), "worst escaping fits MAX_ORDER");
        assert!(publish("x".repeat(MAX_SKATER + 1)).is_none());
        assert!(publish("a\0b".into()).is_none(), "NUL");
        assert!(order(&crate::directory::encode(&Order::FetchSkater { friend: 0 })).is_none());
        assert_eq!(order(&crate::directory::encode(&Order::SkaterList)), Some(Order::SkaterList));
    }
}
